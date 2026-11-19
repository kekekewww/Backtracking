use super::*;

// The test copy owns the packet clock. Nothing is sent to a playback endpoint.
pub(super) fn run(
    spec: &str,
    commands: mpsc::Receiver<Command>,
    startup: mpsc::SyncSender<Result<Started, String>>,
    wake: DataWake,
) {
    let config = match spec.split_once(':') {
        Some((layout, activity @ ("all" | "stereo"))) => match layout {
            "5.1" => Some((6, 0x60f, activity)),
            "7.1" => Some((8, 0x63f, activity)),
            "7.1.4" => Some((12, 0x2d63f, activity)),
            _ => None,
        },
        _ => None,
    };
    let Some((width, mask, activity)) = config else {
        let _ = startup.send(Err(format!("Unknown synthetic format {spec}")));
        return;
    };
    let mut wave = float_wave(width, mask);
    let format = Arc::new(unsafe { Format::parse(NonNull::from(&mut wave).cast()) }.unwrap());
    let packets = TimedAudioQueue::new(48_000, PACKET_QUEUE_CAPACITY, format.channels);
    if startup
        .send(Ok(Started {
            format: format.clone(),
            packets: packets.clone(),
        }))
        .is_err()
    {
        return;
    }
    let mut quiet = QuietModeState::default();
    let mut running = true;
    let mut time = now_ns();
    let mut index = 0u64;
    let mut decoded = vec![0.; 480 * format.channels];
    let mut bytes = Vec::with_capacity(decoded.len() * 4);
    loop {
        let _ = quiet.poll_release();
        match commands.recv_timeout(Duration::from_nanos(
            time.saturating_add(10_000_000)
                .saturating_sub(now_ns())
                .min(10_000_000),
        )) {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Command::Pause(reply)) => {
                running = false;
                let _ = reply.send(Ok(()));
            }
            Ok(Command::Resume(reply)) => {
                running = true;
                time = now_ns();
                let _ = reply.send(Ok(()));
            }
            Ok(Command::PrepareRetainSilence(enabled, reply, release)) => {
                quiet.prepare(enabled, reply, release)
            }
            Err(mpsc::RecvTimeoutError::Timeout) if running => {
                bytes.clear();
                for f in index..index + 480 {
                    for ch in 0..width {
                        let sample = if activity == "all" || ch < 2 {
                            (std::f64::consts::TAU * (f % 48000) as f64 * f64::from(220 + 30 * ch)
                                / 48000.)
                                .sin() as f32
                                * 0.25
                        } else {
                            0.
                        };
                        bytes.extend_from_slice(&sample.to_le_bytes());
                    }
                }
                format.decode(&bytes, &mut decoded);
                packets.push_interleaved_with_discontinuity(time, 48000, &decoded, false);
                index += 480;
                time += 10_000_000;
                wake();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => time = now_ns(),
        }
    }
}

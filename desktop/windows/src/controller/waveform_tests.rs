use super::*;
use backtracking_core::SenderEndpoint;
use std::sync::atomic::AtomicUsize;

#[test]
fn cold_channel_waveforms_leave_buffers_untouched_until_ready() {
    let controller = Controller::new().unwrap();
    {
        let mut state = lock(&controller.state);
        let mut capture =
            SynchronizedCapture::new(8_000, 12, None, DurationChoice::Seconds20).unwrap();
        capture.set_retain_silence(true);
        capture
            .push_native_interleaved(0, &vec![0.25; 8_000 * 12], false)
            .unwrap();
        state.capture = Some(capture);
        state.shape = Some((8_000, 12, None));
        state.capture_lanes = [Some(0), None];
    }
    let mut low = [7.0; 32];
    let mut high = [8.0; 32];
    assert!(
        controller
            .try_render_channel_waveforms(
                &[(0, 11)],
                &[],
                None,
                &mut [(&mut low, &mut high)],
                &mut []
            )
            .is_none()
    );
    assert_eq!(low, [7.0; 32]);
    assert_eq!(high, [8.0; 32]);
}

fn mixed_waveform_controller() -> Controller {
    let controller = Controller::new().unwrap();
    let mut state = lock(&controller.state);
    let mut capture =
        SynchronizedCapture::new(8_000, 4, Some(1), DurationChoice::Seconds20).unwrap();
    capture.set_retain_silence(true);
    let audio: Vec<_> = (0..8_000)
        .flat_map(|_| [0.1, 0.2, 0.3, 0.4, -0.5])
        .collect();
    capture.push_native_interleaved(0, &audio, false).unwrap();
    state.capture = Some(capture);
    state.shape = Some((8_000, 4, Some(1)));
    state.capture_lanes = [Some(0), Some(1)];
    state.revision = 42;
    drop(state);
    controller
}

fn block_waveform_worker(controller: &Controller) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    controller.channel_waveforms.lock().unwrap().before_render = Some(Arc::new(move || {
        entered.send(()).unwrap();
        released.lock().unwrap().recv().unwrap();
    }));
    (waiting, release)
}

#[test]
fn channel_work_releases_capture_lock_and_keeps_mixed_rows_on_one_revision() {
    let controller = mixed_waveform_controller();
    let (waiting, release) = block_waveform_worker(&controller);
    let (repaint, repainted) = mpsc::channel();
    controller.set_repaint(Arc::new(move || {
        let _ = repaint.send(());
    }));
    let mut low = [7.0; 32];
    let mut high = [8.0; 32];
    let mut view_low = [9.0; 32];
    let mut view_high = [10.0; 32];
    assert!(
        controller
            .try_render_channel_waveforms(
                &[(0, 3)],
                &[(1, WaveformView::Merged)],
                None,
                &mut [(&mut low, &mut high)],
                &mut [(&mut view_low, &mut view_high)],
            )
            .is_none()
    );
    waiting.recv_timeout(Duration::from_secs(2)).unwrap();
    // The actual capture worker can answer while the cold renderer is blocked.
    let snapshot = controller
        .request_snapshots(None)
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(snapshot[0].frame_range(), 0..8_000);
    {
        let mut state = lock(&controller.state);
        state
            .capture
            .as_mut()
            .unwrap()
            .capture_mut()
            .push_interleaved(&[1.0, 1.0, 1.0, 1.0, -1.0])
            .unwrap();
        state.revision = 43;
    }
    release.send(()).unwrap();
    repainted.recv_timeout(Duration::from_secs(2)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let rendered = loop {
        if let Some(rendered) = controller.try_render_channel_waveforms(
            &[(0, 3)],
            &[(1, WaveformView::Merged)],
            None,
            &mut [(&mut low, &mut high)],
            &mut [(&mut view_low, &mut view_high)],
        ) {
            break rendered;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(rendered.0, 0..8_000);
    assert_eq!(rendered.1, 0..8_000);
    assert_eq!(
        rendered.4, 42,
        "the final live tail must still need a redraw"
    );
    assert_eq!(low, [0.4; 32]);
    assert_eq!(high, [0.4; 32]);
    assert_eq!(view_low, [-0.5; 32]);
    assert_eq!(view_high, [-0.5; 32]);
}

#[test]
fn a_changed_channel_view_cancels_old_work_and_clear_rejects_old_audio() {
    let mut controller = mixed_waveform_controller();
    let (waiting, release) = block_waveform_worker(&controller);
    let mut low = [7.0; 16];
    let mut high = [8.0; 16];
    assert!(
        controller
            .try_render_channel_waveforms(
                &[(0, 3)],
                &[],
                None,
                &mut [(&mut low, &mut high)],
                &mut [],
            )
            .is_none()
    );
    waiting.recv_timeout(Duration::from_secs(2)).unwrap();
    for channel in [2, 1, 3, 2] {
        assert!(
            controller
                .try_render_channel_waveforms(
                    &[(0, channel)],
                    &[],
                    None,
                    &mut [(&mut low, &mut high)],
                    &mut [],
                )
                .is_none()
        );
        assert_eq!(low, [7.0; 16]);
    }
    controller.clear();
    controller.channel_waveforms.lock().unwrap().before_render = None;
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let rendered = loop {
        if let Some(rendered) = controller.try_render_channel_waveforms(
            &[(0, 2)],
            &[],
            None,
            &mut [(&mut low, &mut high)],
            &mut [],
        ) {
            break rendered;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    };
    assert!(rendered.0.is_empty());
    assert_eq!(low, [0.0; 16]);
    assert_eq!(high, [0.0; 16]);
}

#[test]
#[ignore = "release timing experiment: ten minutes at 48 kHz, up to 3.3 GB of audio"]
fn wide_waveform_capture_latency() {
    const RATE: usize = 48_000;
    const PACKET_FRAMES: usize = 480;
    let mut measurements = Vec::new();
    for channels in [12, 28] {
        let controller = Controller::new().unwrap();
        let mut capture = SynchronizedCapture::new_with_budget(
            RATE as u32,
            channels,
            None,
            DurationChoice::Minutes10,
            RATE * channels * 600,
        )
        .unwrap();
        capture.set_retain_silence(true);
        let block = vec![0.25; RATE * channels];
        for second in 0..600 {
            capture
                .push_native_interleaved(second * 1_000_000_000, &block, false)
                .unwrap();
        }
        let history_frames = (RATE * 600) as u64;
        assert_eq!(capture.capture().frame_range(), 0..history_frames);
        let mut endpoint = SenderEndpoint::new().unwrap();
        endpoint.update("waveform latency test", true, RATE as u32, channels);
        let id = endpoint.id().to_owned();
        let receiver = SenderReceiver::connect(&id).unwrap();
        endpoint.update("waveform latency test", true, RATE as u32, channels);
        let format = Arc::new(Format::sender(RATE as u32, channels));
        let source = CaptureSource::Sender {
            id: id.clone(),
            name: "waveform latency test".into(),
        };
        {
            let mut state = lock(&controller.state);
            state.sources = [
                Some(SourceSettings {
                    source: source.clone(),
                    route: ChannelRoute::Merged,
                }),
                None,
            ];
            state.inputs = vec![Input {
                source,
                stream: Stream::Sender {
                    receiver: Some(receiver),
                    format: Arc::clone(&format),
                    id,
                    error: None,
                },
            }];
            state.formats = [Some(format), None];
            state.shape = Some((RATE as u32, channels, None));
            state.capture_lanes = [Some(0), None];
            state.capture = Some(capture);
            state.paused = false;
            state.retain_silence = true;
            state.requested_retain_silence = true;
            state.next_layout_check = Instant::now() + Duration::from_secs(60);
            state.next_retry = Instant::now() + Duration::from_secs(60);
        }
        let finished = Arc::new(AtomicBool::new(false));
        let published = Arc::new(AtomicUsize::new(0));
        let (start, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let producer = {
            let finished = Arc::clone(&finished);
            let published = Arc::clone(&published);
            let wake = Arc::clone(&lock(&controller.state).wake);
            thread::spawn(move || {
                started.recv().unwrap();
                thread::sleep(Duration::from_millis(10));
                let began = Instant::now();
                for packet in 0..1_000 {
                    let audio = vec![(packet + 1) as f32; PACKET_FRAMES * channels];
                    endpoint
                        .publish(
                            "waveform latency test",
                            RATE as u32,
                            channels,
                            600_000_000_000 + packet as u64 * 10_000_000,
                            &audio,
                            &[0],
                        )
                        .unwrap();
                    published.store(packet + 1, Ordering::Release);
                    wake();
                    if packet >= 99 && finished.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(
                        (began + Duration::from_millis((packet + 1) as u64 * 10))
                            .saturating_duration_since(Instant::now()),
                    );
                }
                released.recv_timeout(Duration::from_secs(15)).unwrap();
            })
        };
        let probe_done = Arc::new(AtomicBool::new(false));
        let probe = {
            let state = Arc::clone(&controller.state);
            let done = Arc::clone(&probe_done);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(10));
                let mut longest_wait = Duration::ZERO;
                let mut previous_end = history_frames;
                let mut last_progress = Instant::now();
                let mut longest_capture_delay = Duration::ZERO;
                while !done.load(Ordering::Acquire) {
                    let waiting = Instant::now();
                    let state = lock(&state);
                    longest_wait = longest_wait.max(waiting.elapsed());
                    let end = state.capture.as_ref().unwrap().capture().frame_range().end;
                    if end != previous_end {
                        longest_capture_delay = longest_capture_delay.max(last_progress.elapsed());
                        previous_end = end;
                        last_progress = Instant::now();
                    }
                    drop(state);
                    thread::sleep(Duration::from_millis(1));
                }
                (longest_wait, longest_capture_delay)
            })
        };
        let requested: Vec<_> = (0..channels).map(|channel| (0, channel)).collect();
        let mut lows = vec![vec![0.0; 2_000]; channels];
        let mut highs = lows.clone();
        let mut traces: Vec<_> = lows
            .iter_mut()
            .zip(&mut highs)
            .map(|(low, high)| (low.as_mut_slice(), high.as_mut_slice()))
            .collect();
        start.send(()).unwrap();
        let drawing = Instant::now();
        let first_draw = controller.try_render_channel_waveforms(
            &requested,
            &[],
            Some(0..history_frames),
            &mut traces,
            &mut [],
        );
        let first_call = drawing.elapsed();
        let first_lock =
            Duration::from_nanos(controller.channel_waveform_lock_ns.load(Ordering::Relaxed));
        let mut longest_call = first_call;
        let mut ready = first_draw.is_some();
        while !ready && drawing.elapsed() < Duration::from_secs(30) {
            thread::sleep(Duration::from_millis(2));
            let call_at = Instant::now();
            ready = controller
                .try_render_channel_waveforms(
                    &requested,
                    &[],
                    Some(0..history_frames),
                    &mut traces,
                    &mut [],
                )
                .is_some();
            longest_call = longest_call.max(call_at.elapsed());
        }
        let completed = drawing.elapsed();
        finished.store(true, Ordering::Release);
        thread::sleep(Duration::from_millis(1_100));
        probe_done.store(true, Ordering::Release);
        let (longest_wait, longest_capture_delay) = probe.join().unwrap();
        let sent = published.load(Ordering::Acquire);
        let state = lock(&controller.state);
        let capture = state.capture.as_ref().unwrap().capture();
        let range = capture.frame_range();
        let bars = capture.bar_markers(history_frames..range.end);
        let tail = capture.freeze_range(history_frames..range.end).unwrap();
        let silent = tail
            .slices(history_frames..range.end)
            .flat_map(|slice| slice.chunks_exact(channels))
            .filter(|frame| frame.iter().all(|&sample| sample == 0.0))
            .count();
        let received = bars.len();
        println!(
            "waveform,channels={channels},seconds=600,rate={RATE},bins=2000,first_call_ms={:.3},first_lock_hold_ms={:.3},max_ui_call_ms={:.3},ready_ms={:.3},max_lock_wait_ms={:.3},max_capture_progress_delay_ms={:.3},sent_packets={sent},received_packets={received},missing_packets={},silent_gap_frames={silent},tail_frames={},status={:?}",
            first_call.as_secs_f64() * 1_000.0,
            first_lock.as_secs_f64() * 1_000.0,
            longest_call.as_secs_f64() * 1_000.0,
            completed.as_secs_f64() * 1_000.0,
            longest_wait.as_secs_f64() * 1_000.0,
            longest_capture_delay.as_secs_f64() * 1_000.0,
            sent.saturating_sub(received),
            range.end - history_frames,
            state.statuses,
        );
        drop(state);
        release.send(()).unwrap();
        producer.join().unwrap();
        measurements.push((channels, ready, first_call, sent, received, silent));
    }
    for (channels, ready, first_call, sent, received, silent) in measurements {
        assert!(ready, "{channels} channels never completed");
        assert!(
            first_call < Duration::from_millis(50),
            "{channels} channels blocked the UI for {first_call:?}"
        );
        assert_eq!(received, sent, "{channels} channels lost Sender packets");
        assert_eq!(silent, 0, "{channels} channels inserted a capture gap");
    }
}

#[test]
fn worker_draws_pair_rows_with_native_rows_on_the_same_revision() {
    use backtracking_core::{
        spatial,
        waveform::{PairGroup, PairSide},
    };
    let controller = mixed_waveform_controller();
    let labels = ["FL", "FR", "FC"];
    let group = PairGroup::new(1, &labels.map(str::to_string)).unwrap();
    let rows = [
        (0, ChannelWaveform::Pair(group.clone(), PairSide::Left)),
        (0, ChannelWaveform::Pair(group, PairSide::Right)),
        (1, ChannelWaveform::Native(0)),
    ];
    let mut expected = [0.; 2];
    spatial::fold_down(
        &[0.2, 0.3, 0.4],
        &spatial::stereo_weights(&labels),
        &mut expected,
    )
    .unwrap();
    let mut outputs = [([9.; 3], [9.; 3]); 3];
    let (waiting, release) = block_waveform_worker(&controller);
    let request = |out: &mut [([f32; 3], [f32; 3]); 3]| {
        let mut bufs = out.each_mut().map(|(lo, hi)| (&mut lo[..], &mut hi[..]));
        controller.try_render_rows(&rows, &[], None, &mut bufs, &mut [])
    };
    assert!(request(&mut outputs).is_none());
    waiting.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(controller.state.try_lock().is_ok());
    assert_eq!(outputs, [([9.; 3], [9.; 3]); 3]);
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(result) = request(&mut outputs) {
            assert_eq!(result.4, 42);
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    for ((lo, hi), value) in outputs.into_iter().zip([expected[0], expected[1], -0.5]) {
        assert_eq!(lo, [value; 3]);
        assert_eq!(hi, lo);
    }
}

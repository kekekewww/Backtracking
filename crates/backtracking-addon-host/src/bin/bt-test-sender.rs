//! From desktop/windows/src/ui/test_profile.rs at e9a4efc0d2918bccadeecf2c7e586b84ddd5a4f4.
use backtracking_core::{SenderEndpoint, capture_clock_ns};
use std::time::{Duration, Instant};
fn main() {
    #[cfg(target_os = "macos")]
    backtracking_addon_host::test_home::check().expect("Isolate the test home");
    match std::env::args().nth(1).as_deref() {
        Some("stereo") => serve_stereo_sender(),
        Some("7.1.4") => serve_wide_sender(),
        Some("spectrum") => serve_spectrum_sender(),
        _ => {
            eprintln!("usage: bt-test-sender stereo|7.1.4|spectrum");
            std::process::exit(2)
        }
    }
}
const STEREO_SENDER_ID: &str = "c1c1-5e7d-0002";
const STEREO_SENDER_NAME: &str = "Stereo test tone";
const SPECTRUM_SENDER_ID: &str = "c1c1-5e7d-0005";
const SPECTRUM_SENDER_NAME: &str = "Spectrum test";
const WIDE_SENDER_ID: &str = "c3c1-5e7d-0012";
const WIDE_SENDER_NAME: &str = "7.1.4 test tone";
const WIDE_SENDER_LABELS: [&str; 12] = [
    "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "TFL", "TFR", "TBL", "TBR",
];

fn serve_stereo_sender() {
    serve_sender(
        STEREO_SENDER_ID,
        STEREO_SENDER_NAME,
        2,
        &[],
        |time, pair| {
            // A slow swell tells this lane apart from the steady test tone.
            let swell = 0.35 + 0.3 * (std::f64::consts::TAU * 0.5 * time).sin();
            pair[0] = (swell * (std::f64::consts::TAU * 300.0 * time).sin()) as f32;
            pair[1] = (swell * 0.6 * (std::f64::consts::TAU * 500.0 * time).sin()) as f32;
        },
    );
}

fn serve_wide_sender() {
    let labels = match std::env::var("BACKTRACKING_TEST_LANE_B").as_deref() {
        Ok("sender-7.1.4") | Err(_) => WIDE_SENDER_LABELS.as_slice(),
        Ok("sender-7.1.4-unlabelled") => &[],
        _ => panic!("choose sender-7.1.4 or sender-7.1.4-unlabelled for lane B"),
    };
    // `spread`: log-spaced from 50 Hz to 12 kHz, so each row has its own colour by frequency.
    let spread = std::env::var("BACKTRACKING_TEST_WIDE_TONES").is_ok_and(|value| value == "spread");
    serve_sender(
        WIDE_SENDER_ID,
        WIDE_SENDER_NAME,
        12,
        labels,
        |time, frame| {
            for (channel, sample) in frame.iter_mut().enumerate() {
                let frequency = if spread {
                    50.0 * (12_000.0f64 / 50.0).powf(channel as f64 / 11.0)
                } else {
                    181.0 + 37.0 * channel as f64
                };
                // Start between zero crossings, including at the 48 kHz sample positions.
                *sample =
                    (0.25 * (std::f64::consts::TAU * (frequency * time + 0.12345)).sin()) as f32;
            }
        },
    );
}

fn serve_spectrum_sender() {
    const RATE: f64 = 48_000.0;
    let split =
        std::env::var("BACKTRACKING_TEST_SPECTRUM_SIDES").is_ok_and(|value| value == "split");
    let left = spectrum_program(RATE, false);
    let right = if split {
        spectrum_program(RATE, true)
    } else {
        left.clone()
    };
    serve_sender(
        SPECTRUM_SENDER_ID,
        SPECTRUM_SENDER_NAME,
        2,
        &[],
        move |time, pair| {
            let index = (time * RATE).round() as usize;
            pair[0] = left.get(index).copied().unwrap_or(0.0);
            pair[1] = right.get(index).copied().unwrap_or(0.0);
        },
    );
}

/// Three seconds of silence, then once: a 70 Hz burst, an 8 kHz burst, a log sweep from 40 Hz
/// to 12 kHz, pink noise and a kick with a hi-hat, 100 ms apart. `mirrored` swaps the bursts,
/// sweeps down and plays the drums before the noise, for the other side of a split view.
fn spectrum_program(rate: f64, mirrored: bool) -> Vec<f32> {
    use std::f64::consts::TAU;
    let samples = |seconds: f64| (seconds * rate) as usize;
    let mut out = vec![0.0f32; samples(3.0)];
    let gap = |out: &mut Vec<f32>| out.extend(std::iter::repeat_n(0.0, samples(0.1)));
    // 10 ms fades keep the edges from clicking across the whole spectrum.
    let fade = |index: usize, length: usize| {
        let edge = samples(0.01) as f64;
        (index as f64 / edge)
            .min((length - index) as f64 / edge)
            .min(1.0)
    };
    let tone = |out: &mut Vec<f32>, frequency: f64| {
        let length = samples(1.5);
        out.extend((0..length).map(|index| {
            (0.5 * fade(index, length) * (TAU * frequency * index as f64 / rate).sin()) as f32
        }));
    };
    let sweep = |out: &mut Vec<f32>, from: f64, to: f64| {
        let length = samples(5.0);
        let seconds = length as f64 / rate;
        let k = (to / from).ln();
        out.extend((0..length).map(|index| {
            let time = index as f64 / rate;
            let phase = TAU * from * seconds / k * ((k * time / seconds).exp() - 1.0);
            (0.5 * fade(index, length) * phase.sin()) as f32
        }));
    };
    // A fixed generator, so every run draws the same noise.
    let mut seed = 0x2545_f491_u32;
    let mut white = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        f64::from(seed) / f64::from(u32::MAX) * 2.0 - 1.0
    };
    let pink = |out: &mut Vec<f32>, white: &mut dyn FnMut() -> f64| {
        // Paul Kellet's economy filter.
        let (mut b0, mut b1, mut b2) = (0.0, 0.0, 0.0);
        let length = samples(2.0);
        out.extend((0..length).map(|index| {
            let w = white();
            b0 = 0.99765 * b0 + w * 0.0990460;
            b1 = 0.96300 * b1 + w * 0.2965164;
            b2 = 0.57000 * b2 + w * 1.0526913;
            (0.12 * fade(index, length) * (b0 + b1 + b2 + w * 0.1848)) as f32
        }));
    };
    let drums = |out: &mut Vec<f32>, white: &mut dyn FnMut() -> f64| {
        let length = samples(3.0);
        let beat = samples(0.5);
        let mut previous = 0.0;
        out.extend((0..length).map(|index| {
            let since_kick = (index % beat) as f64 / rate;
            // The pitch falls from 150 Hz to 50 Hz: the phase is its integral.
            let phase =
                TAU * (50.0 * since_kick + 100.0 * 0.03 * (1.0 - (-since_kick / 0.03).exp()));
            let kick = 0.8 * (-since_kick / 0.15).exp() * phase.sin();
            let since_hat = ((index + beat / 2) % beat) as f64 / rate;
            let noise = white();
            // A first difference leaves mostly the top of the noise.
            let hat = 0.35 * (-since_hat / 0.03).exp() * (noise - previous);
            previous = noise;
            (fade(index, length) * (kick + hat)) as f32
        }));
    };
    if mirrored {
        tone(&mut out, 8_000.0);
        gap(&mut out);
        tone(&mut out, 70.0);
        gap(&mut out);
        sweep(&mut out, 12_000.0, 40.0);
        gap(&mut out);
        drums(&mut out, &mut white);
        gap(&mut out);
        pink(&mut out, &mut white);
    } else {
        tone(&mut out, 70.0);
        gap(&mut out);
        tone(&mut out, 8_000.0);
        gap(&mut out);
        sweep(&mut out, 40.0, 12_000.0);
        gap(&mut out);
        pink(&mut out, &mut white);
        gap(&mut out);
        drums(&mut out, &mut white);
    }
    out
}

fn serve_sender(
    id: &str,
    name: &str,
    channels: usize,
    labels: &[&str],
    fill_frame: impl Fn(f64, &mut [f32]),
) {
    let seconds = std::env::var("BACKTRACKING_TEST_SENDER_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(40.0f64);
    let mut sender = SenderEndpoint::with_id(id).unwrap();
    assert_eq!(
        sender.id(),
        id,
        "an earlier Sender with this id is still registered"
    );
    const RATE: u32 = 48_000;
    const FRAMES: usize = 480;
    if !labels.is_empty() {
        sender
            .update_with_labels(name, true, RATE, channels, labels)
            .unwrap();
    }
    let started = Instant::now();
    let clock = capture_clock_ns();
    let mut frame = 0u64;
    let mut block = vec![0.0f32; FRAMES * channels];
    while started.elapsed().as_secs_f64() < seconds {
        // Windows sleeps in coarse steps, so catch up with every block that is due.
        while frame * 1_000_000 / u64::from(RATE) <= started.elapsed().as_micros() as u64 {
            for (index, samples) in block.chunks_exact_mut(channels).enumerate() {
                let time = (frame + index as u64) as f64 / f64::from(RATE);
                fill_frame(time, samples);
            }
            let timestamp = clock + frame * 1_000_000_000 / u64::from(RATE);
            sender
                .publish(name, RATE, channels, timestamp, &block, &[])
                .unwrap();
            frame += FRAMES as u64;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

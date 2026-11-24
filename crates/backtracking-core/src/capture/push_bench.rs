//! Run with `cargo test --release -p backtracking-core push_cost -- --ignored --nocapture --test-threads=1`.
//!
//! Times appending loud audio, which the gate keeps, in 256-frame chunks at 48 kHz. The
//! history holds 20 seconds, so after the warm-up every page is a recycled one, as in a long
//! recording. Each case reports the median of nine ten-second batches after one warm-up.

use std::hint::black_box;
use std::time::{Duration, Instant};

use super::{Capture, DurationChoice};

const RATE: usize = 48_000;
const CHUNK_FRAMES: usize = 256;
const BATCH_SECONDS: usize = 10;
const REPEATS: usize = 9;

fn audio(channels: usize) -> Vec<f32> {
    (0..CHUNK_FRAMES * channels)
        .map(|index| {
            let frame = index / channels;
            let channel = index % channels;
            ((frame * 31 + channel * 127) % 1_009) as f32 / 504.0 - 1.0
        })
        .collect()
}

fn time_pushes(mut capture: Capture, channels: usize) -> (Duration, Duration) {
    let samples = audio(channels);
    let mut batches = Vec::with_capacity(REPEATS);
    let mut chunks = Vec::new();
    for repetition in 0..=REPEATS {
        let started = Instant::now();
        for _ in 0..RATE * BATCH_SECONDS / CHUNK_FRAMES {
            let chunk = Instant::now();
            capture.push_interleaved(black_box(&samples)).unwrap();
            if repetition != 0 {
                chunks.push(chunk.elapsed());
            }
        }
        if repetition != 0 {
            batches.push(started.elapsed());
        }
    }
    batches.sort_unstable();
    chunks.sort_unstable();
    (batches[REPEATS / 2], chunks[chunks.len() * 99 / 100])
}

#[test]
#[ignore = "release-mode append benchmark"]
fn push_cost() {
    println!("lanes,channels,median_ms_per_audio_second,p99_chunk_us");
    for round in 0..2 {
        for (name, widths) in [
            ("mono", &[1][..]),
            ("stereo", &[2]),
            ("stereo+stereo", &[2, 2]),
            ("stereo+stereo+stereo", &[2, 2, 2]),
            ("stereo+12+mono", &[2, 12, 1]),
            ("64 mono", &[1; 64]),
        ] {
            let channels: usize = widths.iter().sum();
            let budget = RATE * channels * 20;
            let retention = DurationChoice::Minutes10;
            // One or two lanes go through the pair constructor, as the apps build them.
            let capture = match *widths {
                [a] => Capture::new_tracks_with_budget(RATE as u32, a, None, retention, budget),
                [a, b] => Capture::new_tracks_with_budget(RATE as u32, a, Some(b), retention, budget),
                _ => Capture::new_lanes_with_budget(RATE as u32, widths, retention, budget),
            }
            .unwrap();
            let (batch, chunk) = time_pushes(capture, channels);
            println!(
                "round {round},{name},{channels},{:.3},{:.3}",
                batch.as_secs_f64() * 1_000.0 / BATCH_SECONDS as f64,
                chunk.as_secs_f64() * 1_000_000.0
            );
        }
    }
}

use super::*;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

fn fixed_sources(channels: usize) -> Vec<BinauralSource> {
    (0..channels)
        .map(|channel| {
            BinauralSource::Fixed(Some(Direction {
                azimuth: channel as f32 * 360.0 / channels as f32 - 180.0,
                elevation: if channel % 3 == 0 { 45.0 } else { 0.0 },
            }))
        })
        .collect()
}

fn input(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|sample| {
            let frame = sample / channels;
            let channel = sample % channels;
            (frame as f32 * (0.023 + channel as f32 * 0.001)).sin() * 0.03
        })
        .collect()
}

/// Run with `cargo +1.95.0 test --release -p backtracking-core spatial::benchmark -- --ignored --nocapture --test-threads=1`.
#[test]
#[ignore = "measures rendering speed; run explicitly in release mode"]
fn render_performance() {
    let model = Arc::new(HeadModel::new(48_000).unwrap());
    println!("48 kHz, one render thread, block size 256; real-time factor = wall seconds / audio seconds");
    println!("prepared head-model memory: {} bytes", model.memory_bytes());
    for channels in [2, 12, 28] {
        let sources = fixed_sources(channels);
        let mut renderer = BinauralRenderer::new(model.clone(), &sources, None, 0).unwrap();
        let input = input(256, channels);
        let mut output = vec![0.0; 512];
        for _ in 0..16 {
            renderer
                .render(black_box(&input), black_box(&mut output))
                .unwrap();
        }
        let blocks = 48_000 * 5 / 256;
        let start = Instant::now();
        for _ in 0..blocks {
            renderer
                .render(black_box(&input), black_box(&mut output))
                .unwrap();
        }
        let seconds = start.elapsed().as_secs_f64();
        let audio_seconds = (blocks * 256) as f64 / 48_000.0;
        println!(
            "block render {channels:2} channels: {seconds:.6} s / {audio_seconds:.6} s audio, RTF {:.6}, {:.3}% real-time thread budget (wall time)",
            seconds / audio_seconds,
            seconds / audio_seconds * 100.0
        );
        black_box(&output);
    }

    let frames = 48_000 * 30;
    let sources = fixed_sources(12);
    let input = input(frames, sources.len());
    let start = Instant::now();
    let output = render_binaural(model.clone(), &sources, None, 0, black_box(&input)).unwrap();
    let seconds = start.elapsed().as_secs_f64();
    println!(
        "30 s clip, 12 fixed channels: {seconds:.6} s, RTF {:.6}",
        seconds / 30.0
    );
    black_box(&output);
    drop(output);
    drop(input);

    let sources: Vec<_> = (0..16)
        .map(|channel| {
            let keys = (0..=30 * 50)
                .map(|index| {
                    let angle =
                        std::f32::consts::TAU * (index as f32 / 200.0 + channel as f32 / 16.0);
                    PositionKey {
                        frame: index * 960,
                        x: angle.sin(),
                        y: angle.cos(),
                        z: 0.25 * (angle * 0.5).sin(),
                        size: 0.0,
                        gain: 1.0,
                    }
                })
                .collect();
            BinauralSource::Moving(keys)
        })
        .collect();
    let input = self::input(frames, sources.len());
    let start = Instant::now();
    let output = render_binaural(model, &sources, None, 0, black_box(&input)).unwrap();
    let seconds = start.elapsed().as_secs_f64();
    println!(
        "30 s clip, 16 moving objects (50 keys/s): {seconds:.6} s, RTF {:.6}",
        seconds / 30.0
    );
    black_box(&output);
}

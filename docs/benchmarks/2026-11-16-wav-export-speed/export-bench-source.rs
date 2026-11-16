//! WAV export timing harness for docs/benchmarks/2026-11-16-wav-export-speed.
//!
//! It builds against the backtracking-core at ../../crates/backtracking-core, so the same
//! file times the base commit and the change. Every run prints one JSON line. Nothing is
//! averaged, sorted or discarded here.
//!
//! exports <out-dir> <build-label> <clip-seconds,...> <source-rate> <export-rate,...> <runs> [pause-ms]
//!     Real exports through the public export API. Two stereo lanes of synthetic noise are
//!     held in a Capture and frozen, as the apps do before a drag or snapshot. Each case
//!     exports the A/B pair, then lane A alone, as 32-bit float WAVs in <out-dir>. Every
//!     file is deleted after its size is read.
//!
//! writes <out-dir> <bytes,...> <runs> [pause-ms]
//!     Plain writes of new files of the given sizes into <out-dir>: in 4 KiB pieces through
//!     an 8 KiB BufWriter, the pattern the exporter uses, and in one call.
//!
//! With pause-ms, every timed run waits that long first, untimed, so the disk work of the
//! runs before it can settle. Without it, runs follow each other directly.

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

use backtracking_core::{
    export_routed_tracks_with_profile, Capture, ChannelRoute, DurationChoice, FrozenTrack,
    WavExportProfile, WavSampleFormat,
};

fn list<T: FromStr>(text: &str) -> Vec<T>
where
    T::Err: std::fmt::Debug,
{
    text.split(',').map(|value| value.parse().unwrap()).collect()
}

/// Two stereo lanes holding `seconds` of deterministic noise at `rate`, frozen.
fn lanes(rate: u32, seconds: usize) -> Vec<FrozenTrack> {
    let channels = 4;
    let mut capture = Capture::new_tracks(rate, 2, Some(2), DurationChoice::Minutes10).unwrap();
    capture.set_retain_silence(true);
    let total = seconds * rate as usize;
    let block_frames = (rate / 100) as usize;
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut block = vec![0.0f32; block_frames * channels];
    let mut pushed = 0;
    while pushed < total {
        let count = (total - pushed).min(block_frames);
        for sample in &mut block[..count * channels] {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *sample = ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.8;
        }
        capture.push_interleaved(&block[..count * channels]).unwrap();
        pushed += count;
    }
    let tracks = capture.freeze().tracks();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].frame_count(), total);
    tracks
}

/// Waits before a timed run; with no pause the run starts at once.
fn pause(ms: u64) {
    if ms > 0 {
        thread::sleep(Duration::from_millis(ms));
    }
}

fn exports(
    dir: &Path,
    build: &str,
    lengths: &[usize],
    source_rate: u32,
    export_rates: &[u32],
    runs: usize,
    pause_ms: u64,
) {
    let threads = thread::available_parallelism().map_or(0, |threads| threads.get());
    println!(
        "{{\"mode\":\"environment\",\"build\":\"{build}\",\"available_parallelism\":{threads}}}"
    );
    for &seconds in lengths {
        let tracks = lanes(source_rate, seconds);
        let frames = tracks[0].frame_count();
        for &export_rate in export_rates {
            let profile = WavExportProfile {
                sample_rate: export_rate,
                sample_format: WavSampleFormat::Float32,
            };
            for lanes in [2, 1] {
                for run in 1..=runs {
                    pause(pause_ms);
                    let start = Instant::now();
                    let paths = export_routed_tracks_with_profile(
                        dir,
                        &tracks[..lanes],
                        0..frames,
                        profile,
                        &[ChannelRoute::Merged; 2][..lanes],
                        false,
                    )
                    .unwrap();
                    let elapsed = start.elapsed().as_secs_f64();
                    let bytes: u64 = paths
                        .iter()
                        .map(|path| fs::metadata(path).unwrap().len())
                        .sum();
                    for path in &paths {
                        fs::remove_file(path).unwrap();
                    }
                    println!(
                        "{{\"mode\":\"export\",\"build\":\"{build}\",\"clip_seconds\":{seconds},\
                         \"source_rate\":{source_rate},\"export_rate\":{export_rate},\
                         \"format\":\"float32\",\"lanes\":{lanes},\"channels_per_lane\":2,\
                         \"run\":{run},\"pause_before_ms\":{pause_ms},\
                         \"elapsed_s\":{elapsed:.6},\"bytes\":{bytes}}}"
                    );
                }
            }
        }
    }
}

fn writes(dir: &Path, sizes: &[usize], runs: usize, pause_ms: u64) {
    for &bytes in sizes {
        let data = vec![0x3fu8; bytes];
        for run in 1..=runs {
            for pattern in ["4KiB_pieces_8KiB_BufWriter", "one_call"] {
                let path = dir.join("write-probe.bin");
                pause(pause_ms);
                let start = Instant::now();
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .unwrap();
                if pattern == "one_call" {
                    let mut file = file;
                    file.write_all(&data).unwrap();
                } else {
                    let mut writer = BufWriter::new(file);
                    for piece in data.chunks(4096) {
                        writer.write_all(piece).unwrap();
                    }
                    writer.flush().unwrap();
                }
                // The file is closed above, so the time includes opening and closing it.
                let elapsed = start.elapsed().as_secs_f64();
                fs::remove_file(&path).unwrap();
                println!(
                    "{{\"mode\":\"write\",\"pattern\":\"{pattern}\",\"bytes\":{bytes},\
                     \"run\":{run},\"pause_before_ms\":{pause_ms},\"elapsed_s\":{elapsed:.6}}}"
                );
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&args[2]);
    fs::create_dir_all(&dir).unwrap();
    let pause_ms = |at: usize| args.get(at).map_or(0, |ms| ms.parse().unwrap());
    match args[1].as_str() {
        "exports" => exports(
            &dir,
            &args[3],
            &list(&args[4]),
            args[5].parse().unwrap(),
            &list(&args[6]),
            args[7].parse().unwrap(),
            pause_ms(8),
        ),
        "writes" => writes(&dir, &list(&args[3]), args[4].parse().unwrap(), pause_ms(5)),
        mode => panic!("unknown mode {mode}"),
    }
}

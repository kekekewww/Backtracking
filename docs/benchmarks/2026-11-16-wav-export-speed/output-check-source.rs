//! Output check for docs/benchmarks/2026-11-16-wav-export-speed.
//!
//! Built as src/bin/output-check.rs beside the timing harness, with the same Cargo.toml and
//! Cargo.lock, once against each core. It exports two lanes of deterministic noise through
//! the public export function for every combination below and keeps the files under fixed
//! names, so that the base and changed builds' files can be compared byte for byte. It
//! prints one JSON line per file.
//!
//! output-check <empty-out-dir>
//!
//! Combinations: 48 and 44.1 kHz sources; 96, 48 and 44.1 kHz exports; 16-bit, 24-bit and
//! 32-bit float samples; five route settings that between them use every channel map:
//! stereo lanes merged and forced to mono, and mono lanes placed left and right, as side
//! and merged, and as mid and center; the whole 20-second clip and two short ranges that
//! start part way in; the A/B pair and lane A alone.

use std::fs;
use std::path::PathBuf;

use backtracking_core::{
    export_routed_tracks_with_profile, Capture, ChannelRoute, DurationChoice, FrozenTrack,
    WavExportProfile, WavSampleFormat,
};

const CLIP_SECONDS: usize = 20;

/// Two lanes of `lane_channels` channels each, holding `CLIP_SECONDS` of deterministic noise
/// at `rate`, frozen.
fn lanes(rate: u32, lane_channels: usize) -> Vec<FrozenTrack> {
    let channels = 2 * lane_channels;
    let mut capture =
        Capture::new_tracks(rate, lane_channels, Some(lane_channels), DurationChoice::Minutes10)
            .unwrap();
    capture.set_retain_silence(true);
    let total = CLIP_SECONDS * rate as usize;
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

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("an empty output folder"));
    fs::create_dir_all(&dir).unwrap();
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "the output folder must be empty");
    let settings = [
        (2, [ChannelRoute::Merged, ChannelRoute::Merged], false),
        (2, [ChannelRoute::Merged, ChannelRoute::Merged], true),
        (1, [ChannelRoute::Left, ChannelRoute::Right], false),
        (1, [ChannelRoute::Side, ChannelRoute::Merged], false),
        (1, [ChannelRoute::Mid, ChannelRoute::Center], false),
    ];
    let formats = [WavSampleFormat::Pcm16, WavSampleFormat::Pcm24, WavSampleFormat::Float32];
    let mut case = 0;
    for source_rate in [48_000, 44_100] {
        let stereo = lanes(source_rate, 2);
        let mono = lanes(source_rate, 1);
        let whole = 0..stereo[0].frame_count();
        let ranges = [whole, 4_321..5_321, 98_765..111_070];
        for export_rate in [96_000, 48_000, 44_100] {
            for sample_format in formats {
                let profile = WavExportProfile {
                    sample_rate: export_rate,
                    sample_format,
                };
                for (lane_channels, routes, force_mono) in settings {
                    let tracks = if lane_channels == 2 { &stereo } else { &mono };
                    for frames in &ranges {
                        for lanes in [2, 1] {
                            case += 1;
                            let paths = export_routed_tracks_with_profile(
                                &dir,
                                &tracks[..lanes],
                                frames.clone(),
                                profile,
                                &routes[..lanes],
                                force_mono,
                            )
                            .unwrap();
                            assert_eq!(paths.len(), lanes);
                            for (index, path) in paths.iter().enumerate() {
                                let name = format!("case-{case:03}-file-{index}.wav");
                                fs::rename(path, dir.join(&name)).unwrap();
                                let bytes = fs::metadata(dir.join(&name)).unwrap().len();
                                println!(
                                    "{{\"case\":{case},\"source_rate\":{source_rate},\
                                     \"export_rate\":{export_rate},\"format\":\"{sample_format:?}\",\
                                     \"lane_channels\":{lane_channels},\"routes\":\"{:?}\",\
                                     \"force_mono\":{force_mono},\"frames\":\"{}..{}\",\
                                     \"lanes\":{lanes},\"file\":\"{name}\",\"bytes\":{bytes}}}",
                                    &routes[..lanes],
                                    frames.start,
                                    frames.end,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

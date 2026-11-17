//! Run with `cargo test --release --workspace channel_cache_benchmark -- --ignored --nocapture --test-threads=1`.

use std::hint::black_box;
use std::ops::Range;
use std::time::{Duration, Instant};

use super::{waveform_bins, Capture, DurationChoice};

const RATE: usize = 48_000;
const BINS: usize = 2_000;
const REPEATS: usize = 7;
const CHUNK_FRAMES: usize = 256;

fn audio(channels: usize) -> Vec<f32> {
    (0..CHUNK_FRAMES * channels)
        .map(|index| {
            let frame = index / channels;
            let channel = index % channels;
            ((frame * 31 + channel * 127) % 1_009) as f32 / 504.0 - 1.0
        })
        .collect()
}

fn append(capture: &mut Capture, samples: &[f32], frames: usize, channels: usize) {
    let mut remaining = frames;
    while remaining != 0 {
        let count = remaining.min(CHUNK_FRAMES);
        capture
            .push_interleaved(black_box(&samples[..count * channels]))
            .unwrap();
        remaining -= count;
    }
}

// The old per-channel path: one strided sample scan for each bin, without a
// temporary channel buffer. Keep it here so later runs still measure "before".
fn uncached(
    capture: &Capture,
    channel: usize,
    range: Range<u64>,
    low: &mut [f32],
    high: &mut [f32],
) -> usize {
    let retained = capture.frame_range();
    let range = range.start.max(retained.start)..range.end.min(retained.end);
    waveform_bins(range, low, high, |frames| {
        let mut minimum = f32::INFINITY;
        let mut maximum = f32::NEG_INFINITY;
        for slice in capture.history.slices(frames) {
            for frame in slice.chunks_exact(capture.channels) {
                let value = frame[channel];
                if value.is_finite() {
                    minimum = minimum.min(value);
                    maximum = maximum.max(value);
                }
            }
        }
        if minimum.is_finite() {
            (minimum, maximum)
        } else {
            (0.0, 0.0)
        }
    })
}

fn median(mut values: Vec<Duration>) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

fn time_redraw(mut draw: impl FnMut()) -> Duration {
    let mut elapsed = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let started = Instant::now();
        draw();
        elapsed.push(started.elapsed());
    }
    median(elapsed)
}

fn memory(capture: &Capture, channels: usize, stage: &str) {
    let (metadata, payload) = capture.history.channel_cache_bytes();
    print!(
        "memory,channels={channels},stage={stage},channel_metadata_bytes={metadata},channel_peak_bytes={payload}"
    );
    #[cfg(windows)]
    match windows_memory::snapshot() {
        Some((working_set, private_commit, total, available)) => print!(
            ",working_set_bytes={working_set},private_commit_bytes={private_commit},machine_physical_bytes={total},machine_available_bytes={available}"
        ),
        None => print!(",process_memory=unavailable"),
    }
    println!();
}

#[cfg(windows)]
mod windows_memory {
    use std::ffi::c_void;
    use std::mem::size_of;

    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }

    #[repr(C)]
    #[derive(Default)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    }

    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
    }

    pub(super) fn snapshot() -> Option<(usize, usize, u64, u64)> {
        let mut process = ProcessMemoryCountersEx {
            cb: size_of::<ProcessMemoryCountersEx>() as u32,
            ..Default::default()
        };
        let mut machine = MemoryStatusEx {
            length: size_of::<MemoryStatusEx>() as u32,
            ..Default::default()
        };
        let process_size = process.cb;
        // Both buffers match the Windows structures and carry their exact size.
        // GetCurrentProcess returns a pseudo-handle that must not be closed.
        let read = unsafe {
            GetProcessMemoryInfo(GetCurrentProcess(), &mut process, process_size) != 0
                && GlobalMemoryStatusEx(&mut machine) != 0
        };
        read.then_some((
            process.working_set_size,
            process.private_usage,
            machine.total_phys,
            machine.avail_phys,
        ))
    }
}

#[test]
#[ignore = "release-mode timing benchmark; allocates the default retained-audio budget"]
fn channel_cache_benchmark_redraw() {
    println!(
        "redraw: 48 kHz, {BINS} bins/channel, median of {REPEATS} draws; default sample budget"
    );
    println!("memory: channel bytes count requested allocations; OS working-set/commit deltas also include faults, allocator behavior, and the open-page copy on freeze");
    println!("channels,requested_seconds,visible_seconds,uncached_ms,cached_ms,frozen_ms,speedup");
    for channels in [12, 40] {
        let mut capture = Capture::new(RATE as u32, channels, DurationChoice::Minutes10).unwrap();
        capture.set_retain_silence(true);
        let samples = audio(channels);
        // Put the oldest boundary inside a page and leave an unfinished open page.
        let frames = capture.effective_retention_frames() + RATE + 137;
        append(&mut capture, &samples, frames, channels);
        let retained = capture.frame_range();
        let mut low = vec![0.0; BINS];
        let mut high = vec![0.0; BINS];
        memory(&capture, channels, "before_cold");
        let cold_started = Instant::now();
        for channel in 0..channels {
            capture.track_channel_waveform_range(0, channel, retained.clone(), &mut low, &mut high);
            black_box((&low, &high));
        }
        println!(
            "cold_all_channels,{channels},{:.6},elapsed_ms={:.3}",
            capture.effective_retention_seconds(),
            cold_started.elapsed().as_secs_f64() * 1_000.0,
        );
        memory(&capture, channels, "after_cold");
        let cache_before_freeze = capture.history.channel_cache_bytes();
        let frozen = capture.freeze().track(0).unwrap();
        assert_eq!(capture.history.channel_cache_bytes(), cache_before_freeze);
        memory(&capture, channels, "after_freeze");
        for requested_seconds in [Some(10), Some(60), None] {
            let visible = requested_seconds
                .map(|seconds| seconds * RATE as u64)
                .unwrap_or(retained.end - retained.start);
            let range = retained.end.saturating_sub(visible).max(retained.start)..retained.end;
            let mut expected_low = vec![0.0; BINS];
            let mut expected_high = vec![0.0; BINS];
            for channel in 0..channels {
                let expected = uncached(
                    &capture,
                    channel,
                    range.clone(),
                    &mut expected_low,
                    &mut expected_high,
                );
                assert_eq!(
                    capture.track_channel_waveform_range(
                        0,
                        channel,
                        range.clone(),
                        &mut low,
                        &mut high,
                    ),
                    expected,
                );
                assert_eq!(low, expected_low);
                assert_eq!(high, expected_high);
                assert_eq!(
                    frozen.channel_waveform(channel, range.clone(), &mut low, &mut high),
                    expected,
                );
                assert_eq!(low, expected_low);
                assert_eq!(high, expected_high);
            }
            let before = time_redraw(|| {
                for channel in 0..channels {
                    black_box(uncached(
                        &capture,
                        channel,
                        range.clone(),
                        &mut low,
                        &mut high,
                    ));
                    black_box((&low, &high));
                }
            });
            let after = time_redraw(|| {
                for channel in 0..channels {
                    black_box(capture.track_channel_waveform_range(
                        0,
                        channel,
                        range.clone(),
                        &mut low,
                        &mut high,
                    ));
                    black_box((&low, &high));
                }
            });
            let frozen_elapsed = time_redraw(|| {
                for channel in 0..channels {
                    black_box(frozen.channel_waveform(channel, range.clone(), &mut low, &mut high));
                    black_box((&low, &high));
                }
            });
            println!(
                "{channels},{},{:.6},{:.3},{:.3},{:.3},{:.2}",
                requested_seconds
                    .map(|seconds| seconds.to_string())
                    .unwrap_or_else(|| "full".into()),
                (range.end - range.start) as f64 / RATE as f64,
                before.as_secs_f64() * 1_000.0,
                after.as_secs_f64() * 1_000.0,
                frozen_elapsed.as_secs_f64() * 1_000.0,
                before.as_secs_f64() / after.as_secs_f64(),
            );
        }
    }
}

#[test]
#[ignore = "release-mode capture throughput benchmark"]
fn channel_cache_benchmark_capture_path() {
    const BATCH_SECONDS: usize = 10;
    println!("capture: 48 kHz, 256-frame chunks, median of {REPEATS} ten-second batches");
    println!("channels,viewed,median_ms_per_audio_second,p99_chunk_us,max_chunk_us");
    for channels in [2, 12, 40] {
        for viewed in [false, true] {
            let mut capture = Capture::new_with_budget(
                RATE as u32,
                channels,
                DurationChoice::Minutes10,
                RATE * channels * 2,
            )
            .unwrap();
            capture.set_retain_silence(true);
            let samples = audio(channels);
            append(&mut capture, &samples, RATE * 2 + 137, channels);
            let mut low = vec![0.0; BINS];
            let mut high = vec![0.0; BINS];
            let mut batches = Vec::with_capacity(REPEATS);
            let mut chunks = Vec::new();
            // Exclude one warm-up batch from timings.
            for repetition in 0..=REPEATS {
                if viewed {
                    for channel in 0..channels {
                        capture.track_channel_waveform_range(
                            0,
                            channel,
                            capture.frame_range(),
                            &mut low[..1],
                            &mut high[..1],
                        );
                        black_box((&low, &high));
                    }
                }
                let batch_started = Instant::now();
                let mut remaining = RATE * BATCH_SECONDS;
                while remaining != 0 {
                    let count = remaining.min(CHUNK_FRAMES);
                    let chunk_started = Instant::now();
                    capture
                        .push_interleaved(black_box(&samples[..count * channels]))
                        .unwrap();
                    let elapsed = chunk_started.elapsed();
                    if repetition != 0 {
                        chunks.push(elapsed);
                    }
                    remaining -= count;
                }
                if repetition != 0 {
                    batches.push(batch_started.elapsed());
                }
            }
            chunks.sort_unstable();
            println!(
                "{channels},{viewed},{:.3},{:.3},{:.3}",
                median(batches).as_secs_f64() * 1_000.0 / BATCH_SECONDS as f64,
                chunks[(chunks.len() * 99 / 100).min(chunks.len() - 1)].as_secs_f64() * 1_000_000.0,
                chunks.last().unwrap().as_secs_f64() * 1_000_000.0,
            );
        }
    }
}

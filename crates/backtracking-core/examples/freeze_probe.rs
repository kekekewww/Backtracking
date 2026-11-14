//! Core-only probe for what an edit freeze costs, without third-party dependencies.
//!
//! Run from the repository root:
//! `cargo run --release --locked -p backtracking-core --example freeze_probe`
//!
//! It fills a capture with 600 seconds of 48 kHz stereo non-silent audio in 10 ms
//! blocks, then reports, as JSONL:
//! - the wall time of pinning the retained audio for editing,
//! - the bytes that pinning allocated (every allocation is counted, so this is the
//!   memory the freeze adds on top of the capture itself),
//! - the wall time of the edit preview peaks at 1000 bins, for the whole range and
//!   for a 5 second window that starts and ends inside a page and a peak block.
//!
//! The `flat_copy` rows use the owned `Snapshot` copy and a raw sample scan. They
//! are what the edit freeze did before pages became shareable, and are kept so
//! both paths can be compared in one run on the same machine. The `shared_pages`
//! rows use the frozen view the apps edit from: it shares the capture's complete
//! pages, copies only the page still being written, and reads peaks from the
//! capture's peak cache. The probe checks that both paths draw the same peaks.
//!
//! This is not a GUI or audio-device test. System load and CPU frequency are not
//! controlled, so reruns vary; compare rows from the same run.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use backtracking_core::waveform::view_peaks;
use backtracking_core::{Capture, DurationChoice, WaveformView};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const SECONDS: usize = 600;
const BINS: usize = 1000;
const REPEATS: usize = 7;

struct CountingAllocator;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATED.fetch_add(size.saturating_sub(layout.size()), Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

struct Timing {
    /// The first run, which is the one a user feels: nothing is warm yet.
    first_ms: f64,
    median_ms: f64,
    /// Bytes one run allocated.
    bytes: usize,
}

/// Runs `work` several times and returns its timing and the last result.
fn measure<T>(mut work: impl FnMut() -> T) -> (Timing, T) {
    let mut times = Vec::with_capacity(REPEATS);
    let mut bytes = 0;
    let mut last = None;
    for _ in 0..REPEATS {
        drop(last.take());
        let before = ALLOCATED.load(Ordering::Relaxed);
        let start = Instant::now();
        let result = black_box(work());
        times.push(elapsed_ms(start));
        bytes = ALLOCATED.load(Ordering::Relaxed) - before;
        last = Some(result);
    }
    let first_ms = times[0];
    times.sort_by(|a, b| a.total_cmp(b));
    let timing = Timing { first_ms, median_ms: times[times.len() / 2], bytes };
    (timing, last.expect("at least one run"))
}

fn report(path: &str, stage: &str, timing: &Timing, with_bytes: bool) {
    let bytes = if with_bytes {
        format!(",\"allocated_bytes\":{}", timing.bytes)
    } else {
        String::new()
    };
    println!(
        "{{\"path\":\"{path}\",\"stage\":\"{stage}\",\"first_ms\":{:.4},\"median_ms\":{:.4}{bytes}}}",
        timing.first_ms, timing.median_ms
    );
}

/// One second of deterministic, never silent stereo audio.
fn one_second() -> Vec<f32> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..RATE as usize)
        .flat_map(|frame| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let noise = (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
            let tone = (frame as f32 * 0.057).sin() * 0.4;
            [tone + noise * 0.2, tone * 0.5 - noise * 0.3]
        })
        .collect()
}

fn main() {
    let mut capture =
        Capture::new_tracks(RATE, CHANNELS, None, DurationChoice::Minutes10).unwrap();
    let second = one_second();
    let block = RATE as usize / 100 * CHANNELS;
    let start = Instant::now();
    for _ in 0..SECONDS {
        for samples in second.chunks(block) {
            capture.push_interleaved(black_box(samples)).unwrap();
        }
    }
    println!("{{\"path\":\"capture\",\"stage\":\"fill_600s\",\"ms\":{:.4}}}", elapsed_ms(start));
    let history = capture.frame_range();
    assert_eq!(history.end - history.start, (RATE as usize * SECONDS) as u64);
    // Starts and ends inside a page and inside a 128 frame peak block.
    let window_start = history.start + 300 * u64::from(RATE) + 12_345;
    let window = window_start..window_start + 5 * u64::from(RATE);
    let mut low = vec![0.0f32; BINS];
    let mut high = vec![0.0f32; BINS];

    // The freeze used before pages were shareable: one flat copy of the lane.
    let (timing, snapshot) = measure(|| capture.track_snapshot(0).unwrap());
    report("flat_copy", "freeze", &timing, true);
    let (timing, _) = measure(|| {
        view_peaks(&snapshot.samples, snapshot.channels, WaveformView::Merged, &mut low, &mut high);
        (low[BINS / 2], high[BINS / 2])
    });
    report("flat_copy", "preview_full_range_1000_bins", &timing, false);
    let relative = (window.start - snapshot.first_frame) as usize
        ..(window.end - snapshot.first_frame) as usize;
    let (timing, _) = measure(|| {
        let samples = snapshot.frames(relative.clone()).unwrap();
        view_peaks(samples, snapshot.channels, WaveformView::Merged, &mut low, &mut high);
        (low[BINS / 2], high[BINS / 2])
    });
    report("flat_copy", "preview_5s_window_1000_bins", &timing, false);
    let flat_window = (low.clone(), high.clone());
    view_peaks(&snapshot.samples, snapshot.channels, WaveformView::Merged, &mut low, &mut high);
    let flat_full = (low.clone(), high.clone());
    drop(snapshot);

    // The freeze the apps use now: the lanes of a view that shares the pages.
    let (timing, tracks) = measure(|| capture.freeze().tracks());
    report("shared_pages", "freeze", &timing, true);
    let track = &tracks[0];
    assert_eq!(track.frame_range(), history);
    let (timing, _) = measure(|| {
        track.waveform(WaveformView::Merged, history.clone(), &mut low, &mut high);
        (low[BINS / 2], high[BINS / 2])
    });
    report("shared_pages", "preview_full_range_1000_bins", &timing, false);
    assert!((&low, &high) == (&flat_full.0, &flat_full.1), "full range peaks differ");
    let (timing, _) = measure(|| {
        track.waveform(WaveformView::Merged, window.clone(), &mut low, &mut high);
        (low[BINS / 2], high[BINS / 2])
    });
    report("shared_pages", "preview_5s_window_1000_bins", &timing, false);
    assert!((&low, &high) == (&flat_window.0, &flat_window.1), "window peaks differ");
}

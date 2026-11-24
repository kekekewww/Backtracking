use std::{mem::size_of, sync::Barrier, thread};

use super::*;

fn wide_cache(page: &Page) -> &ChannelPeaks {
    match &page.peaks {
        PagePeaks::Wide(wide) => &wide.channels,
        PagePeaks::Traces(_) => panic!("expected a wide page"),
    }
}

fn read(page: &Page, layout: Layout, channel: usize, frames: Range<usize>) -> (f32, f32) {
    let mut bounds = Bounds::EMPTY;
    page.channel_extrema(layout, channel, frames, &mut bounds);
    bounds.result()
}

fn scan(page: &Page, layout: Layout, channel: usize, frames: Range<usize>) -> (f32, f32) {
    let mut low = f32::INFINITY;
    let mut high = f32::NEG_INFINITY;
    for frame in page.samples[frames.start * layout.channels..frames.end * layout.channels]
        .chunks_exact(layout.channels)
    {
        let sample = frame[channel];
        if sample.is_finite() {
            low = low.min(sample);
            high = high.max(sample);
        }
    }
    if low.is_finite() {
        (low, high)
    } else {
        (0.0, 0.0)
    }
}

fn audio(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            (0..channels)
                .map(move |channel| ((frame * 17 + channel * 13) % 503) as f32 / 251.0 - 1.0)
        })
        .collect()
}

#[test]
fn narrow_pages_keep_the_original_size_and_peak_allocation() {
    assert_eq!(size_of::<PagePeaks>(), size_of::<Vec<[Bounds; TRACES]>>());
    // The one addition is the pointer to the lazily built band cache.
    assert_eq!(
        size_of::<Page>(),
        size_of::<(u64, Vec<f32>, Vec<[Bounds; TRACES]>, Arc<()>)>()
    );
    for (a_channels, channels) in [(1, 1), (2, 2), (1, 2), (1, 3), (2, 4)] {
        let layout = Layout {
            page_frames: 1_024,
            channels,
            a_channels,
        };
        let page = Page::from_samples(0, &audio(1_024, channels), layout).unwrap();
        let original_allocation = page.peaks.as_ptr();
        let PagePeaks::Traces(traces) = &page.peaks else {
            panic!("mono/stereo lanes must not allocate a wide cache");
        };
        assert_eq!(traces.capacity(), 1_024 / PEAK_FRAMES);
        for channel in 0..channels {
            assert_eq!(
                read(&page, layout, channel, 0..1_024),
                scan(&page, layout, channel, 0..1_024)
            );
        }
        assert_eq!(page.peaks.as_ptr(), original_allocation);
        assert!(matches!(page.peaks, PagePeaks::Traces(_)));
    }
}

#[test]
fn extra_peaks_allocate_only_for_requested_channels_and_complete_blocks() {
    let layout = Layout {
        page_frames: 1_024,
        channels: 14,
        a_channels: 2,
    };
    let page = Page::from_samples(0, &audio(641, layout.channels), layout).unwrap();
    let cache = wide_cache(&page);
    assert!(cache.native.get().is_none());
    // Existing left/right traces and sample-level requests need no new cache.
    for channel in 0..layout.channels {
        for frames in [1..15, 17..31, 640..641] {
            assert_eq!(
                read(&page, layout, channel, frames.clone()),
                scan(&page, layout, channel, frames)
            );
        }
    }
    for channel in [0, 1, 2, 3] {
        read(&page, layout, channel, 0..641);
    }
    assert!(cache.native.get().is_none());
    read(&page, layout, 13, 128..512);
    let slots = cache.native.get().unwrap();
    assert_eq!(slots.len(), 10);
    for (index, slot) in slots.iter().enumerate() {
        let peaks = slot.lock().unwrap();
        if index == 9 {
            assert_eq!(peaks.fine.len(), 40);
            assert_eq!(peaks.fine.capacity(), 40);
            assert_eq!(peaks.coarse.len(), 5);
            assert_eq!(peaks.coarse.capacity(), 5);
        } else {
            assert_eq!(peaks.fine.len(), 0);
            assert_eq!(peaks.fine.capacity(), 0);
            assert_eq!(peaks.coarse.len(), 0);
            assert_eq!(peaks.coarse.capacity(), 0);
        }
    }
    read(&page, layout, 4, 0..512);
    assert_eq!(
        slots
            .iter()
            .filter(|slot| slot.lock().unwrap().fine.capacity() != 0)
            .count(),
        2
    );
}

#[test]
fn a_short_standalone_page_reserves_only_its_actual_audio_length() {
    let layout = Layout {
        page_frames: u32::MAX as usize,
        channels: 12,
        a_channels: 12,
    };
    let page = Page::from_samples(0, &audio(32, 12), layout).unwrap();
    assert_eq!(
        read(&page, layout, 11, 0..32),
        scan(&page, layout, 11, 0..32)
    );
    let peaks = wide_cache(&page).native.get().unwrap()[9].lock().unwrap();
    assert_eq!(peaks.fine.capacity(), 2);
    assert_eq!(peaks.coarse.capacity(), 0);
}

#[test]
fn open_page_prefixes_share_cold_caches_and_warmed_peak_storage() {
    let layout = Layout {
        page_frames: 1_024,
        channels: 12,
        a_channels: 12,
    };
    let page = Page::from_samples(0, &audio(337, 12), layout).unwrap();
    let short = page.prefix(200, 12);
    let long = page.prefix(337, 12);
    assert!(Arc::ptr_eq(wide_cache(&page), wide_cache(&short)));
    assert!(Arc::ptr_eq(wide_cache(&short), wide_cache(&long)));
    assert!(wide_cache(&page).native.get().is_none());
    assert_eq!(
        read(&short, layout, 11, 0..200),
        scan(&short, layout, 11, 0..200)
    );
    let (allocation, capacity) = {
        let peaks = wide_cache(&short).native.get().unwrap()[9].lock().unwrap();
        assert_eq!(peaks.fine.len(), 12);
        assert_eq!(peaks.coarse.len(), 1);
        (
            (peaks.fine.as_ptr(), peaks.coarse.as_ptr()),
            (peaks.fine.capacity(), peaks.coarse.capacity()),
        )
    };
    assert_eq!(
        read(&long, layout, 11, 0..337),
        scan(&long, layout, 11, 0..337)
    );
    {
        let peaks = wide_cache(&page).native.get().unwrap()[9].lock().unwrap();
        assert_eq!(peaks.fine.len(), 21);
        assert_eq!(peaks.coarse.len(), 2);
        assert_eq!((peaks.fine.as_ptr(), peaks.coarse.as_ptr()), allocation);
        assert_eq!((peaks.fine.capacity(), peaks.coarse.capacity()), capacity);
    }
    // A shorter prefix must ignore blocks built later by a longer sibling.
    assert_eq!(
        read(&short, layout, 11, 0..200),
        scan(&short, layout, 11, 0..200)
    );
}

#[test]
fn shared_prefix_queries_can_run_while_the_live_page_appends() {
    let mut history = History::new(1_024, 12, 12, 2_048).unwrap();
    for frame in audio(257, 12).chunks_exact(12) {
        history.ensure_append_capacity().unwrap();
        history.push(frame, 1.0);
    }
    let layout = history.layout;
    let short = history.open.as_ref().unwrap().prefix(129, 12);
    let long = history.open.as_ref().unwrap().prefix(257, 12);
    let expected_short = scan(&short, layout, 11, 0..129);
    let expected_long = scan(&long, layout, 11, 0..257);
    let barrier = Barrier::new(3);
    thread::scope(|scope| {
        scope.spawn(|| {
            barrier.wait();
            for _ in 0..500 {
                assert_eq!(read(&short, layout, 11, 0..129), expected_short);
            }
        });
        scope.spawn(|| {
            barrier.wait();
            for _ in 0..500 {
                assert_eq!(read(&long, layout, 11, 0..257), expected_long);
            }
        });
        barrier.wait();
        for frame in audio(643, 12).chunks_exact(12) {
            history.ensure_append_capacity().unwrap();
            history.push(frame, 1.0);
            let page = history.open.as_ref().unwrap();
            let frames = page.frames(12);
            assert_eq!(
                read(page, layout, 11, 0..frames),
                scan(page, layout, 11, 0..frames)
            );
        }
    });
    assert!(Arc::ptr_eq(
        wide_cache(history.open.as_ref().unwrap()),
        wide_cache(&short)
    ));
    assert_eq!(read(&short, layout, 11, 0..129), expected_short);
    assert_eq!(read(&long, layout, 11, 0..257), expected_long);
}

#[test]
fn reset_reuses_unique_cache_capacity_but_detaches_shared_prefixes() {
    let layout = Layout {
        page_frames: 1_024,
        channels: 12,
        a_channels: 12,
    };
    let mut page = Page::from_samples(0, &audio(512, 12), layout).unwrap();
    read(&page, layout, 11, 0..512);
    let owner = Arc::as_ptr(wide_cache(&page));
    let (allocation, capacity) = {
        let peaks = wide_cache(&page).native.get().unwrap()[9].lock().unwrap();
        (
            (peaks.fine.as_ptr(), peaks.coarse.as_ptr()),
            (peaks.fine.capacity(), peaks.coarse.capacity()),
        )
    };
    page.reset(2_048);
    assert_eq!(Arc::as_ptr(wide_cache(&page)), owner);
    {
        let peaks = wide_cache(&page).native.get().unwrap()[9].lock().unwrap();
        assert!(peaks.fine.is_empty());
        assert!(peaks.coarse.is_empty());
        assert_eq!((peaks.fine.capacity(), peaks.coarse.capacity()), capacity);
        assert_eq!((peaks.fine.as_ptr(), peaks.coarse.as_ptr()), allocation);
    }
    page.samples.extend_from_slice(&audio(256, 12));
    for block in 0..2 {
        let bounds = page.summarize(block, 12, 12);
        page.peaks.push(bounds);
    }
    read(&page, layout, 11, 0..256);
    let prefix = page.prefix(256, 12);
    let expected = scan(&prefix, layout, 11, 0..256);
    page.reset(4_096);
    assert!(!Arc::ptr_eq(wide_cache(&page), wide_cache(&prefix)));
    assert!(wide_cache(&page).native.get().is_none());
    assert_eq!(read(&prefix, layout, 11, 0..256), expected);
    let peaks = wide_cache(&prefix).native.get().unwrap()[9].lock().unwrap();
    assert_eq!((peaks.fine.as_ptr(), peaks.coarse.as_ptr()), allocation);
}

#[test]
fn fading_an_open_page_detaches_its_frozen_prefix_cache() {
    let mut history = History::new(1_024, 12, 12, 2_048).unwrap();
    for _ in 0..385 {
        history.ensure_append_capacity().unwrap();
        history.push(&[1.0; 12], 1.0);
    }
    let frozen = history.share(&history.frame_range());
    assert_eq!(history.channel_extrema(11, 0..385), (1.0, 1.0));
    let layout = history.layout;
    assert!(Arc::ptr_eq(
        wide_cache(history.open.as_ref().unwrap()),
        wide_cache(&frozen[0])
    ));
    history.fade_out_tail(130).unwrap();
    assert!(!Arc::ptr_eq(
        wide_cache(history.open.as_ref().unwrap()),
        wide_cache(&frozen[0])
    ));
    let live = history.open.as_ref().unwrap();
    assert_eq!(
        read(live, layout, 11, 256..384),
        scan(live, layout, 11, 256..384)
    );
    assert!(read(live, layout, 11, 256..384).1 < 1.0);
    assert_eq!(read(&frozen[0], layout, 11, 256..384), (1.0, 1.0));
}

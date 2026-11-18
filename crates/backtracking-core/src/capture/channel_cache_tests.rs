use std::ops::Range;

use super::{Capture, DurationChoice, FrozenCapture, FrozenTrack, Snapshot};

struct Random(u64);

impl Random {
    fn below(&mut self, limit: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % limit.max(1)
    }

    fn audio(&mut self, frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|_| match self.below(97) {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => f32::NEG_INFINITY,
                3 => -0.0,
                _ => (self.below(65_537) as f32 - 32_768.0) / 16_384.0,
            })
            .collect()
    }
}

fn scan(flat: &Snapshot, channel: usize, frames: Range<u64>) -> (f32, f32) {
    let first = frames.start.max(flat.first_frame);
    let end = frames.end.min(flat.frame_range().end);
    let mut low = f32::INFINITY;
    let mut high = f32::NEG_INFINITY;
    if channel < flat.channels && first < end {
        let first = (first - flat.first_frame) as usize;
        let end = (end - flat.first_frame) as usize;
        for frame in first..end {
            let sample = flat.samples[frame * flat.channels + channel];
            if sample.is_finite() {
                low = low.min(sample);
                high = high.max(sample);
            }
        }
    }
    if low.is_finite() {
        (low, high)
    } else {
        (0.0, 0.0)
    }
}

/// A separate sample scan, including bin boundaries and clipping, is the oracle.
fn scan_bins(
    flat: &Snapshot,
    channel: usize,
    frames: Range<u64>,
    low_len: usize,
    high_len: usize,
) -> (usize, Vec<f32>, Vec<f32>) {
    let mut low = vec![0.0; low_len];
    let mut high = vec![0.0; high_len];
    if channel >= flat.channels {
        return (0, low, high);
    }
    let first = frames.start.max(flat.first_frame);
    let end = frames.end.min(flat.frame_range().end);
    let count = end.saturating_sub(first) as usize;
    let bins = low_len.min(high_len);
    if count != 0 {
        for bin in 0..bins {
            let start = first + (bin as u128 * count as u128 / bins as u128) as u64;
            let stop = first + ((bin + 1) as u128 * count as u128 / bins as u128) as u64;
            (low[bin], high[bin]) = scan(flat, channel, start..stop.max(start + 1));
        }
    }
    (count, low, high)
}

fn check_range(
    live: Option<(&Capture, usize)>,
    track: &FrozenTrack,
    flat: &Snapshot,
    channel: usize,
    range: Range<u64>,
    lengths: (usize, usize),
) {
    let (count, expected_low, expected_high) =
        scan_bins(flat, channel, range.clone(), lengths.0, lengths.1);
    let mut low = vec![37.0; lengths.0];
    let mut high = vec![37.0; lengths.1];
    let context = format!(
        "{} channels, channel {channel}, requested {range:?}, retained {:?}, lengths {lengths:?}",
        flat.channels,
        flat.frame_range()
    );
    assert_eq!(
        track.channel_waveform(channel, range.clone(), &mut low, &mut high),
        count,
        "frozen count: {context}"
    );
    assert_eq!(low, expected_low, "frozen minima: {context}");
    assert_eq!(high, expected_high, "frozen maxima: {context}");
    assert_eq!(
        track.channel_extrema(channel, range.clone()),
        scan(flat, channel, range.clone()),
        "frozen extrema: {context}"
    );
    if let Some((capture, lane)) = live {
        low.fill(37.0);
        high.fill(37.0);
        assert_eq!(
            capture.track_channel_waveform_range(lane, channel, range, &mut low, &mut high),
            count,
            "live count: {context}"
        );
        assert_eq!(low, expected_low, "live minima: {context}");
        assert_eq!(high, expected_high, "live maxima: {context}");
    }
}

fn check_view(
    live: Option<&Capture>,
    view: &FrozenCapture,
    flats: &[Snapshot],
    random: &mut Random,
) {
    for (lane, flat) in flats.iter().enumerate() {
        let track = view.track(lane).unwrap();
        let range = flat.frame_range();
        let page = u64::from(flat.sample_rate);
        let live = live.map(|capture| (capture, lane));
        // Query every channel over full blocks before any subsequent mutation.
        for channel in 0..flat.channels {
            check_range(live, &track, flat, channel, 0..u64::MAX, (3, 4));
        }
        let channels = [0, flat.channels / 2, flat.channels - 1, flat.channels];
        let edges = [
            0..range.start,
            range.start.saturating_sub(129)..range.start.saturating_add(257),
            range.end.saturating_sub(page + 129)..range.end.saturating_add(91),
            range.end..u64::MAX,
            range.start..range.start,
            range.end..range.start,
        ];
        for channel in channels {
            for (index, request) in edges.iter().enumerate() {
                let lengths = [(0, 3), (1, 1), (7, 5), (257, 259), (19, 0), (9, 9)][index];
                check_range(live, &track, flat, channel, request.clone(), lengths);
            }
        }
        for _ in 0..24 {
            let span = range.end.saturating_sub(range.start) + page * 2 + 1;
            let origin = range.start.saturating_sub(page);
            let mut a = origin.saturating_add(random.below(span));
            let mut b = origin.saturating_add(random.below(span));
            if random.below(3) == 0 {
                a = a / 128 * 128;
                b = (b / page * page).saturating_add(random.below(3));
            }
            let request = if random.below(8) == 0 {
                a..a.saturating_add(1)
            } else {
                a.min(b)..a.max(b)
            };
            let bins = random.below(350) as usize;
            check_range(
                live,
                &track,
                flat,
                random.below(flat.channels as u64) as usize,
                request,
                (bins, bins + random.below(3) as usize),
            );
        }
    }
}

fn flats(capture: &Capture) -> Vec<Snapshot> {
    (0..2)
        .filter_map(|lane| capture.track_snapshot(lane))
        .collect()
}

#[test]
fn channel_cache_matches_scans_through_random_live_and_frozen_histories() {
    for (case, (rate, a, b)) in [
        (256, 1, None),
        (257, 2, None),
        (384, 2, Some(2)),
        (257, 12, None),
        (384, 40, None),
        (257, 64, None),
        (384, 12, Some(2)),
        (257, 2, Some(12)),
        (384, 24, Some(40)),
    ]
    .into_iter()
    .enumerate()
    {
        let channels = a + b.unwrap_or(0);
        let mut random = Random(0x28a3_1d5b_9301 + case as u64);
        let retained = rate as usize * 3 + 137;
        let mut capture = Capture::new_tracks_with_budget(
            rate,
            a,
            b,
            DurationChoice::Minutes10,
            retained * channels,
        )
        .unwrap();
        capture.set_retain_silence(true);
        let mut old: Vec<(FrozenCapture, Vec<Snapshot>)> = Vec::new();
        for step in 0..16 {
            match step {
                5 => capture.append_known_silence(129).unwrap(),
                8 => {
                    capture.pause();
                    capture.smooth_next_join();
                    capture.resume();
                    capture
                        .push_interleaved(&random.audio(137, channels))
                        .unwrap();
                }
                11 => capture
                    .append_known_silence((retained * 3 + 71) as u64)
                    .unwrap(),
                13 => capture.clear(),
                _ => {
                    let count = 1 + random.below((rate * 2) as u64) as usize;
                    capture
                        .push_interleaved(&random.audio(count, channels))
                        .unwrap();
                }
            }
            let view = capture.freeze();
            let expected = flats(&capture);
            check_view(Some(&capture), &view, &expected, &mut random);
            for (view, expected) in &old {
                check_view(None, view, expected, &mut random);
            }
            if step % 4 == 0 {
                old.push((view, expected));
                if old.len() > 2 {
                    old.remove(0);
                }
            }
        }
    }
}

#[test]
fn channel_cache_fades_invalidate_only_live_peaks_across_shared_page_boundary() {
    let mut random = Random(0x5353_1a23);
    // The three-frame join reaches the last complete peak block of a shared page.
    let mut capture = Capture::new(1_024, 64, DurationChoice::Seconds10).unwrap();
    capture.set_retain_silence(true);
    let mut audio = random.audio(3_073, 64);
    // Every faded sample is an extremum, so stale cached bounds cannot pass.
    audio[2_944 * 64..].fill(1.0);
    capture.push_interleaved(&audio).unwrap();
    let before = capture.freeze();
    let expected_before = flats(&capture);
    check_view(Some(&capture), &before, &expected_before, &mut random);
    capture.pause();
    capture.smooth_next_join();
    capture.resume();
    capture.push_interleaved(&[0.8; 7 * 64]).unwrap();
    let after = capture.freeze();
    assert_eq!(after.shared_pages(&before), 2);
    for channel in 0..64 {
        for request in [2_944..3_080, 3_069..3_074, 0..3_080] {
            check_range(
                Some((&capture, 0)),
                &after.track(0).unwrap(),
                &capture.track_snapshot(0).unwrap(),
                channel,
                request.clone(),
                (1, 1),
            );
            check_range(
                None,
                &before.track(0).unwrap(),
                &expected_before[0],
                channel,
                request,
                (1, 1),
            );
        }
    }
    check_view(Some(&capture), &after, &flats(&capture), &mut random);
    check_view(None, &before, &expected_before, &mut random);
}

#[test]
fn channel_cache_survives_retention_changes_and_partial_freezes() {
    let mut random = Random(0x4937_333a);
    let mut capture = Capture::new_tracks(257, 12, Some(2), DurationChoice::Seconds20).unwrap();
    capture.set_retain_silence(true);
    capture.push_interleaved(&random.audio(4_913, 14)).unwrap();
    let frozen = capture.freeze_range(131..4_777).unwrap();
    let expected: Vec<_> = (0..2)
        .map(|lane| capture.track_snapshot_range(lane, 131..4_777).unwrap())
        .collect();
    check_view(None, &frozen, &expected, &mut random);
    capture.set_retention(DurationChoice::Seconds10);
    capture.push_interleaved(&random.audio(317, 14)).unwrap();
    check_view(
        Some(&capture),
        &capture.freeze(),
        &flats(&capture),
        &mut random,
    );
    capture.set_retention(DurationChoice::Seconds30);
    capture.push_interleaved(&random.audio(3_171, 14)).unwrap();
    check_view(
        Some(&capture),
        &capture.freeze(),
        &flats(&capture),
        &mut random,
    );
    check_view(None, &frozen, &expected, &mut random);
    capture.clear();
    capture.push_interleaved(&random.audio(3_039, 14)).unwrap();
    check_view(
        Some(&capture),
        &capture.freeze(),
        &flats(&capture),
        &mut random,
    );
    check_view(None, &frozen, &expected, &mut random);
}

#[test]
fn channel_cache_standalone_pages_match_nonfinite_and_sample_level_scans() {
    let mut random = Random(0x9949_5dba);
    let mut audio = random.audio(1_173, 64);
    // Entire cached blocks can contain no finite values in the selected channel.
    for frame in 0..512 {
        audio[frame * 64 + 63] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY][frame % 3];
    }
    let first = u64::MAX - 5_000;
    let view = FrozenCapture::from_interleaved(257, 64, first, &audio).unwrap();
    let flat = Snapshot {
        sample_rate: 257,
        channels: 64,
        samples: audio,
        first_frame: first,
    };
    let track = view.track(0).unwrap();
    for channel in [0, 31, 63] {
        for relative in [0..512, 127..257, 255..259, 1_024..1_173, 1_170..1_173] {
            for lengths in [(1, 1), (3, 7), (2_000, 2_003), (0, 0)] {
                check_range(
                    None,
                    &track,
                    &flat,
                    channel,
                    first + relative.start..first + relative.end,
                    lengths,
                );
            }
        }
    }
    check_range(None, &track, &flat, 63, 0..u64::MAX, (23, 19));
    check_range(None, &track, &flat, 64, 0..u64::MAX, (23, 19));
}

#[test]
fn activity_uses_retained_edges_open_pages_and_frozen_audio() {
    let mut capture =
        Capture::new_tracks_with_budget(1000, 4, None, DurationChoice::Seconds10, 4000).unwrap();
    capture.set_retain_silence(true);
    let mut samples = vec![0.; 1001 * 4];
    samples[0] = 0.1;
    samples[5] = -0.000032;
    samples[6] = -0.000031;
    samples[4003] = 0.2;
    capture.push_interleaved(&samples).unwrap();
    assert_eq!(capture.active_channels(0), 0b1010);
    let frozen = capture.freeze().track(0).unwrap();
    capture.push_interleaved(&[0.; 4]).unwrap();
    assert_eq!(capture.active_channels(0), 0b1000);
    assert_eq!(frozen.active_channels(), 0b1010);
}

#[test]
#[ignore = "ten minutes of wide retained audio"]
fn activity_benchmark() {
    for width in [1, 2] {
        let mut capture = Capture::new(48000, width, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&vec![0.1; 480 * width]).unwrap();
        let now = std::time::Instant::now();
        for _ in 0..1_000_000 {
            std::hint::black_box(std::hint::black_box(&capture).active_channels(0));
        }
        eprintln!(
            "narrow width {width}: {} ns/query; cache bytes {:?}",
            now.elapsed().as_nanos() as f64 / 1e6,
            capture.history.channel_cache_bytes()
        );
        assert_eq!(capture.history.channel_cache_bytes(), (0, 0));
    }
    for width in [8, 12, 32] {
        let rate = 48000;
        let count = rate as usize * 600;
        let mut capture = Capture::new_tracks_with_budget(
            rate,
            width,
            None,
            DurationChoice::Minutes10,
            count * width,
        )
        .unwrap();
        capture.set_retain_silence(true);
        let mut packet = vec![0.; 800 * width];
        packet[0] = 0.1;
        for _ in 0..count / 800 {
            capture.push_interleaved(&packet).unwrap();
        }
        assert_eq!(capture.frame_range(), 0..count as u64);
        let now = std::time::Instant::now();
        let expected = 1;
        assert_eq!(capture.active_channels(0), expected);
        let cold = now.elapsed();
        let now = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(std::hint::black_box(&capture).active_channels(0), expected);
        }
        eprintln!(
            "{width} channels, 600 s, 48 kHz: cold {cold:?}, warm {:?}",
            now.elapsed() / 100
        );
    }
}

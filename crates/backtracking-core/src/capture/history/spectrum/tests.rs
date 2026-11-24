use std::cell::Cell;

use super::*;
use crate::waveform::{PairGroup, PairSide, SpectrumRow, WaveformView};
use crate::{Capture, DurationChoice, FrozenTrack};

thread_local! {
    /// Blocks run through the filters on this thread, pre-roll included.
    pub(super) static ANALYSED: Cell<usize> = const { Cell::new(0) };
}

fn analysed() -> usize {
    ANALYSED.with(Cell::get)
}

const MERGED: SpectrumRow<'static> = SpectrumRow::View(WaveformView::Merged);

/// A small deterministic generator of uniform samples in -1..1.
struct Random(u64);

impl Random {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

fn sine(rate: u32, frequency: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            (0.5 * (std::f64::consts::TAU * frequency * frame as f64 / f64::from(rate)).sin())
                as f32
        })
        .collect()
}

fn white(frames: usize, seed: u64) -> Vec<f32> {
    let mut random = Random(seed);
    (0..frames).map(|_| 0.3 * random.next()).collect()
}

/// Paul Kellet's refined pink filter, -3 dB per octave from about 10 Hz at 44.1 kHz.
fn pink(frames: usize, seed: u64) -> Vec<f32> {
    let mut random = Random(seed);
    let mut b = [0.0f32; 7];
    (0..frames)
        .map(|_| {
            let white = random.next();
            b[0] = 0.99886 * b[0] + white * 0.055_517_9;
            b[1] = 0.99332 * b[1] + white * 0.075_075_9;
            b[2] = 0.969 * b[2] + white * 0.153_852;
            b[3] = 0.8665 * b[3] + white * 0.310_485_6;
            b[4] = 0.55 * b[4] + white * 0.532_952_2;
            b[5] = -0.7616 * b[5] - white * 0.016_898;
            let pink = b.iter().sum::<f32>() + white * 0.5362;
            b[6] = white * 0.115_926;
            pink * 0.05
        })
        .collect()
}

/// A capture that keeps everything it is given, as `channels` interleaved channels.
fn capture_of(rate: u32, channels: usize, interleaved: &[f32]) -> Capture {
    let mut capture = Capture::new(rate, channels, DurationChoice::Seconds60).unwrap();
    capture.set_retain_silence(true);
    capture.push_interleaved(interleaved).unwrap();
    capture
}

fn spectrum(
    capture: &Capture,
    row: SpectrumRow<'_>,
    range: Range<u64>,
    bins: usize,
) -> Vec<SpectrumBin> {
    let mut out = vec![Some([-1.0; BANDS]); bins];
    capture.track_spectrum_range(0, row, range, &mut out);
    out
}

fn shares(energies: &BandEnergies) -> [f64; BANDS] {
    let total: f64 = energies.iter().map(|&energy| f64::from(energy)).sum();
    energies.map(|energy| f64::from(energy) / total)
}

fn bits(bins: &[SpectrumBin]) -> Vec<Option<[u32; BANDS]>> {
    bins.iter()
        .map(|bin| bin.map(|energies| energies.map(f32::to_bits)))
        .collect()
}

#[test]
fn a_low_sine_fills_the_lowest_band_and_a_high_one_the_highest_in_every_block() {
    for rate in [44_100, 48_000, 96_000, 192_000, 384_000] {
        for (frequency, band) in [(60.0, 0), (10_000.0, BANDS - 1)] {
            let frames = rate as usize * 5 / 2;
            let capture = capture_of(rate, 1, &sine(rate, frequency, frames));
            // One bin per block, across both page boundaries.
            let blocks = frames / BLOCK;
            let bins = spectrum(&capture, MERGED, 0..(blocks * BLOCK) as u64, blocks);
            let worst = bins
                .iter()
                .map(|bin| shares(&bin.unwrap())[band])
                .enumerate()
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert!(
                worst.1 > 0.98,
                "{rate} Hz, {frequency} Hz, block {}: {}",
                worst.0,
                worst.1
            );
        }
    }
}

#[test]
fn every_sample_rate_puts_tones_in_the_bands_it_can_hold() {
    for (rate, frequency, band) in [
        (8_000, 60.0, 0),
        (8_000, 3_000.0, 4),
        (11_025, 400.0, 2),
        (22_050, 7_000.0, 5),
        (32_000, 150.0, 1),
        (88_200, 1_200.0, 3),
        (176_400, 15_000.0, 5),
    ] {
        let frames = rate as usize * 3 / 2;
        let capture = capture_of(rate, 1, &sine(rate, frequency, frames));
        for bin in spectrum(&capture, MERGED, 0..frames as u64, 24) {
            let share = shares(&bin.unwrap())[band];
            assert!(share > 0.8, "{rate} Hz, {frequency} Hz: {share}");
        }
    }
    // Too low a rate to hold even the lowest band.
    let capture = capture_of(40, 1, &sine(40, 5.0, 200));
    assert_eq!(spectrum(&capture, MERGED, 0..200, 4), vec![None; 4]);
}

#[test]
fn pink_noise_is_roughly_flat_and_white_noise_leans_high() {
    for rate in [44_100, 48_000, 96_000] {
        let frames = rate as usize * 4;
        let pink = capture_of(rate, 1, &pink(frames, 7));
        let bin = spectrum(&pink, MERGED, rate as u64 / 2..frames as u64, 1)[0].unwrap();
        let flat = shares(&bin);
        for (band, &share) in flat.iter().enumerate() {
            assert!(
                (0.85..1.15).contains(&(share * BANDS as f64)),
                "{rate} Hz pink band {band}: {flat:?}"
            );
        }
        let white = capture_of(rate, 1, &white(frames, 9));
        let bin = spectrum(&white, MERGED, 0..frames as u64, 1)[0].unwrap();
        let rising = shares(&bin);
        assert!(
            rising.windows(2).all(|pair| pair[0] < pair[1]),
            "{rate} Hz white: {rising:?}"
        );
        assert!(rising[BANDS - 1] > 0.4, "{rate} Hz white: {rising:?}");
    }
}

#[test]
fn silence_gaps_and_invalid_rows_have_no_data() {
    let rate = 48_000;
    let mut audio = sine(rate, 300.0, rate as usize);
    audio.resize(rate as usize * 3, 0.0);
    let capture = capture_of(rate, 1, &audio);
    let bins = spectrum(&capture, MERGED, 0..audio.len() as u64, 30);
    assert!(bins[..10].iter().all(Option::is_some));
    // The smoothing of the lowest band dies away within a tenth of a second.
    assert!(bins[11..].iter().all(Option::is_none), "{bins:?}");
    // Frames outside the history, a missing lane and missing channels.
    let mut out = vec![Some([1.0; BANDS]); 8];
    assert_eq!(
        capture.track_spectrum_range(0, MERGED, 200_000..300_000, &mut out),
        0
    );
    assert_eq!(out, vec![None; 8]);
    for (lane, row) in [
        (1, MERGED),
        (0, SpectrumRow::Channel(1)),
        (
            0,
            SpectrumRow::Pair(
                &PairGroup::new(0, &["FL".into(), "FR".into(), "FC".into()]).unwrap(),
                PairSide::Left,
            ),
        ),
    ] {
        out.fill(Some([1.0; BANDS]));
        assert_eq!(
            capture.track_spectrum_range(lane, row, 0..1_000, &mut out),
            0
        );
        assert_eq!(out, vec![None; 8]);
    }
    // Known silence appended after a long gap keeps no colour either.
    let mut gap = Capture::new(rate, 2, DurationChoice::Seconds10).unwrap();
    gap.set_retain_silence(true);
    gap.push_interleaved(&[0.25; 2_000]).unwrap();
    gap.append_known_silence(rate as u64 * 20).unwrap();
    let range = gap.frame_range();
    let mut out = vec![Some([1.0; BANDS]); 16];
    assert_eq!(
        gap.track_spectrum_range(0, MERGED, range.clone(), &mut out),
        (range.end - range.start) as usize
    );
    assert_eq!(out, vec![None; 16]);
}

/// Music-like audio: a bass line, a chord, hats and noise.
fn mixture(rate: u32, frames: usize, channels: usize) -> Vec<f32> {
    let mut random = Random(3);
    (0..frames)
        .flat_map(|frame| {
            let t = frame as f64 / f64::from(rate);
            let bass = 0.4 * (std::f64::consts::TAU * 55.0 * t).sin();
            let chord = 0.2 * (std::f64::consts::TAU * 440.0 * t).sin() * (t * 3.0).fract();
            let hat = if (t * 8.0).fract() < 0.05 { 0.3 } else { 0.01 };
            let noise = random.next() as f64;
            (0..channels)
                .map(move |channel| (bass + chord + hat * noise * (channel + 1) as f64) as f32)
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn values_do_not_depend_on_read_order_ranges_bin_counts_or_page_seams() {
    let rate = 48_000;
    let frames = rate as usize * 7 / 2;
    let audio = mixture(rate, frames, 2);
    let reads: [(Range<u64>, usize); 4] = [
        (0..frames as u64, 997),
        (100_000..150_000, 4_096),
        (47_000..49_000, 2_000),
        (12_345..12_346, 3),
    ];
    let forward = capture_of(rate, 2, &audio);
    let backward = capture_of(rate, 2, &audio);
    let row = SpectrumRow::View(WaveformView::Right);
    let first: Vec<_> = reads
        .iter()
        .map(|(range, bins)| bits(&spectrum(&forward, row, range.clone(), *bins)))
        .collect();
    let mut second: Vec<_> = reads
        .iter()
        .rev()
        .map(|(range, bins)| bits(&spectrum(&backward, row, range.clone(), *bins)))
        .collect();
    second.reverse();
    assert_eq!(first, second);
    // One bin per frame across a page seam equals the two halves read apart, and a
    // block's value is the same whichever bin holds it.
    let seam = rate as u64;
    let joined = spectrum(&forward, row, seam - 300..seam + 300, 600);
    let mut halves = spectrum(&backward, row, seam - 300..seam, 300);
    halves.extend(spectrum(&backward, row, seam..seam + 300, 300));
    assert_eq!(bits(&joined), bits(&halves));
    // Frames 47_700 to 48_000 end the first page's blocks 372, 373 and 374.
    let block = spectrum(&forward, row, seam - 128..seam, 1);
    assert_eq!(bits(&block), bits(&joined[172..173]));
    for blocks in [0..44, 44..172, 172..300, 300..428, 428..556, 556..600] {
        assert!(joined[blocks.clone()]
            .iter()
            .all(|bin| bin == &joined[blocks.start]));
    }
    assert_eq!(
        joined.windows(2).filter(|pair| pair[0] != pair[1]).count(),
        5
    );
}

#[test]
fn live_and_frozen_reads_share_one_cache_and_agree_with_a_copy_of_the_audio() {
    let rate = 44_100;
    let frames = rate as usize * 2 + 7_001;
    let audio = mixture(rate, frames, 2);
    let capture = capture_of(rate, 2, &audio);
    let range = 0..frames as u64;
    let live = spectrum(&capture, MERGED, range.clone(), 1_500);
    let track = capture.freeze().track(0).unwrap();
    let before = analysed();
    let mut frozen = vec![None; 1_500];
    assert_eq!(track.spectrum(MERGED, range.clone(), &mut frozen), frames);
    assert_eq!(analysed(), before, "the frozen copy reuses the live cache");
    assert_eq!(bits(&frozen), bits(&live));
    // Pages of their own, analysed separately, give the same values.
    let copy = FrozenTrack::from_interleaved(rate, 2, 0, &audio).unwrap();
    let mut separate = vec![None; 1_500];
    copy.spectrum(MERGED, range, &mut separate);
    assert_eq!(bits(&separate), bits(&live));
    assert!(analysed() > before);
}

#[test]
fn the_cache_is_built_only_by_reads_only_for_asked_channels_and_reused() {
    let rate = 48_000;
    let frames = rate as usize * 3;
    let mut capture = capture_of(rate, 2, &mixture(rate, frames, 2));
    let pages = |capture: &Capture| -> Vec<Arc<BandCache>> {
        let history = &capture.history;
        history
            .sealed
            .iter()
            .map(|page| Arc::clone(&page.bands))
            .chain(history.open.iter().map(|page| Arc::clone(&page.bands)))
            .collect()
    };
    let built = |cache: &BandCache, channel: usize| {
        cache
            .channels
            .get()
            .is_some_and(|slots| !slots[channel].lock().unwrap().blocks.is_empty())
    };
    let mut low = [0.0; 512];
    let mut high = [0.0; 512];
    capture.track_waveform_range(
        0,
        WaveformView::Merged,
        0..frames as u64,
        &mut low,
        &mut high,
    );
    capture.track_channel_waveform_range(0, 1, 0..frames as u64, &mut low, &mut high);
    assert!(pages(&capture)
        .iter()
        .all(|cache| cache.channels.get().is_none()));
    assert_eq!(capture.spectrum_cache_pages(), 0);
    let before = analysed();
    let left = spectrum(
        &capture,
        SpectrumRow::View(WaveformView::Left),
        0..frames as u64,
        700,
    );
    let work = analysed() - before;
    assert!(work >= frames / BLOCK);
    assert!(pages(&capture)
        .iter()
        .all(|cache| built(cache, 0) && !built(cache, 1)));
    assert_eq!(capture.spectrum_cache_pages(), pages(&capture).len());
    // Sealed pages drop their filter state and keep exactly one value per block.
    for cache in &pages(&capture)[..3] {
        let slot = cache.channels.get().unwrap()[0].lock().unwrap();
        assert!(slot.chain.is_none());
        assert_eq!((slot.blocks.len(), slot.blocks.capacity()), (375, 375));
    }
    let again = spectrum(&capture, SpectrumRow::Channel(0), 0..frames as u64, 700);
    assert_eq!(analysed() - before, work);
    assert_eq!(bits(&again), bits(&left));
    // Clearing keeps nothing from before, and new audio starts a fresh cache.
    capture.clear();
    capture.push_interleaved(&mixture(rate, 1_000, 2)).unwrap();
    let range = capture.frame_range();
    let fresh = spectrum(&capture, SpectrumRow::Channel(0), range.clone(), 8);
    assert!(fresh.iter().all(Option::is_some));
}

#[test]
fn the_open_page_extends_as_audio_arrives_and_ends_where_one_late_read_does() {
    let rate = 22_050;
    let frames = rate as usize * 2 + 3_333;
    let audio = mixture(rate, frames, 2);
    let mut growing = Capture::new(rate, 2, DurationChoice::Seconds60).unwrap();
    growing.set_retain_silence(true);
    let mut pushed = 0;
    let mut chunk = 61;
    while pushed < frames {
        let count = chunk.min(frames - pushed);
        growing
            .push_interleaved(&audio[pushed * 2..(pushed + count) * 2])
            .unwrap();
        pushed += count;
        chunk = chunk * 3 % 1_999 + 1;
        let all = spectrum(&growing, MERGED, 0..pushed as u64, 97);
        assert!(all.iter().all(Option::is_some));
        // Past the page's first 100 ms, its newest, partial block repeats the last complete one.
        let page = (pushed as u64 - 1) / u64::from(rate) * u64::from(rate);
        let partial = page + (pushed as u64 - page) / BLOCK as u64 * BLOCK as u64;
        if partial != pushed as u64 && pushed as u64 - page > 2_400 {
            assert_eq!(
                spectrum(&growing, MERGED, partial..pushed as u64, 1),
                spectrum(&growing, MERGED, partial - BLOCK as u64..partial, 1)
            );
        }
    }
    let late = capture_of(rate, 2, &audio);
    let range = 0..frames as u64;
    assert_eq!(
        bits(&spectrum(&growing, MERGED, range.clone(), 4_096)),
        bits(&spectrum(&late, MERGED, range, 4_096))
    );
}

#[test]
fn frozen_prefixes_extend_the_shared_cache_and_edits_detach_it() {
    let layout = Layout {
        page_frames: 9_600,
        channels: 1,
        a_channels: 1,
    };
    let mut history = History::new(layout.page_frames, 1, 1, 48_000).unwrap();
    let audio = mixture(48_000, 8_000, 1);
    let append = |history: &mut History, samples: &[f32]| {
        for sample in samples {
            history.ensure_append_capacity().unwrap();
            history.push(std::slice::from_ref(sample), 1.0);
        }
    };
    append(&mut history, &audio[..6_000]);
    let old = history.share(&history.frame_range());
    let read = |page: &Page, range: Range<u64>| {
        let mut out = vec![None; 40];
        let held = page.overlap(&range, 1);
        bins(
            std::iter::once((page, held)),
            layout,
            48_000,
            &[(0, 1.0)],
            range,
            &mut out,
        );
        bits(&out)
    };
    let cached = |page: &Page| {
        page.bands.channels.get().unwrap()[0]
            .lock()
            .unwrap()
            .blocks
            .len()
    };
    let old_values = read(&old[0], 0..6_000);
    assert_eq!(cached(&old[0]), 6_000 / BLOCK);
    append(&mut history, &audio[6_000..]);
    let open = history.open.as_ref().unwrap();
    assert!(Arc::ptr_eq(&old[0].bands, &open.bands));
    // The live page continues the blocks the frozen prefix cached. Only the last bin, which
    // holds the prefix's partial block, differs.
    assert_eq!(read(open, 0..6_000)[..39], old_values[..39]);
    assert_ne!(read(open, 0..6_000)[39], old_values[39]);
    assert_eq!(cached(&old[0]), 8_000 / BLOCK);
    assert_eq!(read(&old[0], 0..6_000), old_values);
    let unfaded = read(open, 7_500..8_000);
    history.fade_out_tail(500).unwrap();
    let open = history.open.as_ref().unwrap();
    assert!(!Arc::ptr_eq(&old[0].bands, &open.bands));
    assert_ne!(read(open, 7_500..8_000), unfaded);
    assert_eq!(read(open, 0..6_000)[..39], old_values[..39]);
    assert_eq!(read(&old[0], 0..6_000), old_values);
}

#[test]
fn rows_of_several_channels_sum_their_members_energies() {
    let rate = 48_000;
    let frames = rate as usize * 3 / 2;
    let channels = 6;
    let audio: Vec<f32> = mixture(rate, frames, 1)
        .into_iter()
        .zip(white(frames, 5))
        .zip(sine(rate, 70.0, frames))
        .flat_map(|((music, noise), bass)| {
            [music, noise, bass, music * 0.5, noise * 0.25, bass * 2.0]
        })
        .collect();
    let mut capture =
        Capture::new_tracks(rate, 2, Some(channels), DurationChoice::Seconds10).unwrap();
    capture.set_retain_silence(true);
    let stereo = sine(rate, 1_000.0, frames);
    let interleaved: Vec<f32> = stereo
        .iter()
        .zip(audio.chunks_exact(channels))
        .flat_map(|(&a, b)| [a, -a].into_iter().chain(b.iter().copied()))
        .collect();
    capture.push_interleaved(&interleaved).unwrap();
    let range = 0..frames as u64;
    let read = |lane, row| {
        let mut out = vec![None; 50];
        capture.track_spectrum_range(lane, row, range.clone(), &mut out);
        out.into_iter().map(Option::unwrap).collect::<Vec<_>>()
    };
    let single: Vec<_> = (0..channels)
        .map(|channel| read(1, SpectrumRow::Channel(channel)))
        .collect();
    let close = |actual: &[BandEnergies], expected: &[[f64; BANDS]]| {
        for (actual, expected) in actual.iter().zip(expected) {
            for (&actual, &expected) in actual.iter().zip(expected) {
                assert!(
                    (f64::from(actual) - expected).abs() <= 1e-5 * expected.max(1e-6),
                    "{actual} {expected}"
                );
            }
        }
    };
    let sum = |weights: &[f32]| -> Vec<[f64; BANDS]> {
        (0..50)
            .map(|bin| {
                let mut total = [0.0; BANDS];
                for (channel, &weight) in weights.iter().enumerate() {
                    for (total, &energy) in total.iter_mut().zip(&single[channel][bin]) {
                        *total += f64::from(energy) * f64::from(weight);
                    }
                }
                total
            })
            .collect()
    };
    close(&read(1, MERGED), &sum(&[1.0; 6]));
    let labels: Vec<String> = ["FL", "FR", "FC", "LFE", "Ls", "Rs"]
        .map(String::from)
        .into();
    let group = PairGroup::new(0, &labels).unwrap();
    for side in [PairSide::Left, PairSide::Right] {
        let weights: Vec<f32> = group
            .weights
            .iter()
            .map(|w| w[side as usize].powi(2))
            .collect();
        close(&read(1, SpectrumRow::Pair(&group, side)), &sum(&weights));
    }
    // Lane A's left and right views are its first and second channels.
    assert_eq!(
        read(0, SpectrumRow::View(WaveformView::Left)),
        read(0, SpectrumRow::Channel(0))
    );
    assert_eq!(
        read(0, SpectrumRow::View(WaveformView::Right)),
        read(0, SpectrumRow::Channel(1))
    );
    assert_eq!(read(1, SpectrumRow::View(WaveformView::Right)), single[1]);
}

/// Run with `cargo test --release --workspace spectrum_analysis_cost -- --ignored --nocapture`.
#[test]
#[ignore = "release timing probe with 230 MB of audio"]
fn spectrum_analysis_cost() {
    use std::time::Instant;
    let rate = 48_000;
    let seconds = 600;
    let mut capture = Capture::new(rate, 2, DurationChoice::Minutes10).unwrap();
    capture.set_retain_silence(true);
    let second = mixture(rate, rate as usize, 2);
    for _ in 0..seconds {
        capture.push_interleaved(&second).unwrap();
    }
    let range = capture.frame_range();
    assert_eq!(range.end - range.start, u64::from(rate) * seconds);
    let mut out = vec![None; 4_096];
    for (phase, row) in [
        ("cold, both channels", MERGED),
        ("warm, both channels", MERGED),
        ("warm, one channel", SpectrumRow::Channel(0)),
    ] {
        let started = Instant::now();
        capture.track_spectrum_range(0, row, range.clone(), &mut out);
        println!(
            "spectrum 10 min stereo 48 kHz, {phase}: {:.1} ms",
            started.elapsed().as_secs_f64() * 1e3
        );
    }
    let bytes: usize = capture
        .history
        .sealed
        .iter()
        .map(|page| &**page)
        .chain(capture.history.open.iter())
        .flat_map(|page| page.bands.channels.get().unwrap().iter())
        .map(|slot| {
            let slot = slot.lock().unwrap();
            slot.blocks.capacity() * std::mem::size_of::<BandEnergies>()
                + slot
                    .chain
                    .as_ref()
                    .map_or(0, |_| std::mem::size_of::<Chain>())
        })
        .sum();
    println!(
        "spectrum cache: {bytes} bytes, {:.0} bytes per channel-second",
        bytes as f64 / (2 * seconds) as f64
    );
}

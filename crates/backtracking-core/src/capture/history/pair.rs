use super::*;
use crate::spatial::fold_down;
use crate::waveform::{PairGroup, PairSide};

const PAIR_BLOCK: usize = 32;

pub(super) struct PairBlocks {
    group: PairGroup,
    peaks: Vec<[Bounds; 2]>,
}

struct Fold<'a> {
    group: &'a PairGroup,
    lane: usize,
    stride: usize,
    offset: usize,
}

impl Fold<'_> {
    fn scan(&self, page: &Page, frames: Range<usize>) -> [Bounds; 2] {
        let mut peaks = [Bounds::EMPTY; 2];
        let weights = &self.group.weights;
        for frame in page.samples(frames, self.stride).chunks_exact(self.stride) {
            let mut stereo = [0.; 2];
            fold_down(
                &frame[self.offset..self.offset + weights.len()],
                weights,
                &mut stereo,
            )
            .expect("one complete group frame");
            peaks[0].add(stereo[0]);
            peaks[1].add(stereo[1]);
        }
        peaks
    }

    fn extrema(&self, page: &Page, frames: Range<usize>, side: usize, result: &mut Bounds) {
        let first = frames.start.div_ceil(PAIR_BLOCK);
        let last = frames.end / PAIR_BLOCK;
        if first >= last {
            result.combine(self.scan(page, frames)[side]);
            return;
        }
        let PagePeaks::Wide(wide) = &page.peaks else {
            unreachable!("pair on a narrow lane")
        };
        let slots = wide.channels.pairs.get_or_init(Box::default);
        let mut slot = slots[self.lane].lock().unwrap_or_else(|e| e.into_inner());
        if !slot
            .as_ref()
            .is_some_and(|cache| cache.group.matches(self.group))
        {
            *slot = Some(PairBlocks {
                group: self.group.clone(),
                peaks: Vec::with_capacity(wide.capacity_frames / PAIR_BLOCK),
            });
        }
        let blocks = &mut slot.as_mut().unwrap().peaks;
        for block in blocks.len()..page.frames(self.stride) / PAIR_BLOCK {
            blocks.push(self.scan(page, block * PAIR_BLOCK..(block + 1) * PAIR_BLOCK));
        }
        for peak in &blocks[first..last] {
            result.combine(peak[side]);
        }
        drop(slot);
        result.combine(self.scan(page, frames.start..first * PAIR_BLOCK)[side]);
        result.combine(self.scan(page, last * PAIR_BLOCK..frames.end)[side]);
    }
}

pub(in crate::capture) fn extrema<'a>(
    pages: impl Iterator<Item = (&'a Page, Range<usize>)>,
    layout: Layout,
    lane: usize,
    group: &PairGroup,
    side: PairSide,
) -> (f32, f32) {
    let fold = Fold {
        group,
        stride: layout.channels,
        lane,
        offset: group.first + if lane == 0 { 0 } else { layout.a_channels },
    };
    let mut bounds = Bounds::EMPTY;
    for (page, frames) in pages {
        fold.extrema(page, frames, side as usize, &mut bounds);
    }
    bounds.result()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capture, DurationChoice, FrozenTrack};
    use std::time::Instant;

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pair_bins_match_folded_samples_across_recycling_and_layout_changes() {
        let mut cap =
            Capture::new_tracks_with_budget(257, 2, Some(6), DurationChoice::Seconds20, 8 * 903)
                .unwrap();
        cap.set_retain_silence(true);
        let groups = [
            PairGroup::new(1, &labels(&["FL", "FR", "FC", "LFE"])).unwrap(),
            PairGroup::new(1, &labels(&["FR", "FC", "LFE", "FL"])).unwrap(),
        ];
        let mut saved = None;
        let mut rng = 1297u32;
        let mut next = || {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            rng
        };
        for step in 0..8 {
            let raw: Vec<_> = (0..8 * 339)
                .map(|_| next() as i32 as f32 / i32::MAX as f32)
                .collect();
            cap.push_interleaved(&raw).unwrap();
            let track = cap.freeze().track(1).unwrap();
            for group in &groups {
                let flat = track.snapshot();
                let group_pcm: Vec<_> = flat
                    .samples
                    .chunks_exact(6)
                    .flat_map(|f| f[1..5].iter().copied())
                    .collect();
                let mut stereo = vec![0.; track.frame_count() * 2];
                fold_down(&group_pcm, &group.weights, &mut stereo).unwrap();
                let reference =
                    FrozenTrack::from_interleaved(257, 2, track.first_frame(), &stereo).unwrap();
                for i in 0..24 {
                    let begin = track.first_frame().saturating_sub(33) + u64::from(next() % 950);
                    let range = if i == 0 {
                        0..track.frame_range().end + 12
                    } else {
                        begin..begin + u64::from(next() % 600)
                    };
                    let bins = if i == 0 { 1 } else { (next() % 293) as usize };
                    for (ch, side) in [PairSide::Left, PairSide::Right].into_iter().enumerate() {
                        let (mut lo, mut hi) = (vec![0.; bins], vec![0.; bins + 1]);
                        let n = reference.channel_waveform(ch, range.clone(), &mut lo, &mut hi);
                        let expected = (n, lo.clone(), hi.clone());
                        let n = cap.track_pair_waveform_range(
                            1,
                            group,
                            side,
                            range.clone(),
                            &mut lo,
                            &mut hi,
                        );
                        assert_eq!((n, lo.clone(), hi.clone()), expected);
                        let n = track.pair_waveform(group, side, range.clone(), &mut lo, &mut hi);
                        assert_eq!((n, lo, hi), expected);
                    }
                }
            }
            if step == 1 {
                let mut extremes = ([0.; 1], [0.; 1]);
                track.pair_waveform(
                    &groups[0],
                    PairSide::Right,
                    track.frame_range(),
                    &mut extremes.0,
                    &mut extremes.1,
                );
                saved = Some((track, extremes));
            }
        }
        let (track, expected) = saved.unwrap();
        assert!(cap.frame_range().start > track.frame_range().end);
        let mut result = ([0.; 1], [0.; 1]);
        track.pair_waveform(
            &groups[0],
            PairSide::Right,
            track.frame_range(),
            &mut result.0,
            &mut result.1,
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn frozen_prefixes_extend_the_same_cache_and_edits_detach_it() {
        let mut history = History::new(257, 3, 3, 1028).unwrap();
        let group = PairGroup::new(0, &labels(&["FL", "FR", "FC"])).unwrap();
        let append = |history: &mut History, n: usize| {
            for _ in 0..n {
                history.ensure_append_capacity().unwrap();
                history.push(&[0.5, -0.25, 0.125], 1.);
            }
        };
        append(&mut history, 99);
        let old = history.share(&history.frame_range());
        let layout = history.layout;
        let read = |page: &Page, count| {
            extrema(
                std::iter::once((page, 0..count)),
                layout,
                0,
                &group,
                PairSide::Left,
            )
        };
        let old_peaks = read(&old[0], 99);
        append(&mut history, 101);
        let shared = |page: &Page| match &page.peaks {
            PagePeaks::Wide(wide) => Arc::clone(&wide.channels),
            _ => unreachable!(),
        };
        let cache = shared(&old[0]);
        assert!(Arc::ptr_eq(&cache, &shared(history.open.as_ref().unwrap())));
        assert_eq!(read(history.open.as_ref().unwrap(), 200), old_peaks);
        assert_eq!(
            cache.pairs.get().unwrap()[0]
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .peaks
                .len(),
            200 / PAIR_BLOCK
        );
        history.fade_out_tail(150).unwrap();
        let live = history.open.as_ref().unwrap();
        assert!(!Arc::ptr_eq(&cache, &shared(live)));
        assert_eq!(read(live, 200).0, 0.);
        assert_eq!(read(&old[0], 99), old_peaks);
    }

    #[test]
    #[ignore = "release benchmark with up to 3.3 GB of audio"]
    fn pair_redraw_cost() {
        let rate = 48000;
        for width in [12, 28] {
            let names: Vec<_> = [
                "FL", "FR", "FC", "LFE", "Ls", "Rs", "Lb", "Rb", "Ltf", "Rtf", "Ltr", "Rtr",
            ]
            .into_iter()
            .cycle()
            .take(width)
            .collect();
            let group = PairGroup::new(0, &labels(&names)).unwrap();
            let pcm: Vec<_> = (0..rate * width).map(|i| (i % 331) as f32 / 331.).collect();
            for seconds in [10, 60, 600] {
                let mut cap = Capture::new_with_budget(
                    rate as u32,
                    width,
                    DurationChoice::Minutes10,
                    seconds * rate * width,
                )
                .unwrap();
                cap.set_retain_silence(true);
                for _ in 0..seconds {
                    cap.push_interleaved(&pcm).unwrap();
                }
                let track = cap.freeze().track(0).unwrap();
                assert_eq!(track.frame_count(), seconds * rate);
                let mut lo = [0.; 2000];
                let mut hi = [0.; 2000];
                for phase in ["cold", "warm"] {
                    let start = Instant::now();
                    for side in [PairSide::Left, PairSide::Right] {
                        track.pair_waveform(&group, side, track.frame_range(), &mut lo, &mut hi);
                        std::hint::black_box((&lo, &hi));
                    }
                    println!(
                        "pair width={width} seconds={seconds} phase={phase} ms={:.3}",
                        start.elapsed().as_secs_f64() * 1000.
                    );
                }
                let bytes: usize = cap
                    .history
                    .sealed
                    .iter()
                    .map(AsRef::as_ref)
                    .chain(cap.history.open.iter())
                    .map(|page| {
                        let PagePeaks::Wide(wide) = &page.peaks else {
                            unreachable!()
                        };
                        let slots = wide.channels.pairs.get().unwrap();
                        slots[0].lock().unwrap().as_ref().unwrap().peaks.capacity()
                            * std::mem::size_of::<[Bounds; 2]>()
                            + std::mem::size_of_val(&**slots)
                    })
                    .sum();
                println!("pair width={width} seconds={seconds} cache_bytes={bytes}");
            }
        }
    }
}

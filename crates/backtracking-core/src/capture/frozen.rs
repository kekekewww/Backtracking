use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use super::history::{self, Layout, Page};
use super::{waveform_bins, CaptureError, Snapshot};
use crate::waveform::WaveformView;

/// An immutable view of the audio a capture retained at one moment.
///
/// It shares the capture's complete one-second pages instead of copying them, so
/// taking one costs a pointer per retained second plus a copy of the page still being
/// written, and cloning it costs one reference count. Nothing the capture does
/// afterwards changes what the view reads. The memory of a page is released when
/// neither the capture nor any view refers to it.
#[derive(Clone)]
pub struct FrozenCapture {
    sample_rate: u32,
    layout: Layout,
    /// The pages holding `frames`, oldest first and without gaps.
    pages: Arc<[Arc<Page>]>,
    frames: Range<u64>,
}

impl FrozenCapture {
    pub(super) fn new(
        sample_rate: u32,
        layout: Layout,
        pages: Arc<[Arc<Page>]>,
        frames: Range<u64>,
    ) -> Self {
        Self {
            sample_rate,
            layout,
            pages,
            frames,
        }
    }

    /// Copies `interleaved` into a view of its own, as one lane whose first frame sits
    /// at `first_frame` on the retained-audio timeline. This is for audio that does not
    /// come from a live capture; [`Capture::freeze`](super::Capture::freeze) never copies.
    pub fn from_interleaved(
        sample_rate: u32,
        channels: usize,
        first_frame: u64,
        interleaved: &[f32],
    ) -> Result<Self, CaptureError> {
        if sample_rate == 0 {
            return Err(CaptureError::InvalidSampleRate);
        }
        if channels == 0 {
            return Err(CaptureError::InvalidChannelCount);
        }
        if interleaved.len() % channels != 0 {
            return Err(CaptureError::IncompleteFrame);
        }
        let end = first_frame
            .checked_add((interleaved.len() / channels) as u64)
            .ok_or(CaptureError::BufferTooLarge)?;
        let layout = Layout {
            page_frames: sample_rate as usize,
            channels,
            a_channels: channels,
        };
        Ok(Self {
            sample_rate,
            layout,
            pages: history::pages_from_interleaved(first_frame, interleaved, layout)?,
            frames: first_frame..end,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The interleaved channels of every lane together.
    pub fn channels(&self) -> usize {
        self.layout.channels
    }

    /// The frames this view holds, on the retained-audio timeline.
    pub fn frame_range(&self) -> Range<u64> {
        self.frames.clone()
    }

    pub fn frame_count(&self) -> usize {
        (self.frames.end - self.frames.start) as usize
    }

    pub fn track_count(&self) -> usize {
        1 + usize::from(self.layout.a_channels < self.layout.channels)
    }

    fn track_channels(&self, lane: usize) -> Option<Range<usize>> {
        let Layout {
            channels,
            a_channels,
            ..
        } = self.layout;
        match lane {
            0 => Some(0..a_channels),
            1 if a_channels < channels => Some(a_channels..channels),
            _ => None,
        }
    }

    /// One lane of this view. It shares the same pages.
    pub fn track(&self, lane: usize) -> Option<FrozenTrack> {
        Some(FrozenTrack {
            channels: self.track_channels(lane)?,
            lane,
            capture: self.clone(),
        })
    }

    /// Every lane of this view, lane A first.
    pub fn tracks(&self) -> Vec<FrozenTrack> {
        (0..2).filter_map(|lane| self.track(lane)).collect()
    }

    fn clamp(&self, frames: Range<u64>) -> Range<u64> {
        let start = frames.start.max(self.frames.start);
        start..frames.end.min(self.frames.end).max(start)
    }

    /// A view of only `frames`, which must lie inside this one. It keeps just the
    /// pages holding those frames.
    pub fn slice(&self, frames: Range<u64>) -> Option<Self> {
        if frames.start < self.frames.start || frames.end > self.frames.end || frames.end < frames.start
        {
            return None;
        }
        Some(Self {
            sample_rate: self.sample_rate,
            layout: self.layout,
            pages: history::covering(&self.pages, self.layout, &frames).into(),
            frames,
        })
    }

    fn pages(&self, frames: Range<u64>) -> impl Iterator<Item = (&Page, Range<usize>)> {
        let frames = self.clamp(frames);
        let pages = history::covering(&self.pages, self.layout, &frames);
        history::page_ranges(
            pages.iter().map(|page| &**page),
            frames,
            self.layout.channels,
        )
    }

    /// The audio of `frames` in order, as contiguous runs of whole frames with every
    /// lane's channels interleaved. A run ends where a one-second page does. Frames
    /// outside the view are left out.
    pub fn slices(&self, frames: Range<u64>) -> impl Iterator<Item = &[f32]> {
        let channels = self.layout.channels;
        self.pages(frames)
            .map(move |(page, frames)| page.samples(frames, channels))
    }

    fn trace_extrema(&self, trace: usize, frames: Range<u64>) -> (f32, f32) {
        history::extrema(self.pages(frames), self.layout, trace)
    }

    fn trace_waveform(
        &self,
        trace: usize,
        frames: Range<u64>,
        minimum: &mut [f32],
        maximum: &mut [f32],
    ) -> usize {
        waveform_bins(self.clamp(frames), minimum, maximum, |frames| {
            self.trace_extrema(trace, frames)
        })
    }

    /// The lowest and highest finite sample of `frames` across every channel, or zeros
    /// when there is none. Whole 128 frame blocks are answered from the peak cache.
    pub fn extrema(&self, frames: Range<u64>) -> (f32, f32) {
        self.trace_extrema(0, frames)
    }

    /// Writes chronological waveform bins for the part of `frames` this view holds,
    /// across every channel, and returns how many frames that is.
    pub fn waveform(&self, frames: Range<u64>, minimum: &mut [f32], maximum: &mut [f32]) -> usize {
        self.trace_waveform(0, frames, minimum, maximum)
    }

    /// Copies the whole view into one owned buffer.
    pub fn snapshot(&self) -> Snapshot {
        let mut samples = Vec::with_capacity(self.frame_count() * self.layout.channels);
        for slice in self.slices(self.frame_range()) {
            samples.extend_from_slice(slice);
        }
        Snapshot {
            sample_rate: self.sample_rate,
            channels: self.layout.channels,
            samples,
            first_frame: self.frames.start,
        }
    }

    /// How many one-second pages hold this view's audio.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// How many of this view's pages are the very same memory as a page of `other`.
    pub fn shared_pages(&self, other: &Self) -> usize {
        self.pages
            .iter()
            .filter(|page| other.pages.iter().any(|candidate| Arc::ptr_eq(page, candidate)))
            .count()
    }

    #[cfg(test)]
    pub(super) fn page_addresses(&self) -> Vec<*const Page> {
        self.pages.iter().map(Arc::as_ptr).collect()
    }

    /// Observers that do not keep the pages alive.
    #[cfg(test)]
    pub(super) fn page_references(&self) -> Vec<std::sync::Weak<Page>> {
        self.pages.iter().map(Arc::downgrade).collect()
    }
}

impl fmt::Debug for FrozenCapture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrozenCapture")
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.layout.channels)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

/// One lane of a [`FrozenCapture`]: the immutable counterpart of a track [`Snapshot`]
/// that shares the capture's pages instead of owning a flat copy.
#[derive(Clone)]
pub struct FrozenTrack {
    capture: FrozenCapture,
    lane: usize,
    /// This lane's channels within a frame of the capture.
    channels: Range<usize>,
}

impl FrozenTrack {
    /// Copies `interleaved` into a single-lane view. See [`FrozenCapture::from_interleaved`].
    pub fn from_interleaved(
        sample_rate: u32,
        channels: usize,
        first_frame: u64,
        interleaved: &[f32],
    ) -> Result<Self, CaptureError> {
        let capture =
            FrozenCapture::from_interleaved(sample_rate, channels, first_frame, interleaved)?;
        Ok(capture.track(0).expect("a view always has lane A"))
    }

    pub fn sample_rate(&self) -> u32 {
        self.capture.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    /// The first frame's index on the retained-audio timeline.
    pub fn first_frame(&self) -> u64 {
        self.capture.frames.start
    }

    pub fn frame_range(&self) -> Range<u64> {
        self.capture.frame_range()
    }

    pub fn frame_count(&self) -> usize {
        self.capture.frame_count()
    }

    /// The view of every lane this track belongs to.
    pub fn capture(&self) -> &FrozenCapture {
        &self.capture
    }

    /// The same lane restricted to `frames`, which must lie inside this track.
    pub fn slice(&self, frames: Range<u64>) -> Option<Self> {
        Some(Self {
            capture: self.capture.slice(frames)?,
            lane: self.lane,
            channels: self.channels.clone(),
        })
    }

    /// This lane's audio of `frames` in order, as runs of whole interleaved frames
    /// that each end where a one-second page does. A lane that is the capture's only
    /// one borrows the pages; with two lanes each run is a de-interleaved copy.
    /// Frames outside the track are left out.
    pub fn chunks(&self, frames: Range<u64>) -> impl Iterator<Item = Cow<'_, [f32]>> {
        let stride = self.capture.layout.channels;
        let channels = self.channels.clone();
        self.capture.slices(frames).map(move |slice| {
            if channels.len() == stride {
                Cow::Borrowed(slice)
            } else {
                Cow::Owned(
                    slice
                        .chunks_exact(stride)
                        .flat_map(|frame| &frame[channels.clone()])
                        .copied()
                        .collect(),
                )
            }
        })
    }

    /// Fills `output` with whole interleaved frames starting at retained frame `first`.
    /// Returns false, leaving `output` alone, unless the track holds all of them.
    pub fn read_frames(&self, first: u64, output: &mut [f32]) -> bool {
        let width = self.channels.len();
        if output.len() % width != 0 {
            return false;
        }
        let available = self.frame_range();
        let Some(end) = first
            .checked_add((output.len() / width) as u64)
            .filter(|&end| first >= available.start && end <= available.end)
        else {
            return false;
        };
        let stride = self.capture.layout.channels;
        let mut written = 0;
        for slice in self.capture.slices(first..end) {
            if width == stride {
                output[written..written + slice.len()].copy_from_slice(slice);
                written += slice.len();
            } else {
                for frame in slice.chunks_exact(stride) {
                    output[written..written + width].copy_from_slice(&frame[self.channels.clone()]);
                    written += width;
                }
            }
        }
        debug_assert_eq!(written, output.len());
        true
    }

    /// The lowest and highest finite value `view` takes over `frames`, or zeros when
    /// there is none. Whole 128 frame blocks are answered from the peak cache; only the
    /// partial blocks at either end, and at page ends, are read sample by sample.
    pub fn extrema(&self, view: WaveformView, frames: Range<u64>) -> (f32, f32) {
        self.capture
            .trace_extrema(Layout::trace(self.lane, view), frames)
    }

    /// Writes chronological waveform bins for the part of `frames` this track holds
    /// and returns how many frames that is. The bins are those of
    /// [`view_peaks`](crate::waveform::view_peaks) over the same frames.
    pub fn waveform(
        &self,
        view: WaveformView,
        frames: Range<u64>,
        minimum: &mut [f32],
        maximum: &mut [f32],
    ) -> usize {
        self.capture
            .trace_waveform(Layout::trace(self.lane, view), frames, minimum, maximum)
    }

    /// Copies the whole lane into one owned buffer.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_range(self.frame_range())
            .expect("the track holds its own frames")
    }

    /// Copies only the requested frames, preserving their absolute position.
    pub fn snapshot_range(&self, frames: Range<u64>) -> Option<Snapshot> {
        let available = self.frame_range();
        if frames.start < available.start || frames.end > available.end || frames.end < frames.start
        {
            return None;
        }
        let capacity = usize::try_from(frames.end - frames.start)
            .ok()?
            .checked_mul(self.channels.len())?;
        let mut samples = Vec::new();
        samples.try_reserve_exact(capacity).ok()?;
        for chunk in self.chunks(frames.clone()) {
            samples.extend_from_slice(&chunk);
        }
        Some(Snapshot {
            sample_rate: self.capture.sample_rate,
            channels: self.channels.len(),
            samples,
            first_frame: frames.start,
        })
    }
}

impl fmt::Debug for FrozenTrack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrozenTrack")
            .field("lane", &self.lane)
            .field("sample_rate", &self.capture.sample_rate)
            .field("channels", &self.channels.len())
            .field("frames", &self.capture.frames)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::super::history::PEAK_FRAMES;
    use super::super::{Capture, CaptureState, DurationChoice};
    use super::*;
    use crate::waveform::{self, view_peaks};

    const VIEWS: [WaveformView; 3] = [
        WaveformView::Merged,
        WaveformView::Left,
        WaveformView::Right,
    ];

    /// A small deterministic generator, so a failure names a seed that replays it.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, limit: u64) -> u64 {
            (self.next() >> 11) % limit.max(1)
        }

        fn sample(&mut self) -> f32 {
            match self.below(400) {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => f32::NEG_INFINITY,
                3 => -0.0,
                _ => (self.below(1 << 24) as f32 / (1 << 23) as f32 - 1.0) * 1.5,
            }
        }

        /// A range inside `bounds` whose ends favour page and peak-block seams.
        fn range(&mut self, bounds: &Range<u64>, page_frames: u64) -> Range<u64> {
            let length = bounds.end - bounds.start;
            let edge = |random: &mut Self| {
                let frame = bounds.start + random.below(length + 1);
                let snapped = match random.below(6) {
                    0 => frame / page_frames * page_frames,
                    1 => frame / page_frames * page_frames + PEAK_FRAMES as u64,
                    2 => frame / PEAK_FRAMES as u64 * PEAK_FRAMES as u64,
                    _ => frame,
                };
                snapped.clamp(bounds.start, bounds.end)
            };
            let (a, b) = (edge(self), edge(self));
            match self.below(12) {
                0 => a..a,
                1 => a..(a + 1).min(bounds.end),
                _ => a.min(b)..a.max(b),
            }
        }
    }

    fn bits(samples: &[f32]) -> Vec<u32> {
        samples.iter().map(|sample| sample.to_bits()).collect()
    }

    /// The extrema of `view` over a flat interleaved copy, read sample by sample.
    fn scan(interleaved: &[f32], channels: usize, view: WaveformView) -> (f32, f32) {
        waveform::extrema(
            interleaved
                .chunks_exact(channels)
                .map(|frame| view.sample(frame)),
        )
    }

    /// Holds a view to the flat copies taken at the same moment: all of it, and
    /// `ranges` random ranges of it.
    fn assert_matches_flat(
        view: &FrozenCapture,
        flat: &Snapshot,
        lanes: &[Snapshot],
        random: &mut Random,
        ranges: usize,
        context: &str,
    ) {
        let frames = flat.frame_range();
        assert_eq!(view.frame_range(), frames, "{context}");
        assert_eq!(view.frame_count(), flat.frame_count(), "{context}");
        assert_eq!(view.channels(), flat.channels, "{context}");
        assert_eq!(view.sample_rate(), flat.sample_rate, "{context}");
        assert_eq!(bits(&view.snapshot().samples), bits(&flat.samples), "{context}");
        let tracks = view.tracks();
        assert_eq!(tracks.len(), lanes.len(), "{context}");
        assert_eq!(view.track_count(), lanes.len(), "{context}");
        for (track, lane) in tracks.iter().zip(lanes) {
            assert_eq!(track.frame_range(), frames, "{context}");
            assert_eq!(track.first_frame(), lane.first_frame, "{context}");
            assert_eq!(track.channels(), lane.channels, "{context}");
            assert_eq!(track.sample_rate(), lane.sample_rate, "{context}");
            assert_eq!(bits(&track.snapshot().samples), bits(&lane.samples), "{context}");
        }
        let page_frames = u64::from(flat.sample_rate);
        for _ in 0..ranges {
            let range = random.range(&frames, page_frames);
            let context = format!("{context}, frames {range:?} of {frames:?}");
            let relative =
                (range.start - frames.start) as usize..(range.end - frames.start) as usize;
            let expected = flat.frames(relative.clone()).unwrap();
            let joined: Vec<f32> = view.slices(range.clone()).flatten().copied().collect();
            assert_eq!(bits(&joined), bits(expected), "{context}");
            assert!(view.slices(range.clone()).all(|slice| !slice.is_empty()) || range.is_empty());
            assert_eq!(view.extrema(range.clone()), scan(expected, 1, WaveformView::Left), "{context}");
            let sliced = view.slice(range.clone()).unwrap();
            assert_eq!(sliced.frame_range(), range, "{context}");
            assert_eq!(bits(&sliced.snapshot().samples), bits(expected), "{context}");

            let bins = 1 + random.below(300) as usize;
            let (mut low, mut high) = (vec![9.0; bins], vec![9.0; bins + 1]);
            let (mut expected_low, mut expected_high) = (low.clone(), high.clone());
            for (track, lane) in tracks.iter().zip(lanes) {
                let expected = lane.frames(relative.clone()).unwrap();
                assert_eq!(
                    bits(&track.snapshot_range(range.clone()).unwrap().samples),
                    bits(expected),
                    "{context}"
                );
                let chunks: Vec<f32> = track
                    .chunks(range.clone())
                    .flat_map(|chunk| chunk.into_owned())
                    .collect();
                assert_eq!(bits(&chunks), bits(expected), "{context}");
                let mut read = vec![7.0; expected.len()];
                assert!(track.read_frames(range.start, &mut read), "{context}");
                assert_eq!(bits(&read), bits(expected), "{context}");
                let sliced = track.slice(range.clone()).unwrap();
                assert_eq!(bits(&sliced.snapshot().samples), bits(expected), "{context}");
                for view in VIEWS {
                    assert_eq!(
                        track.extrema(view, range.clone()),
                        scan(expected, lane.channels, view),
                        "{context}, {view:?}"
                    );
                    assert_eq!(
                        sliced.extrema(view, frames.clone()),
                        scan(expected, lane.channels, view),
                        "{context}, {view:?} of a slice"
                    );
                    view_peaks(expected, lane.channels, view, &mut expected_low, &mut expected_high);
                    assert_eq!(
                        track.waveform(view, range.clone(), &mut low, &mut high),
                        relative.len(),
                        "{context}"
                    );
                    assert_eq!(low, expected_low, "{context}, {view:?}, {bins} bins");
                    assert_eq!(high, expected_high, "{context}, {view:?}, {bins} bins");
                }
            }
        }
        // Requests reaching outside the view are refused or clipped, never misread.
        assert!(view.slice(frames.start..frames.end + 1).is_none(), "{context}");
        assert_eq!(view.slices(0..u64::MAX).map(<[f32]>::len).sum::<usize>(), flat.samples.len());
        let mut outside = [7.0; 4];
        let width = tracks[0].channels();
        assert!(!tracks[0].read_frames(frames.end, &mut outside[..width]), "{context}");
        assert_eq!(outside, [7.0; 4], "{context}");
    }

    fn flat_lanes(capture: &Capture) -> Vec<Snapshot> {
        (0..2).filter_map(|lane| capture.track_snapshot(lane)).collect()
    }

    #[test]
    fn frozen_views_match_flat_copies_through_random_capture_histories() {
        const RETENTIONS: [DurationChoice; 3] = [
            DurationChoice::Seconds10,
            DurationChoice::Seconds20,
            DurationChoice::Seconds30,
        ];
        // Low rates keep the histories small while every block spans several pages.
        // Pages of 256 frames are whole peak blocks; the others end in a partial one.
        for (case, (rate, a_channels, b_channels)) in
            [(150, 1, None), (256, 2, None), (190, 1, Some(2)), (300, 2, Some(2))]
                .into_iter()
                .enumerate()
        {
            for seed in 1..=2u64 {
                let mut random = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) + case as u64);
                let mut capture =
                    Capture::new_tracks(rate, a_channels, b_channels, DurationChoice::Seconds20)
                        .unwrap();
                let channels = a_channels + b_channels.unwrap_or(0);
                // Views taken earlier, with the flat copies of that moment.
                let mut frozen: Vec<(FrozenCapture, Snapshot, Vec<Snapshot>)> = Vec::new();
                for step in 0..140 {
                    let context = format!("case {case}, seed {seed}, step {step}");
                    match random.below(20) {
                        0..=7 => {
                            let frames = 1 + random.below(500) as usize;
                            let block: Vec<f32> =
                                (0..frames * channels).map(|_| random.sample()).collect();
                            capture.push_interleaved(&block).unwrap();
                        }
                        8 | 9 => {
                            let frames = 1 + random.below(400) as usize;
                            capture.push_interleaved(&vec![0.0; frames * channels]).unwrap();
                        }
                        10 => capture.set_retain_silence(random.below(2) == 0),
                        11 => {
                            let frames = match random.below(4) {
                                0 => 1 + random.below(40_000_000),
                                _ => 1 + random.below(3_000),
                            };
                            capture.append_known_silence(frames).unwrap();
                        }
                        12 => {
                            // The next retained frame fades the tail it joins.
                            capture.pause();
                            capture.smooth_next_join();
                            capture.resume();
                        }
                        13 => capture.smooth_next_start(),
                        14 | 15 => {
                            capture.set_retention(RETENTIONS[random.below(3) as usize]);
                        }
                        16 if random.below(3) == 0 => capture.clear(),
                        _ => {
                            let view = capture.freeze();
                            let flat = capture.snapshot();
                            let lanes = flat_lanes(&capture);
                            assert_matches_flat(&view, &flat, &lanes, &mut random, 6, &context);
                            // A partial freeze matches the same frames of the flat copy.
                            let retained = capture.frame_range();
                            let range = random.range(&retained, u64::from(rate));
                            let part = capture.freeze_range(range.clone()).unwrap();
                            for (lane, track) in part.tracks().iter().enumerate() {
                                let expected =
                                    capture.track_snapshot_range(lane, range.clone()).unwrap();
                                assert_eq!(track.frame_range(), range, "{context}");
                                assert_eq!(
                                    bits(&track.snapshot().samples),
                                    bits(&expected.samples),
                                    "{context}"
                                );
                            }
                            assert!(capture.freeze_range(retained.start..retained.end + 1).is_none());
                            if frozen.len() == 3 {
                                let (view, flat, lanes) = frozen.remove(random.below(3) as usize);
                                assert_matches_flat(&view, &flat, &lanes, &mut random, 6, &context);
                            }
                            frozen.push((view, flat, lanes));
                        }
                    }
                    // Whatever the capture did since, an earlier view still reads the
                    // audio and the peaks of the moment it was taken.
                    if let Some((view, flat, lanes)) = frozen.get(step % 3) {
                        assert_matches_flat(view, flat, lanes, &mut random, 2, &context);
                    }
                }
                for (view, flat, lanes) in &frozen {
                    assert_matches_flat(view, flat, lanes, &mut random, 6, "at the end");
                }
            }
        }
    }

    fn loud(frames: Range<usize>, channels: usize) -> Vec<f32> {
        frames
            .flat_map(|frame| {
                (0..channels).map(move |channel| {
                    0.25 + ((frame * 7 + channel * 3) % 1_000) as f32 / 2_000.0
                })
            })
            .collect()
    }

    #[test]
    fn a_frozen_view_is_unaffected_by_later_pushes_truncation_fades_clears_and_retention_changes() {
        let mut random = Random(41);
        let mut capture =
            Capture::new_tracks(1_000, 2, Some(1), DurationChoice::Seconds30).unwrap();
        capture.push_interleaved(&loud(0..25_500, 3)).unwrap();
        let view = capture.freeze();
        let flat = capture.snapshot();
        let lanes = flat_lanes(&capture);
        assert_eq!(view.frame_range(), 0..25_500);
        let mut check =
            |step: &str| assert_matches_flat(&view, &flat, &lanes, &mut random, 8, step);
        check("frozen");

        // A fade rewrites the tail the view was taken from.
        capture.pause();
        capture.smooth_next_join();
        capture.resume();
        capture.push_interleaved(&loud(0..10, 3)).unwrap();
        assert_eq!(flat.frames(25_499..25_500).unwrap(), loud(25_499..25_500, 3));
        assert_eq!(
            capture.snapshot().frames(25_499..25_500).unwrap(),
            [0.0; 3],
            "the live tail was faded"
        );
        check("after a fade");

        // New audio fills the page the view copied and rolls the whole history over.
        capture.push_interleaved(&loud(0..40_000, 3)).unwrap();
        assert_eq!(capture.frame_range(), 35_510..65_510);
        check("after the history rolled over");

        capture.set_retention(DurationChoice::Seconds10);
        assert_eq!(capture.frame_range(), 55_510..65_510);
        check("after a shorter retention");
        capture.set_retention(DurationChoice::Minutes10);
        check("after a longer retention");

        capture.set_retain_silence(true);
        capture.append_known_silence(5_000_000).unwrap();
        check("after a long retained silence");

        capture.clear();
        assert!(capture.frame_range().is_empty());
        check("after a clear");
        capture.push_interleaved(&loud(0..2_500, 3)).unwrap();
        check("after recording again");

        // A view outlives the capture it was taken from.
        drop(capture);
        check("after the capture is gone");
    }

    #[test]
    fn freezing_shares_complete_pages_and_copies_only_the_page_being_written() {
        let mut capture = Capture::new(1_000, 2, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&loud(0..5_500, 2)).unwrap();
        let live = capture.history.sealed_page_addresses();
        assert_eq!(live.len(), 5);

        let first = capture.freeze();
        let second = capture.freeze();
        assert_eq!(first.page_count(), 6);
        // The five complete pages are the capture's own memory. Only the half-written
        // sixth is a copy, and each view has its own.
        assert_eq!(first.page_addresses()[..5], live);
        assert!(!live.contains(&first.page_addresses()[5]));
        assert_eq!(first.shared_pages(&second), 5);
        assert_eq!(first.shared_pages(&first.clone()), 6);
        assert_eq!(capture.history.sealed_page_addresses(), live, "freezing moves nothing");

        // Lanes and slices are further references to the same pages.
        let track = first.track(0).unwrap();
        assert_eq!(track.capture().page_addresses(), first.page_addresses());
        assert_eq!(track.slice(1_200..3_400).unwrap().capture().page_addresses(), live[1..4]);

        // A range that ends before the page being written copies no audio at all.
        let part = capture.freeze_range(1_200..3_400).unwrap();
        assert_eq!(part.page_addresses(), live[1..4]);
        let whole_pages = capture.freeze_range(0..5_000).unwrap();
        assert_eq!(whole_pages.page_addresses(), live);
        assert_eq!(capture.freeze_range(2_000..2_000).unwrap().page_count(), 0);

        // Appending never touches a shared page: the view copied the open one.
        capture.push_interleaved(&loud(0..700, 2)).unwrap();
        let sealed = capture.history.sealed_page_addresses();
        assert_eq!(sealed[..5], live);
        assert_eq!(sealed.len(), 6);
        assert_eq!(capture.freeze().shared_pages(&first), 5);
    }

    #[test]
    fn a_fade_copies_only_the_shared_pages_it_rewrites() {
        let fade_join = |capture: &mut Capture| {
            capture.pause();
            capture.smooth_next_join();
            capture.resume();
            capture.push_interleaved(&[0.5, 0.5]).unwrap();
        };
        // The two faded frames are the last of page four and the first of the open page.
        let mut capture = Capture::new(1_000, 2, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&loud(0..5_001, 2)).unwrap();
        let before = capture.history.sealed_page_addresses();
        let allocated = capture.history.allocated_frames();
        let view = capture.freeze();
        let flat = capture.snapshot();
        fade_join(&mut capture);
        let after = capture.history.sealed_page_addresses();
        assert_eq!(after[..4], before[..4], "pages the fade does not reach stay shared");
        assert_ne!(after[4], before[4], "the faded page was copied for the capture");
        assert_eq!(view.page_addresses()[..5], before, "the view keeps the original");
        assert_eq!(bits(&view.snapshot().samples), bits(&flat.samples));
        assert_eq!(
            capture.snapshot().frames(4_999..5_002).unwrap()[..4],
            [flat.samples[9_998], flat.samples[9_999], 0.0, 0.0]
        );
        // The copy is one of the spare pages. The original now belongs to the view alone.
        assert_eq!(capture.history.allocated_frames(), allocated - 1_000);
        assert_eq!(capture.freeze().shared_pages(&view), 4);
        // The cached peaks of both versions follow their own samples.
        assert_eq!(
            view.extrema(4_992..5_001),
            scan(flat.frames(4_992..5_001).unwrap(), 1, WaveformView::Left)
        );
        let live = capture.snapshot();
        let mut low = [0.0; 40];
        let mut high = [0.0; 40];
        capture.waveform(&mut low, &mut high);
        for bin in 0..40 {
            let frames = bin * live.frame_count() / 40..(bin + 1) * live.frame_count() / 40;
            assert_eq!(
                (low[bin], high[bin]),
                scan(live.frames(frames).unwrap(), 1, WaveformView::Left)
            );
        }

        // With no view holding the page, the same fade happens in place.
        let mut capture = Capture::new(1_000, 2, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&loud(0..5_001, 2)).unwrap();
        let before = capture.history.sealed_page_addresses();
        drop(capture.freeze());
        fade_join(&mut capture);
        assert_eq!(capture.history.sealed_page_addresses(), before);
        assert_eq!(capture.state(), CaptureState::Recording);
    }

    #[test]
    fn pages_that_roll_out_while_frozen_are_released_with_the_view_and_replaced_on_demand() {
        let mut capture = Capture::new(100, 1, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&loud(0..1_000, 1)).unwrap();
        assert_eq!(capture.history.allocated_frames(), 6_000);
        let view = capture.freeze();
        let flat = capture.snapshot();
        let held = view.page_references();
        assert_eq!(held.len(), 10);

        // The whole frozen range rolls out. Its pages cannot be reused, so they leave
        // the ring and stay alive only through the view.
        capture.push_interleaved(&loud(1_000..2_050, 1)).unwrap();
        assert_eq!(capture.frame_range(), 1_050..2_050);
        assert!(held.iter().all(|page| page.strong_count() == 1));
        assert_eq!(capture.history.allocated_frames(), 5_100, "nine pages left with the view");
        assert_eq!(bits(&view.snapshot().samples), bits(&flat.samples));
        assert_eq!(capture.snapshot().samples, loud(1_050..2_050, 1));

        // Capture carries on in spare pages, then in new ones, without waiting.
        capture.set_retention(DurationChoice::Minutes3);
        capture.push_interleaved(&loud(2_050..9_000, 1)).unwrap();
        assert_eq!(capture.frame_range(), 1_050..9_000);
        assert_eq!(capture.snapshot().samples, loud(1_050..9_000, 1));
        assert_eq!(bits(&view.snapshot().samples), bits(&flat.samples));

        // Dropping the last view frees every page only it still held: nothing leaks.
        let track = view.track(0).unwrap();
        drop(view);
        assert!(held.iter().all(|page| page.strong_count() == 1));
        drop(track);
        assert!(held.iter().all(|page| page.strong_count() == 0));

        // A view dropped before its pages roll out leaves them to be recycled as before.
        let allocated = capture.history.allocated_frames();
        drop(capture.freeze());
        capture.set_retention(DurationChoice::Seconds10);
        assert_eq!(capture.history.allocated_frames(), 6_000);
        assert!(allocated > 6_000);
        capture.clear();
        assert_eq!(capture.history.allocated_frames(), 6_000);
    }

    #[test]
    fn extrema_from_the_peak_cache_equal_a_brute_force_scan() {
        let mut random = Random(7);
        for (rate, a_channels, b_channels) in [(1_000, 2, Some(1)), (441, 1, None), (128, 2, Some(2))] {
            let channels = a_channels + b_channels.unwrap_or(0);
            let mut capture =
                Capture::new_tracks(rate, a_channels, b_channels, DurationChoice::Seconds30).unwrap();
            let audio: Vec<f32> = (0..(rate as usize * 12 + 77) * channels)
                .map(|_| random.sample())
                .collect();
            capture.push_interleaved(&audio).unwrap();
            let view = capture.freeze();
            let copied = FrozenCapture::from_interleaved(rate, channels, 500, &audio).unwrap();
            let frames = view.frame_range();
            assert_eq!(frames, 0..(audio.len() / channels) as u64);
            let lanes = flat_lanes(&capture);
            for _ in 0..400 {
                let range = random.range(&frames, u64::from(rate));
                let relative = range.start as usize..range.end as usize;
                let every = scan(
                    &audio[relative.start * channels..relative.end * channels],
                    1,
                    WaveformView::Left,
                );
                assert_eq!(view.extrema(range.clone()), every, "{range:?} at {rate} Hz");
                assert_eq!(copied.extrema(range.start + 500..range.end + 500), every, "{range:?}");
                for (track, lane) in view.tracks().iter().zip(&lanes) {
                    let expected = lane.frames(relative.clone()).unwrap();
                    for waveform_view in VIEWS {
                        assert_eq!(
                            track.extrema(waveform_view, range.clone()),
                            scan(expected, lane.channels, waveform_view),
                            "{range:?} {waveform_view:?} at {rate} Hz"
                        );
                    }
                }
            }
            // The live capture's own waveform is the same computation on the same pages.
            for bins in [1, 7, 333, 1_000] {
                let (mut low, mut high) = (vec![0.0; bins], vec![0.0; bins]);
                let (mut frozen_low, mut frozen_high) = (low.clone(), high.clone());
                capture.waveform(&mut low, &mut high);
                view.waveform(frames.clone(), &mut frozen_low, &mut frozen_high);
                assert_eq!((low, high), (frozen_low, frozen_high));
                for (lane, track) in view.tracks().iter().enumerate() {
                    for waveform_view in VIEWS {
                        let (mut low, mut high) = (vec![0.0; bins], vec![0.0; bins]);
                        let (mut frozen_low, mut frozen_high) = (low.clone(), high.clone());
                        capture.track_waveform(lane, waveform_view, &mut low, &mut high);
                        track.waveform(waveform_view, 0..u64::MAX, &mut frozen_low, &mut frozen_high);
                        assert_eq!((low, high), (frozen_low, frozen_high));
                    }
                }
            }
        }
    }

    #[test]
    fn a_view_built_from_a_flat_buffer_reads_back_the_same_audio() {
        let audio = loud(0..2_300, 2);
        let track = FrozenTrack::from_interleaved(1_000, 2, 40, &audio).unwrap();
        assert_eq!(track.frame_range(), 40..2_340);
        assert_eq!((track.channels(), track.sample_rate(), track.first_frame()), (2, 1_000, 40));
        assert_eq!(track.capture().page_count(), 3);
        let snapshot = track.snapshot();
        assert_eq!(
            snapshot,
            Snapshot { sample_rate: 1_000, channels: 2, samples: audio.clone(), first_frame: 40 }
        );
        assert_eq!(track.snapshot_range(1_039..1_042).unwrap().samples, audio[1_998..2_004]);
        assert!(track.snapshot_range(39..42).is_none());
        let empty = FrozenTrack::from_interleaved(48_000, 1, 9, &[]).unwrap();
        assert_eq!(empty.frame_range(), 9..9);
        assert!(empty.snapshot().samples.is_empty());
        assert_eq!(empty.extrema(WaveformView::Merged, 0..100), (0.0, 0.0));
        for (rate, channels, samples, error) in [
            (0, 1, 2, CaptureError::InvalidSampleRate),
            (10, 0, 2, CaptureError::InvalidChannelCount),
            (10, 2, 3, CaptureError::IncompleteFrame),
        ] {
            assert_eq!(
                FrozenTrack::from_interleaved(rate, channels, 0, &audio[..samples]).unwrap_err(),
                error
            );
        }
        assert_eq!(
            FrozenCapture::from_interleaved(10, 1, u64::MAX, &audio[..2]).unwrap_err(),
            CaptureError::BufferTooLarge
        );
    }
}

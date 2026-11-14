use std::{collections::VecDeque, ops::Range, sync::Arc};

use super::{CaptureError, MAX_BUFFER_SAMPLES};
use crate::waveform::WaveformView;

const BASE_SECONDS: usize = 60;
pub(super) const PEAK_FRAMES: usize = 128;
const TRACES: usize = 7;

#[derive(Clone, Copy)]
pub(super) struct Bounds {
    min: f32,
    max: f32,
}

impl Bounds {
    pub(super) const EMPTY: Self = Self {
        min: f32::INFINITY,
        max: f32::NEG_INFINITY,
    };

    fn add(&mut self, value: f32) {
        if value.is_finite() {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }
    }

    fn combine(&mut self, bounds: Self) {
        self.min = self.min.min(bounds.min);
        self.max = self.max.max(bounds.max);
    }

    pub(super) fn result(self) -> (f32, f32) {
        if self.min.is_finite() {
            (self.min, self.max)
        } else {
            (0.0, 0.0)
        }
    }
}

/// How every page of one history lays out its frames.
#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub(super) page_frames: usize,
    pub(super) channels: usize,
    pub(super) a_channels: usize,
}

impl Layout {
    /// The waveform trace of one lane's view. Trace zero spans every channel.
    pub(super) fn trace(lane: usize, view: WaveformView) -> usize {
        1 + lane * 3 + view as usize
    }

    /// The pages holding `range`, counted from the page that starts at `first_page_frame`.
    fn page_indices(&self, first_page_frame: u64, range: &Range<u64>) -> Range<usize> {
        if range.start >= range.end {
            return 0..0;
        }
        let index = |frame: u64| ((frame - first_page_frame) / self.page_frames as u64) as usize;
        index(range.start)..index(range.end - 1) + 1
    }
}

/// Up to one second of interleaved audio and its peak cache.
pub(super) struct Page {
    first_frame: u64,
    samples: Vec<f32>,
    peaks: Vec<[Bounds; TRACES]>,
}

impl Page {
    fn new(page_frames: usize, channels: usize) -> Result<Self, CaptureError> {
        let mut samples = Vec::new();
        let mut peaks = Vec::new();
        samples
            .try_reserve_exact(page_frames * channels)
            .map_err(|_| CaptureError::AllocationFailed)?;
        peaks
            .try_reserve_exact(page_frames / PEAK_FRAMES)
            .map_err(|_| CaptureError::AllocationFailed)?;
        Ok(Self {
            first_frame: 0,
            samples,
            peaks,
        })
    }

    /// A page sized for exactly `samples`, with its peak cache filled in.
    fn from_samples(first_frame: u64, samples: &[f32], layout: Layout) -> Result<Self, CaptureError> {
        let mut page = Self::new(samples.len() / layout.channels, layout.channels)?;
        page.first_frame = first_frame;
        page.samples.extend_from_slice(samples);
        for block in 0..page.frames(layout.channels) / PEAK_FRAMES {
            let bounds = page.summarize(block, layout.channels, layout.a_channels);
            page.peaks.push(bounds);
        }
        Ok(page)
    }

    fn reset(&mut self, first_frame: u64) {
        self.first_frame = first_frame;
        self.samples.clear();
        self.peaks.clear();
    }

    /// Both pages were reserved for the same layout, so this does not allocate.
    fn copy_from(&mut self, page: &Self) {
        self.reset(page.first_frame);
        self.samples.extend_from_slice(&page.samples);
        self.peaks.extend_from_slice(&page.peaks);
    }

    /// An owned copy of the first `frames` frames and the peak blocks they complete.
    fn prefix(&self, frames: usize, channels: usize) -> Self {
        Self {
            first_frame: self.first_frame,
            samples: self.samples[..frames * channels].to_vec(),
            peaks: self.peaks[..frames / PEAK_FRAMES].to_vec(),
        }
    }

    fn frames(&self, channels: usize) -> usize {
        self.samples.len() / channels
    }

    /// The part of `range`, in frames from the start of this page, that it holds.
    fn overlap(&self, range: &Range<u64>, channels: usize) -> Range<usize> {
        let start = range.start.saturating_sub(self.first_frame) as usize;
        let end = (range.end.saturating_sub(self.first_frame) as usize).min(self.frames(channels));
        start..end.max(start)
    }

    pub(super) fn samples(&self, frames: Range<usize>, channels: usize) -> &[f32] {
        &self.samples[frames.start * channels..frames.end * channels]
    }

    fn summarize(&self, block: usize, channels: usize, a_channels: usize) -> [Bounds; TRACES] {
        let mut peaks = [Bounds::EMPTY; TRACES];
        let start = block * PEAK_FRAMES * channels;
        let samples = &self.samples[start..start + PEAK_FRAMES * channels];
        for frame in samples.chunks_exact(channels) {
            for &sample in frame {
                peaks[0].add(sample);
            }
            for (lane, track) in [&frame[..a_channels], &frame[a_channels..]]
                .into_iter()
                .enumerate()
            {
                if let Some(&left) = track.first() {
                    let right = track.get(1).copied().unwrap_or(left);
                    peaks[1 + lane * 3].add(WaveformView::Merged.sample(track));
                    peaks[2 + lane * 3].add(left);
                    peaks[3 + lane * 3].add(right);
                }
            }
        }
        peaks
    }

    /// Adds the extrema of `frames` to `bounds`: whole 128 frame blocks come from the
    /// peak cache, and the partial blocks at either end are read sample by sample.
    fn extrema(&self, layout: Layout, trace: usize, frames: Range<usize>, bounds: &mut Bounds) {
        let first_block = frames.start.div_ceil(PEAK_FRAMES);
        let last_block = frames.end / PEAK_FRAMES;
        if first_block < last_block {
            let first_cached = first_block * PEAK_FRAMES;
            let last_cached = last_block * PEAK_FRAMES;
            self.scan(layout, trace, frames.start..first_cached, bounds);
            for block in &self.peaks[first_block..last_block] {
                bounds.combine(block[trace]);
            }
            self.scan(layout, trace, last_cached..frames.end, bounds);
        } else {
            self.scan(layout, trace, frames, bounds);
        }
    }

    fn scan(&self, layout: Layout, trace: usize, frames: Range<usize>, bounds: &mut Bounds) {
        let samples = self.samples(frames, layout.channels);
        if trace == 0 {
            for &sample in samples {
                bounds.add(sample);
            }
            return;
        }
        let track = if trace <= 3 {
            0..layout.a_channels
        } else {
            layout.a_channels..layout.channels
        };
        let view = WaveformView::from_code(((trace - 1) % 3) as u32).expect("valid waveform view");
        for frame in samples.chunks_exact(layout.channels) {
            bounds.add(view.sample(&frame[track.clone()]));
        }
    }
}

/// Pairs each page with the frames of `range` it holds.
pub(super) fn page_ranges<'a>(
    pages: impl Iterator<Item = &'a Page>,
    range: Range<u64>,
    channels: usize,
) -> impl Iterator<Item = (&'a Page, Range<usize>)> {
    pages.map(move |page| (page, page.overlap(&range, channels)))
}

pub(super) fn extrema<'a>(
    pages: impl Iterator<Item = (&'a Page, Range<usize>)>,
    layout: Layout,
    trace: usize,
) -> (f32, f32) {
    let mut bounds = Bounds::EMPTY;
    for (page, frames) in pages {
        page.extrema(layout, trace, frames, &mut bounds);
    }
    bounds.result()
}

/// Splits `interleaved` into pages of their own, for audio that never was in a history.
pub(super) fn pages_from_interleaved(
    first_frame: u64,
    interleaved: &[f32],
    layout: Layout,
) -> Result<Arc<[Arc<Page>]>, CaptureError> {
    let mut pages = Vec::new();
    pages
        .try_reserve_exact(interleaved.len().div_ceil(layout.page_frames * layout.channels))
        .map_err(|_| CaptureError::AllocationFailed)?;
    for (index, samples) in interleaved
        .chunks(layout.page_frames * layout.channels)
        .enumerate()
    {
        let first = first_frame
            .checked_add((index * layout.page_frames) as u64)
            .ok_or(CaptureError::BufferTooLarge)?;
        pages.push(Arc::new(Page::from_samples(first, samples, layout)?));
    }
    Ok(pages.into())
}

/// The pages of `pages` that cover `range`. `pages` must start at the page holding
/// the first frame its owner retains.
pub(super) fn covering<'a>(
    pages: &'a [Arc<Page>],
    layout: Layout,
    range: &Range<u64>,
) -> &'a [Arc<Page>] {
    match pages.first() {
        Some(first) => &pages[layout.page_indices(first.first_frame, range)],
        None => pages,
    }
}

/// One-second pages let the capture consumer add or release capacity without moving
/// existing samples. Each page's peak cache keeps redraw cost bounded for long histories.
///
/// A complete page is sealed behind a reference count, so a frozen view shares it
/// instead of copying its audio. Only the page still being written is owned outright;
/// a view takes a copy of that one, which keeps appending free of any sharing check.
pub(super) struct History {
    /// Complete pages, oldest first. A frozen view may hold any of them, so the
    /// history writes to one only through `unshare`.
    sealed: VecDeque<Arc<Page>>,
    /// The page being written. It follows the sealed pages and is never shared.
    open: Option<Page>,
    free: Vec<Page>,
    layout: Layout,
    start: u64,
    end: u64,
}

impl History {
    pub(super) fn new(
        page_frames: usize,
        channels: usize,
        a_channels: usize,
    ) -> Result<Self, CaptureError> {
        page_frames
            .checked_mul(channels)
            .and_then(|samples| samples.checked_mul(BASE_SECONDS))
            .filter(|&samples| samples <= MAX_BUFFER_SAMPLES)
            .ok_or(CaptureError::BufferTooLarge)?;
        let free = (0..BASE_SECONDS)
            .map(|_| Page::new(page_frames, channels))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            sealed: VecDeque::new(),
            open: None,
            free,
            layout: Layout {
                page_frames,
                channels,
                a_channels,
            },
            start: 0,
            end: 0,
        })
    }

    fn active_pages(&self) -> usize {
        self.sealed.len() + usize::from(self.open.is_some())
    }

    #[cfg(test)]
    pub(super) fn allocated_frames(&self) -> usize {
        (self.active_pages() + self.free.len()) * self.layout.page_frames
    }

    /// The address of every sealed page holding retained audio, oldest first.
    #[cfg(test)]
    pub(super) fn sealed_page_addresses(&self) -> Vec<*const Page> {
        self.sealed.iter().map(Arc::as_ptr).collect()
    }

    pub(super) fn layout(&self) -> Layout {
        self.layout
    }

    pub(super) fn total_frames(&self) -> u64 {
        self.end
    }

    pub(super) fn frame_range(&self) -> Range<u64> {
        self.start..self.end
    }

    /// A page leaving the ring is reused unless a frozen view still reads it. That
    /// view then owns the memory and releases it; the ring allocates a replacement
    /// only when it next needs one, so nothing waits and nothing leaks.
    fn recycle(free: &mut Vec<Page>, page: Arc<Page>) {
        if let Ok(page) = Arc::try_unwrap(page) {
            free.push(page);
        }
    }

    fn spare_page(&mut self) -> Result<Page, CaptureError> {
        match self.free.pop() {
            Some(page) => Ok(page),
            None => Page::new(self.layout.page_frames, self.layout.channels),
        }
    }

    /// Makes the history the only owner of a sealed page before it writes to it.
    /// A frozen view keeps the page it was given; the history continues in a copy.
    fn unshare(&mut self, index: usize) -> Result<(), CaptureError> {
        if Arc::get_mut(&mut self.sealed[index]).is_none() {
            let mut copy = self.spare_page()?;
            copy.copy_from(&self.sealed[index]);
            self.sealed[index] = Arc::new(copy);
        }
        Ok(())
    }

    pub(super) fn ensure_append_capacity(&mut self) -> Result<(), CaptureError> {
        if self
            .open
            .as_ref()
            .is_some_and(|page| page.samples.len() < self.layout.page_frames * self.layout.channels)
        {
            return Ok(());
        }
        self.sealed
            .try_reserve(1)
            .map_err(|_| CaptureError::AllocationFailed)?;
        let mut page = self.spare_page()?;
        page.reset(self.end);
        if let Some(complete) = self.open.replace(page) {
            self.sealed.push_back(Arc::new(complete));
        }
        Ok(())
    }

    pub(super) fn push(&mut self, frame: &[f32], gain: f32) {
        let Layout {
            channels,
            a_channels,
            ..
        } = self.layout;
        let page = self.open.as_mut().expect("capacity was reserved");
        if gain == 1.0 {
            page.samples.extend_from_slice(frame);
        } else {
            page.samples.extend(frame.iter().map(|&sample| sample * gain));
        }
        self.end += 1;
        let frames = page.samples.len() / channels;
        if frames % PEAK_FRAMES == 0 {
            let bounds = page.summarize(frames / PEAK_FRAMES - 1, channels, a_channels);
            page.peaks.push(bounds);
        }
    }

    fn recycle_all(&mut self) {
        for page in self.sealed.drain(..) {
            Self::recycle(&mut self.free, page);
        }
        self.free.extend(self.open.take());
    }

    /// Preserve a known zero interval, materializing only the retained suffix.
    pub(super) fn append_silence(
        &mut self,
        frames: u64,
        retained_frames: usize,
    ) -> Result<(), CaptureError> {
        self.end.checked_add(frames).ok_or(CaptureError::BufferTooLarge)?;
        let skipped = frames.saturating_sub(retained_frames as u64);
        if skipped != 0 {
            // The retained zero suffix will immediately reuse these pages;
            // do not release and reallocate a previously grown history.
            self.recycle_all();
            self.end += skipped;
            self.start = self.end;
        }
        let mut remaining = frames - skipped;
        let zero = Bounds { min: 0.0, max: 0.0 };
        let mut zero_peaks = [Bounds::EMPTY; TRACES];
        let Layout {
            page_frames,
            channels,
            a_channels,
        } = self.layout;
        let traces = if a_channels < channels { TRACES } else { 4 };
        zero_peaks[..traces].fill(zero);
        while remaining != 0 {
            self.ensure_append_capacity()?;
            let page = self.open.as_mut().expect("capacity was reserved");
            let previous_frames = page.samples.len() / channels;
            let count = remaining.min((page_frames - previous_frames) as u64) as usize;
            page.samples.resize((previous_frames + count) * channels, 0.0);
            let completed_blocks = (previous_frames + count) / PEAK_FRAMES;
            while page.peaks.len() < completed_blocks {
                let block = page.peaks.len();
                let bounds = if block * PEAK_FRAMES < previous_frames {
                    page.summarize(block, channels, a_channels)
                } else {
                    zero_peaks
                };
                page.peaks.push(bounds);
            }
            self.end += count as u64;
            remaining -= count as u64;
            self.truncate_to(retained_frames);
        }
        Ok(())
    }

    pub(super) fn truncate_to(&mut self, frames: usize) {
        self.start = self.start.max(self.end.saturating_sub(frames as u64));
        let page_frames = self.layout.page_frames as u64;
        while self
            .sealed
            .front()
            .is_some_and(|page| page.first_frame + page_frames <= self.start)
        {
            let page = self.sealed.pop_front().expect("oldest page exists");
            Self::recycle(&mut self.free, page);
        }
        if self.sealed.is_empty()
            && self
                .open
                .as_ref()
                .is_some_and(|page| page.first_frame + page_frames <= self.start)
        {
            self.free.extend(self.open.take());
        }
    }

    pub(super) fn release_unused_capacity(&mut self, retained_frames: usize) {
        // The oldest active page can be partly retained. Only free whole unused pages.
        let target = retained_frames
            .div_ceil(self.layout.page_frames)
            .max(BASE_SECONDS);
        self.free
            .truncate(target.saturating_sub(self.active_pages()));
    }

    pub(super) fn clear(&mut self) {
        self.start = self.end;
        self.recycle_all();
        self.release_unused_capacity(0);
    }

    /// The sealed pages holding part of `range`, and whether the open page holds the rest.
    fn page_indices(&self, range: &Range<u64>) -> (Range<usize>, bool) {
        let first = self.sealed.front().map(|page| &**page).or(self.open.as_ref());
        let Some(first) = first else {
            return (0..0, false);
        };
        let indices = self.layout.page_indices(first.first_frame, range);
        let sealed = self.sealed.len();
        (
            indices.start.min(sealed)..indices.end.min(sealed),
            indices.end > sealed,
        )
    }

    pub(super) fn slices(&self, range: Range<u64>) -> impl Iterator<Item = &[f32]> {
        let channels = self.layout.channels;
        self.pages(range)
            .map(move |(page, frames)| page.samples(frames, channels))
    }

    fn pages(&self, range: Range<u64>) -> impl Iterator<Item = (&Page, Range<usize>)> {
        let (sealed, open) = self.page_indices(&range);
        let pages = self
            .sealed
            .range(sealed)
            .map(|page| &**page)
            .chain(self.open.as_ref().filter(|_| open));
        page_ranges(pages, range, self.layout.channels)
    }

    /// The pages holding `range`, which must lie inside the retained frames. Sealed
    /// pages are shared, not copied. Only the part of the open page that `range`
    /// reaches is copied, because the capture keeps writing to that page.
    pub(super) fn share(&self, range: &Range<u64>) -> Arc<[Arc<Page>]> {
        let (sealed, open) = self.page_indices(range);
        let tail = self.open.as_ref().filter(|_| open).map(|page| {
            let frames = page.overlap(range, self.layout.channels).end;
            Arc::new(page.prefix(frames, self.layout.channels))
        });
        self.sealed.range(sealed).cloned().chain(tail).collect()
    }

    pub(super) fn fade_out_tail(&mut self, frames: usize) -> Result<(), CaptureError> {
        let count = (self.end - self.start).min(frames as u64);
        if count == 0 {
            return Ok(());
        }
        let start = self.end - count;
        let end = self.end;
        let denominator = count.saturating_sub(1).max(1) as f32;
        let (sealed, open) = self.page_indices(&(start..end));
        // A frozen view keeps the audio as it was: fade private copies of the pages
        // it shares. Nothing is faded unless every one of them could be copied.
        for index in sealed.clone() {
            self.unshare(index)?;
        }
        let Layout {
            channels,
            a_channels,
            ..
        } = self.layout;
        let sealed = self
            .sealed
            .range_mut(sealed)
            .map(|page| Arc::get_mut(page).expect("the faded pages are not shared"));
        for page in sealed.chain(self.open.as_mut().filter(|_| open)) {
            let start = start.saturating_sub(page.first_frame) as usize;
            for (offset, frame) in page
                .samples
                .chunks_exact_mut(channels)
                .enumerate()
                .skip(start)
            {
                let distance = end - 1 - page.first_frame - offset as u64;
                let gain = distance as f32 / denominator;
                for sample in frame {
                    *sample *= gain;
                }
            }
            for block in start / PEAK_FRAMES..page.peaks.len() {
                page.peaks[block] = page.summarize(block, channels, a_channels);
            }
        }
        Ok(())
    }

    pub(super) fn extrema(&self, trace: usize, range: Range<u64>) -> (f32, f32) {
        extrema(self.pages(range), self.layout, trace)
    }
}

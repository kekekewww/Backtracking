use std::{
    collections::VecDeque,
    ops::{Deref, DerefMut, Range},
    sync::{Arc, Mutex, OnceLock},
};

use super::CaptureError;
use crate::waveform::WaveformView;

#[cfg(test)]
mod cache_tests;
pub(super) mod pair;

const BASE_SECONDS: usize = 60;
pub(super) const PEAK_FRAMES: usize = 128;
const CHANNEL_PEAK_FRAMES: usize = 16;
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
    fn extra_channels(self) -> usize {
        self.a_channels.saturating_sub(2) + (self.channels - self.a_channels).saturating_sub(2)
    }

    /// The first two channels of each lane already have left/right peak traces.
    fn channel_trace(self, channel: usize) -> Result<usize, usize> {
        let (lane, local, preceding) = if channel < self.a_channels {
            (0, channel, 0)
        } else {
            (
                1,
                channel - self.a_channels,
                self.a_channels.saturating_sub(2),
            )
        };
        if local < 2 {
            Ok(2 + lane * 3 + local)
        } else {
            Err(preceding + local - 2)
        }
    }

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

#[derive(Default)]
struct ChannelBlocks {
    fine: Vec<Bounds>,
    coarse: Vec<Bounds>,
}

#[derive(Default)]
struct SharedPeaks {
    native: OnceLock<Box<[Mutex<ChannelBlocks>]>>,
    pairs: OnceLock<Box<[Mutex<Option<pair::PairBlocks>>; 2]>>,
}

type ChannelPeaks = Arc<SharedPeaks>;

struct WidePeaks {
    activity: [OnceLock<u64>; 2],
    traces: Vec<[Bounds; TRACES]>,
    capacity_frames: usize,
    // Prefix copies share this even before its first read. Only complete blocks
    // are stored, so appending can extend it without changing a frozen prefix.
    channels: ChannelPeaks,
}

// Vec's unused capacity values let this enum fit in the original Vec's space.
// Mono/stereo lanes keep the original allocation and peak-building path.
enum PagePeaks {
    Traces(Vec<[Bounds; TRACES]>),
    Wide(Box<WidePeaks>),
}

impl Deref for PagePeaks {
    type Target = Vec<[Bounds; TRACES]>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Traces(traces) => traces,
            Self::Wide(wide) => &wide.traces,
        }
    }
}

impl DerefMut for PagePeaks {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Traces(traces) => traces,
            Self::Wide(wide) => &mut wide.traces,
        }
    }
}

impl PagePeaks {
    fn new(traces: Vec<[Bounds; TRACES]>, layout: Layout, capacity_frames: usize) -> Self {
        if layout.extra_channels() == 0 {
            Self::Traces(traces)
        } else {
            Self::Wide(Box::new(WidePeaks {
                activity: Default::default(),
                traces,
                capacity_frames,
                channels: Arc::new(SharedPeaks::default()),
            }))
        }
    }

    fn prefix(&self, blocks: usize) -> Self {
        let traces = self[..blocks].to_vec();
        match self {
            Self::Traces(_) => Self::Traces(traces),
            Self::Wide(wide) => Self::Wide(Box::new(WidePeaks {
                activity: Default::default(),
                traces,
                capacity_frames: wide.capacity_frames,
                channels: Arc::clone(&wide.channels),
            })),
        }
    }

    /// Samples are about to change. Keep frozen caches intact, and reuse the
    /// allocated vectors when no frozen prefix still shares them.
    fn invalidate_channels(&mut self) {
        if let Self::Wide(wide) = self {
            wide.activity = Default::default();
            if let Some(cache) = Arc::get_mut(&mut wide.channels) {
                cache.pairs.take();
                if let Some(channels) = cache.native.get_mut() {
                    for channel in channels {
                        let blocks = channel.get_mut().unwrap_or_else(|error| error.into_inner());
                        blocks.fine.clear();
                        blocks.coarse.clear();
                    }
                }
            } else {
                wide.channels = Arc::new(SharedPeaks::default());
            }
        }
    }
}

/// Up to one second of interleaved audio and its peak cache.
pub(super) struct Page {
    first_frame: u64,
    samples: Vec<f32>,
    peaks: PagePeaks,
}

impl Page {
    fn new(page_frames: usize, layout: Layout) -> Result<Self, CaptureError> {
        let mut samples = Vec::new();
        let mut peaks = Vec::new();
        samples
            .try_reserve_exact(page_frames * layout.channels)
            .map_err(|_| CaptureError::AllocationFailed)?;
        peaks
            .try_reserve_exact(page_frames / PEAK_FRAMES)
            .map_err(|_| CaptureError::AllocationFailed)?;
        Ok(Self {
            first_frame: 0,
            samples,
            peaks: PagePeaks::new(peaks, layout, page_frames),
        })
    }

    /// A page sized for exactly `samples`, with its peak cache filled in.
    fn from_samples(
        first_frame: u64,
        samples: &[f32],
        layout: Layout,
    ) -> Result<Self, CaptureError> {
        let mut page = Self::new(samples.len() / layout.channels, layout)?;
        page.first_frame = first_frame;
        page.samples.extend_from_slice(samples);
        for block in 0..page.frames(layout.channels) / PEAK_FRAMES {
            let bounds = page.summarize(block, layout.channels, layout.a_channels);
            page.peaks.push(bounds);
        }
        Ok(page)
    }

    fn reset(&mut self, first_frame: u64) {
        self.peaks.invalidate_channels();
        self.first_frame = first_frame;
        self.samples.clear();
        self.peaks.clear();
    }

    /// Sample and trace buffers have room for the same layout. A channel cache
    /// still shared with a frozen prefix is detached by reset.
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
            peaks: self.peaks.prefix(frames / PEAK_FRAMES),
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

    fn scan_channel(
        &self,
        channels: usize,
        channel: usize,
        frames: Range<usize>,
        bounds: &mut Bounds,
    ) {
        for frame in self.samples(frames, channels).chunks_exact(channels) {
            bounds.add(frame[channel]);
        }
    }

    fn channel_extrema(
        &self,
        layout: Layout,
        channel: usize,
        frames: Range<usize>,
        bounds: &mut Bounds,
    ) {
        let cache_channel = match layout.channel_trace(channel) {
            Ok(trace) => {
                self.extrema(layout, trace, frames, bounds);
                return;
            }
            Err(channel) => channel,
        };
        // Fine blocks limit expensive strided edge reads to 30 samples per bin.
        // Coarse blocks keep long ranges from visiting every fine block.
        let first_block = frames.start.div_ceil(CHANNEL_PEAK_FRAMES);
        let last_block = frames.end / CHANNEL_PEAK_FRAMES;
        if first_block >= last_block {
            self.scan_channel(layout.channels, channel, frames, bounds);
            return;
        }
        let PagePeaks::Wide(wide) = &self.peaks else {
            unreachable!("extra channels have a wide cache");
        };
        let channels = wide.channels.native.get_or_init(|| {
            (0..layout.extra_channels())
                .map(|_| Mutex::new(ChannelBlocks::default()))
                .collect()
        });
        let mut peaks = channels[cache_channel]
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let complete_blocks = self.frames(layout.channels) / CHANNEL_PEAK_FRAMES;
        if peaks.fine.len() < complete_blocks {
            // Reserve once per requested channel/page. Reads, never appends,
            // build the blocks that have arrived since the last read.
            if peaks.fine.capacity() == 0 {
                peaks
                    .fine
                    .reserve_exact(wide.capacity_frames / CHANNEL_PEAK_FRAMES);
                peaks
                    .coarse
                    .reserve_exact(wide.capacity_frames / PEAK_FRAMES);
            }
            for block in peaks.fine.len()..complete_blocks {
                let mut peak = Bounds::EMPTY;
                self.scan_channel(
                    layout.channels,
                    channel,
                    block * CHANNEL_PEAK_FRAMES..(block + 1) * CHANNEL_PEAK_FRAMES,
                    &mut peak,
                );
                peaks.fine.push(peak);
            }
            let ratio = PEAK_FRAMES / CHANNEL_PEAK_FRAMES;
            for block in peaks.coarse.len()..complete_blocks / ratio {
                let mut peak = Bounds::EMPTY;
                for &fine in &peaks.fine[block * ratio..(block + 1) * ratio] {
                    peak.combine(fine);
                }
                peaks.coarse.push(peak);
            }
        }
        self.scan_channel(
            layout.channels,
            channel,
            frames.start..first_block * CHANNEL_PEAK_FRAMES,
            bounds,
        );
        let ratio = PEAK_FRAMES / CHANNEL_PEAK_FRAMES;
        let first_coarse = first_block.div_ceil(ratio);
        let last_coarse = last_block / ratio;
        if first_coarse < last_coarse {
            for &peak in &peaks.fine[first_block..first_coarse * ratio] {
                bounds.combine(peak);
            }
            for &peak in &peaks.coarse[first_coarse..last_coarse] {
                bounds.combine(peak);
            }
            for &peak in &peaks.fine[last_coarse * ratio..last_block] {
                bounds.combine(peak);
            }
        } else {
            for &peak in &peaks.fine[first_block..last_block] {
                bounds.combine(peak);
            }
        }
        self.scan_channel(
            layout.channels,
            channel,
            last_block * CHANNEL_PEAK_FRAMES..frames.end,
            bounds,
        );
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

/// Reads one channel, using shared page peaks for complete blocks and samples at
/// the edges. Extra channel peaks are built lazily, outside the capture path.
pub(super) fn channel_extrema<'a>(
    pages: impl Iterator<Item = (&'a Page, Range<usize>)>,
    layout: Layout,
    channel: usize,
) -> (f32, f32) {
    let mut bounds = Bounds::EMPTY;
    for (page, frames) in pages {
        page.channel_extrema(layout, channel, frames, &mut bounds);
    }
    bounds.result()
}

pub(super) fn active_channels<'a>(
    pages: impl Iterator<Item = (&'a Page, Range<usize>)>,
    layout: Layout,
    track: Range<usize>,
) -> u64 {
    if track.len() <= 2 {
        return 0;
    }
    let full = u64::MAX >> (64 - track.len());
    let mut active = 0;
    for (page, frames) in pages {
        let PagePeaks::Wide(wide) = &page.peaks else {
            unreachable!()
        };
        let bounds_mask = || {
            let mut mask = 0;
            for (i, channel) in track.clone().enumerate() {
                let mut bounds = Bounds::EMPTY;
                page.channel_extrema(layout, channel, frames.clone(), &mut bounds);
                if bounds.min < -3.16e-5 || bounds.max > 3.16e-5 {
                    mask |= 1 << i;
                }
            }
            mask
        };
        active |= if frames == (0..layout.page_frames) {
            *wide.activity[usize::from(track.start != 0)].get_or_init(bounds_mask)
        } else {
            bounds_mask()
        };
        if active == full {
            break;
        }
    }
    active
}

/// Splits `interleaved` into pages of their own, for audio that never was in a history.
pub(super) fn pages_from_interleaved(
    first_frame: u64,
    interleaved: &[f32],
    layout: Layout,
) -> Result<Arc<[Arc<Page>]>, CaptureError> {
    let mut pages = Vec::new();
    pages
        .try_reserve_exact(
            interleaved
                .len()
                .div_ceil(layout.page_frames * layout.channels),
        )
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
    base_pages: usize,
    layout: Layout,
    start: u64,
    end: u64,
}

impl History {
    pub(super) fn new(
        page_frames: usize,
        channels: usize,
        a_channels: usize,
        budget_frames: usize,
    ) -> Result<Self, CaptureError> {
        let page_frames = page_frames.min(budget_frames);
        page_frames
            .checked_mul(channels)
            .ok_or(CaptureError::BufferTooLarge)?;
        let base_pages = BASE_SECONDS.min(budget_frames / page_frames);
        let layout = Layout {
            page_frames,
            channels,
            a_channels,
        };
        let free = (0..base_pages)
            .map(|_| Page::new(page_frames, layout))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            sealed: VecDeque::new(),
            open: None,
            free,
            base_pages,
            layout,
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

    /// Requested allocation bytes, excluding allocator headers and existing traces.
    #[cfg(test)]
    pub(super) fn channel_cache_bytes(&self) -> (usize, usize) {
        let mut metadata = 0;
        let mut payload = 0;
        for page in self
            .sealed
            .iter()
            .map(|page| &**page)
            .chain(self.open.iter())
            .chain(&self.free)
        {
            if let PagePeaks::Wide(wide) = &page.peaks {
                metadata += std::mem::size_of::<WidePeaks>()
                    + std::mem::size_of_val(&*wide.channels)
                    + 2 * std::mem::size_of::<usize>();
                if let Some(channels) = wide.channels.native.get() {
                    metadata += std::mem::size_of_val(&**channels);
                    for channel in channels {
                        let blocks = channel.lock().unwrap();
                        payload += (blocks.fine.capacity() + blocks.coarse.capacity())
                            * std::mem::size_of::<Bounds>();
                    }
                }
            }
        }
        (metadata, payload)
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
            None => Page::new(self.layout.page_frames, self.layout),
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
            page.samples
                .extend(frame.iter().map(|&sample| sample * gain));
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
        self.end
            .checked_add(frames)
            .ok_or(CaptureError::BufferTooLarge)?;
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
            page.samples
                .resize((previous_frames + count) * channels, 0.0);
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
            .max(self.base_pages);
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
        let first = self
            .sealed
            .front()
            .map(|page| &**page)
            .or(self.open.as_ref());
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

    pub(super) fn pages(&self, range: Range<u64>) -> impl Iterator<Item = (&Page, Range<usize>)> {
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
            page.peaks.invalidate_channels();
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

    pub(super) fn active_channels(&self, track: Range<usize>) -> u64 {
        active_channels(self.pages(self.frame_range()), self.layout, track)
    }

    pub(super) fn channel_extrema(&self, channel: usize, range: Range<u64>) -> (f32, f32) {
        channel_extrema(self.pages(range), self.layout, channel)
    }
}

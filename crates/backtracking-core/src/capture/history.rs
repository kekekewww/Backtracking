use std::{collections::VecDeque, ops::Range};

use super::{CaptureError, MAX_BUFFER_SAMPLES};
use crate::waveform::WaveformView;

const BASE_SECONDS: usize = 60;
const PEAK_FRAMES: usize = 128;
const TRACES: usize = 7;

#[derive(Clone, Copy)]
struct Bounds {
    min: f32,
    max: f32,
}

impl Bounds {
    const EMPTY: Self = Self {
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

    fn result(self) -> (f32, f32) {
        if self.min.is_finite() {
            (self.min, self.max)
        } else {
            (0.0, 0.0)
        }
    }
}

struct Page {
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

    fn reset(&mut self, first_frame: u64) {
        self.first_frame = first_frame;
        self.samples.clear();
        self.peaks.clear();
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
}

/// One-second pages let the capture consumer add or release capacity without moving
/// existing samples. Each page's peak cache keeps redraw cost bounded for long histories.
pub(super) struct History {
    active: VecDeque<Page>,
    free: Vec<Page>,
    page_frames: usize,
    channels: usize,
    a_channels: usize,
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
            active: VecDeque::new(),
            free,
            page_frames,
            channels,
            a_channels,
            start: 0,
            end: 0,
        })
    }

    #[cfg(test)]
    pub(super) fn allocated_frames(&self) -> usize {
        (self.active.len() + self.free.len()) * self.page_frames
    }

    pub(super) fn total_frames(&self) -> u64 {
        self.end
    }

    pub(super) fn frame_range(&self) -> Range<u64> {
        self.start..self.end
    }

    pub(super) fn ensure_append_capacity(&mut self) -> Result<(), CaptureError> {
        if self
            .active
            .back()
            .is_some_and(|page| page.samples.len() < self.page_frames * self.channels)
        {
            return Ok(());
        }
        self.active
            .try_reserve(1)
            .map_err(|_| CaptureError::AllocationFailed)?;
        let mut page = match self.free.pop() {
            Some(page) => page,
            None => Page::new(self.page_frames, self.channels)?,
        };
        page.reset(self.end);
        self.active.push_back(page);
        Ok(())
    }

    pub(super) fn push(&mut self, frame: &[f32], gain: f32) {
        let page = self.active.back_mut().expect("capacity was reserved");
        if gain == 1.0 {
            page.samples.extend_from_slice(frame);
        } else {
            page.samples.extend(frame.iter().map(|&sample| sample * gain));
        }
        self.end += 1;
        let frames = page.samples.len() / self.channels;
        if frames % PEAK_FRAMES == 0 {
            let bounds = page.summarize(frames / PEAK_FRAMES - 1, self.channels, self.a_channels);
            page.peaks.push(bounds);
        }
    }

    pub(super) fn truncate_to(&mut self, frames: usize) {
        self.start = self.start.max(self.end.saturating_sub(frames as u64));
        while self
            .active
            .front()
            .is_some_and(|page| page.first_frame + self.page_frames as u64 <= self.start)
        {
            self.free.push(self.active.pop_front().expect("oldest page exists"));
        }
    }

    pub(super) fn release_unused_capacity(&mut self, retained_frames: usize) {
        // The oldest active page can be partly retained. Only free whole unused pages.
        let target = retained_frames.div_ceil(self.page_frames).max(BASE_SECONDS);
        self.free.truncate(target.saturating_sub(self.active.len()));
    }

    pub(super) fn clear(&mut self) {
        self.start = self.end;
        self.free.extend(self.active.drain(..));
        self.release_unused_capacity(0);
    }

    fn page_index(&self, frame: u64) -> usize {
        let first = self.active.front().expect("history has audio").first_frame;
        ((frame - first) / self.page_frames as u64) as usize
    }

    pub(super) fn slices(&self, range: Range<u64>) -> impl Iterator<Item = &[f32]> {
        self.pages(range)
            .map(|(page, range)| &page.samples[range.start * self.channels..range.end * self.channels])
    }

    fn pages(&self, range: Range<u64>) -> impl Iterator<Item = (&Page, Range<usize>)> {
        let indices = if range.is_empty() {
            0..0
        } else {
            self.page_index(range.start)..self.page_index(range.end - 1) + 1
        };
        self.active.range(indices).map(move |page| {
            let start = range.start.saturating_sub(page.first_frame) as usize;
            let end =
                ((range.end - page.first_frame) as usize).min(page.samples.len() / self.channels);
            (page, start..end)
        })
    }

    pub(super) fn fade_out_tail(&mut self, frames: usize) {
        let count = (self.end - self.start).min(frames as u64);
        if count == 0 {
            return;
        }
        let start = self.end - count;
        let denominator = count.saturating_sub(1).max(1) as f32;
        let first = self.page_index(start);
        let last = self.page_index(self.end - 1);
        for page in self.active.range_mut(first..=last) {
            let start = start.saturating_sub(page.first_frame) as usize;
            for (offset, frame) in page
                .samples
                .chunks_exact_mut(self.channels)
                .enumerate()
                .skip(start)
            {
                let distance = self.end - 1 - page.first_frame - offset as u64;
                let gain = distance as f32 / denominator;
                for sample in frame {
                    *sample *= gain;
                }
            }
            for block in start / PEAK_FRAMES..page.peaks.len() {
                page.peaks[block] = page.summarize(block, self.channels, self.a_channels);
            }
        }
    }

    pub(super) fn extrema(&self, trace: usize, range: Range<u64>) -> (f32, f32) {
        let mut bounds = Bounds::EMPTY;
        for (page, range) in self.pages(range) {
            let first_block = range.start.div_ceil(PEAK_FRAMES);
            let last_block = range.end / PEAK_FRAMES;
            if first_block < last_block {
                let first_cached = first_block * PEAK_FRAMES;
                let last_cached = last_block * PEAK_FRAMES;
                self.scan(trace, page, range.start..first_cached, &mut bounds);
                for block in &page.peaks[first_block..last_block] {
                    bounds.combine(block[trace]);
                }
                self.scan(trace, page, last_cached..range.end, &mut bounds);
            } else {
                self.scan(trace, page, range, &mut bounds);
            }
        }
        bounds.result()
    }

    fn scan(&self, trace: usize, page: &Page, frames: Range<usize>, bounds: &mut Bounds) {
        let samples = &page.samples[frames.start * self.channels..frames.end * self.channels];
        if trace == 0 {
            for &sample in samples {
                bounds.add(sample);
            }
            return;
        }
        let track = if trace <= 3 {
            0..self.a_channels
        } else {
            self.a_channels..self.channels
        };
        let view = WaveformView::from_code(((trace - 1) % 3) as u32).expect("valid waveform view");
        for frame in samples.chunks_exact(self.channels) {
            bounds.add(view.sample(&frame[track.clone()]));
        }
    }
}

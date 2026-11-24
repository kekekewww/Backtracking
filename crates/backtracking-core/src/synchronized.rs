use std::collections::VecDeque;
use std::sync::Arc;

use crate::export::ResamplingKernel;
use crate::{Capture, CaptureError, DurationChoice};

const NANOS_PER_SECOND: u128 = 1_000_000_000;
const JITTER_MS: u32 = 80;
const NATIVE_TIMESTAMP_JITTER_NS: u64 = 2_000_000;
const GAP_TAIL_MS: u32 = 250;
const GAP_FADE_MS: u32 = 2;
const MAX_PENDING_SECONDS: u32 = 2;
const MAX_PENDING_PACKETS: usize = 1024;
const DRAIN_FRAMES: usize = 2048;
const CLOCK_JITTER_FRAMES: f64 = 2.05;
const MAX_CLOCK_DRIFT: f64 = 0.005;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum ChannelRoute {
    Left = 0,
    Right = 1,
    Center = 2,
    Mid = 3,
    Side = 4,
    #[default]
    Merged = 5,
    AllChannels = 7,
}

impl ChannelRoute {
    pub const ALL: [Self; 7] = [
        Self::Left,
        Self::Right,
        Self::Center,
        Self::Mid,
        Self::Side,
        Self::Merged,
        Self::AllChannels,
    ];

    pub const fn from_code(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Left),
            1 => Some(Self::Right),
            2 => Some(Self::Center),
            3 => Some(Self::Mid),
            4 => Some(Self::Side),
            5 => Some(Self::Merged),
            7 => Some(Self::AllChannels),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Center => "Center",
            Self::Mid => "Mid",
            Self::Side => "Side",
            Self::Merged => "Merged",
            Self::AllChannels => "All channels / 所有聲道",
        }
    }

    /// Multichannel sources supply a stereo downmix when using Merged.
    /// Other routes use the original channel layout, with L/R in positions 0/1.
    pub fn output_channels(self, native: usize, center: Option<usize>) -> Option<usize> {
        match self {
            Self::AllChannels if (3..=64).contains(&native) => Some(native),
            Self::Left if native >= 1 => Some(1),
            Self::Right | Self::Mid | Self::Side if native >= 2 => Some(1),
            Self::Center if center.is_some_and(|index| index < native) => Some(1),
            Self::Merged if native >= 1 => Some(native.min(2)),
            _ => None,
        }
    }

    pub fn route_interleaved(
        self,
        samples: &[f32],
        native: usize,
        center: Option<usize>,
        output: &mut Vec<f32>,
    ) -> Result<(), CaptureError> {
        output.clear();
        let channels = self
            .output_channels(native, center)
            .ok_or(CaptureError::InvalidChannelCount)?;
        if samples.len() % native != 0 {
            return Err(CaptureError::IncompleteFrame);
        }
        if matches!(self, Self::Merged | Self::AllChannels) {
            if channels != native {
                return Err(CaptureError::InvalidChannelCount);
            }
            output.extend_from_slice(samples);
            return Ok(());
        }
        output.extend(samples.chunks_exact(native).map(|frame| match self {
            Self::Left => frame[0],
            Self::Right => frame[1],
            Self::Center => frame[center.expect("validated center")],
            Self::Mid => frame[0] * 0.5 + frame[1] * 0.5,
            Self::Side => frame[0] * 0.5 - frame[1] * 0.5,
            Self::Merged | Self::AllChannels => unreachable!(),
        }));
        Ok(())
    }
}

/// A device route or an unconditional native pass-through. Code 6 retains its
/// original native meaning for callers using the ChannelRouting wire format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelRouting {
    Routed(ChannelRoute),
    /// Keeps every source channel in its original order, without a downmix.
    Native,
}

impl ChannelRouting {
    /// Codes 0 to 5 retain the existing routes; 6 selects every native channel.
    pub const fn from_code(value: u32) -> Option<Self> {
        if value == 6 {
            Some(Self::Native)
        } else {
            match ChannelRoute::from_code(value) {
                Some(route) => Some(Self::Routed(route)),
                None => None,
            }
        }
    }

    pub const fn code(self) -> u32 {
        match self {
            Self::Routed(route) => route as u32,
            Self::Native => 6,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Routed(route) => route.label(),
            Self::Native => "Native",
        }
    }

    /// Native accepts 1 to 64 channels. Legacy routes keep their existing rules.
    pub fn output_channels(self, native: usize, center: Option<usize>) -> Option<usize> {
        match self {
            Self::Routed(route) => route.output_channels(native, center),
            Self::Native => (1..=64).contains(&native).then_some(native),
        }
    }

    pub fn route_interleaved(
        self,
        samples: &[f32],
        native: usize,
        center: Option<usize>,
        output: &mut Vec<f32>,
    ) -> Result<(), CaptureError> {
        match self {
            Self::Routed(route) => route.route_interleaved(samples, native, center, output),
            Self::Native => {
                output.clear();
                if !(1..=64).contains(&native) {
                    return Err(CaptureError::InvalidChannelCount);
                }
                if samples.len() % native != 0 {
                    return Err(CaptureError::IncompleteFrame);
                }
                output.extend_from_slice(samples);
                Ok(())
            }
        }
    }
}

struct Clock {
    timestamp_ns: u64,
    frames: usize,
    sample_rate: u32,
    projected_end: f64,
    first_timestamp_grid: f64,
    input_frames: u64,
}

impl Clock {
    fn continues_at(&self, timestamp_ns: u64, sample_rate: u32) -> bool {
        if self.sample_rate != sample_rate {
            return false;
        }
        let elapsed = timestamp_ns.saturating_sub(self.timestamp_ns);
        let observed_frames = elapsed as f64 * f64::from(sample_rate) / NANOS_PER_SECOND as f64;
        (observed_frames - self.frames as f64).abs()
            <= (self.frames as f64 * MAX_CLOCK_DRIFT).max(CLOCK_JITTER_FRAMES)
    }

    fn corrected_duration(&self, timestamp_grid: f64, grid_per_input: f64, nominal: f64) -> f64 {
        let expected = self.input_frames as f64 * grid_per_input;
        let actual = timestamp_grid - self.first_timestamp_grid;
        let clock_error = actual - expected;
        let phase_error = timestamp_grid - self.projected_end;
        let adjustment = if clock_error.abs() > CLOCK_JITTER_FRAMES {
            // Estimate frequency over the contiguous run, rather than stretching an
            // individual packet to match two potentially jittery timestamps.
            nominal * (clock_error / expected) + phase_error * 0.25
        } else {
            (phase_error.abs() - CLOCK_JITTER_FRAMES).max(0.) * phase_error.signum() * 0.25
        };
        nominal + adjustment.clamp(-nominal * MAX_CLOCK_DRIFT, nominal * MAX_CLOCK_DRIFT)
    }
}

// A packet, or consecutive packets with the same source clock, projected onto
// the capture grid. origin is fractional because independently clocked inputs
// are resampled before the gate. Each input sample maps like a bar marker.
#[derive(Clone, Copy)]
struct ClockSpan {
    time: u64,
    origin: f64,
    grid_per_input: f64,
    samples: u32,
    rate: u32,
}

impl ClockSpan {
    fn time_at(self, sample: u32) -> u64 {
        self.time
            .saturating_add(u64::from(sample) * 1_000_000_000 / u64::from(self.rate))
    }

    fn grid_at(self, sample: u32) -> f64 {
        self.origin + f64::from(sample) * self.grid_per_input
    }

    fn frame_at(self, sample: u32) -> u64 {
        ceil_grid(self.grid_at(sample))
    }

    fn first_at_or_after(self, frame: u64) -> u32 {
        let mut i = (((frame as f64 - 1. - self.origin) / self.grid_per_input + 1.).floor() as u32)
            .min(self.samples);
        while i > 0 && self.frame_at(i - 1) >= frame {
            i -= 1;
        }
        while i < self.samples && self.frame_at(i) < frame {
            i += 1;
        }
        i
    }

    fn slice(self, grid: std::ops::Range<u64>, destination: u64) -> Option<Self> {
        let first = self.first_at_or_after(grid.start);
        let end = self.first_at_or_after(grid.end);
        (first < end).then(|| Self {
            time: self.time_at(first),
            origin: (self.grid_at(first) - grid.start as f64) + destination as f64,
            samples: end - first,
            ..self
        })
    }

    fn merge(&mut self, next: Self) -> bool {
        let Some(total) = self.samples.checked_add(next.samples) else {
            return false;
        };
        // One ns accommodates integer packet timestamps. Check both ends so a
        // small frequency correction cannot accumulate into a displaced frame.
        if self.rate != next.rate
            || self.time_at(self.samples).abs_diff(next.time) > 1
            || (self.grid_at(self.samples) - next.origin).abs() > 1e-7
            || (self.grid_at(total) - next.grid_at(next.samples)).abs() > 1e-7
        {
            return false;
        }
        self.samples = total;
        true
    }

    fn lookup(self, time: u64) -> Option<u64> {
        if time < self.time || time >= self.time_at(self.samples) {
            return None;
        }
        let i = ((u128::from(time - self.time + 1) * u128::from(self.rate) - 1) / NANOS_PER_SECOND)
            as u32;
        Some(self.frame_at(i))
    }
}

// Retained stretches occupy the front, and current gate pre-roll occupies the
// tail in its own frame coordinates. Both draw from one fixed allocation.
const CLOCK_ENTRIES: usize = 64_000;
struct FrameClock {
    spans: VecDeque<ClockSpan>,
    kept: usize,
    quiet_end: u64,
    quiet_limit: u64,
    latest: u64,
    discarded_before: u64,
    unordered: bool,
    overflowed: bool,
    break_next: bool,
}

impl FrameClock {
    fn new(quiet_limit: usize) -> Result<Self, CaptureError> {
        let mut spans = VecDeque::new();
        spans
            .try_reserve_exact(CLOCK_ENTRIES)
            .map_err(|_| CaptureError::AllocationFailed)?;
        Ok(Self {
            spans,
            kept: 0,
            quiet_end: 0,
            quiet_limit: quiet_limit as u64,
            latest: 0,
            discarded_before: 0,
            unordered: false,
            overflowed: false,
            break_next: false,
        })
    }

    fn trim_retained(&mut self, first: u64) {
        while self.kept > 0 && self.spans[0].frame_at(self.spans[0].samples - 1) < first {
            self.spans.pop_front();
            self.kept -= 1;
        }
    }

    fn trim_quiet(&mut self, first: u64) {
        while let Some(span) = self.spans.get(self.kept).copied() {
            let n = span.first_at_or_after(first);
            if n > 0 {
                self.discarded_before = self.discarded_before.max(span.time_at(n));
            }
            if n < span.samples {
                break;
            }
            self.spans.remove(self.kept);
        }
    }

    fn discard_quiet(&mut self) {
        self.trim_quiet(u64::MAX);
        self.quiet_end = 0;
        self.break_next = true;
    }

    fn promote_quiet(&mut self, frame: u64, pre_roll: usize) {
        let count = self.quiet_end.min(pre_roll as u64);
        let first = self.quiet_end - count;
        self.trim_quiet(first);
        for span in self.spans.iter_mut().skip(self.kept) {
            *span = span
                .slice(first..self.quiet_end, frame - count)
                .expect("retained pre-roll");
            self.unordered |= span.time < self.latest;
            self.latest = self.latest.max(span.time_at(span.samples));
        }
        self.kept = self.spans.len();
        self.quiet_end = 0;
    }

    fn append(&mut self, span: ClockSpan, kept: bool, new_stretch: bool) {
        if kept {
            self.unordered |= span.time < self.latest;
            self.latest = self.latest.max(span.time_at(span.samples));
        }
        let same_region = kept || self.spans.len() > self.kept;
        let boundary = std::mem::take(&mut self.break_next) || new_stretch;
        if !boundary && same_region && self.spans.back_mut().is_some_and(|last| last.merge(span)) {
            return;
        }
        if self.spans.len() == CLOCK_ENTRIES {
            let oldest = self.spans.pop_front().expect("full clock");
            self.discarded_before = self.discarded_before.max(oldest.time_at(oldest.samples));
            self.kept = self.kept.saturating_sub(1);
            self.overflowed = true;
        }
        self.spans.push_back(span);
        if kept {
            self.kept += 1;
        }
    }

    fn record(
        &mut self,
        source: Option<ClockSpan>,
        grid: std::ops::Range<u64>,
        retained: Option<(u64, usize)>,
        limit: u64,
    ) {
        if let Some((first, pre_roll)) = retained {
            let end = first + (grid.end - grid.start);
            self.trim_retained(end.saturating_sub(limit));
            let opening = self.quiet_end != 0;
            if opening {
                self.promote_quiet(first, pre_roll);
            }
            if let Some(span) = source.and_then(|s| s.slice(grid, first)) {
                self.append(span, true, opening);
            }
        } else {
            let start = self.quiet_end;
            self.quiet_end += grid.end - grid.start;
            self.trim_quiet(self.quiet_end.saturating_sub(self.quiet_limit));
            if let Some(span) = source.and_then(|s| s.slice(grid, start)) {
                self.append(span, false, false);
            }
            self.trim_quiet(self.quiet_end.saturating_sub(self.quiet_limit));
        }
    }

    fn lookup(&self, time: u64) -> Option<u64> {
        if time >= self.latest || self.kept == 0 {
            return None;
        }
        if self.unordered {
            self.spans
                .range(..self.kept)
                .rev()
                .find_map(|span| span.lookup(time))
        } else {
            let mut lo = 0;
            let mut hi = self.kept;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if self.spans[mid].time <= time {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo.checked_sub(1).and_then(|i| self.spans[i].lookup(time))
        }
    }
}

// Batch the gate's decisions into runs. No clock record is written per sample.
fn push_timed(
    capture: &mut Capture,
    clocks: &mut [Option<Box<FrameClock>>],
    sources: &[Option<ClockSpan>],
    grid: u64,
    samples: &[f32],
    bars: &[usize],
) -> Result<(), CaptureError> {
    let limit = capture.effective_retention_frames() as u64;
    let mut record = |first: usize, end: usize, retained: Option<(u64, usize)>| {
        for (clock, &source) in clocks.iter_mut().zip(sources) {
            if let Some(clock) = clock {
                clock.record(
                    source,
                    grid + first as u64..grid + end as u64,
                    retained,
                    limit,
                );
            }
        }
    };
    let mut first = 0;
    let mut end = 0;
    let mut run = None;
    let result = capture.push_observed(samples, bars, |offset, retained| {
        if end == 0
            || retained.is_some() != run.is_some()
            || retained.is_some_and(|(_, pre)| pre > 0)
        {
            if end != 0 {
                record(first, end, run);
            }
            first = offset;
            run = retained;
        }
        end = offset + 1;
    });
    if end > first {
        record(first, end, run);
    }
    result
}

struct Packet {
    timestamp_ns: u64,
    source_rate: u32,
    start: u64,
    end: u64,
    origin: f64,
    step: f64,
    segment: u64,
    prefix_frames: usize,
    input_frames: usize,
    context_frames: usize,
    resampler: Option<Arc<ResamplingKernel>>,
    fade_in: bool,
    fade_out: bool,
    samples: Vec<f32>,
    bar_frames: Vec<u64>,
}

impl Packet {
    fn clock_span(&self) -> ClockSpan {
        ClockSpan {
            time: self.timestamp_ns,
            origin: self.origin,
            grid_per_input: 1. / self.step,
            samples: self.input_frames as u32,
            rate: self.source_rate,
        }
    }
    fn append_context(&mut self, following: &[f32], channels: usize) -> bool {
        let limit = (self.prefix_frames + self.input_frames + self.context_frames) * channels;
        let need = limit - self.samples.len();
        self.samples
            .extend_from_slice(&following[..need.min(following.len())]);
        // Older packets in the same segment have at least as much lookahead.
        need != 0
    }

    fn append_frame(&self, frame: u64, channels: usize, fade_frames: u64, output: &mut Vec<f32>) {
        let position = self.prefix_frames as f64 + (frame as f64 - self.origin) * self.step;
        let offset = position.floor() as usize;
        let weight = position.fract() as f32;
        let last = self.samples.len() / channels - 1;
        let start = output.len();
        if let Some(kernel) = &self.resampler {
            output.extend_from_slice(&kernel.at(&self.samples, channels, position)[..channels]);
        } else if weight == 0. {
            let offset = offset.min(last) * channels;
            output.extend_from_slice(&self.samples[offset..offset + channels]);
        } else {
            let current = offset.min(last) * channels;
            let next = (offset + 1).min(last) * channels;
            for channel in 0..channels {
                let a = self.samples[current + channel];
                let b = self.samples[next + channel];
                output.push(a + (b - a) * weight);
            }
        }
        if !self.fade_in && !self.fade_out {
            return;
        }
        let ramp = fade_frames
            .min(self.end - self.start)
            .saturating_sub(1)
            .max(1);
        let fade_in = if self.fade_in {
            (frame - self.start).min(ramp)
        } else {
            ramp
        };
        let fade_out = if self.fade_out {
            (self.end - 1 - frame).min(ramp)
        } else {
            ramp
        };
        let fade = fade_in.min(fade_out);
        if fade < ramp {
            let gain = fade as f32 / ramp as f32;
            for sample in &mut output[start..] {
                *sample *= gain;
            }
        }
    }
}

struct Lane {
    channels: usize,
    packets: VecDeque<Packet>,
    last_end: Option<u64>,
    clock: Option<Clock>,
    segment: u64,
    previous_samples: Vec<f32>,
    resampler: Option<(u32, Arc<ResamplingKernel>)>,
    waiting_after_gap: bool,
}

impl Lane {
    fn new(channels: usize) -> Self {
        Self {
            channels,
            packets: VecDeque::new(),
            last_end: None,
            clock: None,
            segment: 0,
            previous_samples: Vec::new(),
            resampler: None,
            waiting_after_gap: false,
        }
    }

    fn discard_before(&mut self, frame: u64) {
        while self
            .packets
            .front()
            .is_some_and(|packet| packet.end <= frame)
        {
            self.packets.pop_front();
        }
    }

    fn reset(&mut self) {
        self.packets.clear();
        self.last_end = None;
        self.clock = None;
        self.previous_samples.clear();
        self.waiting_after_gap = false;
    }

    fn append_frame(&mut self, frame: u64, fade_frames: u64, output: &mut Vec<f32>) -> bool {
        self.discard_before(frame);
        if let Some(packet) = self.packets.front().filter(|packet| packet.start <= frame) {
            packet.append_frame(frame, self.channels, fade_frames, output);
            packet.bar_frames.binary_search(&frame).is_ok()
        } else {
            output.resize(output.len() + self.channels, 0.);
            false
        }
    }

    fn ready_end(&self, fade_frames: u64) -> u64 {
        self.last_end.unwrap_or(0).saturating_sub(
            self.packets.back().map_or(fade_frames, |packet| {
                ((packet.context_frames as f64 / packet.step).ceil() as u64).max(fade_frames)
            }),
        )
    }
}

/// Aligns timestamped input lanes on one sample grid, then feeds one silence gate.
/// Each packet timestamp is the monotonic host time of its first input frame.
///
/// There are 1 to [`crate::MAX_LANES`] lanes. Every lane is aligned against the same
/// clock with the same jitter window, dropout fades and gap rules, and the gate keeps
/// a frame when any lane is loud.
pub struct SynchronizedCapture {
    sample_rate: u32,
    lanes: Vec<Lane>,
    capture: Capture,
    cursor: Option<u64>,
    native_end_ns: Option<u64>,
    scratch: Vec<f32>,
    bar_scratch: Vec<usize>,
    capture_error: Option<CaptureError>,
    // One entry per lane; `None` until that lane's frame clock is enabled.
    clocks: Vec<Option<Box<FrameClock>>>,
    // The clock span each lane contributes to the block being pushed, reused per block.
    sources: Vec<Option<ClockSpan>>,
}

impl SynchronizedCapture {
    pub fn new(
        rate: u32,
        a_channels: usize,
        b_channels: Option<usize>,
        retention: DurationChoice,
    ) -> Result<Self, CaptureError> {
        Self::new_with_budget(
            rate,
            a_channels,
            b_channels,
            retention,
            crate::capture::DEFAULT_MAX_BUFFER_SAMPLES,
        )
    }

    /// Creates one or two lanes with at most 64 channels in total. The sample
    /// budget caps retained history; use `capture().effective_retention_frames()` to read
    /// the retention that fits.
    pub fn new_with_budget(
        rate: u32,
        a_channels: usize,
        b_channels: Option<usize>,
        retention: DurationChoice,
        max_buffer_samples: usize,
    ) -> Result<Self, CaptureError> {
        // Validate the pair exactly as `Capture::new_tracks_with_budget` does, so a
        // zero-width lane B still reports the same error as before.
        let capture = Capture::new_tracks_with_budget(
            rate,
            a_channels,
            b_channels,
            retention,
            max_buffer_samples,
        )?;
        let widths = [a_channels, b_channels.unwrap_or(0)];
        Ok(Self::with_capture(
            rate,
            &widths[..1 + usize::from(b_channels.is_some())],
            capture,
        ))
    }

    /// Creates one lane per entry of `lane_channels`, in order, with the default
    /// sample budget. See [`Self::new_lanes_with_budget`] for the limits.
    pub fn new_lanes(
        rate: u32,
        lane_channels: &[usize],
        retention: DurationChoice,
    ) -> Result<Self, CaptureError> {
        Self::new_lanes_with_budget(
            rate,
            lane_channels,
            retention,
            crate::capture::DEFAULT_MAX_BUFFER_SAMPLES,
        )
    }

    /// Creates one lane per entry of `lane_channels`, in order. There must be 1 to
    /// [`crate::MAX_LANES`] lanes, each with 1 to 64 channels and at most 64 channels in
    /// total; otherwise this returns [`CaptureError::InvalidChannelCount`]. Lane `i` is
    /// pushed with `push(i, ..)` and occupies the matching channels of every history
    /// frame. One or two lanes behave exactly as [`Self::new_with_budget`].
    pub fn new_lanes_with_budget(
        rate: u32,
        lane_channels: &[usize],
        retention: DurationChoice,
        max_buffer_samples: usize,
    ) -> Result<Self, CaptureError> {
        let capture =
            Capture::new_lanes_with_budget(rate, lane_channels, retention, max_buffer_samples)?;
        Ok(Self::with_capture(rate, lane_channels, capture))
    }

    fn with_capture(rate: u32, lane_channels: &[usize], capture: Capture) -> Self {
        let channels: usize = lane_channels.iter().sum();
        Self {
            sample_rate: rate,
            lanes: lane_channels.iter().map(|&width| Lane::new(width)).collect(),
            capture,
            cursor: None,
            native_end_ns: None,
            scratch: Vec::with_capacity(DRAIN_FRAMES * channels),
            bar_scratch: Vec::new(),
            capture_error: None,
            clocks: lane_channels.iter().map(|_| None).collect(),
            sources: vec![None; lane_channels.len()],
        }
    }

    /// The number of lanes, from 1 to [`crate::MAX_LANES`].
    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// Enable the bounded timestamp index on a positioned lane or trim its expired entries.
    /// The record pool is reserved once before accepting packets.
    pub fn enable_frame_clock(&mut self, lane: usize) -> Result<(), CaptureError> {
        if lane >= self.lanes.len() {
            return Err(CaptureError::InvalidChannelCount);
        }
        if let Some(clock) = &mut self.clocks[lane] {
            clock.trim_retained(self.capture.frame_range().start);
        } else {
            self.clocks[lane] = Some(Box::new(FrameClock::new(self.capture.pre_roll_capacity())?));
        }
        Ok(())
    }

    /// After a budget eviction, return the first frame still covered by this lane.
    /// A monitor can trim previously resolved positions and publish a history notice.
    pub fn take_frame_clock_eviction(&mut self, lane: usize) -> Option<u64> {
        let clock = self.clocks.get_mut(lane)?.as_mut()?;
        if !std::mem::take(&mut clock.overflowed) {
            return None;
        }
        let history = self.capture.frame_range();
        Some(if clock.kept == 0 {
            history.end
        } else {
            clock.spans[0].frame_at(0).max(history.start)
        })
    }

    /// Map a source timestamp to retained audio, accounting for resampling and the gate.
    pub fn retained_frame_at(&self, lane: usize, time_ns: u64) -> std::task::Poll<Option<u64>> {
        use std::task::Poll;
        let Some(clock) = self.clocks.get(lane).and_then(Option::as_ref) else {
            return Poll::Ready(None);
        };
        if let Some(frame) = clock.lookup(time_ns) {
            Poll::Ready(self.capture.frame_range().contains(&frame).then_some(frame))
        } else if time_ns < clock.latest.max(clock.discarded_before) {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }

    pub fn capture(&self) -> &Capture {
        &self.capture
    }

    pub fn capture_mut(&mut self) -> &mut Capture {
        &mut self.capture
    }

    pub fn retain_silence(&self) -> bool {
        self.capture.retain_silence()
    }

    pub fn set_retain_silence(&mut self, retain_silence: bool) {
        if retain_silence == self.retain_silence() {
            return;
        }
        // Commit already received packets using the previous policy. Re-anchor
        // future input so an earlier unobserved quiet gap is not restored.
        self.drain(true);
        self.reset_input();
        self.capture.set_retain_silence(retain_silence);
    }

    /// Returns the last history allocation failure from asynchronous lane draining.
    pub fn take_capture_error(&mut self) -> Option<CaptureError> {
        self.capture_error.take()
    }

    pub fn reset_input(&mut self) {
        for clock in self.clocks.iter_mut().flatten() {
            clock.discard_quiet();
        }
        for lane in &mut self.lanes {
            lane.reset();
        }
        self.cursor = None;
        self.native_end_ns = None;
    }

    /// Append all routed lanes of one physical source together at its native rate.
    /// Use this instead of `push` when every lane shares the same hardware clock.
    pub fn push_native_interleaved(
        &mut self,
        timestamp_ns: u64,
        interleaved: &[f32],
        discontinuity: bool,
    ) -> Result<(), CaptureError> {
        self.push_native_interleaved_with_markers(timestamp_ns, interleaved, discontinuity, &[])
    }

    /// Native-clock variant carrying sorted bar starts relative to the first input frame.
    pub fn push_native_interleaved_with_markers(
        &mut self,
        timestamp_ns: u64,
        interleaved: &[f32],
        discontinuity: bool,
        source_block_offset_of_bars: &[usize],
    ) -> Result<(), CaptureError> {
        let channels: usize = self.lanes.iter().map(|lane| lane.channels).sum();
        let frames = interleaved.len() / channels;
        if interleaved.len() % channels != 0 {
            return Err(CaptureError::IncompleteFrame);
        }
        if frames == 0 {
            return Ok(());
        }
        if discontinuity {
            for clock in self.clocks.iter_mut().flatten() {
                clock.break_next = true;
            }
        }
        if let Some(end_ns) = self.native_end_ns {
            let gap_ns = timestamp_ns.saturating_sub(end_ns);
            if discontinuity || gap_ns > NATIVE_TIMESTAMP_JITTER_NS {
                // Synthetic gap frames have no Sender timestamp. Keep only real
                // pre-roll that still fits after the gap.

                let gap_frames = ((u128::from(gap_ns) * u128::from(self.sample_rate)
                    + NANOS_PER_SECOND / 2)
                    / NANOS_PER_SECOND)
                    .min(u128::from(u64::MAX)) as u64;
                if self.retain_silence() {
                    // Known quiet time remains on the timeline. A timestamp gap
                    // alone is not a compressed edit seam; real discontinuities
                    // keep the existing protection against abrupt joins.
                    if discontinuity {
                        self.capture.smooth_next_join();
                        let prefix = gap_frames.min(frames_for_ms(self.sample_rate, GAP_FADE_MS));
                        self.scratch.clear();
                        self.scratch.resize(prefix as usize * channels, 0.);
                        self.capture.push_interleaved(&self.scratch)?;
                        self.capture.append_known_silence(gap_frames - prefix)?;
                    } else {
                        self.capture.append_known_silence(gap_frames)?;
                    }
                    if discontinuity && gap_frames != 0 {
                        self.capture.smooth_next_start();
                    }
                } else {
                    self.capture.smooth_next_join();
                    // Compression needs only enough silence to close the gate
                    // and fill pre-roll, independent of the interruption length.
                    let mut remaining =
                        gap_frames.min(frames_for_ms(self.sample_rate, GAP_TAIL_MS));
                    let have_gap = remaining != 0;

                    self.scratch.clear();
                    self.scratch.resize(DRAIN_FRAMES * channels, 0.);
                    while remaining > 0 {
                        let count = remaining.min(DRAIN_FRAMES as u64) as usize;
                        let samples = &self.scratch[..count * channels];
                        if self.clocks.iter().any(Option::is_some) {
                            self.sources.fill(None);
                            push_timed(
                                &mut self.capture,
                                &mut self.clocks,
                                &self.sources,
                                0,
                                samples,
                                &[],
                            )?;
                        } else {
                            self.capture.push_interleaved(samples)?;
                        }
                        remaining -= count as u64;
                    }
                    if have_gap {
                        self.capture.smooth_next_start();
                    }
                }
            }
        }
        if self.clocks.iter().any(Option::is_some) {
            let source = ClockSpan {
                time: timestamp_ns,
                origin: 0.,
                grid_per_input: 1.,
                samples: u32::try_from(frames).map_err(|_| CaptureError::BufferTooLarge)?,
                rate: self.sample_rate,
            };
            self.sources.fill(Some(source));
            push_timed(
                &mut self.capture,
                &mut self.clocks,
                &self.sources,
                0,
                interleaved,
                source_block_offset_of_bars,
            )?;
        } else {
            self.capture
                .push_interleaved_with_markers(interleaved, source_block_offset_of_bars)?;
        }
        let duration_ns = (frames as u128 * NANOS_PER_SECOND / u128::from(self.sample_rate))
            .min(u128::from(u64::MAX)) as u64;
        self.native_end_ns = Some(timestamp_ns.saturating_add(duration_ns));
        Ok(())
    }

    pub fn push(
        &mut self,
        lane: usize,
        timestamp_ns: u64,
        source_rate: u32,
        interleaved: &[f32],
    ) -> Result<(), CaptureError> {
        self.push_with_discontinuity(lane, timestamp_ns, source_rate, interleaved, false)
    }

    /// `discontinuity` signals that samples were lost even if timestamps happen to be adjacent.
    pub fn push_with_discontinuity(
        &mut self,
        lane: usize,
        timestamp_ns: u64,
        source_rate: u32,
        interleaved: &[f32],
        discontinuity: bool,
    ) -> Result<(), CaptureError> {
        self.push_with_markers(
            lane,
            timestamp_ns,
            source_rate,
            interleaved,
            discontinuity,
            &[],
        )
    }

    /// Projects packet-relative bar starts onto the same time grid as both audio lanes.
    pub fn push_with_markers(
        &mut self,
        lane: usize,
        timestamp_ns: u64,
        source_rate: u32,
        interleaved: &[f32],
        discontinuity: bool,
        source_block_offset_of_bars: &[usize],
    ) -> Result<(), CaptureError> {
        let lane = self
            .lanes
            .get_mut(lane)
            .ok_or(CaptureError::InvalidChannelCount)?;
        if source_rate == 0 {
            return Err(CaptureError::InvalidSampleRate);
        }
        if interleaved.len() % lane.channels != 0 {
            return Err(CaptureError::IncompleteFrame);
        }
        let frames = interleaved.len() / lane.channels;
        if frames == 0 {
            return Ok(());
        }
        if frames as u64 > u64::from(source_rate) * u64::from(MAX_PENDING_SECONDS) {
            return Err(CaptureError::BufferTooLarge);
        }
        if !discontinuity
            && lane
                .clock
                .as_ref()
                .is_some_and(|clock| timestamp_ns <= clock.timestamp_ns)
        {
            return Ok(());
        }
        let grid = u128::from(timestamp_ns) * u128::from(self.sample_rate);
        let timestamp_grid = (grid / NANOS_PER_SECOND) as f64
            + (grid % NANOS_PER_SECOND) as f64 / NANOS_PER_SECOND as f64;
        let contiguous = lane
            .clock
            .as_ref()
            .filter(|clock| !discontinuity && clock.continues_at(timestamp_ns, source_rate));
        let restart = lane.clock.is_some() && contiguous.is_none();
        let grid_per_input = f64::from(self.sample_rate) / f64::from(source_rate);
        let nominal_duration = frames as f64 * grid_per_input;
        let (origin, duration, first_timestamp_grid, preceding_input_frames) =
            if let Some(clock) = contiguous {
                for packet in lane.packets.iter_mut().rev() {
                    if packet.segment != lane.segment
                        || !packet.append_context(interleaved, lane.channels)
                    {
                        break;
                    }
                }
                (
                    clock.projected_end,
                    clock.corrected_duration(timestamp_grid, grid_per_input, nominal_duration),
                    clock.first_timestamp_grid,
                    clock.input_frames,
                )
            } else {
                lane.segment = lane.segment.wrapping_add(1);
                lane.previous_samples.clear();
                if let Some(previous) = lane.packets.back_mut() {
                    previous.fade_out = true;
                }
                (
                    timestamp_grid
                        .round()
                        .max(lane.last_end.unwrap_or(0) as f64),
                    nominal_duration,
                    timestamp_grid,
                    0,
                )
            };
        let projected_end = origin + duration;
        let start = ceil_grid(origin);
        let end = ceil_grid(projected_end);
        lane.clock = Some(Clock {
            timestamp_ns,
            frames,
            sample_rate: source_rate,
            projected_end,
            first_timestamp_grid,
            input_frames: preceding_input_frames.saturating_add(frames as u64),
        });
        lane.last_end = Some(end);
        let first = start.max(self.cursor.unwrap_or(start));
        let resampler = if source_rate == self.sample_rate {
            None
        } else {
            let nominal_step = f64::from(source_rate) / f64::from(self.sample_rate);
            if lane
                .resampler
                .as_ref()
                .is_none_or(|(rate, _)| *rate != source_rate)
            {
                lane.resampler = Some((source_rate, Arc::new(ResamplingKernel::new(nominal_step))));
            }
            lane.resampler
                .as_ref()
                .map(|(_, filter)| Arc::clone(filter))
        };
        let context_frames = resampler.as_ref().map_or(1, |filter| filter.radius());
        let context_samples = context_frames * lane.channels;
        let prefix_frames = lane.previous_samples.len() / lane.channels;
        let mut samples =
            Vec::with_capacity(lane.previous_samples.len() + interleaved.len() + context_samples);
        samples.extend_from_slice(&lane.previous_samples);
        samples.extend_from_slice(interleaved);
        lane.previous_samples.clear();
        lane.previous_samples
            .extend_from_slice(&samples[samples.len().saturating_sub(context_samples)..]);
        if first >= end {
            lane.waiting_after_gap |= first > start;
            return Ok(());
        }
        let bar_frames = source_block_offset_of_bars
            .iter()
            .copied()
            .filter(|&offset| offset < frames)
            .map(|offset| ceil_grid(origin + offset as f64 * duration / frames as f64))
            .filter(|&frame| first <= frame && frame < end)
            .collect();
        let waiting_after_gap = std::mem::take(&mut lane.waiting_after_gap);
        lane.packets.push_back(Packet {
            timestamp_ns,
            source_rate,
            start: first,
            end,
            origin,
            step: frames as f64 / duration,
            segment: lane.segment,
            prefix_frames,
            input_frames: frames,
            context_frames,
            resampler,
            fade_in: restart || first > start || waiting_after_gap,
            fade_out: false,
            samples,
            bar_frames,
        });
        let backlog = u64::from(self.sample_rate) * u64::from(MAX_PENDING_SECONDS);
        lane.discard_before(end.saturating_sub(backlog));
        if lane.packets.len() > MAX_PENDING_PACKETS {
            lane.packets.pop_front();
            if let Some(first) = lane.packets.front_mut() {
                first.fade_in = true;
            }
        }
        Ok(())
    }

    /// In normal polling, allows the lagging source a small amount of time to catch up.
    /// Flush commits the available tail before pausing or a final snapshot.
    pub fn drain(&mut self, flush: bool) {
        let Some(latest) = self
            .lanes
            .iter()
            .filter_map(|lane| lane.last_end)
            .max()
        else {
            return;
        };
        let fade_frames = frames_for_ms(self.sample_rate, GAP_FADE_MS);
        let target = if flush {
            latest
        } else {
            let settled = self
                .lanes
                .iter()
                .map(|lane| lane.ready_end(fade_frames))
                .min()
                .unwrap_or(latest);
            let jitter = frames_for_ms(self.sample_rate, JITTER_MS);
            let candidate = settled.max(latest.saturating_sub(jitter));
            // Hold enough context to fade a lane's last packet while it can still catch up.
            // Once the jitter window has passed its end, that lane no longer delays the grid.
            self.lanes
                .iter()
                .filter(|lane| lane.last_end.is_some_and(|end| end >= candidate))
                .map(|lane| lane.ready_end(fade_frames))
                .fold(candidate, u64::min)
        };
        // A source still missing after the jitter window is an actual dropout on this grid.
        // Fade its last available packet independently so the other lanes stay intact.
        if !flush {
            for lane in &mut self.lanes {
                if lane.last_end.is_some_and(|end| end < target) {
                    lane.waiting_after_gap = true;
                    if let Some(last) = lane.packets.back_mut() {
                        last.fade_out = true;
                    }
                }
            }
        }
        let Some(mut cursor) = self.cursor.or_else(|| {
            self.lanes
                .iter()
                .filter_map(|lane| lane.packets.front().map(|packet| packet.start))
                .min()
        }) else {
            return;
        };
        // The first packet to arrive need not be the earliest. Keep the cursor unset
        // until something is actually committed past the jitter window.
        if cursor >= target {
            return;
        }
        let gap_tail = frames_for_ms(self.sample_rate, GAP_TAIL_MS);
        while cursor < target {
            for lane in &mut self.lanes {
                lane.discard_before(cursor);
            }
            let next_packet = self
                .lanes
                .iter()
                .filter_map(|lane| lane.packets.front().map(|packet| packet.start))
                .min()
                .unwrap_or(target);
            let gap_end = target.min(next_packet);
            if gap_end.saturating_sub(cursor) > gap_tail {
                if self.retain_silence() {
                    if let Err(error) = self.capture.append_known_silence(gap_end - cursor) {
                        self.capture_error = Some(error);
                    }
                } else {
                    let tail_end = cursor + gap_tail;
                    while cursor < tail_end {
                        let end = tail_end.min(cursor + DRAIN_FRAMES as u64);
                        self.push_block(cursor, end, fade_frames);
                        cursor = end;
                    }
                }
                cursor = gap_end;
            } else {
                let end = target.min(cursor + DRAIN_FRAMES as u64);
                self.push_block(cursor, end, fade_frames);
                cursor = end;
            }
        }
        self.cursor = Some(cursor);
    }

    fn push_block(&mut self, start: u64, end: u64, fade_frames: u64) {
        if self.clocks.iter().all(Option::is_none) {
            self.render_block(start, end, fade_frames, false);
            return;
        }
        let mut cursor = start;
        while cursor < end {
            let mut boundary = end;
            self.sources.fill(None);
            for (i, lane) in self.lanes.iter_mut().enumerate() {
                let Some(clock) = &mut self.clocks[i] else {
                    continue;
                };
                lane.discard_before(cursor);
                if let Some(packet) = lane.packets.front() {
                    if cursor < packet.start {
                        boundary = boundary.min(packet.start);
                    } else {
                        if cursor == packet.start && packet.fade_in {
                            clock.break_next = true;
                        }
                        boundary = boundary.min(packet.end);
                        self.sources[i] = Some(packet.clock_span());
                    }
                }
            }
            self.render_block(cursor, boundary, fade_frames, true);
            cursor = boundary;
        }
    }

    /// Renders every lane's frames `start..end` into one block and pushes it. When
    /// `timed`, `self.sources` holds each lane's clock span for the block.
    fn render_block(&mut self, start: u64, end: u64, fade_frames: u64, timed: bool) {
        self.scratch.clear();
        self.bar_scratch.clear();
        for frame in start..end {
            let mut bar = false;
            for lane in &mut self.lanes {
                bar |= lane.append_frame(frame, fade_frames, &mut self.scratch);
            }
            if bar {
                self.bar_scratch.push((frame - start) as usize);
            }
        }
        let result = if timed {
            push_timed(
                &mut self.capture,
                &mut self.clocks,
                &self.sources,
                start,
                &self.scratch,
                &self.bar_scratch,
            )
        } else {
            self.capture
                .push_interleaved_with_markers(&self.scratch, &self.bar_scratch)
        };
        if let Err(error) = result {
            self.capture_error = Some(error);
        }
    }
}

fn frames_for_ms(rate: u32, milliseconds: u32) -> u64 {
    (u64::from(rate) * u64::from(milliseconds)).div_ceil(1_000)
}

fn ceil_grid(position: f64) -> u64 {
    // Fractional projections can land just above an integer through floating-point error.
    let nearest = position.round();
    let tolerance = (8. * position.abs() * f64::EPSILON).clamp(1e-7, 0.01);
    if (position - nearest).abs() <= tolerance {
        nearest as u64
    } else {
        position.ceil() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "release packet ingest timings"]
    fn stereo_capture_packet_cost() {
        for timed in [false, true] {
            let mut capture =
                SynchronizedCapture::new(48_000, 2, None, DurationChoice::Seconds10).unwrap();
            if timed {
                capture.enable_frame_clock(0).unwrap();
            }
            let packet: Vec<_> = (0..960).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
            let mut ns = 0;
            let mut batches = Vec::new();
            for batch in 0..8 {
                let start = std::time::Instant::now();
                for _ in 0..2_000 {
                    capture
                        .push_native_interleaved(ns, std::hint::black_box(&packet), false)
                        .unwrap();
                    ns += 10_000_000;
                }
                if batch != 0 {
                    batches.push(start.elapsed().as_secs_f64() * 1e6 / 2000.);
                }
            }
            batches.sort_by(f64::total_cmp);
            eprintln!(
                "clock={timed}, median {:.3} us per native stereo 480-frame packet",
                batches[3]
            );
        }
    }

    #[test]
    #[ignore = "release packet ingest timings"]
    fn aligned_lanes_drain_cost() {
        for widths in [&[2][..], &[2, 2], &[2, 2, 2], &[1; 64]] {
            let mut capture = match *widths {
                [a] => SynchronizedCapture::new(48_000, a, None, DurationChoice::Seconds10),
                [a, b] => SynchronizedCapture::new(48_000, a, Some(b), DurationChoice::Seconds10),
                _ => SynchronizedCapture::new_lanes(48_000, widths, DurationChoice::Seconds10),
            }
            .unwrap();
            let packets: Vec<Vec<f32>> = widths
                .iter()
                .map(|&width| (0..480 * width).map(|i| (i as f32 * 0.01).sin() * 0.3).collect())
                .collect();
            let mut ns = 0;
            let mut batches = Vec::new();
            for batch in 0..8 {
                let start = std::time::Instant::now();
                for _ in 0..2_000 {
                    for (lane, packet) in packets.iter().enumerate() {
                        capture.push(lane, ns, 48_000, std::hint::black_box(packet)).unwrap();
                    }
                    capture.drain(false);
                    ns += 10_000_000;
                }
                if batch != 0 {
                    batches.push(start.elapsed().as_secs_f64() * 1e6 / 2000.);
                }
            }
            batches.sort_by(f64::total_cmp);
            eprintln!("lanes {widths:?}: median {:.3} us per 480-frame packet on each lane", batches[3]);
        }
    }

    #[test]
    fn clock_budget_evicts_only_the_oldest_and_reports_once() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, None, DurationChoice::Minutes10).unwrap();
        capture.enable_frame_clock(0).unwrap();
        for i in 0..CLOCK_ENTRIES as u64 + 3 {
            capture
                .push_native_interleaved(i * 1_000_000 + i % 2 * 125_000, &[0.25], false)
                .unwrap();
        }
        assert_eq!(capture.capture().frame_range().start, 0);
        assert_eq!(
            capture.retained_frame_at(0, 0),
            std::task::Poll::Ready(None)
        );
        assert_eq!(
            capture.retained_frame_at(0, 3_125_000),
            std::task::Poll::Ready(Some(3))
        );
        assert_eq!(capture.take_frame_clock_eviction(0), Some(3));
        assert_eq!(capture.take_frame_clock_eviction(0), None);
    }

    #[test]
    #[ignore = "clock storage for ten minutes of packet ingestion"]
    fn frame_clock_storage() {
        for mode in ["steady", "gated", "jitter"] {
            let (rate, packets) = if mode == "jitter" {
                (1000, 54_002)
            } else {
                (48_000, 60_000)
            };
            let mut capture =
                SynchronizedCapture::new(rate, 1, None, DurationChoice::Minutes10).unwrap();
            capture.enable_frame_clock(0).unwrap();
            let loud = vec![0.25; (rate / 100) as usize];
            let quiet = vec![0.; loud.len()];
            for i in 0..packets {
                let time = i * 10_000_000 + if mode == "jitter" { i % 2 * 250_000 } else { 0 };
                let samples = if mode == "gated" && i % 100 >= 50 {
                    &quiet
                } else {
                    &loud
                };
                capture
                    .push_native_interleaved(time, samples, false)
                    .unwrap();
            }
            let clock = capture.clocks[0].as_ref().unwrap();
            let overhead =
                std::mem::size_of::<FrameClock>() + std::mem::size_of::<Option<Box<FrameClock>>>();
            let bytes = std::mem::size_of::<ClockSpan>();
            let reserved = overhead + clock.spans.capacity() * bytes;
            eprintln!(
                "{mode}: records={} ({} retained), used={} reserved={reserved} bytes",
                clock.spans.len(),
                clock.kept,
                overhead + clock.spans.len() * bytes
            );
            assert!(reserved <= 2 * 1024 * 1024);
            assert!(!clock.overflowed);
            if mode == "steady" {
                assert_eq!(clock.kept, 1);
            }
            if mode == "gated" {
                assert!((600..=1200).contains(&clock.kept));
            }
            if mode == "jitter" {
                assert_eq!(clock.kept, 54002);
                assert_eq!(
                    capture.retained_frame_at(0, 14_250_000),
                    std::task::Poll::Ready(Some(14))
                );
            }
        }
    }

    #[test]
    fn independent_clock_maps_the_drifting_source_bar() {
        use std::task::Poll;
        let mut capture =
            SynchronizedCapture::new(48000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture.enable_frame_clock(0).unwrap();
        let mut time = 0;
        for packet in 0..200 {
            let first = packet * 10_002_000;
            let bars: &[usize] = if packet == 177 {
                time = first + 173 * 1_000_000_000u64 / 44100;
                &[173]
            } else {
                &[]
            };
            capture
                .push_with_markers(0, first, 44100, &[0.25; 441], false, bars)
                .unwrap();
            capture
                .push(1, packet * 10_000_000, 48000, &[0.1; 480])
                .unwrap();
            capture.drain(false);
        }
        capture.drain(true);
        let bars = capture.capture().bar_markers(0..u64::MAX);
        assert_eq!(bars.len(), 1);
        assert_eq!(
            capture.retained_frame_at(0, time),
            Poll::Ready(Some(bars[0]))
        );
    }

    #[test]
    fn gate_closing_inside_packet_keeps_only_retained_timestamps() {
        use std::task::Poll;
        let mut capture =
            SynchronizedCapture::new(1000, 1, None, DurationChoice::Seconds10).unwrap();
        capture.enable_frame_clock(0).unwrap();
        capture
            .push_native_interleaved(0, &[0.25; 5], false)
            .unwrap();
        capture
            .push_native_interleaved_with_markers(5_000_000, &[0.; 240], false, &[149, 150, 220])
            .unwrap();
        capture
            .push_native_interleaved(245_000_000, &[0.5; 5], false)
            .unwrap();
        assert_eq!(
            capture.retained_frame_at(0, 154_000_000),
            Poll::Ready(Some(154))
        );
        assert_eq!(capture.retained_frame_at(0, 155_000_000), Poll::Ready(None));
        assert_eq!(
            capture.retained_frame_at(0, 225_000_000),
            Poll::Ready(Some(175))
        );
        assert_eq!(capture.capture().bar_markers(0..u64::MAX), [154, 175]);
    }

    #[test]
    fn retained_clock_survives_nine_minutes_of_jittery_packets() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, None, DurationChoice::Minutes10).unwrap();
        capture.enable_frame_clock(0).unwrap();
        for i in 0..54002 {
            let time = i * 10_000_000 + i % 2 * 250_000;
            capture
                .push_native_interleaved_with_markers(
                    time,
                    &[0.2; 10],
                    false,
                    if i == 1 { &[4] } else { &[] },
                )
                .unwrap();
        }
        let bars = capture.capture().bar_markers(0..u64::MAX);
        assert_eq!(bars, [14]);
        assert_eq!(
            capture.retained_frame_at(0, 14_250_000),
            std::task::Poll::Ready(Some(14))
        );
    }

    #[test]
    fn frame_counted_sender_is_exact_and_one_ms_jitter_fades_packet_edges() {
        for jitter in [false, true] {
            let mut capture =
                SynchronizedCapture::new(48000, 1, Some(1), DurationChoice::Seconds10).unwrap();
            let packet: Vec<_> = (0..480).map(|i| 0.2 + i as f32 / 1000.).collect();
            for i in 0..20 {
                let time = i * 10_000_000;
                capture
                    .push(
                        0,
                        time + if jitter { i % 2 * 1_000_000 } else { 0 },
                        48000,
                        &packet,
                    )
                    .unwrap();
                capture.push(1, time, 48000, &[0.3; 480]).unwrap();
                capture.drain(false);
            }
            capture.drain(true);
            let audio = capture.capture().track_snapshot(0).unwrap().samples;
            if jitter {
                for i in 1..19 {
                    let start = i * 480 + 48;
                    assert_eq!(audio[start], 0.);
                    assert_eq!(audio[start + 100], packet[100]);
                    assert_eq!(audio[start + 479], 0.);
                }
            } else {
                assert_eq!(audio, packet.repeat(20));
            }
        }
    }

    #[test]
    fn native_routing_preserves_every_channel_and_legacy_codes() {
        for (code, route) in ChannelRoute::ALL[..6].iter().copied().enumerate() {
            let routing = ChannelRouting::Routed(route);
            assert_eq!(route as u32, code as u32);
            assert_eq!(ChannelRoute::from_code(code as u32), Some(route));
            assert_eq!(routing.code(), code as u32);
            assert_eq!(ChannelRouting::from_code(code as u32), Some(routing));
            assert_eq!(routing.label(), route.label());
            assert_eq!(
                routing.output_channels(12, Some(2)),
                route.output_channels(12, Some(2))
            );
        }
        assert_eq!(ChannelRoute::from_code(6), None);
        assert_eq!(ChannelRouting::from_code(6), Some(ChannelRouting::Native));
        assert_eq!(ChannelRouting::Native.code(), 6);
        assert_eq!(ChannelRouting::Native.label(), "Native");
        let all = ChannelRouting::Routed(ChannelRoute::AllChannels);
        assert_eq!(all.code(), 7);
        assert_eq!(ChannelRouting::from_code(7), Some(all));
        assert_eq!(all.output_channels(2, None), None);
        assert_eq!(all.output_channels(8, None), Some(8));
        assert_eq!(ChannelRouting::from_code(8), None);

        let mut routed = Vec::new();
        for channels in [1, 2, 12, 64] {
            let samples: Vec<f32> = (0..channels * 3)
                .map(|sample| sample as f32 / 256.)
                .collect();
            assert_eq!(
                ChannelRouting::Native.output_channels(channels, None),
                Some(channels)
            );
            ChannelRouting::Native
                .route_interleaved(&samples, channels, None, &mut routed)
                .unwrap();
            assert_eq!(routed, samples);
            assert_eq!(
                ChannelRoute::Merged.output_channels(channels, None),
                Some(channels.min(2))
            );
        }
        for channels in [0, 65, usize::MAX] {
            assert_eq!(ChannelRouting::Native.output_channels(channels, None), None);
            assert_eq!(
                ChannelRouting::Native.route_interleaved(&[], channels, None, &mut routed),
                Err(CaptureError::InvalidChannelCount)
            );
            assert!(routed.is_empty());
        }
        assert_eq!(
            ChannelRouting::Native.route_interleaved(&[0.; 13], 12, None, &mut routed),
            Err(CaptureError::IncompleteFrame)
        );
        assert!(routed.is_empty());
    }

    #[test]
    fn twelve_and_two_channel_lanes_keep_every_frame_aligned() {
        let mut capture =
            SynchronizedCapture::new(1000, 12, Some(2), DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        let a: Vec<f32> = (0..20)
            .flat_map(|frame| (0..12).map(move |channel| (frame * 12 + channel) as f32 / 512.))
            .collect();
        let b: Vec<f32> = (0..20)
            .flat_map(|frame| [frame as f32 / 32., -(frame as f32) / 64.])
            .collect();
        for packet in 0..2 {
            let timestamp = packet as u64 * 10_000_000;
            capture
                .push(0, timestamp, 1000, &a[packet * 120..(packet + 1) * 120])
                .unwrap();
            capture
                .push(1, timestamp, 1000, &b[packet * 20..(packet + 1) * 20])
                .unwrap();
            capture.drain(false);
        }
        capture.drain(true);
        assert!(capture.take_capture_error().is_none());
        let a_track = capture.capture().track_snapshot(0).unwrap();
        let b_track = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(a_track.frame_range(), 0..20);
        assert_eq!(a_track.frame_range(), b_track.frame_range());
        assert_eq!(a_track.samples, a);
        assert_eq!(b_track.samples, b);
    }

    #[test]
    fn multichannel_resampling_matches_independent_channel_filters() {
        const CHANNELS: usize = 12;
        const INPUT_RATE: u32 = 500;
        const OUTPUT_RATE: u32 = 1000;
        let mut capture =
            SynchronizedCapture::new(OUTPUT_RATE, CHANNELS, None, DurationChoice::Seconds10)
                .unwrap();
        capture.set_retain_silence(true);
        let raw: Vec<f32> = (0..300)
            .flat_map(|frame| {
                (0..CHANNELS).map(move |channel| {
                    let phase = frame as f32 * (channel + 1) as f32 * 0.013;
                    phase.sin() * 0.25 + channel as f32 / 64.
                })
            })
            .collect();
        for packet in 0..3 {
            let first = packet * 100;
            capture
                .push(
                    0,
                    first as u64 * 1_000_000_000 / u64::from(INPUT_RATE),
                    INPUT_RATE,
                    &raw[first * CHANNELS..(first + 100) * CHANNELS],
                )
                .unwrap();
            capture.drain(false);
        }
        capture.drain(true);
        assert!(capture.take_capture_error().is_none());
        let snapshot = capture.capture().track_snapshot(0).unwrap();
        assert_eq!(snapshot.frame_range(), 0..600);
        let step = f64::from(INPUT_RATE) / f64::from(OUTPUT_RATE);
        let kernel = ResamplingKernel::new(step);
        for channel in 0..CHANNELS {
            let mono: Vec<f32> = raw
                .chunks_exact(CHANNELS)
                .map(|frame| frame[channel])
                .collect();
            for (frame, samples) in snapshot.samples.chunks_exact(CHANNELS).enumerate() {
                let expected = kernel.at(&mono, 1, frame as f64 * step)[0];
                assert!(
                    (samples[channel] - expected).abs() < 1e-5,
                    "channel {channel}, frame {frame}"
                );
            }
        }
    }

    #[test]
    fn native_capture_accepts_sixty_four_channels_and_rejects_larger_totals() {
        let mut capture =
            SynchronizedCapture::new(1000, 64, None, DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        let samples: Vec<f32> = (0..64 * 8).map(|sample| sample as f32 / 1024.).collect();
        capture.push_native_interleaved(0, &samples, false).unwrap();
        assert_eq!(capture.capture().snapshot().samples, samples);
        for (a, b) in [
            (65, None),
            (64, Some(1)),
            (12, Some(53)),
            (0, None),
            (12, Some(0)),
        ] {
            assert!(matches!(
                SynchronizedCapture::new(1000, a, b, DurationChoice::Seconds10),
                Err(CaptureError::InvalidChannelCount)
            ));
        }
    }

    #[test]
    fn synchronized_sample_budget_shortens_retention() {
        let mut capture =
            SynchronizedCapture::new_with_budget(1000, 12, Some(2), DurationChoice::Seconds10, 140)
                .unwrap();
        capture.set_retain_silence(true);
        let samples: Vec<f32> = (0..20 * 14).map(|sample| sample as f32 / 512.).collect();
        capture.push_native_interleaved(0, &samples, false).unwrap();
        let retained = capture.capture().snapshot();
        assert_eq!(retained.frame_range(), 10..20);
        assert_eq!(retained.samples, samples[140..]);
    }

    #[test]
    fn retain_silence_keeps_native_quiet_gaps_and_bar_positions_on_both_lanes() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        assert!(!capture.retain_silence());
        capture.set_retain_silence(true);
        let first = [0.0001, -0.0002, 0., 0., -0.0001, 0.0002, 0., 0.];
        let last = [0.0003, -0.0004, 0., 0., 0.0002, -0.0001, 0., 0.];
        capture
            .push_native_interleaved_with_markers(0, &first, false, &[0, 3])
            .unwrap();
        capture
            .push_native_interleaved_with_markers(2_000_000_000, &last, false, &[1])
            .unwrap();
        let a = capture.capture().track_snapshot(0).unwrap();
        let b = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), 0..2004);
        assert_eq!(a.frame_range(), b.frame_range());
        assert_eq!(&a.samples[..4], &[0.0001, 0., -0.0001, 0.]);
        assert_eq!(&b.samples[..4], &[-0.0002, 0., 0.0002, 0.]);
        assert!(a.samples[4..2000].iter().all(|&sample| sample == 0.));
        assert!(b.samples[4..2000].iter().all(|&sample| sample == 0.));
        assert_eq!(&a.samples[2000..], &[0.0003, 0., 0.0002, 0.]);
        assert_eq!(&b.samples[2000..], &[-0.0004, 0., -0.0001, 0.]);
        assert_eq!(capture.capture().bar_markers(0..2004), vec![0, 3, 2001]);
    }

    #[test]
    fn retain_silence_keeps_async_quiet_gaps_and_aligned_markers() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        for lane in 0..2 {
            capture
                .push_with_markers(lane, 0, 1000, &[0.; 10], false, &[2])
                .unwrap();
        }
        capture.drain(true);
        for lane in 0..2 {
            capture
                .push_with_markers(lane, 2_000_000_000, 1000, &[0.; 10], false, &[5])
                .unwrap();
        }
        capture.drain(true);
        let a = capture.capture().track_snapshot(0).unwrap();
        let b = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), 0..2010);
        assert_eq!(a.frame_range(), b.frame_range());
        assert!(a
            .samples
            .iter()
            .chain(&b.samples)
            .all(|&sample| sample == 0.));
        assert_eq!(capture.capture().bar_markers(0..2010), vec![2, 2005]);
    }

    #[test]
    fn retain_silence_switch_does_not_restore_pending_or_earlier_unobserved_quiet() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, None, DurationChoice::Seconds10).unwrap();
        capture
            .push_with_markers(0, 0, 1000, &[0.; 100], false, &[10])
            .unwrap();
        capture.set_retain_silence(true);
        assert!(capture.capture().frame_range().is_empty());
        capture
            .push_with_markers(0, 5_000_000_000, 1000, &[0.; 10], false, &[2])
            .unwrap();
        capture.drain(true);
        assert_eq!(capture.capture().frame_range(), 0..10);
        assert_eq!(capture.capture().bar_markers(0..10), vec![2]);

        let before = capture.capture().snapshot();
        capture.set_retain_silence(false);
        assert_eq!(capture.capture().snapshot(), before);
        capture.set_retain_silence(true);
        assert_eq!(capture.capture().snapshot(), before);
        capture
            .push_native_interleaved(10_000_000_000, &[0.; 5], false)
            .unwrap();
        capture.set_retain_silence(true);
        capture
            .push_native_interleaved(11_000_000_000, &[0.; 5], false)
            .unwrap();
        assert_eq!(capture.capture().frame_range(), 0..1015);
    }

    #[test]
    fn retained_native_long_gap_rolls_history_without_losing_absolute_time() {
        let mut capture = SynchronizedCapture::new(10, 1, None, DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        capture
            .push_native_interleaved(0, &[0.125; 2], false)
            .unwrap();
        capture
            .push_native_interleaved_with_markers(1_000_000_000_000, &[0.25; 2], false, &[1])
            .unwrap();
        let snapshot = capture.capture().snapshot();
        assert_eq!(snapshot.frame_range(), 9902..10002);
        assert!(snapshot.samples[..98].iter().all(|&sample| sample == 0.));
        assert_eq!(&snapshot.samples[98..], &[0.25; 2]);
        assert_eq!(
            capture.capture().bar_markers(snapshot.frame_range()),
            vec![10001]
        );
    }

    #[test]
    fn retain_silence_preserves_explicit_native_discontinuity_fades() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, None, DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        capture
            .push_native_interleaved(0, &[0.25; 8], false)
            .unwrap();
        capture
            .push_native_interleaved(9_000_000, &[0.75; 8], true)
            .unwrap();
        let snapshot = capture.capture().snapshot();
        assert_eq!(snapshot.frame_range(), 0..17);
        assert_eq!(&snapshot.samples[..7], &[0.25; 7]);
        assert_eq!(&snapshot.samples[7..10], &[0.; 3]);
        assert_eq!(&snapshot.samples[10..], &[0.75; 7]);
    }

    #[test]
    fn retain_silence_switch_preserves_an_explicit_pause_and_existing_audio() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, None, DurationChoice::Seconds10).unwrap();
        capture
            .push_native_interleaved(0, &[0.25; 4], false)
            .unwrap();
        let before = capture.capture().snapshot();
        capture.capture_mut().pause();
        capture.set_retain_silence(true);
        assert_eq!(capture.capture().state(), crate::CaptureState::Paused);
        assert_eq!(capture.capture().snapshot(), before);
        capture
            .push_native_interleaved(10_000_000_000, &[0.; 4], false)
            .unwrap();
        capture
            .push_native_interleaved(12_000_000_000, &[0.; 4], false)
            .unwrap();
        capture.set_retain_silence(false);
        assert_eq!(capture.capture().state(), crate::CaptureState::Paused);
        assert_eq!(capture.capture().snapshot(), before);
    }

    #[test]
    fn sender_bars_use_the_shared_grid_and_survive_only_with_retained_audio() {
        let mut capture =
            SynchronizedCapture::new(100, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture
            .push_with_markers(0, 0, 50, &[0.; 20], false, &[1, 18, 19])
            .unwrap();
        capture
            .push_with_markers(0, 400_000_000, 50, &[0.5; 20], false, &[0, 5])
            .unwrap();
        capture
            .push_with_markers(1, 400_000_000, 100, &[0.25; 40], false, &[0, 20])
            .unwrap();
        capture.drain(true);
        let capture = capture.capture();
        assert_eq!(
            capture.track_snapshot(0).unwrap().frame_range(),
            capture.track_snapshot(1).unwrap().frame_range()
        );
        let markers = capture.bar_markers(0..80);
        assert_eq!(markers.len(), 5);
        assert_eq!(
            &markers[1..],
            &[
                markers[0] + 2,
                markers[0] + 4,
                markers[0] + 14,
                markers[0] + 24
            ]
        );
    }

    #[test]
    fn same_source_mid_and_side_keep_every_native_frame_under_timestamp_jitter() {
        const RATE: u32 = 44_100;
        let mut capture =
            SynchronizedCapture::new(RATE, 1, Some(1), DurationChoice::Seconds10).unwrap();
        let mut expected_mid = Vec::new();
        let mut expected_side = Vec::new();
        let mut frame = 0;
        for packet in 0..48 {
            let start = 1_000_000_000 + frame * 1_000_000_000 / u64::from(RATE);
            let timestamp = if packet % 2 == 0 {
                start + 500_000
            } else {
                start - 500_000
            };
            let mut samples = Vec::new();
            for _ in 0..[257, 441, 169][packet % 3] {
                let angle = frame as f32 * 0.31;
                let left = angle.sin() * 0.25 + 0.5;
                let right = (angle * 0.71).cos() * 0.12;
                let mid = left * 0.5 + right * 0.5;
                let side = left * 0.5 - right * 0.5;
                samples.extend_from_slice(&[side, mid]);
                expected_side.push(side);
                expected_mid.push(mid);
                frame += 1;
            }
            capture
                .push_native_interleaved(timestamp, &samples, false)
                .unwrap();
            capture.drain(false);
        }
        capture.drain(true);
        let side = capture.capture().track_snapshot(0).unwrap();
        let mid = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(side.sample_rate, RATE);
        assert_eq!(side.frame_range(), mid.frame_range());
        assert_eq!(side.samples, expected_side);
        assert_eq!(mid.samples, expected_mid);
    }

    #[test]
    fn native_long_gap_closes_gate_and_reset_drops_the_old_clock() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture
            .push_native_interleaved(0, &[0.25; 8], false)
            .unwrap();
        capture
            .push_native_interleaved(3_600_000_000_000, &[0.5; 8], true)
            .unwrap();
        let a = capture.capture().track_snapshot(0).unwrap();
        assert_eq!(a.samples.len(), 198);
        assert_eq!(a.samples[0], 0.25);
        assert!(a.samples[3..194].iter().all(|&sample| sample == 0.));
        assert_eq!(a.samples[197], 0.5);
        capture.reset_input();
        capture
            .push_native_interleaved(7_200_000_000_000, &[0.75, 0.125], false)
            .unwrap();
        let b = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(b.samples.len(), 199);
        assert_eq!(b.samples[198], 0.125);
    }

    #[test]
    fn native_short_gap_preserves_its_actual_duration_and_fades_only_at_the_join() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture
            .push_native_interleaved(0, &[0.25; 16], false)
            .unwrap();
        capture
            .push_native_interleaved(9_000_000, &[0.75; 16], true)
            .unwrap();
        let a = capture.capture().track_snapshot(0).unwrap();
        assert_eq!(a.samples.len(), 17);
        assert_eq!(&a.samples[..7], &[0.25; 7]);
        assert_eq!(&a.samples[7..10], &[0.; 3]);
        assert_eq!(&a.samples[10..], &[0.75; 7]);
    }

    #[test]
    fn different_sample_rates_match_whole_clip_resampling_across_packet_boundaries() {
        const RATE: u32 = 48_000;
        const INPUT_RATE: u32 = 44_100;
        let mut capture =
            SynchronizedCapture::new(RATE, 1, Some(1), DurationChoice::Seconds10).unwrap();
        let mut raw = Vec::new();
        let mut grid_frames = 0;
        for packet in 0..48 {
            let from = raw.len();
            let to = from + [257, 441, 169][packet % 3];
            raw.extend((from..to).map(|frame| {
                let time = frame as f64 / f64::from(INPUT_RATE);
                (0.5 * (std::f64::consts::TAU * 4400. * time).sin()
                    + 0.2 * (std::f64::consts::TAU * 730. * time).cos()) as f32
            }));
            let to_grid = (to as u64 * u64::from(RATE)).div_ceil(u64::from(INPUT_RATE));
            capture
                .push(
                    0,
                    grid_frames * 1_000_000_000 / u64::from(RATE),
                    RATE,
                    &vec![0.25; (to_grid - grid_frames) as usize],
                )
                .unwrap();
            capture
                .push(
                    1,
                    from as u64 * 1_000_000_000 / u64::from(INPUT_RATE),
                    INPUT_RATE,
                    &raw[from..to],
                )
                .unwrap();
            capture.drain(false);
            grid_frames = to_grid;
        }
        capture.drain(true);
        let snapshot = capture.capture().track_snapshot(1).unwrap();
        let step = f64::from(INPUT_RATE) / f64::from(RATE);
        let kernel = ResamplingKernel::new(step);
        assert_eq!(snapshot.samples.len(), grid_frames as usize);
        let max_error = snapshot
            .samples
            .iter()
            .enumerate()
            .map(|(frame, &sample)| (sample - kernel.at(&raw, 1, frame as f64 * step)[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_error < 0.002, "packet boundary error {max_error}");
    }

    #[test]
    fn a_lost_packet_fades_one_lane_without_touching_the_other() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture.push(0, 0, 1000, &[0.25; 30]).unwrap();
        capture.push(1, 0, 1000, &[0.75; 10]).unwrap();
        capture.drain(false);
        capture
            .push_with_discontinuity(1, 15_000_000, 1000, &[0.75; 15], true)
            .unwrap();
        capture.drain(true);
        let a = capture.capture().track_snapshot(0).unwrap();
        let b = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), b.frame_range());
        assert_eq!(a.samples, [0.25; 30]);
        assert_eq!(&b.samples[..9], &[0.75; 9]);
        assert_eq!(&b.samples[9..16], &[0.; 7]);
        assert_eq!(&b.samples[16..], &[0.75; 14]);
    }

    #[test]
    fn a_late_lane_can_still_supply_an_earlier_start_inside_the_jitter_window() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture.push(0, 100_000_000, 1000, &[0.25; 10]).unwrap();
        capture.drain(false);
        assert!(capture.capture().frame_range().is_empty());
        capture.push(1, 90_000_000, 1000, &[0.75; 20]).unwrap();
        capture.drain(true);
        let a = capture.capture().track_snapshot(0).unwrap();
        let b = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), 0..20);
        assert_eq!(a.frame_range(), b.frame_range());
        assert_eq!(&a.samples[..10], &[0.; 10]);
        assert_eq!(&a.samples[10..], &[0.25; 10]);
        assert_eq!(b.samples, [0.75; 20]);
    }

    #[test]
    fn catching_up_keeps_a_stale_lane_tail_until_it_can_be_faded() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture.push(0, 0, 1000, &[0.25; 100]).unwrap();
        capture.push(1, 0, 1000, &[0.75; 20]).unwrap();
        capture.drain(false);
        assert_eq!(capture.capture().frame_range(), 0..18);
        capture.push(0, 100_000_000, 1000, &[0.25; 5]).unwrap();
        capture.drain(false);
        assert_eq!(capture.capture().frame_range(), 0..25);
        capture.push(1, 20_000_000, 1000, &[0.75; 10]).unwrap();
        capture.push(1, 30_000_000, 1000, &[0.75; 10]).unwrap();
        capture.drain(true);
        let a = capture.capture().track_snapshot(0).unwrap();
        let b = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(a.samples, [0.25; 105]);
        assert_eq!(&b.samples[..19], &[0.75; 19]);
        assert_eq!(&b.samples[19..26], &[0.; 7]);
        assert_eq!(&b.samples[26..40], &[0.75; 14]);
        assert_eq!(&b.samples[40..], &[0.; 65]);
    }

    #[test]
    fn routes_preserve_original_left_right_and_discrete_center() {
        let frames = [0.75, -0.25, 0.125, -0.5, 0.25, -0.125];
        let mut samples = Vec::new();
        for (route, expected) in [
            (ChannelRoute::Left, [0.75, -0.5]),
            (ChannelRoute::Right, [-0.25, 0.25]),
            (ChannelRoute::Center, [0.125, -0.125]),
            (ChannelRoute::Mid, [0.25, -0.125]),
            (ChannelRoute::Side, [0.5, -0.375]),
        ] {
            route
                .route_interleaved(&frames, 3, Some(2), &mut samples)
                .unwrap();
            assert_eq!(samples, expected);
        }
        assert!(ChannelRoute::Center.output_channels(2, None).is_none());
        assert!(ChannelRoute::Right.output_channels(1, None).is_none());
        ChannelRoute::Merged
            .route_interleaved(&frames[..2], 2, None, &mut samples)
            .unwrap();
        assert_eq!(samples, frames[..2]);
    }

    #[test]
    fn independent_rates_and_late_start_keep_one_shared_frame_grid() {
        let mut aligned =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        aligned.push(0, 200_000_000, 1000, &[0.25; 10]).unwrap();
        aligned.push(1, 203_000_000, 500, &[0.75; 3]).unwrap();
        aligned.drain(true);
        let a = aligned.capture().track_snapshot(0).unwrap();
        let b = aligned.capture().track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), 0..10);
        assert_eq!(a.frame_range(), b.frame_range());
        assert_eq!(a.samples, [0.25; 10]);
        assert_eq!(&b.samples[..3], &[0.; 3]);
        assert!(b.samples[3..9]
            .iter()
            .all(|&sample| (sample - 0.75).abs() < 1e-6));
        assert_eq!(b.samples[9], 0.);
        let meter_a = aligned.capture().track_meter_readings(0).unwrap();
        let meter_b = aligned.capture().track_meter_readings(1).unwrap();
        assert!(meter_b.peak_dbfs > meter_a.peak_dbfs + 8.);
    }

    #[test]
    fn drift_correction_preserves_continuous_input_with_no_silent_holes() {
        let mut aligned =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        for index in 0..30 {
            aligned
                .push(0, index * 100_000_000, 1000, &[0.25; 100])
                .unwrap();
            aligned
                .push(1, index * 100_200_000, 1000, &[0.75; 100])
                .unwrap();
            aligned.drain(false);
        }
        aligned.drain(true);
        let a = aligned.capture().track_snapshot(0).unwrap();
        let b = aligned.capture().track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), 0..3006);
        assert_eq!(a.frame_range(), b.frame_range());
        assert!(a.samples[..3000].iter().all(|&sample| sample == 0.25));
        assert_eq!(a.samples[3000..], [0.; 6]);
        assert!(b.samples.iter().all(|&sample| sample == 0.75));
    }

    #[test]
    fn long_independent_clock_drift_stays_aligned_without_dropping_packets() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds60).unwrap();
        let mut sent = [0_u64; 2];
        let periods = [10_000_000, 10_020_000];
        let limits = [6000, 5990];
        while let Some(lane) = (0..2)
            .filter(|&lane| sent[lane] < limits[lane])
            .min_by_key(|&lane| (sent[lane] + 1) * periods[lane])
        {
            capture
                .push(
                    lane,
                    sent[lane] * periods[lane],
                    1000,
                    &[0.25 + lane as f32 * 0.5; 10],
                )
                .unwrap();
            sent[lane] += 1;
            capture.drain(false);
        }
        capture.drain(true);
        let a = capture.capture().track_snapshot(0).unwrap();
        let b = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), 20..60_020);
        assert_eq!(a.frame_range(), b.frame_range());
        assert!(a.samples[..59_980].iter().all(|&sample| sample == 0.25));
        assert_eq!(a.samples[59_980..], [0.; 20]);
        assert!(b.samples.iter().all(|&sample| sample == 0.75));
    }

    #[test]
    fn lane_lists_follow_the_capture_limits_and_refuse_lanes_past_the_last() {
        for widths in [&[][..], &[0], &[2, 0], &[1; crate::MAX_LANES + 1], &[32, 32, 1]] {
            assert!(matches!(
                SynchronizedCapture::new_lanes(1000, widths, DurationChoice::Seconds10),
                Err(CaptureError::InvalidChannelCount)
            ));
        }
        let mut capture =
            SynchronizedCapture::new_lanes(1000, &[1; crate::MAX_LANES], DurationChoice::Seconds10)
                .unwrap();
        assert_eq!(capture.lane_count(), crate::MAX_LANES);
        assert_eq!(capture.capture().lane_count(), crate::MAX_LANES);
        let past = crate::MAX_LANES;
        assert!(matches!(
            capture.push(past, 0, 1000, &[0.5; 10]),
            Err(CaptureError::InvalidChannelCount)
        ));
        assert!(matches!(
            capture.enable_frame_clock(past),
            Err(CaptureError::InvalidChannelCount)
        ));
        assert_eq!(capture.take_frame_clock_eviction(past), None);
        assert_eq!(capture.retained_frame_at(past, 0), std::task::Poll::Ready(None));
        // Every lane is pushed on its own and lands in its own channel of one frame grid.
        for lane in 0..crate::MAX_LANES {
            capture.push(lane, 0, 1000, &[lane as f32 / 64.0; 20]).unwrap();
        }
        capture.drain(true);
        for lane in 0..crate::MAX_LANES {
            let snapshot = capture.capture().track_snapshot(lane).unwrap();
            assert_eq!(snapshot.frame_range(), 0..20);
            assert_eq!(snapshot.samples, [lane as f32 / 64.0; 20]);
        }
        // The native path splits one physical source's frames across every lane.
        let mut native =
            SynchronizedCapture::new_lanes(1000, &[2, 12, 1], DurationChoice::Seconds10).unwrap();
        native.set_retain_silence(true);
        let frame: Vec<f32> = (0..15).map(|channel| channel as f32 / 16.0).collect();
        native.push_native_interleaved(0, &frame.repeat(30), false).unwrap();
        assert!(matches!(
            native.push_native_interleaved(30_000_000, &frame[..14], false),
            Err(CaptureError::IncompleteFrame)
        ));
        for (lane, channels) in [0..2, 2..14, 14..15].into_iter().enumerate() {
            let snapshot = native.capture().track_snapshot(lane).unwrap();
            assert_eq!(snapshot.samples, frame[channels].repeat(30));
        }
    }

    #[test]
    fn three_sources_align_when_one_starts_late_and_another_drops_out_and_returns() {
        let mut capture =
            SynchronizedCapture::new_lanes(1000, &[1, 2, 1], DurationChoice::Seconds10).unwrap();
        capture.enable_frame_clock(2).unwrap();
        // Lane A plays throughout. Lane B starts past the jitter window, so the grid has
        // already moved on without it. Lane C drops five packets, then comes back.
        for packet in 0..30u64 {
            let time = packet * 10_000_000;
            capture.push(0, time, 1000, &[0.25; 10]).unwrap();
            if packet >= 12 {
                capture.push(1, time, 1000, &[0.5; 20]).unwrap();
            }
            if !(16..21).contains(&packet) {
                capture.push(2, time, 1000, &[0.75; 10]).unwrap();
            }
            capture.drain(false);
        }
        capture.drain(true);
        let lanes: Vec<_> = (0..3)
            .map(|lane| capture.capture().track_snapshot(lane).unwrap())
            .collect();
        for lane in &lanes {
            assert_eq!(lane.frame_range(), 0..300);
        }
        assert_eq!(lanes[0].samples, [0.25; 300]);
        assert_eq!(&lanes[1].samples[..240], &[0.0; 240]);
        assert_eq!(&lanes[1].samples[240..], &[0.5; 360]);
        // The dropout fades the packet before it and the one after, a frame each, and the
        // gap between stays silent, without touching the other lanes.
        let c = &lanes[2].samples;
        assert_eq!(&c[..159], &[0.75; 159]);
        assert_eq!(&c[159..211], &[0.0; 52]);
        assert_eq!(&c[211..], &[0.75; 89]);
        // Lane C's own clock maps its timestamps to the same grid, and its gap to nothing.
        use std::task::Poll;
        assert_eq!(capture.retained_frame_at(2, 50_000_000), Poll::Ready(Some(50)));
        assert_eq!(capture.retained_frame_at(2, 250_000_000), Poll::Ready(Some(250)));
        assert_eq!(capture.retained_frame_at(2, 180_000_000), Poll::Ready(None));
    }

    #[test]
    fn both_lanes_share_silence_removal_and_long_gaps_have_bounded_cost() {
        let mut aligned =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        aligned.push(0, 0, 1000, &[0.5; 10]).unwrap();
        aligned.drain(true);
        aligned
            .push(1, 3_600_000_000_000, 1000, &[0.75; 10])
            .unwrap();
        aligned.drain(true);
        let a = aligned.capture().track_snapshot(0).unwrap();
        let b = aligned.capture().track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), 0..210);
        assert_eq!(a.frame_range(), b.frame_range());
        assert_eq!(a.samples[..10], [0.5; 10]);
        assert!(a.samples[10..].iter().all(|&sample| sample == 0.));
        assert!(b.samples[..200].iter().all(|&sample| sample == 0.));
        assert_eq!(b.samples[200..], [0.75; 10]);
    }
}

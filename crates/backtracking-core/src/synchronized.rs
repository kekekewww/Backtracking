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
}

impl ChannelRoute {
    pub const ALL: [Self; 6] = [
        Self::Left,
        Self::Right,
        Self::Center,
        Self::Mid,
        Self::Side,
        Self::Merged,
    ];

    pub const fn from_code(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Left),
            1 => Some(Self::Right),
            2 => Some(Self::Center),
            3 => Some(Self::Mid),
            4 => Some(Self::Side),
            5 => Some(Self::Merged),
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
        }
    }

    /// Multichannel sources supply a stereo downmix when using Merged.
    /// Other routes use the original channel layout, with L/R in positions 0/1.
    pub fn output_channels(self, native: usize, center: Option<usize>) -> Option<usize> {
        match self {
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
        if self == Self::Merged {
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
            Self::Merged => unreachable!(),
        }));
        Ok(())
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

struct Packet {
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
pub struct SynchronizedCapture {
    sample_rate: u32,
    lanes: [Option<Lane>; 2],
    capture: Capture,
    cursor: Option<u64>,
    native_end_ns: Option<u64>,
    scratch: Vec<f32>,
    bar_scratch: Vec<usize>,
    capture_error: Option<CaptureError>,
}

impl SynchronizedCapture {
    pub fn new(
        rate: u32,
        a_channels: usize,
        b_channels: Option<usize>,
        retention: DurationChoice,
    ) -> Result<Self, CaptureError> {
        let capture = Capture::new_tracks(rate, a_channels, b_channels, retention)?;
        Ok(Self {
            sample_rate: rate,
            lanes: [Some(Lane::new(a_channels)), b_channels.map(Lane::new)],
            capture,
            cursor: None,
            native_end_ns: None,
            scratch: Vec::with_capacity(DRAIN_FRAMES * (a_channels + b_channels.unwrap_or(0))),
            bar_scratch: Vec::new(),
            capture_error: None,
        })
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
        for lane in self.lanes.iter_mut().flatten() {
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
        let channels: usize = self.lanes.iter().flatten().map(|lane| lane.channels).sum();
        let frames = interleaved.len() / channels;
        if interleaved.len() % channels != 0 {
            return Err(CaptureError::IncompleteFrame);
        }
        if frames == 0 {
            return Ok(());
        }
        if let Some(end_ns) = self.native_end_ns {
            let gap_ns = timestamp_ns.saturating_sub(end_ns);
            if discontinuity || gap_ns > NATIVE_TIMESTAMP_JITTER_NS {
                let gap_frames = ((u128::from(gap_ns) * u128::from(self.sample_rate)
                    + NANOS_PER_SECOND / 2)
                    / NANOS_PER_SECOND)
                    .min(u128::from(u64::MAX))
                    as u64;
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
                    let mut remaining = gap_frames.min(frames_for_ms(self.sample_rate, GAP_TAIL_MS));
                    let have_gap = remaining != 0;
                    self.scratch.clear();
                    self.scratch.resize(DRAIN_FRAMES * channels, 0.);
                    while remaining > 0 {
                        let count = remaining.min(DRAIN_FRAMES as u64) as usize;
                        self.capture.push_interleaved(&self.scratch[..count * channels])?;
                        remaining -= count as u64;
                    }
                    if have_gap {
                        self.capture.smooth_next_start();
                    }
                }
            }
        }
        self.capture
            .push_interleaved_with_markers(interleaved, source_block_offset_of_bars)?;
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
        self.push_with_markers(lane, timestamp_ns, source_rate, interleaved, discontinuity, &[])
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
            .and_then(Option::as_mut)
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
            .flatten()
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
                .flatten()
                .map(|lane| lane.ready_end(fade_frames))
                .min()
                .unwrap_or(latest);
            let jitter = frames_for_ms(self.sample_rate, JITTER_MS);
            let candidate = settled.max(latest.saturating_sub(jitter));
            // Hold enough context to fade a lane's last packet while it can still catch up.
            // Once the jitter window has passed its end, that lane no longer delays the grid.
            self.lanes
                .iter()
                .flatten()
                .filter(|lane| lane.last_end.is_some_and(|end| end >= candidate))
                .map(|lane| lane.ready_end(fade_frames))
                .fold(candidate, u64::min)
        };
        // A source still missing after the jitter window is an actual dropout on this grid.
        // Fade its last available packet independently so the other lane stays intact.
        if !flush {
            for lane in self.lanes.iter_mut().flatten() {
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
                .flatten()
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
            for lane in self.lanes.iter_mut().flatten() {
                lane.discard_before(cursor);
            }
            let next_packet = self
                .lanes
                .iter()
                .flatten()
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
                    self.push_block(cursor, cursor + gap_tail, fade_frames);
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
        self.scratch.clear();
        self.bar_scratch.clear();
        for frame in start..end {
            let mut bar = false;
            for lane in self.lanes.iter_mut().flatten() {
                bar |= lane.append_frame(frame, fade_frames, &mut self.scratch);
            }
            if bar {
                self.bar_scratch.push((frame - start) as usize);
            }
        }
        // Each lane appends exactly its configured channel count to every frame.
        if let Err(error) = self
            .capture
            .push_interleaved_with_markers(&self.scratch, &self.bar_scratch)
        {
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
    fn retain_silence_keeps_native_quiet_gaps_and_bar_positions_on_both_lanes() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        assert!(!capture.retain_silence());
        capture.set_retain_silence(true);
        let first = [0.0001, -0.0002, 0., 0., -0.0001, 0.0002, 0., 0.];
        let last = [0.0003, -0.0004, 0., 0., 0.0002, -0.0001, 0., 0.];
        capture.push_native_interleaved_with_markers(0, &first, false, &[0, 3]).unwrap();
        capture.push_native_interleaved_with_markers(2_000_000_000, &last, false, &[1]).unwrap();
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
            capture.push_with_markers(lane, 0, 1000, &[0.; 10], false, &[2]).unwrap();
        }
        capture.drain(true);
        for lane in 0..2 {
            capture.push_with_markers(lane, 2_000_000_000, 1000, &[0.; 10], false, &[5]).unwrap();
        }
        capture.drain(true);
        let a = capture.capture().track_snapshot(0).unwrap();
        let b = capture.capture().track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), 0..2010);
        assert_eq!(a.frame_range(), b.frame_range());
        assert!(a.samples.iter().chain(&b.samples).all(|&sample| sample == 0.));
        assert_eq!(capture.capture().bar_markers(0..2010), vec![2, 2005]);
    }

    #[test]
    fn retain_silence_switch_does_not_restore_pending_or_earlier_unobserved_quiet() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, None, DurationChoice::Seconds10).unwrap();
        capture.push_with_markers(0, 0, 1000, &[0.; 100], false, &[10]).unwrap();
        capture.set_retain_silence(true);
        assert!(capture.capture().frame_range().is_empty());
        capture.push_with_markers(0, 5_000_000_000, 1000, &[0.; 10], false, &[2]).unwrap();
        capture.drain(true);
        assert_eq!(capture.capture().frame_range(), 0..10);
        assert_eq!(capture.capture().bar_markers(0..10), vec![2]);

        let before = capture.capture().snapshot();
        capture.set_retain_silence(false);
        assert_eq!(capture.capture().snapshot(), before);
        capture.set_retain_silence(true);
        assert_eq!(capture.capture().snapshot(), before);
        capture.push_native_interleaved(10_000_000_000, &[0.; 5], false).unwrap();
        capture.set_retain_silence(true);
        capture.push_native_interleaved(11_000_000_000, &[0.; 5], false).unwrap();
        assert_eq!(capture.capture().frame_range(), 0..1015);
    }

    #[test]
    fn retained_native_long_gap_rolls_history_without_losing_absolute_time() {
        let mut capture =
            SynchronizedCapture::new(10, 1, None, DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        capture.push_native_interleaved(0, &[0.125; 2], false).unwrap();
        capture.push_native_interleaved_with_markers(1_000_000_000_000, &[0.25; 2], false, &[1]).unwrap();
        let snapshot = capture.capture().snapshot();
        assert_eq!(snapshot.frame_range(), 9902..10002);
        assert!(snapshot.samples[..98].iter().all(|&sample| sample == 0.));
        assert_eq!(&snapshot.samples[98..], &[0.25; 2]);
        assert_eq!(capture.capture().bar_markers(snapshot.frame_range()), vec![10001]);
    }

    #[test]
    fn retain_silence_preserves_explicit_native_discontinuity_fades() {
        let mut capture =
            SynchronizedCapture::new(1000, 1, None, DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        capture.push_native_interleaved(0, &[0.25; 8], false).unwrap();
        capture.push_native_interleaved(9_000_000, &[0.75; 8], true).unwrap();
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
        capture.push_native_interleaved(0, &[0.25; 4], false).unwrap();
        let before = capture.capture().snapshot();
        capture.capture_mut().pause();
        capture.set_retain_silence(true);
        assert_eq!(capture.capture().state(), crate::CaptureState::Paused);
        assert_eq!(capture.capture().snapshot(), before);
        capture.push_native_interleaved(10_000_000_000, &[0.; 4], false).unwrap();
        capture.push_native_interleaved(12_000_000_000, &[0.; 4], false).unwrap();
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
            &[markers[0] + 2, markers[0] + 4, markers[0] + 14, markers[0] + 24]
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

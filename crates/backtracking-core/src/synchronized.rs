use std::collections::VecDeque;

use crate::export::ResamplingKernel;
use crate::{Capture, CaptureError, DurationChoice};

const NANOS_PER_SECOND: u128 = 1_000_000_000;
const JITTER_MS: u32 = 80;
const GAP_TAIL_MS: u32 = 250;
const MAX_PENDING_SECONDS: u32 = 2;
const DRAIN_FRAMES: usize = 2048;

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
    time_scale: f64,
}

impl Clock {
    fn time_scale(&self, timestamp_ns: u64, sample_rate: u32) -> Option<f64> {
        let elapsed = timestamp_ns.checked_sub(self.timestamp_ns)?;
        if self.sample_rate != sample_rate {
            return None;
        }
        let nominal = self.frames as f64 * NANOS_PER_SECOND as f64 / f64::from(sample_rate);
        let observed = elapsed as f64 / nominal;
        // A larger difference indicates a discontinuity, not clock drift.
        if !(0.995..=1.005).contains(&observed) {
            return None;
        }
        Some(self.time_scale * 0.75 + observed * 0.25)
    }
}

struct Packet {
    start: u64,
    end: u64,
    samples: Vec<f32>,
}

struct Lane {
    channels: usize,
    packets: VecDeque<Packet>,
    last_end: Option<u64>,
    clock: Option<Clock>,
    resampler: Option<(u32, ResamplingKernel)>,
}

impl Lane {
    fn new(channels: usize) -> Self {
        Self {
            channels,
            packets: VecDeque::new(),
            last_end: None,
            clock: None,
            resampler: None,
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
    }

    fn append_frame(&mut self, frame: u64, output: &mut Vec<f32>) {
        self.discard_before(frame);
        if let Some(packet) = self.packets.front().filter(|packet| packet.start <= frame) {
            let offset = (frame - packet.start) as usize * self.channels;
            output.extend_from_slice(&packet.samples[offset..offset + self.channels]);
        } else {
            output.resize(output.len() + self.channels, 0.);
        }
    }
}

/// Aligns timestamped input lanes on one sample grid, then feeds one silence gate.
/// Each packet timestamp is the monotonic host time of its first input frame.
pub struct SynchronizedCapture {
    sample_rate: u32,
    lanes: [Option<Lane>; 2],
    capture: Capture,
    cursor: Option<u64>,
    scratch: Vec<f32>,
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
            scratch: Vec::with_capacity(DRAIN_FRAMES * (a_channels + b_channels.unwrap_or(0))),
        })
    }

    pub fn capture(&self) -> &Capture {
        &self.capture
    }

    pub fn capture_mut(&mut self) -> &mut Capture {
        &mut self.capture
    }

    pub fn reset_input(&mut self) {
        for lane in self.lanes.iter_mut().flatten() {
            lane.reset();
        }
        self.cursor = None;
    }

    pub fn push(
        &mut self,
        lane: usize,
        timestamp_ns: u64,
        source_rate: u32,
        interleaved: &[f32],
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
        if lane
            .clock
            .as_ref()
            .is_some_and(|clock| timestamp_ns <= clock.timestamp_ns)
        {
            return Ok(());
        }
        let grid = u128::from(timestamp_ns) * u128::from(self.sample_rate);
        let integer_grid = (grid / NANOS_PER_SECOND) as u64;
        let fraction_grid = (grid % NANOS_PER_SECOND) as f64 / NANOS_PER_SECOND as f64;
        let estimated_scale = lane
            .clock
            .as_ref()
            .and_then(|clock| clock.time_scale(timestamp_ns, source_rate));
        let time_scale = estimated_scale.unwrap_or(1.);
        let native_duration = frames as f64 * f64::from(self.sample_rate) / f64::from(source_rate);
        let projected_end = (fraction_grid + native_duration * time_scale).round() as u64;
        let timestamp_start = integer_grid + fraction_grid.round() as u64;
        let mut start = timestamp_start;
        let end = (integer_grid + projected_end).max(start + 1);
        if let Some(previous_end) = lane.last_end {
            // Keep adjacent packets continuous when timestamp rounding differs by one frame.
            if estimated_scale.is_some() && start.abs_diff(previous_end) <= 1 {
                start = previous_end;
            } else {
                start = start.max(previous_end);
            }
        }
        lane.clock = Some(Clock {
            timestamp_ns,
            frames,
            sample_rate: source_rate,
            time_scale,
        });
        lane.last_end = Some(end.max(start));
        if start >= end {
            return Ok(());
        }
        let resample_start = start.min(timestamp_start);
        let output_frames = (end - resample_start) as usize;
        let first = start.max(self.cursor.unwrap_or(start));
        if first >= end {
            return Ok(());
        }
        let retained_frames = (end - first) as usize;
        let skipped = (first - resample_start) as usize;
        let samples = if output_frames == frames {
            interleaved[skipped * lane.channels..(skipped + retained_frames) * lane.channels]
                .to_vec()
        } else {
            let filter = if source_rate != self.sample_rate {
                if lane
                    .resampler
                    .as_ref()
                    .is_none_or(|(rate, _)| *rate != source_rate)
                {
                    let step = f64::from(source_rate) / f64::from(self.sample_rate);
                    lane.resampler = Some((source_rate, ResamplingKernel::new(step)));
                }
                lane.resampler.as_ref().map(|(_, kernel)| kernel)
            } else {
                None
            };
            resample_packet(
                interleaved,
                lane.channels,
                output_frames,
                skipped,
                retained_frames,
                filter,
            )
        };
        lane.packets.push_back(Packet {
            start: first,
            end,
            samples,
        });
        let backlog = u64::from(self.sample_rate) * u64::from(MAX_PENDING_SECONDS);
        lane.discard_before(end.saturating_sub(backlog));
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
        let target = if flush {
            latest
        } else {
            let settled = self
                .lanes
                .iter()
                .flatten()
                .map(|lane| lane.last_end.unwrap_or(0))
                .min()
                .unwrap_or(latest);
            let jitter = frames_for_ms(self.sample_rate, JITTER_MS);
            settled.max(latest.saturating_sub(jitter))
        };
        let Some(mut cursor) = self.cursor.or_else(|| {
            self.lanes
                .iter()
                .flatten()
                .filter_map(|lane| lane.packets.front().map(|packet| packet.start))
                .min()
        }) else {
            return;
        };
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
                self.push_block(cursor, cursor + gap_tail);
                cursor = gap_end;
            } else {
                let end = target.min(cursor + DRAIN_FRAMES as u64);
                self.push_block(cursor, end);
                cursor = end;
            }
        }
        self.cursor = Some(cursor);
    }

    fn push_block(&mut self, start: u64, end: u64) {
        self.scratch.clear();
        for frame in start..end {
            for lane in self.lanes.iter_mut().flatten() {
                lane.append_frame(frame, &mut self.scratch);
            }
        }
        // Each lane appends exactly its configured channel count to every frame.
        let _ = self.capture.push_interleaved(&self.scratch);
    }
}

fn frames_for_ms(rate: u32, milliseconds: u32) -> u64 {
    (u64::from(rate) * u64::from(milliseconds)).div_ceil(1_000)
}

fn resample_packet(
    samples: &[f32],
    channels: usize,
    output_frames: usize,
    skip: usize,
    count: usize,
    filter: Option<&ResamplingKernel>,
) -> Vec<f32> {
    let input_frames = samples.len() / channels;
    let mut output = Vec::with_capacity(count * channels);
    for frame in skip..skip + count {
        let position = frame as u64 * input_frames as u64;
        let index = (position / output_frames as u64) as usize;
        let weight = (position % output_frames as u64) as f64 / output_frames as f64;
        if let Some(filter) = filter {
            let frame = filter.at(samples, channels, index as f64 + weight);
            output.extend_from_slice(&frame[..channels]);
            continue;
        }
        let next = (index + 1).min(input_frames - 1);
        for channel in 0..channels {
            let a = f64::from(samples[index * channels + channel]);
            let b = f64::from(samples[next * channels + channel]);
            output.push((a + (b - a) * weight) as f32);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

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

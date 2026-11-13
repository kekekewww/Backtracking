mod history;

use std::collections::VecDeque;
use std::time::Instant;
use std::{error::Error, fmt, ops::Range};

use crate::meter::{Meter, MeterReadings};
use crate::waveform::WaveformView;
use history::History;

const OPEN_LEVEL: f32 = 0.001;
const CLOSE_LEVEL: f32 = 0.000_5;
const PRE_ROLL_MS: u32 = 40;
const TAIL_MS: u32 = 150;
const JOIN_FADE_MS: u32 = 2;
const MAX_BUFFER_SAMPLES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurationChoice {
    Seconds10,
    Seconds20,
    Seconds30,
    Seconds60,
    Minutes3,
    Minutes5,
    Minutes10,
}

impl DurationChoice {
    pub const ALL: [Self; 7] = [
        Self::Seconds10,
        Self::Seconds20,
        Self::Seconds30,
        Self::Seconds60,
        Self::Minutes3,
        Self::Minutes5,
        Self::Minutes10,
    ];

    pub const fn from_seconds(seconds: u32) -> Option<Self> {
        match seconds {
            10 => Some(Self::Seconds10),
            20 => Some(Self::Seconds20),
            30 => Some(Self::Seconds30),
            60 => Some(Self::Seconds60),
            180 => Some(Self::Minutes3),
            300 => Some(Self::Minutes5),
            600 => Some(Self::Minutes10),
            _ => None,
        }
    }

    pub const fn seconds(self) -> u32 {
        match self {
            Self::Seconds10 => 10,
            Self::Seconds20 => 20,
            Self::Seconds30 => 30,
            Self::Seconds60 => 60,
            Self::Minutes3 => 180,
            Self::Minutes5 => 300,
            Self::Minutes10 => 600,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureState {
    Waiting,
    Recording,
    Paused,
}

#[derive(Debug, Eq, PartialEq)]
pub enum CaptureError {
    InvalidSampleRate,
    InvalidChannelCount,
    IncompleteFrame,
    BufferTooLarge,
    AllocationFailed,
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSampleRate => "sample rate must be greater than zero",
            Self::InvalidChannelCount => "channel count must be greater than zero",
            Self::IncompleteFrame => "interleaved samples do not contain complete frames",
            Self::BufferTooLarge => "capture buffer exceeds the maximum supported size",
            Self::AllocationFailed => "could not allocate the capture buffer",
        })
    }
}

impl Error for CaptureError {}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
    /// The first frame's index on the retained-audio timeline.
    pub first_frame: u64,
}

impl Snapshot {
    pub fn frame_count(&self) -> usize {
        self.samples.len().checked_div(self.channels).unwrap_or(0)
    }

    pub fn frame_range(&self) -> Range<u64> {
        self.first_frame..self.first_frame.saturating_add(self.frame_count() as u64)
    }

    pub fn frames(&self, range: Range<usize>) -> Option<&[f32]> {
        if self.channels == 0 {
            return None;
        }
        let start = range.start.checked_mul(self.channels)?;
        let end = range.end.checked_mul(self.channels)?;
        self.samples.get(start..end)
    }
}

pub struct Capture {
    sample_rate: u32,
    channels: usize,
    a_channels: usize,
    retention_frames: usize,
    history: History,
    pre_roll: FrameRing,
    bar_markers: VecDeque<u64>,
    pending_bars: VecDeque<u64>,
    tail_frames: usize,
    quiet_frames: usize,
    join_fade_frames: usize,
    join_fade: JoinFade,
    state: CaptureState,
    revision: u64,
    meter: Meter,
    meter_b: Option<Meter>,
}

enum JoinFade {
    None,
    Pending {
        fade_tail: bool,
        wait_for_signal: bool,
    },
    FadeIn(usize),
}

impl JoinFade {
    fn push(
        &mut self,
        history: &mut History,
        frame: &[f32],
        fade_frames: usize,
    ) -> Result<(), CaptureError> {
        history.ensure_append_capacity()?;
        let remaining = match *self {
            Self::None => {
                history.push(frame, 1.0);
                return Ok(());
            }
            Self::Pending {
                fade_tail,
                wait_for_signal,
            } => {
                if fade_tail {
                    history.fade_out_tail(fade_frames);
                }
                if wait_for_signal && frame.iter().all(|&sample| sample == 0.0) {
                    // A gap's synthetic pre-roll must not use up the fade-in.
                    history.push(frame, 1.0);
                    *self = Self::Pending {
                        fade_tail: false,
                        wait_for_signal: true,
                    };
                    return Ok(());
                }
                fade_frames
            }
            Self::FadeIn(remaining) => remaining,
        };
        let gain = (fade_frames - remaining) as f32 / fade_frames.saturating_sub(1).max(1) as f32;
        history.push(frame, gain);
        *self = if remaining > 1 {
            Self::FadeIn(remaining - 1)
        } else {
            Self::None
        };
        Ok(())
    }
}

impl Capture {
    pub fn new(
        sample_rate: u32,
        channels: usize,
        retention: DurationChoice,
    ) -> Result<Self, CaptureError> {
        Self::build(sample_rate, channels, None, retention)
    }

    /// Creates a shared history for one or two independently metered mono/stereo lanes.
    pub fn new_tracks(
        sample_rate: u32,
        a_channels: usize,
        b_channels: Option<usize>,
        retention: DurationChoice,
    ) -> Result<Self, CaptureError> {
        if !(1..=2).contains(&a_channels)
            || b_channels.is_some_and(|channels| !(1..=2).contains(&channels))
        {
            return Err(CaptureError::InvalidChannelCount);
        }
        Self::build(sample_rate, a_channels, b_channels, retention)
    }

    fn build(
        sample_rate: u32,
        a_channels: usize,
        b_channels: Option<usize>,
        retention: DurationChoice,
    ) -> Result<Self, CaptureError> {
        if sample_rate == 0 {
            return Err(CaptureError::InvalidSampleRate);
        }
        if a_channels == 0 {
            return Err(CaptureError::InvalidChannelCount);
        }
        let channels = a_channels
            .checked_add(b_channels.unwrap_or(0))
            .ok_or(CaptureError::BufferTooLarge)?;
        let retention_frames = (sample_rate as usize)
            .checked_mul(retention.seconds() as usize)
            .ok_or(CaptureError::BufferTooLarge)?;
        let frames_for_ms = |ms| (u64::from(sample_rate) * u64::from(ms)).div_ceil(1_000) as usize;

        Ok(Self {
            sample_rate,
            channels,
            a_channels,
            retention_frames,
            history: History::new(sample_rate as usize, channels, a_channels)?,
            pre_roll: FrameRing::new(frames_for_ms(PRE_ROLL_MS), channels)?,
            bar_markers: VecDeque::new(),
            pending_bars: VecDeque::new(),
            tail_frames: frames_for_ms(TAIL_MS),
            quiet_frames: 0,
            join_fade_frames: frames_for_ms(JOIN_FADE_MS),
            join_fade: JoinFade::None,
            state: CaptureState::Waiting,
            revision: 0,
            meter: Meter::new(sample_rate, a_channels)?,
            meter_b: b_channels
                .map(|channels| Meter::new(sample_rate, channels))
                .transpose()?,
        })
    }

    pub fn push_interleaved(&mut self, samples: &[f32]) -> Result<(), CaptureError> {
        self.push_interleaved_with_markers(samples, &[])
    }

    /// Bar offsets are sorted positions within this block, before silence is removed.
    pub fn push_interleaved_with_markers(
        &mut self,
        samples: &[f32],
        source_block_offset_of_bars: &[usize],
    ) -> Result<(), CaptureError> {
        let frames = samples.chunks_exact(self.channels);
        if !frames.remainder().is_empty() {
            return Err(CaptureError::IncompleteFrame);
        }
        if self.state == CaptureState::Paused || samples.is_empty() {
            return Ok(());
        }

        let now = Instant::now();
        self.meter.begin_block(now);
        if let Some(meter) = &mut self.meter_b {
            meter.begin_block(now);
        }
        let previous_end = self.history.total_frames();
        let result = self.retain_frames(frames, source_block_offset_of_bars);
        if self.history.total_frames() != previous_end {
            self.revision = self.revision.wrapping_add(1);
            self.prune_bars();
        }
        let now = Instant::now();
        self.meter.end_block(now);
        if let Some(meter) = &mut self.meter_b {
            meter.end_block(now);
        }
        result
    }

    fn retain_frames<'a>(
        &mut self,
        frames: impl Iterator<Item = &'a [f32]>,
        source_block_offset_of_bars: &[usize],
    ) -> Result<(), CaptureError> {
        let mut bars = source_block_offset_of_bars.iter().copied().peekable();
        for (offset, frame) in frames.enumerate() {
            let mut bar_here = false;
            while let Some(&bar) = bars.peek().filter(|&&bar| bar <= offset) {
                bars.next();
                bar_here |= bar == offset;
            }
            let (a, b) = frame.split_at(self.a_channels);
            let mut peak = self.meter.push_frame(a);
            if let Some(meter) = &mut self.meter_b {
                peak = peak.max(meter.push_frame(b));
            }

            if self.state == CaptureState::Waiting {
                if peak < OPEN_LEVEL {
                    if bar_here {
                        self.pending_bars.push_back(self.pre_roll.total_frames);
                    }
                    self.pre_roll.push(frame);
                    let pre_roll_start = self.pre_roll.total_frames - self.pre_roll.len_frames as u64;
                    while self
                        .pending_bars
                        .front()
                        .is_some_and(|&frame| frame < pre_roll_start)
                    {
                        self.pending_bars.pop_front();
                    }
                    continue;
                }

                // The opening frame has not entered pre-roll, so it will be written only once.
                let (first, second) = self.pre_roll.as_slices();
                let pre_roll_start = self.pre_roll.total_frames - self.pre_roll.len_frames as u64;
                let retained_start = self.history.total_frames();
                let pre_roll_frames = self.pre_roll.len_frames;
                for (offset, pending_frame) in first
                    .chunks_exact(self.channels)
                    .chain(second.chunks_exact(self.channels))
                    .enumerate()
                {
                    if let Err(error) = self.join_fade
                        .push(&mut self.history, pending_frame, self.join_fade_frames)
                    {
                        self.commit_pre_roll_bars(pre_roll_start, retained_start, offset);
                        self.pre_roll.clear();
                        self.history.truncate_to(self.retention_frames);
                        return Err(error);
                    }
                }
                self.commit_pre_roll_bars(pre_roll_start, retained_start, pre_roll_frames);
                self.pre_roll.clear();
                self.history.truncate_to(self.retention_frames);
                self.state = CaptureState::Recording;
            }

            let retained_frame = self.history.total_frames();
            self.join_fade
                .push(&mut self.history, frame, self.join_fade_frames)?;
            if bar_here {
                self.bar_markers.push_back(retained_frame);
            }
            self.history.truncate_to(self.retention_frames);

            if peak < CLOSE_LEVEL {
                self.quiet_frames += 1;
                if self.quiet_frames == self.tail_frames {
                    self.quiet_frames = 0;
                    self.state = CaptureState::Waiting;
                }
            } else {
                self.quiet_frames = 0;
            }
        }
        Ok(())
    }

    fn commit_pre_roll_bars(&mut self, source_start: u64, retained_start: u64, frames: usize) {
        self.bar_markers.extend(self.pending_bars.drain(..).filter_map(|bar| {
            let offset = bar - source_start;
            (offset < frames as u64).then_some(retained_start + offset)
        }));
    }

    fn prune_bars(&mut self) {
        let start = self.frame_range().start;
        while self.bar_markers.front().is_some_and(|&frame| frame < start) {
            self.bar_markers.pop_front();
        }
    }

    /// Locates bar starts on the retained-audio timeline, using a half-open range.
    pub fn bar_markers(&self, range: Range<u64>) -> Vec<u64> {
        let first = self.bar_markers.partition_point(|&frame| frame < range.start);
        let end = self.bar_markers.partition_point(|&frame| frame < range.end);
        self.bar_markers
            .range(first..end.max(first))
            .copied()
            .collect()
    }

    pub fn pause(&mut self) {
        self.state = CaptureState::Paused;
        self.quiet_frames = 0;
        self.join_fade = JoinFade::None;
        self.pre_roll.clear();
        self.pending_bars.clear();
        self.meter.reset();
        if let Some(meter) = &mut self.meter_b {
            meter.reset();
        }
    }

    pub fn resume(&mut self) {
        if self.state == CaptureState::Paused {
            self.state = CaptureState::Waiting;
        }
    }

    /// Ramps down the retained tail and ramps up the resumed audio when it is appended.
    /// The retained history is unchanged until the next frame is kept by the gate.
    pub fn smooth_next_join(&mut self) {
        if !self.frame_range().is_empty() {
            self.join_fade = JoinFade::Pending {
                fade_tail: true,
                wait_for_signal: false,
            };
        }
    }

    pub(crate) fn smooth_next_start(&mut self) {
        self.join_fade = JoinFade::Pending {
            fade_tail: matches!(
                self.join_fade,
                JoinFade::Pending {
                    fade_tail: true,
                    ..
                }
            ),
            wait_for_signal: true,
        };
    }

    pub fn state(&self) -> CaptureState {
        self.state
    }

    pub fn meter_readings(&self) -> MeterReadings {
        self.meter.readings(Instant::now())
    }

    pub fn track_meter_readings(&self, lane: usize) -> Option<MeterReadings> {
        match lane {
            0 => Some(self.meter_readings()),
            1 => self
                .meter_b
                .as_ref()
                .map(|meter| meter.readings(Instant::now())),
            _ => None,
        }
    }

    fn track_range(&self, lane: usize) -> Option<Range<usize>> {
        match lane {
            0 => Some(0..self.a_channels),
            1 if self.a_channels < self.channels => Some(self.a_channels..self.channels),
            _ => None,
        }
    }

    pub fn track_channels(&self, lane: usize) -> Option<usize> {
        self.track_range(lane).map(|range| range.len())
    }

    /// Changes whenever the retained samples change.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// These indices describe audio kept by the shared silence gate.
    pub fn frame_range(&self) -> Range<u64> {
        self.history.frame_range()
    }

    pub fn snapshot(&self) -> Snapshot {
        let range = self.frame_range();
        let mut samples = Vec::with_capacity((range.end - range.start) as usize * self.channels);
        for segment in self.history.slices(range.clone()) {
            samples.extend_from_slice(segment);
        }
        Snapshot {
            sample_rate: self.sample_rate,
            channels: self.channels,
            samples,
            first_frame: range.start,
        }
    }

    pub fn track_snapshot(&self, lane: usize) -> Option<Snapshot> {
        self.track_snapshot_range(lane, self.frame_range())
    }

    /// Copies only the requested retained frames, preserving their absolute position.
    pub fn track_snapshot_range(&self, lane: usize, frames: Range<u64>) -> Option<Snapshot> {
        let channels = self.track_range(lane)?;
        let retained = self.frame_range();
        if frames.start < retained.start || frames.end > retained.end || frames.end < frames.start {
            return None;
        }
        let capacity = usize::try_from(frames.end - frames.start)
            .ok()?
            .checked_mul(channels.len())?;
        let mut samples = Vec::new();
        samples.try_reserve_exact(capacity).ok()?;
        for segment in self.history.slices(frames.clone()) {
            if channels.len() == self.channels {
                samples.extend_from_slice(segment);
            } else {
                for frame in segment.chunks_exact(self.channels) {
                    samples.extend_from_slice(&frame[channels.clone()]);
                }
            }
        }
        Some(Snapshot {
            sample_rate: self.sample_rate,
            channels: channels.len(),
            samples,
            first_frame: frames.start,
        })
    }

    /// Writes chronological waveform bins spanning the entire retained history.
    pub fn waveform(&self, minimum: &mut [f32], maximum: &mut [f32]) -> usize {
        self.waveform_for(0, self.frame_range(), minimum, maximum)
    }

    pub fn track_waveform(
        &self,
        lane: usize,
        view: WaveformView,
        minimum: &mut [f32],
        maximum: &mut [f32],
    ) -> usize {
        self.track_waveform_range(lane, view, self.frame_range(), minimum, maximum)
    }

    /// Writes peaks for the intersection of the requested range and retained history.
    pub fn track_waveform_range(
        &self,
        lane: usize,
        view: WaveformView,
        range: Range<u64>,
        minimum: &mut [f32],
        maximum: &mut [f32],
    ) -> usize {
        if self.track_range(lane).is_none() {
            minimum.fill(0.);
            maximum.fill(0.);
            return 0;
        }
        let retained = self.frame_range();
        let range = range.start.max(retained.start)..range.end.min(retained.end);
        self.waveform_for(1 + lane * 3 + view as usize, range, minimum, maximum)
    }

    fn waveform_for(
        &self,
        trace: usize,
        range: Range<u64>,
        minimum: &mut [f32],
        maximum: &mut [f32],
    ) -> usize {
        minimum.fill(0.0);
        maximum.fill(0.0);
        let bin_count = minimum.len().min(maximum.len());
        let frame_count = range.end.saturating_sub(range.start) as usize;
        if frame_count == 0 || bin_count == 0 {
            return frame_count;
        }

        let frame_for_bin =
            |bin| range.start + (bin as u128 * frame_count as u128 / bin_count as u128) as u64;

        for bin in 0..bin_count {
            let start = frame_for_bin(bin);
            // When there are more bins than frames, repeat the mapped frame.
            let end = frame_for_bin(bin + 1).max(start + 1);
            let (low, high) = self.history.extrema(trace, start..end);
            minimum[bin] = low;
            maximum[bin] = high;
        }
        frame_count
    }

    pub fn set_retention(&mut self, retention: DurationChoice) {
        self.retention_frames =
            (self.sample_rate as usize).saturating_mul(retention.seconds() as usize);
        let retained = self.frame_range();
        if retained.end - retained.start > self.retention_frames as u64 {
            self.revision = self.revision.wrapping_add(1);
        }
        self.history.truncate_to(self.retention_frames);
        self.history.release_unused_capacity(self.retention_frames);
        self.prune_bars();
    }

    /// Drops retained audio and pre-roll. An armed capture waits for fresh signal.
    pub fn clear(&mut self) {
        if !self.frame_range().is_empty() {
            self.revision = self.revision.wrapping_add(1);
        }
        self.history.clear();
        self.pre_roll.clear();
        self.bar_markers.clear();
        self.pending_bars.clear();
        self.quiet_frames = 0;
        self.join_fade = JoinFade::None;
        self.meter.reset();
        if let Some(meter) = &mut self.meter_b {
            meter.reset();
        }
        if self.state == CaptureState::Recording {
            self.state = CaptureState::Waiting;
        }
    }
}

struct FrameRing {
    samples: Vec<f32>,
    channels: usize,
    capacity_frames: usize,
    next_frame: usize,
    len_frames: usize,
    total_frames: u64,
}

impl FrameRing {
    fn new(capacity_frames: usize, channels: usize) -> Result<Self, CaptureError> {
        let sample_capacity = capacity_frames
            .checked_mul(channels)
            .filter(|&samples| samples <= MAX_BUFFER_SAMPLES)
            .ok_or(CaptureError::BufferTooLarge)?;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(sample_capacity)
            .map_err(|_| CaptureError::AllocationFailed)?;
        samples.resize(sample_capacity, 0.0);
        Ok(Self {
            samples,
            channels,
            capacity_frames,
            next_frame: 0,
            len_frames: 0,
            total_frames: 0,
        })
    }

    fn push(&mut self, frame: &[f32]) {
        self.push_with_gain(frame, 1.0);
    }

    fn push_with_gain(&mut self, frame: &[f32], gain: f32) {
        let offset = self.next_frame * self.channels;
        let destination = &mut self.samples[offset..offset + self.channels];
        if gain == 1.0 {
            destination.copy_from_slice(frame);
        } else {
            for (out, sample) in destination.iter_mut().zip(frame) {
                *out = sample * gain;
            }
        }
        self.next_frame += 1;
        if self.next_frame == self.capacity_frames {
            self.next_frame = 0;
        }
        self.len_frames = (self.len_frames + 1).min(self.capacity_frames);
        self.total_frames = self.total_frames.saturating_add(1);
    }

    fn clear(&mut self) {
        self.next_frame = 0;
        self.len_frames = 0;
    }

    fn as_slices(&self) -> (&[f32], &[f32]) {
        let start_frame =
            (self.next_frame + self.capacity_frames - self.len_frames) % self.capacity_frames;
        let first_frames = self.len_frames.min(self.capacity_frames - start_frame);
        let first_start = start_frame * self.channels;
        let first_end = (start_frame + first_frames) * self.channels;
        let second_end = (self.len_frames - first_frames) * self.channels;
        (
            &self.samples[first_start..first_end],
            &self.samples[..second_end],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_only_as_audio_is_kept_and_shrinks_without_moving_the_recording_boundary() {
        let mut capture = Capture::new_tracks(10, 1, Some(2), DurationChoice::Minutes10).unwrap();
        let original: Vec<f32> = (1..=6_000)
            .flat_map(|frame| [frame as f32, -(frame as f32), 0.5])
            .collect();
        assert_eq!(capture.history.allocated_frames(), 600);
        capture.push_interleaved(&original[..600]).unwrap();
        capture.set_retention(DurationChoice::Seconds30);
        assert_eq!(capture.snapshot().samples, original[..600]);
        capture.set_retention(DurationChoice::Minutes10);
        capture.push_interleaved(&original[600..]).unwrap();
        assert_eq!(capture.history.allocated_frames(), 6_000);

        capture.set_retention(DurationChoice::Minutes3);
        assert_eq!(capture.frame_range(), 4_200..6_000);
        assert_eq!(capture.history.allocated_frames(), 1_800);
        assert_eq!(capture.snapshot().samples, original[4_200 * 3..]);
        capture.set_retention(DurationChoice::Seconds30);
        assert_eq!(capture.history.allocated_frames(), 600);
        assert_eq!(capture.state(), CaptureState::Recording);
        capture.push_interleaved(&[6_001.0, -6_001.0, 0.5]).unwrap();
        assert_eq!(capture.frame_range(), 5_701..6_001);
        let a = capture.track_snapshot(0).unwrap();
        let b = capture.track_snapshot(1).unwrap();
        assert_eq!(a.frame_range(), b.frame_range());
        assert_eq!(
            a.samples,
            (5_702..=6_001).map(|i| i as f32).collect::<Vec<_>>()
        );
        assert_eq!(
            b.samples,
            (5_702..=6_001)
                .flat_map(|i| [-(i as f32), 0.5])
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn bar_markers_only_follow_frames_retained_by_the_gate() {
        let mut capture = Capture::new(100, 1, DurationChoice::Seconds10).unwrap();
        capture
            .push_interleaved_with_markers(&[0.0; 30], &[1, 26, 29])
            .unwrap();
        assert!(capture.bar_markers(0..10).is_empty());
        capture
            .push_interleaved_with_markers(&[0.5; 20], &[0, 10])
            .unwrap();
        assert_eq!(capture.frame_range(), 0..24);
        assert_eq!(capture.bar_markers(0..24), vec![0, 3, 4, 14]);
        assert_eq!(capture.bar_markers(3..14), vec![3, 4]);
        capture
            .push_interleaved_with_markers(&[0.0; 100], &[17, 96, 99])
            .unwrap();
        capture.pause();
        capture
            .push_interleaved_with_markers(&[0.5; 8], &[0, 7])
            .unwrap();
        capture.resume();
        capture
            .push_interleaved_with_markers(&[0.5; 8], &[0])
            .unwrap();
        assert_eq!(capture.bar_markers(0..100), vec![0, 3, 4, 14, 39]);
        capture.push_interleaved(&[0.5; 1_010]).unwrap();
        assert!(capture.bar_markers(0..u64::MAX).is_empty());
    }

    #[test]
    fn range_waveform_reads_only_the_requested_retained_frames() {
        let mut capture = Capture::new(4, 1, DurationChoice::Seconds10).unwrap();
        let audio: Vec<_> = (1..=60).map(|frame| frame as f32).collect();
        capture.push_interleaved(&audio).unwrap();
        let mut low = [0.0; 3];
        let mut high = [0.0; 3];
        assert_eq!(
            capture.track_waveform_range(0, WaveformView::Merged, 17..29, &mut low, &mut high),
            9
        );
        assert_eq!(low, [21.0, 24.0, 27.0]);
        assert_eq!(high, [23.0, 26.0, 29.0]);
        let clip = capture.track_snapshot_range(0, 23..29).unwrap();
        assert_eq!(clip.frame_range(), 23..29);
        assert_eq!(clip.samples, &audio[23..29]);
        assert!(capture.track_snapshot_range(0, 17..29).is_none());
    }

    #[test]
    fn resume_fades_both_lanes_across_fragmented_blocks_without_retiming() {
        let mut capture =
            Capture::new_tracks(48_000, 1, Some(2), DurationChoice::Seconds10).unwrap();
        let old = [0.75, -0.5, 0.25];
        let new = [-0.75, 0.5, -0.25];
        let clip_frames = 120;
        capture.push_interleaved(&old.repeat(clip_frames)).unwrap();
        capture.pause();
        let frozen = capture.snapshot();
        capture.smooth_next_join();
        capture.resume();
        assert_eq!(capture.snapshot(), frozen);

        for frames in [1, 7, 64, 48] {
            capture.push_interleaved(&new.repeat(frames)).unwrap();
        }
        let snapshot = capture.snapshot();
        let unchanged = clip_frames - 96;
        assert_eq!(snapshot.frame_range(), 0..240);
        assert_eq!(
            snapshot.frames(0..unchanged).unwrap(),
            old.repeat(unchanged)
        );
        assert_eq!(
            snapshot.frames(240 - unchanged..240).unwrap(),
            new.repeat(unchanged)
        );
        assert_eq!(snapshot.frames(119..121).unwrap(), [0.0; 6]);
        for (a, b) in snapshot
            .samples
            .chunks_exact(3)
            .zip(snapshot.samples[3..].chunks_exact(3))
        {
            assert!(a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.008));
        }
        for lane in 0..2 {
            let lane = capture.track_snapshot(lane).unwrap();
            assert_eq!(lane.frame_range(), snapshot.frame_range());
        }
    }

    #[test]
    fn deferred_join_covers_retained_pre_roll_and_a_wrapped_tail() {
        let mut capture = Capture::new(1_000, 1, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&[0.5; 60_001]).unwrap();
        capture.pause();
        capture.smooth_next_join();
        capture.resume();
        capture.push_interleaved(&[0.00025]).unwrap();
        let waiting = capture.snapshot();
        assert_eq!(waiting.frame_range(), 50_001..60_001);
        assert_eq!(waiting.samples.last(), Some(&0.5));

        capture.push_interleaved(&[-0.5]).unwrap();
        let resumed = capture.snapshot();
        assert_eq!(resumed.frame_range(), 50_003..60_003);
        assert_eq!(resumed.frames(9_996..10_000).unwrap(), [0.5, 0., 0., -0.5]);

        capture.smooth_next_join();
        capture.clear();
        capture.push_interleaved(&[0.5]).unwrap();
        assert_eq!(capture.snapshot().samples, [0.5]);
    }

    #[test]
    fn gap_fade_in_starts_after_silent_pre_roll() {
        let mut capture = Capture::new(1_000, 1, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&[0.5; 16]).unwrap();
        capture.smooth_next_join();
        capture.push_interleaved(&[0.0; 250]).unwrap();
        assert_eq!(capture.state(), CaptureState::Waiting);

        capture.smooth_next_start();
        capture.push_interleaved(&[-0.5]).unwrap();
        capture.push_interleaved(&[-0.5, -0.5]).unwrap();
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.frame_count(), 16 + 150 + 40 + 3);
        assert_eq!(&snapshot.samples[14..17], &[0.5, 0., 0.]);
        assert_eq!(&snapshot.samples[206..], &[0., -0.5, -0.5]);
    }

    #[test]
    fn waveform_routes_match_snapshot_reference_across_ring_wrap() {
        let mut capture =
            Capture::new_tracks(1_000, 1, Some(2), DurationChoice::Seconds10).unwrap();
        let mut samples: Vec<_> = (0..65_013)
            .flat_map(|frame| {
                [
                    (frame % 61 - 30) as f32 / 64.,
                    (frame % 67 - 33) as f32 / 64.,
                    (frame % 71 - 35) as f32 / 64.,
                ]
            })
            .collect();
        samples[59_999 * 3] = -4.;
        samples[60_000 * 3 + 1] = f32::NAN;
        samples[60_000 * 3 + 2] = 4.;
        samples[60_001 * 3 + 2] = f32::INFINITY;
        capture.push_interleaved(&samples).unwrap();
        assert!(capture.history.slices(capture.frame_range()).count() > 1);

        for bins in [7, 17, 280] {
            let mut low = vec![0.; bins];
            let mut high = vec![0.; bins + 1];
            let mut expected_low = low.clone();
            let mut expected_high = high.clone();
            for lane in 0..2 {
                let snapshot = capture.track_snapshot(lane).unwrap();
                for view in [
                    WaveformView::Merged,
                    WaveformView::Left,
                    WaveformView::Right,
                ] {
                    crate::waveform::view_peaks(
                        &snapshot.samples,
                        snapshot.channels,
                        view,
                        &mut expected_low,
                        &mut expected_high,
                    );
                    assert_eq!(
                        capture.track_waveform(lane, view, &mut low, &mut high),
                        10_000
                    );
                    assert_eq!(low, expected_low);
                    assert_eq!(high, expected_high);
                }
            }

            let snapshot = capture.snapshot();
            assert_eq!(capture.waveform(&mut low, &mut high), 10_000);
            for bin in 0..bins {
                let first = bin * 10_000 / bins;
                let end = (bin + 1) * 10_000 / bins;
                assert_eq!(
                    (low[bin], high[bin]),
                    crate::waveform::extrema(snapshot.frames(first..end).unwrap().iter().copied())
                );
            }
            assert_eq!(high[bins], 0.0);
        }
    }

    #[test]
    fn absolute_retained_frame_range_survives_wrap_and_clear() {
        let mut capture = Capture::new(1000, 1, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&[0.5; 65_000]).unwrap();
        assert_eq!(capture.frame_range(), 55_000..65_000);
        assert_eq!(capture.snapshot().frame_range(), 55_000..65_000);
        capture.clear();
        assert_eq!(capture.frame_range(), 65_000..65_000);
        capture.push_interleaved(&[0.75; 2]).unwrap();
        assert_eq!(capture.frame_range(), 65_000..65_002);
    }

    #[test]
    fn long_silence_preserves_history_and_joins_once_with_pre_roll_and_tail() {
        let mut capture = Capture::new(1_000, 1, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&[0.0; 200]).unwrap();
        assert_eq!(capture.state(), CaptureState::Waiting);
        assert_eq!(capture.snapshot().frame_count(), 0);

        capture.push_interleaved(&[0.5]).unwrap();
        assert_eq!(capture.state(), CaptureState::Recording);
        let mut expected = vec![0.0; 40];
        expected.push(0.5);

        capture.push_interleaved(&[0.0; 1_000]).unwrap();
        assert_eq!(capture.state(), CaptureState::Waiting);
        expected.extend_from_slice(&[0.0; 150]);
        assert_eq!(capture.snapshot().samples, expected);

        capture.push_interleaved(&[0.75]).unwrap();
        expected.extend_from_slice(&[0.0; 40]);
        expected.push(0.75);
        assert_eq!(capture.snapshot().samples, expected);
    }

    #[test]
    fn gate_uses_peak_across_channels_and_hysteresis() {
        let mut capture = Capture::new(1_000, 2, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&[0.0, 0.000_75]).unwrap();
        assert_eq!(capture.state(), CaptureState::Waiting);
        capture.push_interleaved(&[0.0, -0.01]).unwrap();
        assert_eq!(capture.state(), CaptureState::Recording);

        for _ in 0..200 {
            capture.push_interleaved(&[0.0, 0.000_75]).unwrap();
        }
        assert_eq!(capture.state(), CaptureState::Recording);
        assert_eq!(capture.snapshot().frame_count(), 202);
    }

    #[test]
    fn pause_ignores_input_and_resume_discards_pre_pause_pre_roll() {
        let mut capture = Capture::new(1_000, 1, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&[0.25]).unwrap();
        capture.push_interleaved(&[0.0; 150]).unwrap();
        capture.push_interleaved(&[0.000_25; 40]).unwrap();
        let before_pause = capture.snapshot();

        capture.pause();
        capture.push_interleaved(&[1.0; 1_000]).unwrap();
        assert_eq!(capture.state(), CaptureState::Paused);
        assert_eq!(capture.snapshot(), before_pause);

        capture.resume();
        assert_eq!(capture.state(), CaptureState::Waiting);
        capture.push_interleaved(&[0.5]).unwrap();
        assert_eq!(
            capture.snapshot().frame_count(),
            before_pause.frame_count() + 1
        );
        assert_eq!(capture.snapshot().samples.last(), Some(&0.5));
    }

    #[test]
    fn wrapped_stereo_history_keeps_frame_order_and_obeys_retention() {
        let mut capture = Capture::new(1, 2, DurationChoice::Seconds10).unwrap();
        let input: Vec<f32> = (1..=65)
            .flat_map(|frame| [frame as f32, -(frame as f32)])
            .collect();
        capture.push_interleaved(&input).unwrap();
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.frame_count(), 10);
        assert_eq!(snapshot.samples, &input[110..]);
        assert_eq!(snapshot.frames(3..7), Some(&input[116..124]));
        assert_eq!(snapshot.frames(0..11), None);

        let mut minimum = [0.0; 2];
        let mut maximum = [0.0; 2];
        assert_eq!(capture.waveform(&mut minimum, &mut maximum), 10);
        assert_eq!(minimum, [-60.0, -65.0]);
        assert_eq!(maximum, [60.0, 65.0]);

        capture.set_retention(DurationChoice::Seconds60);
        assert_eq!(capture.snapshot(), snapshot);
        capture.push_interleaved(&[66.0, -66.0]).unwrap();
        assert_eq!(capture.snapshot().frame_count(), 11);
        capture.set_retention(DurationChoice::Seconds10);
        assert_eq!(capture.snapshot().frames(0..1), Some(&[57.0, -57.0][..]));
    }

    #[test]
    fn invalid_interleaving_does_not_mutate_capture_and_clear_keeps_pause() {
        let mut capture = Capture::new(100, 2, DurationChoice::Seconds20).unwrap();
        assert_eq!(
            capture.push_interleaved(&[0.5, 0.5, 0.5]),
            Err(CaptureError::IncompleteFrame)
        );
        assert_eq!(capture.snapshot().frame_count(), 0);
        capture.push_interleaved(&[0.5, 0.5]).unwrap();
        capture.pause();
        capture.clear();
        assert_eq!(capture.state(), CaptureState::Paused);
        assert_eq!(capture.snapshot().frame_count(), 0);

        let mut minimum = [1.0; 3];
        let mut maximum = [1.0; 3];
        assert_eq!(capture.waveform(&mut minimum, &mut maximum), 0);
        assert_eq!(minimum, [0.0; 3]);
        assert_eq!(maximum, [0.0; 3]);
    }

    #[test]
    fn clear_discards_pre_roll_and_waits_for_fresh_signal() {
        let mut capture = Capture::new(1_000, 1, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&[0.000_25; 40]).unwrap();
        assert!(capture.meter_readings().peak_dbfs.is_finite());
        assert_eq!(capture.state(), CaptureState::Waiting);
        capture.clear();
        assert_eq!(capture.state(), CaptureState::Waiting);
        capture.push_interleaved(&[0.5]).unwrap();
        assert_eq!(capture.snapshot().samples, [0.5]);

        let old_revision = capture.revision();
        capture.clear();
        assert_eq!(capture.state(), CaptureState::Waiting);
        assert_eq!(capture.meter_readings(), MeterReadings::default());
        assert_ne!(capture.revision(), old_revision);
        assert_eq!(capture.snapshot().frame_count(), 0);
        capture.push_interleaved(&[0.0; 150]).unwrap();
        assert_eq!(capture.snapshot().frame_count(), 0);
        capture.push_interleaved(&[0.25]).unwrap();
        assert_eq!(capture.state(), CaptureState::Recording);
        assert!(capture.meter_readings().peak_dbfs.is_finite());
        capture.pause();
        capture.push_interleaved(&[1.0; 200]).unwrap();
        assert_eq!(capture.meter_readings(), MeterReadings::default());
    }
}

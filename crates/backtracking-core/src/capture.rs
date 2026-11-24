mod frozen;
mod history;

#[cfg(test)]
mod channel_cache_bench;
#[cfg(test)]
mod channel_cache_tests;
#[cfg(test)]
mod push_bench;

use std::collections::VecDeque;
use std::time::Instant;
use std::{error::Error, fmt, ops::Range};

use crate::meter::{Meter, MeterReadings};
use crate::waveform::{SpectrumBin, SpectrumRow, WaveformView};
pub use frozen::{FrozenCapture, FrozenTrack};
use history::{History, Layout};

const OPEN_LEVEL: f32 = 0.001;
const CLOSE_LEVEL: f32 = 0.000_5;
const PRE_ROLL_MS: u32 = 40;
const TAIL_MS: u32 = 150;
const JOIN_FADE_MS: u32 = 2;
/// Default limit on retained interleaved audio samples (256 MiB of float samples).
/// Page rounding, peak caches, pre-roll and pages held by frozen views are additional.
pub const DEFAULT_MAX_BUFFER_SAMPLES: usize = 64 * 1024 * 1024;

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
            Self::InvalidChannelCount => {
                "each lane must have 1 to 64 channels, at most 64 in total"
            }
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

/// The most lanes one capture holds: one per channel of the 64 it can hold in all.
pub const MAX_LANES: usize = 64;

/// One lane's channels within a frame and its meter.
struct LaneMeter {
    channels: Range<usize>,
    meter: Meter,
}

pub struct Capture {
    sample_rate: u32,
    channels: usize,
    retention_frames: usize,
    max_buffer_samples: usize,
    history: History,
    pre_roll: FrameRing,
    bar_markers: VecDeque<u64>,
    pending_bars: VecDeque<u64>,
    tail_frames: usize,
    quiet_frames: usize,
    join_fade_frames: usize,
    join_fade: JoinFade,
    retain_silence: bool,
    gate_recording: bool,
    state: CaptureState,
    revision: u64,
    /// Every lane in order, lane A first; never empty.
    lanes: Vec<LaneMeter>,
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
                    // Nothing has changed yet if a shared page could not be copied,
                    // so the fade is tried again with the next frame.
                    history.fade_out_tail(fade_frames)?;
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
        Self::new_with_budget(sample_rate, channels, retention, DEFAULT_MAX_BUFFER_SAMPLES)
    }

    /// Creates one lane with a limit on retained interleaved samples. The effective
    /// retention is shortened when the requested duration would exceed this limit.
    ///
    /// History reserves up to 60 seconds initially and grows as audio arrives. The
    /// limit excludes up to two pages of rounding/reuse space (each at most one
    /// second), peak caches, 40 ms pre-roll, meters and pages held by frozen views.
    /// The budget must hold at least one whole frame.
    pub fn new_with_budget(
        sample_rate: u32,
        channels: usize,
        retention: DurationChoice,
        max_buffer_samples: usize,
    ) -> Result<Self, CaptureError> {
        Self::new_tracks_with_budget(sample_rate, channels, None, retention, max_buffer_samples)
    }

    /// Creates a shared history for one or two independently metered lanes,
    /// each with 1 to 64 channels and at most 64 channels together.
    pub fn new_tracks(
        sample_rate: u32,
        a_channels: usize,
        b_channels: Option<usize>,
        retention: DurationChoice,
    ) -> Result<Self, CaptureError> {
        Self::new_tracks_with_budget(
            sample_rate,
            a_channels,
            b_channels,
            retention,
            DEFAULT_MAX_BUFFER_SAMPLES,
        )
    }

    /// Creates one or two lanes sharing a retained-sample budget. See
    /// [`Self::new_with_budget`] for the memory excluded from that budget.
    pub fn new_tracks_with_budget(
        sample_rate: u32,
        a_channels: usize,
        b_channels: Option<usize>,
        retention: DurationChoice,
        max_buffer_samples: usize,
    ) -> Result<Self, CaptureError> {
        let lanes = [a_channels, b_channels.unwrap_or(0)];
        let lanes = &lanes[..1 + usize::from(b_channels.is_some())];
        Self::new_lanes_with_budget(sample_rate, lanes, retention, max_buffer_samples)
    }

    /// Creates a shared history for `lane_channels.len()` independently metered lanes, lane
    /// `i` with `lane_channels[i]` channels side by side in each interleaved frame, lane A
    /// first. There are 1 to [`MAX_LANES`] lanes of 1 to 64 channels each, and at most 64
    /// channels in all; anything else is [`CaptureError::InvalidChannelCount`].
    ///
    /// Every lane shares one sample rate, frame index, retention and sample budget, the bar
    /// markers, pause, clear and revision, and one silence gate: quiet stretches are left out
    /// only while every lane is quiet, so the lanes stay sample-aligned. One or two lanes
    /// behave exactly as [`Self::new_tracks`] does.
    pub fn new_lanes(
        sample_rate: u32,
        lane_channels: &[usize],
        retention: DurationChoice,
    ) -> Result<Self, CaptureError> {
        Self::new_lanes_with_budget(
            sample_rate,
            lane_channels,
            retention,
            DEFAULT_MAX_BUFFER_SAMPLES,
        )
    }

    /// [`Self::new_lanes`] with a limit on retained interleaved samples, shared by every
    /// lane: the retention is shortened to `max_buffer_samples / total channels` frames when
    /// the requested duration would exceed it. See [`Self::new_with_budget`] for the memory
    /// excluded from that budget.
    pub fn new_lanes_with_budget(
        sample_rate: u32,
        lane_channels: &[usize],
        retention: DurationChoice,
        max_buffer_samples: usize,
    ) -> Result<Self, CaptureError> {
        if sample_rate == 0 {
            return Err(CaptureError::InvalidSampleRate);
        }
        if lane_channels.is_empty()
            || lane_channels.len() > MAX_LANES
            || lane_channels.iter().any(|channels| !(1..=64).contains(channels))
        {
            return Err(CaptureError::InvalidChannelCount);
        }
        let channels: usize = lane_channels.iter().sum();
        if channels > 64 {
            return Err(CaptureError::InvalidChannelCount);
        }
        let budget_frames = max_buffer_samples / channels;
        if budget_frames == 0 {
            return Err(CaptureError::BufferTooLarge);
        }
        let retention_frames = (sample_rate as usize)
            .saturating_mul(retention.seconds() as usize)
            .min(budget_frames);
        let frames_for_ms = |ms| (u64::from(sample_rate) * u64::from(ms)).div_ceil(1_000) as usize;
        let history = History::new(sample_rate as usize, lane_channels, budget_frames)?;
        let lanes = history
            .layout()
            .lanes()
            .map(|channels| {
                let meter = Meter::new(sample_rate, channels.len())?;
                Ok(LaneMeter { channels, meter })
            })
            .collect::<Result<_, CaptureError>>()?;

        Ok(Self {
            sample_rate,
            channels,
            retention_frames,
            max_buffer_samples,
            history,
            pre_roll: FrameRing::new(frames_for_ms(PRE_ROLL_MS), channels)?,
            bar_markers: VecDeque::new(),
            pending_bars: VecDeque::new(),
            tail_frames: frames_for_ms(TAIL_MS),
            quiet_frames: 0,
            join_fade_frames: frames_for_ms(JOIN_FADE_MS),
            join_fade: JoinFade::None,
            retain_silence: false,
            gate_recording: false,
            state: CaptureState::Waiting,
            revision: 0,
            lanes,
        })
    }

    /// How many lanes this capture records, from 1 to [`MAX_LANES`].
    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    fn begin_meter_block(&mut self, now: Instant) {
        for lane in &mut self.lanes {
            lane.meter.begin_block(now);
        }
    }

    fn end_meter_block(&mut self, now: Instant) {
        for lane in &mut self.lanes {
            lane.meter.end_block(now);
        }
    }

    fn reset_meters(&mut self) {
        for lane in &mut self.lanes {
            lane.meter.reset();
        }
    }

    /// Meters each lane's part of `frame` and returns the frame's peak across every lane,
    /// which the one silence gate shared by all lanes follows.
    #[inline]
    fn meter_frame(lanes: &mut [LaneMeter], frame: &[f32]) -> f32 {
        // One or two lanes split the frame as captures did before lanes could be more.
        match lanes {
            [a] => a.meter.push_frame(frame),
            [a, b] => {
                let (first, second) = frame.split_at(b.channels.start);
                a.meter.push_frame(first).max(b.meter.push_frame(second))
            }
            lanes => {
                let mut peak = 0.0f32;
                for lane in lanes {
                    peak = peak.max(lane.meter.push_frame(&frame[lane.channels.clone()]));
                }
                peak
            }
        }
    }

    pub fn push_interleaved(&mut self, samples: &[f32]) -> Result<(), CaptureError> {
        self.push_interleaved_with_markers(samples, &[])
    }

    pub(crate) fn pre_roll_capacity(&self) -> usize {
        self.pre_roll.capacity_frames
    }

    pub fn retain_silence(&self) -> bool {
        self.retain_silence
    }

    /// Applies to future input without restoring discarded pre-roll or changing pause.
    pub fn set_retain_silence(&mut self, enabled: bool) {
        if self.retain_silence == enabled {
            return;
        }
        if enabled {
            self.gate_recording = self.state == CaptureState::Recording;
        } else if self.state != CaptureState::Paused {
            self.state = if self.gate_recording {
                CaptureState::Recording
            } else {
                CaptureState::Waiting
            };
        }
        self.retain_silence = enabled;
        self.pre_roll.clear();
        self.pending_bars.clear();
    }

    pub(crate) fn append_known_silence(&mut self, frames: u64) -> Result<(), CaptureError> {
        if !self.retain_silence || self.state == CaptureState::Paused || frames == 0 {
            return Ok(());
        }
        let previous_end = self.history.total_frames();
        previous_end
            .checked_add(frames)
            .ok_or(CaptureError::BufferTooLarge)?;
        let mut zero = Vec::new();
        zero.try_reserve_exact(self.channels)
            .map_err(|_| CaptureError::AllocationFailed)?;
        zero.resize(self.channels, 0.0);
        self.begin_meter_block(Instant::now());
        let prefix = if matches!(self.join_fade, JoinFade::None) {
            0
        } else {
            frames.min(self.join_fade_frames as u64)
        };
        let result = self
            .retain_frames(
                std::iter::repeat_n(zero.as_slice(), prefix as usize),
                &[],
                |_, _| {},
            )
            .and_then(|()| {
                self.history
                    .append_silence(frames - prefix, self.retention_frames)
            });
        let retained = self.history.total_frames() - previous_end;
        let bulk = retained.saturating_sub(prefix);
        if bulk != 0 {
            self.advance_quiet_gate(bulk);
            self.state = CaptureState::Recording;
            // An input gap this long is already stale to the meters. Short gaps
            // still replay every frame, preserving their release/window behavior.
            if bulk >= u64::from(self.sample_rate) {
                self.reset_meters();
            } else {
                for _ in 0..bulk {
                    Self::meter_frame(&mut self.lanes, &zero);
                }
            }
        }
        self.end_meter_block(Instant::now());
        if retained != 0 {
            self.revision = self.revision.wrapping_add(1);
            self.prune_bars();
        }
        result
    }

    fn advance_quiet_gate(&mut self, frames: u64) {
        if self.gate_recording {
            if frames >= self.tail_frames.saturating_sub(self.quiet_frames) as u64 {
                self.gate_recording = false;
                self.quiet_frames = 0;
            } else {
                self.quiet_frames += frames as usize;
            }
        }
    }

    /// Bar offsets are sorted positions within this block, before silence is removed.
    pub fn push_interleaved_with_markers(
        &mut self,
        samples: &[f32],
        source_block_offset_of_bars: &[usize],
    ) -> Result<(), CaptureError> {
        self.push_observed(samples, source_block_offset_of_bars, |_, _| {})
    }

    pub(crate) fn push_observed(
        &mut self,
        samples: &[f32],
        bars: &[usize],
        observe: impl FnMut(usize, Option<(u64, usize)>),
    ) -> Result<(), CaptureError> {
        let frames = samples.chunks_exact(self.channels);
        if !frames.remainder().is_empty() {
            return Err(CaptureError::IncompleteFrame);
        }
        if self.state == CaptureState::Paused || samples.is_empty() {
            return Ok(());
        }

        self.begin_meter_block(Instant::now());
        let previous_end = self.history.total_frames();
        let result = self.retain_frames(frames, bars, observe);
        if self.history.total_frames() != previous_end {
            self.revision = self.revision.wrapping_add(1);
            self.prune_bars();
        }
        self.end_meter_block(Instant::now());
        result
    }

    fn retain_frames<'a>(
        &mut self,
        frames: impl Iterator<Item = &'a [f32]>,
        source_block_offset_of_bars: &[usize],
        mut observe: impl FnMut(usize, Option<(u64, usize)>),
    ) -> Result<(), CaptureError> {
        let mut bars = source_block_offset_of_bars.iter().copied().peekable();
        for (offset, frame) in frames.enumerate() {
            let mut bar_here = false;
            while let Some(&bar) = bars.peek().filter(|&&bar| bar <= offset) {
                bars.next();
                bar_here |= bar == offset;
            }
            let peak = Self::meter_frame(&mut self.lanes, frame);

            if self.retain_silence {
                let retained_frame = self.history.total_frames();
                self.join_fade
                    .push(&mut self.history, frame, self.join_fade_frames)?;
                observe(offset, Some((retained_frame, 0)));
                if bar_here {
                    self.bar_markers.push_back(retained_frame);
                }
                self.history.truncate_to(self.retention_frames);
                if !self.gate_recording && peak >= OPEN_LEVEL {
                    self.gate_recording = true;
                    self.quiet_frames = 0;
                }
                if peak < CLOSE_LEVEL {
                    self.advance_quiet_gate(1);
                } else {
                    self.quiet_frames = 0;
                }
                self.state = CaptureState::Recording;
                continue;
            }

            let mut committed_pre_roll = 0;
            if self.state == CaptureState::Waiting {
                if peak < OPEN_LEVEL {
                    if bar_here {
                        self.pending_bars.push_back(self.pre_roll.total_frames);
                    }
                    observe(offset, None);
                    self.pre_roll.push(frame);
                    let pre_roll_start =
                        self.pre_roll.total_frames - self.pre_roll.len_frames as u64;
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
                committed_pre_roll = pre_roll_frames;
                for (offset, pending_frame) in first
                    .chunks_exact(self.channels)
                    .chain(second.chunks_exact(self.channels))
                    .enumerate()
                {
                    if let Err(error) =
                        self.join_fade
                            .push(&mut self.history, pending_frame, self.join_fade_frames)
                    {
                        self.commit_pre_roll_bars(pre_roll_start, retained_start, offset);
                        self.pre_roll.clear();
                        self.history.truncate_to(self.retention_frames);
                        return Err(error);
                    }
                    // A small budget may hold less than the pre-roll itself.
                    self.history.truncate_to(self.retention_frames);
                }
                self.commit_pre_roll_bars(pre_roll_start, retained_start, pre_roll_frames);
                self.pre_roll.clear();
                self.history.truncate_to(self.retention_frames);
                self.state = CaptureState::Recording;
            }

            let retained_frame = self.history.total_frames();
            self.join_fade
                .push(&mut self.history, frame, self.join_fade_frames)?;
            observe(offset, Some((retained_frame, committed_pre_roll)));
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
        self.bar_markers
            .extend(self.pending_bars.drain(..).filter_map(|bar| {
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
        let first = self
            .bar_markers
            .partition_point(|&frame| frame < range.start);
        let end = self.bar_markers.partition_point(|&frame| frame < range.end);
        self.bar_markers
            .range(first..end.max(first))
            .copied()
            .collect()
    }

    pub fn pause(&mut self) {
        self.state = CaptureState::Paused;
        self.gate_recording = false;
        self.quiet_frames = 0;
        self.join_fade = JoinFade::None;
        self.pre_roll.clear();
        self.pending_bars.clear();
        self.reset_meters();
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

    /// The meter of lane A.
    pub fn meter_readings(&self) -> MeterReadings {
        self.lanes[0].meter.readings(Instant::now())
    }

    /// The meter of `lane`, or `None` past the last lane.
    pub fn track_meter_readings(&self, lane: usize) -> Option<MeterReadings> {
        let lane = self.lanes.get(lane)?;
        Some(lane.meter.readings(Instant::now()))
    }

    fn track_range(&self, lane: usize) -> Option<Range<usize>> {
        self.lanes.get(lane).map(|lane| lane.channels.clone())
    }

    /// The channels of `lane`, or `None` past the last lane.
    pub fn track_channels(&self, lane: usize) -> Option<usize> {
        self.track_range(lane).map(|range| range.len())
    }

    /// Active native channels in a wide lane, bit zero for its first channel.
    /// Returns zero for a missing or narrow lane.
    pub fn active_channels(&self, lane: usize) -> u64 {
        let Some(track) = self.track_range(lane) else {
            return 0;
        };
        self.history.active_channels(track)
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

    /// Pins all retained audio as it is now. The view shares the capture's complete
    /// one-second pages, so it costs a pointer per retained second and a copy of at
    /// most the one page still being written. Audio pushed, faded, trimmed or cleared
    /// afterwards never changes it: the capture copies a page the view holds before
    /// writing to it, and leaves pages that roll out of the history to the view.
    pub fn freeze(&self) -> FrozenCapture {
        let frames = self.frame_range();
        FrozenCapture::new(
            self.sample_rate,
            self.history.layout(),
            self.history.share(&frames),
            frames,
        )
    }

    /// Pins only the requested retained frames, preserving their absolute position.
    pub fn freeze_range(&self, frames: Range<u64>) -> Option<FrozenCapture> {
        let retained = self.frame_range();
        if frames.start < retained.start || frames.end > retained.end || frames.end < frames.start {
            return None;
        }
        Some(FrozenCapture::new(
            self.sample_rate,
            self.history.layout(),
            self.history.share(&frames),
            frames,
        ))
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
        self.waveform_for(Layout::trace(lane, view), range, minimum, maximum)
    }

    /// Writes the finite min/max samples of one zero-based channel within a lane.
    /// Reads interleaved pages directly without copying a lane. The range uses the
    /// retained-audio timeline; invalid lanes or channels produce zeros and return 0.
    /// Shared page peaks cover complete blocks. Extra channels use 16-frame peaks
    /// and 128-frame summaries, built and extended by reads, never audio appends.
    pub fn track_channel_waveform_range(
        &self,
        lane: usize,
        channel: usize,
        range: Range<u64>,
        minimum: &mut [f32],
        maximum: &mut [f32],
    ) -> usize {
        let Some(track) = self.track_range(lane).filter(|track| channel < track.len()) else {
            minimum.fill(0.0);
            maximum.fill(0.0);
            return 0;
        };
        let retained = self.frame_range();
        let range = range.start.max(retained.start)..range.end.min(retained.end);
        waveform_bins(range, minimum, maximum, |frames| {
            self.history.channel_extrema(track.start + channel, frames)
        })
    }

    /// Writes the band energies of one waveform row of `lane` for each chronological bin of
    /// the retained part of `range`, and returns its frame count. The bins are the waveform
    /// calls' bins over the same range, so bin `i` colours waveform column `i`. A bin is
    /// `None` where it has no audio (silence included). An invalid lane or row clears `bins`
    /// and returns 0.
    ///
    /// Band energies are built per page and channel by the first read that needs them, then
    /// cached for later reads and frozen copies, so a first read of a long history is slow
    /// (about half a second for 10 minutes of 48 kHz stereo in a release build; a cached read
    /// of the same takes about 2 ms): prefer a frozen copy off the audio thread's lock for
    /// that. See [`SpectrumRow`] for the
    /// rows and [`crate::waveform::SPECTRUM_BAND_EDGES_HZ`] for the bands.
    pub fn track_spectrum_range(
        &self,
        lane: usize,
        row: SpectrumRow<'_>,
        range: Range<u64>,
        bins: &mut [SpectrumBin],
    ) -> usize {
        let Some(members) = self.track_range(lane).and_then(|track| row.members(track)) else {
            bins.fill(None);
            return 0;
        };
        let retained = self.frame_range();
        let range = range.start.max(retained.start)..range.end.min(retained.end);
        self.history
            .spectrum(self.sample_rate, &members, range, bins)
    }

    /// How many retained pages hold band energies that a spectrum read, of this capture or of
    /// a frozen copy, has started. Zero until something reads a spectrum, which is how an app
    /// that never colours its waveform by frequency can check it never paid for one.
    pub fn spectrum_cache_pages(&self) -> usize {
        self.history.spectrum_pages()
    }

    fn waveform_for(
        &self,
        trace: usize,
        range: Range<u64>,
        minimum: &mut [f32],
        maximum: &mut [f32],
    ) -> usize {
        waveform_bins(range, minimum, maximum, |frames| {
            self.history.extrema(trace, frames)
        })
    }

    pub fn set_retention(&mut self, retention: DurationChoice) {
        self.retention_frames = (self.sample_rate as usize)
            .saturating_mul(retention.seconds() as usize)
            .min(self.max_buffer_samples / self.channels);
        let retained = self.frame_range();
        if retained.end - retained.start > self.retention_frames as u64 {
            self.revision = self.revision.wrapping_add(1);
        }
        self.history.truncate_to(self.retention_frames);
        self.history.release_unused_capacity(self.retention_frames);
        self.prune_bars();
    }

    /// The maximum retained frame count after applying the requested duration and budget.
    pub fn effective_retention_frames(&self) -> usize {
        self.retention_frames
    }

    /// The maximum retained duration after applying the sample budget, in seconds.
    pub fn effective_retention_seconds(&self) -> f64 {
        self.retention_frames as f64 / f64::from(self.sample_rate)
    }

    /// The configured limit on retained interleaved audio samples.
    pub fn max_buffer_samples(&self) -> usize {
        self.max_buffer_samples
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
        self.gate_recording = false;
        self.join_fade = JoinFade::None;
        self.reset_meters();
        if self.state == CaptureState::Recording {
            self.state = CaptureState::Waiting;
        }
    }
}

/// Writes one pair of extrema per chronological bin of `range` and returns its frame count.
fn waveform_bins(
    range: Range<u64>,
    minimum: &mut [f32],
    maximum: &mut [f32],
    extrema: impl Fn(Range<u64>) -> (f32, f32),
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
        let (low, high) = extrema(start..end);
        minimum[bin] = low;
        maximum[bin] = high;
    }
    frame_count
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
            .filter(|&samples| samples <= DEFAULT_MAX_BUFFER_SAMPLES)
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
    fn sample_budget_shortens_retention_and_initial_reserve() {
        let channels = 14;
        let mut capture = Capture::new_tracks_with_budget(
            100,
            12,
            Some(2),
            DurationChoice::Minutes10,
            channels * 15 + 5,
        )
        .unwrap();
        assert_eq!(capture.max_buffer_samples(), 215);
        assert_eq!(capture.effective_retention_frames(), 15);
        assert_eq!(capture.effective_retention_seconds(), 0.15);
        assert_eq!(capture.history.allocated_frames(), 15);
        capture.set_retain_silence(true);
        let samples: Vec<_> = (0..98 * channels)
            .map(|sample| sample as f32 / 4096.0)
            .collect();
        capture.push_interleaved(&samples).unwrap();
        assert_eq!(capture.frame_range(), 83..98);
        assert_eq!(capture.snapshot().samples, samples[83 * channels..]);
        assert!(capture.history.allocated_frames() <= 15 + 2 * 15);
        capture.set_retention(DurationChoice::Seconds10);
        assert_eq!(capture.effective_retention_frames(), 15);
        assert_eq!(capture.freeze().snapshot(), capture.snapshot());
        capture.clear();
        assert_eq!(capture.history.allocated_frames(), 15);
    }

    #[test]
    fn retention_changes_stay_inside_the_configured_budget() {
        let mut capture =
            Capture::new_with_budget(100, 12, DurationChoice::Seconds10, 1_500 * 12).unwrap();
        assert_eq!(capture.effective_retention_frames(), 1_000);
        capture.set_retention(DurationChoice::Minutes10);
        assert_eq!(capture.effective_retention_frames(), 1_500);
        assert_eq!(capture.effective_retention_seconds(), 15.0);
        capture.set_retain_silence(true);
        capture.push_interleaved(&vec![0.5; 1_600 * 12]).unwrap();
        let frozen = capture.freeze();
        assert_eq!(capture.frame_range(), 100..1_600);
        capture.set_retention(DurationChoice::Seconds10);
        assert_eq!(capture.frame_range(), 600..1_600);
        assert_eq!(frozen.frame_range(), 100..1_600);
        assert_eq!(frozen.snapshot().samples.len(), 1_500 * 12);
    }

    #[test]
    fn channel_limits_and_a_budget_smaller_than_one_frame_are_rejected() {
        for (a, b) in [
            (0, None),
            (65, None),
            (1, Some(0)),
            (63, Some(2)),
            (1, Some(64)),
        ] {
            assert!(matches!(
                Capture::new_tracks(100, a, b, DurationChoice::Seconds10),
                Err(CaptureError::InvalidChannelCount),
            ));
        }
        assert!(Capture::new_tracks(100, 62, Some(2), DurationChoice::Seconds10).is_ok());
        assert!(matches!(
            Capture::new_with_budget(100, 12, DurationChoice::Seconds10, 11),
            Err(CaptureError::BufferTooLarge),
        ));
        assert!(matches!(
            Capture::new_with_budget(100, 1, DurationChoice::Seconds10, 0),
            Err(CaptureError::BufferTooLarge),
        ));
    }

    #[test]
    fn lane_lists_hold_up_to_sixty_four_lanes_and_sixty_four_channels() {
        for widths in [
            &[][..],
            &[0],
            &[65],
            &[2, 0, 1],
            &[1; MAX_LANES + 1],
            &[32, 32, 1],
            &[60, 2, 2, 1],
        ] {
            assert!(
                matches!(
                    Capture::new_lanes(100, widths, DurationChoice::Seconds10),
                    Err(CaptureError::InvalidChannelCount),
                ),
                "{widths:?}"
            );
        }
        assert_eq!(
            CaptureError::InvalidChannelCount.to_string(),
            "each lane must have 1 to 64 channels, at most 64 in total"
        );
        assert!(matches!(
            Capture::new_lanes(0, &[1, 1, 1], DurationChoice::Seconds10),
            Err(CaptureError::InvalidSampleRate),
        ));
        assert!(matches!(
            Capture::new_lanes_with_budget(100, &[2, 12, 1], DurationChoice::Seconds10, 14),
            Err(CaptureError::BufferTooLarge),
        ));
        for widths in [&[1; MAX_LANES][..], &[64], &[2, 12, 1], &[32, 32], &[1, 2, 3, 58]] {
            let capture = Capture::new_lanes(100, widths, DurationChoice::Seconds10).unwrap();
            assert_eq!(capture.lane_count(), widths.len());
            for (lane, &width) in widths.iter().enumerate() {
                assert_eq!(capture.track_channels(lane), Some(width), "{widths:?}");
                assert!(capture.track_meter_readings(lane).is_some());
            }
            // Past the last lane there is nothing to read, and nothing panics.
            let past = widths.len();
            assert_eq!(capture.track_channels(past), None);
            assert!(capture.track_meter_readings(past).is_none());
            assert!(capture.track_snapshot(past).is_none());
            assert_eq!(capture.active_channels(past), 0);
            let (mut low, mut high) = ([9.0; 3], [9.0; 3]);
            let range = 0..u64::MAX;
            assert_eq!(
                capture.track_waveform_range(past, WaveformView::Merged, range.clone(), &mut low, &mut high),
                0
            );
            assert_eq!((low, high), ([0.0; 3], [0.0; 3]));
            assert_eq!(capture.track_channel_waveform_range(past, 0, range, &mut low, &mut high), 0);
        }
        // The pair constructors are the same captures as one or two lanes.
        let layout = |capture: Capture| {
            let layout = capture.history.layout();
            (capture.lane_count(), layout.page_frames, layout.channels, layout.lane_starts)
        };
        assert_eq!(
            layout(Capture::new_tracks(100, 2, Some(12), DurationChoice::Seconds10).unwrap()),
            (2, 100, 14, 0b101)
        );
        assert_eq!(
            layout(Capture::new_lanes(100, &[2, 12], DurationChoice::Seconds10).unwrap()),
            (2, 100, 14, 0b101)
        );
        assert_eq!(layout(Capture::new(100, 3, DurationChoice::Seconds10).unwrap()), (1, 100, 3, 1));
        assert_eq!(
            layout(Capture::new_lanes(100, &[2, 12, 1], DurationChoice::Seconds10).unwrap()),
            (3, 100, 15, 1 | 1 << 2 | 1 << 14)
        );
    }

    /// Interleaves `lanes`, each of `widths[i]` channels, into whole frames of all of them.
    fn interleave(widths: &[usize], lanes: &[Vec<f32>]) -> Vec<f32> {
        let frames = lanes[0].len() / widths[0];
        (0..frames)
            .flat_map(|frame| {
                lanes
                    .iter()
                    .zip(widths)
                    .flat_map(move |(lane, &width)| lane[frame * width..(frame + 1) * width].iter().copied())
            })
            .collect()
    }

    #[test]
    fn mixed_width_lanes_keep_their_own_channels_views_and_meters() {
        let widths = [2, 12, 1];
        let frames = 1_337;
        let lanes: Vec<Vec<f32>> = widths
            .iter()
            .enumerate()
            .map(|(lane, &width)| {
                (0..frames * width)
                    .map(|i| ((i * 31 + lane * 7) % 409) as f32 / 409.0 * (lane + 1) as f32 * 0.25 - 0.1)
                    .collect()
            })
            .collect();
        let mut capture = Capture::new_lanes(1_000, &widths, DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        capture.push_interleaved(&interleave(&widths, &lanes)).unwrap();
        assert_eq!(capture.frame_range(), 0..frames as u64);
        let frozen = capture.freeze();
        assert_eq!(frozen.track_count(), 3);
        assert!(frozen.track(3).is_none());
        for (lane, (&width, samples)) in widths.iter().zip(&lanes).enumerate() {
            let alone = FrozenTrack::from_interleaved(1_000, width, 0, samples).unwrap();
            let snapshot = capture.track_snapshot(lane).unwrap();
            assert_eq!(snapshot.channels, width);
            assert_eq!(&snapshot.samples, samples);
            let track = frozen.track(lane).unwrap();
            assert_eq!(track.snapshot(), snapshot);
            for view in [WaveformView::Merged, WaveformView::Left, WaveformView::Right] {
                for (range, bins) in [(0..frames as u64, 7), (129..1_024, 300), (5..6, 1)] {
                    let mut expected = (vec![0.0; bins], vec![0.0; bins]);
                    alone.waveform(view, range.clone(), &mut expected.0, &mut expected.1);
                    let mut live = (vec![9.0; bins], vec![9.0; bins]);
                    capture.track_waveform_range(lane, view, range.clone(), &mut live.0, &mut live.1);
                    assert_eq!(live, expected, "lane {lane} {view:?} {range:?}");
                    let mut held = (vec![9.0; bins], vec![9.0; bins]);
                    track.waveform(view, range.clone(), &mut held.0, &mut held.1);
                    assert_eq!(held, expected, "lane {lane} {view:?} {range:?}");
                }
            }
            for channel in 0..width {
                let mut expected = (vec![0.0; 50], vec![0.0; 50]);
                alone.channel_waveform(channel, 0..frames as u64, &mut expected.0, &mut expected.1);
                let mut live = (vec![9.0; 50], vec![9.0; 50]);
                capture.track_channel_waveform_range(lane, channel, 0..frames as u64, &mut live.0, &mut live.1);
                assert_eq!(live, expected, "lane {lane} channel {channel}");
            }
            assert_eq!(capture.active_channels(lane), alone.active_channels(), "lane {lane}");
            // Each lane is metered on its own channels: louder lanes read higher.
            let peak = capture.track_meter_readings(lane).unwrap().peak_dbfs;
            if lane > 0 {
                assert!(peak > capture.track_meter_readings(lane - 1).unwrap().peak_dbfs, "lane {lane}");
            }
        }
    }

    #[test]
    fn sixty_four_mono_lanes_each_hold_their_own_channel() {
        let widths = [1; MAX_LANES];
        let mut capture = Capture::new_lanes(1_000, &widths, DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        let frames = 700;
        let audio: Vec<f32> = (0..frames * MAX_LANES)
            .map(|i| {
                let (frame, lane) = (i / MAX_LANES, i % MAX_LANES);
                (lane + 1) as f32 / 64.0 * if frame % 2 == 0 { 1.0 } else { -0.5 }
            })
            .collect();
        capture.push_interleaved(&audio).unwrap();
        let frozen = capture.freeze();
        assert_eq!(frozen.track_count(), MAX_LANES);
        for lane in 0..MAX_LANES {
            let level = (lane + 1) as f32 / 64.0;
            let expected: Vec<f32> = audio.iter().skip(lane).step_by(MAX_LANES).copied().collect();
            assert_eq!(capture.track_snapshot(lane).unwrap().samples, expected);
            assert_eq!(frozen.track(lane).unwrap().snapshot().samples, expected);
            for view in [WaveformView::Merged, WaveformView::Left, WaveformView::Right] {
                let (mut low, mut high) = ([0.0; 1], [0.0; 1]);
                capture.track_waveform(lane, view, &mut low, &mut high);
                assert_eq!((low[0], high[0]), (-0.5 * level, level), "lane {lane}");
            }
        }
    }

    #[test]
    fn one_loud_lane_among_quiet_ones_opens_the_shared_gate_for_every_lane() {
        let widths = [1, 2, 1, 3, 1];
        let channels: usize = widths.iter().sum();
        let mut capture = Capture::new_lanes(1_000, &widths, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&vec![0.0; 60 * channels]).unwrap();
        assert_eq!(capture.state(), CaptureState::Waiting);
        // Only lane D, the fourth, has something to say, on its last channel.
        let mut loud = vec![0.0; channels];
        loud[6] = 0.75;
        capture.push_interleaved(&loud).unwrap();
        assert_eq!(capture.state(), CaptureState::Recording);
        assert_eq!(capture.frame_range(), 0..41);
        for (lane, &width) in widths.iter().enumerate() {
            let snapshot = capture.track_snapshot(lane).unwrap();
            assert_eq!(snapshot.frame_range(), 0..41, "lane {lane}");
            let mut last = vec![0.0; width];
            if lane == 3 {
                last[2] = 0.75;
            }
            assert_eq!(snapshot.frames(40..41).unwrap(), last, "lane {lane}");
            let peak = capture.track_meter_readings(lane).unwrap().peak_dbfs;
            if lane == 3 {
                assert!(peak > -3.0, "{peak}");
            } else {
                assert!(peak < -60.0, "lane {lane}: {peak}");
            }
        }
        // The gate closes only once every lane has been quiet long enough.
        let mut quiet_but_one = vec![0.0; channels];
        quiet_but_one[0] = 0.5;
        capture.push_interleaved(&quiet_but_one.repeat(400)).unwrap();
        assert_eq!(capture.state(), CaptureState::Recording);
        assert_eq!(capture.frame_range(), 0..441);
        capture.push_interleaved(&vec![0.0; 2_000 * channels]).unwrap();
        assert_eq!(capture.state(), CaptureState::Waiting);
        let kept = capture.frame_range();
        assert!(kept.end < 2_441, "{kept:?}");
        for lane in 0..widths.len() {
            assert_eq!(capture.track_snapshot(lane).unwrap().frame_range(), kept);
        }
    }

    #[test]
    fn last_channel_opens_the_gate_and_all_channels_receive_join_fades() {
        let mut capture = Capture::new(1_000, 12, DurationChoice::Seconds10).unwrap();
        capture.push_interleaved(&vec![0.0; 60 * 12]).unwrap();
        assert_eq!(capture.state(), CaptureState::Waiting);
        let mut last_channel = [0.0; 12];
        last_channel[11] = 0.75;
        capture.push_interleaved(&last_channel).unwrap();
        assert_eq!(capture.state(), CaptureState::Recording);
        assert_eq!(capture.frame_range(), 0..41);
        assert_eq!(&capture.snapshot().samples[40 * 12..], &last_channel);
        capture.push_interleaved(&[0.5; 36]).unwrap();
        let frozen = capture.freeze();
        capture.smooth_next_join();
        capture.push_interleaved(&[0.75; 36]).unwrap();
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.frames(42..43).unwrap(), &[0.5; 12]);
        assert_eq!(snapshot.frames(43..45).unwrap(), &[0.0; 24]);
        assert_eq!(snapshot.frames(45..47).unwrap(), &[0.75; 24]);
        assert_eq!(frozen.snapshot().frames(43..44).unwrap(), &[0.5; 12]);
    }

    #[test]
    fn a_one_frame_budget_survives_pre_roll_and_gate_reopening() {
        let mut capture =
            Capture::new_with_budget(1_000, 64, DurationChoice::Minutes10, 64).unwrap();
        capture.push_interleaved(&[0.0; 64 * 50]).unwrap();
        let mut frame = [0.0; 64];
        frame[63] = 0.5;
        capture.push_interleaved(&frame).unwrap();
        assert_eq!(capture.frame_range(), 40..41);
        assert_eq!(capture.snapshot().samples, frame);
        capture.push_interleaved(&[0.0; 64 * 200]).unwrap();
        assert_eq!(capture.state(), CaptureState::Waiting);
        capture.push_interleaved(&frame).unwrap();
        assert_eq!(capture.snapshot().samples, frame);
        assert_eq!(capture.effective_retention_frames(), 1);
        assert!(capture.history.allocated_frames() <= 3);
    }

    #[test]
    fn retained_silence_is_opt_in_and_does_not_restore_discarded_pre_roll() {
        let mut capture = Capture::new(1_000, 1, DurationChoice::Seconds10).unwrap();
        assert!(!capture.retain_silence());
        capture
            .push_interleaved_with_markers(&[0.; 100], &[20])
            .unwrap();
        assert!(capture.snapshot().samples.is_empty());

        capture.set_retain_silence(true);
        assert_eq!(capture.state(), CaptureState::Waiting);
        let quiet: Vec<_> = [0., 0.000_01, -0.000_4, 0., 0.000_2].repeat(20);
        capture
            .push_interleaved_with_markers(&quiet, &[0, 50, 99])
            .unwrap();
        assert_eq!(capture.snapshot().samples, quiet);
        assert_eq!(capture.frame_range(), 0..100);
        assert_eq!(capture.bar_markers(0..100), [0, 50, 99]);
        assert_eq!(capture.state(), CaptureState::Recording);

        let previous = capture.snapshot();
        capture.set_retain_silence(false);
        assert_eq!(capture.snapshot(), previous);
        capture.push_interleaved(&[0.; 1_000]).unwrap();
        let later = capture.snapshot();
        assert_eq!(&later.samples[..100], previous.samples);
        assert!(later.frame_count() < 1_100);
        assert_eq!(capture.state(), CaptureState::Waiting);
    }

    #[test]
    fn silence_mode_preserves_manual_pause_clear_and_existing_dual_track_samples() {
        let mut capture =
            Capture::new_tracks(1_000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        capture
            .push_interleaved(&[0.000_1, -0.000_2, 0., 0.])
            .unwrap();
        let fixed = capture.snapshot();
        capture.pause();
        capture.set_retain_silence(false);
        capture.set_retain_silence(true);
        capture.push_interleaved(&[0.; 20]).unwrap();
        capture.append_known_silence(1_000).unwrap();
        assert_eq!(capture.state(), CaptureState::Paused);
        assert_eq!(capture.snapshot(), fixed);
        capture.resume();
        capture.push_interleaved(&[0.000_3, -0.000_4]).unwrap();
        assert_eq!(
            capture.snapshot().samples,
            [0.000_1, -0.000_2, 0., 0., 0.000_3, -0.000_4]
        );
        assert_eq!(
            capture.track_snapshot(0).unwrap().frame_range(),
            capture.track_snapshot(1).unwrap().frame_range()
        );
        capture.clear();
        assert!(capture.retain_silence());
        capture.push_interleaved(&[0., 0.]).unwrap();
        assert_eq!(capture.snapshot().samples, [0., 0.]);
        capture.push_interleaved(&[0.5, -0.5]).unwrap();
        capture.clear();
        capture.push_interleaved(&[0., 0.]).unwrap();
        capture.set_retain_silence(false);
        capture.push_interleaved(&[0.; 20]).unwrap();
        assert_eq!(capture.snapshot().samples, [0., 0.]);
    }

    #[test]
    fn long_retained_quiet_gap_advances_the_clock_with_bounded_history() {
        let mut capture =
            Capture::new_tracks(1_000, 1, Some(1), DurationChoice::Seconds10).unwrap();
        capture.set_retain_silence(true);
        capture
            .push_interleaved_with_markers(&[0.2, -0.3, 0.4, -0.5], &[0])
            .unwrap();
        let fixed = capture.snapshot();
        capture.append_known_silence(1_000_000).unwrap();
        assert_eq!(capture.frame_range(), 990_002..1_000_002);
        assert_eq!(capture.snapshot().samples, vec![0.; 20_000]);
        assert!(capture.bar_markers(0..u64::MAX).is_empty());
        assert_eq!(capture.state(), CaptureState::Recording);
        assert!(capture.history.allocated_frames() <= 60_000);
        assert_eq!(fixed.samples, [0.2, -0.3, 0.4, -0.5]);
        capture.push_interleaved(&[0.000_1, -0.000_2]).unwrap();
        assert_eq!(capture.frame_range(), 990_003..1_000_003);
        assert_eq!(
            capture.track_snapshot(0).unwrap().samples.last(),
            Some(&0.000_1)
        );
        assert_eq!(
            capture.track_snapshot(1).unwrap().samples.last(),
            Some(&-0.000_2)
        );
        let previous = capture.snapshot();
        assert_eq!(
            capture.append_known_silence(u64::MAX),
            Err(CaptureError::BufferTooLarge)
        );
        assert_eq!(capture.snapshot(), previous);
    }

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

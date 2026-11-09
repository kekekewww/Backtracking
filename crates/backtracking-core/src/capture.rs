use std::time::Instant;
use std::{error::Error, fmt, ops::Range};

use crate::meter::{Meter, MeterReadings};

const OPEN_LEVEL: f32 = 0.001;
const CLOSE_LEVEL: f32 = 0.000_5;
const PRE_ROLL_MS: u32 = 40;
const TAIL_MS: u32 = 150;
const MAX_BUFFER_SAMPLES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurationChoice {
    Seconds10,
    Seconds20,
    Seconds30,
}

impl DurationChoice {
    pub const fn seconds(self) -> u32 {
        match self {
            Self::Seconds10 => 10,
            Self::Seconds20 => 20,
            Self::Seconds30 => 30,
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
}

impl Snapshot {
    pub fn frame_count(&self) -> usize {
        self.samples.len().checked_div(self.channels).unwrap_or(0)
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
    retention_frames: usize,
    history: FrameRing,
    pre_roll: FrameRing,
    tail_frames: usize,
    quiet_frames: usize,
    state: CaptureState,
    revision: u64,
    meter: Meter,
}

impl Capture {
    pub fn new(
        sample_rate: u32,
        channels: usize,
        retention: DurationChoice,
    ) -> Result<Self, CaptureError> {
        if sample_rate == 0 {
            return Err(CaptureError::InvalidSampleRate);
        }
        if channels == 0 {
            return Err(CaptureError::InvalidChannelCount);
        }

        let capacity_frames = usize::try_from(
            u64::from(sample_rate) * u64::from(DurationChoice::Seconds30.seconds()),
        )
        .map_err(|_| CaptureError::BufferTooLarge)?;
        let frames_for_ms = |ms| (u64::from(sample_rate) * u64::from(ms)).div_ceil(1_000) as usize;

        Ok(Self {
            sample_rate,
            channels,
            retention_frames: sample_rate as usize * retention.seconds() as usize,
            history: FrameRing::new(capacity_frames, channels)?,
            pre_roll: FrameRing::new(frames_for_ms(PRE_ROLL_MS), channels)?,
            tail_frames: frames_for_ms(TAIL_MS),
            quiet_frames: 0,
            state: CaptureState::Waiting,
            revision: 0,
            meter: Meter::new(sample_rate, channels)?,
        })
    }

    pub fn push_interleaved(&mut self, samples: &[f32]) -> Result<(), CaptureError> {
        let frames = samples.chunks_exact(self.channels);
        if !frames.remainder().is_empty() {
            return Err(CaptureError::IncompleteFrame);
        }
        if self.state == CaptureState::Paused || samples.is_empty() {
            return Ok(());
        }

        self.meter.begin_block(Instant::now());
        let mut changed = false;
        for frame in frames {
            let peak = self.meter.push_frame(frame);

            if self.state == CaptureState::Waiting {
                if peak < OPEN_LEVEL {
                    self.pre_roll.push(frame);
                    continue;
                }

                // The opening frame has not entered pre-roll, so it will be written only once.
                let (first, second) = self.pre_roll.as_slices();
                for pending_frame in first
                    .chunks_exact(self.channels)
                    .chain(second.chunks_exact(self.channels))
                {
                    self.history.push(pending_frame);
                }
                self.pre_roll.clear();
                self.state = CaptureState::Recording;
            }

            self.history.push(frame);
            self.history.truncate_to(self.retention_frames);
            changed = true;

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
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        self.meter.end_block(Instant::now());
        Ok(())
    }

    pub fn pause(&mut self) {
        self.state = CaptureState::Paused;
        self.quiet_frames = 0;
        self.pre_roll.clear();
        self.meter.reset();
    }

    pub fn resume(&mut self) {
        if self.state == CaptureState::Paused {
            self.state = CaptureState::Waiting;
        }
    }

    pub fn state(&self) -> CaptureState {
        self.state
    }

    pub fn meter_readings(&self) -> MeterReadings {
        self.meter.readings(Instant::now())
    }

    /// Changes whenever the retained samples change.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn snapshot(&self) -> Snapshot {
        let (first, second) = self.history.as_slices();
        let mut samples = Vec::with_capacity(first.len() + second.len());
        samples.extend_from_slice(first);
        samples.extend_from_slice(second);
        Snapshot {
            sample_rate: self.sample_rate,
            channels: self.channels,
            samples,
        }
    }

    /// Writes chronological waveform bins spanning the entire retained history.
    pub fn waveform(&self, minimum: &mut [f32], maximum: &mut [f32]) -> usize {
        minimum.fill(0.0);
        maximum.fill(0.0);
        let bin_count = minimum.len().min(maximum.len());
        let frame_count = self.history.len_frames;
        if frame_count == 0 || bin_count == 0 {
            return frame_count;
        }

        let (first, second) = self.history.as_slices();
        let first_frames = first.len() / self.channels;
        let frame_for_bin = |bin| (bin as u128 * frame_count as u128 / bin_count as u128) as usize;

        for bin in 0..bin_count {
            let start = frame_for_bin(bin);
            // When there are more bins than frames, repeat the mapped frame.
            let end = frame_for_bin(bin + 1).max(start + 1);
            let first_start = start.min(first_frames) * self.channels;
            let first_end = end.min(first_frames) * self.channels;
            let second_start = start.saturating_sub(first_frames) * self.channels;
            let second_end = end.saturating_sub(first_frames) * self.channels;
            let mut samples = first[first_start..first_end]
                .iter()
                .chain(&second[second_start..second_end])
                .copied()
                .filter(|sample| sample.is_finite());

            if let Some(first_sample) = samples.next() {
                let (low, high) = samples
                    .fold((first_sample, first_sample), |(low, high), sample| {
                        (low.min(sample), high.max(sample))
                    });
                minimum[bin] = low;
                maximum[bin] = high;
            }
        }
        frame_count
    }

    pub fn set_retention(&mut self, retention: DurationChoice) {
        self.retention_frames = self.sample_rate as usize * retention.seconds() as usize;
        if self.history.len_frames > self.retention_frames {
            self.revision = self.revision.wrapping_add(1);
        }
        self.history.truncate_to(self.retention_frames);
    }

    /// Drops retained audio and pre-roll. An armed capture waits for fresh signal.
    pub fn clear(&mut self) {
        if self.history.len_frames != 0 {
            self.revision = self.revision.wrapping_add(1);
        }
        self.history.clear();
        self.pre_roll.clear();
        self.quiet_frames = 0;
        self.meter.reset();
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
        })
    }

    fn push(&mut self, frame: &[f32]) {
        let offset = self.next_frame * self.channels;
        self.samples[offset..offset + self.channels].copy_from_slice(frame);
        self.next_frame += 1;
        if self.next_frame == self.capacity_frames {
            self.next_frame = 0;
        }
        self.len_frames = (self.len_frames + 1).min(self.capacity_frames);
    }

    fn truncate_to(&mut self, capacity_frames: usize) {
        self.len_frames = self.len_frames.min(capacity_frames);
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
        let input: Vec<f32> = (1..=35)
            .flat_map(|frame| [frame as f32, -(frame as f32)])
            .collect();
        capture.push_interleaved(&input).unwrap();
        let snapshot = capture.snapshot();
        assert_eq!(snapshot.frame_count(), 10);
        assert_eq!(snapshot.samples, &input[50..]);
        assert_eq!(snapshot.frames(3..7), Some(&input[56..64]));
        assert_eq!(snapshot.frames(0..11), None);

        let mut minimum = [0.0; 2];
        let mut maximum = [0.0; 2];
        assert_eq!(capture.waveform(&mut minimum, &mut maximum), 10);
        assert_eq!(minimum, [-30.0, -35.0]);
        assert_eq!(maximum, [30.0, 35.0]);

        capture.set_retention(DurationChoice::Seconds30);
        assert_eq!(capture.snapshot(), snapshot);
        capture.push_interleaved(&[36.0, -36.0]).unwrap();
        assert_eq!(capture.snapshot().frame_count(), 11);
        capture.set_retention(DurationChoice::Seconds10);
        assert_eq!(capture.snapshot().frames(0..1), Some(&[27.0, -27.0][..]));
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

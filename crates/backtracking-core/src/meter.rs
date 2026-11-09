use std::time::{Duration, Instant};

use crate::CaptureError;

const PEAK_RELEASE_DB_PER_SECOND: f64 = 30.0;
const VU_WINDOW_MS: u32 = 300;
const LOUDNESS_WINDOW_MS: u32 = 400;
const STALE_AFTER: Duration = Duration::from_millis(400);
const MIN_ENERGY: f64 = 1e-12;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterReadings {
    /// Sample peak, with instantaneous attack and a 30 dB/s release.
    pub peak_dbfs: f32,
    /// 300 ms RMS, calibrated to 0 VU for a 1 kHz sine at −18 dBFS peak.
    pub vu: f32,
    /// Ungated 400 ms momentary loudness for mono and stereo.
    pub momentary_lufs: f32,
}

impl Default for MeterReadings {
    fn default() -> Self {
        Self {
            peak_dbfs: f32::NEG_INFINITY,
            vu: f32::NEG_INFINITY,
            momentary_lufs: f32::NEG_INFINITY,
        }
    }
}

pub(crate) struct Meter {
    peak: f64,
    peak_decay: f64,
    rms: EnergyWindow,
    loudness: Option<EnergyWindow>,
    weighting: [BiquadCoefficients; 2],
    delay: [[BiquadState; 2]; 2],
    last_input: Option<Instant>,
}

impl Meter {
    pub(crate) fn new(sample_rate: u32, channels: usize) -> Result<Self, CaptureError> {
        let at_48khz = [
            // ITU-R BS.1770-5 Annex 1 Tables 1 and 2.
            BiquadCoefficients {
                b: [
                    1.535_124_859_586_97,
                    -2.691_696_189_406_38,
                    1.198_392_810_852_85,
                ],
                a: [1.0, -1.690_659_293_182_41, 0.732_480_774_215_85],
            },
            BiquadCoefficients {
                b: [1.0, -2.0, 1.0],
                a: [1.0, -1.990_047_454_833_98, 0.990_072_250_366_21],
            },
        ];
        Ok(Self {
            peak: 0.0,
            peak_decay: 10.0_f64
                .powf(-PEAK_RELEASE_DB_PER_SECOND / (20.0 * f64::from(sample_rate))),
            rms: EnergyWindow::new(sample_rate, VU_WINDOW_MS)?,
            loudness: if channels <= 2 {
                Some(EnergyWindow::new(sample_rate, LOUDNESS_WINDOW_MS)?)
            } else {
                None
            },
            weighting: at_48khz.map(|filter| filter.at_sample_rate(sample_rate)),
            delay: [[BiquadState::default(); 2]; 2],
            last_input: None,
        })
    }

    pub(crate) fn begin_block(&mut self, now: Instant) {
        if self
            .last_input
            .is_some_and(|last| now.duration_since(last) >= STALE_AFTER)
        {
            self.reset();
        }
    }

    /// Returns the raw peak of this frame for the capture gate as well.
    pub(crate) fn push_frame(&mut self, frame: &[f32]) -> f32 {
        let mut raw_peak = 0.0_f32;
        let mut rms = 0.0;
        let mut weighted = 0.0;
        for (channel, &sample) in frame.iter().enumerate() {
            let sample = if sample.is_finite() { sample } else { 0.0 };
            raw_peak = raw_peak.max(sample.abs());
            let sample = f64::from(sample);
            rms += sample * sample;
            if self.loudness.is_some() {
                let stages = &mut self.delay[channel];
                let output = stages[0].apply(self.weighting[0], sample);
                let output = stages[1].apply(self.weighting[1], output);
                weighted += output * output;
            }
        }
        self.peak = (self.peak * self.peak_decay).max(f64::from(raw_peak));
        self.rms.push(rms / frame.len() as f64);
        if let Some(loudness) = &mut self.loudness {
            // BS.1770 channel weights are 1 for mono, left and right.
            loudness.push(weighted);
        }
        raw_peak
    }

    pub(crate) fn end_block(&mut self, now: Instant) {
        for channel in &mut self.delay {
            for stage in channel {
                if stage.z1.abs().max(stage.z2.abs()) < 1e-30 {
                    *stage = BiquadState::default();
                }
            }
        }
        if self.peak < MIN_ENERGY.sqrt() {
            self.peak = 0.0;
        }
        self.last_input = Some(now);
    }

    pub(crate) fn readings(&self, now: Instant) -> MeterReadings {
        if !self
            .last_input
            .is_some_and(|last| now.duration_since(last) < STALE_AFTER)
        {
            return MeterReadings::default();
        }
        MeterReadings {
            peak_dbfs: energy_db(self.peak * self.peak),
            vu: energy_db(self.rms.mean() * 2.0) + 18.0,
            momentary_lufs: self
                .loudness
                .as_ref()
                .map_or(f32::NEG_INFINITY, |loudness| {
                    energy_db(loudness.mean()) - 0.691
                }),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.peak = 0.0;
        self.rms.reset();
        if let Some(loudness) = &mut self.loudness {
            loudness.reset();
        }
        self.delay = [[BiquadState::default(); 2]; 2];
        self.last_input = None;
    }
}

fn energy_db(energy: f64) -> f32 {
    if energy >= MIN_ENERGY {
        (10.0 * energy.log10()) as f32
    } else {
        f32::NEG_INFINITY
    }
}

struct EnergyWindow {
    samples: Vec<f64>,
    next: usize,
    full: bool,
    sum: f64,
    cycle_sum: f64,
}

impl EnergyWindow {
    fn new(sample_rate: u32, milliseconds: u32) -> Result<Self, CaptureError> {
        let size = ((u64::from(sample_rate) * u64::from(milliseconds) + 500) / 1_000).max(1);
        let size = usize::try_from(size).map_err(|_| CaptureError::BufferTooLarge)?;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(size)
            .map_err(|_| CaptureError::AllocationFailed)?;
        samples.resize(size, 0.0);
        Ok(Self {
            samples,
            next: 0,
            full: false,
            sum: 0.0,
            cycle_sum: 0.0,
        })
    }

    fn push(&mut self, energy: f64) {
        if self.full {
            self.sum -= self.samples[self.next];
        }
        self.sum += energy;
        self.cycle_sum += energy;
        self.samples[self.next] = energy;
        self.next += 1;
        if self.next == self.samples.len() {
            // Rebase from a complete window each time around the ring, so
            // subtraction error cannot accumulate during long sessions.
            self.sum = self.cycle_sum;
            self.cycle_sum = 0.0;
            self.next = 0;
            self.full = true;
        }
    }

    fn mean(&self) -> f64 {
        // Treat the unfilled part of the window as initial silence.
        self.sum.max(0.0) / self.samples.len() as f64
    }

    fn reset(&mut self) {
        self.next = 0;
        self.full = false;
        self.sum = 0.0;
        self.cycle_sum = 0.0;
    }
}

#[derive(Clone, Copy)]
struct BiquadCoefficients {
    b: [f64; 3],
    a: [f64; 3],
}

impl BiquadCoefficients {
    fn at_sample_rate(self, sample_rate: u32) -> Self {
        // Apply the inverse bilinear transform at 48 kHz and then the
        // bilinear transform at the host rate. Combined, this substitutes
        // (p + q z^-1) / (q + p z^-1) into the 48 kHz transfer function.
        let ratio = f64::from(sample_rate) / 48_000.0;
        let p = 1.0 - ratio;
        let q = 1.0 + ratio;
        let transform = |[c0, c1, c2]: [f64; 3]| {
            [
                c0 * q * q + c1 * p * q + c2 * p * p,
                2.0 * c0 * p * q + c1 * (p * p + q * q) + 2.0 * c2 * p * q,
                c0 * p * p + c1 * p * q + c2 * q * q,
            ]
        };
        let a = transform(self.a);
        Self {
            b: transform(self.b).map(|value| value / a[0]),
            a: a.map(|value| value / a[0]),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct BiquadState {
    z1: f64,
    z2: f64,
}

impl BiquadState {
    fn apply(&mut self, coefficients: BiquadCoefficients, input: f64) -> f64 {
        let [b0, b1, b2] = coefficients.b;
        let [_, a1, a2] = coefficients.a;
        let output = b0 * input + self.z1;
        self.z1 = b1 * input - a1 * output + self.z2;
        self.z2 = b2 * input - a2 * output;
        output
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;

    fn meter_tone(sample_rate: u32, channels: usize, dbfs: f64, frequency: f64) -> Meter {
        let mut meter = Meter::new(sample_rate, channels).unwrap();
        let amplitude = 10.0_f64.powf(dbfs / 20.0);
        let now = Instant::now();
        meter.begin_block(now);
        for frame in 0..sample_rate {
            let phase = TAU * f64::from(frame) * frequency / f64::from(sample_rate);
            let sample = (phase.sin() * amplitude) as f32;
            meter.push_frame(&[sample; 2][..channels]);
        }
        meter.end_block(Instant::now());
        meter
    }

    #[test]
    fn alignment_tone_calibrates_vu_and_momentary_matches_reference() {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let mono = meter_tone(rate, 1, -18.0, 1_000.0).readings(Instant::now());
            let stereo = meter_tone(rate, 2, -18.0, 1_000.0).readings(Instant::now());
            assert!((mono.peak_dbfs + 18.0).abs() < 0.03, "{rate}: {mono:?}");
            assert!(mono.vu.abs() < 0.001, "{rate}: {mono:?}");
            assert!(
                (mono.momentary_lufs + 21.0).abs() < 0.05,
                "{rate}: {mono:?}"
            );
            assert!(
                (stereo.momentary_lufs + 18.0).abs() < 0.05,
                "{rate}: {stereo:?}"
            );
        }
    }

    #[test]
    fn meters_release_on_silence_and_reject_non_finite_input() {
        let rate = 48_000;
        let mut meter = meter_tone(rate, 1, -18.0, 1_000.0);
        let last = meter.last_input.unwrap();
        assert_eq!(meter.readings(last + STALE_AFTER), MeterReadings::default());
        meter.begin_block(last);
        for _ in 0..rate {
            assert_eq!(meter.push_frame(&[f32::INFINITY]), 0.0);
        }
        meter.end_block(last);
        let silence = meter.readings(last);
        assert!((silence.peak_dbfs + 48.0).abs() < 0.03, "{silence:?}");
        assert_eq!(silence.vu, f32::NEG_INFINITY);
        assert_eq!(silence.momentary_lufs, f32::NEG_INFINITY);
    }

    #[test]
    fn rolling_window_expires_old_energy_at_the_exact_frame() {
        let mut window = EnergyWindow::new(1_000, 400).unwrap();
        window.push(1.0);
        for _ in 1..400 {
            window.push(0.0);
        }
        assert_eq!(window.mean(), 1.0 / 400.0);
        window.push(0.0);
        assert_eq!(window.mean(), 0.0);
    }
}

//! The reduced SADIE II D1 bank. See the root third-party notices for its provenance.

use super::{Direction, RenderError};

const SOURCE_RATE: u32 = 48_000;
const SOURCE_TAPS: usize = 256;
const AZIMUTHS: usize = 19;
const ELEVATIONS: usize = 13;
const TABLE: &[u8] = include_bytes!("sadie-d1.bin");

/// Number of stored directions; left-side directions reflect these right-side responses.
pub const HEAD_MODEL_DIRECTIONS: usize = AZIMUTHS * ELEVATIONS;
/// Bytes embedded in the executable for the reduced 48 kHz head model.
pub const HEAD_MODEL_TABLE_BYTES: usize = HEAD_MODEL_DIRECTIONS * 2 * SOURCE_TAPS * 4;

/// A head model prepared for one sample rate, shareable between renderers with `Arc`.
///
/// Construction decodes the table and, outside 48 kHz, resamples every response once.
/// Direction lookup then only interpolates prepared taps, without allocating.
pub struct HeadModel {
    sample_rate: u32,
    taps: usize,
    samples: Vec<f32>,
}

impl HeadModel {
    /// Prepares the reduced SADIE II D1 model for a rate from 8,000 through 384,000 Hz.
    pub fn new(sample_rate: u32) -> Result<Self, RenderError> {
        if !(8_000..=384_000).contains(&sample_rate) {
            return Err(RenderError::InvalidSampleRate);
        }
        let source: Vec<f32> = TABLE
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect();
        let taps = (SOURCE_TAPS as u64 * sample_rate as u64).div_ceil(SOURCE_RATE as u64) as usize;
        let samples = if sample_rate == SOURCE_RATE {
            source
        } else {
            let plan = resampling_plan(sample_rate, taps);
            let mut result = vec![0.0; HEAD_MODEL_DIRECTIONS * 2 * taps];
            for (input, output) in source
                .chunks_exact(SOURCE_TAPS)
                .zip(result.chunks_exact_mut(taps))
            {
                for (sample, weights) in output.iter_mut().zip(&plan) {
                    *sample = weights
                        .iter()
                        .map(|&(index, weight)| input[index] as f64 * weight)
                        .sum::<f64>() as f32;
                }
            }
            result
        };
        Ok(Self {
            sample_rate,
            taps,
            samples,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Length of each prepared ear response, including its leading acoustic delay.
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// Owned sample storage and this value's fields, excluding the embedded source table.
    /// Allocator bookkeeping is not included.
    pub fn memory_bytes(&self) -> usize {
        self.samples.capacity() * std::mem::size_of::<f32>() + std::mem::size_of::<Self>()
    }

    /// Writes a response to two buffers of exactly `taps()` values. `None` is unfiltered.
    pub(crate) fn filter(&self, direction: Option<Direction>, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), self.taps);
        assert_eq!(right.len(), self.taps);
        let Some(direction) =
            direction.filter(|value| value.azimuth.is_finite() && value.elevation.is_finite())
        else {
            left.fill(0.0);
            right.fill(0.0);
            left[0] = std::f32::consts::FRAC_1_SQRT_2;
            right[0] = std::f32::consts::FRAC_1_SQRT_2;
            return;
        };
        // Preserve the exact magnitude of opposite angles within the usual signed range.
        let mut azimuth = direction.azimuth % 360.0;
        if azimuth > 180.0 {
            azimuth -= 360.0;
        } else if azimuth < -180.0 {
            azimuth += 360.0;
        }
        let az = azimuth.abs() / 10.0;
        let el = (direction.elevation.clamp(-90.0, 90.0) + 90.0) / 15.0;
        let a0 = (az as usize).min(AZIMUTHS - 2);
        let e0 = (el as usize).min(ELEVATIONS - 2);
        let af = az - a0 as f32;
        let ef = el - e0 as f32;
        let rows = [
            e0 * AZIMUTHS + a0,
            e0 * AZIMUTHS + a0 + 1,
            (e0 + 1) * AZIMUTHS + a0,
            (e0 + 1) * AZIMUTHS + a0 + 1,
        ];
        let weights = [
            (1.0 - af) * (1.0 - ef),
            af * (1.0 - ef),
            (1.0 - af) * ef,
            af * ef,
        ];
        let (left, right) = if azimuth < 0.0 {
            (right, left)
        } else {
            (left, right)
        };
        for (ear, output) in [left, right].into_iter().enumerate() {
            let offsets = rows.map(|row| (row * 2 + ear) * self.taps);
            for (tap, output) in output.iter_mut().enumerate() {
                *output = self.samples[offsets[0] + tap] * weights[0]
                    + self.samples[offsets[1] + tap] * weights[1]
                    + self.samples[offsets[2] + tap] * weights[2]
                    + self.samples[offsets[3] + tap] * weights[3];
            }
        }
    }
}

/// A Blackman-windowed sinc lowpass, built once per model, in source-sample coordinates.
/// Each source tap's output weights sum to one so resampling preserves the FIR's DC gain.
fn resampling_plan(sample_rate: u32, taps: usize) -> Vec<Vec<(usize, f64)>> {
    let ratio = sample_rate as f64 / SOURCE_RATE as f64;
    let cutoff = ratio.min(1.0);
    let radius = 16.0 / cutoff;
    let mut plan = Vec::with_capacity(taps);
    let mut sums = [0.0; SOURCE_TAPS];
    for output in 0..taps {
        let position = output as f64 / ratio;
        let start = (position - radius).ceil().max(0.0) as usize;
        let end = ((position + radius).floor() as usize + 1).min(SOURCE_TAPS);
        let mut weights = Vec::with_capacity(end.saturating_sub(start));
        for (index, sum) in sums.iter_mut().enumerate().take(end).skip(start) {
            let delta = position - index as f64;
            let phase = std::f64::consts::PI * cutoff * delta;
            let sinc = if phase.abs() < 1e-12 {
                1.0
            } else {
                phase.sin() / phase
            };
            let window_phase = std::f64::consts::PI * delta / radius;
            let window = 0.42 + 0.5 * window_phase.cos() + 0.08 * (2.0 * window_phase).cos();
            let weight = cutoff / ratio * sinc * window;
            *sum += weight;
            weights.push((index, weight));
        }
        plan.push(weights);
    }
    for weights in &mut plan {
        for (index, weight) in weights {
            *weight /= sums[*index];
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(model: &HeadModel, azimuth: f32, elevation: f32) -> (Vec<f32>, Vec<f32>) {
        let mut left = vec![0.0; model.taps()];
        let mut right = left.clone();
        model.filter(
            Some(Direction { azimuth, elevation }),
            &mut left,
            &mut right,
        );
        (left, right)
    }

    #[test]
    fn embedded_table_has_expected_size_and_finite_taps() {
        assert_eq!(TABLE.len(), HEAD_MODEL_TABLE_BYTES);
        assert!(TABLE.len() < 1_048_576);
        let model = HeadModel::new(48_000).unwrap();
        assert_eq!(model.taps(), 256);
        assert!(model.samples.iter().all(|tap| tap.is_finite()));
        assert_eq!(
            model.memory_bytes(),
            HEAD_MODEL_TABLE_BYTES + std::mem::size_of::<HeadModel>()
        );
        eprintln!(
            "head model: embedded={} bytes, rate=48000, owned={} bytes, taps={}",
            TABLE.len(),
            model.memory_bytes(),
            model.taps()
        );
    }

    #[test]
    fn opposite_azimuths_mirror_exactly_and_median_planes_have_equal_ears() {
        let model = HeadModel::new(48_000).unwrap();
        for elevation in [-90.0, -37.5, 0.0, 7.5, 60.0, 90.0] {
            for azimuth in [0.0, 5.5, 30.0, 33.2, 90.0, 137.5, 180.0] {
                let (left, right) = response(&model, azimuth, elevation);
                let (mirror_left, mirror_right) = response(&model, -azimuth, elevation);
                assert_eq!(left, mirror_right);
                assert_eq!(right, mirror_left);
                if azimuth == 0.0 || azimuth == 180.0 || elevation.abs() == 90.0 {
                    assert_eq!(left, right);
                }
            }
        }
    }

    #[test]
    fn absent_direction_is_an_equal_unfiltered_impulse() {
        let model = HeadModel::new(8_000).unwrap();
        let mut left = vec![9.0; model.taps()];
        let mut right = left.clone();
        model.filter(None, &mut left, &mut right);
        assert_eq!(left, right);
        assert_eq!(left[0], std::f32::consts::FRAC_1_SQRT_2);
        assert!(left[1..].iter().all(|&value| value == 0.0));
    }

    #[test]
    fn resampling_preserves_dc_gain_and_mirror_symmetry_at_supported_extremes() {
        let original = HeadModel::new(48_000).unwrap();
        let (original_left, _) = response(&original, -30.0, 0.0);
        let original_dc = original_left.iter().map(|&tap| tap as f64).sum::<f64>();
        for rate in [8_000, 44_100, 96_000, 384_000] {
            let model = HeadModel::new(rate).unwrap();
            eprintln!(
                "head model: rate={rate}, owned={} bytes, taps={}",
                model.memory_bytes(),
                model.taps()
            );
            assert_eq!(model.sample_rate(), rate);
            assert_eq!(
                model.taps(),
                (256_u64 * rate as u64).div_ceil(48_000) as usize
            );
            assert!(model.samples.iter().all(|tap| tap.is_finite()));
            let (left, right) = response(&model, -30.0, 0.0);
            let (mirror_left, mirror_right) = response(&model, 30.0, 0.0);
            assert_eq!(left, mirror_right);
            assert_eq!(right, mirror_left);
            let dc = left.iter().map(|&tap| tap as f64).sum::<f64>();
            assert!(
                (dc - original_dc).abs() < 1e-6,
                "rate {rate}: {dc} != {original_dc}"
            );
        }
        assert!(HeadModel::new(7_999).is_err());
        assert!(HeadModel::new(384_001).is_err());
    }

    #[test]
    fn downsampling_rejects_energy_above_the_new_nyquist_limit() {
        let plan = resampling_plan(8_000, 43);
        let gain = |frequency: f64| {
            (8..35)
                .map(|output| {
                    let (real, imaginary) = plan[output].iter().fold(
                        (0.0, 0.0),
                        |(real, imaginary), &(index, weight)| {
                            let phase =
                                2.0 * std::f64::consts::PI * frequency * index as f64 / 48_000.0;
                            (
                                real + weight * phase.cos(),
                                imaginary + weight * phase.sin(),
                            )
                        },
                    );
                    (real * real + imaginary * imaginary).sqrt()
                })
                .sum::<f64>()
                / 27.0
        };
        assert!(gain(10_000.0) < gain(1_000.0) * 0.001);
    }
}

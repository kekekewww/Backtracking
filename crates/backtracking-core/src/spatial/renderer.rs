use std::sync::Arc;

use super::{Direction, HeadModel, RenderError};

const FILTER_BLOCK: usize = 128;

/// A source update on the input's frame timeline. Coordinates are listener-relative:
/// x is right, y is forward, z is up. Distance and size are not rendered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PositionKey {
    pub frame: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub size: f32,
    /// Linear source gain, applied to the input before filtering. Must be nonnegative.
    pub gain: f32,
}

/// One source per input channel. Labels can be converted with [`super::speaker_directions`].
#[derive(Clone, Debug)]
pub enum BinauralSource {
    /// `None` is the equal-ear, unfiltered path used for LFE and unknown labels.
    Fixed(Option<Direction>),
    /// Keys must have strictly increasing frames and finite values, size in 0..=1,
    /// and nonnegative gain. Include the last key before a selection and the next
    /// key after it to preserve interpolation at both edges. An empty track, or
    /// frames before the first key, use the equal-ear path with gain 1.
    Moving(Vec<PositionKey>),
}

impl BinauralSource {
    fn validate(&self) -> Result<(), RenderError> {
        match self {
            Self::Fixed(Some(direction))
                if !direction.azimuth.is_finite()
                    || !direction.elevation.is_finite()
                    || direction.elevation.abs() > 90.0 =>
            {
                Err(RenderError::InvalidSource)
            }
            Self::Moving(keys) => {
                if keys.iter().any(|key| {
                    ![key.x, key.y, key.z, key.size, key.gain]
                        .iter()
                        .all(|value| value.is_finite())
                        || !(0.0..=1.0).contains(&key.size)
                        || key.gain < 0.0
                }) || keys.windows(2).any(|pair| pair[0].frame >= pair[1].frame)
                {
                    Err(RenderError::InvalidPositionKeys)
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }

    fn pose(&self, frame: u64) -> (Option<Direction>, f32) {
        match self {
            Self::Fixed(direction) => (*direction, 1.0),
            Self::Moving(keys) => {
                let next = keys.partition_point(|key| key.frame <= frame);
                if next == 0 {
                    return (None, 1.0);
                }
                let a = keys[next - 1];
                let (x, y, z, gain) = if let Some(b) = keys.get(next) {
                    let t = (frame - a.frame) as f64 / (b.frame - a.frame) as f64;
                    let lerp = |a: f32, b: f32| f64::from(a) * (1.0 - t) + f64::from(b) * t;
                    (
                        lerp(a.x, b.x),
                        lerp(a.y, b.y),
                        lerp(a.z, b.z),
                        lerp(a.gain, b.gain),
                    )
                } else {
                    (
                        f64::from(a.x),
                        f64::from(a.y),
                        f64::from(a.z),
                        f64::from(a.gain),
                    )
                };
                let horizontal = x.hypot(y);
                let direction = if horizontal == 0.0 && z == 0.0 {
                    None
                } else {
                    Some(Direction {
                        azimuth: x.atan2(y).to_degrees() as f32,
                        elevation: z.atan2(horizontal).to_degrees() as f32,
                    })
                };
                (direction, gain as f32)
            }
        }
    }

    fn gain(&self, frame: u64, next_key: &mut usize) -> f32 {
        let Self::Moving(keys) = self else { return 1.0 };
        while keys.get(*next_key).is_some_and(|key| key.frame <= frame) {
            *next_key += 1;
        }
        let next = *next_key;
        if next == 0 {
            return 1.0;
        }
        let a = keys[next - 1];
        match keys.get(next) {
            Some(b) if a.gain != b.gain => {
                let t = (frame - a.frame) as f64 / (b.frame - a.frame) as f64;
                (f64::from(a.gain) * (1.0 - t) + f64::from(b.gain) * t) as f32
            }
            _ => a.gain,
        }
    }
}

struct SourceState {
    source: BinauralSource,
    selected: bool,
    history: Vec<f32>,
    cursor: usize,
    next_key: usize,
    left: Vec<f32>,
    right: Vec<f32>,
    next_left: Vec<f32>,
    next_right: Vec<f32>,
    changing: bool,
}

impl SourceState {
    fn new(source: BinauralSource, selected: bool, model: &HeadModel, frame: u64) -> Self {
        let taps = model.taps();
        let mut state = Self {
            source,
            selected,
            history: vec![0.0; taps * 2],
            cursor: 0,
            next_key: 0,
            left: vec![0.0; taps],
            right: vec![0.0; taps],
            next_left: vec![0.0; taps],
            next_right: vec![0.0; taps],
            changing: false,
        };
        state.reset(model, frame);
        state
    }

    fn reset(&mut self, model: &HeadModel, frame: u64) {
        self.history.fill(0.0);
        self.cursor = 0;
        self.next_key = match &self.source {
            BinauralSource::Moving(keys) => keys.partition_point(|key| key.frame <= frame),
            BinauralSource::Fixed(_) => 0,
        };
        model.filter(self.source.pose(frame).0, &mut self.left, &mut self.right);
        self.next_left.copy_from_slice(&self.left);
        self.next_right.copy_from_slice(&self.right);
        self.changing = false;
    }

    fn update(&mut self, model: &HeadModel, frame: u64) {
        if matches!(self.source, BinauralSource::Moving(_)) {
            std::mem::swap(&mut self.left, &mut self.next_left);
            std::mem::swap(&mut self.right, &mut self.next_right);
            model.filter(
                self.source.pose(frame).0,
                &mut self.next_left,
                &mut self.next_right,
            );
            self.changing = self.left != self.next_left || self.right != self.next_right;
        }
    }

    fn sample(&mut self, sample: f32, phase: usize) -> [f32; 2] {
        let taps = self.left.len();
        self.history[self.cursor] = sample;
        self.history[self.cursor + taps] = sample;
        let history = &self.history[self.cursor..self.cursor + taps];
        let mut value = convolve(history, &self.left, &self.right);
        if self.changing {
            let next = convolve(history, &self.next_left, &self.next_right);
            let blend = phase as f32 / FILTER_BLOCK as f32;
            for ear in 0..2 {
                value[ear] += (next[ear] - value[ear]) * blend;
            }
        }
        self.cursor = if self.cursor == 0 {
            taps - 1
        } else {
            self.cursor - 1
        };
        [value[0] * 0.5, value[1] * 0.5]
    }
}

/// A stateful, single-threaded renderer. Setup clones the source tracks and allocates
/// all filter and history buffers. Sharing the head model avoids repeated resampling.
/// A renderer has zero additional buffering latency beyond the HRIR itself.
pub struct BinauralRenderer {
    model: Arc<HeadModel>,
    sources: Vec<SourceState>,
    position: u64,
    phase: usize,
}

impl BinauralRenderer {
    /// Select `None` for every channel, or a list of unique native channel indices.
    /// `Some(&[])` renders silence. Input still contains all channels in native order.
    /// History starts silent at `first_frame`; use `reset` to seek to another clip.
    pub fn new(
        model: Arc<HeadModel>,
        sources: &[BinauralSource],
        selected: Option<&[usize]>,
        first_frame: u64,
    ) -> Result<Self, RenderError> {
        if sources.is_empty() {
            return Err(RenderError::InvalidSource);
        }
        for source in sources {
            source.validate()?;
        }
        if let Some(indices) = selected {
            if indices
                .iter()
                .enumerate()
                .any(|(i, &index)| index >= sources.len() || indices[..i].contains(&index))
            {
                return Err(RenderError::InvalidSelection);
            }
        }
        let states = sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                SourceState::new(
                    source.clone(),
                    selected.is_none_or(|indices| indices.contains(&index)),
                    &model,
                    first_frame,
                )
            })
            .collect();
        Ok(Self {
            model,
            sources: states,
            position: first_frame,
            phase: 0,
        })
    }

    /// Next input frame on the caller's timeline.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Discards filter history and restarts the internal filter cadence without allocating.
    pub fn reset(&mut self, first_frame: u64) {
        self.position = first_frame;
        self.phase = 0;
        for source in &mut self.sources {
            source.reset(&self.model, first_frame);
        }
    }

    /// Overwrites stereo `output`. Its length must be twice the input frame count.
    /// No allocation, deallocation, locking, I/O, or threads are used here. Finite
    /// PCM is expected; values outside full scale are preserved. Filters update on
    /// internal 128-frame boundaries and crossfade over the following 128 frames.
    /// Positions are linearly interpolated in Cartesian space at those boundaries.
    pub fn render(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), RenderError> {
        let channels = self.sources.len();
        if input.len() % channels != 0
            || (input.len() / channels).checked_mul(2) != Some(output.len())
        {
            return Err(RenderError::InvalidBuffer);
        }
        let frames = input.len() / channels;
        let end = self
            .position
            .checked_add(frames as u64)
            .ok_or(RenderError::TimelineOverflow)?;
        output.fill(0.0);
        let mut offset = 0;
        while offset < frames {
            let count = (FILTER_BLOCK - self.phase).min(frames - offset);
            for (channel, source) in self.sources.iter_mut().enumerate() {
                if !source.selected {
                    continue;
                }
                if self.phase == 0 {
                    source.update(&self.model, self.position);
                }
                for index in 0..count {
                    let sample = input[(offset + index) * channels + channel]
                        * source
                            .source
                            .gain(self.position + index as u64, &mut source.next_key);
                    let value = source.sample(sample, self.phase + index);
                    output[(offset + index) * 2] += value[0];
                    output[(offset + index) * 2 + 1] += value[1];
                }
            }
            offset += count;
            self.position += count as u64;
            self.phase = (self.phase + count) % FILTER_BLOCK;
        }
        debug_assert_eq!(self.position, end);
        Ok(())
    }
}

/// Renders a whole clip with the same arithmetic, cadence and tail cutting as preview.
/// Allocates the output and renderer; callers may render independent clips on workers.
pub fn render_binaural(
    model: Arc<HeadModel>,
    sources: &[BinauralSource],
    selected: Option<&[usize]>,
    first_frame: u64,
    input: &[f32],
) -> Result<Vec<f32>, RenderError> {
    let mut renderer = BinauralRenderer::new(model, sources, selected, first_frame)?;
    if input.len() % sources.len() != 0 {
        return Err(RenderError::InvalidBuffer);
    }
    let length = (input.len() / sources.len())
        .checked_mul(2)
        .ok_or(RenderError::InvalidBuffer)?;
    let mut output = vec![0.0; length];
    renderer.render(input, &mut output)?;
    Ok(output)
}

// Use wider multiply-accumulates when supported. Keep the SSE2 and scalar
// paths for hosts without FMA and for other architectures.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx,fma")]
unsafe fn convolve_fma(history: &[f32], left: &[f32], right: &[f32]) -> [f32; 2] {
    use std::arch::x86_64::*;
    let taps = history.len().min(left.len()).min(right.len());
    let mut l = [_mm256_setzero_ps(); 2];
    let mut r = [_mm256_setzero_ps(); 2];
    let mut index = 0;
    while index + 16 <= taps {
        for lane in 0..2 {
            let i = index + lane * 8;
            let h = _mm256_loadu_ps(history.as_ptr().add(i));
            l[lane] = _mm256_fmadd_ps(h, _mm256_loadu_ps(left.as_ptr().add(i)), l[lane]);
            r[lane] = _mm256_fmadd_ps(h, _mm256_loadu_ps(right.as_ptr().add(i)), r[lane]);
        }
        index += 16;
    }
    let sum = |a, b| {
        let x = _mm256_add_ps(a, b);
        let x = _mm_add_ps(_mm256_castps256_ps128(x), _mm256_extractf128_ps::<1>(x));
        let x = _mm_add_ps(x, _mm_movehl_ps(x, x));
        _mm_cvtss_f32(_mm_add_ss(x, _mm_shuffle_ps::<0x55>(x, x)))
    };
    let mut result = [sum(l[0], l[1]), sum(r[0], r[1])];
    for i in index..taps {
        result[0] += history[i] * left[i];
        result[1] += history[i] * right[i];
    }
    result
}

#[cfg(target_arch = "x86_64")]
fn convolve(history: &[f32], left: &[f32], right: &[f32]) -> [f32; 2] {
    if is_x86_feature_detected!("fma") && is_x86_feature_detected!("avx") {
        // SAFETY: both required CPU features are present.
        return unsafe { convolve_fma(history, left, right) };
    }
    convolve_sse2(history, left, right)
}

#[cfg(target_arch = "x86_64")]
fn convolve_sse2(history: &[f32], left: &[f32], right: &[f32]) -> [f32; 2] {
    use std::arch::x86_64::*;
    let taps = history.len().min(left.len()).min(right.len());
    // SAFETY: SSE2 is always available on x86-64. Each load lies within the slices;
    // unaligned loads are used, and scalar code handles any remaining taps.
    unsafe {
        let mut l = [_mm_setzero_ps(); 4];
        let mut r = [_mm_setzero_ps(); 4];
        let mut index = 0;
        while index + 16 <= taps {
            for lane in 0..4 {
                let i = index + lane * 4;
                let h = _mm_loadu_ps(history.as_ptr().add(i));
                l[lane] = _mm_add_ps(l[lane], _mm_mul_ps(h, _mm_loadu_ps(left.as_ptr().add(i))));
                r[lane] = _mm_add_ps(r[lane], _mm_mul_ps(h, _mm_loadu_ps(right.as_ptr().add(i))));
            }
            index += 16;
        }
        let mut ls = [0.0; 4];
        let mut rs = [0.0; 4];
        _mm_storeu_ps(
            ls.as_mut_ptr(),
            _mm_add_ps(_mm_add_ps(l[0], l[1]), _mm_add_ps(l[2], l[3])),
        );
        _mm_storeu_ps(
            rs.as_mut_ptr(),
            _mm_add_ps(_mm_add_ps(r[0], r[1]), _mm_add_ps(r[2], r[3])),
        );
        let mut value = [ls.iter().sum(), rs.iter().sum()];
        for i in index..taps {
            value[0] += history[i] * left[i];
            value[1] += history[i] * right[i];
        }
        value
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn convolve(history: &[f32], left: &[f32], right: &[f32]) -> [f32; 2] {
    let mut value = [0.0; 2];
    for ((&sample, &left), &right) in history.iter().zip(left).zip(right) {
        value[0] += sample * left;
        value[1] += sample * right;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convolution_uses_the_common_prefix_of_unequal_slices() {
        let history = [1.0; 37];
        let left = [2.0; 37];
        let right = [-3.0; 37];
        for n in [0, 1, 15, 16, 17, 32, 35] {
            for (h, l, r) in [(n, 36, 37), (37, n, 36), (36, 37, n)] {
                let (h, l, r) = (&history[..h], &left[..l], &right[..r]);
                let expected = [2.0 * n as f32, -3.0 * n as f32];
                assert_eq!(convolve(h, l, r), expected);
                #[cfg(target_arch = "x86_64")]
                {
                    assert_eq!(convolve_sse2(h, l, r), expected);
                    if is_x86_feature_detected!("avx") && is_x86_feature_detected!("fma") {
                        // SAFETY: this host supports both target features.
                        assert_eq!(unsafe { convolve_fma(h, l, r) }, expected);
                    }
                }
            }
        }
    }
}

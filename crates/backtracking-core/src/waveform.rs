/// Aggregate signed peaks across channels for a small waveform display.
pub fn peaks(interleaved: &[f32], channels: usize, minimum: &mut [f32], maximum: &mut [f32]) {
    minimum.fill(0.0);
    maximum.fill(0.0);
    let bins = minimum.len().min(maximum.len());
    if channels == 0 || bins == 0 {
        return;
    }
    let frames = interleaved.len() / channels;
    if frames == 0 {
        return;
    }

    for bin in 0..bins {
        let first = bin * frames / bins;
        let end = ((bin + 1) * frames / bins).max(first + 1);
        let samples = &interleaved[first * channels..end * channels];
        minimum[bin] = samples.iter().copied().fold(f32::INFINITY, f32::min);
        maximum[bin] = samples.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        if !minimum[bin].is_finite() {
            minimum[bin] = 0.0;
        }
        if !maximum[bin].is_finite() {
            maximum[bin] = 0.0;
        }
    }
}

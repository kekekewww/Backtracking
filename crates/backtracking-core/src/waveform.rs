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

pub const HINT_BOLD_FAMILY: &str = "BacktrackingHintBold";
pub const HINT_BOLD_FONT: &[u8] = include_bytes!("../assets/OpenSans-Bold-Latin.ttf");

/// Place a cursor hint in right, left, below, above order within the client area.
/// Coordinates increase downwards. The caller clips hints larger than the area.
pub fn hint_position(cursor: [f32; 2], size: [f32; 2], bounds: [f32; 4]) -> [f32; 2] {
    const GAP: f32 = 12.0;
    let [x, y] = cursor;
    let [left, top, right, bottom] = bounds;
    let width = size[0].min((right - left).max(0.0));
    let height = size[1].min((bottom - top).max(0.0));
    let max_x = (right - width).max(left);
    let max_y = (bottom - height).max(top);
    let center_x = (x - width / 2.0).clamp(left, max_x);
    let center_y = (y - height / 2.0).clamp(top, max_y);
    let options = [
        [x + GAP, center_y],
        [x - GAP - width, center_y],
        [center_x, y + GAP],
        [center_x, y - GAP - height],
    ];

    options
        .into_iter()
        .find(|&[x, y]| x >= left && x <= max_x && y >= top && y <= max_y)
        .unwrap_or([(x + GAP).clamp(left, max_x), center_y])
}

#[cfg(test)]
mod tests {
    use super::hint_position;

    #[test]
    fn cursor_hint_uses_direction_priority() {
        let wide = [0.0, 0.0, 300.0, 120.0];
        let narrow = [0.0, 0.0, 120.0, 120.0];
        let size = [80.0, 30.0];
        assert_eq!(hint_position([150.0, 60.0], size, wide), [162.0, 45.0]);
        assert_eq!(hint_position([280.0, 60.0], size, wide), [188.0, 45.0]);
        assert_eq!(hint_position([60.0, 25.0], size, narrow), [20.0, 37.0]);
        assert_eq!(hint_position([60.0, 95.0], size, narrow), [20.0, 53.0]);
    }

    #[test]
    fn cursor_hint_handles_corners_and_an_oversized_hint() {
        let bounds = [10.0, 20.0, 300.0, 120.0];
        assert_eq!(
            hint_position([292.0, 21.0], [80.0, 30.0], bounds),
            [200.0, 20.0]
        );
        assert_eq!(
            hint_position([150.0, 65.0], [480.0, 160.0], bounds),
            [10.0, 20.0]
        );
    }
}

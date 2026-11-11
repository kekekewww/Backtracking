use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum WaveformView {
    #[default]
    Merged = 0,
    Left = 1,
    Right = 2,
}

impl WaveformView {
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Merged),
            1 => Some(Self::Left),
            2 => Some(Self::Right),
            _ => None,
        }
    }

    pub(crate) fn sample(self, frame: &[f32]) -> f32 {
        match self {
            Self::Merged => match frame {
                [mono] => *mono,
                [left, right] => left * 0.5 + right * 0.5,
                _ => {
                    (frame.iter().map(|&sample| f64::from(sample)).sum::<f64>()
                        / frame.len() as f64) as f32
                }
            },
            Self::Left => frame[0],
            Self::Right => frame.get(1).copied().unwrap_or(frame[0]),
        }
    }
}

pub(crate) fn extrema(samples: impl Iterator<Item = f32>) -> (f32, f32) {
    let mut samples = samples.filter(|sample| sample.is_finite());
    let Some(first) = samples.next() else {
        return (0., 0.);
    };
    samples.fold((first, first), |(low, high), sample| {
        (low.min(sample), high.max(sample))
    })
}

/// Computes signed peaks from one selected channel or a mono fold-down.
pub fn view_peaks(
    interleaved: &[f32],
    channels: usize,
    view: WaveformView,
    minimum: &mut [f32],
    maximum: &mut [f32],
) {
    minimum.fill(0.);
    maximum.fill(0.);
    let bins = minimum.len().min(maximum.len());
    let frames = interleaved.len().checked_div(channels).unwrap_or(0);
    if frames == 0 {
        return;
    }
    for bin in 0..bins {
        let first = bin * frames / bins;
        let end = ((bin + 1) * frames / bins).max(first + 1);
        (minimum[bin], maximum[bin]) = extrema(
            interleaved[first * channels..end * channels]
                .chunks_exact(channels)
                .map(|frame| view.sample(frame)),
        );
    }
}

/// The right boundary of a clip may coincide with the right edge of the visible window.
pub fn flag_fraction(position: u64, visible: Range<u64>) -> Option<f64> {
    if visible.is_empty() || position < visible.start || position > visible.end {
        return None;
    }
    Some((position - visible.start) as f64 / (visible.end - visible.start) as f64)
}

/// Snaps to the nearest visible boundary within a caller-provided screen distance.
pub fn snap_to_flags(
    fraction: f64,
    visible: Range<u64>,
    flags: Option<&Range<u64>>,
    tolerance_fraction: f64,
) -> f64 {
    let Some(flags) = flags else { return fraction };
    [flags.start, flags.end]
        .into_iter()
        .filter_map(|position| flag_fraction(position, visible.clone()))
        .map(|flag| (flag, (flag - fraction).abs()))
        .filter(|&(_, distance)| distance <= tolerance_fraction)
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map_or(fraction, |(flag, _)| flag)
}

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
    use super::{flag_fraction, hint_position, snap_to_flags, view_peaks, WaveformView};

    #[test]
    fn fold_down_and_separate_views_read_the_requested_samples() {
        let audio = [0.75, -0.75, -0.5, 0.25];
        let mut low = [0.; 2];
        let mut high = [0.; 2];
        for (view, expected) in [
            (WaveformView::Left, [0.75, -0.5]),
            (WaveformView::Right, [-0.75, 0.25]),
            (WaveformView::Merged, [0., -0.125]),
        ] {
            view_peaks(&audio, 2, view, &mut low, &mut high);
            assert_eq!(low, expected);
            assert_eq!(high, expected);
        }
    }

    #[test]
    fn flags_move_with_audio_and_only_visible_nearby_edges_snap() {
        let flags = 100..150;
        assert_eq!(flag_fraction(flags.start, 50..150), Some(0.5));
        assert_eq!(flag_fraction(flags.end, 50..150), Some(1.0));
        assert_eq!(flag_fraction(flags.start, 120..220), None);
        assert_eq!(flag_fraction(flags.end, 120..220), Some(0.3));
        assert_eq!(snap_to_flags(0.28, 120..220, Some(&flags), 0.05), 0.3);
        assert_eq!(snap_to_flags(0.22, 120..220, Some(&flags), 0.05), 0.22);
        assert_eq!(snap_to_flags(0.0, 120..220, Some(&flags), 0.05), 0.0);
    }

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

use std::ops::Range;

mod geometry;
pub use geometry::{EnvelopeColumn, SamplePoint, WaveformGeometry, minimum_zoom_frames, waveform_geometry};

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

/// Hard allocation limit even on unusually large or malformed viewports.
pub const MAX_WAVEFORM_BINS: usize = 4096;
const MAX_TIMELINE_GUIDES: usize = 2048;
const MAX_PLOT_WIDTH: f32 = 16_384.0;

/// Geometry and detail settings shared by waveform renderers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayPolicy {
    pub bins: usize,
    pub waveform_stroke: f32,
    pub bar_stroke: f32,
    pub seconds_stroke: f32,
    pub bar_spacing_px: f32,
    pub seconds_spacing_px: f32,
    pub plot_width: f32,
}

impl DisplayPolicy {
    /// Logical-pixel geometry determines detail; zoomed and edited views get more
    /// bins, while long, idle views aggregate peaks into wider screen columns.
    pub fn for_view(
        width: f32,
        height: f32,
        visible_frames: u64,
        sample_rate: u32,
        interactive: bool,
    ) -> Self {
        let valid_geometry = width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0;
        let width = if valid_geometry { width.min(MAX_PLOT_WIDTH) } else { 0.0 };
        let height = if valid_geometry { height.min(8192.0) } else { 0.0 };
        let scale = if valid_geometry {
            ((width / 610.0).powf(0.2) * (height / 80.0).sqrt()).clamp(0.75, 2.4)
        } else {
            1.0
        };
        let bins = if valid_geometry && visible_frames != 0 && sample_rate != 0 {
            if visible_frames <= MAX_WAVEFORM_BINS as u64
                && visible_frames as f64 <= f64::from(width) {
                // Once each sample fits on screen, keep its exact frame instead
                // of aggregating it away through overview quantization.
                visible_frames as usize
            } else {
                let seconds_per_pixel = visible_frames as f64 / f64::from(sample_rate) / f64::from(width);
                let overview_spacing = 1.25 + (seconds_per_pixel / 0.01).log2().max(0.0) * 0.55;
                let column_spacing = if interactive { overview_spacing / 2.5 } else { overview_spacing };
                let desired = (f64::from(width) / column_spacing.max(0.5)).ceil() as usize;
                // Quantization avoids reallocating/rebuilding peaks for each pixel of a resize.
                let quantized = desired.div_ceil(32).saturating_mul(32);
                let pixel_limit = (width * if interactive { 2.0 } else { 1.0 }).ceil() as usize;
                quantized.clamp(1, MAX_WAVEFORM_BINS)
                    .min(pixel_limit.max(1))
                    .min(usize::try_from(visible_frames).unwrap_or(usize::MAX))
            }
        } else {
            0
        };
        Self {
            bins,
            waveform_stroke: (1.15 * scale).clamp(0.95, 3.0),
            bar_stroke: scale.clamp(0.95, 2.4),
            seconds_stroke: (0.8 * scale).clamp(0.75, 1.8),
            bar_spacing_px: (16.0 * scale).clamp(12.0, 28.0),
            seconds_spacing_px: (48.0 * scale).clamp(40.0, 80.0),
            plot_width: width,
        }
    }
}

/// Exact frame positions painted by the caller and offered to snapping.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TimelineGuides {
    pub bars: Vec<u64>,
    pub seconds: Vec<u64>,
}

/// Select the exact guides to both draw and snap to. `bars` must be sorted by
/// retained frame position. Seconds stay anchored at retained frame zero and
/// use multiples of the selected interval when a long view needs fewer lines.
pub fn timeline_guides(
    visible: Range<u64>,
    sample_rate: u32,
    seconds_interval: u32,
    bars: &[u64],
    show_bars: bool,
    show_seconds: bool,
    background_visible: bool,
    policy: &DisplayPolicy,
) -> TimelineGuides {
    if !background_visible || visible.is_empty() || policy.bins == 0
        || !policy.plot_width.is_finite() || policy.plot_width <= 0.0 {
        return TimelineGuides::default();
    }
    let width = f64::from(policy.plot_width.min(MAX_PLOT_WIDTH));
    let span = (visible.end - visible.start) as f64;
    let mut guides = TimelineGuides::default();
    if show_bars {
        let spacing = f64::from(policy.bar_spacing_px.max(1.0));
        let first = bars.partition_point(|frame| *frame < visible.start);
        let mut previous = None;
        for &frame in bars[first..].iter().take_while(|&&frame| frame <= visible.end) {
            if previous.is_none_or(|previous| (frame - previous) as f64 * width / span >= spacing) {
                guides.bars.push(frame);
                previous = Some(frame);
                if guides.bars.len() == MAX_TIMELINE_GUIDES { break; }
            }
        }
    }
    let base_step = u64::from(sample_rate) * u64::from(seconds_interval);
    if show_seconds && base_step != 0 {
        let max_lines = (width / f64::from(policy.seconds_spacing_px.max(1.0)))
            .floor().clamp(1.0, (MAX_TIMELINE_GUIDES - 1) as f64);
        let minimum_step = span / max_lines;
        let multiplier = nice_guide_multiple((minimum_step / base_step as f64).ceil().max(1.0) as u64);
        if let Some(step) = base_step.checked_mul(multiplier) {
            if let Some(mut frame) = visible.start.div_ceil(step).checked_mul(step) {
                while frame <= visible.end && guides.seconds.len() < MAX_TIMELINE_GUIDES {
                    guides.seconds.push(frame);
                    let Some(next) = frame.checked_add(step) else { break };
                    frame = next;
                }
            }
        } else if visible.start == 0 {
            // No second aligned boundary fits in u64; frame zero is still valid.
            guides.seconds.push(0);
        }
    }
    guides
}

fn nice_guide_multiple(minimum: u64) -> u64 {
    let mut power = 1_u64;
    loop {
        for factor in [1_u64, 2, 5] {
            let candidate = power.saturating_mul(factor);
            if candidate >= minimum { return candidate; }
        }
        let Some(next) = power.checked_mul(10) else { return u64::MAX };
        power = next;
    }
}

/// Snapping follows category priority, not global proximity: any in-range flag
/// wins over a bar, and any in-range bar wins over a seconds line. Within one
/// category the nearest boundary wins, with the earlier frame breaking ties.
/// The caller supplies only flag boundaries it paints and the same `guides`
/// used to paint the timeline. Background transparency never hides flags here.
pub fn snap_to_visible_markers(
    fraction: f64,
    visible: Range<u64>,
    flags: &[u64],
    guides: &TimelineGuides,
    tolerance_fraction: f64,
) -> f64 {
    if visible.is_empty() || !fraction.is_finite() || !tolerance_fraction.is_finite() || tolerance_fraction < 0.0 {
        return fraction;
    }
    let span = (visible.end - visible.start) as f64;
    let pointer_offset = fraction * span;
    if !pointer_offset.is_finite() { return fraction; }
    let tolerance_frames = tolerance_fraction * span;
    for category in [flags, &guides.bars, &guides.seconds] {
        let snapped = category.iter()
            .filter_map(|&frame| flag_fraction(frame, visible.clone()).map(|position|
                (frame, position, ((frame - visible.start) as f64 - pointer_offset).abs())))
            .filter(|(_, _, distance)| *distance <= tolerance_frames)
            .min_by(|(frame_a, _, a), (frame_b, _, b)| a.total_cmp(b).then(frame_a.cmp(frame_b)));
        if let Some((_, position, _)) = snapped { return position; }
    }
    fraction
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
    use super::{flag_fraction, hint_position, snap_to_flags, view_peaks, DisplayPolicy, TimelineGuides, WaveformView, timeline_guides, snap_to_visible_markers};

    #[test]
    fn snapping_respects_marker_priority_before_nearness_and_earlier_frame_ties() {
        let guides = TimelineGuides { bars: vec![499], seconds: vec![500] };
        assert_eq!(snap_to_visible_markers(0.5, 0..1000, &[490], &guides, 0.02), 0.49);
        assert_eq!(snap_to_visible_markers(0.5, 0..1000, &[470], &guides, 0.02), 0.499);
        assert_eq!(snap_to_visible_markers(0.5, 0..1000, &[], &TimelineGuides { bars: vec![], seconds: vec![510, 490] }, 0.02), 0.49);
        assert_eq!(snap_to_visible_markers(0.2, 0..1000, &[], &TimelineGuides { bars: vec![], seconds: vec![300, 100] }, 0.11), 0.1);
    }

    #[test]
    fn snapping_uses_only_visible_boundaries_and_the_rendered_guide_set() {
        let policy = DisplayPolicy::for_view(600.0, 80.0, 60_000, 1000, false);
        let hidden = timeline_guides(1000..61_000, 1000, 1, &[1500], false, false, true, &policy);
        assert_eq!(hidden, TimelineGuides::default());
        assert_eq!(snap_to_visible_markers(0.01, 1000..61_000, &[900], &hidden, 0.05), 0.01);
        assert_eq!(snap_to_visible_markers(0.01, 1000..61_000, &[1000], &hidden, 0.05), 0.0);
        let transparent = timeline_guides(1000..61_000, 1000, 1, &[1500], true, true, false, &policy);
        assert_eq!(transparent, TimelineGuides::default());
        let visible = timeline_guides(1000..61_000, 1000, 1, &[1500], true, true, true, &policy);
        assert_eq!(snap_to_visible_markers(0.5, 1000..61_000, &[], &visible, 0.00001), 0.5);
        assert_eq!(snap_to_visible_markers(0.01, 1000..61_000, &[], &visible, 0.00001), 0.01);
    }

    #[test]
    fn long_views_reduce_detail_and_interaction_and_large_windows_restore_it() {
        let short = DisplayPolicy::for_view(600.0, 60.0, 10 * 48_000, 48_000, false);
        let long = DisplayPolicy::for_view(600.0, 60.0, 600 * 48_000, 48_000, false);
        let editing = DisplayPolicy::for_view(600.0, 60.0, 600 * 48_000, 48_000, true);
        let large = DisplayPolicy::for_view(1600.0, 320.0, 600 * 48_000, 48_000, false);
        assert!(short.bins > long.bins);
        assert!(editing.bins > long.bins);
        assert!(large.bins > long.bins);
        assert!(large.waveform_stroke > long.waveform_stroke);
        assert!(large.bar_stroke > long.bar_stroke);
        assert!(large.seconds_stroke > long.seconds_stroke);
    }

    #[test]
    fn timeline_density_preserves_interval_multiples_and_visible_edges() {
        let policy = DisplayPolicy::for_view(600.0, 80.0, 600_000, 1000, false);
        let dense_bars: Vec<_> = (0..=600_000).step_by(500).collect();
        let guides = timeline_guides(0..600_000, 1000, 5, &dense_bars, true, true, true, &policy);
        assert!(guides.seconds.len() < 121);
        assert!(guides.bars.len() < dense_bars.len());
        assert!(guides.seconds.iter().all(|frame| frame % 5000 == 0));
        assert_eq!(guides.bars.first(), Some(&0));
        assert!(guides.bars.windows(2).all(|pair| (pair[1] - pair[0]) as f32 * 600.0 / 600_000.0 >= policy.bar_spacing_px - 0.01));
        let end = timeline_guides(599_000..600_000, 1000, 1, &[600_000], true, true, true, &DisplayPolicy::for_view(600.0, 80.0, 1000, 1000, false));
        assert!(end.seconds.contains(&600_000));
        assert!(end.bars.contains(&600_000));
    }

    #[test]
    fn malformed_and_extreme_views_have_bounded_allocations_and_no_overflow() {
        use super::{MAX_TIMELINE_GUIDES, MAX_WAVEFORM_BINS};
        for (width, height, frames, rate) in [
            (0.0, 80.0, 1000, 1000),
            (600.0, 0.0, 1000, 1000),
            (f32::NAN, 80.0, 1000, 1000),
            (600.0, f32::INFINITY, 1000, 1000),
            (600.0, 80.0, 0, 1000),
            (600.0, 80.0, 1000, 0),
        ] {
            let policy = DisplayPolicy::for_view(width, height, frames, rate, true);
            assert_eq!(policy.bins, 0);
            assert_eq!(timeline_guides(0..1000, 1000, 1, &[500], true, true, true, &policy), TimelineGuides::default());
        }
        let huge = DisplayPolicy::for_view(f32::MAX, f32::MAX, u64::MAX, 1, true);
        assert!((1..=MAX_WAVEFORM_BINS).contains(&huge.bins));
        let guides = timeline_guides(0..u64::MAX, 1, 1, &[0, u64::MAX], true, true, true, &huge);
        assert!(guides.seconds.len() <= MAX_TIMELINE_GUIDES);
        assert!(guides.bars.len() <= MAX_TIMELINE_GUIDES);
        assert!(guides.seconds.windows(2).all(|pair| pair[0] < pair[1]));
        let near_end = timeline_guides(u64::MAX-100..u64::MAX, 1000, 1, &[u64::MAX], true, true, true,
            &DisplayPolicy::for_view(600.0, 80.0, 100, 1000, false));
        assert_eq!(near_end.bars, [u64::MAX]);
        assert!(near_end.seconds.is_empty());
        let one_frame = DisplayPolicy::for_view(1600.0, 320.0, 1, 48000, true);
        assert_eq!(one_frame.bins, 1);
    }

    #[test]
    fn snapping_threshold_is_in_screen_space_and_empty_or_invalid_inputs_do_not_snap() {
        let guides = TimelineGuides { bars: vec![500], seconds: vec![600] };
        let tolerance = 8.0 / 1000.0;
        assert_eq!(snap_to_visible_markers(0.491, 0..1000, &[], &guides, tolerance), 0.491);
        assert_eq!(snap_to_visible_markers(0.492, 0..1000, &[], &guides, tolerance), 0.5);
        assert_eq!(snap_to_visible_markers(0.5, 10..10, &[10], &guides, tolerance), 0.5);
        assert_eq!(snap_to_visible_markers(0.491, 0..1000, &[500], &guides, -1.0), 0.491);
        assert!(snap_to_visible_markers(f64::NAN, 0..1000, &[500], &guides, tolerance).is_nan());
        assert_eq!(snap_to_visible_markers(0.5, 0..1000, &[500], &guides, f64::INFINITY), 0.5);
    }

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

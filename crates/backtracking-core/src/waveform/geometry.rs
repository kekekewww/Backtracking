use std::ops::Range;

use super::MAX_WAVEFORM_BINS;

/// One paired upper/lower boundary of a continuous envelope strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopeColumn {
    pub x: f32,
    pub minimum: f32,
    pub maximum: f32,
}

/// A real sample position, measured from the first visible frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplePoint {
    pub x: f32,
    pub value: f32,
}

/// Each inner vector is a separate finite run. Never connect different runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum WaveformGeometry {
    #[default]
    Empty,
    Envelope(Vec<Vec<EnvelopeColumn>>),
    Samples(Vec<Vec<SamplePoint>>),
}

/// Produces bounded display-only geometry; the original audio is not changed.
///
/// Envelope columns cover complete bin extents. Render adjacent columns as a
/// filled strip and draw both outlines, rather than separate vertical strokes.
/// Samples are only used when each input bin describes exactly one real frame.
/// Coordinates use the half-open visible range: its first sample is at x = 0,
/// and the last sample is at (frame_count - 1) / frame_count.
///
/// Non-finite pairs split paths. Finite amplitudes are clipped to the plot's
/// [-1, 1] range, and reversed extrema are sorted. Invalid or oversized bin
/// counts are rejected instead of truncating and stretching a partial trace.
pub fn waveform_geometry(
    minimum: &[f32],
    maximum: &[f32],
    visible: Range<u64>,
) -> WaveformGeometry {
    let frames = visible.end.saturating_sub(visible.start);
    let count = minimum.len().min(maximum.len());
    if frames == 0 || count == 0 || count > MAX_WAVEFORM_BINS {
        return WaveformGeometry::Empty;
    }
    let pairs = || minimum.iter().zip(maximum).take(count);
    let samples = count as u64 == frames && pairs().all(|(&low, &high)| {
        !low.is_finite() || !high.is_finite() || low == high
    });
    if samples {
        let mut runs = Vec::new();
        let mut run = Vec::new();
        for (index, (&low, &high)) in pairs().enumerate() {
            if low.is_finite() && high.is_finite() {
                run.push(SamplePoint {
                    x: (index as f64 / count as f64) as f32,
                    value: low.clamp(-1.0, 1.0),
                });
            } else if !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
        }
        if !run.is_empty() { runs.push(run); }
        return if runs.is_empty() { WaveformGeometry::Empty } else { WaveformGeometry::Samples(runs) };
    }

    let mut runs = Vec::new();
    let mut run: Vec<EnvelopeColumn> = Vec::new();
    for (index, (&low, &high)) in pairs().enumerate() {
        if low.is_finite() && high.is_finite() {
            let mut column = EnvelopeColumn {
                x: (index as f64 / count as f64) as f32,
                minimum: low.min(high).clamp(-1.0, 1.0),
                maximum: low.max(high).clamp(-1.0, 1.0),
            };
            if run.is_empty() { run.push(column); }
            column.x = ((index as f64 + 0.5) / count as f64) as f32;
            run.push(column);
        } else if let Some(&last) = run.last() {
            run.push(EnvelopeColumn { x: (index as f64 / count as f64) as f32, ..last });
            runs.push(std::mem::take(&mut run));
        }
    }
    if let Some(&last) = run.last() {
        run.push(EnvelopeColumn { x: 1.0, ..last });
        runs.push(run);
    }
    if runs.is_empty() { WaveformGeometry::Empty } else { WaveformGeometry::Envelope(runs) }
}

/// The finest newly requested view places about four logical pixels per sample.
/// A resize does not itself request a new view.
pub fn minimum_zoom_frames(audio_width: f32, retained_frames: u64) -> u64 {
    let desired = if audio_width.is_finite() && audio_width > 0.0 {
        (audio_width.min(super::MAX_PLOT_WIDTH) / 4.0).floor().max(2.0) as u64
    } else {
        2
    };
    desired.min(MAX_WAVEFORM_BINS as u64).min(retained_frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_covers_bin_edges_and_keeps_every_peak_without_column_gaps() {
        let low = [-0.1, -0.95, -0.2, 0.1];
        let high = [0.2, 0.3, 0.99, 0.4];
        let WaveformGeometry::Envelope(runs) = waveform_geometry(&low, &high, 100..1_100) else {
            panic!("an overview must have an envelope");
        };
        assert_eq!(runs.len(), 1);
        let columns = &runs[0];
        assert_eq!(columns.len(), 6);
        assert_eq!(columns.first().unwrap().x, 0.0);
        assert_eq!(columns.last().unwrap().x, 1.0);
        for (index, column) in columns[1..5].iter().enumerate() {
            assert_eq!(column.x, (index as f32 + 0.5) / 4.0);
            assert_eq!(column.minimum, low[index]);
            assert_eq!(column.maximum, high[index]);
        }
        assert!(columns.windows(2).all(|pair| pair[0].x < pair[1].x));
    }

    #[test]
    fn samples_keep_frame_positions_and_values_without_a_half_bin_offset() {
        let values = [-0.25, 0.5, -0.75, 1.0];
        let WaveformGeometry::Samples(runs) = waveform_geometry(&values, &values, 1_000_000..1_000_004) else {
            panic!("one bin per frame must use the original samples");
        };
        assert_eq!(runs, vec![vec![
            SamplePoint { x: 0.0, value: -0.25 },
            SamplePoint { x: 0.25, value: 0.5 },
            SamplePoint { x: 0.5, value: -0.75 },
            SamplePoint { x: 0.75, value: 1.0 },
        ]]);
    }

    #[test]
    fn unequal_extrema_are_not_mistaken_for_samples_or_replaced_with_midpoints() {
        let WaveformGeometry::Envelope(runs) = waveform_geometry(&[-0.8, -0.2], &[0.1, 0.9], 10..12) else {
            panic!("extrema contain no sample ordering information");
        };
        assert_eq!(runs[0][1].minimum, -0.8);
        assert_eq!(runs[0][1].maximum, 0.1);
        assert_eq!(runs[0][2].maximum, 0.9);
    }

    #[test]
    fn non_finite_bins_split_both_modes_without_bridging_unknown_audio() {
        let WaveformGeometry::Envelope(runs) = waveform_geometry(
            &[0.0, f32::NAN, -0.5, 0.1, f32::INFINITY],
            &[1.0, 0.5, 0.5, 0.2, 0.9], 0..100,
        ) else { panic!("finite envelope runs remain visible"); };
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].first().unwrap().x, 0.0);
        assert_eq!(runs[0].last().unwrap().x, 0.2);
        assert_eq!(runs[1].first().unwrap().x, 0.4);
        assert_eq!(runs[1].last().unwrap().x, 0.8);

        let values = [0.1, f32::NAN, -0.2, f32::INFINITY];
        let WaveformGeometry::Samples(runs) = waveform_geometry(&values, &values, 0..4) else {
            panic!("finite samples remain at their own positions");
        };
        assert_eq!(runs, vec![
            vec![SamplePoint { x: 0.0, value: 0.1 }],
            vec![SamplePoint { x: 0.5, value: -0.2 }],
        ]);
        assert_eq!(waveform_geometry(&[f32::NAN], &[f32::NAN], 0..1), WaveformGeometry::Empty);
    }

    #[test]
    fn a_single_sample_is_one_real_point_and_a_quiet_envelope_still_has_a_continuous_outline() {
        assert_eq!(waveform_geometry(&[0.25], &[0.25], 50..51),
            WaveformGeometry::Samples(vec![vec![SamplePoint { x: 0.0, value: 0.25 }]]));
        let WaveformGeometry::Envelope(runs) = waveform_geometry(&[0.0, 0.0], &[0.0, 0.0], 0..100) else {
            panic!("zero amplitude has a visible full-width outline");
        };
        assert_eq!(runs[0].first().unwrap().x, 0.0);
        assert_eq!(runs[0].last().unwrap().x, 1.0);
        assert!(runs[0].iter().all(|column| column.minimum == 0.0 && column.maximum == 0.0));
    }

    #[test]
    fn repeated_bins_are_a_legal_envelope_and_never_claim_extra_sample_positions() {
        let repeated = [0.25; 4];
        let WaveformGeometry::Envelope(runs) = waveform_geometry(&repeated, &repeated, 50..51) else {
            panic!("core can legally repeat one frame across several requested bins");
        };
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len(), 6);
        assert_eq!(runs[0].first().unwrap().x, 0.0);
        assert_eq!(runs[0].last().unwrap().x, 1.0);
        assert!(runs[0].iter().all(|column| column.minimum == 0.25 && column.maximum == 0.25));
    }

    #[test]
    fn malformed_and_extreme_geometry_is_bounded_and_stays_inside_the_plot() {
        assert_eq!(waveform_geometry(&[], &[], 0..100), WaveformGeometry::Empty);
        assert_eq!(waveform_geometry(&[0.0], &[0.0], 4..4), WaveformGeometry::Empty);
        assert_eq!(waveform_geometry(&[0.0], &[0.0], 9..1), WaveformGeometry::Empty);
        let oversized = vec![0.0; MAX_WAVEFORM_BINS + 1];
        assert_eq!(waveform_geometry(&oversized, &oversized, 0..u64::MAX), WaveformGeometry::Empty);

        let WaveformGeometry::Samples(runs) = waveform_geometry(
            &[-f32::MAX, f32::MAX], &[-f32::MAX, f32::MAX], u64::MAX-2..u64::MAX,
        ) else { panic!("large absolute frame numbers must retain small relative positions"); };
        assert_eq!(runs[0], vec![SamplePoint { x: 0.0, value: -1.0 }, SamplePoint { x: 0.5, value: 1.0 }]);
        let WaveformGeometry::Envelope(runs) = waveform_geometry(&[2.0], &[-3.0], 0..u64::MAX) else {
            panic!("finite inverted extrema remain a sorted display envelope");
        };
        assert!(runs[0].iter().all(|column| column.minimum == -1.0 && column.maximum == 1.0));
    }

    #[test]
    fn the_finest_zoom_uses_plot_width_and_handles_empty_tiny_and_invalid_views() {
        assert_eq!(minimum_zoom_frames(600.0, 48_000), 150);
        assert_eq!(minimum_zoom_frames(1_200.0, 48_000), 300);
        assert_eq!(minimum_zoom_frames(600.0, 100), 100);
        assert_eq!(minimum_zoom_frames(600.0, 1), 1);
        assert_eq!(minimum_zoom_frames(600.0, 0), 0);
        assert_eq!(minimum_zoom_frames(1.0, u64::MAX), 2);
        assert_eq!(minimum_zoom_frames(f32::MAX, u64::MAX), 4_096);
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(minimum_zoom_frames(invalid, 1_000), 2);
        }
    }

    #[test]
    fn display_policy_preserves_every_frame_once_the_view_is_sample_sized() {
        for (width, frames) in [(600.0, 500), (600.0, 480), (1_500.0, 1_000)] {
            for interactive in [false, true] {
                let policy = super::super::DisplayPolicy::for_view(width, 160.0, frames, 48_000, interactive);
                assert_eq!(policy.bins as u64, frames);
            }
        }
        let wide = super::super::DisplayPolicy::for_view(10_000.0, 160.0, 5_000, 48_000, false);
        assert!(wide.bins <= MAX_WAVEFORM_BINS);
    }
}

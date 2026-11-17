//! What a selection shows and does beyond its two flags: the fades dragged from them and the
//! playhead of its preview.

use std::ops::Range;

// The fade math lives in core so the macOS app, which draws natively, shares it.
pub use backtracking_core::{FadeEnd, dragged_fades, fade_waveform_bins};
use backtracking_core::Fades;
use egui::{Color32, Pos2, Rect, Stroke};

use crate::timeline::{FlagReach, flag_handle_at};

/// The ramp whose flag lies under `position`, judged as [`flag_handle_at`] picks up flags.
pub fn fade_end_at(
    rect: Rect,
    visible: Range<u64>,
    selection: &Range<u64>,
    position: Pos2,
    reach: FlagReach,
) -> Option<FadeEnd> {
    let (_, moved) = flag_handle_at(rect, visible, selection, position, reach)?;
    Some(if moved == selection.start { FadeEnd::In } else { FadeEnd::Out })
}

/// Where `frame` falls across `rect` while it shows `visible`, extended past either edge.
fn unclamped_x(rect: Rect, visible: &Range<u64>, frame: u64) -> f32 {
    let length = visible.end.saturating_sub(visible.start).max(1) as f64;
    let fraction = (frame as f64 - visible.start as f64) / length;
    rect.left() + (fraction * f64::from(rect.width())) as f32
}

/// Draws the selection's fades on one waveform row: a wedge from silence at the flag to full
/// level where the ramp ends, with the cut-away level shaded. `middle` is the row's zero line
/// and `extent` the height of full scale above it.
#[allow(clippy::too_many_arguments)]
pub fn paint_fades(
    painter: &egui::Painter,
    rect: Rect,
    middle: f32,
    extent: f32,
    visible: &Range<u64>,
    selection: &Range<u64>,
    fades: Fades,
    color: Color32,
) {
    let length = selection.end.saturating_sub(selection.start);
    let fades = fades.fitted(length);
    if visible.is_empty() {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let shade = Color32::from_black_alpha(110);
    let line = Stroke::new(1.0, color);
    for (edge, full) in [
        (selection.start, selection.start + fades.fade_in),
        (selection.end, selection.end - fades.fade_out),
    ] {
        if edge == full {
            continue;
        }
        let edge = unclamped_x(rect, visible, edge);
        let full = unclamped_x(rect, visible, full);
        let silent = Pos2::new(edge, middle);
        let top = Pos2::new(full, middle - extent);
        let bottom = Pos2::new(full, middle + extent);
        for corner in [top, bottom] {
            painter.add(egui::Shape::convex_polygon(
                vec![silent, Pos2::new(edge, corner.y), corner],
                shade,
                Stroke::NONE,
            ));
        }
        painter.line_segment([silent, top], line);
        painter.line_segment([silent, bottom], line);
    }
}

/// Draws a preview's playhead at `frame` across `rect`, when `visible` includes it.
pub fn paint_playhead(
    painter: &egui::Painter,
    rect: Rect,
    visible: &Range<u64>,
    frame: u64,
    color: Color32,
) {
    if visible.is_empty() || frame < visible.start || frame > visible.end {
        return;
    }
    let x = unclamped_x(rect, visible, frame).clamp(rect.left(), rect.right());
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    painter.vline(x, rect.y_range(), Stroke::new(1.5, color));
    let size = rect.height().clamp(4.0, 10.0) * 0.5;
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(x - size, rect.top()),
            Pos2::new(x + size, rect.top()),
            Pos2::new(x, rect.top() + size),
        ],
        color,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use egui::Vec2;

    use super::*;

    #[test]
    fn the_start_flag_controls_the_fade_in_and_the_end_flag_the_fade_out() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1_000.0, 80.0));
        let reach = FlagReach { above: 2.0, below: 13.0, radius: 8.0 };
        let selection = 200..600;
        let at = |x| fade_end_at(rect, 0..1_000, &selection, Pos2::new(x, 4.0), reach);
        assert_eq!(at(203.0), Some(FadeEnd::In));
        assert_eq!(at(596.0), Some(FadeEnd::Out));
        assert_eq!(at(400.0), None);
        // Only the flag band at the top of the plot picks a flag up.
        assert_eq!(fade_end_at(rect, 0..1_000, &selection, Pos2::new(200.0, 40.0), reach), None);
    }

    #[test]
    fn the_playhead_is_drawn_only_while_its_frame_is_on_screen() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(100.0, 40.0));
        let shapes = |frame| {
            let context = egui::Context::default();
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                paint_playhead(ui.painter(), rect, &(1_000..2_000), frame, Color32::WHITE);
            });
            let shapes: Vec<_> = output.shapes.drain(..).map(|shape| shape.shape).collect();
            output.textures_delta.clear();
            output.drop_without_applying_deltas();
            shapes
        };
        assert!(shapes(999).is_empty());
        assert!(shapes(2_001).is_empty());
        let drawn = shapes(1_500);
        let egui::Shape::LineSegment { points, .. } = &drawn[0] else {
            panic!("expected the playhead line, got {:?}", drawn[0]);
        };
        assert_eq!(points[0].x, 60.0);
        assert_eq!((points[0].y, points[1].y), (20.0, 60.0));
    }
}

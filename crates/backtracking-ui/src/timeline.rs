use std::ops::Range;

use backtracking_core::waveform;
use egui::{PointerButton, Pos2, Rect, Stroke};

/// Keeps a view of `count` frames starting at `start` inside the retained `history`.
pub fn fit_frame_range(history: &Range<u64>, start: u64, count: u64) -> Range<u64> {
    let count = count.min(history.end - history.start);
    let start = start.clamp(history.start, history.end - count);
    start..start + count
}

/// What the modifiers held at a press mean: `(selecting, snap)`. Shift selects and snaps,
/// Ctrl selects freely, and Alt selects freely where `alt_selects` allows it.
pub fn selection_modifiers(modifiers: egui::Modifiers, alt_selects: bool) -> (bool, bool) {
    (
        (alt_selects && modifiers.alt) || modifiers.ctrl || modifiers.shift,
        modifiers.shift,
    )
}

/// The latest primary-button press among `events`, as `(position, selecting, snap)`.
pub fn waveform_press(events: &[egui::Event], alt_selects: bool) -> Option<(Pos2, bool, bool)> {
    events.iter().rev().find_map(|event| match event {
        egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers } => {
            let (selecting, snap) = selection_modifiers(*modifiers, alt_selects);
            Some((*pos, selecting, snap))
        }
        _ => None,
    })
}

/// Where `frame` is drawn across `rect` while it shows the `visible` frames.
pub fn frame_x(rect: Rect, visible: Range<u64>, frame: u64) -> Option<f32> {
    let fraction = waveform::flag_fraction(frame, visible)?;
    Some(rect.left() + fraction as f32 * rect.width())
}

/// How close to a flag a press has to be to pick it up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlagReach {
    /// How far above the top of the plot the press may be.
    pub above: f32,
    /// How far below the top of the plot the press may be.
    pub below: f32,
    /// How far to either side of the flag the press may be.
    pub radius: f32,
}

/// The flag of `flags` under `position`, as `(fixed, moved)`: the boundary that stays where it
/// is and the one the pointer picked up. The nearer flag wins, and the start flag wins a tie.
pub fn flag_handle_at(
    rect: Rect,
    visible: Range<u64>,
    flags: &Range<u64>,
    position: Pos2,
    reach: FlagReach,
) -> Option<(u64, u64)> {
    if position.y < rect.top() - reach.above || position.y > rect.top() + reach.below {
        return None;
    }
    [(flags.start, flags.end), (flags.end, flags.start)]
        .into_iter()
        .filter_map(|(moved, fixed)| {
            let x = frame_x(rect, visible.clone(), moved)?;
            let distance = (x - position.x).abs();
            (distance <= reach.radius).then_some((distance, fixed, moved))
        })
        .min_by(|(a, _, _), (b, _, _)| a.total_cmp(b))
        .map(|(_, fixed, moved)| (fixed, moved))
}

/// Draws one vertical line across `rect` for each of `frames` inside the `visible` range.
pub fn paint_guide_lines(
    painter: &egui::Painter,
    rect: Rect,
    visible: &Range<u64>,
    frames: &[u64],
    stroke: Stroke,
) {
    for &frame in frames {
        if let Some(x) = frame_x(rect, visible.clone(), frame) {
            painter.vline(x, rect.y_range(), stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::Vec2;

    use super::*;

    #[test]
    fn waveform_pan_clamps_to_retained_frames_and_preserves_view_length() {
        assert_eq!(fit_frame_range(&(100..500), 170, 120), 170..290);
        assert_eq!(fit_frame_range(&(100..500), 0, 120), 100..220);
        assert_eq!(fit_frame_range(&(100..500), 900, 120), 380..500);
        assert_eq!(fit_frame_range(&(220..270), 200, 120), 220..270);
        assert_eq!(fit_frame_range(&(220..220), 900, 120), 220..220);
    }

    #[test]
    fn shift_alone_starts_a_snapped_selection_and_other_selection_modifiers_remain_free() {
        assert_eq!(selection_modifiers(egui::Modifiers::SHIFT, true), (true, true));
        assert_eq!(selection_modifiers(egui::Modifiers::CTRL, true), (true, false));
        assert_eq!(selection_modifiers(egui::Modifiers::ALT, true), (true, false));
        assert_eq!(selection_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, true), (true, true));
        assert_eq!(selection_modifiers(egui::Modifiers::NONE, true), (false, false));
    }

    #[test]
    fn alt_starts_a_selection_only_where_it_is_not_reserved() {
        assert_eq!(selection_modifiers(egui::Modifiers::ALT, false), (false, false));
        assert_eq!(selection_modifiers(egui::Modifiers::ALT | egui::Modifiers::SHIFT, false), (true, true));
        assert_eq!(selection_modifiers(egui::Modifiers::ALT | egui::Modifiers::CTRL, false), (true, false));
        assert_eq!(selection_modifiers(egui::Modifiers::CTRL, false), (true, false));
        assert_eq!(selection_modifiers(egui::Modifiers::NONE, false), (false, false));
    }

    #[test]
    fn shift_pointer_input_selects_and_snaps_while_ctrl_remains_free() {
        for (ctrl, shift, selecting, snap) in [
            (false, true, true, true), (true, false, true, false),
            (true, true, true, true), (false, false, false, false),
        ] {
            let context = egui::Context::default();
            let modifiers = egui::Modifiers { ctrl, shift, ..Default::default() };
            let point = Pos2::new(17.0, 27.0);
            context.run_ui(egui::RawInput {
                events: vec![egui::Event::PointerMoved(point), egui::Event::PointerButton {
                    pos: point, button: PointerButton::Primary, pressed: true, modifiers,
                }],
                ..Default::default()
            }, |ui| {
                for alt_selects in [false, true] {
                    let press = ui.input(|input| waveform_press(&input.events, alt_selects));
                    assert_eq!(press, Some((point, selecting, snap)));
                }
            }).drop_without_applying_deltas();
        }
    }

    #[test]
    fn the_latest_primary_press_wins_and_other_buttons_are_ignored() {
        let press = |pos, button, pressed, modifiers| egui::Event::PointerButton {
            pos, button, pressed, modifiers,
        };
        let first = Pos2::new(4.0, 5.0);
        let second = Pos2::new(40.0, 50.0);
        let events = [
            press(first, PointerButton::Primary, true, egui::Modifiers::NONE),
            press(first, PointerButton::Primary, false, egui::Modifiers::NONE),
            press(second, PointerButton::Primary, true, egui::Modifiers::ALT),
            press(first, PointerButton::Secondary, true, egui::Modifiers::SHIFT),
        ];
        assert_eq!(waveform_press(&events, true), Some((second, true, false)));
        assert_eq!(waveform_press(&events, false), Some((second, false, false)));
        assert_eq!(waveform_press(&events[1..2], true), None);
        assert_eq!(waveform_press(&[], true), None);
    }

    #[test]
    fn frames_map_to_the_plot_including_its_right_edge_and_nothing_outside_it() {
        let rect = Rect::from_min_size(Pos2::new(900.0, 40.0), Vec2::new(100.0, 80.0));
        assert_eq!(frame_x(rect, 1000..11000, 1000), Some(900.0));
        assert_eq!(frame_x(rect, 1000..11000, 2500), Some(915.0));
        assert_eq!(frame_x(rect, 1000..11000, 11000), Some(1000.0));
        assert_eq!(frame_x(rect, 1000..11000, 999), None);
        assert_eq!(frame_x(rect, 1000..11000, 11001), None);
        assert_eq!(frame_x(rect, 1000..1000, 1000), None);
    }

    #[test]
    fn a_press_picks_up_the_nearer_flag_within_reach_and_keeps_the_other_fixed() {
        let rect = Rect::from_min_size(Pos2::new(18.0, 28.0), Vec2::new(224.0, 84.0));
        let visible = 100..1_100;
        // The flags stand at x = 40.4 and x = 107.6.
        let flags = 200..500;
        let reach = FlagReach { above: 4.0, below: 14.0, radius: 8.0 };
        let at = |x, y| flag_handle_at(rect, visible.clone(), &flags, Pos2::new(x, y), reach);

        assert_eq!(at(40.0, 30.0), Some((500, 200)));
        assert_eq!(at(44.0, 30.0), Some((500, 200)));
        assert_eq!(at(107.0, 30.0), Some((200, 500)));
        assert_eq!(at(100.0, 30.0), Some((200, 500)));
        assert_eq!(at(48.4, 30.0), Some((500, 200)), "The radius is inclusive");
        assert_eq!(at(49.0, 30.0), None);
        assert_eq!(at(74.0, 30.0), None);

        // The band is measured from the top of the plot, and both of its edges are inclusive.
        assert_eq!(at(40.0, 24.0), Some((500, 200)));
        assert_eq!(at(40.0, 23.9), None);
        assert_eq!(at(40.0, 42.0), Some((500, 200)));
        assert_eq!(at(40.0, 42.1), None);
        let narrow = FlagReach { above: 2.0, below: 13.0, radius: 8.0 };
        let at_narrow = |y| flag_handle_at(rect, visible.clone(), &flags, Pos2::new(40.0, y), narrow);
        assert_eq!(at_narrow(26.0), Some((500, 200)));
        assert_eq!(at_narrow(25.0), None);
        assert_eq!(at_narrow(41.0), Some((500, 200)));
        assert_eq!(at_narrow(42.0), None);
    }

    #[test]
    fn close_flags_resolve_to_the_nearer_one_and_hidden_flags_cannot_be_picked_up() {
        // One frame per point, so every flag stands at an exact position.
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(256.0, 80.0));
        let reach = FlagReach { above: 2.0, below: 13.0, radius: 8.0 };
        let at = |visible: Range<u64>, flags: Range<u64>, x| {
            flag_handle_at(rect, visible, &flags, Pos2::new(x, 5.0), reach)
        };
        // Flags at x = 100 and x = 106.
        assert_eq!(at(0..256, 100..106, 102.0), Some((106, 100)));
        assert_eq!(at(0..256, 100..106, 104.0), Some((100, 106)));
        assert_eq!(at(0..256, 100..106, 103.0), Some((106, 100)), "The start flag wins a tie");
        // The start flag has scrolled out of view; the end flag is still there.
        assert_eq!(at(150..406, 100..200, 0.0), None);
        assert_eq!(at(150..406, 100..200, 50.0), Some((100, 200)));
    }

    #[test]
    fn guide_lines_are_drawn_at_their_frames_across_the_whole_plot() {
        let rect = Rect::from_min_size(Pos2::new(18.0, 28.0), Vec2::new(224.0, 84.0));
        let stroke = Stroke::new(0.75, egui::Color32::from_white_alpha(52));
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            paint_guide_lines(ui.painter(), rect, &(100..1_100), &[50, 100, 400, 1_100, 1_200], stroke);
        });
        let lines: Vec<_> = output.shapes.iter().map(|shape| match &shape.shape {
            egui::Shape::LineSegment { points, stroke } => (*points, *stroke),
            other => panic!("unexpected shape {other:?}"),
        }).collect();
        output.textures_delta.clear();
        output.drop_without_applying_deltas();
        assert_eq!(lines, [18.0, 85.200005, 242.0].map(|x| {
            ([Pos2::new(x, 28.0), Pos2::new(x, 112.0)], stroke)
        }));
    }
}

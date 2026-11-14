use egui::{Color32, Pos2, Rect, Stroke, Vec2};

/// A square toolbar button with no label. Its icon is painted over it by the caller.
pub fn icon_button(size: f32) -> egui::Button<'static> {
    egui::Button::new("").min_size(Vec2::splat(size))
}

/// The measurements of the start/pause icon, relative to the centre of its button.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportGlyph {
    /// The corners of the start triangle.
    pub play: [Vec2; 3],
    /// How far to either side of the centre each pause bar stands.
    pub pause_offset: f32,
    /// The size of one pause bar.
    pub pause_bar: Vec2,
}

/// Paints the start triangle while capture is paused, and the pause bars while it runs.
pub fn paint_transport_glyph(
    painter: &egui::Painter,
    center: Pos2,
    paused: bool,
    color: Color32,
    glyph: &TransportGlyph,
) {
    if paused {
        painter.add(egui::Shape::convex_polygon(
            glyph.play.iter().map(|&corner| center + corner).collect(),
            color,
            Stroke::NONE,
        ));
    } else {
        for x in [-glyph.pause_offset, glyph.pause_offset] {
            painter.rect_filled(
                Rect::from_center_size(center + Vec2::new(x, 0.0), glyph.pause_bar),
                0.5,
                color,
            );
        }
    }
}

/// A status light: a dot of `radius` centred in `size`, named `status` for the tooltip and
/// for assistive technology.
pub fn status_led(ui: &mut egui::Ui, size: Vec2, radius: f32, color: Color32, status: &str) {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, status));
    response.on_hover_text(status);
    ui.painter().circle_filled(rect.center(), radius, color);
}

/// A meter reading to one decimal place, or a dash while there is none. `signed` writes the
/// plus sign of a positive reading.
pub fn meter_number(value: Option<f32>, signed: bool) -> String {
    match value.filter(|value| value.is_finite()) {
        Some(value) if signed => format!("{value:+.1}"),
        Some(value) => format!("{value:.1}"),
        None => "—".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_numbers_have_one_decimal_and_a_dash_stands_for_no_reading() {
        assert_eq!(meter_number(Some(-12.34), false), "-12.3");
        assert_eq!(meter_number(Some(-12.34), true), "-12.3");
        assert_eq!(meter_number(Some(1.25), false), "1.2");
        assert_eq!(meter_number(Some(1.26), true), "+1.3");
        assert_eq!(meter_number(Some(0.0), true), "+0.0");
        assert_eq!(meter_number(Some(0.0), false), "0.0");
        for signed in [false, true] {
            assert_eq!(meter_number(None, signed), "—");
            assert_eq!(meter_number(Some(f32::NEG_INFINITY), signed), "—");
            assert_eq!(meter_number(Some(f32::NAN), signed), "—");
        }
    }
}

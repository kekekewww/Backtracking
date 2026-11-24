//! The Options form: its type scale, colours and the widgets every page is built from.
//!
//! The desktop shows the form in a window of its own and the plug-in inside its editor. Each
//! lays out its own frame around these pieces, so the pages read the same in both.

use backtracking_core::waveform;
use egui::{Color32, Pos2, Rect, Vec2};

use crate::options::combo_popup_height;

// Text in the Options form, brightest first: a title, what a control holds, the name of a
// setting, and a note under one. Titles and values lead; names and notes step back.
pub const OPTIONS_TITLE_TEXT: Color32 = Color32::from_gray(240);
pub const OPTIONS_VALUE_TEXT: Color32 = Color32::from_gray(222);
pub const OPTIONS_LABEL_TEXT: Color32 = Color32::from_gray(186);
pub const OPTIONS_NOTE_TEXT: Color32 = Color32::from_gray(138);
/// The outline that sets a button, a drop-down or a checkbox apart from the page.
pub const OPTIONS_CONTROL_OUTLINE: Color32 = Color32::from_gray(74);
pub const OPTIONS_NAVIGATION_WIDTH: f32 = 172.0;
pub const OPTIONS_NAVIGATION_ROW: f32 = 36.0;
/// A setting's name sits beside its control from this form width up, and above it below.
pub const OPTIONS_TWO_COLUMN_WIDTH: f32 = 440.0;
pub const OPTIONS_LABEL_WIDTH: f32 = 236.0;
/// The most of a two-column row that the name of a setting may take.
const OPTIONS_LABEL_SHARE: f32 = 0.46;
/// Every drop-down has this width when there is room, so a page lines up.
pub const OPTIONS_CONTROL_WIDTH: f32 = 300.0;
pub const OPTIONS_MENU_ROW_GAP: f32 = 4.0;

/// The bundled bold face, or the regular one where it is not loaded, as in a headless frame.
/// `RichText::strong` only changes colour in egui, so headings name the face itself.
pub fn options_bold_family(ui: &egui::Ui) -> egui::FontFamily {
    let bold = egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into());
    if ui.fonts(|fonts| fonts.families().contains(&bold)) {
        bold
    } else {
        egui::FontFamily::Proportional
    }
}

pub fn configure_options_style(ui: &mut egui::Ui) {
    let bold = options_bold_family(ui);
    let style = ui.style_mut();
    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(13.0));
    style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::new(20.0, bold));
    style.spacing.interact_size.y = 32.0;
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    // One gap between every pair of rows, whatever kind of control they hold.
    style.spacing.item_spacing.y = 10.0;
    // A page that runs on fades out at the edge and keeps its scroll handle in view.
    style.spacing.scroll.dormant_handle_opacity = 0.5;
    style.spacing.scroll.fade.strength = 1.0;
    style.spacing.scroll.fade.size = 28.0;
    let widgets = &mut style.visuals.widgets;
    widgets.noninteractive.fg_stroke.color = OPTIONS_LABEL_TEXT;
    widgets.inactive.fg_stroke.color = OPTIONS_VALUE_TEXT;
    widgets.inactive.bg_stroke = egui::Stroke::new(1.0, OPTIONS_CONTROL_OUTLINE);
}

pub fn options_heading_text(ui: &egui::Ui, label: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(label)
        .font(egui::FontId::new(size, options_bold_family(ui)))
        .color(OPTIONS_TITLE_TEXT)
}

pub fn options_group_heading(ui: &mut egui::Ui, label: &str) {
    ui.add_space(2.0);
    let text = options_heading_text(ui, label, 15.0);
    ui.label(text);
}

/// One page in the list on the left. Its size never depends on whether it is selected.
pub fn options_navigation_item(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    accent: Color32,
) -> egui::Response {
    let size = Vec2::new(ui.available_width(), OPTIONS_NAVIGATION_ROW);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, selected, label)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if selected {
            Color32::from_gray(48)
        } else if response.hovered() || response.has_focus() {
            Color32::from_gray(38)
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect, 6.0, fill);
        if selected {
            let bar = Rect::from_min_max(
                Pos2::new(rect.left(), rect.top() + 9.0),
                Pos2::new(rect.left() + 3.0, rect.bottom() - 9.0),
            );
            painter.rect_filled(bar, 1.5, accent);
        }
        painter.text(
            Pos2::new(rect.left() + 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(15.0),
            if selected { OPTIONS_TITLE_TEXT } else { OPTIONS_LABEL_TEXT },
        );
    }
    response
}

pub fn options_control_width(ui: &egui::Ui) -> f32 {
    ui.available_width().min(OPTIONS_CONTROL_WIDTH)
}

/// Where the right edge of the control in an `options_field` row placed in `ui` now falls, so
/// something above the rows can line up with them.
pub fn options_control_right(ui: &egui::Ui) -> f32 {
    let (left, width) = (ui.cursor().left(), ui.available_width());
    if width < OPTIONS_TWO_COLUMN_WIDTH {
        return left + width.min(OPTIONS_CONTROL_WIDTH);
    }
    let label = OPTIONS_LABEL_WIDTH.min(width * OPTIONS_LABEL_SHARE);
    let spacing = ui.spacing().item_spacing.x;
    left + label + spacing + (width - label - spacing).min(OPTIONS_CONTROL_WIDTH)
}

/// A value that is shown, not edited, on the same line height as a control.
pub fn options_value(ui: &mut egui::Ui, text: &str) {
    let size = Vec2::new(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_height(size.y);
        ui.add(egui::Label::new(egui::RichText::new(text).color(OPTIONS_VALUE_TEXT)).truncate())
            .on_hover_text(text);
    });
}

/// A quiet line of explanation under a control.
pub fn options_note(ui: &mut egui::Ui, text: impl Into<String>) {
    let text = egui::RichText::new(text).size(13.0).color(OPTIONS_NOTE_TEXT);
    ui.add(egui::Label::new(text).wrap());
}

/// One page of the form: its rows run down, as wide as the space, and long text wraps.
pub fn options_list<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
        configure_options_style(ui);
        ui.set_width(ui.available_width());
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        contents(ui)
    }).inner
}

pub fn options_menu_height(ui: &egui::Ui, item_count: usize) -> f32 {
    combo_popup_height(ui, item_count, OPTIONS_MENU_ROW_GAP)
}

/// Makes an open drop-down list measure its rows again on the next frame.
///
/// egui measures a list once, when it opens, and its scroll area never grows past that size.
/// A list that gains rows while it is open would keep its first height and hide the rest.
/// `menu` is `ui.layer_id()` taken inside the list. Call this before the list is drawn; the
/// list is invisible for the one frame in which it is measured.
pub fn remeasure_options_menu(ctx: &egui::Context, menu: egui::LayerId) {
    if !egui::Popup::is_id_open(ctx, menu.id) {
        return;
    }
    if let Some(mut state) = egui::AreaState::load(ctx, menu.id) {
        state.size = None;
        ctx.memory_mut(|memory| memory.areas_mut().set_state(menu, state));
    }
}

pub fn configure_options_menu(ui: &mut egui::Ui, width: f32) {
    configure_options_style(ui);
    ui.set_max_width(width);
    ui.spacing_mut().item_spacing.y = OPTIONS_MENU_ROW_GAP;
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
}

/// A named setting: the name on the left and its control on the right, or stacked when the
/// form is too narrow for both.
pub fn options_field<R>(
    ui: &mut egui::Ui,
    label: &str,
    control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let width = ui.available_width();
    if width < OPTIONS_TWO_COLUMN_WIDTH {
        return ui.vertical(|ui| {
            ui.label(label);
            control(ui)
        }).inner;
    }
    ui.horizontal_top(|ui| {
        // The name is centred on the first line of the control beside it.
        let name = Vec2::new(
            OPTIONS_LABEL_WIDTH.min(width * OPTIONS_LABEL_SHARE),
            ui.spacing().interact_size.y,
        );
        ui.allocate_ui_with_layout(name, egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_size(name);
            ui.add(egui::Label::new(label).truncate()).on_hover_text(label);
        });
        ui.vertical(|ui| control(ui)).inner
    }).inner
}

/// A drop-down for one of `choices`. True when the user picked a different one.
pub fn options_choice<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    selected: &mut T,
    choices: impl IntoIterator<Item = (T, String)>,
) -> bool {
    let choices: Vec<_> = choices.into_iter().collect();
    let text = choices.iter().find(|(value, _)| value == selected)
        .map_or_else(String::new, |(_, text)| text.clone());
    let before = *selected;
    let width = options_control_width(ui);
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .height(options_menu_height(ui, choices.len()))
        .selected_text(&text)
        .truncate()
        .show_ui(ui, |ui| {
            configure_options_menu(ui, width);
            for (value, label) in choices {
                ui.selectable_value(selected, value, label);
            }
        }).response.on_hover_text(text);
    *selected != before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: f32, contents: impl FnMut(&mut egui::Ui)) {
        egui::Context::default().run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 400.0))),
            ..Default::default()
        }, contents).drop_without_applying_deltas();
    }

    #[test]
    fn a_setting_sits_beside_its_control_only_when_the_form_is_wide_enough() {
        for (width, beside) in [(300.0, false), (439.0, false), (460.0, true), (700.0, true)] {
            frame(width, |ui| {
                options_list(ui, |ui| {
                    let form = ui.available_width();
                    let mut value = 1;
                    let (row, control) = {
                        let row = ui.scope(|ui| options_field(ui, "A setting", |ui| {
                            options_choice(ui, "choice", &mut value,
                                [(1, "One".to_owned()), (2, "Two".to_owned())]);
                            ui.min_rect()
                        }));
                        (row.response.rect, row.inner)
                    };
                    assert_eq!(control.left() > row.left() + 100.0, beside, "at {width}");
                    assert!(row.right() <= ui.max_rect().left() + form + 1.0, "at {width}");
                    // The name never takes more than its share, so the control keeps room.
                    if beside {
                        assert!(control.left() - row.left() <= OPTIONS_LABEL_WIDTH + 16.0);
                        assert!(control.width() >= 200.0, "control {control:?} at {width}");
                    }
                });
            });
        }
    }

    /// Draws one frame of a drop-down list of `rows` rows and returns where its button is and,
    /// while the list is open, its layer.
    fn menu_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        rows: usize,
        remeasure: Option<egui::LayerId>,
    ) -> (Rect, Option<egui::LayerId>) {
        let mut seen = (Rect::NOTHING, None);
        ctx.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 700.0))),
            events,
            ..Default::default()
        }, |ui| {
            if let Some(menu) = remeasure {
                remeasure_options_menu(ui.ctx(), menu);
            }
            options_list(ui, |ui| {
                let width = options_control_width(ui);
                let button = egui::ComboBox::from_id_salt("menu")
                    .width(width)
                    .height(options_menu_height(ui, rows))
                    .selected_text("Row 0")
                    .show_ui(ui, |ui| {
                        configure_options_menu(ui, width);
                        seen.1 = Some(ui.layer_id());
                        for row in 0..rows {
                            let _ = ui.selectable_label(row == 0, format!("Row {row}"));
                        }
                    }).response.rect;
                seen.0 = button;
            });
        }).drop_without_applying_deltas();
        seen
    }

    fn menu_height(ctx: &egui::Context, menu: egui::LayerId) -> f32 {
        egui::AreaState::load(ctx, menu.id).and_then(|state| state.size).map_or(0.0, |size| size.y)
    }

    #[test]
    fn a_list_that_gains_rows_while_open_shows_them_once_it_is_measured_again() {
        let ctx = egui::Context::default();
        let (button, closed) = menu_frame(&ctx, Vec::new(), 1, None);
        assert!(closed.is_none());
        let press = |pressed| egui::Event::PointerButton {
            pos: button.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        menu_frame(&ctx, vec![egui::Event::PointerMoved(button.center()), press(true)], 1, None);
        // The list opens with one row, as the source list does while it is still scanning.
        let menu = menu_frame(&ctx, vec![press(false)], 1, None).1.expect("the list opened");
        menu_frame(&ctx, Vec::new(), 1, None);
        let one_row = menu_height(&ctx, menu);

        // Eight rows arrive. Without a new measurement the list stays too short for them.
        let eight_rows = 8.0 * 24.0;
        for _ in 0..3 {
            assert_eq!(menu_frame(&ctx, Vec::new(), 8, None).1, Some(menu));
        }
        let stuck = menu_height(&ctx, menu);
        assert!(stuck < eight_rows, "the list grew by itself from {one_row} to {stuck}");

        menu_frame(&ctx, Vec::new(), 8, Some(menu));
        assert_eq!(menu_frame(&ctx, Vec::new(), 8, None).1, Some(menu), "the list stays open");
        let measured = menu_height(&ctx, menu);
        assert!(measured >= eight_rows, "the list is {measured} tall after measuring, {stuck} before");

        // A closed list is left alone.
        let ctx = egui::Context::default();
        menu_frame(&ctx, Vec::new(), 1, Some(menu));
        assert!(egui::AreaState::load(&ctx, menu.id).is_none());
    }

    #[test]
    fn the_form_style_stays_inside_the_form() {
        frame(600.0, |ui| {
            let outside = ui.style().text_styles.clone();
            options_list(ui, |ui| {
                assert!(ui.style().text_styles[&egui::TextStyle::Body].size >= 15.0);
                assert!(ui.style().text_styles[&egui::TextStyle::Button].size >= 15.0);
                assert!(ui.style().text_styles[&egui::TextStyle::Small].size >= 13.0);
            });
            assert_eq!(ui.style().text_styles, outside);
        });
    }

    #[test]
    fn a_navigation_item_keeps_its_size_and_reports_a_click() {
        let context = egui::Context::default();
        let mut clicked = 0;
        let mut sizes = Vec::new();
        let press = |pressed| egui::Event::PointerButton {
            pos: Pos2::new(40.0, 18.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [vec![], vec![egui::Event::PointerMoved(Pos2::new(40.0, 18.0)), press(true)],
            vec![press(false)], vec![]]
        {
            context.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(172.0, 200.0))),
                events,
                ..Default::default()
            }, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                for selected in [clicked > 0, false] {
                    let item = options_navigation_item(ui, "Capture", selected, Color32::GREEN);
                    sizes.push(item.rect.size());
                    if selected == (clicked > 0) && item.clicked() && item.rect.top() < 1.0 {
                        clicked += 1;
                    }
                }
            }).drop_without_applying_deltas();
        }
        assert_eq!(clicked, 1);
        assert!(sizes.iter().all(|size| *size == Vec2::new(172.0, OPTIONS_NAVIGATION_ROW)));
    }
}

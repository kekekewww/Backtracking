use std::collections::BTreeMap;

use egui::Color32;

/// The backing of the waveform plot.
pub const PLOT_BACKGROUND: Color32 = Color32::from_rgb(22, 26, 29);
/// The zero line through each waveform row.
pub const CENTER_LINE: Color32 = Color32::from_rgb(47, 60, 64);
/// The bright half of the recording blink.
pub const RECORDING_LIT: Color32 = Color32::from_rgb(249, 78, 88);
/// The dark half of the recording blink.
pub const RECORDING_DIM: Color32 = Color32::from_rgb(103, 44, 48);

pub fn accent_color([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn muted_accent(color: Color32) -> Color32 {
    Color32::from_rgb(color.r() / 3, color.g() / 3, color.b() / 3)
}

/// The colour of the selected part of the waveform: the complement of the accent, or white
/// when no channel of the complement differs from the accent by at least `contrast`.
pub fn selected_waveform_color(accent: Color32, contrast: u8) -> Color32 {
    let [r, g, b, _] = accent.to_array();
    if [r, g, b]
        .into_iter()
        .any(|channel| channel.abs_diff(255 - channel) >= contrast)
    {
        Color32::from_rgb(255 - r, 255 - g, 255 - b)
    } else {
        Color32::WHITE
    }
}

/// How the desktop window's waveform plot looks: its colours and a few widths and radii.
///
/// `Theme::default()` is the window's own look. A look may change only these values, never
/// what the window shows or how it behaves; the capture status colours, text, fonts and sizes
/// are not here on purpose. A token that is `None` is worked out as the window always has,
/// mostly from the user's accent colour, so read every token through its method where it has
/// one: the method takes what the value depends on and returns what to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// The waveform. `None` draws it in the accent colour.
    pub waveform: Option<Color32>,
    /// The selected part of the waveform. `None` takes [`selected_waveform_color`] of the
    /// waveform's colour.
    pub waveform_selected: Option<Color32>,
    /// The backing of the plot.
    pub plot_background: Color32,
    /// The zero line through each row.
    pub center_line: Color32,
    /// The plot's border, where the user has not chosen a border colour. `None` uses the
    /// accent.
    pub border: Option<Color32>,
    /// The line between lanes A and B while rows are edited separately. `None` is the accent
    /// at 70 %.
    pub lane_divider: Option<Color32>,
    /// The guide line at each bar, before the window's transparency fades it.
    pub bar_line: Color32,
    /// The guide line at each interval of seconds, before the window's transparency fades it.
    pub second_line: Color32,
    /// The channel names beside the rows.
    pub row_label: Color32,
    /// The selection's flags and the lines of its fades.
    pub flag: Color32,
    /// The line that follows a playing selection.
    pub playhead: Color32,
    /// The window around the plot. `None` keeps egui's panel colour.
    pub panel_background: Option<Color32>,
    /// The backing of a stacked lane's group switch.
    pub chip_background: Color32,
    /// The names in the group switch, except the current one and the one under the pointer.
    pub chip_text: Color32,
    /// The current group in the switch. `None` is the muted accent.
    pub chip_current: Option<Color32>,
    /// The thumb of a lane's scroll bar. `None` is mid grey, lighter while it is dragged or
    /// hovered; a colour of its own lightens halfway to white then.
    pub scroll_thumb: Option<Color32>,
    /// What lies behind the rows a drag out of the window would take. `None` is the muted
    /// accent.
    pub preview_highlight: Option<Color32>,
    /// The plot border's width in points, while the user shows a border. `None` keeps the
    /// user's width; a width here takes its place while this look is on.
    pub border_width: Option<f32>,
    /// The rounding of the plot's corners in the full window. Waveform-only mode is square.
    pub corner_radius: f32,
    /// How much thicker than usual the waveform's line is drawn.
    pub waveform_stroke_scale: f32,
    /// How strongly the strip between a waveform's outlines takes the waveform's colour.
    pub waveform_fill_opacity: f32,
    /// The waveform colour of the rows of channels with these labels (such as "LFE"), in
    /// place of [`Self::waveform`]. Labels match exactly.
    pub row_colors: BTreeMap<String, Color32>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            waveform: None,
            waveform_selected: None,
            plot_background: PLOT_BACKGROUND,
            center_line: CENTER_LINE,
            border: None,
            lane_divider: None,
            bar_line: Color32::from_white_alpha(104),
            second_line: Color32::from_white_alpha(52),
            row_label: Color32::from_gray(180),
            flag: Color32::from_rgb(255, 201, 109),
            playhead: Color32::from_gray(245),
            panel_background: None,
            chip_background: Color32::from_black_alpha(168),
            chip_text: Color32::from_gray(180),
            chip_current: None,
            scroll_thumb: None,
            preview_highlight: None,
            border_width: None,
            corner_radius: 6.0,
            waveform_stroke_scale: 1.0,
            waveform_fill_opacity: 0.4,
            row_colors: BTreeMap::new(),
        }
    }
}

/// How strongly the accent draws the line between lanes A and B.
const LANE_DIVIDER_ALPHA: f32 = 0.7;

/// `color` as the backing of a window that is `transparency` percent see-through.
fn translucent(color: Color32, transparency: u8) -> Color32 {
    let alpha = ((100u16.saturating_sub(u16::from(transparency))) * 255 + 50) / 100;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha as u8)
}

/// A guide line `color` faded as far as the window is see-through.
fn faded(color: Color32, transparency: u8) -> Color32 {
    let keep = u16::from(100u8.saturating_sub(transparency));
    let [r, g, b, a] = color.to_array().map(|channel| (u16::from(channel) * keep / 100) as u8);
    Color32::from_rgba_premultiplied(r, g, b, a)
}

impl Theme {
    /// The waveform's colour for this accent.
    pub fn waveform(&self, accent: Color32) -> Color32 {
        self.waveform.unwrap_or(accent)
    }

    /// The colour of the selected part of the waveform. `contrast` is what
    /// [`selected_waveform_color`] asks of the complement.
    pub fn waveform_selected(&self, accent: Color32, contrast: u8) -> Color32 {
        self.waveform_selected
            .unwrap_or_else(|| selected_waveform_color(self.waveform(accent), contrast))
    }

    /// The waveform colour of a row of the channel labelled `label`, if this look gives it one.
    pub fn row_color(&self, label: &str) -> Option<Color32> {
        self.row_colors.get(label).copied()
    }

    /// The waveform colour of a row: its channel's, else the waveform's. `label` is the
    /// channel's label, for a row that draws one channel.
    pub fn row_waveform(&self, label: Option<&str>, accent: Color32) -> Color32 {
        label
            .and_then(|label| self.row_color(label))
            .unwrap_or_else(|| self.waveform(accent))
    }

    /// The plot's backing in waveform-only mode, where it carries the window's transparency.
    pub fn translucent_plot_background(&self, transparency: u8) -> Color32 {
        translucent(self.plot_background, transparency)
    }

    /// The plot's border: the user's colour (`preference`) when they chose one, else this
    /// look's, else the accent; made as see-through as the window when the user has the
    /// border follow the background.
    pub fn border(
        &self,
        accent: Color32,
        preference: Option<Color32>,
        follows_background: bool,
        transparency: u8,
    ) -> Color32 {
        let color = preference.or(self.border).unwrap_or(accent);
        if follows_background {
            translucent(color, transparency)
        } else {
            color
        }
    }

    /// The border's width, given the user's choice of width.
    pub fn border_width(&self, preference: u8) -> f32 {
        self.border_width.unwrap_or(f32::from(preference))
    }

    /// The line between lanes A and B while rows are edited separately.
    pub fn lane_divider(&self, accent: Color32) -> Color32 {
        self.lane_divider
            .unwrap_or_else(|| accent.gamma_multiply(LANE_DIVIDER_ALPHA))
    }

    /// The bar guide line in a window `transparency` percent see-through.
    pub fn bar_line(&self, transparency: u8) -> Color32 {
        faded(self.bar_line, transparency)
    }

    /// The seconds guide line in a window `transparency` percent see-through.
    pub fn second_line(&self, transparency: u8) -> Color32 {
        faded(self.second_line, transparency)
    }

    /// The window around the plot. `panel_fill` is egui's panel colour.
    pub fn panel_background(&self, panel_fill: Color32, transparency: u8) -> Color32 {
        translucent(self.panel_background.unwrap_or(panel_fill), transparency)
    }

    /// The current group in a group switch.
    pub fn chip_current(&self, accent: Color32) -> Color32 {
        self.chip_current.unwrap_or_else(|| muted_accent(accent))
    }

    /// A scroll bar's thumb, `active` while it is dragged or under the pointer.
    pub fn scroll_thumb(&self, active: bool) -> Color32 {
        match (self.scroll_thumb, active) {
            (None, active) => Color32::from_gray(if active { 200 } else { 128 }),
            (Some(color), false) => color,
            (Some(color), true) => color.lerp_to_gamma(Color32::WHITE, 0.5),
        }
    }

    /// What lies behind the rows a drag out of the window would take.
    pub fn preview_highlight(&self, accent: Color32) -> Color32 {
        self.preview_highlight.unwrap_or_else(|| muted_accent(accent))
    }

    /// The waveform's line width, where it would otherwise be `stroke`.
    pub fn waveform_stroke(&self, stroke: f32) -> f32 {
        stroke * self.waveform_stroke_scale
    }

    /// The strip between the outlines of a waveform drawn in `color`.
    pub fn waveform_fill(&self, color: Color32) -> Color32 {
        color.linear_multiply(self.waveform_fill_opacity)
    }
}

/// How far apart a capture status mark's colour and what lies behind it must be: the sum of
/// their red, green and blue differences.
pub const MARK_DISTANCE: u32 = 90;
/// The least contrast ratio (WCAG 2, from relative luminance) a look's waveform, selection,
/// row colours, flags and playhead keep against the plot's backing.
pub const GRAPHIC_CONTRAST: f32 = 2.0;
/// The least contrast ratio a look's channel names and group switch names keep against what
/// is behind them, and the window's own text against a look's panel.
pub const TEXT_CONTRAST: f32 = 3.0;
/// The least alpha (of 255) of a look's waveform, selection, row, flag and playhead colours:
/// 40 % opaque.
pub const MIN_ALPHA: u8 = 102;

/// Whether a status mark drawn in `mark` stands out from `behind`.
pub fn distinguishable(mark: Color32, behind: Color32) -> bool {
    let [r, g, b, _] = mark.to_array();
    let [br, bg, bb, _] = behind.to_array();
    [(r, br), (g, bg), (b, bb)]
        .into_iter()
        .map(|(a, b)| u32::from(a.abs_diff(b)))
        .sum::<u32>()
        >= MARK_DISTANCE
}

/// The relative luminance of an opaque colour, as WCAG 2 defines it.
fn luminance(color: Color32) -> f32 {
    let linear = |channel: u8| {
        let value = f32::from(channel) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b, _] = color.to_array();
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// The WCAG 2 contrast ratio of two opaque colours, from 1 (the same) to 21 (black and white).
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `color` drawn over the opaque `behind`.
fn over(color: Color32, behind: Color32) -> Color32 {
    let keep = u16::from(255 - color.a());
    let [r, g, b, _] = color.to_array();
    let [br, bg, bb, _] = behind.to_array();
    let mix = |top: u8, bottom: u8| top.saturating_add(((u16::from(bottom) * keep + 127) / 255) as u8);
    Color32::from_rgb(mix(r, br), mix(g, bg), mix(b, bb))
}

/// `color` with its alpha dropped.
fn opaque(color: Color32) -> Color32 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    Color32::from_rgb(r, g, b)
}

/// Whether some channel of `a` differs from `b` by `contrast`, as [`selected_waveform_color`]
/// asks of the accent's complement.
fn apart(a: Color32, b: Color32, contrast: u8) -> bool {
    let [r, g, bl, _] = a.to_array();
    let [br, bg, bb, _] = b.to_array();
    r.abs_diff(br) >= contrast || g.abs_diff(bg) >= contrast || bl.abs_diff(bb) >= contrast
}

/// What a look must keep readable that is not its own: values from the window that draws it.
#[derive(Clone, Copy, Debug)]
pub struct Legibility<'a> {
    /// The user's accent, which the tokens a look leaves out are made from.
    pub accent: Color32,
    /// What [`selected_waveform_color`] asks of the selection.
    pub selection_contrast: u8,
    /// Every colour of the capture status mark drawn on the plot.
    pub plot_marks: &'a [Color32],
    /// Every colour of the capture status light drawn on the panel beside the plot.
    pub panel_marks: &'a [Color32],
    /// The colour of the window's own text on the panel.
    pub panel_text: Color32,
    /// The panel's colour where a look leaves it alone. In the full window with any
    /// transparency the plot has no backing of its own and sits on the panel.
    pub panel_fill: Color32,
}

/// A token [`Theme::legible`] can put back to the window's own value.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    PlotBackground,
    PanelBackground,
    Waveform,
    WaveformSelected,
    Row(String),
    Flag,
    Playhead,
    RowLabel,
    ChipText,
    ChipBackground,
    ChipCurrent,
    PreviewHighlight,
    LaneDivider,
    ScrollThumb,
}

/// The first token that is set (`true`), which is the one a failed pair blames.
fn blame(candidates: impl IntoIterator<Item = (bool, Token)>) -> Option<Token> {
    candidates
        .into_iter()
        .find_map(|(set, token)| set.then_some(token))
}

impl Theme {
    /// This look with every token that would make the plot or the capture status hard to see
    /// put back to the window's own, and the rest kept. A look's alpha on the plot's and the
    /// panel's backing is dropped, so the user's transparency alone decides how see-through
    /// they are.
    ///
    /// A pair fails when it is short of [`GRAPHIC_CONTRAST`], [`TEXT_CONTRAST`], [`MIN_ALPHA`]
    /// or [`MARK_DISTANCE`], or when the selection is not apart from the waveform as
    /// [`selected_waveform_color`] asks. What the plot draws is checked on both backings it
    /// can sit on: its own and the panel's. A failed pair puts back the token the look set
    /// that is drawn on top (the waveform before a backing), and the pairs are checked again
    /// until every one the look has a part in passes. Pairs made only of the window's own
    /// values, such as the user's accent on the usual backing, are not checked. The centre
    /// line, the guide lines and the border are decoration and are not checked.
    pub fn legible(mut self, needs: &Legibility) -> Self {
        self.plot_background = opaque(self.plot_background);
        self.panel_background = self.panel_background.map(opaque);
        while let Some(token) = self.illegible(needs) {
            self.reset(token);
        }
        self
    }

    /// Whether every pair this look has a part in passes, as [`Self::legible`] leaves it.
    pub fn is_legible(&self, needs: &Legibility) -> bool {
        self.plot_background.a() == 255
            && self.panel_background.is_none_or(|panel| panel.a() == 255)
            && self.illegible(needs).is_none()
    }

    fn reset(&mut self, token: Token) {
        let own = Self::default();
        match token {
            Token::PlotBackground => self.plot_background = own.plot_background,
            Token::PanelBackground => self.panel_background = own.panel_background,
            Token::Waveform => self.waveform = own.waveform,
            Token::WaveformSelected => self.waveform_selected = own.waveform_selected,
            Token::Row(label) => {
                self.row_colors.remove(&label);
            }
            Token::Flag => self.flag = own.flag,
            Token::Playhead => self.playhead = own.playhead,
            Token::RowLabel => self.row_label = own.row_label,
            Token::ChipText => self.chip_text = own.chip_text,
            Token::ChipBackground => self.chip_background = own.chip_background,
            Token::ChipCurrent => self.chip_current = own.chip_current,
            Token::PreviewHighlight => self.preview_highlight = own.preview_highlight,
            Token::LaneDivider => self.lane_divider = own.lane_divider,
            Token::ScrollThumb => self.scroll_thumb = own.scroll_thumb,
        }
    }

    /// The token to put back for the first pair that fails, if any does.
    ///
    /// The plot sits on its own backing, and in the full window with any transparency on the
    /// panel's instead, so what is drawn on it has to read on both. A pair that fails on a
    /// backing blames what the look drew on top first, then that backing.
    fn illegible(&self, needs: &Legibility) -> Option<Token> {
        use Token::*;
        let own = Self::default();
        let plot = self.plot_background;
        let plot_set = plot != own.plot_background;
        let panel = self.panel_background.unwrap_or(needs.panel_fill);
        let panel_set = self.panel_background.is_some();
        let backings = [(plot, plot_set, PlotBackground), (panel, panel_set, PanelBackground)];
        // The first token to blame when `reads` fails on a backing: one of `drawn` that the
        // look set, else that backing if the look set it.
        let on_backings = |reads: &dyn Fn(Color32) -> bool, drawn: &[(bool, Token)]| {
            backings.iter().find_map(|(backing, set, token)| {
                if reads(*backing) {
                    return None;
                }
                blame(drawn.iter().cloned().chain([(*set, token.clone())]))
            })
        };
        let graphic = |color: Color32, backing: Color32| {
            color.a() >= MIN_ALPHA
                && contrast_ratio(over(color, backing), backing) >= GRAPHIC_CONTRAST
        };
        let text = |color: Color32, behind: Color32| {
            contrast_ratio(over(color, behind), behind) >= TEXT_CONTRAST
        };
        let distinct = |a: Color32, b: Color32, backing: Color32| {
            apart(over(a, backing), over(b, backing), needs.selection_contrast)
        };
        let marks_lost = |behind: Color32, marks: &[Color32]| {
            marks.iter().any(|&mark| !distinguishable(mark, behind))
        };

        // The capture status mark is drawn on the plot's own backing, the status light and
        // the window's text on the panel.
        if plot_set && marks_lost(plot, needs.plot_marks) {
            return Some(PlotBackground);
        }
        if panel_set && (marks_lost(panel, needs.panel_marks) || !text(needs.panel_text, panel)) {
            return Some(PanelBackground);
        }

        let waveform = self.waveform(needs.accent);
        let waveform_set = self.waveform.is_some();
        if let Some(token) = on_backings(&|b| graphic(waveform, b), &[(waveform_set, Waveform)]) {
            return Some(token);
        }
        let selected = self.waveform_selected(needs.accent, needs.selection_contrast);
        if let Some(token) = on_backings(
            &|b| graphic(selected, b) && distinct(selected, waveform, b),
            &[(self.waveform_selected.is_some(), WaveformSelected), (waveform_set, Waveform)],
        ) {
            return Some(token);
        }
        for (label, &color) in &self.row_colors {
            let reads = |b| graphic(color, b) && distinct(color, selected, b);
            if backings.iter().any(|(backing, ..)| !reads(*backing)) {
                return Some(Row(label.clone()));
            }
        }
        for (color, own_color, token) in [
            (self.flag, own.flag, Flag),
            (self.playhead, own.playhead, Playhead),
        ] {
            if let Some(token) = on_backings(&|b| graphic(color, b), &[(color != own_color, token)]) {
                return Some(token);
            }
        }
        if let Some(token) = on_backings(
            &|b| text(self.row_label, b),
            &[(self.row_label != own.row_label, RowLabel)],
        ) {
            return Some(token);
        }

        // The group switch: its names on its backing, and white for the current group and
        // the one under the pointer.
        let chip_background_set = self.chip_background != own.chip_background;
        let chip_current_set = self.chip_current.is_some();
        let chip_current = self.chip_current(needs.accent);
        if let Some(token) = on_backings(
            &|b| text(self.chip_text, over(self.chip_background, b)),
            &[
                (self.chip_text != own.chip_text, ChipText),
                (chip_background_set, ChipBackground),
            ],
        ) {
            return Some(token);
        }
        for (backing, ..) in &backings {
            let switch = over(self.chip_background, *backing);
            let current = over(chip_current, switch);
            if !text(Color32::WHITE, switch)
                && let Some(token) = blame([(chip_background_set, ChipBackground)])
            {
                return Some(token);
            }
            if !(text(Color32::WHITE, current)
                && apart(current, switch, needs.selection_contrast))
                && let Some(token) = blame([
                    (chip_current_set, ChipCurrent),
                    (chip_background_set, ChipBackground),
                ])
            {
                return Some(token);
            }
        }

        // Highlights and lines a look may colour, judged only when it does.
        if let Some(highlight) = self.preview_highlight
            && (highlight.a() < MIN_ALPHA
                || backings
                    .iter()
                    .any(|(backing, ..)| !distinct(highlight, waveform, *backing)))
        {
            return Some(PreviewHighlight);
        }
        if let Some(divider) = self.lane_divider
            && backings.iter().any(|(backing, ..)| !graphic(divider, *backing))
        {
            return Some(LaneDivider);
        }
        if self.scroll_thumb.is_some()
            && backings.iter().any(|(backing, ..)| {
                !(graphic(self.scroll_thumb(false), *backing)
                    && graphic(self.scroll_thumb(true), *backing))
            })
        {
            return Some(ScrollThumb);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muted_accent_is_a_third_of_each_channel() {
        for (accent, muted) in [
            ([103, 231, 181], [34, 77, 60]),
            ([128, 128, 128], [42, 42, 42]),
            ([0, 0, 0], [0, 0, 0]),
            ([255, 255, 255], [85, 85, 85]),
        ] {
            assert_eq!(muted_accent(accent_color(accent)), accent_color(muted));
        }
    }

    #[test]
    fn selection_takes_the_complement_unless_it_is_too_close_to_the_accent() {
        let accent = accent_color([103, 231, 181]);
        assert_eq!(selected_waveform_color(accent, 80), accent_color([152, 24, 74]));
        assert_eq!(selected_waveform_color(accent, 64), accent_color([152, 24, 74]));

        // Mid grey is its own complement, so the selection would vanish without the fallback.
        let grey = accent_color([128, 128, 128]);
        assert_eq!(selected_waveform_color(grey, 80), Color32::WHITE);
        assert_eq!(selected_waveform_color(grey, 64), Color32::WHITE);

        // The largest channel difference here is 75: enough for one threshold, not the other.
        let muted = accent_color([100, 150, 165]);
        assert_eq!(selected_waveform_color(muted, 80), Color32::WHITE);
        assert_eq!(selected_waveform_color(muted, 64), accent_color([155, 105, 90]));
    }

    /// What the desktop window drew before its look was gathered here, written the way its
    /// drawing code wrote each value.
    #[allow(clippy::too_many_arguments)]
    fn old_values(
        accent: Color32,
        border_preference: Option<Color32>,
        border_follows_background: bool,
        border_width_preference: u8,
        transparency: u8,
        panel_fill: Color32,
        active: bool,
        stroke: f32,
    ) -> Vec<(&'static str, String)> {
        let background = |color: Color32| {
            let alpha = ((100 - u16::from(transparency)) * 255 + 50) / 100;
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha as u8)
        };
        let guide = |alpha: u16| {
            Color32::from_white_alpha((alpha * u16::from(100 - transparency) / 100) as u8)
        };
        let border = border_preference.unwrap_or(accent);
        let border = if border_follows_background { background(border) } else { border };
        vec![
            ("waveform", format!("{accent:?}")),
            ("waveform_selected", format!("{:?}", selected_waveform_color(accent, 80))),
            ("plot_background", format!("{:?}", Color32::from_rgb(22, 26, 29))),
            ("plot_background_translucent", format!("{:?}", background(Color32::from_rgb(22, 26, 29)))),
            ("center_line", format!("{:?}", Color32::from_rgb(47, 60, 64))),
            ("border", format!("{border:?}")),
            ("lane_divider", format!("{:?}", accent.gamma_multiply(0.7))),
            ("bar_line", format!("{:?}", guide(104))),
            ("second_line", format!("{:?}", guide(52))),
            ("row_label", format!("{:?}", Color32::from_gray(180))),
            ("flag", format!("{:?}", Color32::from_rgb(255, 201, 109))),
            ("playhead", format!("{:?}", Color32::from_gray(245))),
            ("panel_background", format!("{:?}", background(panel_fill))),
            ("chip_background", format!("{:?}", Color32::from_black_alpha(168))),
            ("chip_text", format!("{:?}", Color32::from_gray(180))),
            ("chip_current", format!("{:?}", muted_accent(accent))),
            ("scroll_thumb", format!("{:?}", Color32::from_gray(if active { 200 } else { 128 }))),
            ("preview_highlight", format!("{:?}", muted_accent(accent))),
            ("border_width", format!("{:?}", f32::from(border_width_preference))),
            ("corner_radius", format!("{:?}", 6.0f32)),
            ("waveform_stroke", format!("{stroke:?}")),
            ("waveform_fill", format!("{:?}", accent.linear_multiply(0.4))),
            ("row_waveform", format!("{accent:?}")),
        ]
    }

    /// Every token `theme` resolves to for these inputs, named as in [`old_values`].
    #[allow(clippy::too_many_arguments)]
    fn resolved(
        theme: &Theme,
        accent: Color32,
        border_preference: Option<Color32>,
        border_follows_background: bool,
        border_width_preference: u8,
        transparency: u8,
        panel_fill: Color32,
        active: bool,
        stroke: f32,
    ) -> Vec<(&'static str, String)> {
        let waveform = theme.waveform(accent);
        vec![
            ("waveform", format!("{waveform:?}")),
            ("waveform_selected", format!("{:?}", theme.waveform_selected(accent, 80))),
            ("plot_background", format!("{:?}", theme.plot_background)),
            ("plot_background_translucent", format!("{:?}", theme.translucent_plot_background(transparency))),
            ("center_line", format!("{:?}", theme.center_line)),
            ("border", format!("{:?}", theme.border(
                accent, border_preference, border_follows_background, transparency,
            ))),
            ("lane_divider", format!("{:?}", theme.lane_divider(accent))),
            ("bar_line", format!("{:?}", theme.bar_line(transparency))),
            ("second_line", format!("{:?}", theme.second_line(transparency))),
            ("row_label", format!("{:?}", theme.row_label)),
            ("flag", format!("{:?}", theme.flag)),
            ("playhead", format!("{:?}", theme.playhead)),
            ("panel_background", format!("{:?}", theme.panel_background(panel_fill, transparency))),
            ("chip_background", format!("{:?}", theme.chip_background)),
            ("chip_text", format!("{:?}", theme.chip_text)),
            ("chip_current", format!("{:?}", theme.chip_current(accent))),
            ("scroll_thumb", format!("{:?}", theme.scroll_thumb(active))),
            ("preview_highlight", format!("{:?}", theme.preview_highlight(accent))),
            ("border_width", format!("{:?}", theme.border_width(border_width_preference))),
            ("corner_radius", format!("{:?}", theme.corner_radius)),
            ("waveform_stroke", format!("{:?}", theme.waveform_stroke(stroke))),
            ("waveform_fill", format!("{:?}", theme.waveform_fill(waveform))),
            ("row_waveform", format!("{:?}", theme.row_waveform(Some("LFE"), accent))),
        ]
    }

    const PANEL_FILL: Color32 = Color32::from_gray(27);

    #[test]
    fn the_default_theme_resolves_to_what_the_window_drew_before() {
        for accent in [[103, 231, 181], [128, 128, 128], [249, 78, 88], [0, 0, 0]].map(accent_color) {
            for border in [None, Some(accent_color([10, 120, 240]))] {
                for follows in [false, true] {
                    for transparency in [0, 25, 50, 100] {
                        for (width, active, stroke) in [(1, false, 1.5), (3, true, 0.75)] {
                            assert_eq!(
                                resolved(&Theme::default(), accent, border, follows, width,
                                    transparency, PANEL_FILL, active, stroke),
                                old_values(accent, border, follows, width, transparency,
                                    PANEL_FILL, active, stroke),
                                "accent {accent:?}, border {border:?}, follows {follows}, \
                                 transparency {transparency}",
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn setting_a_token_changes_what_that_token_resolves_to_and_nothing_else() {
        let wild = Color32::from_rgb(1, 2, 3);
        let cases: Vec<(&str, fn(&mut Theme))> = vec![
            ("waveform_selected", |theme| theme.waveform_selected = Some(Color32::from_rgb(1, 2, 3))),
            ("center_line", |theme| theme.center_line = Color32::from_rgb(1, 2, 3)),
            ("border", |theme| theme.border = Some(Color32::from_rgb(1, 2, 3))),
            ("lane_divider", |theme| theme.lane_divider = Some(Color32::from_rgb(1, 2, 3))),
            ("bar_line", |theme| theme.bar_line = Color32::from_rgb(1, 2, 3)),
            ("second_line", |theme| theme.second_line = Color32::from_rgb(1, 2, 3)),
            ("row_label", |theme| theme.row_label = Color32::from_rgb(1, 2, 3)),
            ("flag", |theme| theme.flag = Color32::from_rgb(1, 2, 3)),
            ("playhead", |theme| theme.playhead = Color32::from_rgb(1, 2, 3)),
            ("panel_background", |theme| theme.panel_background = Some(Color32::from_rgb(1, 2, 3))),
            ("chip_background", |theme| theme.chip_background = Color32::from_rgb(1, 2, 3)),
            ("chip_text", |theme| theme.chip_text = Color32::from_rgb(1, 2, 3)),
            ("chip_current", |theme| theme.chip_current = Some(Color32::from_rgb(1, 2, 3))),
            ("scroll_thumb", |theme| theme.scroll_thumb = Some(Color32::from_rgb(1, 2, 3))),
            ("preview_highlight", |theme| theme.preview_highlight = Some(Color32::from_rgb(1, 2, 3))),
            ("border_width", |theme| theme.border_width = Some(9.0)),
            ("corner_radius", |theme| theme.corner_radius = 1.0),
            ("waveform_stroke", |theme| theme.waveform_stroke_scale = 2.0),
            ("waveform_fill", |theme| theme.waveform_fill_opacity = 0.9),
            ("row_waveform", |theme| {
                theme.row_colors.insert("LFE".into(), Color32::from_rgb(1, 2, 3));
            }),
        ];
        let accent = accent_color([103, 231, 181]);
        let all = |theme: &Theme| resolved(theme, accent, None, false, 1, 50, PANEL_FILL, false, 1.5);
        let before = all(&Theme::default());
        for (token, change) in cases {
            let mut theme = Theme::default();
            change(&mut theme);
            let changed: Vec<_> = before
                .iter()
                .zip(all(&theme))
                .filter(|(old, new)| **old != *new)
                .map(|(old, _)| old.0)
                .collect();
            assert_eq!(changed, [token], "setting {token}");
        }

        // The plot's backing is drawn opaque and translucent; both follow the one token.
        let mut theme = Theme::default();
        theme.plot_background = wild;
        assert_eq!(theme.translucent_plot_background(0), wild);
        assert_eq!(theme.translucent_plot_background(50), Color32::from_rgba_unmultiplied(1, 2, 3, 128));

        // The waveform's own colour also carries its fill and, unless set, the selected colour
        // that contrasts with it.
        let mut theme = Theme::default();
        theme.waveform = Some(wild);
        assert_eq!(theme.waveform(accent), wild);
        assert_eq!(theme.row_waveform(None, accent), wild);
        assert_eq!(theme.waveform_selected(accent, 80), selected_waveform_color(wild, 80));
        assert_eq!(theme.lane_divider(accent), accent.gamma_multiply(0.7), "the divider follows the accent");
    }

    #[test]
    fn the_users_border_colour_wins_over_the_themes_and_the_themes_over_the_accent() {
        let accent = accent_color([103, 231, 181]);
        let chosen = accent_color([10, 120, 240]);
        let mut theme = Theme::default();
        theme.border = Some(Color32::from_rgb(1, 2, 3));
        assert_eq!(theme.border(accent, Some(chosen), false, 0), chosen);
        assert_eq!(theme.border(accent, None, false, 0), Color32::from_rgb(1, 2, 3));
        assert_eq!(theme.border(accent, None, true, 50), Color32::from_rgba_unmultiplied(1, 2, 3, 128));
        // A look's width takes the place of the user's while that look is on.
        theme.border_width = Some(2.5);
        assert_eq!(theme.border_width(1), 2.5);
    }

    #[test]
    fn a_row_takes_its_channels_colour_and_otherwise_the_waveforms() {
        let accent = accent_color([103, 231, 181]);
        let lfe = Color32::from_rgb(200, 40, 40);
        let mut theme = Theme::default();
        assert_eq!(theme.row_color("LFE"), None);
        assert_eq!(theme.row_waveform(Some("LFE"), accent), accent);

        theme.row_colors.insert("LFE".into(), lfe);
        assert_eq!(theme.row_color("LFE"), Some(lfe));
        assert_eq!(theme.row_color("lfe"), None, "labels are matched exactly");
        assert_eq!(theme.row_color("C"), None);
        assert_eq!(theme.row_waveform(Some("LFE"), accent), lfe);
        assert_eq!(theme.row_waveform(Some("C"), accent), accent);
        assert_eq!(theme.row_waveform(None, accent), accent);

        theme.waveform = Some(Color32::from_rgb(1, 2, 3));
        assert_eq!(theme.row_waveform(Some("C"), accent), Color32::from_rgb(1, 2, 3));
        assert_eq!(theme.row_waveform(Some("LFE"), accent), lfe);
    }

    const ACCENT: Color32 = Color32::from_rgb(103, 231, 181);
    /// Status colours like the desktop window's: bright marks on the plot, dark lights on the
    /// panel.
    const PLOT_MARKS: [Color32; 4] = [
        Color32::from_rgb(246, 196, 118),
        Color32::from_gray(214),
        Color32::from_gray(132),
        RECORDING_LIT,
    ];
    const PANEL_MARKS: [Color32; 2] = [Color32::from_rgb(54, 57, 60), Color32::from_rgb(76, 86, 88)];

    fn needs(accent: Color32) -> Legibility<'static> {
        Legibility {
            accent,
            selection_contrast: 80,
            plot_marks: &PLOT_MARKS,
            panel_marks: &PANEL_MARKS,
            panel_text: Color32::from_gray(140),
            panel_fill: PANEL_FILL,
        }
    }

    /// Both backings the plot can sit on: its own, and the panel in the full window with any
    /// transparency.
    fn backings(theme: &Theme) -> [Color32; 2] {
        [theme.plot_background, theme.panel_background.unwrap_or(PANEL_FILL)]
    }

    /// Whether `color` is drawn visibly and with graphic contrast on every backing.
    fn reads_on_both(theme: &Theme, color: Color32) -> bool {
        color.a() >= MIN_ALPHA
            && backings(theme)
                .into_iter()
                .all(|backing| contrast_ratio(over(color, backing), backing) >= GRAPHIC_CONTRAST)
    }

    #[test]
    fn a_white_panel_cannot_hide_what_the_plot_draws_on_it() {
        let theme = Theme {
            panel_background: Some(Color32::WHITE),
            ..Theme::default()
        };
        // The window's own playhead on white is about 1.09 : 1.
        assert!(!theme.is_legible(&needs(ACCENT)));
        let theme = theme.legible(&needs(ACCENT));
        assert!(theme.is_legible(&needs(ACCENT)));
        assert_eq!(theme.panel_background, None);
        for drawn in [theme.playhead, theme.flag, theme.waveform(ACCENT)] {
            assert!(reads_on_both(&theme, drawn), "{drawn:?}");
        }
        // A panel that keeps everything readable stays, and so do the lines that read on it.
        let dark = Theme {
            panel_background: Some(Color32::from_rgb(10, 14, 40)),
            playhead: Color32::from_rgb(250, 250, 120),
            ..Theme::default()
        };
        assert_eq!(dark.clone().legible(&needs(ACCENT)), dark);
    }

    #[test]
    fn a_light_plot_with_dark_lines_reads_on_the_dark_panel_too() {
        let theme = Theme {
            plot_background: Color32::WHITE,
            waveform: Some(Color32::from_rgb(20, 20, 20)),
            waveform_selected: Some(Color32::from_rgb(200, 0, 0)),
            flag: Color32::from_rgb(40, 0, 0),
            playhead: Color32::BLACK,
            row_label: Color32::BLACK,
            ..Theme::default()
        };
        // Each reads on white, which is the plot's own backing.
        assert!(contrast_ratio(Color32::from_rgb(20, 20, 20), Color32::WHITE) > 10.0);
        let theme = theme.legible(&needs(ACCENT));
        assert!(theme.is_legible(&needs(ACCENT)), "{theme:?}");
        for drawn in [
            theme.waveform(ACCENT),
            theme.waveform_selected(ACCENT, 80),
            theme.flag,
            theme.playhead,
        ] {
            assert!(reads_on_both(&theme, drawn), "{drawn:?}");
        }
        for backing in backings(&theme) {
            assert!(contrast_ratio(over(theme.row_label, backing), backing) >= TEXT_CONTRAST);
        }
    }

    #[test]
    fn a_drag_highlight_must_be_visible_and_apart_from_the_waveform() {
        for highlight in [
            Color32::from_rgba_unmultiplied(200, 60, 60, 40),
            Color32::from_rgb(100, 228, 178),
        ] {
            let theme = Theme {
                preview_highlight: Some(highlight),
                ..Theme::default()
            };
            assert_eq!(theme.legible(&needs(ACCENT)).preview_highlight, None, "{highlight:?}");
        }
        let kept = Theme {
            preview_highlight: Some(Color32::from_rgb(60, 30, 90)),
            ..Theme::default()
        };
        assert_eq!(kept.clone().legible(&needs(ACCENT)), kept);
    }

    #[test]
    fn the_group_switchs_white_names_and_current_group_stay_readable() {
        // White on a light current group.
        let theme = Theme {
            chip_current: Some(Color32::from_gray(230)),
            ..Theme::default()
        };
        assert_eq!(theme.legible(&needs(ACCENT)).chip_current, None);
        // White on a light switch, where the pointer is.
        let theme = Theme {
            chip_background: Color32::from_gray(235),
            chip_text: Color32::BLACK,
            ..Theme::default()
        };
        let theme = theme.legible(&needs(ACCENT));
        assert_eq!(theme.chip_background, Theme::default().chip_background);
        // The current group lost on the switch.
        let theme = Theme {
            chip_current: Some(Color32::from_rgb(4, 6, 8)),
            ..Theme::default()
        };
        assert_eq!(theme.legible(&needs(ACCENT)).chip_current, None);
        let kept = Theme {
            chip_current: Some(Color32::from_rgb(20, 60, 160)),
            ..Theme::default()
        };
        assert_eq!(kept.clone().legible(&needs(ACCENT)), kept);
    }

    #[test]
    fn the_lane_divider_and_scroll_thumb_must_read_on_both_backings() {
        for color in [
            Color32::from_rgba_unmultiplied(255, 255, 255, 40),
            PLOT_BACKGROUND,
            PANEL_FILL,
        ] {
            let theme = Theme {
                lane_divider: Some(color),
                scroll_thumb: Some(color),
                ..Theme::default()
            }
            .legible(&needs(ACCENT));
            assert_eq!((theme.lane_divider, theme.scroll_thumb), (None, None), "{color:?}");
        }
        let kept = Theme {
            lane_divider: Some(Color32::from_rgb(200, 120, 255)),
            scroll_thumb: Some(Color32::from_gray(150)),
            ..Theme::default()
        };
        assert_eq!(kept.clone().legible(&needs(ACCENT)), kept);
    }

    /// A dark blue backing that keeps every plot mark apart, with the accent and its
    /// complement readable on it.
    const BACKING: Color32 = Color32::from_rgb(10, 20, 50);
    const YELLOW: Color32 = Color32::from_rgb(240, 200, 40);

    #[test]
    fn contrast_is_measured_as_wcag_does() {
        assert!((contrast_ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 0.01);
        assert_eq!(contrast_ratio(BACKING, BACKING), 1.0);
        // Mid grey on black is about 5.3 : 1.
        let grey = contrast_ratio(Color32::from_gray(128), Color32::BLACK);
        assert!((5.2..5.4).contains(&grey), "{grey}");
        assert!(distinguishable(Color32::from_gray(214), PLOT_BACKGROUND));
        assert!(!distinguishable(Color32::from_rgb(54, 57, 60), Color32::from_rgb(30, 30, 30)));
        assert!(distinguishable(Color32::from_rgb(54, 57, 60), Color32::from_gray(27)), "exactly 90 apart");
    }

    #[test]
    fn the_windows_own_look_is_left_as_it_is() {
        for accent in [ACCENT, accent_color([128, 128, 128]), PLOT_BACKGROUND, Color32::BLACK] {
            assert_eq!(Theme::default().legible(&needs(accent)), Theme::default());
            assert!(Theme::default().is_legible(&needs(accent)), "{accent:?}");
        }
    }

    #[test]
    fn a_waveform_lost_on_the_backing_is_dropped_and_the_backing_kept() {
        let theme = Theme {
            plot_background: BACKING,
            waveform: Some(Color32::from_rgb(14, 28, 60)),
            flag: Color32::from_rgb(255, 255, 0),
            ..Theme::default()
        };
        assert!(!theme.is_legible(&needs(ACCENT)));
        let kept = theme.legible(&needs(ACCENT));
        assert_eq!(kept.waveform, None, "the accent draws it again");
        assert_eq!(kept.plot_background, BACKING);
        assert_eq!(kept.flag, Color32::from_rgb(255, 255, 0), "the rest stays");
        assert!(kept.is_legible(&needs(ACCENT)));
    }

    #[test]
    fn a_backing_the_accent_is_lost_on_is_dropped_when_the_look_leaves_the_waveform_alone() {
        let theme = Theme {
            plot_background: Color32::from_rgb(100, 225, 180),
            ..Theme::default()
        };
        assert_eq!(theme.legible(&needs(ACCENT)), Theme::default());
    }

    #[test]
    fn the_selection_must_read_on_the_backing_and_apart_from_the_waveform() {
        let waveform = YELLOW;
        // On the backing: drop the selection's colour, keep the waveform's, whose complement
        // reads there.
        let theme = Theme {
            plot_background: BACKING,
            waveform: Some(waveform),
            waveform_selected: Some(BACKING),
            ..Theme::default()
        }
        .legible(&needs(ACCENT));
        assert_eq!((theme.waveform, theme.waveform_selected), (Some(waveform), None));
        // Too close to the waveform: no channel is 80 apart.
        let theme = Theme {
            waveform: Some(waveform),
            waveform_selected: Some(Color32::from_rgb(200, 160, 60)),
            ..Theme::default()
        }
        .legible(&needs(ACCENT));
        assert_eq!((theme.waveform, theme.waveform_selected), (Some(waveform), None));
        assert!(theme.is_legible(&needs(ACCENT)));
        // A selection worked out from a look's waveform that the backing then hides: the
        // waveform goes, and the accent's own selection comes back.
        let theme = Theme {
            plot_background: Color32::from_gray(250),
            waveform: Some(Color32::from_rgb(10, 10, 10)),
            ..Theme::default()
        }
        .legible(&needs(Color32::from_rgb(10, 60, 30)));
        assert!(theme.is_legible(&needs(Color32::from_rgb(10, 60, 30))));
        assert_eq!(theme.waveform_selected, None);
        let apart_enough = Theme {
            waveform: Some(waveform),
            waveform_selected: Some(Color32::from_rgb(240, 110, 40)),
            ..Theme::default()
        };
        assert_eq!(apart_enough.clone().legible(&needs(ACCENT)), apart_enough, "90 apart in green");
        // A white waveform's own selection would be black, lost on the usual backing.
        let white = Theme {
            waveform: Some(Color32::WHITE),
            ..Theme::default()
        };
        assert_eq!(white.legible(&needs(ACCENT)), Theme::default());
    }

    #[test]
    fn a_row_colour_lost_on_the_backing_or_on_the_selection_is_dropped_alone() {
        let selected = Color32::from_rgb(255, 255, 0);
        let mut theme = Theme {
            waveform_selected: Some(selected),
            ..Theme::default()
        };
        theme.row_colors.insert("LFE".into(), Color32::from_rgb(200, 40, 40));
        theme.row_colors.insert("C".into(), PLOT_BACKGROUND);
        theme.row_colors.insert("L".into(), Color32::from_rgb(250, 240, 20));
        let theme = theme.legible(&needs(ACCENT));
        assert_eq!(
            theme.row_colors,
            [("LFE".to_owned(), Color32::from_rgb(200, 40, 40))].into()
        );
        assert_eq!(theme.waveform_selected, Some(selected));
    }

    #[test]
    fn flags_and_the_playhead_must_read_on_the_backing() {
        let theme = Theme {
            flag: Color32::from_rgb(30, 34, 38),
            playhead: Color32::from_rgb(25, 25, 25),
            ..Theme::default()
        }
        .legible(&needs(ACCENT));
        assert_eq!((theme.flag, theme.playhead), (Theme::default().flag, Theme::default().playhead));
        // The window's own flag on a look's backing it is lost on: the backing goes.
        let theme = Theme {
            plot_background: Color32::from_rgb(250, 200, 110),
            waveform: Some(Color32::BLACK),
            ..Theme::default()
        }
        .legible(&needs(ACCENT));
        assert_eq!(theme.plot_background, PLOT_BACKGROUND);
    }

    #[test]
    fn colours_too_transparent_to_see_are_dropped() {
        let faint = |alpha| Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
        let theme = Theme {
            waveform: Some(faint(90)),
            flag: faint(90),
            playhead: faint(20),
            ..Theme::default()
        };
        // White at 90 of 255 still contrasts with the dark backing; it is too faint all the same.
        assert!(contrast_ratio(over(faint(90), PLOT_BACKGROUND), PLOT_BACKGROUND) >= GRAPHIC_CONTRAST);
        let theme = theme.legible(&needs(ACCENT));
        assert_eq!(theme, Theme::default());
        let visible = Theme {
            waveform: Some(faint(MIN_ALPHA)),
            ..Theme::default()
        };
        assert_eq!(visible.clone().legible(&needs(ACCENT)), visible);
    }

    #[test]
    fn channel_names_and_group_names_keep_text_contrast() {
        let theme = Theme {
            row_label: Color32::from_gray(60),
            chip_text: Color32::from_gray(40),
            ..Theme::default()
        }
        .legible(&needs(ACCENT));
        assert_eq!(theme.row_label, Theme::default().row_label);
        assert_eq!(theme.chip_text, Theme::default().chip_text);
        // A group switch backing that hides the window's own names goes; one that keeps a
        // look's names readable stays. A light switch never does: the current group's name
        // and the one under the pointer are white.
        let theme = Theme {
            chip_background: Color32::from_gray(180),
            ..Theme::default()
        }
        .legible(&needs(ACCENT));
        assert_eq!(theme.chip_background, Theme::default().chip_background);
        let readable = Theme {
            // Dark, and apart from the current group's muted accent.
            chip_background: Color32::from_rgb(130, 10, 30),
            chip_text: Color32::from_gray(210),
            ..Theme::default()
        };
        assert_eq!(readable.clone().legible(&needs(ACCENT)), readable);
    }

    #[test]
    fn a_backing_that_hides_a_status_mark_or_the_windows_text_is_dropped() {
        // Close to the waiting mark's grey.
        let theme = Theme {
            plot_background: Color32::from_gray(120),
            waveform: Some(YELLOW),
            ..Theme::default()
        }
        .legible(&needs(ACCENT));
        assert_eq!(theme.plot_background, PLOT_BACKGROUND);
        assert_eq!(theme.waveform, Some(YELLOW));
        // Close to the paused light, and to the window's grey text.
        for panel in [Color32::from_rgb(50, 55, 60), Color32::from_gray(140)] {
            let theme = Theme {
                panel_background: Some(panel),
                ..Theme::default()
            };
            assert_eq!(theme.legible(&needs(ACCENT)).panel_background, None, "{panel:?}");
        }
        let dark_blue = Theme {
            panel_background: Some(Color32::from_rgb(10, 14, 40)),
            ..Theme::default()
        };
        assert_eq!(dark_blue.clone().legible(&needs(ACCENT)), dark_blue);
    }

    #[test]
    fn the_users_transparency_still_decides_how_see_through_the_backing_is() {
        let theme = Theme {
            plot_background: Color32::from_rgba_unmultiplied(20, 60, 120, 64),
            panel_background: Some(Color32::from_rgba_unmultiplied(10, 14, 40, 0)),
            ..Theme::default()
        };
        assert!(!theme.is_legible(&needs(ACCENT)));
        let theme = theme.legible(&needs(ACCENT));
        assert_eq!(theme.plot_background.a(), 255);
        assert_eq!(theme.panel_background.map(|panel| panel.a()), Some(255));
        assert_eq!(theme.translucent_plot_background(0).a(), 255);
        assert_eq!(theme.translucent_plot_background(50).a(), 128);
        assert_eq!(theme.translucent_plot_background(100).a(), 0);
    }

    /// Every colour token and row set to `color`.
    fn all_one_colour(color: Color32) -> Theme {
        Theme {
            waveform: Some(color),
            waveform_selected: Some(color),
            plot_background: color,
            center_line: color,
            border: Some(color),
            lane_divider: Some(color),
            bar_line: color,
            second_line: color,
            row_label: color,
            flag: color,
            playhead: color,
            panel_background: Some(color),
            chip_background: color,
            chip_text: color,
            chip_current: Some(color),
            scroll_thumb: Some(color),
            preview_highlight: Some(color),
            row_colors: [("LFE".to_owned(), color), ("C".to_owned(), color)].into(),
            ..Theme::default()
        }
    }

    #[test]
    fn a_wild_look_all_in_the_backings_colour_ends_up_legible() {
        for color in [
            PLOT_BACKGROUND,
            Color32::BLACK,
            Color32::WHITE,
            Color32::from_gray(128),
            Color32::from_rgb(255, 0, 255),
            ACCENT,
            Color32::from_gray(214),
            Color32::from_rgba_unmultiplied(40, 80, 160, 30),
        ] {
            for accent in [ACCENT, color, Color32::from_rgb(249, 78, 88)] {
                let needs = needs(accent);
                let theme = all_one_colour(color).legible(&needs);
                assert!(theme.is_legible(&needs), "{color:?} with accent {accent:?}: {theme:?}");
                // Spelled out: what the look still sets stands out from both backings the plot
                // can sit on, and so does all that is drawn on a backing of the look's.
                let own = Theme::default();
                for (backing, backing_set) in [
                    (theme.plot_background, theme.plot_background != own.plot_background),
                    (theme.panel_background.unwrap_or(PANEL_FILL), theme.panel_background.is_some()),
                ] {
                    let drawn = [
                        (theme.waveform.is_some(), theme.waveform(accent)),
                        (theme.waveform_selected.is_some(), theme.waveform_selected(accent, 80)),
                        (theme.flag != own.flag, theme.flag),
                        (theme.playhead != own.playhead, theme.playhead),
                    ]
                    .into_iter()
                    .chain(theme.row_colors.values().map(|&row| (true, row)))
                    .chain(theme.lane_divider.map(|divider| (true, divider)))
                    .chain(theme.scroll_thumb.map(|thumb| (true, thumb)));
                    for (set, drawn) in drawn {
                        if set || backing_set {
                            assert!(drawn.a() >= MIN_ALPHA);
                            assert!(
                                contrast_ratio(over(drawn, backing), backing) >= GRAPHIC_CONTRAST,
                                "{drawn:?} on {backing:?}"
                            );
                        }
                    }
                    if theme.row_label != own.row_label || backing_set {
                        assert!(contrast_ratio(over(theme.row_label, backing), backing) >= TEXT_CONTRAST);
                    }
                }
                let backing = theme.plot_background;
                for mark in PLOT_MARKS {
                    assert!(distinguishable(mark, backing) || backing == PLOT_BACKGROUND);
                }
                // Lines and highlights the guard does not judge keep the look's colour.
                assert_eq!(theme.center_line, color);
            }
        }
    }
}

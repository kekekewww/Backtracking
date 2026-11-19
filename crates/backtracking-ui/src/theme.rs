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
}

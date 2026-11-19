//! The look an add-on offers (its `appearance`), once the user has chosen that add-on on the
//! Appearance page of Options.
//!
//! An offer becomes a [`Theme`] by [`theme_from`], which sets only the tokens the offer names,
//! and then passes [`Theme::legible`], which puts back whatever would make the waveform, the
//! selection, the names or the capture status hard to see. A look changes how the window
//! looks and nothing else: the capture status colours, text, sizes and hit areas are not
//! tokens, and the border's width is the only one that moves anything.
//!
//! The choice is the `addon-look` preference. A capture window that is not the first reads it
//! when it starts and keeps changes made in its own Options for as long as it runs, as it does
//! the accent; the offers themselves reach it through the add-on bridge.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use backtracking_addon::protocol::{
    APPEARANCE_COLOR_NAMES, APPEARANCE_NUMBER_RANGES, Appearance, MAX_APPEARANCE_ROWS,
};
use backtracking_ui::{Legibility, Theme, options_choice, options_field, options_note};
use eframe::egui::{self, Color32};

use super::{CaptureIndicator, Recorder, SELECTION_CONTRAST, text::Language};
use crate::addons::{AddonAppearances, AddonHandle, AddonSnapshot};
use crate::preferences::Key;

/// Writes the setter of every colour and number token, each named by its `Theme` field, so a
/// token's name is never written twice. The kinds say how a value is stored: `some` in an
/// `Option`, `opaque` with the colour's alpha dropped (the user's transparency decides how
/// see-through a backing is).
macro_rules! tokens {
    (
        colors: [$($color:ident: $color_kind:ident),* $(,)?],
        numbers: [$($number:ident: $number_kind:ident),* $(,)?] $(,)?
    ) => {
        /// The colour names, and how each sets its token.
        const COLOR_TOKENS: [(&str, fn(&mut Theme, [u8; 4])); APPEARANCE_COLOR_NAMES.len()] = [
            $((stringify!($color), |theme: &mut Theme, rgba: [u8; 4]| {
                theme.$color = tokens!(@$color_kind rgba);
            }),)*
        ];

        /// The number names, and how each sets its token.
        const NUMBER_TOKENS: [(&str, fn(&mut Theme, f32)); APPEARANCE_NUMBER_RANGES.len()] = [
            $((stringify!($number), |theme: &mut Theme, value: f32| {
                theme.$number = tokens!(@$number_kind value);
            }),)*
        ];

        /// Names every field of `Theme`: one added there without a token here does not compile.
        #[cfg(test)]
        #[allow(dead_code)]
        fn every_field_is_a_token(theme: Theme) {
            let Theme { $($color: _,)* $($number: _,)* row_colors: _ } = theme;
        }
    };
    (@plain $rgba:ident) => { color($rgba) };
    (@some $rgba:ident) => { Some(color($rgba)) };
    (@opaque $rgba:ident) => { opaque($rgba) };
    (@some_opaque $rgba:ident) => { Some(opaque($rgba)) };
    (@number $value:ident) => { $value };
    (@some_number $value:ident) => { Some($value) };
}

tokens! {
    colors: [
        waveform: some,
        waveform_selected: some,
        plot_background: opaque,
        center_line: plain,
        border: some,
        lane_divider: some,
        bar_line: plain,
        second_line: plain,
        row_label: plain,
        flag: plain,
        playhead: plain,
        panel_background: some_opaque,
        chip_background: plain,
        chip_text: plain,
        chip_current: some,
        scroll_thumb: some,
        preview_highlight: some,
    ],
    numbers: [
        border_width: some_number,
        corner_radius: number,
        waveform_stroke_scale: number,
        waveform_fill_opacity: number,
    ],
}

fn color([r, g, b, a]: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn opaque([r, g, b, _]: [u8; 4]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The look an add-on offers, before it is checked: the window's own look with each token the
/// offer names set. Names the contract does not list are ignored.
pub(super) fn theme_from(appearance: &Appearance) -> Theme {
    let mut theme = Theme::default();
    for name in APPEARANCE_COLOR_NAMES {
        if let Some(&rgba) = appearance.colors.get(name)
            && let Some((_, set)) = COLOR_TOKENS.iter().find(|(token, _)| *token == name)
        {
            set(&mut theme, rgba);
        }
    }
    for (name, range) in APPEARANCE_NUMBER_RANGES {
        if let Some(&value) = appearance.numbers.get(name).filter(|value| value.is_finite())
            && let Some((_, set)) = NUMBER_TOKENS.iter().find(|(token, _)| *token == name)
        {
            set(&mut theme, value.clamp(*range.start(), *range.end()) as f32);
        }
    }
    theme.row_colors = appearance
        .rows
        .iter()
        .take(MAX_APPEARANCE_ROWS)
        .map(|(label, &rgba)| (label.clone(), color(rgba)))
        .collect();
    theme
}

const INDICATORS: [CaptureIndicator; 4] = [
    CaptureIndicator::Unavailable,
    CaptureIndicator::Paused,
    CaptureIndicator::Recording,
    CaptureIndicator::Waiting,
];

/// What a look has to keep readable in this window, for this accent.
fn checked(theme: Theme, accent: Color32) -> Theme {
    let marks = |paint: fn(CaptureIndicator, bool) -> Color32| {
        let mut colors: Vec<Color32> = [false, true]
            .into_iter()
            .flat_map(|lit| INDICATORS.map(|indicator| paint(indicator, lit)))
            .collect();
        colors.dedup();
        colors
    };
    let plot_marks = marks(CaptureIndicator::compact_color);
    let panel_marks = marks(CaptureIndicator::led_color);
    theme.legible(&Legibility {
        accent,
        selection_contrast: SELECTION_CONTRAST,
        plot_marks: &plot_marks,
        panel_marks: &panel_marks,
        panel_text: egui::Visuals::dark().text_color(),
    })
}

/// Which add-on's look the window takes, and the look worked out from its latest offer.
pub(super) struct AddonLook {
    chosen: Option<String>,
    /// The offers the look was last worked out from.
    offers: Arc<AddonAppearances>,
    /// The add-ons' names, for the Options row.
    names: Arc<AddonSnapshot>,
    /// The accent the look was last worked out for.
    accent: Option<Color32>,
    stale: bool,
    /// The look last handed to the window.
    shown: Theme,
    /// How many times a look has been worked out.
    builds: usize,
    /// Whether a new offer should wake the window: only while a look is chosen.
    wants_offers: Arc<AtomicBool>,
}

impl AddonLook {
    pub(super) fn new(chosen: Option<String>) -> Self {
        Self {
            wants_offers: Arc::new(AtomicBool::new(chosen.is_some())),
            chosen,
            offers: Arc::default(),
            names: Arc::default(),
            accent: None,
            stale: true,
            shown: Theme::default(),
            builds: 0,
        }
    }

    pub(super) fn chosen(&self) -> Option<&str> {
        self.chosen.as_deref()
    }

    /// Takes `chosen` as the add-on whose look is on, `None` for the window's own. Returns
    /// whether that changed anything.
    pub(super) fn choose(&mut self, chosen: Option<String>) -> bool {
        if self.chosen == chosen {
            return false;
        }
        self.wants_offers.store(chosen.is_some(), Ordering::Relaxed);
        self.chosen = chosen;
        self.stale = true;
        true
    }

    /// Whether the chosen add-on offers a look now.
    pub(super) fn offered(&self) -> bool {
        self.chosen
            .as_ref()
            .is_some_and(|id| self.offers.contains_key(id))
    }

    /// Takes the newest offers (`None` while they are being published) and the accent, and
    /// returns the look to draw when it is not the one last returned. Unchanged offers and
    /// accent do no work.
    pub(super) fn update(
        &mut self,
        offers: Option<Arc<AddonAppearances>>,
        accent: Color32,
    ) -> Option<Theme> {
        if let Some(offers) = offers
            && !Arc::ptr_eq(&offers, &self.offers)
        {
            self.offers = offers;
            self.stale = true;
        }
        if self.accent != Some(accent) {
            self.accent = Some(accent);
            self.stale = true;
        }
        if !std::mem::take(&mut self.stale) {
            return None;
        }
        self.builds += 1;
        let theme = self
            .chosen
            .as_ref()
            .and_then(|id| self.offers.get(id))
            .map_or_else(Theme::default, |offer| checked(theme_from(offer), accent));
        if theme == self.shown {
            return None;
        }
        self.shown = theme.clone();
        Some(theme)
    }

    /// The add-ons that offer a look now, by id and name, in id order.
    fn offering(&self) -> Vec<(String, String)> {
        self.offers
            .keys()
            .map(|id| (id.clone(), self.name(id)))
            .collect()
    }

    /// An add-on's name, or its id when it is not installed.
    fn name(&self, id: &str) -> String {
        self.names
            .addon(id)
            .and_then(|addon| addon.manifest.as_ref())
            .map_or_else(|| id.to_owned(), |manifest| manifest.name.clone())
    }
}

/// The "Look from add-on" row: None, each add-on in `offering` (id and name), and the chosen
/// add-on named `chosen_name` even while it offers nothing. Returns the new choice when the
/// user made one.
pub(super) fn look_row(
    ui: &mut egui::Ui,
    language: Language,
    chosen: Option<&str>,
    chosen_name: &str,
    offering: &[(String, String)],
) -> Option<Option<String>> {
    let mut entries: Vec<(Option<&str>, String)> = vec![(None, language.look_none().to_owned())];
    entries.extend(
        offering
            .iter()
            .map(|(id, name)| (Some(id.as_str()), name.clone())),
    );
    let missing = chosen.filter(|id| !offering.iter().any(|(offered, _)| offered == id));
    if let Some(id) = missing {
        entries.push((Some(id), chosen_name.to_owned()));
    }
    let before = entries
        .iter()
        .position(|(id, _)| *id == chosen)
        .unwrap_or(0);
    let mut selected = before;
    options_field(ui, language.look_field(), |ui| {
        options_choice(
            ui,
            "addon-look",
            &mut selected,
            entries
                .iter()
                .enumerate()
                .map(|(index, (_, name))| (index, name.clone())),
        );
        if missing.is_some() {
            options_note(ui, language.look_not_offered(chosen_name));
        } else if offering.is_empty() {
            options_note(ui, language.look_none_offered());
        }
    });
    (selected != before).then(|| entries[selected].0.map(str::to_owned))
}

impl Recorder {
    /// The row on the Appearance page that chooses an add-on's look.
    pub(super) fn addon_look_settings(&mut self, ui: &mut egui::Ui) {
        if let Some(snapshot) = self.controller.addons().and_then(AddonHandle::try_snapshot) {
            self.look.names = snapshot;
        }
        let offering = self.look.offering();
        let chosen_name = self.look.chosen().map(|id| self.look.name(id)).unwrap_or_default();
        if let Some(choice) = look_row(
            ui,
            Language::current(),
            self.look.chosen(),
            &chosen_name,
            &offering,
        ) {
            self.choose_look(choice);
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    /// Chooses the add-on whose look the window takes, or none, and keeps the choice. The
    /// window takes the look in its next pass.
    pub(super) fn choose_look(&mut self, choice: Option<String>) {
        if self.look.choose(choice.clone()) {
            self.save_preference(Key::AddonLook, choice.unwrap_or_default());
        }
    }

    /// Takes the chosen add-on's newest look, once per pass. A look that changes the border's
    /// width resizes the waveform-only window as a new width in Options does.
    pub(super) fn refresh_look(&mut self, ctx: &egui::Context) {
        let offers = self.controller.addons().and_then(AddonHandle::try_appearances);
        if let Some(theme) = self.look.update(offers, self.accent) {
            let border_inset = self.border_inset();
            self.set_theme(theme);
            self.fit_border_change(ctx, border_inset);
        }
    }

    /// Wakes the window when an add-on's offer changes while a look is chosen, so a look that
    /// moves is drawn as it moves and nothing is drawn again otherwise.
    pub(super) fn wake_on_new_looks(&self, ctx: &egui::Context) {
        let Some(addons) = self.controller.addons() else {
            return;
        };
        let wanted = Arc::clone(&self.look.wants_offers);
        let ctx = ctx.clone();
        addons.subscribe_appearances(Arc::new(move || {
            if wanted.load(Ordering::Relaxed) {
                ctx.request_repaint_after_for(Duration::from_millis(1), egui::ViewportId::ROOT);
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use backtracking_ui::{
        GRAPHIC_CONTRAST, MIN_ALPHA, PLOT_BACKGROUND, contrast_ratio, selected_waveform_color,
    };
    use eframe::egui::{Pos2, Rect, Vec2};

    use super::*;
    use crate::addons::{AddonInfo, AddonState};
    use crate::ui::{paint_compact_status, paint_row_waveform};
    use backtracking_core::waveform;

    const ACCENT: Color32 = Color32::from_rgb(103, 231, 181);

    /// Each top-level field of a theme as `Debug` writes it, by name.
    fn fields(theme: &Theme) -> BTreeMap<String, String> {
        let text = format!("{theme:#?}");
        let mut fields = BTreeMap::new();
        let mut current: Option<String> = None;
        for line in text.lines() {
            let field = line
                .strip_prefix("    ")
                .filter(|rest| !rest.starts_with(' '))
                .and_then(|rest| rest.split_once(':'))
                .map(|(name, _)| name.to_owned());
            if let Some(name) = field {
                current = Some(name.clone());
                fields.insert(name, String::new());
            }
            if let Some(name) = &current {
                fields.get_mut(name).unwrap().push_str(line);
            }
        }
        fields
    }

    /// The fields of `theme` that differ from the window's own look.
    fn changed(theme: &Theme) -> Vec<String> {
        let own = fields(&Theme::default());
        fields(theme)
            .into_iter()
            .filter(|(name, text)| own.get(name) != Some(text))
            .map(|(name, _)| name)
            .collect()
    }

    fn appearance(colors: &[(&str, [u8; 4])], numbers: &[(&str, f64)]) -> Appearance {
        Appearance {
            colors: colors
                .iter()
                .map(|(name, rgba)| ((*name).to_owned(), *rgba))
                .collect(),
            numbers: numbers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), *value))
                .collect(),
            rows: BTreeMap::new(),
        }
    }

    #[test]
    fn every_appearance_name_sets_the_theme_field_of_that_name_and_back() {
        let colors: BTreeSet<&str> = COLOR_TOKENS.iter().map(|(name, _)| *name).collect();
        let numbers: BTreeSet<&str> = NUMBER_TOKENS.iter().map(|(name, _)| *name).collect();
        assert_eq!(colors, APPEARANCE_COLOR_NAMES.into_iter().collect());
        assert_eq!(
            numbers,
            APPEARANCE_NUMBER_RANGES
                .iter()
                .map(|(name, _)| *name)
                .collect()
        );
        // Every field of the theme but the row colours has a name, and only one.
        let theme_fields: BTreeSet<String> = fields(&Theme::default()).into_keys().collect();
        let names: BTreeSet<String> = colors
            .iter()
            .chain(&numbers)
            .map(|name| (*name).to_owned())
            .chain(["row_colors".to_owned()])
            .collect();
        assert_eq!(theme_fields, names);
        assert_eq!(colors.len() + numbers.len() + 1, theme_fields.len());

        // Each name sets the field it names and nothing else.
        for name in APPEARANCE_COLOR_NAMES {
            let theme = theme_from(&appearance(&[(name, [1, 2, 3, 200])], &[]));
            assert_eq!(changed(&theme), [name], "{name}");
        }
        for (name, range) in APPEARANCE_NUMBER_RANGES {
            let theme = theme_from(&appearance(&[], &[(name, *range.start())]));
            assert_eq!(changed(&theme), [name], "{name}");
        }
    }

    #[test]
    fn an_offer_sets_only_the_tokens_it_names() {
        assert_eq!(theme_from(&Appearance::default()), Theme::default());
        let mut offer = appearance(
            &[
                ("waveform", [10, 20, 200, 255]),
                ("border", [255, 255, 255, 128]),
                ("plot_background", [20, 29, 42, 64]),
                ("no_such_colour", [1, 1, 1, 255]),
            ],
            &[
                ("border_width", 2.0),
                ("corner_radius", 99.0),
                ("waveform_fill_opacity", f64::NAN),
                ("no_such_number", 1.0),
            ],
        );
        offer.rows.insert("LFE".into(), [255, 160, 88, 255]);
        let theme = theme_from(&offer);
        assert_eq!(
            theme,
            Theme {
                waveform: Some(Color32::from_rgb(10, 20, 200)),
                border: Some(Color32::from_rgba_unmultiplied(255, 255, 255, 128)),
                // The backing's alpha is the user's transparency, not the look's.
                plot_background: Color32::from_rgb(20, 29, 42),
                border_width: Some(2.0),
                corner_radius: 16.0,
                row_colors: [("LFE".to_owned(), Color32::from_rgb(255, 160, 88))].into(),
                ..Theme::default()
            }
        );
    }

    fn offers(entries: &[(&str, Appearance)]) -> Arc<AddonAppearances> {
        Arc::new(
            entries
                .iter()
                .map(|(id, look)| ((*id).to_owned(), look.clone()))
                .collect(),
        )
    }

    fn blue() -> Appearance {
        appearance(
            &[("waveform", [90, 160, 255, 255]), ("flag", [255, 80, 200, 255])],
            &[("border_width", 3.0)],
        )
    }

    #[test]
    fn a_look_is_off_until_chosen_and_follows_the_chosen_offer() {
        let mut look = AddonLook::new(None);
        let both = offers(&[("com.example.theme", blue()), ("com.example.other", appearance(&[("flag", [0, 0, 255, 255])], &[]))]);
        // Nothing chosen: offers change nothing.
        assert_eq!(look.update(Some(Arc::clone(&both)), ACCENT), None);
        assert!(!look.wants_offers.load(Ordering::Relaxed));

        assert!(look.choose(Some("com.example.theme".into())));
        assert!(look.wants_offers.load(Ordering::Relaxed));
        let theme = look.update(None, ACCENT).expect("the chosen look");
        assert_eq!(theme.waveform, Some(Color32::from_rgb(90, 160, 255)));
        assert_eq!(theme.flag, Color32::from_rgb(255, 80, 200));
        assert_eq!(theme.border_width, Some(3.0));
        assert!(look.offered());

        // At most one look: choosing another replaces it.
        assert!(look.choose(Some("com.example.other".into())));
        let theme = look.update(None, ACCENT).unwrap();
        assert_eq!(theme.flag, Color32::from_rgb(0, 0, 255));
        assert_eq!(theme.waveform, None);

        // None puts the window's own look back exactly.
        assert!(look.choose(None));
        assert_eq!(look.update(None, ACCENT), Some(Theme::default()));
        assert!(!look.choose(None));
    }

    #[test]
    fn a_chosen_add_on_that_offers_nothing_shows_the_windows_look_and_keeps_the_choice() {
        let mut look = AddonLook::new(Some("com.example.theme".into()));
        assert_eq!(look.update(Some(offers(&[])), ACCENT), None, "the window's own look");
        assert!(!look.offered());
        assert_eq!(look.chosen(), Some("com.example.theme"));

        // The add-on starts offering: its look comes back.
        let theme = look.update(Some(offers(&[("com.example.theme", blue())])), ACCENT);
        assert_eq!(theme.unwrap().waveform, Some(Color32::from_rgb(90, 160, 255)));
        // It stops: the window's own look, and the choice stays.
        assert_eq!(look.update(Some(offers(&[])), ACCENT), Some(Theme::default()));
        assert_eq!(look.chosen(), Some("com.example.theme"));
    }

    #[test]
    fn unchanged_offers_do_no_work() {
        let mut look = AddonLook::new(Some("com.example.theme".into()));
        let same = offers(&[("com.example.theme", blue())]);
        assert!(look.update(Some(Arc::clone(&same)), ACCENT).is_some());
        assert_eq!(look.builds, 1);
        for _ in 0..5 {
            assert_eq!(look.update(Some(Arc::clone(&same)), ACCENT), None);
            assert_eq!(look.update(None, ACCENT), None, "offers being published");
        }
        assert_eq!(look.builds, 1);
        // A new offer with the same values is worked out once, and changes nothing drawn.
        assert_eq!(look.update(Some(offers(&[("com.example.theme", blue())])), ACCENT), None);
        assert_eq!(look.builds, 2);
        // A new accent can change what the look keeps.
        look.update(Some(same), Color32::from_rgb(10, 10, 10));
        assert_eq!(look.builds, 3);
    }

    #[test]
    fn a_wild_offer_is_drawn_legibly() {
        // Everything in the backing's colour, and nearly transparent.
        for rgba in [[22, 26, 29, 255], [255, 255, 255, 255], [128, 128, 128, 10], [103, 231, 181, 255]] {
            let mut offer = Appearance {
                colors: APPEARANCE_COLOR_NAMES
                    .into_iter()
                    .map(|name| (name.to_owned(), rgba))
                    .collect(),
                ..Appearance::default()
            };
            offer.rows.insert("LFE".into(), rgba);
            let mut look = AddonLook::new(Some("wild".into()));
            let theme = look
                .update(Some(offers(&[("wild", offer)])), ACCENT)
                .unwrap_or_default();
            let backing = theme.plot_background;
            for drawn in [
                theme.waveform(ACCENT),
                theme.waveform_selected(ACCENT, SELECTION_CONTRAST),
                theme.flag,
                theme.playhead,
                theme.row_waveform(Some("LFE"), ACCENT),
            ] {
                assert!(drawn.a() >= MIN_ALPHA, "{rgba:?}: {drawn:?}");
                assert!(
                    contrast_ratio(drawn, backing) >= GRAPHIC_CONTRAST,
                    "{rgba:?}: {drawn:?} on {backing:?}"
                );
            }
            for indicator in INDICATORS {
                for lit in [false, true] {
                    assert!(backtracking_ui::distinguishable(indicator.compact_color(lit), backing));
                }
            }
        }
    }

    /// The colours of what the status mark paints over a row drawn in `theme`.
    fn status_colors(theme: &Theme) -> Vec<Color32> {
        let plot = Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 60.0));
        let mut colors = Vec::new();
        for problem in [None, Some("No audio device")] {
            for indicator in INDICATORS {
                for lit in [false, true] {
                    let context = egui::Context::default();
                    let mut output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(plot),
                            ..Default::default()
                        },
                        |ui| {
                            let geometry =
                                waveform::waveform_geometry(&[-0.6; 3], &[0.8; 3], 0..300);
                            paint_row_waveform(
                                ui.painter(),
                                plot,
                                &geometry,
                                1.5,
                                None,
                                theme,
                                ACCENT,
                                None,
                            );
                            paint_compact_status(ui.painter(), plot, indicator, lit, problem);
                        },
                    );
                    let shapes = std::mem::take(&mut output.shapes);
                    output.drop_without_applying_deltas();
                    colors.extend(shapes.into_iter().filter_map(|shape| match shape.shape {
                        egui::Shape::Circle(circle) => Some(circle.fill),
                        egui::Shape::Rect(rect) => Some(rect.fill),
                        egui::Shape::Text(text) => Some(text.fallback_color),
                        _ => None,
                    }));
                }
            }
        }
        colors
    }

    /// The outline colours of one row's waveform with a selection over its middle.
    fn waveform_lines(theme: &Theme, channel: Option<&str>) -> Vec<Color32> {
        let audio = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(120.0, 100.0));
        let geometry = waveform::waveform_geometry(&[-0.6; 3], &[0.8; 3], 100..400);
        let context = egui::Context::default();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0))),
                ..Default::default()
            },
            |ui| {
                paint_row_waveform(
                    ui.painter(),
                    audio,
                    &geometry,
                    1.5,
                    Some(&(0.25..0.75)),
                    theme,
                    ACCENT,
                    channel,
                );
            },
        );
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        let mut lines: Vec<Color32> = shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Path(path) => match path.stroke.color {
                    egui::epaint::ColorMode::Solid(color) => Some(color),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        lines.dedup();
        lines
    }

    #[test]
    fn the_status_colours_are_the_same_under_any_look() {
        let own = status_colors(&Theme::default());
        for rgba in [[22, 26, 29, 255], [255, 255, 255, 255], [246, 196, 118, 255], [255, 0, 255, 255]] {
            let wild = Appearance {
                colors: APPEARANCE_COLOR_NAMES
                    .into_iter()
                    .map(|name| (name.to_owned(), rgba))
                    .collect(),
                ..Appearance::default()
            };
            for theme in [theme_from(&wild), checked(theme_from(&wild), ACCENT)] {
                assert_eq!(status_colors(&theme), own, "{rgba:?}");
            }
        }
    }

    #[test]
    fn a_chosen_look_paints_the_waveform_in_its_colours() {
        let mut offer = blue();
        offer.colors.insert("waveform_selected".into(), [255, 230, 0, 255]);
        offer.rows.insert("LFE".into(), [255, 120, 60, 255]);
        let mut look = AddonLook::new(Some("com.example.theme".into()));
        let theme = look
            .update(Some(offers(&[("com.example.theme", offer)])), ACCENT)
            .unwrap();
        let waveform = Color32::from_rgb(90, 160, 255);
        let selected = Color32::from_rgb(255, 230, 0);
        assert_eq!(waveform_lines(&theme, None), [waveform, selected, waveform]);
        assert_eq!(
            waveform_lines(&theme, Some("LFE")),
            [Color32::from_rgb(255, 120, 60), selected, Color32::from_rgb(255, 120, 60)]
        );
        // Without a look, the accent and its complement as always.
        let own = selected_waveform_color(ACCENT, SELECTION_CONTRAST);
        assert_eq!(waveform_lines(&Theme::default(), None), [ACCENT, own, ACCENT]);
        assert_eq!(theme.plot_background, PLOT_BACKGROUND);
    }

    fn addon(id: &str, name: &str) -> AddonInfo {
        AddonInfo {
            id: id.into(),
            manifest: Some(
                backtracking_addon::manifest::Manifest::parse(&format!(
                    r#"{{"contract":1,"id":"{id}","name":"{name}","version":"0.1.0",
                        "author":"Example","description":"A look.","capabilities":["appearance"],
                        "run":{{"windows-x86_64":{{"program":"bin/look.exe"}}}}}}"#
                ))
                .unwrap(),
            ),
            enabled: true,
            state: AddonState::Running,
            pid: None,
            message: None,
            sources: Vec::new(),
            settings: Vec::new(),
            actions: Vec::new(),
        }
    }

    /// Draws the row once and returns what it wrote and the choice it made.
    fn draw_row(
        language: Language,
        chosen: Option<&str>,
        chosen_name: &str,
        offering: &[(String, String)],
    ) -> (Vec<String>, Option<Option<String>>) {
        let mut choice = None;
        let context = egui::Context::default();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 300.0))),
                ..Default::default()
            },
            |ui| {
                choice = look_row(ui, language, chosen, chosen_name, offering);
            },
        );
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        let text = shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        (text, choice)
    }

    #[test]
    fn the_options_row_draws_in_both_languages_with_and_without_offers() {
        let mut look = AddonLook::new(Some("com.example.theme".into()));
        look.names = Arc::new(AddonSnapshot {
            revision: 1,
            addons: vec![addon("com.example.theme", "Example appearance")],
            error: None,
        });
        let name = look.name("com.example.theme");
        assert_eq!(name, "Example appearance");
        assert_eq!(look.name("com.example.gone"), "com.example.gone");
        for language in [Language::English, Language::TraditionalChinese] {
            let joined = |text: Vec<String>| text.join("\n");

            // Nothing offered, nothing chosen.
            let (text, choice) = draw_row(language, None, "", &[]);
            let text = joined(text);
            assert!(text.contains(language.look_field()), "{text}");
            assert!(text.contains(language.look_none()), "{text}");
            assert!(text.contains(language.look_none_offered()), "{text}");
            assert_eq!(choice, None);

            // The chosen add-on offers its look.
            let offering = vec![("com.example.theme".to_owned(), name.clone())];
            let (text, _) = draw_row(language, Some("com.example.theme"), &name, &offering);
            let text = joined(text);
            assert!(text.contains("Example appearance"), "{text}");
            assert!(!text.contains(&language.look_not_offered(&name)), "{text}");

            // The chosen add-on is not offering one: its name and a note.
            let (text, _) = draw_row(language, Some("com.example.theme"), &name, &[]);
            let text = joined(text);
            assert!(text.contains("Example appearance"), "{text}");
            assert!(text.contains(&language.look_not_offered(&name)), "{text}");
        }
    }
}

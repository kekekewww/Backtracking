//! The declarative values an add-on may offer for the app's appearance.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::{finite_number, RawFields, MAX_CHANNEL_LABEL_CHARS};

/// Names accepted in an appearance's color map.
pub const APPEARANCE_COLOR_NAMES: [&str; 17] = [
    "waveform",
    "waveform_selected",
    "plot_background",
    "center_line",
    "border",
    "lane_divider",
    "bar_line",
    "second_line",
    "row_label",
    "flag",
    "playhead",
    "panel_background",
    "chip_background",
    "chip_text",
    "chip_current",
    "scroll_thumb",
    "preview_highlight",
];

/// Names and inclusive ranges accepted in an appearance's number map.
pub const APPEARANCE_NUMBER_RANGES: [(&str, RangeInclusive<f64>); 4] = [
    ("border_width", 0.0..=8.0),
    ("corner_radius", 0.0..=16.0),
    ("waveform_stroke_scale", 0.5..=3.0),
    ("waveform_fill_opacity", 0.0..=1.0),
];

/// The maximum number of channel row colors kept from one appearance.
pub const MAX_APPEARANCE_ROWS: usize = 64;

/// The rolling one-second limit for appearance messages from one add-on.
pub const MAX_APPEARANCE_MESSAGES_PER_SECOND: usize = 10;

/// A complete set of appearance overrides. Missing entries use the app's own look.
/// Colors are stored as red, green, blue and alpha, with alpha 255 for opaque colors.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    #[serde(with = "hex_colors", skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, [u8; 4]>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub numbers: BTreeMap<String, f64>,
    #[serde(with = "hex_colors", skip_serializing_if = "BTreeMap::is_empty")]
    pub rows: BTreeMap<String, [u8; 4]>,
}

impl Appearance {
    /// Whether the add-on offers any overrides.
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty() && self.numbers.is_empty() && self.rows.is_empty()
    }
}

pub(super) fn decode_appearance(fields: &RawFields<'_>) -> Option<Appearance> {
    let colors = entries(fields, "colors")?
        .into_iter()
        .filter(|(name, _)| APPEARANCE_COLOR_NAMES.contains(&name.as_str()))
        .filter_map(color_entry)
        .collect();
    let ranges = APPEARANCE_NUMBER_RANGES;
    let numbers = entries(fields, "numbers")?
        .into_iter()
        .filter_map(|(name, raw)| {
            let (_, range) = ranges.iter().find(|(known, _)| *known == name)?;
            let number = finite_number(raw)?;
            Some((name, number.clamp(*range.start(), *range.end())))
        })
        .collect();
    let rows = entries(fields, "rows")?
        .into_iter()
        .filter(|(label, _)| (1..=MAX_CHANNEL_LABEL_CHARS).contains(&label.chars().count()))
        .filter_map(color_entry)
        .take(MAX_APPEARANCE_ROWS)
        .collect();
    Some(Appearance {
        colors,
        numbers,
        rows,
    })
}

fn entries<'a>(fields: &RawFields<'a>, name: &str) -> Option<RawFields<'a>> {
    match fields.get(name).copied() {
        Some(raw) => serde_json::from_str(raw.get()).ok(),
        None => Some(BTreeMap::new()),
    }
}

fn color_entry((name, raw): (String, &RawValue)) -> Option<(String, [u8; 4])> {
    let hex: String = serde_json::from_str(raw.get()).ok()?;
    Some((name, parse_color(&hex)?))
}

fn parse_color(hex: &str) -> Option<[u8; 4]> {
    let digits = hex.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut rgba = [255; 4];
    for (index, channel) in rgba.iter_mut().take(digits.len() / 2).enumerate() {
        *channel = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(rgba)
}

mod hex_colors {
    use super::*;

    pub(super) fn serialize<S: serde::Serializer>(
        colors: &BTreeMap<String, [u8; 4]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_map(colors.iter().map(|(name, &[r, g, b, a])| {
            let color = if a == 255 {
                format!("#{r:02x}{g:02x}{b:02x}")
            } else {
                format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
            };
            (name, color)
        }))
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<String, [u8; 4]>, D::Error> {
        BTreeMap::<String, String>::deserialize(deserializer)?
            .into_iter()
            .map(|(name, hex)| {
                parse_color(&hex)
                    .map(|color| (name, color))
                    .ok_or_else(|| serde::de::Error::custom("expected #rrggbb or #rrggbbaa"))
            })
            .collect()
    }
}

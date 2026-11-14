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
}

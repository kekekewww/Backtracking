use backtracking_core::WavSampleFormat;

/// The WAV sample rates on offer, with the label each is listed under.
pub const WAV_SAMPLE_RATES: [(u32, &str); 3] =
    [(44_100, "44.1 kHz"), (48_000, "48 kHz"), (96_000, "96 kHz")];
/// The WAV encodings on offer, with the label each is listed under.
pub const WAV_SAMPLE_FORMATS: [(WavSampleFormat, &str); 3] = [
    (WavSampleFormat::Pcm16, "16-bit PCM"),
    (WavSampleFormat::Pcm24, "24-bit PCM"),
    (WavSampleFormat::Float32, "32-bit float"),
];

/// The height of one row of a drop-down list in the current style.
pub fn combo_row_height(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y.max(
        ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y,
    )
}

/// The height a drop-down list needs to show `item_count` rows `row_gap` apart without
/// scrolling. It stops growing at ten rows, and an empty list still gets one.
pub fn combo_popup_height(ui: &egui::Ui, item_count: usize, row_gap: f32) -> f32 {
    let rows = item_count.clamp(1, 10) as f32;
    rows * combo_row_height(ui) + (rows - 1.0) * row_gap
}

#[cfg(test)]
mod tests {
    use backtracking_core::WavExportProfile;

    use super::*;

    #[test]
    fn every_supported_wav_rate_and_encoding_is_listed_once() {
        assert_eq!(WAV_SAMPLE_RATES.map(|(rate, _)| rate), WavExportProfile::SAMPLE_RATES);
        assert_eq!(
            WAV_SAMPLE_FORMATS.map(|(format, _)| format),
            [WavSampleFormat::Pcm16, WavSampleFormat::Pcm24, WavSampleFormat::Float32],
        );
    }

    #[test]
    fn popup_height_counts_rows_and_gaps_and_stops_at_ten_rows() {
        let context = egui::Context::default();
        context.run_ui(egui::RawInput::default(), |ui| {
            let style = ui.style_mut();
            style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
            style.spacing.interact_size.y = 32.0;
            style.spacing.button_padding.y = 6.0;
            let row = combo_row_height(ui);
            assert!(row >= 32.0);
            assert!(row >= ui.text_style_height(&egui::TextStyle::Button) + 12.0);

            for gap in [4.0, 6.0] {
                assert_eq!(combo_popup_height(ui, 1, gap), row);
                assert_eq!(combo_popup_height(ui, 0, gap), row);
                assert_eq!(combo_popup_height(ui, 3, gap), 3.0 * row + 2.0 * gap);
                let ten = combo_popup_height(ui, 10, gap);
                assert_eq!(ten, 10.0 * row + 9.0 * gap);
                assert_eq!(combo_popup_height(ui, 11, gap), ten);
                assert_eq!(combo_popup_height(ui, 200, gap), ten);
            }

            // A tall interaction size sets the row height when the text is shorter.
            ui.spacing_mut().interact_size.y = 60.0;
            assert_eq!(combo_row_height(ui), 60.0);
        }).drop_without_applying_deltas();
    }
}

//! Drawing, widgets and interaction math shared by the Backtracking desktop window and the
//! plug-in editor.
//!
//! Both show the same waveform, toolbar and options form, with their own sizes, colours and
//! wording. Whatever differs between them is a parameter here, so each keeps its own look.
//! The crate depends on egui and `backtracking-core` only, except on Windows, where it also
//! maps the system fonts.

mod form;
mod options;
#[cfg(windows)]
mod system_fonts;
mod theme;
mod timeline;
mod toolbar;
mod trace;

pub use form::{
    OPTIONS_CONTROL_OUTLINE, OPTIONS_CONTROL_WIDTH, OPTIONS_LABEL_TEXT, OPTIONS_LABEL_WIDTH,
    OPTIONS_MENU_ROW_GAP, OPTIONS_NAVIGATION_ROW, OPTIONS_NAVIGATION_WIDTH, OPTIONS_NOTE_TEXT,
    OPTIONS_TITLE_TEXT, OPTIONS_TWO_COLUMN_WIDTH, OPTIONS_VALUE_TEXT, configure_options_menu,
    configure_options_style, options_bold_family, options_choice, options_control_width,
    options_field, options_group_heading, options_heading_text, options_list,
    options_menu_height, options_navigation_item, options_note, options_value,
    remeasure_options_menu,
};
pub use options::{WAV_SAMPLE_FORMATS, WAV_SAMPLE_RATES, combo_popup_height, combo_row_height};
#[cfg(windows)]
pub use system_fonts::add_system_font_fallbacks;
pub use theme::{
    CENTER_LINE, PLOT_BACKGROUND, RECORDING_DIM, RECORDING_LIT, accent_color, muted_accent,
    selected_waveform_color,
};
pub use timeline::{
    FlagReach, fit_frame_range, flag_handle_at, frame_x, paint_guide_lines, selection_modifiers,
    waveform_press,
};
pub use toolbar::{TransportGlyph, icon_button, meter_number, paint_transport_glyph, status_led};
pub use trace::{LoneSample, TraceStyle, paint_waveform_trace};

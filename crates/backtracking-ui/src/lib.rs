//! Drawing, widgets and interaction math shared by the Backtracking desktop window and the
//! plug-in editor.
//!
//! Both show the same waveform, toolbar and options form, with their own sizes, colours and
//! wording. Whatever differs between them is a parameter here, so each keeps its own look.
//! The crate is platform-neutral: it depends on egui and `backtracking-core` only.

mod options;
mod theme;
mod timeline;
mod toolbar;
mod trace;

pub use options::{WAV_SAMPLE_FORMATS, WAV_SAMPLE_RATES, combo_popup_height, combo_row_height};
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

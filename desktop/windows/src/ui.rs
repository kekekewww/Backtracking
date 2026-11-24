use std::collections::VecDeque;
use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::clip_export::{self, ExportOptions};
use crate::context_menu::{self, WindowCommand};
use crate::controller::{
    CaptureSource, ChannelWaveform, Controller, EditSnapshot, LaneGroup, LaneMeterReadings,
    SourceSettings,
};
use crate::file_drag;
use crate::hotkeys::{Action, Binding, Hotkeys, Modifiers, Scope};
use crate::lane_key;
use crate::playback::PreviewPlayer;
use crate::preferences::{
    BORDER_WIDTHS, DEFAULT_ACCENT, Key, MeterMode, PairRender, PreferenceWriter, Preferences,
    Settings, SourceNames, StatusMark, WaveformColoring, WideExportMode, serialize_sources,
    wav_sample_format_preference,
};
use crate::snapshot_hotkey::{self, SnapshotHotkey};
use crate::startup;
use crate::tray::{self, Tray};
use crate::updater::UpdateOptions;
use crate::window_order;
use backtracking_core::waveform::{PairGroup, PairSide, PreviewLane};
use backtracking_core::{
    AutoResumeDelay, ChannelRoute, DurationChoice, Fades, FrozenTrack, Lane, Preview,
    SOURCE_NAME_LIMIT, TakeLabel, WavExportProfile, WaveformView, cleanup, file_name_source,
    waveform,
};
use backtracking_core::{FrozenPositionTrack, spatial::HeadModel};
use backtracking_ui::stack::{self, LaneBox};
use backtracking_ui::{
    FadeEnd, FlagReach, LoneSample, OPTIONS_NAVIGATION_WIDTH, RECORDING_DIM, RECORDING_LIT, Theme,
    TraceStyle, TransportGlyph, WAV_SAMPLE_FORMATS, WAV_SAMPLE_RATES, accent_color,
    add_system_font_fallbacks, configure_options_menu, configure_options_style, dragged_fades,
    fade_end_at, fade_waveform_bins, fit_frame_range, flag_handle_at, icon_button, meter_number,
    muted_accent, options_choice, options_control_width, options_field, options_group_heading,
    options_heading_text, options_list, options_menu_height, options_navigation_item, options_note,
    options_value, paint_colored_waveform_trace, paint_fades, paint_guide_lines, paint_playhead,
    paint_transport_glyph, paint_waveform_trace, remeasure_options_menu, status_led, trace_bin,
    waveform_press,
};
use eframe::egui::{
    self, Color32, CursorIcon, PointerButton, Pos2, Rect, ResizeDirection, Stroke, Vec2,
    ViewportCommand,
};
use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    UI::{
        Input::KeyboardAndMouse::GetCapture,
        WindowsAndMessaging::{
            GetCursorPos, GetWindowRect, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
        },
    },
};
use winit::{
    platform::windows::{Color, CornerPreference, WindowExtWindows},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

mod addon_sources;
mod addons_page;
mod harness;
mod hotkeys_page;
mod look;
mod spectrum;
#[cfg(test)]
mod test_profile;
mod text;

use addon_sources::AddonSources;
use addons_page::{AddonsPage, PageRequest};
use hotkeys_page::HotkeysPage;
use look::AddonLook;
use spectrum::WaveformColors;
use text::Language;

/// How often the window looks at work that only reports progress when asked.
const REFRESH: Duration = Duration::from_millis(80);
/// Frame interval while a drag, gesture, export or pending reply is in progress.
/// It is also the shortest wait the window ever asks for.
const BUSY_REFRESH: Duration = Duration::from_millis(16);
/// The capture worker announces new audio every 80 ms. The waveform may be
/// redrawn slightly sooner, so that timer jitter never skips an announcement.
const WAVEFORM_REFRESH: Duration = Duration::from_millis(70);
/// A meter reads silence 400 ms after its last input, which nothing announces.
const METER_STALE_CHECK: Duration = Duration::from_millis(420);
/// The waveform is drawn finer for this long after the last interaction.
const FINE_EDIT_SETTLE: Duration = Duration::from_millis(250);
/// With nothing to wait for, the window still looks at everything this often,
/// which bounds the cost of any wake-up that was missed.
const SAFETY_TICK: Duration = Duration::from_secs(2);
const PIN_RETRY: Duration = Duration::from_millis(320);
const WINDOW_POSITION_SAVE_DELAY: Duration = Duration::from_millis(250);
/// A retry that keeps failing, or a rescan that cannot run, never repeats faster.
const RETRY_FLOOR: Duration = Duration::from_millis(250);

/// What the window is waiting for that neither an input event, the capture
/// worker nor another thread's repaint request will announce.
#[derive(Clone, Debug, Default)]
struct Waits {
    /// The window is hidden or minimized, so nothing on screen can change.
    hidden: bool,
    /// A drag, gesture, export or reply that is checked every frame.
    busy: bool,
    /// A background job that is only checked for completion.
    polling: bool,
    tray_retry: Option<Duration>,
    position_save: Option<Duration>,
    /// Time left before a paused capture resumes by itself.
    auto_resume: Option<Duration>,
    /// The "resumes in N s" text is on screen or one hover away.
    countdown_shown: bool,
    /// Seconds into the recording blink, while the LED is shown.
    blink: Option<f64>,
    meters_live: bool,
    /// Time since the last interaction, while the waveform is still drawn finer.
    fine_edit: Option<Duration>,
    /// Time until newer audio than the waveform shows may be drawn.
    waveform: Option<Duration>,
    /// Time until the Options window rescans what it lists.
    options_scan: Option<Duration>,
}

/// Time until the "resumes in N s" countdown shows a different number.
fn countdown_tick(remaining: Duration) -> Duration {
    let part = Duration::from_nanos(u64::from(remaining.subsec_nanos()));
    // The label rounds up, so a whole number of seconds is already showing.
    let next = if part.is_zero() {
        Duration::from_secs(1)
    } else {
        part
    };
    next.min(remaining) + Duration::from_millis(2)
}

/// Time until the recording LED, lit for the first half of each second, changes.
fn blink_tick(seconds: f64) -> Duration {
    let into_half = seconds.max(0.0).rem_euclid(0.5);
    Duration::from_secs_f64(0.5 - into_half) + Duration::from_millis(2)
}

/// How long the window may sleep before it must run again without being asked.
fn next_wake(waits: &Waits) -> Duration {
    let mut wake = SAFETY_TICK;
    let mut by = |delay: Option<Duration>| {
        if let Some(delay) = delay {
            wake = wake.min(delay);
        }
    };
    by(waits.busy.then_some(BUSY_REFRESH));
    by(waits.polling.then_some(REFRESH));
    by(waits.tray_retry.map(|retry| retry.max(RETRY_FLOOR)));
    by(waits.position_save);
    // Without the countdown on screen, only the resume itself needs a wake-up.
    by(waits.auto_resume.map(|remaining| {
        if waits.countdown_shown && !waits.hidden {
            countdown_tick(remaining)
        } else {
            remaining
        }
    }));
    if !waits.hidden {
        by(waits.blink.map(blink_tick));
        by(waits.meters_live.then_some(METER_STALE_CHECK));
        by(waits
            .fine_edit
            .map(|since| FINE_EDIT_SETTLE.saturating_sub(since) + Duration::from_millis(1)));
        by(waits.waveform);
        by(waits.options_scan.map(|scan| scan.max(RETRY_FLOOR)));
    }
    wake.max(BUSY_REFRESH)
}
const TRANSPARENCIES: [u8; 5] = [0, 25, 50, 75, 100];
/// The space between two rows of an open Options drop-down.
const WAITING_LED: Color32 = Color32::from_rgb(76, 86, 88);
const PAUSED_LED: Color32 = Color32::from_rgb(54, 57, 60);
const ERROR_LED: Color32 = Color32::from_rgb(246, 196, 118);
// The toolbar's paused and waiting LEDs are too dark to read on the bare waveform.
const COMPACT_PAUSED: Color32 = Color32::from_gray(214);
const COMPACT_WAITING: Color32 = Color32::from_gray(132);
// Centre of the waveform-only status mark, measured from the plot's top-left corner.
const COMPACT_STATUS_OFFSET: Vec2 = Vec2::new(12.0, 11.0);
const NO_AUDIO_HINT: &str = "No captured audio";
const TOOLBAR_BUTTON_SIZE: f32 = 24.0;
const TRANSPORT_GLYPH: TransportGlyph = TransportGlyph {
    play: [
        Vec2::new(-3.5, -5.5),
        Vec2::new(5.0, 0.0),
        Vec2::new(-3.5, 5.5),
    ],
    pause_offset: 2.4,
    pause_bar: Vec2::new(2.4, 10.0),
};
const RESIZE_EDGE: f32 = 7.0;
const RESIZE_CORNER: f32 = 14.0;
const FULL_MIN_WIDTH: f32 = 510.0;
const FULL_MIN_HEIGHT: f32 = 132.0;
const WAVEFORM_MIN_WIDTH: f32 = 240.0;
const WAVEFORM_MIN_HEIGHT: f32 = 48.0;
// Plot height in the default full window, used when launching directly into waveform-only mode.
const DEFAULT_WAVEFORM_HEIGHT: f32 = 59.0;
const WAVEFORM_INSET: f32 = 8.0;
const FLAG_SNAP_DISTANCE: f64 = 8.0;
const FLAG_REACH: FlagReach = FlagReach {
    above: 2.0,
    below: 13.0,
    radius: FLAG_SNAP_DISTANCE as f32,
};
/// Alt is taken: dragging with it moves the window, except on a flag of the selection, where
/// it drags that end's fade.
const ALT_SELECTS: bool = false;
/// While a drag selecting rows is past the edge of a lane that scrolls, the lane shows its
/// next row this often.
const EDGE_SCROLL_STEP: Duration = Duration::from_millis(90);
/// The rule under a wide lane's L/R pair, fainter than the line between two lanes.
const PAIR_DIVIDER: Color32 = Color32::from_rgba_premultiplied(22, 22, 22, 22);
/// The backing behind a wide lane's L/R pair, a shade lighter than the plot.
const PAIR_BACKING: Color32 = Color32::from_rgba_premultiplied(7, 7, 7, 7);
/// The selected waveform takes the accent's complement when it differs by at least this much.
const SELECTION_CONTRAST: u8 = 80;
const PREPARING_WAV: &str = "Preparing WAV…";
/// Shown when Space comes before the selected audio is ready to play.
const PREPARING_SELECTION: &str = "Preparing selection…";

pub(crate) fn run(controller: Controller, primary: bool) -> eframe::Result<()> {
    let show_options_on_start = std::env::args_os().any(|arg| arg == "--show-options");
    let test_copy = startup::test_instance().is_some();
    let test_size = (show_options_on_start || test_copy)
        .then(|| std::env::var("BACKTRACKING_TEST_WINDOW_SIZE").ok())
        .flatten()
        .filter(|size| !size.trim().is_empty());
    let test_position = test_copy
        .then(|| std::env::var(harness::WINDOW_POSITION).ok())
        .flatten()
        .and_then(|value| {
            let (x, y) = value.split_once(',')?;
            Some(egui::Pos2::new(
                x.trim().parse().ok()?,
                y.trim().parse().ok()?,
            ))
        });
    let preferences = Preferences::for_current_user();
    let settings = preferences.load();
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/backtracking-icon.png"))
            .expect("decode the Backtracking icon");
    let mut viewport = egui::ViewportBuilder::default();
    if test_copy {
        // A test copy appears without taking the keyboard from whatever the user is doing.
        viewport = viewport.with_active(false);
    }
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_inner_size(initial_window_size(
                settings.waveform_only,
                test_size.as_deref(),
            ))
            .with_min_inner_size(min_window_size(settings.waveform_only))
            .with_icon(icon)
            .with_taskbar(show_options_on_start)
            .with_decorations(false)
            // The app handles resizing and moving without entering the shell's drag/snap loop.
            .with_resizable(false)
            .with_maximize_button(false)
            .with_transparent(true)
            .with_window_level(if settings.pinned {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        if primary {
            "Backtracking"
        } else {
            "Backtracking Capture"
        },
        options,
        Box::new(|context| {
            configure_style(&context.egui_ctx, accent_color(settings.accent));
            if settings.waveform_only {
                set_waveform_frame(context.winit_window(), true);
            }
            if let (Some(window), Some(position)) = (context.winit_window(), test_position) {
                // The window is still hidden here; it first appears where it is asked to.
                window.set_outer_position(winit::dpi::PhysicalPosition::new(
                    position.x.round() as i32,
                    position.y.round() as i32,
                ));
            } else if primary
                && let (Some(window), Some(position)) =
                    (context.winit_window(), settings.window_position)
            {
                window_order::restore_position(window, position);
            }
            let mut recorder = Recorder::new(controller, preferences, settings, primary);
            recorder.test_copy = test_copy;
            recorder.auto_grow = test_size.is_none();
            recorder.harness = test_copy.then(harness::Harness::from_environment).flatten();
            // Below the main window and as far off screen, so neither window appears.
            recorder.options_test_position =
                test_position.map(|position| position + Vec2::new(0.0, 2000.0));
            let repaint = context.egui_ctx.clone();
            recorder.controller.set_repaint(Arc::new(move || {
                // A delayed request runs one pass. An immediate one runs two.
                repaint.request_repaint_after_for(Duration::from_millis(1), egui::ViewportId::ROOT);
            }));
            recorder.wake_on_new_looks(&context.egui_ctx);
            recorder.options_in_taskbar = show_options_on_start;
            if show_options_on_start {
                recorder.open_options(&context.egui_ctx);
            }
            // A test copy leaves the shortcut, the tray, the startup entry and updates to the
            // installed app.
            if primary && !test_copy {
                // Only the saved chords are registered, so a default the user moved or cleared
                // is never taken from other apps, not even for a moment.
                let initial = recorder.hotkeys.global_bindings();
                match SnapshotHotkey::start_with(context.egui_ctx.clone(), initial) {
                    Ok(hotkey) => recorder.hotkey = Some(hotkey),
                    Err(error) => recorder.hotkey_error = Some(error.to_string()),
                }
            }
            if test_copy {
                if let Some(window) = context.winit_window() {
                    tray::suppress_taskbar(window);
                }
            } else if !show_options_on_start && let Some(window) = context.winit_window() {
                recorder.attach_tray(window, &context.egui_ctx);
            }
            if primary && !test_copy {
                match startup::enabled() {
                    Ok(enabled) => recorder.launch_at_login = enabled,
                    Err(error) => {
                        recorder.message = format!("Could not read startup setting: {error}")
                    }
                }
                crate::updater::app_ready();
            }
            Ok(Box::new(recorder))
        }),
    )
}

fn set_waveform_frame(window: Option<&Arc<Window>>, waveform_only: bool) {
    if let Some(window) = window {
        // egui-winit enables the Windows shadow for undecorated windows at creation.
        window.set_undecorated_shadow(!waveform_only && window.fullscreen().is_none());
        window.set_border_color((!waveform_only).then_some(Color::SYSTEM_DEFAULT));
        window.set_corner_preference(if waveform_only {
            CornerPreference::DoNotRound
        } else {
            CornerPreference::Default
        });
    }
}

fn min_window_size(waveform_only: bool) -> Vec2 {
    if waveform_only {
        Vec2::new(WAVEFORM_MIN_WIDTH, WAVEFORM_MIN_HEIGHT)
    } else {
        Vec2::new(FULL_MIN_WIDTH, FULL_MIN_HEIGHT)
    }
}

fn initial_window_size(waveform_only: bool, diagnostic_size: Option<&str>) -> Vec2 {
    let fallback = Vec2::new(
        610.0,
        if waveform_only {
            DEFAULT_WAVEFORM_HEIGHT
        } else {
            144.0
        },
    );
    let Some((width, height)) = diagnostic_size.and_then(|size| size.split_once('x')) else {
        return fallback;
    };
    let (Ok(width), Ok(height)) = (width.parse::<f32>(), height.parse::<f32>()) else {
        return fallback;
    };
    let minimum = min_window_size(waveform_only);
    if width.is_finite()
        && height.is_finite()
        && (minimum.x..=4096.0).contains(&width)
        && (minimum.y..=2160.0).contains(&height)
    {
        Vec2::new(width, height)
    } else {
        fallback
    }
}

fn options_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("backtracking-options")
}

fn window_is_hidden(window: &Window) -> bool {
    window.is_visible() == Some(false) || window.is_minimized() == Some(true)
}

fn fit_expanded_window(ctx: &egui::Context, frame: &eframe::Frame, size: Vec2) -> Option<Pos2> {
    let window = frame.winit_window()?;
    let monitor = window.current_monitor()?;
    let origin = window.outer_position().ok()?;
    let outer = window.outer_size();
    let inner = window.inner_size();
    let scale = ctx.pixels_per_point();
    let outer_size = size * scale
        + Vec2::new(
            outer.width.saturating_sub(inner.width) as f32,
            outer.height.saturating_sub(inner.height) as f32,
        );
    let monitor_origin = monitor.position();
    let monitor_size = monitor.size();
    let min = Pos2::new(monitor_origin.x as f32, monitor_origin.y as f32);
    let max = min + Vec2::new(monitor_size.width as f32, monitor_size.height as f32);
    let origin = Pos2::new(origin.x as f32, origin.y as f32);
    let fitted = origin.clamp(min, (max - outer_size).max(min));
    if fitted == origin {
        return None;
    }
    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(fitted / scale));
    Some(origin / scale)
}

fn configure_style(ctx: &egui::Context, accent: Color32) {
    ctx.set_visuals(egui::Visuals::dark());
    apply_accent(ctx, accent);
    let mut fonts = egui::FontDefinitions::default();
    let name = waveform::HINT_BOLD_FAMILY;
    fonts.font_data.insert(
        name.to_owned(),
        Arc::new(egui::FontData::from_static(waveform::HINT_BOLD_FONT)),
    );
    fonts
        .families
        .insert(egui::FontFamily::Name(name.into()), vec![name.to_owned()]);
    // Names, paths and Windows messages can be in any language.
    add_system_font_fallbacks(&mut fonts);
    ctx.set_fonts(fonts);
}

fn apply_accent(ctx: &egui::Context, accent: Color32) {
    ctx.global_style_mut(|style| {
        style.visuals.selection.bg_fill = muted_accent(accent);
        style.visuals.selection.stroke.color = Color32::WHITE;
        style.visuals.widgets.active.bg_stroke.color = accent;
        style.visuals.hyperlink_color = accent;
    });
}

struct Recorder {
    controller: Controller,
    preference_writer: Option<PreferenceWriter>,
    retirement: Option<UiRetirement>,
    primary: bool,
    tray: Option<Tray>,
    tray_available: bool,
    next_tray_attach: Instant,
    launch_at_login: bool,
    quitting: bool,
    output: PathBuf,
    /// Deletes old exports from `output` when the user asks for it.
    output_cleanup: cleanup::Schedule,
    saved_output: PathBuf,
    pinned: bool,
    pin_watch: Option<window_order::PinWatch>,
    next_pin_check: Instant,
    /// The last status poll found the capture worker holding the session.
    controller_busy: bool,
    waveform_only: bool,
    full_waveform_height: f32,
    full_window_size: Vec2,
    waveform_border: bool,
    waveform_border_width: u8,
    waveform_border_color: Option<Color32>,
    status_mark: StatusMark,
    show_title: bool,
    show_meters: bool,
    waveform_border_follows_background: bool,
    bar_lines: bool,
    second_lines: bool,
    second_interval: u8,
    options_open: bool,
    options_in_taskbar: bool,
    options_focus_requested: bool,
    options_section: OptionsSection,
    addons_page: AddonsPage,
    /// What add-on sources add to the Sources section, and the compact transport.
    addon_sources: AddonSources,
    /// Where a test copy puts its Options window, off every screen like its main window.
    options_test_position: Option<Pos2>,
    available_sources: Vec<CaptureSource>,
    source_list_worker: Option<JoinHandle<Vec<CaptureSource>>>,
    source_list_refresh_pending: bool,
    /// The audio source list that is open in Options, and whether a scan has replaced its
    /// rows since it was last measured.
    source_menu: Option<egui::LayerId>,
    source_menu_outdated: bool,
    updates: UpdateOptions,
    window_position: Option<[i32; 2]>,
    window_position_changed: Option<Instant>,
    idle_resume: AutoResumeDelay,
    transparency: u8,
    accent: Color32,
    saved_accent: Color32,
    /// How the plot looks, beyond what the user's own choices above decide.
    theme: Theme,
    /// The add-on whose look `theme` is, when the user chose one.
    look: AddonLook,
    /// The waveform's colour: one colour, or by frequency (see `spectrum`).
    waveform_coloring: WaveformColoring,
    /// The user's own gradient for colouring by frequency, and the one last written.
    waveform_gradient: Option<backtracking_ui::Spectrum>,
    saved_waveform_gradient: Option<backtracking_ui::Spectrum>,
    /// The colours read for the rows on screen, while the waveform is coloured by frequency.
    waveform_colors: WaveformColors,
    meter_modes: [MeterMode; 2],
    split_waveform: bool,
    wav_profile: WavExportProfile,
    wav_force_mono: bool,
    wide_export: WideExportMode,
    /// A test copy opens the Multichannel sources list on the Options page's next pass.
    force_wide_export_menu: bool,
    /// How the pair of a wide lane is played and exported.
    pair_render: PairRender,
    /// A system, application or microphone lane on All channels shows only its active rows.
    hide_silent: bool,
    /// What each logical lane knows of its active channels, while capture runs.
    activity: [ChannelActivity; 2],
    /// Head models for binaural preview, one per sample rate, made once and kept.
    head_models: Vec<Arc<HeadModel>>,
    /// The names the user gave sources for their files.
    source_names: SourceNames,
    last_interaction: Instant,
    recording_started: Option<f64>,
    /// The rows last rendered, and what each one shows.
    peaks: Vec<WaveformPeaks>,
    peak_rows: Vec<RowSource>,
    /// How each lane on screen is drawn, worked out once per pass.
    lanes: Vec<LanePlan>,
    /// The first row on screen of each logical lane that scrolls.
    lane_first_row: [usize; 2],
    /// A scroll thumb being dragged: its lane and where on the thumb it was grabbed.
    scroll_drag: Option<(usize, f32)>,
    /// Presses here belong to a lane's group switch or scroll bar, not to the timeline.
    lane_controls: Vec<Rect>,
    /// The wheel scrolls a lane's rows here instead of zooming.
    lane_scroll_zones: Vec<(usize, Rect)>,
    growth: Option<WindowGrowth>,
    plot_rect: Rect,
    editing: Option<FrozenEdit>,
    edit_request: Option<EditRequest>,
    edit_preview: Option<EditPreviewJob>,
    edit_preview_key: Option<EditPreviewKey>,
    /// When the frozen view was asked for and when its audio arrived, until its first picture
    /// is in; a test copy writes how long that took to its log.
    edit_opening: Option<(Instant, Option<Instant>)>,
    pending_snapshot_reads: Vec<SnapshotRead>,
    pending_edit_save_reads: Vec<EditRequest>,
    export_requested: bool,
    seconds: f32,
    sample_rate: u32,
    history_frames: Range<u64>,
    visible_frames: Range<u64>,
    zoom_frames: Option<Range<u64>>,
    pan: Option<(f32, Range<u64>)>,
    bar_markers: Vec<u64>,
    live_revision: Option<u64>,
    next_refresh: Instant,
    generation: u64,
    selection: Option<Range<u64>>,
    /// "Edit separately": a drag selects only the rows it crosses, and those rows alone are
    /// previewed, dragged out and saved. It needs more than one row on show, and the choice
    /// is not kept between launches.
    separate_rows: bool,
    /// The rows a selection covers while rows are separate, top to bottom: those its drag
    /// crossed. Kept by identity, and kept while the mode is off so that turning it on narrows
    /// the selection to them. Empty covers every row.
    selection_rows: Vec<RowSource>,
    /// By logical lane: whether `selection_rows` held every row of that lane when the
    /// selection was made. Such a lane is saved and dragged out whole.
    selection_whole: [bool; 2],
    /// The row a drag-out without a selection takes while rows are separate.
    drag_row: Option<RowSource>,
    /// While a drag selecting rows is past the edge of a lane that scrolls, when that lane
    /// moves on by its next row.
    edge_scroll: Option<Instant>,
    /// Watches the window for taps of Right Ctrl, which turn "Edit separately" on and off.
    lane_key: Option<lane_key::LaneKey>,
    /// The selection's fade in and fade out, in frames from its flags.
    fades: Fades,
    /// The selection playing, started and stopped with Space.
    preview: Option<PreviewPlayer>,
    flags: Option<FlagUpdate>,
    flag_order: u64,
    gesture: Option<Gesture>,
    window_drag: Option<WindowDrag>,
    export: Option<ExportJob>,
    snapshot_save: Option<JoinHandle<io::Result<ExportedWav>>>,
    pending_snapshot_saves: VecDeque<WavClip>,
    hotkey: Option<SnapshotHotkey>,
    hotkey_error: Option<String>,
    /// The keyboard shortcuts this window runs and the system-wide one, as the Hotkeys page
    /// sets them.
    hotkeys: Hotkeys,
    hotkeys_page: HotkeysPage,
    snapshot_target: Option<String>,
    snapshot_targets: Vec<backtracking_core::SenderInfo>,
    snapshot_target_scan: Instant,
    plugin_snapshot_request: Option<JoinHandle<io::Result<()>>>,
    pending_plugin_snapshots: VecDeque<String>,
    clip_revision: u64,
    exported: Option<ExportedWav>,
    message: String,
    /// Started as a test copy beside the installed app (`startup::test_instance`).
    test_copy: bool,
    /// The window may grow to give a stacked lane's rows room. Off when a test asked for a size.
    auto_grow: bool,
    harness: Option<harness::Harness>,
}

enum Gesture {
    /// `resizing` when a flag of the current selection was picked up, which keeps its fades.
    /// `rows` holds the row the drag started in and the row it is at, for a new selection.
    Select {
        anchor: u64,
        cursor: u64,
        origin_x: f32,
        moved: bool,
        resizing: bool,
        rows: Option<(RowSource, RowSource)>,
    },
    /// Alt-dragging a flag of the selection sets the ramp at that end.
    Fade {
        end: FadeEnd,
        press: u64,
        origin_x: f32,
        initial: Fades,
        moved: bool,
    },
    Export {
        preparing: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum OptionsSection {
    Actions,
    Hotkeys,
    Capture,
    Sources,
    Appearance,
    Export,
    Addons,
    Updates,
}

impl OptionsSection {
    const ALL: [(Self, &'static str); 8] = [
        // One short word each, so the list never wraps and a page is easy to name.
        (Self::Actions, "Actions"),
        (Self::Hotkeys, "Hotkeys"),
        (Self::Capture, "Capture"),
        (Self::Sources, "Sources"),
        (Self::Appearance, "Appearance"),
        (Self::Export, "Export"),
        (Self::Addons, "Add-ons"),
        (Self::Updates, "Updates"),
    ];

    fn title(self) -> &'static str {
        // The Add-ons and Hotkeys pages are written in both languages (see `text`).
        if self == Self::Addons {
            return Language::current().addons_section();
        }
        if self == Self::Hotkeys {
            return Language::current().hotkeys_section();
        }
        Self::ALL
            .iter()
            .find(|(section, _)| *section == self)
            .unwrap()
            .1
    }
}

/// "Off" leaves a paused capture paused. It is stored as 0 seconds, which never meant "at once".
fn auto_resume_label(value: AutoResumeDelay) -> String {
    match value.delay() {
        Some(delay) => format!("{} seconds", delay.as_secs()),
        None => "Off".into(),
    }
}

/// What capture is doing right now. The toolbar LED and the waveform-only mark both show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureIndicator {
    Unavailable,
    Paused,
    Recording,
    Waiting,
}

impl CaptureIndicator {
    fn new(unavailable: bool, paused: bool, recording: bool) -> Self {
        if unavailable {
            Self::Unavailable
        } else if paused {
            Self::Paused
        } else if recording {
            Self::Recording
        } else {
            Self::Waiting
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "Capture unavailable",
            Self::Paused => "Paused",
            Self::Recording => "Recording",
            Self::Waiting => "Waiting for sound",
        }
    }

    /// `lit` is the bright half of the recording blink.
    fn led_color(self, lit: bool) -> Color32 {
        match self {
            Self::Unavailable => ERROR_LED,
            Self::Paused => PAUSED_LED,
            Self::Recording if lit => RECORDING_LIT,
            Self::Recording => RECORDING_DIM,
            Self::Waiting => WAITING_LED,
        }
    }

    /// The same states on the bare waveform, where the toolbar's dark LEDs would disappear.
    fn compact_color(self, lit: bool) -> Color32 {
        match self {
            Self::Paused => COMPACT_PAUSED,
            Self::Waiting => COMPACT_WAITING,
            Self::Unavailable | Self::Recording => self.led_color(lit),
        }
    }
}

/// The source failure the user has to read: the capture error, or else one lane's problem.
fn capture_problem(error: Option<&str>, lanes: [Option<&str>; 2], dual: bool) -> Option<String> {
    if let Some(error) = error {
        return Some(error.to_owned());
    }
    lanes.into_iter().enumerate().find_map(|(lane, status)| {
        let status = status?;
        Some(if dual {
            format!("Source {}: {status}", lane_name(lane))
        } else {
            status.to_owned()
        })
    })
}

/// `remaining` is the time until capture resumes by itself, when it is going to.
fn paused_status(remaining: Option<Duration>) -> String {
    match remaining {
        Some(remaining) => {
            let seconds = (remaining.as_secs_f64().ceil() as u64).max(1);
            format!("Paused · resumes in {seconds} s")
        }
        None => "Paused".into(),
    }
}

/// The line under the waveform while nothing more specific is going on.
fn idle_status(paused: Option<String>, message: &str) -> String {
    match paused {
        Some(paused) if message.is_empty() => paused,
        Some(paused) => format!("{paused} · {message}"),
        None => message.to_owned(),
    }
}

/// Everything the waveform-only mark stands for. It stays one paragraph because its tooltip
/// has to fit inside a window that can be 48 px tall.
fn compact_status_text(state: &str, problem: Option<&str>, message: &str) -> String {
    let mut parts = vec![state];
    parts.extend(problem);
    if !message.is_empty() && Some(message) != problem {
        parts.push(message);
    }
    parts.join(" · ")
}

/// The recorder window's own keys, as the Hotkeys page binds them: the full chord has to match.
/// Held keys do not repeat a command. Cancelling a selection is not here; see `is_cancel_key`.
fn window_shortcut(events: &[egui::Event], hotkeys: &Hotkeys) -> Option<WindowCommand> {
    events.iter().find_map(|event| {
        let (modifiers, key) = hotkeys_page::pressed_chord(event)?;
        WindowCommand::for_action(hotkeys.find(Scope::Window, modifiers, key)?)
    })
}

/// A key that cancels the selection or gesture under way. A binding without modifiers takes the
/// key with any held, as Escape and Backspace always have: Shift is often still down from a
/// snapped selection. Repeats count; there is nothing left to cancel after the first.
fn is_cancel_key(event: &egui::Event, hotkeys: &Hotkeys) -> bool {
    let egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        modifiers,
        ..
    } = event
    else {
        return false;
    };
    let Some(key) = hotkeys_page::shortcut_key(*key, *physical_key) else {
        return false;
    };
    let held = hotkeys_page::shortcut_modifiers(*modifiers);
    hotkeys
        .bindings(Action::CancelSelection)
        .iter()
        .any(|binding| match *binding {
            Binding::Chord {
                key: bound,
                modifiers,
            } => bound == key && (modifiers == Modifiers::NONE || modifiers == held),
            Binding::RightCtrlTap => false,
        })
}

/// What a window command from a key or the Right Ctrl tap does, given whether a selection is on
/// screen. The menu and the toolbar run their commands as they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyEffect {
    Run(WindowCommand),
    /// The pause key with a selection plays it or stops it.
    TogglePreview,
    /// The clear key with a selection drops the selection; wiping the capture takes a second
    /// press.
    CancelSelection,
}

fn key_effect(command: WindowCommand, has_selection: bool) -> KeyEffect {
    match command {
        WindowCommand::Clear if has_selection => KeyEffect::CancelSelection,
        WindowCommand::ToggleCapture if has_selection => KeyEffect::TogglePreview,
        command => KeyEffect::Run(command),
    }
}

/// The command a tap of Right Ctrl runs, if the Hotkeys page gave it one.
fn tap_command(hotkeys: &Hotkeys) -> Option<WindowCommand> {
    WindowCommand::for_action(hotkeys.find_binding(Scope::Window, Binding::RightCtrlTap)?)
}

/// The right-click menu, each item naming its command's first key as the Hotkeys page does.
fn menu_entries(
    state: context_menu::State,
    language: Language,
    hotkeys: &Hotkeys,
) -> Vec<context_menu::Entry> {
    context_menu::entries(state, |command| {
        hotkeys_page::shortcut_label(language, hotkeys, command.action())
    })
}

fn history_label(value: DurationChoice) -> String {
    let seconds = value.seconds();
    if seconds <= 60 {
        format!("{seconds} seconds")
    } else {
        format!("{} minutes", seconds / 60)
    }
}

#[derive(Clone)]
struct WaveformPeaks {
    low: Vec<f32>,
    high: Vec<f32>,
}

impl WaveformPeaks {
    fn empty(bins: usize) -> Self {
        Self {
            low: vec![0.0; bins],
            high: vec![0.0; bins],
        }
    }
}

/// What one waveform row draws. This is also the row's identity: a selection of rows is kept
/// as these, never as positions on screen, so scrolling and group switches cannot move it to
/// other audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowSource {
    /// A mono or stereo lane, merged or one side, as such lanes always were.
    View(usize, WaveformView),
    /// One channel of a lane wider than two channels: the lane, then the channel within it.
    Channel(usize, usize),
    /// One side of the stereo fold-down of a wide lane's Channels group, drawn above its
    /// channel rows. Its audio is never stored; it is made from the channels.
    Pair(usize, PairSide),
}

impl RowSource {
    fn lane(self) -> usize {
        match self {
            Self::View(lane, _) | Self::Channel(lane, _) | Self::Pair(lane, _) => lane,
        }
    }

    /// The pair rows of wide lane `lane`, top first.
    fn pair(lane: usize) -> [Self; 2] {
        [
            Self::Pair(lane, PairSide::Left),
            Self::Pair(lane, PairSide::Right),
        ]
    }
}

/// How one lane is drawn: as before, or one row per channel of the group on show.
#[derive(Clone, Debug, PartialEq)]
struct LanePlan {
    lane: usize,
    kind: LaneKind,
}

#[derive(Clone, Debug, PartialEq)]
enum LaneKind {
    Views(Vec<WaveformView>),
    /// `lead` rows are drawn above the channel rows of `group` and are selected together, as
    /// one: the L/R pair of a wide lane, made from `pair`. `shown` holds the lane-native
    /// channels of `group` that get a row, in order: all of them, or only the active ones while
    /// silent rows are hidden. `groups` names every group the lane can show, as (id, label), in
    /// the source's order.
    Stack {
        lead: Vec<RowSource>,
        pair: Option<PairGroup>,
        group: LaneGroup,
        shown: Vec<usize>,
        groups: Vec<(String, String)>,
    },
}

impl LanePlan {
    /// A wide lane that shows every channel of `group`, with no rows above them.
    #[cfg(test)]
    fn stack(lane: usize, group: LaneGroup, groups: Vec<(String, String)>) -> Self {
        Self {
            lane,
            kind: LaneKind::Stack {
                lead: Vec::new(),
                pair: None,
                shown: group.channels.clone().collect(),
                group,
                groups,
            },
        }
    }

    fn stacked(&self) -> bool {
        matches!(self.kind, LaneKind::Stack { .. })
    }

    fn rows(&self) -> usize {
        match &self.kind {
            LaneKind::Views(views) => views.len(),
            LaneKind::Stack { lead, shown, .. } => lead.len() + shown.len(),
        }
    }

    fn row(&self, row: usize) -> RowSource {
        match &self.kind {
            LaneKind::Views(views) => RowSource::View(self.lane, views[row]),
            LaneKind::Stack { lead, shown, .. } => lead
                .get(row)
                .copied()
                .unwrap_or_else(|| RowSource::Channel(self.lane, shown[row - lead.len()])),
        }
    }

    /// The pair the lead rows are made from, when this lane has them.
    fn pair_group(&self) -> Option<&PairGroup> {
        match &self.kind {
            LaneKind::Stack { pair, lead, .. } if !lead.is_empty() => pair.as_ref(),
            _ => None,
        }
    }

    /// The rows selected together with row `row`: the lead rows as one, any other row alone.
    fn unit(&self, row: usize) -> Range<usize> {
        match &self.kind {
            LaneKind::Stack { lead, .. } if row < lead.len() => 0..lead.len(),
            _ => row..row + 1,
        }
    }

    /// The channel of the group on show that row `row` draws, counted within the group.
    fn group_channel(&self, row: usize) -> Option<(&LaneGroup, usize)> {
        match &self.kind {
            LaneKind::Stack {
                lead, group, shown, ..
            } => row
                .checked_sub(lead.len())
                .and_then(|index| shown.get(index))
                .and_then(|&channel| channel.checked_sub(group.channels.start))
                .filter(|&channel| channel < group.channels.len())
                .map(|channel| (group, channel)),
            LaneKind::Views(_) => None,
        }
    }

    fn spec(&self) -> stack::LaneRows {
        stack::LaneRows {
            rows: self.rows(),
            stacked: self.stacked(),
        }
    }
}

/// The group a lane wider than two channels shows: the chosen one, else the first, else all
/// of its channels, numbered.
fn group_on_show(
    groups: &[LaneGroup],
    shown: Option<&str>,
    channels: usize,
) -> (LaneGroup, Vec<(String, String)>) {
    let group = shown
        .and_then(|id| groups.iter().find(|group| group.id == id))
        .or_else(|| groups.first())
        .filter(|group| !group.channels.is_empty() && group.channels.end <= channels)
        .cloned()
        .unwrap_or_else(|| numbered_group(channels));
    let choices = groups
        .iter()
        .map(|group| (group.id.clone(), group.label.clone()))
        .collect();
    (group, choices)
}

/// Every channel of a lane, numbered from 1, for a lane whose groups do not fit it.
fn numbered_group(channels: usize) -> LaneGroup {
    LaneGroup {
        id: String::new(),
        positioned: false,
        label: String::new(),
        channels: 0..channels,
        labels: (1..=channels).map(|channel| channel.to_string()).collect(),
    }
}

/// The add-on declares whether this group supplies object positions.
fn is_objects(group: &LaneGroup) -> bool {
    group.positioned
}

/// The group the pair of a lane is made from: its first group that is not the objects, or
/// every channel of the lane, numbered, when it has none that fits.
fn channels_group(groups: &[LaneGroup], channels: usize) -> LaneGroup {
    groups
        .iter()
        .find(|group| {
            !is_objects(group) && !group.channels.is_empty() && group.channels.end <= channels
        })
        .cloned()
        .unwrap_or_else(|| numbered_group(channels))
}

/// The pair of a lane, made from its Channels group; `None` when the group is too wide.
fn pair_group(groups: &[LaneGroup], channels: usize) -> Option<PairGroup> {
    let group = channels_group(groups, channels);
    PairGroup::new(group.channels.start, &group.labels)
}

/// How often a lane that hides silent rows asks which of its channels are active.
const ACTIVITY_INTERVAL: Duration = Duration::from_millis(250);
/// A channel's row goes only once this many answers in a row have found it silent.
const QUIET_ANSWERS: u8 = 2;

/// What a wide lane knows of which channels have sound, to hide the rows of silent ones.
#[derive(Clone, Debug, Default)]
struct ChannelActivity {
    /// The capture these answers are about.
    generation: u64,
    /// Channels that get a row, bit 0 for the lane's first native channel. `None` until the
    /// first answer.
    shown: Option<u64>,
    /// By channel: how many answers in a row found it silent while it had a row.
    quiet: Vec<u8>,
    /// When the last answer was taken.
    asked: Option<Instant>,
}

impl ChannelActivity {
    /// Takes one answer, `active`. A channel gets its row from the first answer that finds it
    /// active, and keeps it until `QUIET_ANSWERS` answers in a row find it silent, so a
    /// channel at the edge of the history does not blink.
    fn answer(&mut self, active: u64) {
        let shown = self.shown.unwrap_or(0);
        self.quiet.resize(64, 0);
        let mut next = shown | active;
        for (channel, quiet) in self.quiet.iter_mut().enumerate() {
            let bit = 1 << channel;
            if active & bit != 0 || shown & bit == 0 {
                *quiet = 0;
                continue;
            }
            *quiet += 1;
            if *quiet >= QUIET_ANSWERS {
                next &= !bit;
                *quiet = 0;
            }
        }
        self.shown = Some(next);
    }
}

/// The channels of `group` that get a row: every one when `active` is `None`, else only the
/// active ones, and none at all while no channel but the front left and right is active, which
/// the pair already shows.
fn shown_channels(group: &LaneGroup, active: Option<u64>) -> Vec<usize> {
    let all = group.channels.clone();
    let Some(active) = active else {
        return all.collect();
    };
    let is_active = |channel: usize| channel < 64 && active & (1 << channel) != 0;
    let front = |channel: usize| {
        group
            .labels
            .get(channel - group.channels.start)
            .is_some_and(|label| ["FL", "FR", "L", "R"].contains(&label.as_str()))
    };
    if !all
        .clone()
        .any(|channel| is_active(channel) && !front(channel))
    {
        return Vec::new();
    }
    all.filter(|&channel| is_active(channel)).collect()
}

/// Works out, off the window's thread, which channels of the lanes in `hiding` (by logical
/// lane) are active in frozen `snapshots`.
fn frozen_activity(
    snapshots: &Arc<[FrozenTrack]>,
    lanes: &[usize],
    hiding: [bool; 2],
) -> Option<JoinHandle<[Option<u64>; 2]>> {
    if !hiding.contains(&true) {
        return None;
    }
    let snapshots = Arc::clone(snapshots);
    let lanes = lanes.to_vec();
    thread::Builder::new()
        .name("backtracking-frozen-activity".into())
        .spawn(move || {
            std::array::from_fn(|lane| {
                let index = lanes.iter().position(|&held| held == lane)?;
                let track = snapshots.get(index).filter(|_| hiding[lane])?;
                Some(if track.channels() > 2 {
                    track.active_channels()
                } else {
                    0
                })
            })
        })
        .ok()
}

/// The name of `rows` in drawing order: one row, or the first and last, each named by `name`
/// except the pair. With `dual`, a channel's or pair's name starts with its lane's letter.
fn rows_label(rows: &[RowSource], dual: bool, name: impl Fn(RowSource) -> String) -> String {
    let short = |row: RowSource| match row {
        RowSource::Pair(..) => "L/R".to_owned(),
        _ => name(row),
    };
    let named = |row: RowSource| match row {
        RowSource::Channel(lane, _) | RowSource::Pair(lane, _) if dual => {
            format!("{} {}", lane_name(lane), short(row))
        }
        _ => short(row),
    };
    match rows {
        [] => String::new(),
        [row] => named(*row),
        // The pair alone.
        [RowSource::Pair(lane, _), RowSource::Pair(other, _)] if lane == other => named(rows[0]),
        [first, .., last] if first.lane() == last.lane() => {
            format!("{}–{}", named(*first), short(*last))
        }
        [first, .., last] => format!("{}–{}", named(*first), named(*last)),
    }
}

/// The rows from `anchor` to `cursor`, both included, in drawing order across `lanes`. Rows
/// selected together come whole. A row that is not drawn any more stands for itself alone.
fn rows_between(lanes: &[LanePlan], anchor: RowSource, cursor: RowSource) -> Vec<RowSource> {
    // Every row in drawing order, with the run of rows it is selected with.
    let mut rows = Vec::new();
    let mut units = Vec::new();
    for plan in lanes {
        let base = rows.len();
        for row in 0..plan.rows() {
            rows.push(plan.row(row));
            let unit = plan.unit(row);
            units.push(base + unit.start..base + unit.end);
        }
    }
    let find = |source: RowSource| rows.iter().position(|&row| row == source);
    match (find(anchor), find(cursor)) {
        (Some(a), Some(b)) => {
            let start = units[a].start.min(units[b].start);
            let end = units[a].end.max(units[b].end);
            rows[start..end].to_vec()
        }
        (Some(found), None) | (None, Some(found)) => rows[units[found].clone()].to_vec(),
        (None, None) => vec![anchor],
    }
}

/// The row drawn at `offset` below the top of the rows, laid out as `boxes` (one per plan). A
/// point above or below every row takes the nearest one on screen.
fn row_at_height(lanes: &[LanePlan], boxes: &[LaneBox], offset: f32) -> Option<RowSource> {
    if let Some((index, row)) = stack::row_at(boxes, offset) {
        return lanes.get(index).map(|plan| plan.row(row));
    }
    let shown = || {
        lanes
            .iter()
            .zip(boxes)
            .filter(|(_, lane_box)| lane_box.shown > 0)
    };
    let (plan, first) = shown().next()?;
    if offset < first.top {
        return Some(plan.row(first.first));
    }
    let (plan, last) = shown().next_back()?;
    Some(plan.row(last.first + last.shown - 1))
}

/// Where `rows` are drawn in `audio`, laid out as `boxes`: one rectangle over those on screen,
/// or `None` when none is on screen.
fn rows_rect(
    lanes: &[LanePlan],
    boxes: &[LaneBox],
    audio: Rect,
    rows: &[RowSource],
) -> Option<Rect> {
    lanes
        .iter()
        .zip(boxes)
        .flat_map(|(plan, lane_box)| {
            lane_box
                .visible()
                .filter(move |&row| rows.contains(&plan.row(row)))
                .map(move |row| {
                    let top = audio.top() + lane_box.row_top(row).unwrap_or_default();
                    Rect::from_min_size(
                        Pos2::new(audio.left(), top),
                        Vec2::new(audio.width(), lane_box.row_height),
                    )
                })
        })
        .reduce(|a, b| a.union(b))
}

/// The rows a dragged-out or saved clip writes, empty for every row. While rows are edited
/// separately (`separate`) a selection writes its own rows (`selection`, empty when it covers
/// every row), and a drag-out without one (`selection` is `None`) the row it started in.
fn clip_rows(
    lanes: &[LanePlan],
    separate: bool,
    selection: Option<&[RowSource]>,
    drag_row: Option<RowSource>,
) -> Vec<RowSource> {
    if !separate {
        return Vec::new();
    }
    match selection {
        Some(rows) => rows.to_vec(),
        None => drag_row.map_or_else(Vec::new, |row| rows_between(lanes, row, row)),
    }
}

/// Where a drag across rows goes as the pointer reaches `offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowDrag {
    /// The row under the pointer, or the nearest one.
    To(RowSource),
    /// The pointer is past the top or bottom edge of the scrolling lane the drag is in, which
    /// has more rows that way: that lane (an index into the plans) moves on by a row.
    Scroll { lane: usize, down: bool },
}

/// What a drag across rows, now at `cursor`, does with the pointer at `offset`.
fn row_drag(
    lanes: &[LanePlan],
    boxes: &[LaneBox],
    cursor: RowSource,
    offset: f32,
) -> Option<RowDrag> {
    if let Some(index) = lanes.iter().position(|plan| plan.lane == cursor.lane())
        && let Some(lane_box) = boxes.get(index).filter(|lane_box| lane_box.scrolls())
    {
        if offset < lane_box.top && lane_box.first > 0 {
            return Some(RowDrag::Scroll {
                lane: index,
                down: false,
            });
        }
        if offset >= lane_box.top + lane_box.height
            && lane_box.first + lane_box.shown < lane_box.rows
        {
            return Some(RowDrag::Scroll {
                lane: index,
                down: true,
            });
        }
    }
    row_at_height(lanes, boxes, offset).map(RowDrag::To)
}

/// The rows a choice of rows names, by logical lane: what a selection writes and plays.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ChosenRows {
    /// The one lane the rows lie in, or `None` when they lie in both.
    lane: Option<usize>,
    /// By logical lane: the lane-native channels of the channel rows chosen, in order, or
    /// `None` for the whole lane. Empty when only the pair is chosen.
    native: [Option<Vec<usize>>; 2],
    /// By logical lane: whether the pair (either of its rows) is chosen.
    pair: [bool; 2],
}

/// What `rows` name, or `None` when no rows are named, which means every row.
fn chosen_rows(rows: &[RowSource]) -> Option<ChosenRows> {
    let lanes: Vec<usize> = (0..2)
        .filter(|&lane| rows.iter().any(|row| row.lane() == lane))
        .collect();
    let lane = match lanes.as_slice() {
        [] => return None,
        [lane] => Some(*lane),
        _ => None,
    };
    let in_lane = |lane: usize| rows.iter().filter(move |row| row.lane() == lane);
    let pair =
        std::array::from_fn(|lane| in_lane(lane).any(|row| matches!(row, RowSource::Pair(..))));
    let native: [Option<Vec<usize>>; 2] = std::array::from_fn(|lane| {
        in_lane(lane).next()?;
        let mut channels = Vec::new();
        for row in in_lane(lane) {
            match *row {
                RowSource::View(_, WaveformView::Left) => channels.push(0),
                RowSource::View(_, WaveformView::Right) => channels.push(1),
                // A merged row is the whole lane.
                RowSource::View(..) => return None,
                RowSource::Channel(_, channel) => channels.push(channel),
                RowSource::Pair(..) => {}
            }
        }
        channels.sort_unstable();
        channels.dedup();
        Some(channels)
    });
    Some(ChosenRows { lane, native, pair })
}

/// By logical lane: whether `rows` hold every row `lanes` draw of that lane. Such a lane is
/// written whole, as it is when rows are not separate.
fn lanes_chosen_whole(lanes: &[LanePlan], rows: &[RowSource]) -> [bool; 2] {
    std::array::from_fn(|lane| {
        lanes
            .iter()
            .find(|plan| plan.lane == lane)
            .is_some_and(|plan| {
                plan.rows() > 0 && (0..plan.rows()).all(|row| rows.contains(&plan.row(row)))
            })
    })
}

/// Takes this pass's lanes, `planned`, in place of `lanes`, and starts a lane whose group
/// changed at its first row again. Returns the lanes it replaced, or `None` when they stay.
/// While a pointer gesture on the waveform is in progress (`gesture`), every lane keeps the
/// rows it had when the gesture began; what changed meanwhile, such as an activity answer,
/// shows once it ends.
fn take_plans(
    lanes: &mut Vec<LanePlan>,
    first_row: &mut [usize; 2],
    planned: Vec<LanePlan>,
    gesture: bool,
) -> Option<Vec<LanePlan>> {
    if gesture || planned == *lanes {
        return None;
    }
    for plan in &planned {
        let same = lanes
            .iter()
            .find(|old| old.lane == plan.lane)
            .is_some_and(|old| match (&old.kind, &plan.kind) {
                (LaneKind::Stack { group: old, .. }, LaneKind::Stack { group: new, .. }) => {
                    old.id == new.id && old.channels == new.channels
                }
                _ => false,
            });
        if !same {
            first_row[plan.lane] = 0;
        }
    }
    Some(std::mem::replace(lanes, planned))
}

/// Carries the rows of a selection, `rows`, from lanes drawn as `old` to lanes drawn as `new`.
/// A lane the selection held whole (`whole`, by logical lane) stays whole as its rows appear
/// or go, and covers every row it draws now. Once it shows another group it keeps only the
/// rows it held, as every selection does across a group switch. Rows of other lanes stay
/// exactly as they are.
fn carry_rows(
    old: &[LanePlan],
    new: &[LanePlan],
    rows: &[RowSource],
    whole: &mut [bool; 2],
) -> Vec<RowSource> {
    fn kind(lanes: &[LanePlan], lane: usize) -> Option<&LaneKind> {
        lanes
            .iter()
            .find(|plan| plan.lane == lane)
            .map(|plan| &plan.kind)
    }
    for (lane, whole) in whole.iter_mut().enumerate() {
        let same = match (kind(old, lane), kind(new, lane)) {
            (
                Some(LaneKind::Stack { group: old, .. }),
                Some(LaneKind::Stack { group: new, .. }),
            ) => old.id == new.id && old.channels == new.channels,
            (Some(LaneKind::Views(_)), Some(LaneKind::Views(_))) => true,
            _ => false,
        };
        *whole &= same;
    }
    let mut carried = Vec::with_capacity(rows.len());
    for plan in new {
        if whole.get(plan.lane).copied().unwrap_or(false) && plan.rows() > 0 {
            carried.extend((0..plan.rows()).map(|row| plan.row(row)));
        } else {
            carried.extend(rows.iter().filter(|row| row.lane() == plan.lane));
        }
    }
    // Rows of a lane not drawn now stay as they were.
    carried.extend(
        rows.iter()
            .filter(|row| new.iter().all(|plan| plan.lane != row.lane())),
    );
    carried
}

/// What the export call writes of one wide or narrow snapshot, worked out on the export
/// worker. `groups` are all the lane's groups frozen with the audio, `shown` the group on show
/// when the clip was made, and `active` the lane's active channels in the frozen audio when the
/// lane hides silent rows and is written whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LaneWrite {
    /// Lane-native channels written, or `None` for every channel of `shown`.
    channels: Option<Vec<usize>>,
    pair: bool,
    /// The group the written channels are named by, and whose objects may move.
    shown: Option<LaneGroup>,
    /// The lane's Channels group, which the pair is made from.
    bed: Option<LaneGroup>,
}

fn lane_write(
    width: usize,
    choice: &RowChoice,
    groups: &[LaneGroup],
    shown: Option<&LaneGroup>,
    active: Option<u64>,
) -> LaneWrite {
    if width <= 2 {
        // One side of a stereo lane is written on its own; anything else is the whole lane.
        return LaneWrite {
            channels: choice
                .channels
                .clone()
                .filter(|channels| channels.len() == 1 && width == 2),
            ..LaneWrite::default()
        };
    }
    let fits = |group: &&LaneGroup| !group.channels.is_empty() && group.channels.end <= width;
    let bed = groups
        .iter()
        .filter(fits)
        .find(|group| !is_objects(group))
        .cloned();
    let (channels, pair, shown) = match &choice.channels {
        None => {
            let shown = shown.cloned();
            // A lane that hides silent rows writes what it shows: its active channels, or none
            // but the pair while nothing beyond the front left and right has sound.
            let channels = active.map(|active| {
                let group = shown
                    .clone()
                    .or_else(|| bed.clone())
                    .unwrap_or_else(|| numbered_group(width));
                shown_channels(&group, Some(active))
            });
            (channels, true, shown)
        }
        Some(native) => {
            // The rows are named by their own group, whatever is on show now: the objects
            // when any row is an object, else the group that holds them.
            let holding = |objects: bool| {
                groups.iter().filter(fits).find(|group| {
                    is_objects(group) == objects
                        && native
                            .iter()
                            .any(|channel| group.channels.contains(channel))
                })
            };
            let named = holding(true).or_else(|| holding(false)).or(shown).cloned();
            (Some(native.clone()), choice.pair, named)
        }
    };
    // A lane of objects only makes its pair from every channel, numbered, as its rows show.
    let bed = bed.or_else(|| {
        (pair && shown.as_ref().is_some_and(is_objects)).then(|| numbered_group(width))
    });
    LaneWrite {
        channels,
        pair,
        shown,
        bed,
    }
}

/// The least width of the plot, and height of a lane, that take the compact transport.
const TRANSPORT_MIN_WIDTH: f32 = 240.0;
const TRANSPORT_MIN_LANE_HEIGHT: f32 = 22.0;

/// A lane's compact transport: play or pause, and the position.
struct TransportChip {
    rect: Rect,
    addon_id: String,
    source_id: String,
    chip: addon_sources::Chip,
}

/// One button of a lane's channel group switch.
struct GroupChip {
    rect: Rect,
    lane: usize,
    /// The group a click shows.
    id: String,
    label: String,
    current: bool,
}

/// The window grew so a stacked lane's rows have room. It goes back to `before` when no lane
/// is stacked any more, unless the user has resized it since.
#[derive(Clone, Copy, Debug)]
struct WindowGrowth {
    before: f32,
    after: f32,
    /// Until then the window may still report its old size.
    settles: Instant,
}

/// The tallest a window grows by itself for stacked rows, in points.
const MAX_GROWN_WINDOW_HEIGHT: f32 = 560.0;

/// What the window does for its rows in one pass.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WindowFit {
    Stay,
    /// Grows to this height, so the rows of a stacked lane need not scroll.
    Grow(f32),
    /// Goes back to the height it had before it grew.
    Restore(f32),
}

/// What a window `window` points tall does for lanes laid out as `specs`, which have `current`
/// points of it, and what it then remembers in `growth`. It grows once when a lane starts
/// stacking rows, and again for rows that appear later while the user has not resized it
/// since it last grew; rows that go never shrink it. It goes back when no lane stacks any
/// more, unless the user resized it meanwhile. `cap` is the tallest it may grow to, asked only
/// when it grows. While the user drags the window (`window_drag`), or a pointer gesture on the
/// waveform is in progress (`gesture`), it stays as it is.
#[allow(clippy::too_many_arguments)]
fn fit_window(
    specs: &[stack::LaneRows],
    current: f32,
    window: f32,
    growth: &mut Option<WindowGrowth>,
    now: Instant,
    window_drag: bool,
    gesture: bool,
    cap: impl FnOnce() -> f32,
) -> WindowFit {
    if window_drag || gesture {
        return WindowFit::Stay;
    }
    let wanted = stack::comfortable_height(specs);
    let regrow = match (wanted, *growth) {
        (Some(wanted), Some(grown)) => {
            wanted > current + 0.5
                && (window - grown.after).abs() < 1.0
                && now >= grown.settles
                && grown.after < MAX_GROWN_WINDOW_HEIGHT - 0.5
        }
        _ => false,
    };
    match (wanted, *growth) {
        (Some(wanted), grown) if grown.is_none() || regrow => {
            let target = (window + wanted - current).min(cap()).max(window);
            if grown.is_some() && target <= window + 0.5 {
                return WindowFit::Stay;
            }
            *growth = Some(WindowGrowth {
                before: grown.map_or(window, |grown| grown.before),
                after: target,
                settles: now + Duration::from_secs(1),
            });
            if target > window + 0.5 {
                WindowFit::Grow(target)
            } else {
                WindowFit::Stay
            }
        }
        (None, Some(grown)) => {
            *growth = None;
            let untouched = (window - grown.after).abs() < 1.0 || now < grown.settles;
            if untouched && grown.after > grown.before + 0.5 {
                WindowFit::Restore(grown.before)
            } else {
                WindowFit::Stay
            }
        }
        _ => WindowFit::Stay,
    }
}

/// The meter's number for a lane. Loudness is not measured for lanes wider than stereo.
fn meter_value(mode: MeterMode, readings: LaneMeterReadings) -> Option<f32> {
    match mode {
        MeterMode::Dbfs => Some(readings.peak_dbfs),
        MeterMode::Vu => Some(readings.vu),
        MeterMode::Lufs => readings.momentary_lufs,
    }
}

struct FrozenEdit {
    snapshots: Arc<[FrozenTrack]>,
    lanes: Vec<usize>,
    bars: Vec<u64>,
    /// The audio that fills the plot's width: the history setting or, when wide lanes leave
    /// room for less, what memory holds.
    retention_seconds: f32,
    export_options: ExportOptions,
    /// Channel groups frozen with the audio, by logical lane: every group, not only the one on
    /// show.
    groups: [Vec<LaneGroup>; 2],
    shown_groups: [Option<String>; 2],
    /// Position tracks frozen with the audio, by logical lane.
    positions: [FrozenPositionTrack; 2],
    /// Which channels of each logical lane are active in the frozen audio, once known.
    activity: [Option<u64>; 2],
    /// The answer for `activity`, worked out off the window's thread.
    activity_job: Option<JoinHandle<[Option<u64>; 2]>>,
}

impl FrozenEdit {
    /// The rows `rows` choose, by snapshot (physical order): the lane-native channels of the
    /// channel rows and whether the pair is chosen. No rows choose every row of every lane.
    #[cfg_attr(not(test), allow(dead_code))]
    fn row_choice(&self, rows: &[RowSource]) -> [RowChoice; 2] {
        let chosen = chosen_rows(rows);
        std::array::from_fn(|index| match (chosen.as_ref(), self.lanes.get(index)) {
            (Some(chosen), Some(&lane)) if lane < 2 => RowChoice {
                channels: chosen.native[lane].clone(),
                pair: chosen.pair[lane],
            },
            _ => RowChoice::default(),
        })
    }

    /// Collects the frozen activity once it is worked out. True while it is still coming.
    fn poll_activity(&mut self) -> bool {
        if self
            .activity_job
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
            && let Some(job) = self.activity_job.take()
        {
            self.activity = job.join().unwrap_or_default();
        }
        self.activity_job.is_some()
    }

    fn track(&self, lane: usize) -> Option<&FrozenTrack> {
        self.lanes
            .iter()
            .position(|&logical| logical == lane)
            .and_then(|index| self.snapshots.get(index))
    }

    fn frames(&self) -> Range<u64> {
        self.snapshots
            .first()
            .map_or(0..0, FrozenTrack::frame_range)
    }
}

struct EditRequest {
    result: mpsc::Receiver<Option<EditSnapshot>>,
    generation: u64,
    retention_seconds: f32,
    saves: Vec<SnapshotSpec>,
}

#[derive(Clone, Debug, PartialEq)]
struct EditPreviewKey {
    frames: Range<u64>,
    /// The rows on screen, in drawing order.
    rows: Vec<RowSource>,
    /// By logical lane: what its pair rows are made from.
    pairs: [Option<PairGroup>; 2],
    bins: usize,
}

struct EditPreviewJob {
    key: EditPreviewKey,
    worker: JoinHandle<Vec<WaveformPeaks>>,
}

struct SnapshotRead {
    result: mpsc::Receiver<Option<EditSnapshot>>,
    saves: Vec<SnapshotSpec>,
}

struct SnapshotSpec {
    selection: Option<Range<u64>>,
    fades: Fades,
    output: PathBuf,
    format: ClipFormat,
    flags: Option<FlagUpdate>,
    export_options: ExportOptions,
    /// The rows saved, or none for every row.
    rows: Vec<RowSource>,
    /// By logical lane: `rows` hold every row the window drew of it, so it is saved whole.
    whole: [bool; 2],
}

/// How a clip's files are written: their format, each lane's channels and source name, and
/// which lanes are saved. Routes, names and `lane` count logical lanes, 0 for A and 1 for B.
#[derive(Clone, Debug, PartialEq)]
struct ClipFormat {
    profile: WavExportProfile,
    routes: [ChannelRoute; 2],
    force_mono: bool,
    /// What each lane's source is called in its file name.
    names: [String; 2],
    /// One lane of a two-lane recording, or `None` for every lane.
    lane: Option<usize>,
    /// The L/R pair of a wide lane is written binaural rather than as the stereo fold-down.
    pair_binaural: bool,
    /// By logical lane: the lane hides its silent rows, so written whole it writes only the
    /// channels active in its frozen audio, and the pair.
    hide_silent: [bool; 2],
}

impl ClipFormat {
    /// Every lane, with no source names.
    #[cfg(test)]
    fn unnamed(routes: [ChannelRoute; 2], force_mono: bool) -> Self {
        Self {
            profile: WavExportProfile::default(),
            routes,
            force_mono,
            names: Default::default(),
            lane: None,
            pair_binaural: false,
            hide_silent: [false; 2],
        }
    }
}

type RetiredValue = Box<dyn std::any::Any + Send>;

struct UiRetirement {
    sender: Option<mpsc::Sender<RetiredValue>>,
    worker: Option<JoinHandle<()>>,
}

impl UiRetirement {
    fn new() -> io::Result<Self> {
        let (sender, requests) = mpsc::channel::<RetiredValue>();
        let worker = thread::Builder::new()
            .name("backtracking-selection-cleanup".into())
            .spawn(move || {
                for retired in requests {
                    drop(retired);
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    fn retire<T: Send + 'static>(&self, value: T) -> Result<(), mpsc::SendError<RetiredValue>> {
        self.sender
            .as_ref()
            .expect("cleanup sender lives until shutdown")
            .send(Box::new(value))
    }
}

impl Drop for UiRetirement {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl SnapshotSpec {
    fn frozen_clip(mut self, snapshot: &EditSnapshot) -> Option<WavClip> {
        self.export_options.groups = snapshot_groups(snapshot);
        self.clip(
            Arc::clone(&snapshot.snapshots),
            &snapshot.lanes,
            &snapshot.groups,
        )
        .map(|clip| clip.with_positions(&snapshot.positions))
    }

    /// `groups` are the channel groups frozen with the audio, by logical lane.
    fn clip(
        mut self,
        snapshots: Arc<[FrozenTrack]>,
        lanes: &[usize],
        groups: &[Vec<LaneGroup>; 2],
    ) -> Option<WavClip> {
        if lanes.len() != snapshots.len() || lanes.iter().any(|&lane| lane > 1) {
            return None;
        }
        if let Some(flags) = &mut self.flags {
            flags.frames = snapshots.first()?.frame_range();
        }
        WavClip::new(
            snapshots,
            lanes,
            self.selection.as_ref(),
            self.fades,
            self.output,
            self.format,
            self.flags,
        )
        .and_then(|mut clip| {
            clip.export_options = self.export_options;
            clip.with_rows(&self.rows, groups)
        })
        .map(|clip| clip.whole_lanes(self.whole))
    }
}

fn physical_routes(routes: [ChannelRoute; 2], lanes: &[usize]) -> [ChannelRoute; 2] {
    std::array::from_fn(|index| {
        lanes
            .get(index)
            .and_then(|&lane| routes.get(lane))
            .copied()
            .unwrap_or(ChannelRoute::Merged)
    })
}

fn snapshot_groups(snapshot: &EditSnapshot) -> [Option<crate::controller::LaneGroup>; 2] {
    std::array::from_fn(|index| {
        let &lane = snapshot.lanes.get(index)?;
        if lane > 1 {
            return None;
        }
        snapshot.shown_groups[lane]
            .as_ref()
            .and_then(|id| snapshot.groups[lane].iter().find(|group| &group.id == id))
            .or_else(|| snapshot.groups[lane].first())
            .cloned()
    })
}

fn edit_preview(
    snapshots: &[FrozenTrack],
    lanes: &[usize],
    key: &EditPreviewKey,
) -> Vec<WaveformPeaks> {
    key.rows
        .iter()
        .map(|&row| {
            let mut peaks = WaveformPeaks::empty(key.bins);
            let Some(index) = lanes.iter().position(|&logical| logical == row.lane()) else {
                return peaks;
            };
            let Some(track) = snapshots.get(index) else {
                return peaks;
            };
            // The track answers whole peak blocks from the capture's peak cache and
            // clips the request to the frames it holds. Only rows on screen are asked for.
            match row {
                RowSource::View(_, view) => {
                    track.waveform(view, key.frames.clone(), &mut peaks.low, &mut peaks.high);
                }
                RowSource::Channel(_, channel) if channel < track.channels() => {
                    track.channel_waveform(
                        channel,
                        key.frames.clone(),
                        &mut peaks.low,
                        &mut peaks.high,
                    );
                }
                RowSource::Channel(..) => {}
                RowSource::Pair(lane, side) => {
                    if let Some(group) = key.pairs.get(lane).and_then(Option::as_ref) {
                        track.pair_waveform(
                            group,
                            side,
                            key.frames.clone(),
                            &mut peaks.low,
                            &mut peaks.high,
                        );
                    }
                }
            }
            peaks
        })
        .collect()
}

struct FlagUpdate {
    generation: u64,
    order: u64,
    frames: Range<u64>,
    /// The rows the flags were left on, which a selection taken back from them covers while
    /// rows are edited separately. Empty marks every row.
    rows: Vec<RowSource>,
    /// By logical lane: whether `rows` held every row of that lane when the flags were left.
    whole: [bool; 2],
}

fn snapshot_source(
    editing: Option<&Arc<[FrozenTrack]>>,
    pending: bool,
    live: impl FnOnce() -> Option<Arc<[FrozenTrack]>>,
) -> Option<Arc<[FrozenTrack]>> {
    if let Some(editing) = editing {
        Some(Arc::clone(editing))
    } else if pending {
        None
    } else {
        live()
    }
}

fn edit_snapshot_matches(snapshot: &EditSnapshot, generation: u64) -> bool {
    snapshot.generation == generation
        && snapshot.lanes.len() == snapshot.snapshots.len()
        && matches!(snapshot.lanes.as_slice(), [0] | [1] | [0, 1])
        && snapshot.snapshots.first().is_some_and(|first| {
            !first.frame_range().is_empty()
                && snapshot.snapshots.iter().all(|next| {
                    next.frame_range() == first.frame_range()
                        && next.sample_rate() == first.sample_rate()
                })
        })
}

fn frame_from_pointer(
    audio: Rect,
    x: f32,
    visible: Range<u64>,
    snap: bool,
    flags: &[u64],
    guides: &waveform::TimelineGuides,
) -> u64 {
    let width = f64::from(audio.width().max(1.0));
    let mut fraction = f64::from(x - audio.left()) / width;
    if snap {
        fraction = waveform::snap_to_visible_markers(
            fraction,
            visible.clone(),
            flags,
            guides,
            FLAG_SNAP_DISTANCE / width,
        );
    }
    let length = visible.end.saturating_sub(visible.start);
    visible
        .start
        .saturating_add(((length as f64 * fraction.clamp(0.0, 1.0)).round() as u64).min(length))
}

#[derive(Clone, Copy)]
enum WindowInteraction {
    Move,
    Resize(ResizeDirection),
}

impl WindowInteraction {
    fn cursor(self) -> CursorIcon {
        match self {
            Self::Move => CursorIcon::Move,
            Self::Resize(ResizeDirection::NorthWest | ResizeDirection::SouthEast) => {
                CursorIcon::ResizeNwSe
            }
            Self::Resize(ResizeDirection::NorthEast | ResizeDirection::SouthWest) => {
                CursorIcon::ResizeNeSw
            }
            Self::Resize(ResizeDirection::West | ResizeDirection::East) => {
                CursorIcon::ResizeHorizontal
            }
            Self::Resize(ResizeDirection::North | ResizeDirection::South) => {
                CursorIcon::ResizeVertical
            }
        }
    }
}

struct WindowDrag {
    interaction: WindowInteraction,
    press_position: Pos2,
    origin: Option<(POINT, RECT)>,
    released: bool,
}

impl WindowDrag {
    fn update(&mut self, window: &Window, pixels_per_point: f32, minimum: Vec2) -> Option<()> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return None;
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        if !self.released && unsafe { GetCapture() } != hwnd {
            return None;
        }

        let mut cursor = POINT::default();
        let mut current = RECT::default();
        // The window and mouse are on the event-loop thread. Screen coordinates preserve
        // the drag anchor as the client origin changes and can be negative across monitors.
        unsafe {
            GetCursorPos(&mut cursor).ok()?;
            GetWindowRect(hwnd, &mut current).ok()?;
        }
        let (start, bounds) = if let Some(origin) = self.origin {
            origin
        } else {
            if self.released {
                return None;
            }
            let inner = window.inner_position().ok()?;
            let start = POINT {
                x: inner.x + (self.press_position.x * pixels_per_point).round() as i32,
                y: inner.y + (self.press_position.y * pixels_per_point).round() as i32,
            };
            self.origin = Some((start, current));
            (start, current)
        };

        let mut flags = SWP_NOACTIVATE | SWP_NOZORDER;
        let inner = window.inner_size();
        let min_width = (minimum.x * pixels_per_point).ceil() as i32
            + (current.right - current.left - inner.width as i32).max(0);
        let min_height = (minimum.y * pixels_per_point).ceil() as i32
            + (current.bottom - current.top - inner.height as i32).max(0);
        let target = drag_bounds(
            self.interaction,
            bounds,
            start,
            cursor,
            min_width,
            min_height,
        );
        let changed = match self.interaction {
            WindowInteraction::Move => {
                flags |= SWP_NOSIZE;
                target.left != current.left || target.top != current.top
            }
            WindowInteraction::Resize(_) => target != current,
        };
        if changed {
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    target.left,
                    target.top,
                    target.right - target.left,
                    target.bottom - target.top,
                    flags,
                )
                .ok()?;
            }
        }
        (!self.released).then_some(())
    }
}

enum OptionsAction {
    SelectSaveFolder,
    GoToSaveFolder,
    SelectSnapshotFolder,
    OpenSnapshotFolder,
    PickAddonFile,
    /// Choose a file for this add-on to open.
    OpenAddonFile(String),
    OpenLink(String),
    Hide,
    Show,
    Quit,
    Close,
}

impl MeterMode {
    const OPTIONS: [Self; 3] = [Self::Dbfs, Self::Vu, Self::Lufs];

    fn description(self) -> &'static str {
        match self {
            Self::Dbfs => "Peak level",
            Self::Vu => "VU level",
            Self::Lufs => "Momentary loudness",
        }
    }
}

struct ExportJob {
    revision: u64,
    worker: JoinHandle<io::Result<ExportedWav>>,
}

struct WavClip {
    snapshots: Arc<[FrozenTrack]>,
    /// The logical lane, 0 for A and 1 for B, of each snapshot.
    lanes: Vec<usize>,
    frames: Range<usize>,
    fades: Fades,
    output: PathBuf,
    format: ClipFormat,
    flags: Option<FlagUpdate>,
    /// `groups` holds, by snapshot, the group on show when the clip was made.
    export_options: ExportOptions,
    /// By snapshot: every channel group frozen with the audio, not only the one on show.
    groups: [Vec<LaneGroup>; 2],
    /// By snapshot: the rows chosen, as lane-native channels and the pair.
    rows: [RowChoice; 2],
    /// By snapshot: the position tracks frozen with the audio, when the clip was made from a
    /// frozen view.
    positions: [Option<FrozenPositionTrack>; 2],
}

/// The rows of one lane a clip writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RowChoice {
    /// Lane-native channels of the channel rows chosen, in order; `None` for the whole lane.
    channels: Option<Vec<usize>>,
    /// Whether either pair row is chosen.
    pair: bool,
}

impl WavClip {
    fn new(
        snapshots: Arc<[FrozenTrack]>,
        lanes: &[usize],
        selection: Option<&Range<u64>>,
        fades: Fades,
        output: PathBuf,
        format: ClipFormat,
        flags: Option<FlagUpdate>,
    ) -> Option<Self> {
        if snapshots.len() > format.routes.len()
            || lanes.len() != snapshots.len()
            || lanes.iter().any(|&lane| lane >= format.routes.len())
            || format.lane.is_some_and(|lane| !lanes.contains(&lane))
        {
            return None;
        }
        let visible = snapshots.first()?.frame_range();
        let selected = selection.cloned().unwrap_or_else(|| visible.clone());
        if selected.is_empty() || selected.start < visible.start || selected.end > visible.end {
            return None;
        }
        let start = usize::try_from(selected.start - visible.start).ok()?;
        let end = usize::try_from(selected.end - visible.start).ok()?;
        Some(Self {
            snapshots,
            lanes: lanes.to_vec(),
            frames: start..end,
            fades,
            output,
            format,
            flags,
            export_options: ExportOptions::default(),
            groups: Default::default(),
            rows: Default::default(),
            positions: [None, None],
        })
    }

    /// Hands the clip the position tracks frozen with its audio, by logical lane.
    fn with_positions(mut self, positions: &[FrozenPositionTrack; 2]) -> Self {
        self.positions = std::array::from_fn(|index| {
            self.lanes
                .get(index)
                .and_then(|&lane| positions.get(lane))
                .cloned()
        });
        self
    }

    /// Writes only `rows`, or every row when there are none: only the lanes they lie in, and
    /// of a lane only the channels of its rows. `groups` are the channel groups frozen with
    /// the audio, by logical lane; the rows' own group names their files, even when another
    /// group is on show now.
    fn with_rows(mut self, rows: &[RowSource], groups: &[Vec<LaneGroup>; 2]) -> Option<Self> {
        self.groups = std::array::from_fn(|index| {
            self.lanes
                .get(index)
                .and_then(|&lane| groups.get(lane))
                .cloned()
                .unwrap_or_default()
        });
        let Some(chosen) = chosen_rows(rows) else {
            return Some(self);
        };
        if let Some(lane) = chosen.lane {
            if !self.lanes.contains(&lane) {
                return None;
            }
            self.format.lane = Some(lane);
        }
        for (index, &lane) in self.lanes.iter().enumerate() {
            let channels = chosen.native[lane].clone();
            let track = self.snapshots.get(index)?;
            if channels
                .as_ref()
                .is_some_and(|channels| channels.iter().any(|&channel| channel >= track.channels()))
            {
                return None;
            }
            self.rows[index] = RowChoice {
                channels,
                pair: chosen.pair[lane],
            };
        }
        Some(self)
    }

    /// Writes the lanes in `whole` (by logical lane) whole: their rows were every row drawn.
    fn whole_lanes(mut self, whole: [bool; 2]) -> Self {
        for (index, &lane) in self.lanes.iter().enumerate() {
            if whole.get(lane).copied().unwrap_or(false) {
                self.rows[index] = RowChoice::default();
            }
        }
        self
    }

    /// What snapshot `index` writes. Runs on the export worker: a lane that hides silent rows
    /// and is written whole asks its frozen audio which channels are active.
    fn lane_write(&self, index: usize) -> LaneWrite {
        let Some(track) = self.snapshots.get(index) else {
            return LaneWrite::default();
        };
        let choice = &self.rows[index];
        let active = (track.channels() > 2
            && choice.channels.is_none()
            && self
                .lanes
                .get(index)
                .is_some_and(|&lane| self.format.hide_silent.get(lane) == Some(&true)))
        .then(|| track.active_channels());
        lane_write(
            track.channels(),
            choice,
            &self.groups[index],
            self.export_options.groups[index].as_ref(),
            active,
        )
    }

    /// The snapshots written: every one, or the one of the lane saved on its own.
    fn saved(&self) -> Range<usize> {
        let one = self
            .format
            .lane
            .and_then(|lane| self.lanes.iter().position(|&held| held == lane));
        one.map_or(0..self.snapshots.len(), |index| index..index + 1)
    }

    fn write(self, keep: bool) -> io::Result<ExportedWav> {
        let format = &self.format;
        let saved = self.saved();
        // A lane saved on its own keeps its letter, so its file still says which lane it was.
        let labels: Vec<_> = self.lanes[saved.clone()]
            .iter()
            .map(|&lane| {
                let letter = (self.snapshots.len() == 2)
                    .then(|| Lane::from_index(lane))
                    .flatten();
                TakeLabel::new(format.names[lane].as_str(), letter)
            })
            .collect();
        let routes = physical_routes(format.routes, &self.lanes);
        // Everything is held by snapshot, so it moves along with the snapshots written: a
        // B-only save hands B's rows, groups and positions over in the first place.
        let writes: [LaneWrite; 2] = std::array::from_fn(|slot| {
            if saved.start + slot < saved.end {
                self.lane_write(saved.start + slot)
            } else {
                LaneWrite::default()
            }
        });
        let mut options = self.export_options.clone();
        options.groups = std::array::from_fn(|slot| writes[slot].shown.clone());
        let rows: [clip_export::RowExport<'_>; 2] = std::array::from_fn(|slot| {
            let write = &writes[slot];
            clip_export::RowExport {
                channels: write.channels.clone(),
                pair: write.pair,
                pair_binaural: format.pair_binaural,
                bed: write.bed.as_ref(),
                positions: self
                    .positions
                    .get(saved.start + slot)
                    .filter(|_| saved.start + slot < saved.end)
                    .and_then(Option::as_ref),
            }
        });
        let written = clip_export::export_rows(
            &self.output,
            &self.snapshots[saved.clone()],
            self.frames.clone(),
            self.fades,
            format.profile,
            &routes[saved],
            format.force_mono,
            &labels,
            &options,
            &rows,
        )?;
        let mut result = ExportedWav {
            paths: written.paths,
            objects_in_mono: written.objects_in_mono,
            keep,
            flags: self.flags,
        };
        if !keep {
            for path in &mut result.paths {
                *path = dunce::canonicalize(&path)?;
            }
        }
        Ok(result)
    }
}

struct ExportedWav {
    /// Every file written, the positions file included.
    paths: Vec<PathBuf>,
    /// Objects without positions had to be written in mono.
    objects_in_mono: bool,
    keep: bool,
    flags: Option<FlagUpdate>,
}

type WavPayload = Arc<std::sync::Mutex<Option<WavClip>>>;

fn wav_job(
    clip: WavClip,
    keep: bool,
) -> (
    WavPayload,
    impl FnOnce() -> io::Result<ExportedWav> + Send + 'static,
) {
    // The caller keeps a recovery reference until spawning succeeds. If the OS
    // rejects a new thread, dropping its closure cannot free the large audio on UI.
    let payload = Arc::new(std::sync::Mutex::new(Some(clip)));
    let work = Arc::clone(&payload);
    (payload, move || {
        let clip = work
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .expect("one WAV writer owns the payload");
        clip.write(keep)
    })
}

// The error hands the clip back so it is dropped off this thread; it is rare and short-lived.
#[allow(clippy::result_large_err)]
fn spawn_wav_write(
    name: &str,
    clip: WavClip,
    keep: bool,
) -> Result<JoinHandle<io::Result<ExportedWav>>, (io::Error, WavClip)> {
    let (payload, job) = wav_job(clip, keep);
    match thread::Builder::new().name(name.into()).spawn(job) {
        Ok(worker) => Ok(worker),
        Err(error) => {
            let clip = payload
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .expect("an unstarted WAV job still owns its payload");
            Err((error, clip))
        }
    }
}

impl ExportedWav {
    /// The status line after the files were saved: by the Save command (`snapshot`) or a
    /// drag. It counts the WAV files and the positions files apart, and says once when
    /// objects without positions were written in mono.
    fn saved_message(&self, language: Language, snapshot: bool) -> String {
        let positions = self
            .paths
            .iter()
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            })
            .count();
        let files = match self.paths.as_slice() {
            [file] => file.display().to_string(),
            [first, ..] => language.files_in(
                self.paths.len() - positions,
                positions,
                &first.parent().unwrap_or(first).display().to_string(),
            ),
            [] => String::new(),
        };
        let saved = language.saved(snapshot, &files);
        if self.objects_in_mono {
            language.with_objects_in_mono(&saved)
        } else {
            saved
        }
    }
}

impl Drop for ExportedWav {
    fn drop(&mut self) {
        // Native file drags need a readable path, which must remain stable after acceptance.
        if !self.keep {
            for path in &self.paths {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

impl Recorder {
    fn new(
        mut controller: Controller,
        preferences: Preferences,
        settings: Settings,
        primary: bool,
    ) -> Self {
        let group_error = controller.set_group_preferences(preferences.clone()).err();
        controller.set_retention(settings.history);
        controller.set_retain_silence(settings.retain_silence);
        controller.set_sources(settings.sources);
        controller.resume();
        let generation = controller.generation();
        let (preference_writer, mut preference_error) = if primary {
            match PreferenceWriter::new(preferences) {
                Ok(writer) => (Some(writer), String::new()),
                Err(error) => (None, format!("Could not start settings writer: {error}")),
            }
        } else {
            (None, String::new())
        };
        if let Some(error) = group_error {
            if !preference_error.is_empty() {
                preference_error.push_str("; ");
            }
            preference_error.push_str(&format!("Could not load channel group settings: {error}"));
        }
        let retirement = match UiRetirement::new() {
            Ok(retirement) => Some(retirement),
            Err(error) => {
                if !preference_error.is_empty() {
                    preference_error.push_str("; ");
                }
                preference_error.push_str(&format!("Could not start selection cleanup: {error}"));
                None
            }
        };
        Self {
            controller,
            preference_writer,
            retirement,
            primary,
            tray: None,
            tray_available: false,
            next_tray_attach: Instant::now(),
            launch_at_login: false,
            quitting: false,
            output: settings.output,
            output_cleanup: cleanup::Schedule::new(settings.output_cleanup, Instant::now()),
            saved_output: settings.saved_output,
            pinned: settings.pinned,
            pin_watch: None,
            next_pin_check: Instant::now(),
            controller_busy: false,
            waveform_only: settings.waveform_only,
            full_waveform_height: DEFAULT_WAVEFORM_HEIGHT,
            full_window_size: Vec2::new(610.0, 144.0),
            waveform_border: settings.waveform_border,
            waveform_border_width: settings.waveform_border_width,
            status_mark: settings.status_mark,
            show_title: settings.show_title,
            show_meters: settings.show_meters,
            waveform_border_color: settings.waveform_border_color.map(accent_color),
            waveform_border_follows_background: settings.waveform_border_follows_background,
            bar_lines: settings.bar_lines,
            second_lines: settings.second_lines,
            second_interval: settings.second_interval,
            options_open: false,
            options_in_taskbar: false,
            options_focus_requested: false,
            options_section: OptionsSection::Actions,
            addons_page: AddonsPage::default(),
            addon_sources: AddonSources::new(startup::test_instance().is_some()),
            options_test_position: None,
            available_sources: vec![CaptureSource::System, CaptureSource::DefaultMicrophone],
            source_list_worker: None,
            source_list_refresh_pending: false,
            source_menu: None,
            source_menu_outdated: false,
            updates: UpdateOptions::default(),
            window_position: settings.window_position,
            window_position_changed: None,
            idle_resume: settings.idle_resume,
            transparency: settings.transparency,
            accent: accent_color(settings.accent),
            saved_accent: accent_color(settings.accent),
            theme: Theme::default(),
            look: AddonLook::new(settings.addon_look),
            waveform_coloring: settings.waveform_coloring,
            saved_waveform_gradient: settings.waveform_gradient.clone(),
            waveform_gradient: settings.waveform_gradient,
            waveform_colors: WaveformColors::default(),
            meter_modes: [settings.meter_mode, settings.meter_b_mode],
            split_waveform: settings.split_waveform,
            wav_profile: settings.wav_profile,
            wav_force_mono: settings.wav_force_mono,
            wide_export: settings.wide_export,
            force_wide_export_menu: false,
            pair_render: settings.pair_render,
            hide_silent: settings.hide_silent_channels,
            activity: Default::default(),
            head_models: Vec::new(),
            source_names: settings.source_names,
            last_interaction: Instant::now(),
            recording_started: None,
            peaks: Vec::new(),
            peak_rows: Vec::new(),
            lanes: Vec::new(),
            lane_first_row: [0; 2],
            scroll_drag: None,
            lane_controls: Vec::new(),
            lane_scroll_zones: Vec::new(),
            growth: None,
            plot_rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(610.0, DEFAULT_WAVEFORM_HEIGHT)),
            editing: None,
            edit_request: None,
            edit_preview: None,
            edit_preview_key: None,
            edit_opening: None,
            pending_snapshot_reads: Vec::new(),
            pending_edit_save_reads: Vec::new(),
            export_requested: false,
            seconds: 0.0,
            sample_rate: 48_000,
            history_frames: 0..0,
            visible_frames: 0..0,
            zoom_frames: None,
            pan: None,
            bar_markers: Vec::new(),
            live_revision: None,
            next_refresh: Instant::now(),
            generation,
            selection: None,
            separate_rows: false,
            selection_rows: Vec::new(),
            selection_whole: [false; 2],
            drag_row: None,
            edge_scroll: None,
            lane_key: None,
            fades: Fades::NONE,
            preview: None,
            flags: None,
            flag_order: 0,
            gesture: None,
            window_drag: None,
            export: None,
            snapshot_save: None,
            pending_snapshot_saves: VecDeque::new(),
            hotkey: None,
            hotkey_error: None,
            hotkeys: settings.hotkeys,
            hotkeys_page: HotkeysPage::default(),
            snapshot_target: settings.snapshot_target,
            snapshot_targets: Vec::new(),
            snapshot_target_scan: Instant::now(),
            plugin_snapshot_request: None,
            pending_plugin_snapshots: VecDeque::new(),
            clip_revision: 0,
            exported: None,
            message: preference_error,
            test_copy: false,
            auto_grow: true,
            harness: None,
        }
    }

    /// Keeps what the Hotkeys page changed. It is in use already.
    fn save_hotkeys(&mut self) {
        self.save_preference(Key::Hotkeys, self.hotkeys.to_json());
    }

    fn save_preference(&mut self, key: Key, value: impl AsRef<str>) {
        if let Err(error) = self.write_preference(key, value.as_ref()) {
            self.message = format!("Could not save settings: {error}");
        }
    }

    fn write_preference(&self, key: Key, value: &str) -> io::Result<()> {
        if self.primary {
            self.preference_writer
                .as_ref()
                .ok_or_else(|| io::Error::other("settings writer is unavailable"))?
                .save(key, value)
        } else {
            Ok(())
        }
    }

    fn set_launch_at_login(&mut self, enabled: bool, frame: &eframe::Frame) {
        if self.test_copy {
            self.message = Language::current().test_copy_startup().into();
            return;
        }
        match startup::set_enabled(enabled) {
            Ok(()) => self.launch_at_login = enabled,
            Err(error) => {
                self.message = format!("Could not update startup setting: {error}");
                if self.waveform_only
                    || frame.winit_window().and_then(|window| window.is_visible()) == Some(false)
                {
                    rfd::MessageDialog::new()
                        .set_title("Backtracking")
                        .set_description(&self.message)
                        .set_level(rfd::MessageLevel::Error)
                        .set_parent(frame)
                        .show();
                }
            }
        }
    }

    fn hide(&mut self, ctx: &egui::Context) {
        // Nothing would show the preview or stop it.
        self.stop_preview();
        self.options_open = false;
        self.window_drag = None;
        if self.gesture.take().is_some() {
            self.clear_snapshot();
        }
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    fn attach_tray(&mut self, window: &Window, ctx: &egui::Context) {
        self.next_tray_attach = Instant::now() + Duration::from_secs(2);
        match Tray::attach(window, ctx, self.primary) {
            Ok(tray) => {
                self.tray_available = tray.is_active();
                self.tray = Some(tray);
            }
            Err(error) => self.message = format!("System tray unavailable: {error}"),
        }
        if !self.options_in_taskbar {
            tray::suppress_taskbar(window);
        }
    }

    fn update_tray(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        // Diagnostic taskbar windows remain directly targetable. Tray subclassing
        // otherwise restores WS_EX_TOOLWINDOW whenever winit updates native styles.
        // A test copy has no tray icon at all.
        if self.options_in_taskbar || self.test_copy {
            return;
        }
        if self.tray.is_none()
            && Instant::now() >= self.next_tray_attach
            && let Some(window) = frame.winit_window()
        {
            self.attach_tray(window, ctx);
        }
        let Some(tray) = &self.tray else { return };
        tray.refresh();
        let available = tray.is_active();
        let action = tray.take_action();
        if tray.take_taskbar_recheck()
            && !self.options_in_taskbar
            && let Some(window) = frame.winit_window()
        {
            tray::suppress_taskbar(window);
        }
        if available != self.tray_available {
            self.tray_available = available;
            if !available
                && frame
                    .winit_window()
                    .is_some_and(|window| window.is_visible() == Some(false))
            {
                ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            }
        }
        match action {
            Some(tray::Action::Show) => self.show(ctx),
            Some(tray::Action::Hide) => self.hide(ctx),
            Some(tray::Action::Options) => {
                if frame
                    .winit_window()
                    .is_some_and(|window| window_is_hidden(window))
                {
                    self.show(ctx);
                }
                self.open_options(ctx);
            }
            Some(tray::Action::ToggleStartup) => {
                if self.primary {
                    let enabled = startup::enabled().unwrap_or(self.launch_at_login);
                    self.set_launch_at_login(!enabled, frame);
                }
            }
            Some(tray::Action::Quit) => self.quit(ctx),
            None => {}
        }
        if self.tray_available
            && !self.quitting
            && ctx.input(|input| input.viewport().close_requested())
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide(ctx);
        }
    }

    fn save_accent(&mut self) {
        if self.accent != self.saved_accent {
            let [r, g, b, _] = self.accent.to_array();
            self.save_preference(Key::Accent, format!("#{r:02X}{g:02X}{b:02X}"));
            self.saved_accent = self.accent;
        }
    }

    fn observe_window_position(&mut self, window: &Window, now: Instant) {
        if window.is_visible() == Some(false) {
            self.save_window_position();
            return;
        }
        if window.is_minimized() == Some(true)
            || window.is_maximized()
            || window.fullscreen().is_some()
        {
            return;
        }
        if let Ok(position) = window.outer_position() {
            let position = [position.x, position.y];
            if self.window_position != Some(position) {
                self.window_position = Some(position);
                self.window_position_changed = Some(now);
            }
        }
        if self
            .window_position_changed
            .is_some_and(|changed| now.duration_since(changed) >= WINDOW_POSITION_SAVE_DELAY)
        {
            self.save_window_position();
        }
    }

    fn save_window_position(&mut self) {
        if self.window_position_changed.take().is_some()
            && let Some([x, y]) = self.window_position
        {
            self.save_preference(Key::WindowPosition, format!("{x} {y}"));
        }
    }

    fn resume(&mut self) {
        self.controller.resume();
        self.sync_generation();
        self.last_interaction = Instant::now();
    }

    fn freeze(&mut self) {
        self.controller.pause();
        self.sync_generation();
        self.last_interaction = Instant::now();
        self.live_revision = None;
        self.next_refresh = Instant::now();
        self.refresh_waveform();
    }

    fn dual_source(&self) -> bool {
        self.controller.sources()[1].is_some()
    }

    /// How many rows the lanes draw, on screen or scrolled out.
    fn row_count(&self) -> usize {
        self.lanes.iter().map(LanePlan::rows).sum()
    }

    /// Whether rows are edited on their own now: the user asked for it and the window shows
    /// more than one row.
    fn rows_separate(&self) -> bool {
        self.separate_rows && self.row_count() > 1
    }

    /// The rows the selection covers, or `None` when it covers every row.
    fn selected_rows(&self) -> Option<&[RowSource]> {
        (self.rows_separate() && !self.selection_rows.is_empty())
            .then_some(self.selection_rows.as_slice())
    }

    /// The row under `y` of `audio` and the rows selected with it, while rows are separate.
    fn rows_at(&self, audio: Rect, y: f32) -> Option<Vec<RowSource>> {
        let boxes = self.lane_boxes(audio.height());
        let row = row_at_height(&self.lanes, &boxes, y - audio.top())?;
        Some(rows_between(&self.lanes, row, row))
    }

    /// Right Ctrl or the menu: rows are edited on their own, or all together again. A
    /// selection stays where it is and narrows to the rows its drag crossed, or widens to all.
    fn toggle_separate_rows(&mut self) {
        self.last_interaction = Instant::now();
        if self.gesture.is_some() {
            // A selection or a drag in progress keeps the rows it started with.
            return;
        }
        let language = Language::current();
        if self.row_count() < 2 {
            self.message = language.separate_needs_rows().into();
            return;
        }
        self.separate_rows = !self.separate_rows;
        // What plays, or waits to be dragged out, covered the rows the old way.
        self.stop_preview();
        self.invalidate_export();
        self.message = language.editing_separately(self.separate_rows).into();
    }

    /// A short name for one row: its channel label, L or R, or its lane's letter.
    fn row_name(&self, row: RowSource) -> String {
        match row {
            RowSource::View(_, WaveformView::Left) | RowSource::Pair(_, PairSide::Left) => {
                "L".into()
            }
            RowSource::View(_, WaveformView::Right) | RowSource::Pair(_, PairSide::Right) => {
                "R".into()
            }
            RowSource::View(lane, _) => lane_name(lane).into(),
            RowSource::Channel(lane, channel) => {
                let edit_groups = self.editing.as_ref().and_then(|edit| edit.groups.get(lane));
                let groups =
                    edit_groups.map_or_else(|| self.controller.lane_groups(lane), Vec::as_slice);
                let shown = self.lanes.iter().find_map(|plan| match &plan.kind {
                    LaneKind::Stack { group, .. } if plan.lane == lane => Some(group),
                    _ => None,
                });
                shown
                    .into_iter()
                    .chain(groups)
                    .find(|group| group.channels.contains(&channel))
                    .and_then(|group| group.labels.get(channel - group.channels.start))
                    .map(|label| stack::short_label(label))
                    .filter(|label| !label.is_empty())
                    .unwrap_or_else(|| (channel + 1).to_string())
            }
        }
    }

    /// What the selection's label and the status line call `rows`: one row, or the first and
    /// last. The two pair rows are always chosen together and are called "L/R", which a
    /// channel labelled L or R beside them cannot be mistaken for. With two sources a
    /// channel's name starts with its lane's letter.
    fn rows_name(&self, rows: &[RowSource]) -> String {
        rows_label(rows, self.dual_source(), |row| self.row_name(row))
    }

    fn current_snapshots(&self) -> Option<Arc<[FrozenTrack]>> {
        snapshot_source(
            self.editing.as_ref().map(|edit| &edit.snapshots),
            self.edit_request.is_some(),
            || None,
        )
    }

    fn retire<T: Send + 'static>(&mut self, value: T) {
        if let Some(retirement) = &self.retirement {
            if let Err(error) = retirement.retire(value) {
                self.message = "Selection cleanup worker stopped unexpectedly.".into();
                drop(error.0);
            }
        } else {
            // Thread creation failure was surfaced at startup. Keep the app usable.
            drop(value);
        }
    }

    fn accept_edit_snapshot(
        &mut self,
        snapshot: EditSnapshot,
        generation: u64,
    ) -> Option<EditSnapshot> {
        if edit_snapshot_matches(&snapshot, generation) {
            Some(snapshot)
        } else {
            self.retire(snapshot);
            None
        }
    }

    fn begin_editing(&mut self) {
        if self.editing.is_some() || self.edit_request.is_some() {
            return;
        }
        self.edit_request = Some(EditRequest {
            result: self.controller.request_edit_snapshot(),
            generation: self.generation,
            retention_seconds: self.plot_retention_seconds(),
            saves: Vec::new(),
        });
        self.edit_opening = Some((Instant::now(), None));
        self.live_revision = None;
        self.next_refresh = Instant::now();
    }

    fn discard_editing(&mut self) {
        // The preview plays the editing snapshot, so it ends with it.
        self.stop_preview();
        if let Some(request) = self.edit_request.take() {
            if !request.saves.is_empty() {
                self.pending_edit_save_reads.push(request);
            } else {
                self.retire(request);
            }
            self.zoom_frames = None;
        }
        if let Some(edit) = self.editing.take() {
            self.retire(edit);
            self.zoom_frames = None;
        }
        // Dropping a JoinHandle detaches the immutable preview calculation. No UI wait.
        self.edit_preview = None;
        self.edit_preview_key = None;
        self.export_requested = false;
    }

    fn collect_editing(&mut self) {
        let result =
            self.edit_request
                .as_ref()
                .and_then(|request| match request.result.try_recv() {
                    Ok(snapshots) => Some(snapshots),
                    Err(mpsc::TryRecvError::Disconnected) => Some(None),
                    Err(mpsc::TryRecvError::Empty) => None,
                });
        let Some(snapshots) = result else {
            return;
        };
        let request = self.edit_request.take().expect("edit request was observed");
        let Some(snapshot) =
            snapshots.and_then(|snapshot| self.accept_edit_snapshot(snapshot, request.generation))
        else {
            self.selection = None;
            self.gesture = None;
            self.export_requested = false;
            self.message = "No audio available for this selection.".into();
            return;
        };
        let export_options = ExportOptions {
            groups: snapshot_groups(&snapshot),
            wide_mode: self.wide_export,
            addons: self.controller.addons().cloned(),
        };
        for save in request.saves {
            if let Some(clip) = save.frozen_clip(&snapshot) {
                self.pending_snapshot_saves.push_back(clip);
            }
        }
        let groups = snapshot.groups;
        let shown_groups = snapshot.shown_groups;
        let snapshots = snapshot.snapshots;
        let activity_job = frozen_activity(
            &snapshots,
            &snapshot.lanes,
            [0, 1].map(|lane| self.hides_silent_rows(lane)),
        );
        let frames = snapshots[0].frame_range();
        if let Some(selection) = &mut self.selection {
            selection.start = selection.start.clamp(frames.start, frames.end);
            selection.end = selection.end.clamp(selection.start, frames.end);
        }
        if let Some(Gesture::Select { anchor, cursor, .. }) = &mut self.gesture {
            *anchor = (*anchor).clamp(frames.start, frames.end);
            *cursor = (*cursor).clamp(frames.start, frames.end);
        }
        self.editing = Some(FrozenEdit {
            snapshots,
            lanes: snapshot.lanes,
            bars: snapshot.bar_markers,
            retention_seconds: request.retention_seconds,
            export_options,
            groups,
            shown_groups,
            positions: snapshot.positions,
            activity: [None; 2],
            activity_job,
        });
        if let Some((_, arrived)) = &mut self.edit_opening {
            *arrived = Some(Instant::now());
        }
        if self.message == PREPARING_SELECTION {
            self.message.clear();
        }
        self.next_refresh = Instant::now();
        self.live_revision = None;
        if self.export_requested {
            self.start_export();
        }
    }

    fn sync_generation(&mut self) {
        let generation = self.controller.generation();
        if generation != self.generation {
            self.generation = generation;
            self.flags = None;
            self.gesture = None;
            self.zoom_frames = None;
            self.pan = None;
            self.bar_markers.clear();
            self.clear_snapshot();
            // Never leave the old source's pixels interactive while the new
            // capture worker is still busy preparing its first render.
            self.history_frames = 0..0;
            self.visible_frames = 0..0;
            self.seconds = 0.0;
            for peaks in &mut self.peaks {
                peaks.low.fill(0.0);
                peaks.high.fill(0.0);
            }
            self.lane_first_row = [0; 2];
            self.scroll_drag = None;
        }
    }

    fn flag_update(
        &mut self,
        frames: Range<u64>,
        rows: Vec<RowSource>,
        whole: [bool; 2],
    ) -> FlagUpdate {
        self.flag_order += 1;
        FlagUpdate {
            generation: self.generation,
            order: self.flag_order,
            frames,
            rows,
            whole,
        }
    }

    /// Makes `rows` the rows the selection covers, and notes which lanes they hold whole.
    fn choose_rows(&mut self, rows: Vec<RowSource>) {
        self.selection_whole = lanes_chosen_whole(&self.lanes, &rows);
        self.selection_rows = rows;
    }

    fn apply_flags(&mut self, flags: FlagUpdate) {
        if flags.generation == self.generation
            && self
                .flags
                .as_ref()
                .is_none_or(|current| current.order < flags.order)
        {
            self.flags = Some(flags);
        }
    }

    fn clear_snapshot(&mut self) {
        self.reset_snapshot();
        self.invalidate_export();
    }

    fn reset_snapshot(&mut self) {
        self.selection = None;
        self.fades = Fades::NONE;
        self.discard_editing();
        self.live_revision = None;
        self.next_refresh = Instant::now();
    }

    fn finish_waveform_gesture(&mut self, keep_selection: bool) {
        self.last_interaction = Instant::now();
        if !keep_selection {
            self.clear_snapshot();
        }
    }

    fn clear_capture(&mut self) {
        self.controller.clear();
        self.sync_generation();
        self.flags = None;
        self.clear_snapshot();
        self.message.clear();
        self.last_interaction = Instant::now();
    }

    fn cancel_selection(&mut self) {
        self.gesture = None;
        self.clear_snapshot();
        self.message.clear();
        self.sync_generation();
        self.last_interaction = Instant::now();
    }

    /// Space with a selection: plays it from its start flag to its end flag, or stops it.
    fn toggle_preview(&mut self, ctx: &egui::Context) {
        self.last_interaction = Instant::now();
        if self.preview.is_some() {
            self.stop_preview();
            return;
        }
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let Some(edit) = self.editing.as_ref() else {
            self.message = if self.edit_request.is_some() {
                PREPARING_SELECTION
            } else {
                "The selected audio is unavailable."
            }
            .into();
            return;
        };
        // Selected rows play the lanes they lie in, and of those only their own channels.
        let plan = preview_plan(
            &edit.snapshots,
            &edit.lanes,
            &edit.groups,
            &edit.shown_groups,
            self.routes(),
            self.selected_rows(),
        );
        let preview = plan.and_then(|parts| self.preview_of(&parts, selection));
        let preview = match preview {
            Ok(preview) => preview,
            Err(reason) => {
                self.message = reason.into();
                return;
            }
        };
        let repaint = ctx.clone();
        match PreviewPlayer::start(preview, Arc::new(move || repaint.request_repaint())) {
            Ok(player) => self.preview = Some(player),
            Err(error) => self.message = error,
        }
    }

    /// The preview of `parts` of the frozen view over `frames`. Mono and stereo lanes played
    /// whole take the preview they always did; anything else takes the one that renders the
    /// pair and plays chosen channels and moving objects as the export writes them.
    fn preview_of(
        &mut self,
        parts: &[PreviewPart],
        frames: Range<u64>,
    ) -> Result<Preview, &'static str> {
        let edit = self.editing.as_ref().ok_or(SELECTION_UNAVAILABLE)?;
        let (first, last) = match (parts.first(), parts.last()) {
            (Some(first), Some(last)) => (first.index, last.index),
            _ => return Err(SELECTION_UNAVAILABLE),
        };
        if parts
            .iter()
            .all(|part| part.group.is_none() && part.selected.is_none())
        {
            let tracks = edit
                .snapshots
                .get(first..=last)
                .ok_or(SELECTION_UNAVAILABLE)?;
            let routes: Vec<_> = parts.iter().map(|part| part.route).collect();
            return Preview::new(tracks, frames, &routes, self.wav_force_mono, self.fades)
                .ok_or(SELECTION_UNAVAILABLE);
        }
        let positions: Vec<Option<&FrozenPositionTrack>> = parts
            .iter()
            .map(|part| {
                let lane = *edit.lanes.get(part.index)?;
                part.positions.then(|| edit.positions.get(lane)).flatten()
            })
            .collect();
        // A binaural pair, or objects that move, need the head model of the lane's rate.
        let binaural = self.pair_render.binaural();
        let rate = parts.iter().find_map(|part| {
            let track = edit.snapshots.get(part.index)?;
            let moving = positions
                .iter()
                .zip(parts)
                .filter(|(_, other)| other.index == part.index)
                .any(|(positions, part)| {
                    positions.is_some_and(|positions| moves_in(part, positions, &frames))
                });
            (part.group.is_some() && (binaural || moving)).then(|| track.sample_rate())
        });
        let snapshots = Arc::clone(&edit.snapshots);
        let positions: Vec<Option<FrozenPositionTrack>> =
            positions.into_iter().map(|track| track.cloned()).collect();
        let model = match rate {
            Some(rate) => Some(self.head_model(rate).ok_or(SELECTION_UNAVAILABLE)?),
            None => None,
        };
        let lanes: Vec<PreviewLane<'_>> = parts
            .iter()
            .map(|part| {
                Some(PreviewLane {
                    track: snapshots.get(part.index)?,
                    route: part.route,
                    group: part.group.as_ref(),
                    selected: part.selected.as_deref(),
                })
            })
            .collect::<Option<_>>()
            .ok_or(SELECTION_UNAVAILABLE)?;
        let positions: [Option<&FrozenPositionTrack>; 2] =
            std::array::from_fn(|index| positions.get(index).and_then(Option::as_ref));
        Preview::with_positions(
            &lanes,
            frames,
            model,
            self.wav_force_mono,
            self.fades,
            &positions,
        )
        .ok_or(SELECTION_UNAVAILABLE)
    }

    /// The head model for binaural preview at `rate`, made the first time it is needed.
    fn head_model(&mut self, rate: u32) -> Option<Arc<HeadModel>> {
        if let Some(model) = self
            .head_models
            .iter()
            .find(|model| model.sample_rate() == rate)
        {
            return Some(Arc::clone(model));
        }
        let model = Arc::new(HeadModel::new(rate).ok()?);
        self.head_models.push(Arc::clone(&model));
        Some(model)
    }

    fn stop_preview(&mut self) {
        if let Some(player) = self.preview.take() {
            player.preview().stop();
            // Its share of the audio is released off this thread.
            self.retire(player);
        }
    }

    /// Lets go of a preview that has played to its end or failed.
    fn collect_preview(&mut self) {
        if self.preview.as_ref().is_some_and(PreviewPlayer::is_done) {
            let player = self
                .preview
                .take()
                .expect("a finished preview was observed");
            if let Some(error) = player.take_error() {
                self.message = error;
            }
            self.retire(player);
        }
    }

    /// The ramp whose flag a press at `position` picks up, while the selection is on screen.
    fn fade_flag(&self, audio: Rect, position: Pos2) -> Option<FadeEnd> {
        let selection = self.selection.as_ref()?;
        if self.editing.is_none() || self.history_frames.is_empty() || audio.width() <= 1.0 {
            return None;
        }
        let area = self.flag_area(audio, self.selected_rows().unwrap_or_default());
        fade_end_at(
            area,
            self.visible_frames.clone(),
            selection,
            position,
            FLAG_REACH,
        )
    }

    /// Where `rows` are drawn in `audio`, as one rectangle over those on screen, or `None` when
    /// none is on screen.
    fn rows_rect(&self, audio: Rect, rows: &[RowSource]) -> Option<Rect> {
        rows_rect(&self.lanes, &self.lane_boxes(audio.height()), audio, rows)
    }

    /// Where the flags of a selection of `rows` stand: on top of the rows while rows are
    /// separate, so their fade handles sit on what they fade, else on top of the plot.
    fn flag_area(&self, audio: Rect, rows: &[RowSource]) -> Rect {
        if !self.rows_separate() || rows.is_empty() {
            return audio;
        }
        self.rows_rect(audio, rows).unwrap_or(audio)
    }

    /// Draws the plot with `theme` from the next frame on. The user's own choices of accent,
    /// border and transparency keep applying where the theme leaves them.
    fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Keeps the waveform-only window's plot the same height when the user's own border
    /// width has changed from `old_window_inset` ([`Self::window_border_inset`]).
    fn fit_border_change(&self, ctx: &egui::Context, old_window_inset: f32) {
        fit_window_border(
            ctx,
            self.waveform_only,
            old_window_inset,
            self.window_border_inset(),
        );
    }

    /// The border the waveform-only window's size makes room for: the user's own width. A
    /// look's width is drawn as it is and the plot takes up the difference, so a look never
    /// resizes the window.
    fn window_border_inset(&self) -> f32 {
        window_border_inset(self.waveform_border, self.waveform_border_width)
    }

    /// The border drawn around the plot: a chosen look's width, else the user's own.
    fn border_inset(&self) -> f32 {
        if self.waveform_border {
            self.theme.border_width(self.waveform_border_width)
        } else {
            0.0
        }
    }

    fn invalidate_export(&mut self) {
        self.clip_revision = self.clip_revision.wrapping_add(1);
        self.exported = None;
        if self.message == PREPARING_WAV {
            self.message.clear();
        }
    }

    fn refresh_waveform(&mut self) {
        if self.edit_request.is_some() {
            return;
        }
        if self.editing.is_some() {
            self.refresh_edit_preview();
            return;
        }
        let mut revision = self.controller.revision();
        let now = Instant::now();
        let bins = self
            .display_policy(self.audio_rect(self.plot_rect), self.visible_frames.clone())
            .bins;
        // Scrolling a lane or switching its group asks for other rows.
        let rows = self.visible_rows();
        let resolution_changed =
            self.peak_rows != rows || self.peaks.iter().any(|peaks| peaks.low.len() != bins);
        if !resolution_changed && (self.live_revision == Some(revision) || now < self.next_refresh)
        {
            return;
        }

        // Rows already on screen keep their old peaks until the new ones are in.
        let mut peaks: Vec<WaveformPeaks> = rows
            .iter()
            .map(|row| {
                self.peak_rows
                    .iter()
                    .position(|old| old == row)
                    .and_then(|index| self.peaks.get(index).cloned())
                    .unwrap_or_else(|| WaveformPeaks::empty(bins))
            })
            .collect();
        for peak in &mut peaks {
            peak.low.resize(bins, 0.0);
            peak.high.resize(bins, 0.0);
        }
        let rendered = {
            let (mut channels, mut channel_traces) = (Vec::new(), Vec::new());
            let (mut views, mut view_traces) = (Vec::new(), Vec::new());
            for (peak, row) in peaks.iter_mut().zip(&rows) {
                let trace = (peak.low.as_mut_slice(), peak.high.as_mut_slice());
                match *row {
                    RowSource::Channel(lane, channel) => {
                        channels.push((lane, ChannelWaveform::Native(channel)));
                        channel_traces.push(trace);
                    }
                    RowSource::Pair(lane, side) => {
                        // A pair row is only planned with its group; one without stays flat.
                        if let Some(group) = self.lane_pair(lane) {
                            channels.push((lane, ChannelWaveform::Pair(group.clone(), side)));
                            channel_traces.push(trace);
                        }
                    }
                    RowSource::View(lane, view) => {
                        views.push((lane, view));
                        view_traces.push(trace);
                    }
                }
            }
            // Every row in a mixed view reads the same frozen audio and revision. The first
            // draw of the pair over a long history takes the worker a while; until it is in,
            // this returns nothing and the rows keep their last picture.
            if !channels.is_empty() {
                self.controller
                    .try_render_rows(
                        &channels,
                        &views,
                        self.zoom_frames.clone(),
                        &mut channel_traces,
                        &mut view_traces,
                    )
                    .map(|(history, visible, rate, bars, drawn_revision)| {
                        revision = drawn_revision;
                        (history, visible, rate, bars)
                    })
            } else {
                self.controller.try_render_waveforms(
                    &views,
                    self.zoom_frames.clone(),
                    &mut view_traces,
                )
            }
        };
        let Some((history, visible, rate, bars)) = rendered else {
            self.next_refresh = now + Duration::from_millis(16);
            return;
        };
        self.peaks = peaks;
        self.peak_rows = rows;
        self.history_frames = history;
        self.visible_frames = visible;
        self.sample_rate = rate;
        self.bar_markers = bars;
        self.seconds = (self.visible_frames.end - self.visible_frames.start) as f32
            / self.sample_rate.max(1) as f32;
        let selection_expired = self.selection.as_mut().is_some_and(|range| {
            range.start = range.start.max(self.history_frames.start);
            range.end = range.end.min(self.history_frames.end);
            range.is_empty()
        });
        if selection_expired {
            self.selection = None;
            self.last_interaction = now;
        }
        self.live_revision = Some(revision);
        self.next_refresh = now + WAVEFORM_REFRESH;
    }

    fn is_fine_editing(&self) -> bool {
        self.gesture.is_some()
            || self.pan.is_some()
            || self.last_interaction.elapsed() < FINE_EDIT_SETTLE
    }

    /// Keeps a pinned window above other topmost windows. The check runs on a
    /// native timer, so it does not need a repaint.
    fn watch_pin(&mut self, frame: &eframe::Frame, now: Instant) {
        // A hidden window cannot be covered. Showing it runs a pass that re-arms this.
        let wanted = self.pinned
            && !self.options_open
            && !frame
                .winit_window()
                .is_some_and(|window| window_is_hidden(window));
        if wanted && self.pin_watch.is_none() {
            self.pin_watch = frame
                .winit_window()
                .and_then(|window| window_order::PinWatch::attach(window));
        }
        if let Some(watch) = &mut self.pin_watch {
            watch.set_active(wanted);
        } else if wanted && now >= self.next_pin_check {
            // Without the timer, look whenever the window runs anyway.
            let attempted = frame
                .winit_window()
                .is_some_and(|window| window_order::raise_if_covered(window));
            self.next_pin_check = now + if attempted { PIN_RETRY } else { REFRESH };
        }
    }

    fn waits(&self, ctx: &egui::Context, frame: &eframe::Frame) -> Waits {
        let now = Instant::now();
        let window = frame.winit_window();
        let hidden = window.is_some_and(|window| window_is_hidden(window));
        let meter_shown = |lane: usize| {
            self.controller
                .lane_meter_readings(lane)
                .and_then(|readings| meter_value(self.meter_modes[lane], readings))
                .is_some_and(f32::is_finite)
        };
        let live = self.editing.is_none() && self.edit_request.is_none();
        Waits {
            hidden,
            busy: self.controller_busy
                || self.window_drag.is_some()
                || self.gesture.is_some()
                || self.pan.is_some()
                || self.export.is_some()
                || self.edit_request.is_some()
                || self.edit_preview.is_some()
                || !self.pending_snapshot_reads.is_empty()
                || !self.pending_edit_save_reads.is_empty()
                || self.snapshot_save.is_some()
                || !self.pending_snapshot_saves.is_empty()
                || self.plugin_snapshot_request.is_some()
                || !self.pending_plugin_snapshots.is_empty()
                // The playhead moves every frame.
                || self.preview.is_some(),
            // Add-on manager replies arrive without a repaint of their own.
            polling: self.source_list_worker.is_some()
                || self.addons_page.waiting()
                || self.addon_sources.waiting()
                // The frozen activity says which rows a frozen view shows.
                || self
                    .editing
                    .as_ref()
                    .is_some_and(|edit| edit.activity_job.is_some()),
            tray_retry: if self.options_in_taskbar || self.test_copy {
                None
            } else if let Some(tray) = &self.tray {
                tray.retry_in()
            } else {
                Some(self.next_tray_attach.saturating_duration_since(now))
            },
            // A minimized window keeps its unsaved position until it is restored.
            position_save: self
                .window_position_changed
                .filter(|_| !hidden)
                .map(|changed| {
                    let waited = now.saturating_duration_since(changed);
                    if waited < WINDOW_POSITION_SAVE_DELAY {
                        WINDOW_POSITION_SAVE_DELAY - waited
                    } else {
                        WINDOW_POSITION_SAVE_DELAY
                    }
                }),
            auto_resume: self.auto_resume_remaining(),
            // Waveform-only mode shows the countdown in a tooltip.
            countdown_shown: !self.waveform_only || ctx.input(|input| input.pointer.has_pointer()),
            // Nothing blinks unless the mark is shown while recording.
            blink: self
                .recording_started
                .filter(|_| {
                    self.status_mark == StatusMark::Always && self.controller.is_recording()
                })
                .map(|started| ctx.input(|input| input.time) - started),
            meters_live: !self.waveform_only
                && self.show_meters
                && (meter_shown(0) || (self.dual_source() && meter_shown(1))),
            fine_edit: Some(now.saturating_duration_since(self.last_interaction))
                .filter(|&since| since < FINE_EDIT_SETTLE),
            waveform: (live && self.live_revision != Some(self.controller.revision()))
                .then(|| self.next_refresh.saturating_duration_since(now)),
            options_scan: (self.options_open
                && self.primary
                && self.options_section == OptionsSection::Actions)
                .then(|| self.snapshot_target_scan.saturating_duration_since(now)),
        }
    }

    /// Asks for the next pass. Everything else that changes the window asks for
    /// its own repaint: input, the capture worker, the tray, the shortcut and the
    /// updater threads.
    fn schedule_wake(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let waits = self.waits(ctx, frame);
        // A hidden window shows neither the waveform nor the meters.
        self.controller.set_watching(!waits.hidden);
        ctx.request_repaint_after_for(next_wake(&waits), egui::ViewportId::ROOT);
    }

    fn display_policy(&self, audio: Rect, visible: Range<u64>) -> waveform::DisplayPolicy {
        waveform::DisplayPolicy::for_view(
            audio.width(),
            audio.height(),
            visible.end.saturating_sub(visible.start),
            self.sample_rate,
            self.is_fine_editing(),
        )
    }

    fn refresh_edit_preview(&mut self) {
        let edit = self
            .editing
            .as_ref()
            .expect("editing preview has immutable material");
        let history = edit.frames();
        let visible = self.zoom_frames.as_ref().map_or_else(
            || history.clone(),
            |zoom| fit_frame_range(&history, zoom.start, zoom.end.saturating_sub(zoom.start)),
        );
        let inner = self.timeline_rect(self.plot_rect);
        let width = if visible != history {
            inner.width()
        } else {
            let seconds = (history.end - history.start) as f32 / self.sample_rate.max(1) as f32;
            inner.width() * (seconds / edit.retention_seconds.max(1.0)).clamp(0.0, 1.0)
        };
        let policy = self.display_policy(
            Rect::from_min_size(inner.min, Vec2::new(width, inner.height())),
            visible.clone(),
        );
        let key = EditPreviewKey {
            frames: visible,
            rows: self.visible_rows(),
            pairs: [0, 1].map(|lane| self.lane_pair(lane).cloned()),
            bins: policy.bins,
        };
        if let Some(job) = &self.edit_preview
            && !job.worker.is_finished()
        {
            return;
        }
        if let Some(job) = self.edit_preview.take()
            && let Ok(peaks) = job.worker.join()
            && job.key == key
        {
            // Publish the immutable material's range and its peaks together.
            // A partially filled live view must keep its old right-aligned
            // pointer geometry until this preview is actually ready.
            let edit = self.editing.as_ref().expect("material remains available");
            self.history_frames = edit.frames();
            self.sample_rate = edit.snapshots[0].sample_rate();
            self.bar_markers.clone_from(&edit.bars);
            self.peaks = peaks;
            self.peak_rows.clone_from(&key.rows);
            self.visible_frames = key.frames.clone();
            self.seconds =
                (key.frames.end - key.frames.start) as f32 / self.sample_rate.max(1) as f32;
            self.edit_preview_key = Some(key.clone());
            if let Some((asked, Some(arrived))) = self.edit_opening.take()
                && let Some(harness) = &mut self.harness
            {
                let pair = key
                    .rows
                    .iter()
                    .any(|row| matches!(row, RowSource::Pair(..)));
                harness.note(format!(
                    "frozen view: audio in {:.1} ms after asking, first picture {:.1} ms after \
                     that ({} rows, pair {}, {} frames shown)",
                    arrived.duration_since(asked).as_secs_f64() * 1000.0,
                    arrived.elapsed().as_secs_f64() * 1000.0,
                    key.rows.len(),
                    if pair { "drawn" } else { "absent" },
                    key.frames.end - key.frames.start,
                ));
            }
        }
        if self.edit_preview_key.as_ref() == Some(&key) {
            return;
        }
        let snapshots = Arc::clone(
            &self
                .editing
                .as_ref()
                .expect("material remains available")
                .snapshots,
        );
        let lanes = self
            .editing
            .as_ref()
            .expect("material remains available")
            .lanes
            .clone();
        let render_key = key.clone();
        match thread::Builder::new()
            .name("backtracking-selection-peaks".into())
            .spawn(move || edit_preview(&snapshots, &lanes, &render_key))
        {
            Ok(worker) => self.edit_preview = Some(EditPreviewJob { key, worker }),
            Err(error) => self.message = format!("Could not prepare selection display: {error}"),
        }
    }

    /// The audio that fills the plot's width: the history setting, or less when memory cannot
    /// hold that much of the lanes' channels.
    fn plot_retention_seconds(&self) -> f32 {
        let requested = self.controller.retention().seconds().max(1) as f32;
        self.controller
            .effective_retention_seconds()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .map_or(requested, |seconds| requested.min(seconds as f32).max(1.0))
    }

    /// Mono and stereo lanes are drawn as they always were: merged, or Left / Right for one
    /// stereo source when chosen. A wider lane gets one row per channel of its group on show.
    fn lane_plans(&self) -> Vec<LanePlan> {
        let dual = self.dual_source();
        let edit = self.editing.as_ref();
        (0..if dual { 2 } else { 1 })
            .map(|lane| {
                let channels = edit
                    .and_then(|edit| edit.track(lane))
                    .map(FrozenTrack::channels)
                    .or_else(|| self.controller.track_channels(lane));
                if let Some(channels) = channels.filter(|&channels| channels > 2) {
                    let (groups, shown) = match edit {
                        Some(edit) => (
                            edit.groups[lane].as_slice(),
                            edit.shown_groups[lane].as_deref(),
                        ),
                        None => (
                            self.controller.lane_groups(lane),
                            self.controller
                                .shown_group(lane)
                                .map(|group| group.id.as_str()),
                        ),
                    };
                    let pair = pair_group(groups, channels);
                    let (group, groups) = group_on_show(groups, shown, channels);
                    let shown = shown_channels(&group, self.active_channels(lane));
                    return LanePlan {
                        lane,
                        kind: LaneKind::Stack {
                            lead: if pair.is_some() {
                                RowSource::pair(lane).to_vec()
                            } else {
                                Vec::new()
                            },
                            pair,
                            group,
                            shown,
                            groups,
                        },
                    };
                }
                let views = if !dual && self.split_waveform && channels == Some(2) {
                    vec![WaveformView::Left, WaveformView::Right]
                } else {
                    vec![WaveformView::Merged]
                };
                LanePlan {
                    lane,
                    kind: LaneKind::Views(views),
                }
            })
            .collect()
    }

    /// Whether lane `lane` hides the rows of its silent channels: a system, application or
    /// microphone source wider than stereo, with the preference on. Add-ons say what they
    /// carry, so their rows always show.
    fn hides_silent_rows(&self, lane: usize) -> bool {
        self.hide_silent
            && self
                .controller
                .sources()
                .get(lane)
                .and_then(Option::as_ref)
                .is_some_and(|settings| {
                    matches!(
                        settings.source,
                        CaptureSource::System
                            | CaptureSource::DefaultMicrophone
                            | CaptureSource::Microphone { .. }
                            | CaptureSource::Application { .. }
                    )
                })
    }

    /// The channels of `lane` that have a row while silent rows are hidden, or `None` when
    /// they are not. A frozen view uses the activity of its own audio once it is worked out,
    /// and until then the rows shown when it was frozen. Before the first answer only the pair
    /// shows: rows then appear as their channels are found active, and the window never shows
    /// a row of silence it takes away again a moment later.
    fn active_channels(&self, lane: usize) -> Option<u64> {
        if !self.hides_silent_rows(lane) {
            return None;
        }
        let live = self.activity[lane].shown;
        let frozen = self.editing.as_ref().and_then(|edit| edit.activity[lane]);
        Some(frozen.or(live).unwrap_or(0))
    }

    /// Asks a few times a second which channels of each lane that hides silent rows are
    /// active, while capture runs. The answer comes from the channel activity worker and never
    /// makes the window wait.
    fn update_activity(&mut self) {
        let hiding = [0, 1].map(|lane| self.hides_silent_rows(lane));
        if let Some(edit) = &mut self.editing {
            // Hiding turned on while the view is frozen: work out the frozen activity now.
            if !edit.poll_activity()
                && (0..2).any(|lane| {
                    hiding[lane] && edit.activity[lane].is_none() && edit.lanes.contains(&lane)
                })
            {
                edit.activity_job = frozen_activity(&edit.snapshots, &edit.lanes, hiding);
            }
            return;
        }
        if self.edit_request.is_some() {
            return;
        }
        let now = Instant::now();
        let generation = self.controller.generation();
        for lane in 0..2 {
            if !self.hides_silent_rows(lane) {
                self.activity[lane] = ChannelActivity::default();
                continue;
            }
            let state = &mut self.activity[lane];
            if state.generation != generation {
                *state = ChannelActivity {
                    generation,
                    ..ChannelActivity::default()
                };
            }
            if state
                .asked
                .is_some_and(|asked| now.duration_since(asked) < ACTIVITY_INTERVAL)
            {
                continue;
            }
            if let Some(active) = self.controller.try_active_channels(lane) {
                let state = &mut self.activity[lane];
                state.answer(active);
                state.asked = Some(now);
            }
        }
    }

    /// Works out this pass's lanes. A lane whose group changed starts again at its first row.
    /// A pointer gesture on the waveform keeps the rows it began with until it ends. The
    /// selection and the flags keep their rows across the change (`carry_rows`).
    fn update_lanes(&mut self) {
        let planned = self.lane_plans();
        if let Some(old) = take_plans(
            &mut self.lanes,
            &mut self.lane_first_row,
            planned,
            self.gesture.is_some(),
        ) {
            self.selection_rows = carry_rows(
                &old,
                &self.lanes,
                &self.selection_rows,
                &mut self.selection_whole,
            );
            if let Some(flags) = &mut self.flags {
                flags.rows = carry_rows(&old, &self.lanes, &flags.rows, &mut flags.whole);
            }
            self.live_revision = None;
            self.next_refresh = Instant::now();
        }
        // A taller window shows more rows; never leave empty space below the last one.
        let boxes = self.lane_boxes(self.timeline_rect(self.plot_rect).height());
        for (plan, lane_box) in self.lanes.iter().zip(&boxes) {
            self.lane_first_row[plan.lane] = lane_box.first;
        }
    }

    fn stacked(&self) -> bool {
        self.lanes.iter().any(LanePlan::stacked)
    }

    /// The pair the L/R rows of logical lane `lane` are made from.
    fn lane_pair(&self, lane: usize) -> Option<&PairGroup> {
        self.lanes
            .iter()
            .find(|plan| plan.lane == lane)
            .and_then(LanePlan::pair_group)
    }

    /// Where each lane's rows go in a plot area this high.
    fn lane_boxes(&self, height: f32) -> Vec<LaneBox> {
        let specs: Vec<_> = self.lanes.iter().map(LanePlan::spec).collect();
        let first: Vec<_> = self
            .lanes
            .iter()
            .map(|plan| self.lane_first_row[plan.lane])
            .collect();
        stack::lay_out(height, &specs, &first)
    }

    /// The rows on screen, top to bottom. Only these are rendered.
    fn visible_rows(&self) -> Vec<RowSource> {
        let boxes = self.lane_boxes(self.timeline_rect(self.plot_rect).height());
        self.lanes
            .iter()
            .zip(&boxes)
            .flat_map(|(plan, lane_box)| lane_box.visible().map(|row| plan.row(row)))
            .collect()
    }

    fn scroll_lane_to(&mut self, lane: usize, row: usize) {
        let Some(slot) = self.lane_first_row.get_mut(lane) else {
            return;
        };
        if *slot != row {
            *slot = row;
            // Clamped to the lane's last page, then rendered on the next pass.
            self.update_lanes();
            self.live_revision = None;
            self.next_refresh = Instant::now();
            self.last_interaction = Instant::now();
        }
    }

    /// Shows another channel group of a lane. A selection on screen switches with it, so a
    /// drag-out exports the group the user sees.
    fn show_group(&mut self, lane: usize, id: &str) -> Result<(), String> {
        if let Some(edit) = &mut self.editing
            && let Some(group) = edit
                .groups
                .get(lane)
                .and_then(|groups| groups.iter().find(|group| group.id == id))
        {
            let group = group.clone();
            edit.shown_groups[lane] = Some(id.to_owned());
            if let Some(index) = edit.lanes.iter().position(|&logical| logical == lane) {
                edit.export_options.groups[index] = Some(group);
            }
        }
        let result = self.controller.select_group(lane, id);
        if let Some(slot) = self.lane_first_row.get_mut(lane) {
            *slot = 0;
        }
        self.invalidate_export();
        self.live_revision = None;
        self.next_refresh = Instant::now();
        result
    }

    /// Grows the window when a lane stacks rows, so they need not scroll, and puts it back when
    /// no lane stacks any more unless the user resized it meanwhile (`fit_window`).
    fn fit_window_to_rows(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if !self.auto_grow || self.waveform_only {
            return;
        }
        let window = ctx.viewport_rect().size();
        let specs: Vec<_> = self.lanes.iter().map(LanePlan::spec).collect();
        let current = self.timeline_rect(self.plot_rect).height();
        let (window_drag, gesture) = (self.window_drag.is_some(), self.gesture.is_some());
        let cap = || {
            frame
                .winit_window()
                .and_then(|window| window.current_monitor())
                .map(|monitor| monitor.size().height as f32 / ctx.pixels_per_point() * 0.85)
                .map_or(MAX_GROWN_WINDOW_HEIGHT, |screen| {
                    screen.min(MAX_GROWN_WINDOW_HEIGHT)
                })
        };
        match fit_window(
            &specs,
            current,
            window.y,
            &mut self.growth,
            Instant::now(),
            window_drag,
            gesture,
            cap,
        ) {
            WindowFit::Stay => {}
            WindowFit::Grow(height) => {
                let size = Vec2::new(window.x, height);
                // A test copy stays where it was put, off every screen.
                if !self.test_copy {
                    fit_expanded_window(ctx, frame, size);
                }
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
            }
            WindowFit::Restore(height) => {
                let size = Vec2::new(window.x, height).max(min_window_size(false));
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
            }
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui, interaction_blocked: bool) {
        ui.horizontal(|ui| {
            if self.show_title {
                ui.heading("Backtracking");
            }
            self.transport_button(ui, interaction_blocked);
            self.recording_indicator(ui);
            self.clear_button(ui, interaction_blocked);
            self.pin_button(ui, interaction_blocked);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.options_button(ui, !interaction_blocked && self.gesture.is_none());
                if self.show_meters {
                    if self.dual_source() {
                        self.meter_readout(ui, 1);
                    }
                    self.meter_readout(ui, 0);
                }
            });
        });
    }

    fn toggle_capture(&mut self) {
        if self.controller.is_paused() {
            self.resume();
        } else {
            self.freeze();
        }
    }

    fn transport_button(&mut self, ui: &mut egui::Ui, menu_open: bool) {
        let can_toggle = !menu_open && self.gesture.is_none();
        let response = ui.add_enabled(can_toggle, icon_button(TOOLBAR_BUTTON_SIZE));
        if response.clicked() {
            self.toggle_capture();
        }
        let paused = self.controller.is_paused();
        let label = if self.controller.error().is_some() {
            "Retry capture"
        } else if paused {
            "Start capture"
        } else {
            "Pause capture"
        };
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, can_toggle, label));
        let response = response.on_hover_text(label);
        let center = response.rect.center();
        let color = ui.style().interact(&response).fg_stroke.color;
        paint_transport_glyph(ui.painter(), center, paused, color, &TRANSPORT_GLYPH);
    }

    fn capture_indicator(&self) -> CaptureIndicator {
        CaptureIndicator::new(
            self.controller.error().is_some(),
            self.controller.is_paused(),
            self.controller.is_recording(),
        )
    }

    /// The bright half of the one-second recording blink, which starts lit.
    fn recording_lit(&mut self, time: f64) -> bool {
        if !self.controller.is_recording() {
            self.recording_started = None;
            return false;
        }
        let started = *self.recording_started.get_or_insert(time);
        (time - started).rem_euclid(1.0) < 0.5
    }

    fn recording_indicator(&mut self, ui: &mut egui::Ui) {
        if self.status_mark == StatusMark::Never {
            return;
        }
        let size = Vec2::new(15.0, 24.0);
        let indicator = self.capture_indicator();
        if !status_mark_shown(
            self.status_mark,
            indicator,
            self.controller.error().is_some(),
        ) {
            // The light keeps its place, so the buttons do not move when it comes on.
            ui.allocate_space(size);
            return;
        }
        let status = indicator.label();
        let color = indicator.led_color(self.recording_lit(ui.input(|input| input.time)));

        status_led(ui, size, 4.5, color, status);
    }

    /// Time left before a paused capture resumes by itself, if it is going to.
    /// A gesture or a selection holds the pause, and `Off` never resumes.
    fn auto_resume_remaining(&self) -> Option<Duration> {
        if !self.controller.is_paused() || self.gesture.is_some() || self.selection.is_some() {
            return None;
        }
        self.idle_resume.remaining(self.last_interaction.elapsed())
    }

    fn capture_problem(&self) -> Option<String> {
        let language = Language::current();
        capture_problem(
            self.controller
                .error()
                .map(|error| language.source_message(error)),
            [0, 1].map(|lane| {
                self.controller
                    .source_status(lane)
                    .map(|status| language.source_message(status))
            }),
            self.dual_source(),
        )
    }

    /// The status line under the waveform in the full window.
    fn status_text(&self) -> String {
        if let Some(error) = self.controller.error() {
            return Language::current().source_message(error).to_owned();
        }
        let paused = self.controller.is_paused();
        if self.edit_request.is_some() {
            "Preparing selection…".into()
        } else if self.editing.is_some() && self.export.is_none() && self.snapshot_save.is_none() {
            // A selection of some rows names them, as one of a lane did on the Mac.
            let selecting =
                self.selection.is_some() || matches!(self.gesture, Some(Gesture::Select { .. }));
            let selection = match self.selected_rows().filter(|_| selecting) {
                Some(rows) => format!("Selection · {}", self.rows_name(rows)),
                None => "Selection".to_owned(),
            };
            if paused {
                format!("{selection} / Paused")
            } else {
                selection
            }
        } else {
            idle_status(
                paused.then(|| paused_status(self.auto_resume_remaining())),
                &self.message,
            )
        }
    }

    /// Waveform-only mode has no toolbar and no status line, so the plot carries the state: a
    /// small mark in the corner, the failure text when a source has a problem, and a tooltip
    /// with the rest.
    fn draw_compact_status(&mut self, ui: &egui::Ui, rect: Rect) {
        let rect = rect.shrink(self.border_inset());
        let indicator = self.capture_indicator();
        let problem = self.capture_problem();
        if !status_mark_shown(self.status_mark, indicator, problem.is_some()) {
            return;
        }
        let lit = self.recording_lit(ui.input(|input| input.time));
        let hover = paint_compact_status(
            &ui.painter_at(rect),
            rect,
            indicator,
            lit,
            problem.as_deref(),
        );

        let state = if indicator == CaptureIndicator::Paused {
            paused_status(self.auto_resume_remaining())
        } else {
            indicator.label().to_owned()
        };
        let status = compact_status_text(&state, problem.as_deref(), &self.message);
        let response = ui.interact(
            hover,
            egui::Id::unique("capture-status"),
            egui::Sense::hover(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, &status));
        response.on_hover_text(&status);
    }

    fn menu_state(&self) -> context_menu::State {
        context_menu::State {
            transport: if self.controller.error().is_some() {
                context_menu::Transport::Retry
            } else if self.controller.is_paused() {
                context_menu::Transport::Resume
            } else {
                context_menu::Transport::Pause
            },
            capture_controls: self.gesture.is_none(),
            waveform_only: self.waveform_only,
            keep_on_top: self.pinned,
            several_rows: self.row_count() > 1,
            separate_rows: self.separate_rows,
            separate_label: Language::current().separate_rows(),
        }
    }

    /// Runs a command from the right-click menu or the keyboard through the same methods as
    /// the toolbar and the Options window, so behaviour and saved preferences match.
    fn run_command(&mut self, command: WindowCommand, ctx: &egui::Context, frame: &eframe::Frame) {
        let capture_controls = self.gesture.is_none();
        match command {
            WindowCommand::ToggleCapture if capture_controls => self.toggle_capture(),
            WindowCommand::Clear if capture_controls => self.clear_capture(),
            WindowCommand::SaveSnapshot if capture_controls => self.save_snapshot(),
            WindowCommand::ToggleCapture | WindowCommand::Clear | WindowCommand::SaveSnapshot => {}
            WindowCommand::SeparateLanes => self.toggle_separate_rows(),
            WindowCommand::WaveformOnly => {
                self.waveform_only = !self.waveform_only;
                self.apply_waveform_mode(ctx, frame);
            }
            WindowCommand::KeepOnTop => {
                self.pinned = !self.pinned;
                self.apply_pinned(ctx);
            }
            WindowCommand::Options => self.open_options(ctx),
            WindowCommand::Quit => self.quit(ctx),
        }
    }

    fn show_context_menu(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(window) = frame.winit_window() else {
            // Without a native window there is nowhere to anchor a menu. Keep right-click useful.
            self.open_options(ctx);
            return;
        };
        let entries = menu_entries(self.menu_state(), Language::current(), &self.hotkeys);
        let command = context_menu::show(window, &entries);
        // The menu ran its own message loop. Time spent reading it is not idle time.
        self.last_interaction = Instant::now();
        if let Some(command) = command {
            self.run_command(command, ctx, frame);
        }
    }

    fn clear_button(&mut self, ui: &mut egui::Ui, menu_open: bool) {
        let enabled = !menu_open && self.gesture.is_none();
        let response = ui.add_enabled(enabled, icon_button(TOOLBAR_BUTTON_SIZE));
        if response.clicked() {
            self.clear_capture();
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, enabled, "Clear capture")
        });
        let response = response.on_hover_text("Clear capture");
        let center = response.rect.center();
        let stroke = Stroke::new(1.3, ui.style().interact(&response).fg_stroke.color);
        let point = |x, y| center + Vec2::new(x, y);
        let painter = ui.painter();
        painter.line_segment([point(-6.0, -4.0), point(6.0, -4.0)], stroke);
        painter.line_segment([point(-2.0, -6.0), point(2.0, -6.0)], stroke);
        painter.add(egui::Shape::line(
            vec![
                point(-4.4, -4.0),
                point(-3.8, 6.0),
                point(3.8, 6.0),
                point(4.4, -4.0),
            ],
            stroke,
        ));
        for x in [-1.7, 1.7] {
            painter.line_segment([point(x, -1.5), point(x, 3.7)], stroke);
        }
    }

    fn meter_readout(&self, ui: &mut egui::Ui, lane: usize) {
        let mode = self.meter_modes[lane];
        let readings = self.controller.lane_meter_readings(lane);
        let value = readings.and_then(|readings| meter_value(mode, readings));
        // A wide lane has peak and VU levels but no loudness; it shows a dash, not a number.
        let unmeasured = mode == MeterMode::Lufs
            && readings.is_some_and(|readings| readings.momentary_lufs.is_none());
        let number = meter_number(value, mode == MeterMode::Vu);
        let text = if self.dual_source() {
            format!("{} {number} {}", lane_name(lane), mode.unit())
        } else {
            format!("{number} {}", mode.unit())
        };
        let width = if self.dual_source() { 91.0 } else { 102.0 };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 24.0), egui::Sense::hover());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, &text));
        if unmeasured {
            response.on_hover_text(format!(
                "{}\n{}",
                mode.description(),
                Language::current().no_loudness()
            ));
        } else {
            response.on_hover_text(mode.description());
        }
        let painter = ui.painter();
        painter.rect_filled(rect, 5.0, ui.visuals().widgets.inactive.bg_fill);
        painter.text(
            rect.right_center() - Vec2::new(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            text,
            egui::FontId::monospace(if self.dual_source() { 10.0 } else { 12.0 }),
            ui.visuals().text_color(),
        );
    }

    fn pin_button(&mut self, ui: &mut egui::Ui, menu_open: bool) {
        let response = ui.add_enabled(!menu_open, icon_button(TOOLBAR_BUTTON_SIZE));
        if response.clicked() {
            self.pinned = !self.pinned;
            self.apply_pinned(ui.ctx());
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::CheckBox, !menu_open, self.pinned, "Keep on top")
        });
        let response = response.on_hover_text(if self.pinned {
            "Disable keep on top"
        } else {
            "Keep on top"
        });
        let center = response.rect.center();
        let color = if self.pinned {
            self.accent
        } else {
            ui.style().interact(&response).fg_stroke.color
        };
        let stroke = Stroke::new(1.3, color);
        let points = [
            (-4.5, -7.0),
            (4.5, -7.0),
            (2.8, -5.2),
            (2.8, -1.7),
            (5.5, 1.0),
            (5.5, 2.2),
            (-5.5, 2.2),
            (-5.5, 1.0),
            (-2.8, -1.7),
            (-2.8, -5.2),
        ];
        ui.painter().add(egui::Shape::closed_line(
            points
                .into_iter()
                .map(|(x, y)| center + Vec2::new(x, y))
                .collect(),
            stroke,
        ));
        ui.painter().line_segment(
            [center + Vec2::new(0.0, 2.2), center + Vec2::new(0.0, 7.0)],
            stroke,
        );
    }

    fn apply_pinned(&mut self, ctx: &egui::Context) {
        self.next_pin_check = Instant::now();
        self.save_preference(Key::AlwaysOnTop, if self.pinned { "1" } else { "0" });
        let level = if self.pinned {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        };
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(level));
    }

    fn options_button(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let response = ui
            .add_enabled(enabled, icon_button(TOOLBAR_BUTTON_SIZE))
            .on_hover_text("Options");
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, "Options"));
        let center = response.rect.center();
        let color = ui.style().interact(&response).fg_stroke.color;
        let mut teeth = Vec::with_capacity(32);
        for tooth in 0..8 {
            for (offset, radius) in [(-0.32, 6.0), (-0.20, 8.0), (0.20, 8.0), (0.32, 6.0)] {
                let angle = (tooth as f32 + offset) * std::f32::consts::TAU / 8.0;
                let (sin, cos) = angle.sin_cos();
                teeth.push(center + Vec2::new(cos, sin) * radius);
            }
        }
        let stroke = Stroke::new(1.2, color);
        ui.painter().add(egui::Shape::closed_line(teeth, stroke));
        ui.painter().circle_stroke(center, 2.6, stroke);
        if response.clicked() {
            self.open_options(ui.ctx());
        }
    }

    fn open_options(&mut self, ctx: &egui::Context) {
        self.refresh_source_list();
        self.snapshot_target_scan = Instant::now();
        self.options_open = true;
        self.options_focus_requested = true;
        ctx.send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint();
    }

    fn show(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn apply_waveform_mode(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        set_waveform_frame(frame.winit_window(), self.waveform_only);
        self.save_preference(
            Key::WaveformOnly,
            if self.waveform_only { "1" } else { "0" },
        );
        let minimum = min_window_size(self.waveform_only);
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(minimum));
        let size = ctx.viewport_rect().size();
        let updated = if self.waveform_only {
            self.full_window_size = size;
            Vec2::new(
                size.x,
                self.full_waveform_height + self.window_border_inset() * 2.0,
            )
            .max(minimum)
        } else {
            let restored = self.full_window_size.max(minimum);
            fit_expanded_window(ctx, frame, restored);
            restored
        };
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(updated));
    }

    fn draw_options_window(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if !self.options_open {
            return;
        }
        // A direct repaint of a hidden window can run without the event loop context
        // eframe needs to create an immediate viewport.
        if frame
            .winit_window()
            .is_some_and(|window| window_is_hidden(window))
        {
            self.options_focus_requested = true;
            return;
        }
        let pinned = self.pinned;
        let waveform_only = self.waveform_only;
        let old_window_inset = self.window_border_inset();
        let mut title = if self.primary {
            tray::OPTIONS_TITLE.to_owned()
        } else {
            "Backtracking Capture Options".to_owned()
        };
        if self.test_copy {
            // The installed app's Options window has the plain title, and windows are found
            // by title (`tray::prepare_options_window`).
            title.push_str(" (test copy)");
        }
        let size = self
            .harness
            .as_ref()
            .and_then(harness::Harness::options_size)
            .unwrap_or(Vec2::new(820.0, 640.0));
        let mut builder = egui::ViewportBuilder::default()
            .with_title(&title)
            .with_inner_size(size)
            .with_min_inner_size([680.0, 460.0])
            .with_resizable(true)
            .with_maximize_button(true)
            .with_taskbar(self.options_in_taskbar)
            .with_visible(false)
            .with_window_level(if self.pinned {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            });
        if let Some(position) = self.options_test_position {
            // A test copy's window appears off screen and leaves the keyboard where it is.
            builder = builder.with_position(position).with_active(false);
        }
        let action = ctx.show_viewport_immediate(options_viewport(), builder, |ui, class| {
            if ui.input(|input| input.viewport().close_requested()) {
                return Some(OptionsAction::Close);
            }
            // A row of the Hotkeys page that records takes every key, Escape included, before
            // a button or the window can act on it.
            if self.hotkeys_page.is_recording()
                && let Some(recorded) = ui
                    .ctx()
                    .input_mut(|input| hotkeys_page::take_recorded(&mut input.events))
            {
                let global = hotkeys_page::global(self.hotkey.as_ref());
                if self
                    .hotkeys_page
                    .recorded(recorded, &mut self.hotkeys, global)
                {
                    self.save_hotkeys();
                }
            }
            // Escape closes an open add-on question first, and leaves a text field alone.
            // Otherwise it closes the window unless it is closing a drop-down or a colour picker.
            if !egui::Popup::is_any_open(ui.ctx())
                && ui.input(|input| input.key_pressed(egui::Key::Escape))
                && !ui.ctx().egui_wants_keyboard_input()
                && !(self.options_section == OptionsSection::Addons
                    && self.addons_page.cancel_on_escape())
            {
                return Some(OptionsAction::Close);
            }
            if std::mem::take(&mut self.options_focus_requested)
                && class == egui::ViewportClass::Immediate
            {
                if !self.options_in_taskbar {
                    tray::prepare_options_window(&title);
                }
                ui.ctx().send_viewport_cmd(ViewportCommand::Visible(true));
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Minimized(false));
                if !self.test_copy {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
                }
            }
            if ui.input(|input| input.pointer.any_pressed()) {
                self.last_interaction = Instant::now();
            }
            if let Some(harness) = &mut self.harness {
                harness.drop_on_options(ui.ctx());
            }
            // A package dropped here opens its confirmation on the Add-ons page; another file goes
            // to the add-on that opens it.
            let dropped = dropped_paths(ui.ctx());
            if let Some(package) = addons_page::dropped_package(&dropped) {
                self.review_addon(package);
            } else {
                self.route_dropped_files(ui.ctx(), &dropped, false);
            }
            let action = self.options_contents(ui, frame);
            addons_page::paint_drop_hint(ui.ctx(), self.accent, Language::current());
            if let Some(harness) = &mut self.harness {
                harness.photograph_options(ui.ctx());
            }
            action
        });
        if self.waveform_only != waveform_only {
            self.apply_waveform_mode(ctx, frame);
        } else {
            self.fit_border_change(ctx, old_window_inset);
        }
        if self.pinned != pinned {
            self.apply_pinned(ctx);
        }
        match action {
            Some(OptionsAction::SelectSaveFolder) => self.select_save_folder(frame),
            Some(OptionsAction::GoToSaveFolder) => {
                let result = std::fs::create_dir_all(&self.output)
                    .and_then(|()| Command::new("explorer.exe").arg(&self.output).spawn());
                if let Err(error) = result {
                    self.message = format!("Could not open save folder: {error}");
                }
            }
            Some(OptionsAction::SelectSnapshotFolder) => self.select_saved_snapshot_folder(frame),
            Some(OptionsAction::OpenSnapshotFolder) => self.open_saved_snapshot_folder(frame),
            Some(OptionsAction::PickAddonFile) => self.pick_addon_file(frame),
            Some(OptionsAction::OpenAddonFile(id)) => self.pick_file_for_addon(frame, &id),
            Some(OptionsAction::OpenLink(link)) => self.open_link(&link),
            Some(OptionsAction::Hide) => self.hide(ctx),
            Some(OptionsAction::Show) => self.show(ctx),
            Some(OptionsAction::Quit) => self.quit(ctx),
            Some(OptionsAction::Close) => self.options_open = false,
            None => {}
        }
    }

    fn options_contents(
        &mut self,
        ui: &mut egui::Ui,
        frame: &eframe::Frame,
    ) -> Option<OptionsAction> {
        configure_options_style(ui);
        egui::Panel::left("options-navigation")
            .exact_size(OPTIONS_NAVIGATION_WIDTH)
            .resizable(false)
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(10))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.add_space(6.0);
                for (section, _) in OptionsSection::ALL {
                    if !self.primary && section == OptionsSection::Updates {
                        continue;
                    }
                    let selected = self.options_section == section;
                    if options_navigation_item(ui, section.title(), selected, self.accent).clicked()
                    {
                        self.options_section = section;
                    }
                }
            });
        // Changes apply at once and the window closes from its title bar or with Escape, so
        // the only thing left for a footer is a message, when there is one.
        if !self.message.is_empty() {
            egui::Panel::bottom("options-message")
                .frame(
                    egui::Frame::side_top_panel(ui.style())
                        .inner_margin(egui::Margin::symmetric(22, 8)),
                )
                .show(ui, |ui| {
                    ui.add(egui::Label::new(&self.message).truncate())
                        .on_hover_text(&self.message);
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(22))
            .show(ui, |ui| {
                let title = options_heading_text(ui, self.options_section.title(), 20.0);
                ui.label(title);
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt(self.options_section)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        options_list(ui, |ui| match self.options_section {
                            OptionsSection::Actions => {
                                self.capture_action_options(ui);
                                ui.separator();
                                self.snapshot_options(ui);
                                ui.separator();
                                self.app_options(ui, frame)
                            }
                            OptionsSection::Hotkeys => {
                                self.hotkey_options(ui);
                                None
                            }
                            OptionsSection::Capture => {
                                self.capture_options(ui);
                                None
                            }
                            OptionsSection::Sources => self.source_options(ui),
                            OptionsSection::Appearance => {
                                self.appearance_options(ui);
                                None
                            }
                            OptionsSection::Export => {
                                self.wav_settings(ui);
                                ui.separator();
                                self.folder_options(ui)
                            }
                            OptionsSection::Addons => self.addon_options(ui),
                            OptionsSection::Updates => {
                                self.updates.show(ui).then_some(OptionsAction::Quit)
                            }
                        })
                    })
                    .inner
            })
            .inner
    }

    fn snapshot_options(&mut self, ui: &mut egui::Ui) {
        options_group_heading(ui, "Snapshots");
        let (save, new_window) = ui
            .horizontal_wrapped(|ui| {
                (
                    ui.button("Save snapshot").clicked(),
                    ui.button("New capture window").clicked(),
                )
            })
            .inner;
        if save {
            self.save_snapshot();
        }
        if new_window {
            let launched = std::env::current_exe()
                .and_then(|exe| Command::new(exe).arg("--new-capture-window").spawn());
            if let Err(error) = launched {
                self.message = format!("Could not open capture window: {error}");
            }
        }
        if !self.primary {
            return;
        }
        // The Hotkeys page changes it; this line only names it.
        let language = Language::current();
        options_field(ui, "Global shortcut", |ui| {
            options_value(
                ui,
                &hotkeys_page::bindings_text(
                    language,
                    self.hotkeys.bindings(Action::SaveSnapshotGlobal),
                ),
            );
            if let Some(error) = &self.hotkey_error {
                options_note(ui, language.could_not_register(error));
            }
        });
        if Instant::now() >= self.snapshot_target_scan {
            self.snapshot_targets = backtracking_core::discover_snapshot_targets();
            self.snapshot_target_scan = Instant::now() + Duration::from_secs(1);
        }
        let current = self
            .snapshot_target
            .as_deref()
            .map_or("Desktop recorder", |id| {
                self.snapshot_targets
                    .iter()
                    .find(|target| target.id == id)
                    .map_or("Plugin unavailable", |target| target.name.as_str())
            })
            .to_owned();
        let mut target = self.snapshot_target.clone();
        options_field(ui, "Shortcut target", |ui| {
            let width = options_control_width(ui);
            egui::ComboBox::from_id_salt("snapshot-target")
                .width(width)
                .height(options_menu_height(ui, self.snapshot_targets.len() + 1))
                .selected_text(&current)
                .truncate()
                .show_ui(ui, |ui| {
                    configure_options_menu(ui, width);
                    ui.selectable_value(&mut target, None, "Desktop recorder");
                    for plugin in &self.snapshot_targets {
                        let suffix = plugin
                            .id
                            .get(plugin.id.len().saturating_sub(6)..)
                            .unwrap_or(&plugin.id);
                        ui.selectable_value(
                            &mut target,
                            Some(plugin.id.clone()),
                            format!("{} ({suffix})", plugin.name),
                        );
                    }
                })
                .response
                .on_hover_text(current);
        });
        if self.snapshot_target != target {
            self.save_preference(Key::SnapshotTarget, target.as_deref().unwrap_or(""));
            self.snapshot_target = target;
        }
    }

    fn capture_action_options(&mut self, ui: &mut egui::Ui) {
        options_group_heading(ui, "Capture controls");
        let can_toggle = self.gesture.is_none();
        let transport = if self.controller.error().is_some() {
            "Retry capture"
        } else if self.controller.is_paused() {
            "Play"
        } else {
            "Pause"
        };
        let has_selection = self.selection.is_some();
        let (toggle, cancel, clear) = ui
            .horizontal_wrapped(|ui| {
                (
                    ui.add_enabled(can_toggle, egui::Button::new(transport))
                        .clicked(),
                    ui.add_enabled(has_selection, egui::Button::new("Cancel selection"))
                        .clicked(),
                    ui.add_enabled(can_toggle, egui::Button::new("Clear capture"))
                        .clicked(),
                )
            })
            .inner;
        if toggle {
            self.toggle_capture();
        }
        if cancel {
            self.cancel_selection();
        }
        if clear {
            self.clear_capture();
        }
    }

    /// The Hotkeys page. A change is kept and in use at once: this window reads `hotkeys` for
    /// every key, and the page hands the system-wide shortcut its new chord itself.
    fn hotkey_options(&mut self, ui: &mut egui::Ui) {
        let global = hotkeys_page::global(self.hotkey.as_ref());
        if self.hotkeys_page.show(
            ui,
            Language::current(),
            &mut self.hotkeys,
            global,
            self.hotkey_error.as_deref(),
            self.primary,
        ) {
            self.save_hotkeys();
        }
    }

    /// When wide lanes leave room for less history than the setting asks for, says how much is
    /// kept.
    fn short_history_note(&self) -> Option<String> {
        let requested = f64::from(self.controller.retention().seconds());
        let effective = self.controller.effective_retention_seconds()?;
        if !effective.is_finite() || effective >= requested - 0.5 {
            return None;
        }
        let channels: usize = self
            .controller
            .active_lanes()
            .iter()
            .filter_map(|&lane| self.controller.track_channels(lane))
            .sum();
        Some(Language::current().shorter_history(channels, effective))
    }

    fn capture_options(&mut self, ui: &mut egui::Ui) {
        options_field(ui, "Retain the latest audio", |ui| {
            let mut retention = self.controller.retention();
            if options_choice(
                ui,
                "history",
                &mut retention,
                DurationChoice::ALL
                    .into_iter()
                    .map(|value| (value, history_label(value))),
            ) {
                self.controller.set_retention(retention);
                self.invalidate_export();
                self.live_revision = None;
                self.next_refresh = Instant::now();
                self.save_preference(Key::History, retention.seconds().to_string());
            }
        });
        // Wide lanes fill memory sooner; say how much of the setting is really kept.
        if let Some(note) = self.short_history_note() {
            options_note(ui, note);
        }
        let mut retain_silence = self.controller.retain_silence();
        if ui
            .checkbox(&mut retain_silence, "Keep quiet sections")
            .changed()
        {
            self.controller.set_retain_silence(retain_silence);
            self.save_preference(Key::RetainSilence, u8::from(retain_silence).to_string());
        }
        options_field(ui, "Auto-resume delay", |ui| {
            if options_choice(
                ui,
                "auto-resume",
                &mut self.idle_resume,
                AutoResumeDelay::ALL
                    .into_iter()
                    .map(|value| (value, auto_resume_label(value))),
            ) {
                self.save_preference(Key::IdleResume, self.idle_resume.seconds().to_string());
            }
        });
        self.multichannel_options(ui);
    }

    /// The settings for sources wider than stereo: which rows they show and how their L and R
    /// sound.
    fn multichannel_options(&mut self, ui: &mut egui::Ui) {
        let language = Language::current();
        ui.separator();
        options_group_heading(ui, language.multichannel_group());
        if ui
            .checkbox(&mut self.hide_silent, language.hide_silent_field())
            .changed()
        {
            self.save_preference(
                Key::HideSilentChannels,
                u8::from(self.hide_silent).to_string(),
            );
            self.live_revision = None;
            self.next_refresh = Instant::now();
            // A lane written whole writes what it shows, so a prepared drag is out of date.
            self.invalidate_export();
        }
        options_note(ui, language.hide_silent_note());
        options_field(ui, language.pair_render_field(), |ui| {
            if options_choice(
                ui,
                "pair-render",
                &mut self.pair_render,
                PairRender::ALL
                    .into_iter()
                    .map(|mode| (mode, language.pair_render(mode.binaural()).to_owned())),
            ) {
                self.save_preference(Key::PairRender, self.pair_render.preference());
                self.invalidate_export();
            }
        });
        options_note(ui, language.pair_render_note());
    }

    fn appearance_options(&mut self, ui: &mut egui::Ui) {
        options_group_heading(ui, "Window");
        ui.checkbox(&mut self.waveform_only, "Show waveform only");
        ui.checkbox(&mut self.pinned, "Keep on top");
        options_field(ui, "Background transparency", |ui| {
            if options_choice(
                ui,
                "transparency",
                &mut self.transparency,
                TRANSPARENCIES
                    .into_iter()
                    .map(|value| (value, format!("{value}%"))),
            ) {
                self.save_preference(Key::Transparency, self.transparency.to_string());
            }
        });
        self.accent_settings(ui);
        self.addon_look_settings(ui);
        self.waveform_color_settings(ui);
        ui.separator();
        options_group_heading(ui, "Show");
        if ui
            .checkbox(&mut self.show_title, "App name in the toolbar")
            .changed()
        {
            self.save_preference(Key::ShowTitle, u8::from(self.show_title).to_string());
        }
        if ui.checkbox(&mut self.show_meters, "Level meters").changed() {
            self.save_preference(Key::ShowMeters, u8::from(self.show_meters).to_string());
        }
        options_field(ui, "Capture status mark", |ui| {
            if options_choice(
                ui,
                "status-mark",
                &mut self.status_mark,
                StatusMark::ALL
                    .into_iter()
                    .map(|mark| (mark, mark.label().to_owned())),
            ) {
                self.save_preference(Key::StatusMark, self.status_mark.preference());
            }
        });
        ui.separator();
        options_group_heading(ui, "Timeline guides");
        if ui.checkbox(&mut self.bar_lines, "Show bar lines").changed() {
            self.save_preference(Key::BarLines, u8::from(self.bar_lines).to_string());
        }
        if ui
            .checkbox(&mut self.second_lines, "Show seconds lines")
            .changed()
        {
            self.save_preference(Key::SecondsLines, u8::from(self.second_lines).to_string());
        }
        options_field(ui, "Seconds interval", |ui| {
            ui.add_enabled_ui(self.second_lines, |ui| {
                if options_choice(
                    ui,
                    "seconds-interval",
                    &mut self.second_interval,
                    [1, 5, 10].into_iter().map(|value| {
                        (
                            value,
                            if value == 1 {
                                "1 second".to_owned()
                            } else {
                                format!("{value} seconds")
                            },
                        )
                    }),
                ) {
                    self.save_preference(Key::SecondsInterval, self.second_interval.to_string());
                }
            });
        });
        ui.separator();
        options_group_heading(ui, "Waveform border");
        if ui
            .checkbox(&mut self.waveform_border, "Show a border")
            .changed()
        {
            self.save_preference(
                Key::WaveformBorder,
                u8::from(self.waveform_border).to_string(),
            );
        }
        ui.add_enabled_ui(self.waveform_border, |ui| {
            options_field(ui, "Border width", |ui| {
                if options_choice(
                    ui,
                    "border-width",
                    &mut self.waveform_border_width,
                    BORDER_WIDTHS
                        .into_iter()
                        .map(|width| (width, format!("{width} px"))),
                ) {
                    self.save_preference(
                        Key::WaveformBorderWidth,
                        self.waveform_border_width.to_string(),
                    );
                }
                self.look_border_width_note(ui);
            });
            let mut theme = self.waveform_border_color.is_none();
            let theme_changed = ui.checkbox(&mut theme, "Use theme color").changed();
            let [r, g, b, _] = self.waveform_border_color.unwrap_or(self.accent).to_array();
            let mut color = [r, g, b];
            let color_changed = options_field(ui, "Custom border color", |ui| {
                ui.add_enabled_ui(!theme, |ui| ui.color_edit_button_srgb(&mut color))
                    .inner
                    .changed()
            });
            if theme_changed || color_changed {
                self.waveform_border_color = (!theme).then(|| accent_color(color));
                let [r, g, b] = color;
                self.save_preference(
                    Key::WaveformBorderColor,
                    if theme {
                        String::new()
                    } else {
                        format!("#{r:02x}{g:02x}{b:02x}")
                    },
                );
            }
            if ui
                .checkbox(
                    &mut self.waveform_border_follows_background,
                    "Follow background opacity",
                )
                .changed()
            {
                self.save_preference(
                    Key::WaveformBorderFollowsBackground,
                    u8::from(self.waveform_border_follows_background).to_string(),
                );
            }
        });
        ui.separator();
        options_group_heading(ui, "Display");
        options_field(ui, "Single-source waveform", |ui| {
            ui.add_enabled_ui(!self.dual_source(), |ui| {
                if options_choice(
                    ui,
                    "waveform-layout",
                    &mut self.split_waveform,
                    [
                        (false, "Merged".to_owned()),
                        (true, "Left / Right".to_owned()),
                    ],
                ) {
                    self.save_preference(
                        Key::SplitWaveform,
                        if self.split_waveform { "1" } else { "0" },
                    );
                    self.live_revision = None;
                    self.next_refresh = Instant::now();
                }
            });
        });
        let dual = self.dual_source();
        for lane in 0..if dual { 2 } else { 1 } {
            let label = if dual {
                format!("Meter {}", lane_name(lane))
            } else {
                "Meter".to_owned()
            };
            ui.push_id(lane, |ui| {
                options_field(ui, &label, |ui| {
                    if options_choice(
                        ui,
                        "meter",
                        &mut self.meter_modes[lane],
                        MeterMode::OPTIONS
                            .into_iter()
                            .map(|mode| (mode, mode.unit().to_owned())),
                    ) {
                        self.save_preference(
                            if lane == 0 { Key::Meter } else { Key::MeterB },
                            self.meter_modes[lane].unit(),
                        );
                    }
                })
            });
            if self.meter_modes[lane] == MeterMode::Lufs
                && self
                    .controller
                    .track_channels(lane)
                    .is_some_and(|channels| channels > 2)
            {
                options_note(ui, Language::current().no_loudness());
            }
        }
    }

    fn source_options(&mut self, ui: &mut egui::Ui) -> Option<OptionsAction> {
        let language = Language::current();
        let mut action = None;
        let mut go_to_addons = false;
        // A dropped file that more than one add-on opens waits here for the user's choice.
        if let Some((id, path)) = self.addon_sources.show_question(ui, language) {
            self.open_in_addon(&id, path);
        }
        // The list opened on a spinner and was measured for that; give the scanned rows room.
        if std::mem::take(&mut self.source_menu_outdated)
            && let Some(menu) = self.source_menu
        {
            remeasure_options_menu(ui.ctx(), menu);
        }
        let history_note = self.short_history_note();
        let choices = self.available_sources.as_slice();
        let scanning = self.source_list_worker.is_some();
        let mut source_menu_opened = false;
        let mut open_menu = None;
        let mut sources = self.controller.sources().clone();
        let dual = sources[1].is_some();
        let mut names_changed = false;
        ui.add_enabled_ui(self.gesture.is_none(), |ui| {
            for (lane, settings) in sources.iter_mut().enumerate() {
                ui.push_id(lane, |ui| {
                    options_group_heading(ui, &format!("Source {}", lane_name(lane)));
                    let configured = settings.clone();
                    let mut source = configured.as_ref().map(|settings| settings.source.clone());
                    options_field(ui, "Audio source", |ui| {
                        let source_menu = "source";
                        let was_open = egui::ComboBox::is_open(
                            ui.ctx(),
                            ui.make_persistent_id(egui::IdSalt::new(source_menu)),
                        );
                        let label = source
                            .as_ref()
                            .map_or_else(
                                || std::borrow::Cow::Borrowed("Off"),
                                |source| {
                                    source_picker_label(
                                        choices
                                            .iter()
                                            .find(|choice| choice.same_source(source))
                                            .unwrap_or(source),
                                        choices,
                                    )
                                },
                            )
                            .into_owned();
                        let width = options_control_width(ui);
                        egui::ComboBox::from_id_salt(source_menu)
                            .selected_text(&label)
                            .width(width)
                            .height(options_menu_height(
                                ui,
                                choices.len() + usize::from(lane == 1),
                            ))
                            .truncate()
                            .show_ui(ui, |ui| {
                                configure_options_menu(ui, width);
                                open_menu = Some(ui.layer_id());
                                if lane == 1 {
                                    ui.selectable_value(&mut source, None, "Off");
                                }
                                source_menu_opened |= !was_open;
                                if !was_open || scanning {
                                    ui.spinner();
                                    return;
                                }
                                for choice in choices {
                                    let selected = source
                                        .as_ref()
                                        .is_some_and(|source| choice.same_source(source));
                                    let label = source_picker_label(choice, choices);
                                    let row = ui.selectable_label(selected, label.as_ref());
                                    if row.clicked() && !selected {
                                        source = Some(choice.clone());
                                    }
                                    match choice {
                                        CaptureSource::Application { path, .. } => {
                                            row.on_hover_text(path);
                                        }
                                        CaptureSource::Sender { id, .. } => {
                                            row.on_hover_text(format!("Sender instance {id}"));
                                        }
                                        _ => {}
                                    }
                                }
                            })
                            .response
                            .on_hover_text(label);
                    });
                    let same_source = source
                        .as_ref()
                        .zip(configured.as_ref())
                        .is_some_and(|(source, configured)| source.same_source(&configured.source));
                    *settings = source.map(|source| SourceSettings {
                        source,
                        route: configured
                            .filter(|_| same_source)
                            .map_or(ChannelRoute::Merged, |settings| settings.route),
                    });
                    let mut show_route = true;
                    let mut covered = false;
                    if let Some(settings) = settings
                        && let CaptureSource::Addon {
                            addon_id,
                            source_id,
                            name,
                        } = &settings.source
                    {
                        // An add-on source offers its formats where an ordinary one offers
                        // routes. The routes stay for a format of one or two channels.
                        let channels = self.controller.track_channels(lane);
                        let history = history_note
                            .clone()
                            .filter(|_| channels.is_some_and(|count| count > 2));
                        let outcome = self.addon_sources.show_lane(
                            ui,
                            self.controller.addons(),
                            language,
                            self.accent,
                            lane,
                            (addon_id, source_id, name),
                            channels,
                            history,
                        );
                        show_route = outcome.show_route;
                        covered = outcome.covered;
                        go_to_addons |= outcome.add_ons_page;
                        if let Some(id) = outcome.open_file {
                            action = Some(OptionsAction::OpenAddonFile(id));
                        }
                    }
                    if let Some(settings) = settings.as_mut().filter(|_| show_route) {
                        // All channels is listed only for a device that has more than two,
                        // and, disabled, while it is the stored choice of one that has not.
                        let routes: Vec<(ChannelRoute, bool)> = ChannelRoute::ALL
                            .into_iter()
                            .map(|route| {
                                let available =
                                    same_source && self.controller.route_available(lane, route);
                                (route, available)
                            })
                            .filter(|&(route, available)| {
                                route != ChannelRoute::AllChannels
                                    || available
                                    || settings.route == route
                            })
                            .collect();
                        options_field(ui, "Channel route", |ui| {
                            let width = options_control_width(ui);
                            egui::ComboBox::from_id_salt("route")
                                .selected_text(route_label(settings.route, language))
                                .width(width)
                                .height(options_menu_height(ui, routes.len()))
                                .truncate()
                                .show_ui(ui, |ui| {
                                    configure_options_menu(ui, width);
                                    for &(route, enabled) in &routes {
                                        ui.add_enabled_ui(enabled, |ui| {
                                            ui.selectable_value(
                                                &mut settings.route,
                                                route,
                                                route_label(route, language),
                                            );
                                        });
                                    }
                                });
                        });
                    }
                    // Every source names its files, a wide add-on source without routes too.
                    if let Some(settings) = settings.as_ref() {
                        // The name belongs to the device, so it follows it to the other lane.
                        let source = &settings.source;
                        options_field(ui, "File name", |ui| {
                            let mut name =
                                self.source_names.get(source).unwrap_or_default().to_owned();
                            let field = egui::TextEdit::singleline(&mut name)
                                .id_salt("file-name")
                                .desired_width(options_control_width(ui))
                                .char_limit(SOURCE_NAME_LIMIT)
                                .hint_text(source.default_file_name());
                            if ui.add(field).changed() {
                                names_changed |= self.source_names.set(source, &name);
                            }
                        });
                        let letter = dual.then(|| Lane::from_index(lane)).flatten();
                        let prefix =
                            TakeLabel::new(source_file_name(&self.source_names, source), letter)
                                .file_prefix();
                        options_note(ui, format!("Files start with {prefix}"));
                    }
                    if !covered && let Some(status) = self.controller.source_status(lane) {
                        options_note(ui, language.source_message(status));
                    }
                    if lane == 0 {
                        ui.separator();
                    }
                });
            }
        });
        self.source_menu = open_menu;
        if names_changed {
            self.save_preference(Key::SourceNames, self.source_names.serialize());
        }
        if source_menu_opened {
            self.refresh_source_list();
        }
        if sources != *self.controller.sources() {
            if sources[1].is_none() {
                // The next pair of sources starts with every row edited together.
                self.separate_rows = false;
            }
            self.save_preference(Key::CaptureSources, serialize_sources(&sources));
            self.controller.set_sources(sources);
            self.sync_generation();
        }
        if go_to_addons {
            self.options_section = OptionsSection::Addons;
        }
        action
    }

    /// Sends a file to an add-on and says so in the status line when that fails.
    fn open_in_addon(&mut self, id: &str, path: PathBuf) {
        let Some(handle) = self.controller.addons().cloned() else {
            return;
        };
        let file = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        if let Err(reason) = self.addon_sources.open_file(&handle, id, path) {
            self.message = Language::current().could_not_open_file(&file, &reason);
        }
    }

    /// The "Open file…" button: a file dialog for the types the add-on declared.
    fn pick_file_for_addon(&mut self, frame: &eframe::Frame, id: &str) {
        let language = Language::current();
        let Some((name, files)) = self.addon_sources.open_files_of(id) else {
            return;
        };
        let extensions: Vec<&str> = files
            .extensions
            .iter()
            .map(|extension| extension.trim_start_matches('.'))
            .collect();
        let chosen = rfd::FileDialog::new()
            .set_title(language.open_file_title(&name))
            .add_filter(files.description.as_str(), &extensions)
            .set_parent(frame)
            .pick_file();
        if let Some(path) = chosen {
            self.open_in_addon(id, path);
        }
        self.last_interaction = Instant::now();
    }

    /// Files dropped on a window that are not add-on packages go to the add-on that opens them.
    fn route_dropped_files(
        &mut self,
        ctx: &egui::Context,
        paths: &[PathBuf],
        on_main_window: bool,
    ) {
        let handle = self.controller.addons().cloned();
        match self
            .addon_sources
            .drop_files(handle.as_ref(), paths, Language::current())
        {
            addon_sources::DropResult::Ignored | addon_sources::DropResult::Opening => {}
            addon_sources::DropResult::Message(text) => self.message = text,
            addon_sources::DropResult::Choose => {
                self.options_section = OptionsSection::Sources;
                if !on_main_window {
                    return;
                }
                if self.options_open {
                    self.options_focus_requested = true;
                } else {
                    self.open_options(ctx);
                }
            }
        }
    }

    fn refresh_source_list(&mut self) {
        if self.source_list_worker.is_some() {
            self.source_list_refresh_pending = true;
            return;
        }
        let addons = self.controller.addons().cloned();
        match thread::Builder::new()
            .name("backtracking-sources".into())
            .spawn(move || match addons {
                Some(addons) => Controller::available_sources_with_addons(&addons.snapshot()),
                None => Controller::available_sources(),
            }) {
            Ok(worker) => self.source_list_worker = Some(worker),
            Err(error) => self.message = format!("Could not list audio sources: {error}"),
        }
    }

    fn collect_source_list(&mut self) {
        if self
            .source_list_worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
            && let Some(worker) = self.source_list_worker.take()
        {
            match worker.join() {
                Ok(sources) => {
                    self.available_sources = sources;
                    self.source_menu_outdated = self.source_menu.is_some();
                }
                Err(_) => self.message = "Source list worker stopped unexpectedly.".into(),
            }
            if std::mem::take(&mut self.source_list_refresh_pending) {
                self.refresh_source_list();
            }
        }
    }

    /// The drag-and-drop folder when old exports may be deleted from it. Saved snapshots are
    /// never deleted, so a folder that also holds them is left alone.
    fn output_cleanup_folder(&self) -> Option<&std::path::Path> {
        (self.primary && !cleanup::same_folder(&self.output, &self.saved_output))
            .then_some(self.output.as_path())
    }

    fn clean_output_folder(&mut self) {
        // Only one window cleans, and only when the user chose an age: the common case costs
        // nothing here.
        if !self.primary || self.output_cleanup.age().is_none() {
            return;
        }
        let now = Instant::now();
        // The folder check reads the disk, so it waits until a pass is due.
        let (folder, keep) = if self.output_cleanup.due(now) {
            (
                self.output_cleanup_folder().map(std::path::Path::to_owned),
                // A file still on offer for a drag stays, however long it has waited.
                self.exported
                    .as_ref()
                    .map_or_else(Vec::new, |file| file.paths.clone()),
            )
        } else {
            (None, Vec::new())
        };
        match self.output_cleanup.poll(folder.as_deref(), &keep, now) {
            Some(Ok(0)) | None => {}
            Some(Ok(1)) => self.message = "Deleted 1 old file from the drag-and-drop folder".into(),
            Some(Ok(count)) => {
                self.message = format!("Deleted {count} old files from the drag-and-drop folder");
            }
            Some(Err(error)) => {
                self.message = format!("Could not clean the drag-and-drop folder: {error}");
            }
        }
    }

    fn output_cleanup_options(&mut self, ui: &mut egui::Ui) {
        let mut age = self.output_cleanup.age();
        let note = match age {
            None => "Files you drag into other programs stay in this folder.".to_owned(),
            Some(_) if self.output_cleanup_folder().is_none() => {
                "Not active while this is also the saved snapshots folder.".to_owned()
            }
            Some(days) => format!(
                "Backtracking's WAV files in this folder are permanently deleted {days} days \
                 after they were written. A project that still uses one loses it."
            ),
        };
        let changed = options_field(ui, "Delete old drag-and-drop files", |ui| {
            let choices = std::iter::once(None).chain(cleanup::AGE_CHOICES.map(Some));
            let changed = options_choice(
                ui,
                "output-cleanup",
                &mut age,
                choices.map(|age| (age, cleanup::age_label(age))),
            );
            options_note(ui, note);
            changed
        });
        if changed {
            self.output_cleanup.set_age(age, Instant::now());
            // Off is stored as 0, like the other timed choices.
            self.save_preference(Key::OutputCleanup, age.unwrap_or(0).to_string());
        }
    }

    fn folder_options(&mut self, ui: &mut egui::Ui) -> Option<OptionsAction> {
        let waveform = options_field(ui, "Drag-and-drop folder", |ui| {
            options_value(ui, &self.output.display().to_string());
            ui.horizontal_wrapped(|ui| {
                let select = ui.button("Choose folder…").clicked();
                let open = ui.button("Open folder").clicked();
                if select {
                    Some(OptionsAction::SelectSaveFolder)
                } else {
                    open.then_some(OptionsAction::GoToSaveFolder)
                }
            })
            .inner
        });
        // Other capture windows share the folder but leave its housekeeping to the main one.
        if self.primary {
            self.output_cleanup_options(ui);
        }
        let snapshot = options_field(ui, "Saved snapshots folder", |ui| {
            options_value(ui, &self.saved_output.display().to_string());
            ui.horizontal_wrapped(|ui| {
                let select = ui.button("Choose folder…").clicked();
                let open = ui.button("Open folder").clicked();
                if select {
                    Some(OptionsAction::SelectSnapshotFolder)
                } else {
                    open.then_some(OptionsAction::OpenSnapshotFolder)
                }
            })
            .inner
        });
        waveform.or(snapshot)
    }

    fn app_options(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) -> Option<OptionsAction> {
        options_group_heading(ui, "Application");
        let mut enabled = self.launch_at_login;
        if self.primary && ui.checkbox(&mut enabled, "Start with Windows").changed() {
            self.set_launch_at_login(enabled, frame);
        }
        let visible = frame
            .winit_window()
            .is_none_or(|window| !window_is_hidden(window));
        let tray_available = self.tray_available;
        ui.horizontal_wrapped(|ui| {
            let visibility = if visible {
                ui.add_enabled(tray_available, egui::Button::new("Hide waveform"))
                    .clicked()
                    .then_some(OptionsAction::Hide)
            } else {
                ui.button("Show waveform")
                    .clicked()
                    .then_some(OptionsAction::Show)
            };
            let quit = ui.button("Quit Backtracking").clicked();
            visibility.or_else(|| quit.then_some(OptionsAction::Quit))
        })
        .inner
    }

    fn select_save_folder(&mut self, frame: &eframe::Frame) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Select save folder")
            .set_parent(frame)
            .pick_folder()
        {
            if folder != self.output {
                self.output = folder;
                self.invalidate_export();
                self.output_cleanup.restart(Instant::now());
            }
            let saved = self
                .output
                .to_str()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "output path cannot be encoded as UTF-8",
                    )
                })
                .and_then(|path| self.write_preference(Key::Output, path));
            self.message = match saved {
                Ok(()) => format!("Output: {}", self.output.display()),
                Err(error) => format!("Could not save settings: {error}"),
            };
        }
        self.last_interaction = Instant::now();
    }

    fn select_saved_snapshot_folder(&mut self, frame: &eframe::Frame) {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Select snapshot folder")
            .set_directory(&self.saved_output)
            .set_parent(frame)
            .pick_folder()
        else {
            return;
        };
        let saved = folder
            .to_str()
            .ok_or_else(|| io::Error::other("snapshot folder path is not valid UTF-8"))
            .and_then(|path| self.write_preference(Key::SavedSnapshots, path));
        self.saved_output = folder;
        self.output_cleanup.restart(Instant::now());
        self.message = match saved {
            Ok(()) => format!("Snapshot folder: {}", self.saved_output.display()),
            Err(error) => format!("Could not save settings: {error}"),
        };
    }

    fn addon_options(&mut self, ui: &mut egui::Ui) -> Option<OptionsAction> {
        let handle = self.controller.addons();
        match self
            .addons_page
            .show(ui, handle, Language::current(), self.accent)?
        {
            PageRequest::PickFile => Some(OptionsAction::PickAddonFile),
            PageRequest::OpenLink(link) => Some(OptionsAction::OpenLink(link)),
        }
    }

    /// Shows the confirmation for a dropped or chosen package on the Add-ons page.
    fn review_addon(&mut self, dropped: addons_page::DroppedPackage) {
        self.addons_page.review_package(
            dropped.path,
            dropped.others,
            self.controller.addons(),
            Language::current(),
        );
        self.options_section = OptionsSection::Addons;
    }

    fn pick_addon_file(&mut self, frame: &eframe::Frame) {
        let language = Language::current();
        let chosen = rfd::FileDialog::new()
            .set_title(language.file_dialog_title())
            .add_filter(language.file_filter(), &["btaddon"])
            .set_parent(frame)
            .pick_file();
        if let Some(path) = chosen {
            self.review_addon(addons_page::DroppedPackage { path, others: 0 });
        }
        self.last_interaction = Instant::now();
    }

    /// Opens an add-on's homepage in the default browser, only after the user clicked it.
    fn open_link(&mut self, link: &str) {
        use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        use windows::core::{HSTRING, PCWSTR, w};
        if !addons_page::link_is_openable(link) {
            return;
        }
        let file = HSTRING::from(link);
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &file,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecute reports success with a value above 32.
        if result.0 as isize <= 32 {
            self.message =
                Language::current().could_not_open_link(&io::Error::last_os_error().to_string());
        }
    }

    fn open_saved_snapshot_folder(&mut self, frame: &eframe::Frame) {
        let result = std::fs::create_dir_all(&self.saved_output)
            .and_then(|()| Command::new("explorer.exe").arg(&self.saved_output).spawn());
        if let Err(error) = result {
            self.message = format!("Could not open snapshot folder: {error}");
            rfd::MessageDialog::new()
                .set_title("Could not open snapshot folder")
                .set_description(error.to_string())
                .set_level(rfd::MessageLevel::Error)
                .set_parent(frame)
                .show();
        }
    }

    fn wav_settings(&mut self, ui: &mut egui::Ui) {
        let previous = self.wav_profile;
        options_field(ui, "WAV sample rate", |ui| {
            options_choice(
                ui,
                "wav-rate",
                &mut self.wav_profile.sample_rate,
                WAV_SAMPLE_RATES
                    .into_iter()
                    .map(|(rate, label)| (rate, label.to_owned())),
            );
        });
        options_field(ui, "WAV encoding", |ui| {
            options_choice(
                ui,
                "wav-encoding",
                &mut self.wav_profile.sample_format,
                WAV_SAMPLE_FORMATS
                    .into_iter()
                    .map(|(format, label)| (format, label.to_owned())),
            );
        });
        let mono_changed = ui
            .checkbox(&mut self.wav_force_mono, "Export every file as mono")
            .changed();
        let language = Language::current();
        options_field(ui, language.wide_export_field(), |ui| {
            // A test copy can photograph the list open.
            if std::mem::take(&mut self.force_wide_export_menu) {
                let combo = ui.make_persistent_id(egui::IdSalt::new("wide-export"));
                egui::Popup::open_id(ui.ctx(), combo.with("popup"));
            }
            if options_choice(
                ui,
                "wide-export",
                &mut self.wide_export,
                WideExportMode::ALL
                    .into_iter()
                    .map(|mode| (mode, language.wide_export_mode(mode).to_owned())),
            ) {
                self.save_preference(Key::WideExport, self.wide_export.preference());
                self.invalidate_export();
            }
        });
        options_note(ui, language.wide_export_note());
        if self.wav_profile != previous || mono_changed {
            self.invalidate_export();
            self.message.clear();
            if mono_changed {
                self.save_preference(
                    Key::WavForceMono,
                    if self.wav_force_mono { "1" } else { "0" },
                );
            }
            if self.wav_profile.sample_rate != previous.sample_rate {
                self.save_preference(Key::WavSampleRate, self.wav_profile.sample_rate.to_string());
            }
            if self.wav_profile.sample_format != previous.sample_format {
                self.save_preference(
                    Key::WavSampleFormat,
                    wav_sample_format_preference(self.wav_profile.sample_format),
                );
            }
        }
    }

    fn accent_settings(&mut self, ui: &mut egui::Ui) {
        options_field(ui, "Theme color", |ui| {
            let [r, g, b, _] = self.accent.to_array();
            let mut color = [r, g, b];
            let (changed, reset) = ui
                .horizontal(|ui| {
                    (
                        ui.color_edit_button_srgb(&mut color).changed(),
                        ui.button("Reset").clicked(),
                    )
                })
                .inner;
            let mut accent = accent_color(color);
            if reset {
                accent = accent_color(DEFAULT_ACCENT);
            }
            if (changed || reset) && accent != self.accent {
                self.accent = accent;
                apply_accent(ui.ctx(), accent);
            }
        });
    }

    fn poll_snapshot_hotkey(&mut self) {
        if self
            .plugin_snapshot_request
            .as_ref()
            .is_some_and(|job| job.is_finished())
            && let Some(job) = self.plugin_snapshot_request.take()
        {
            self.message = match job.join() {
                Ok(Ok(())) => "Snapshot requested from the selected plugin.".into(),
                Ok(Err(error)) => format!("Could not request plugin snapshot: {error}"),
                Err(_) => "Plugin snapshot request stopped unexpectedly.".into(),
            };
        }
        while let Some(event) = self.hotkey.as_ref().and_then(SnapshotHotkey::try_next) {
            match event {
                snapshot_hotkey::Event::Availability(result) => {
                    self.hotkey_error = result.err();
                }
                snapshot_hotkey::Event::Save => {
                    if let Some(target) = self.snapshot_target.clone() {
                        self.pending_plugin_snapshots.push_back(target);
                    } else {
                        self.save_snapshot();
                    }
                }
            }
        }
        if self.plugin_snapshot_request.is_none()
            && let Some(target) = self.pending_plugin_snapshots.pop_front()
        {
            self.request_plugin_snapshot(target);
        }
    }

    fn request_plugin_snapshot(&mut self, target: String) {
        match thread::Builder::new()
            .name("backtracking-plugin-snapshot".into())
            .spawn(move || backtracking_core::request_sender_snapshot(&target))
        {
            Ok(job) => self.plugin_snapshot_request = Some(job),
            Err(error) => self.message = format!("Could not request plugin snapshot: {error}"),
        }
    }

    fn save_snapshot(&mut self) {
        self.save_snapshot_to(self.saved_output.clone());
    }

    /// Saves the selection, or everything held without one, into `output`: the Save command
    /// writes to the saved-snapshot folder, and a test copy's `save=` step to its own.
    fn save_snapshot_to(&mut self, output: PathBuf) {
        if self.gesture.is_some() {
            return;
        }
        self.controller.poll();
        self.sync_generation();
        let has_selection = self.selection.is_some();
        let rows: Vec<RowSource> = self
            .selected_rows()
            .filter(|_| has_selection)
            .map(<[RowSource]>::to_vec)
            .unwrap_or_default();
        // A lane the selection held whole when it was made is saved whole.
        let whole = if rows.is_empty() {
            [false; 2]
        } else {
            self.selection_whole
        };
        let spec = SnapshotSpec {
            whole,
            selection: self.selection.clone(),
            fades: if has_selection {
                self.fades
            } else {
                Fades::NONE
            },
            output,
            format: self.clip_format(None),
            flags: (!has_selection).then(|| self.flag_update(0..0, Vec::new(), [false; 2])),
            export_options: self.export_options(),
            // Separate rows save the selection's own rows, and every row without a selection.
            rows,
        };
        if let Some(edit) = &self.editing {
            if let Some(clip) = spec.clip(Arc::clone(&edit.snapshots), &edit.lanes, &edit.groups) {
                self.pending_snapshot_saves
                    .push_back(clip.with_positions(&edit.positions));
            } else {
                self.message = "The selected audio is unavailable.".into();
                return;
            }
        } else if let Some(request) = &mut self.edit_request {
            request.saves.push(spec);
            self.message = "Preparing snapshot…".into();
        } else {
            self.pending_snapshot_reads.push(SnapshotRead {
                result: self.controller.request_edit_snapshot(),
                saves: vec![spec],
            });
            self.message = "Preparing snapshot…".into();
        }
        if has_selection {
            self.clear_snapshot();
        }
        self.collect_snapshot_saves();
    }

    fn export_options(&self) -> ExportOptions {
        let mut options = self.editing.as_ref().map_or_else(
            || ExportOptions {
                groups: std::array::from_fn(|index| {
                    self.controller
                        .active_lanes()
                        .get(index)
                        .and_then(|&lane| self.controller.shown_group(lane))
                        .cloned()
                }),
                wide_mode: self.wide_export,
                addons: self.controller.addons().cloned(),
            },
            |edit| edit.export_options.clone(),
        );
        options.wide_mode = self.wide_export;
        options
    }

    fn wav_clip(&mut self, snapshots: Arc<[FrozenTrack]>, output: PathBuf) -> Option<WavClip> {
        // Separate rows drag out the selection's own rows, or without one the row the drag
        // started in.
        let rows = clip_rows(
            &self.lanes,
            self.rows_separate(),
            self.selection
                .is_some()
                .then_some(self.selection_rows.as_slice()),
            self.drag_row,
        );
        // A lane the selection held whole when it was made is dragged out whole; without a
        // selection, the row the drag took is the lane whole when it is all the lane shows.
        let whole = if self.selection.is_some() {
            self.selection_whole
        } else {
            lanes_chosen_whole(&self.lanes, &rows)
        };
        let flags = if self.selection.is_none() {
            Some(self.flag_update(snapshots.first()?.frame_range(), rows.clone(), whole))
        } else {
            None
        };
        let lanes = self.editing.as_ref().map_or_else(
            || self.controller.active_lanes(),
            |edit| edit.lanes.as_slice(),
        );
        let groups = self.editing.as_ref().map_or_else(
            || [0, 1].map(|lane| self.controller.lane_groups(lane).to_vec()),
            |edit| edit.groups.clone(),
        );
        WavClip::new(
            snapshots,
            lanes,
            self.selection.as_ref(),
            if self.selection.is_some() {
                self.fades
            } else {
                Fades::NONE
            },
            output,
            self.clip_format(None),
            flags,
        )
        .and_then(|mut clip| {
            clip.export_options = self.export_options();
            clip.with_rows(&rows, &groups)
        })
        .map(|clip| clip.whole_lanes(whole))
        .map(|clip| match &self.editing {
            Some(edit) if Arc::ptr_eq(&edit.snapshots, &clip.snapshots) => {
                clip.with_positions(&edit.positions)
            }
            _ => clip,
        })
    }

    /// How a clip of the current recording is written. `lane` saves only that one of two lanes.
    fn clip_format(&self, lane: Option<usize>) -> ClipFormat {
        let sources = self.controller.sources();
        ClipFormat {
            profile: self.wav_profile,
            routes: self.routes(),
            force_mono: self.wav_force_mono,
            names: sources.each_ref().map(|settings| {
                settings.as_ref().map_or_else(String::new, |settings| {
                    source_file_name(&self.source_names, &settings.source).to_owned()
                })
            }),
            lane,
            pair_binaural: self.pair_render.binaural(),
            hide_silent: [0, 1].map(|lane| self.hides_silent_rows(lane)),
        }
    }

    fn routes(&self) -> [ChannelRoute; 2] {
        self.controller.sources().each_ref().map(|settings| {
            settings
                .as_ref()
                .map_or(ChannelRoute::Merged, |settings| settings.route)
        })
    }

    fn collect_snapshot_saves(&mut self) {
        let mut index = 0;
        while index < self.pending_edit_save_reads.len() {
            let result = match self.pending_edit_save_reads[index].result.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Disconnected) => None,
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
            };
            let read = self.pending_edit_save_reads.remove(index);
            if let Some(snapshot) =
                result.and_then(|snapshot| self.accept_edit_snapshot(snapshot, read.generation))
            {
                for spec in read.saves {
                    if let Some(clip) = spec.frozen_clip(&snapshot) {
                        self.pending_snapshot_saves.push_back(clip);
                    } else {
                        self.message = "The selected audio is unavailable.".into();
                    }
                }
                self.retire(snapshot);
            } else {
                self.message = "The capture source changed before this selection was ready.".into();
            }
        }
        let mut index = 0;
        while index < self.pending_snapshot_reads.len() {
            let result = match self.pending_snapshot_reads[index].result.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Disconnected) => None,
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
            };
            let read = self.pending_snapshot_reads.remove(index);
            if let Some(snapshot) = result {
                for spec in read.saves {
                    if let Some(clip) = spec.frozen_clip(&snapshot) {
                        self.pending_snapshot_saves.push_back(clip);
                    } else {
                        self.message = "The selected audio is unavailable.".into();
                    }
                }
                self.retire(snapshot);
            } else {
                self.message = "No audio captured yet.".into();
            }
        }
        if self
            .snapshot_save
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            return;
        }
        if let Some(worker) = self.snapshot_save.take() {
            self.message = match worker.join() {
                Ok(Ok(mut file)) => {
                    if let Some(flags) = file.flags.take() {
                        self.apply_flags(flags);
                    }
                    if let Some(harness) = &mut self.harness {
                        harness.note_saved(&file.paths);
                    }
                    file.saved_message(Language::current(), true)
                }
                Ok(Err(error)) => format!("Could not save snapshot: {error}"),
                Err(_) => "Snapshot worker stopped unexpectedly.".into(),
            };
            // The log holds what the status line says, saved or not.
            if let Some(harness) = &mut self.harness {
                harness.note(format!("status: {}", self.message));
            }
        }
        if let Some(clip) = self.pending_snapshot_saves.pop_front() {
            match spawn_wav_write("backtracking-snapshot", clip, true) {
                Ok(worker) => {
                    self.snapshot_save = Some(worker);
                    self.message = "Saving snapshot…".into();
                }
                Err((error, clip)) => {
                    self.retire(clip);
                    self.message = format!("Could not start snapshot save: {error}");
                }
            }
        }
    }

    fn start_export(&mut self) {
        if self.editing.is_none() && self.edit_request.is_none() {
            self.begin_editing();
        }
        if self.edit_request.is_some() {
            self.export_requested = true;
            self.message = PREPARING_WAV.into();
            return;
        }
        self.export_requested = false;
        let output = self.output.clone();
        let Some(wav) = self
            .current_snapshots()
            .and_then(|snapshot| self.wav_clip(snapshot, output))
        else {
            self.message = "No audio captured yet.".into();
            return;
        };
        match spawn_wav_write("backtracking-wav", wav, false) {
            Ok(worker) => {
                self.export = Some(ExportJob {
                    worker,
                    revision: self.clip_revision,
                });
                self.message = PREPARING_WAV.into();
            }
            Err((error, clip)) => {
                self.retire(clip);
                self.message = format!("Could not prepare WAV: {error}");
            }
        }
    }

    fn collect_export(&mut self) {
        if self
            .export
            .as_ref()
            .is_some_and(|job| !job.worker.is_finished())
        {
            return;
        }
        if let Some(job) = self.export.take() {
            let result = job.worker.join();
            if job.revision == self.clip_revision {
                match result {
                    Ok(Ok(file)) => self.exported = Some(file),
                    Ok(Err(error)) => self.message = format!("Could not prepare WAV: {error}"),
                    Err(_) => self.message = "WAV export worker stopped unexpectedly.".into(),
                }
            }
        }
    }

    fn timeline_rect(&self, rect: Rect) -> Rect {
        if self.waveform_only {
            rect.shrink2(Vec2::new(WAVEFORM_INSET, 0.0))
        } else if self.stacked() {
            // Channel names need a wider column than "L" and "R".
            let inner = rect.shrink(WAVEFORM_INSET);
            Rect::from_min_max(
                Pos2::new(
                    (rect.left() + stack::LABEL_GUTTER).min(inner.right()),
                    inner.top(),
                ),
                inner.max,
            )
        } else {
            rect.shrink(WAVEFORM_INSET)
        }
    }

    fn audio_rect(&self, rect: Rect) -> Rect {
        let inner = self.timeline_rect(rect);
        if self.visible_frames != self.history_frames {
            return inner;
        }
        let history_seconds = (self.history_frames.end - self.history_frames.start) as f32
            / self.sample_rate.max(1) as f32;
        let retention = self
            .editing
            .as_ref()
            .map(|edit| edit.retention_seconds)
            .or_else(|| {
                self.edit_request
                    .as_ref()
                    .map(|request| request.retention_seconds)
            })
            .unwrap_or_else(|| self.plot_retention_seconds());
        let width = inner.width() * (history_seconds / retention.max(1.0)).clamp(0.0, 1.0);
        Rect::from_min_max(Pos2::new(inner.right() - width, inner.top()), inner.max)
    }

    fn interact_waveform(&mut self, ui: &egui::Ui, frame: &eframe::Frame, rect: Rect) {
        if self.history_frames.is_empty() {
            return;
        }
        if self.interact_timeline(ui, rect) {
            return;
        }
        let (position, press, alt_press, down, snap) = ui.input(|input| {
            let press = waveform_press(&input.events, ALT_SELECTS);
            // Whether Alt was held at that press: on a flag of the selection it drags a fade.
            let alt_press = input.events.iter().rev().find_map(|event| match event {
                egui::Event::PointerButton {
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers,
                    ..
                } => Some(modifiers.alt),
                _ => None,
            });
            (
                input.pointer.interact_pos(),
                press,
                alt_press == Some(true),
                input.pointer.button_down(PointerButton::Primary),
                input.modifiers.shift,
            )
        });

        if self.gesture.is_none()
            && self.scroll_drag.is_none()
            && let Some((position, selecting, snap_on_press)) = press.filter(|(point, _, _)| {
                rect.contains(*point)
                    && !self
                        .lane_controls
                        .iter()
                        .any(|control| control.contains(*point))
            })
        {
            let audio = self.audio_rect(rect);
            let resize = self.flag_endpoints(audio, position);
            if let Some(end) = alt_press.then(|| self.fade_flag(audio, position)).flatten() {
                self.last_interaction = Instant::now();
                self.gesture = Some(Gesture::Fade {
                    end,
                    press: self.free_frame_at(audio, position.x),
                    origin_x: position.x,
                    initial: self.fades,
                    moved: false,
                });
            } else if selecting || resize.is_some() {
                // A new or resized selection replaces the one that was playing.
                self.stop_preview();
                self.begin_editing();
                self.last_interaction = Instant::now();
                let start = self.frame_at(audio, position.x, snap_on_press);
                let (anchor, cursor) = resize.unwrap_or((start, start));
                // Flags left by an earlier export start a new selection, not a resize.
                let resizing = resize.is_some() && self.selection.is_some();
                let mut rows = None;
                if let Some(flags) = resize.and(self.flags.as_ref()).filter(|_| !resizing) {
                    // One taken back from flags covers the rows they were left on.
                    self.selection_rows = flags.rows.clone();
                    self.selection_whole = flags.whole;
                } else if !resizing {
                    // A new selection covers the rows its drag crosses, from the one it starts
                    // in, whenever rows are separate.
                    let boxes = self.lane_boxes(audio.height());
                    if let Some(row) = row_at_height(&self.lanes, &boxes, position.y - audio.top())
                    {
                        self.choose_rows(rows_between(&self.lanes, row, row));
                        rows = Some((row, row));
                    } else {
                        self.choose_rows(Vec::new());
                    }
                }
                self.edge_scroll = None;
                self.gesture = Some(Gesture::Select {
                    anchor,
                    cursor,
                    origin_x: position.x,
                    moved: false,
                    resizing,
                    rows,
                });
            } else {
                let boxes = self.lane_boxes(audio.height());
                self.drag_row = row_at_height(&self.lanes, &boxes, position.y - audio.top())
                    .filter(|_| self.rows_separate());
                self.gesture = Some(Gesture::Export { preparing: false });
            }
        }

        let audio_rect = self.audio_rect(rect);
        if let Some(position) = position
            && let Some(Gesture::Fade {
                end,
                press,
                origin_x,
                initial,
                moved,
            }) = self.gesture
            && let Some(selection) = self.selection.clone()
        {
            let moved = moved || (position.x - origin_x).abs() >= 2.0;
            if moved {
                let pointer = self.free_frame_at(audio_rect, position.x);
                self.fades = dragged_fades(end, &selection, initial, press, pointer);
                // A playing preview takes the new ramps from its next block.
                if let Some(player) = &self.preview {
                    player.preview().set_fades(self.fades);
                }
            }
            self.gesture = Some(Gesture::Fade {
                end,
                press,
                origin_x,
                initial,
                moved,
            });
        }
        if let Some(position) = position
            && matches!(self.gesture, Some(Gesture::Select { .. }))
        {
            let endpoint = self.frame_at(audio_rect, position.x, snap);
            if let Some(Gesture::Select {
                rows: Some((anchor, at)),
                ..
            }) = self.gesture
            {
                let at = self.drag_across_rows(audio_rect, position.y, at);
                self.choose_rows(rows_between(&self.lanes, anchor, at));
                // A pointer held still past the edge keeps the lane scrolling.
                if let Some(next) = self.edge_scroll {
                    ui.ctx()
                        .request_repaint_after(next.saturating_duration_since(Instant::now()));
                }
                if let Some(Gesture::Select { rows, .. }) = &mut self.gesture {
                    *rows = Some((anchor, at));
                }
            }
            if let Some(Gesture::Select {
                cursor,
                origin_x,
                moved,
                ..
            }) = &mut self.gesture
            {
                *moved |= (position.x - *origin_x).abs() >= 2.0;
                if *moved {
                    *cursor = endpoint;
                }
            }
        }

        // egui may lose its local pointer position when the cursor exits the viewport.
        let held = match self.gesture {
            Some(Gesture::Export { .. }) => file_drag::primary_button_down(),
            _ => down,
        };
        if !held && let Some(gesture) = self.gesture.take() {
            self.edge_scroll = None;
            let keep_selection = match gesture {
                Gesture::Select {
                    anchor,
                    cursor,
                    moved,
                    resizing,
                    ..
                } => {
                    let range = anchor.min(cursor)..anchor.max(cursor);
                    if moved && !range.is_empty() {
                        let flags = self.flag_update(
                            range.clone(),
                            self.selection_rows.clone(),
                            self.selection_whole,
                        );
                        self.apply_flags(flags);
                        // A resized selection keeps the ramps at its flags; a new one has none.
                        self.fades = if resizing {
                            self.fades.fitted(range.end - range.start)
                        } else {
                            Fades::NONE
                        };
                        self.selection = Some(range);
                        self.invalidate_export();
                    } else {
                        self.selection = None;
                    }
                    self.selection.is_some()
                }
                Gesture::Fade { initial, .. } => {
                    if self.fades != initial {
                        self.invalidate_export();
                    }
                    true
                }
                Gesture::Export { .. } => {
                    self.invalidate_export();
                    false
                }
            };
            self.finish_waveform_gesture(keep_selection);
            return;
        }

        let Some(Gesture::Export { preparing }) = self.gesture else {
            return;
        };
        let Ok(window) = frame.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = window.as_raw() else {
            return;
        };
        let hwnd = HWND(handle.hwnd.get() as _);
        if !file_drag::cursor_outside_app(hwnd) {
            return;
        }
        if let Some(mut file) = self.exported.take() {
            self.gesture = None;
            let result = file_drag::drag_files(hwnd, &file.paths);
            self.invalidate_export();
            match result {
                Ok(true) => {
                    file.keep = true;
                    self.selection = None;
                    if let Some(flags) = file.flags.take() {
                        self.apply_flags(flags);
                    }
                    self.message = file.saved_message(Language::current(), false);
                    self.finish_waveform_gesture(false);
                }
                Ok(false) => self.finish_waveform_gesture(false),
                Err(error) => {
                    self.finish_waveform_gesture(false);
                    self.message = format!("Could not start file drag: {error}");
                }
            }
        } else if !preparing && self.export.is_none() {
            self.begin_editing();
            self.gesture = Some(Gesture::Export { preparing: true });
            self.start_export();
        }
    }

    /// Moves a drag across rows, now at row `at`, on to the pointer at `y`. While rows are
    /// separate and the pointer is past the top or bottom edge of the scrolling lane the drag
    /// is in, that lane scrolls one row every `EDGE_SCROLL_STEP` and the drag takes the row
    /// that comes into view. Returns the row the drag is at now.
    fn drag_across_rows(&mut self, audio: Rect, y: f32, at: RowSource) -> RowSource {
        let boxes = self.lane_boxes(audio.height());
        let offset = y - audio.top();
        let step = if self.rows_separate() {
            row_drag(&self.lanes, &boxes, at, offset)
        } else {
            // All rows are selected anyway; the rows crossed are only remembered.
            row_at_height(&self.lanes, &boxes, offset).map(RowDrag::To)
        };
        match step {
            Some(RowDrag::To(row)) => {
                self.edge_scroll = None;
                row
            }
            Some(RowDrag::Scroll { lane, down }) => {
                let now = Instant::now();
                if self.edge_scroll.is_some_and(|next| now < next) {
                    return at;
                }
                self.edge_scroll = Some(now + EDGE_SCROLL_STEP);
                let (Some(plan), Some(lane_box)) = (self.lanes.get(lane), boxes.get(lane)) else {
                    return at;
                };
                let logical = plan.lane;
                let first = if down {
                    lane_box.first + 1
                } else {
                    lane_box.first.saturating_sub(1)
                };
                self.scroll_lane_to(logical, first);
                let boxes = self.lane_boxes(audio.height());
                match (self.lanes.get(lane), boxes.get(lane)) {
                    (Some(plan), Some(lane_box)) if lane_box.shown > 0 => plan.row(if down {
                        lane_box.first + lane_box.shown - 1
                    } else {
                        lane_box.first
                    }),
                    _ => at,
                }
            }
            None => at,
        }
    }

    /// The group switch of each stacked lane with more than one group, at the lane's bottom
    /// left. When the buttons would cover too much of the lane, one button shows the group on
    /// show and moves to the next.
    fn group_chips(&self, ui: &egui::Ui, rect: Rect, boxes: &[LaneBox]) -> Vec<GroupChip> {
        let mut chips = Vec::new();
        if self.waveform_only {
            return chips;
        }
        let inner = self.timeline_rect(rect);
        let font = egui::FontId::proportional(10.0);
        for (plan, lane_box) in self.lanes.iter().zip(boxes) {
            let LaneKind::Stack { group, groups, .. } = &plan.kind else {
                continue;
            };
            if groups.len() < 2 || lane_box.height < 18.0 {
                continue;
            }
            let width = |label: &str| {
                ui.painter()
                    .layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
                    + 10.0
            };
            let bottom = inner.top() + lane_box.top + lane_box.height - 3.0;
            let mut x = inner.left() + 4.0;
            let total: f32 = groups.iter().map(|(_, label)| width(label)).sum();
            if total <= inner.width() * 0.6 {
                for (id, label) in groups {
                    let rect = Rect::from_min_size(
                        Pos2::new(x, bottom - 14.0),
                        Vec2::new(width(label), 14.0),
                    );
                    x = rect.right();
                    chips.push(GroupChip {
                        rect,
                        lane: plan.lane,
                        id: id.clone(),
                        label: label.clone(),
                        current: *id == group.id,
                    });
                }
            } else {
                let index = groups
                    .iter()
                    .position(|(id, _)| *id == group.id)
                    .unwrap_or(0);
                let (id, _) = &groups[(index + 1) % groups.len()];
                let label = groups
                    .get(index)
                    .map_or_else(|| group.label.clone(), |(_, label)| label.clone());
                let rect = Rect::from_min_size(
                    Pos2::new(x, bottom - 14.0),
                    Vec2::new(width(&label).min(inner.width() * 0.6), 14.0),
                );
                chips.push(GroupChip {
                    rect,
                    lane: plan.lane,
                    id: id.clone(),
                    label,
                    current: false,
                });
            }
        }
        chips
    }

    /// The compact transport of each lane whose add-on source reports one, at the lane's bottom
    /// left, after its group switch. It is left out of a lane too short or a window too narrow
    /// to take it without covering the waveform.
    fn transport_chips(&self, ui: &egui::Ui, rect: Rect, boxes: &[LaneBox]) -> Vec<TransportChip> {
        let mut chips = Vec::new();
        if self.waveform_only {
            return chips;
        }
        let inner = self.timeline_rect(rect);
        if inner.width() < TRANSPORT_MIN_WIDTH {
            return chips;
        }
        let groups = self.group_chips(ui, rect, boxes);
        let now = Instant::now();
        for (plan, lane_box) in self.lanes.iter().zip(boxes) {
            if lane_box.height < TRANSPORT_MIN_LANE_HEIGHT {
                continue;
            }
            let Some(CaptureSource::Addon {
                addon_id,
                source_id,
                ..
            }) = self
                .controller
                .sources()
                .get(plan.lane)
                .and_then(Option::as_ref)
                .map(|settings| &settings.source)
            else {
                continue;
            };
            let Some(chip) = self.addon_sources.chip(addon_id, source_id, now) else {
                continue;
            };
            let size = addon_sources::chip_size(ui.painter(), &chip.text);
            let left = groups
                .iter()
                .filter(|group| group.lane == plan.lane)
                .map(|group| group.rect.right() + 4.0)
                .fold(inner.left() + 4.0, f32::max);
            let bottom = inner.top() + lane_box.top + lane_box.height - 3.0;
            let rect = Rect::from_min_size(Pos2::new(left, bottom - size.y), size);
            if rect.right() > inner.right() - 4.0 {
                continue;
            }
            chips.push(TransportChip {
                rect,
                addon_id: addon_id.clone(),
                source_id: source_id.clone(),
                chip,
            });
        }
        chips
    }

    /// A press on a lane's compact transport plays or pauses its source.
    fn interact_transport(&mut self, ui: &egui::Ui, rect: Rect) -> Option<String> {
        if self.waveform_only {
            return None;
        }
        let boxes = self.lane_boxes(self.timeline_rect(rect).height());
        let chips = self.transport_chips(ui, rect, &boxes);
        self.lane_controls
            .extend(chips.iter().map(|chip| chip.rect));
        let (hover, press) = ui.input(|input| {
            let press = input.events.iter().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    ..
                } => Some(*pos),
                _ => None,
            });
            (input.pointer.hover_pos(), press)
        });
        if self.gesture.is_none()
            && let Some(chip) =
                press.and_then(|point| chips.iter().find(|chip| chip.rect.contains(point)))
            && let Some(handle) = self.controller.addons().cloned()
        {
            self.addon_sources
                .toggle_play(&handle, &chip.addon_id, &chip.source_id);
        }
        let chip = chips
            .iter()
            .find(|chip| hover.is_some_and(|point| chip.rect.contains(point)))?;
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        Some(Language::current().transport_tip(chip.chip.playing, &chip.chip.source))
    }

    fn paint_transport(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: Rect,
        boxes: &[LaneBox],
    ) {
        let hover = ui.input(|input| input.pointer.hover_pos());
        for chip in self.transport_chips(ui, rect, boxes) {
            addon_sources::paint_chip(
                painter,
                chip.rect,
                &chip.chip,
                hover.is_some_and(|point| chip.rect.contains(point)),
                self.accent,
            );
        }
    }

    /// The scroll bar track of a lane that scrolls, in the plot's right margin.
    fn scroll_track(&self, rect: Rect, lane_box: &LaneBox) -> Rect {
        let inner = self.timeline_rect(rect);
        Rect::from_x_y_ranges(
            inner.right()..=rect.right(),
            inner.top() + lane_box.top..=inner.top() + lane_box.top + lane_box.height,
        )
    }

    /// Group switches, scroll bars and the wheel over a stacked lane's margins. Returns the
    /// tooltip for what is under the pointer.
    fn interact_lanes(&mut self, ui: &egui::Ui, rect: Rect) -> Option<String> {
        self.lane_controls.clear();
        self.lane_scroll_zones.clear();
        if !self.stacked() || self.history_frames.is_empty() {
            self.scroll_drag = None;
            return None;
        }
        let language = Language::current();
        let inner = self.timeline_rect(rect);
        let boxes = self.lane_boxes(inner.height());
        let chips = self.group_chips(ui, rect, &boxes);
        self.lane_controls
            .extend(chips.iter().map(|chip| chip.rect));
        for (plan, lane_box) in self.lanes.iter().zip(&boxes) {
            if lane_box.scrolls() {
                let track = self.scroll_track(rect, lane_box);
                self.lane_controls.push(track);
                self.lane_scroll_zones.push((plan.lane, track));
                self.lane_scroll_zones.push((
                    plan.lane,
                    Rect::from_x_y_ranges(rect.left()..=inner.left(), track.y_range()),
                ));
            }
        }
        let (hover, position, press, down, steps) = ui.input(|input| {
            let mut press = None;
            let mut steps = 0.0f32;
            for event in &input.events {
                match event {
                    egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: true,
                        ..
                    } => press = Some(*pos),
                    egui::Event::MouseWheel { unit, delta, .. } => {
                        steps += match unit {
                            egui::MouseWheelUnit::Point => delta.y / 48.0,
                            egui::MouseWheelUnit::Line => delta.y,
                            egui::MouseWheelUnit::Page => delta.y * 3.0,
                        };
                    }
                    _ => {}
                }
            }
            (
                input.pointer.hover_pos(),
                input.pointer.interact_pos(),
                press,
                input.pointer.button_down(PointerButton::Primary),
                steps,
            )
        });
        let by_lane: Vec<(usize, LaneBox)> = self
            .lanes
            .iter()
            .map(|plan| plan.lane)
            .zip(boxes.iter().copied())
            .collect();
        let lane_box = |lane: usize| {
            by_lane
                .iter()
                .find(|(logical, _)| *logical == lane)
                .map(|(_, lane_box)| *lane_box)
        };

        if self.gesture.is_none()
            && let Some(point) = press
        {
            if let Some(chip) = chips.iter().find(|chip| chip.rect.contains(point)) {
                if !chip.current {
                    let (lane, id) = (chip.lane, chip.id.clone());
                    if let Err(error) = self.show_group(lane, &id) {
                        self.message = error;
                    }
                }
            } else if let Some(&(lane, _)) = self
                .lane_scroll_zones
                .iter()
                .find(|(_, zone)| zone.contains(point) && zone.left() >= inner.right())
                && let Some(lane_box) = lane_box(lane)
            {
                let thumb = lane_box.thumb();
                let (top, bottom) = (inner.top() + thumb.start, inner.top() + thumb.end);
                let grab = if (top..=bottom).contains(&point.y) {
                    point.y - top
                } else {
                    // A press beside the thumb brings it there.
                    (bottom - top) / 2.0
                };
                self.scroll_drag = Some((lane, grab));
            }
        }
        if let Some((lane, grab)) = self.scroll_drag {
            if down
                && let Some(point) = position
                && let Some(lane_box) = lane_box(lane)
            {
                let first = lane_box.first_for_thumb(point.y - grab - inner.top());
                self.scroll_lane_to(lane, first);
            } else if !down {
                self.scroll_drag = None;
            }
        }
        if steps != 0.0
            && let Some(point) = hover
            && let Some(&(lane, _)) = self
                .lane_scroll_zones
                .iter()
                .find(|(_, zone)| zone.contains(point))
            && let Some(lane_box) = lane_box(lane)
        {
            // At least one row for each turn of the wheel; up shows earlier channels.
            let rows = steps.abs().ceil().max(1.0) as usize;
            let first = if steps > 0.0 {
                lane_box.first.saturating_sub(rows)
            } else {
                lane_box.first + rows
            };
            self.scroll_lane_to(lane, first);
        }

        let point = hover?;
        if let Some(chip) = chips.iter().find(|chip| chip.rect.contains(point)) {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            return Some(if chip.current {
                language.group_shown(&chip.label)
            } else {
                let next = self.lanes.iter().find_map(|plan| match &plan.kind {
                    LaneKind::Stack { groups, .. } if plan.lane == chip.lane => groups
                        .iter()
                        .find(|(id, _)| *id == chip.id)
                        .map(|(_, label)| label.clone()),
                    _ => None,
                });
                language.show_group(next.as_deref().unwrap_or(&chip.label))
            });
        }
        if point.x >= inner.left() && point.x < inner.right() || !rect.contains(point) {
            return None;
        }
        let (index, row) = stack::row_at(&boxes, point.y - inner.top())?;
        let plan = self.lanes.get(index)?;
        let mut text = if let RowSource::Pair(_, side) = plan.row(row) {
            language.pair_row(side == PairSide::Left).to_owned()
        } else {
            let (group, row) = plan.group_channel(row)?;
            let label = group
                .labels
                .get(row)
                .map_or_else(|| (row + 1).to_string(), Clone::clone);
            language.channel(row + 1, group.channels.len(), &label)
        };
        if boxes[index].scrolls() {
            text.push('\n');
            text.push_str(language.scroll_hint());
        }
        Some(text)
    }

    fn interact_timeline(&mut self, ui: &egui::Ui, rect: Rect) -> bool {
        let audio = self.audio_rect(rect);
        let (hover, position, pan_press, pan_down, steps) = ui.input(|input| {
            let mut press = None;
            let mut steps = 0.0f32;
            for event in &input.events {
                match event {
                    egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Middle,
                        pressed: true,
                        ..
                    } => press = Some(*pos),
                    egui::Event::MouseWheel { unit, delta, .. } => {
                        steps += match unit {
                            egui::MouseWheelUnit::Point => delta.y / 48.0,
                            egui::MouseWheelUnit::Line => delta.y,
                            egui::MouseWheelUnit::Page => delta.y * 3.0,
                        };
                    }
                    _ => {}
                }
            }
            (
                input.pointer.hover_pos(),
                input.pointer.interact_pos(),
                press,
                input.pointer.button_down(PointerButton::Middle),
                steps,
            )
        });
        if self.gesture.is_some() || audio.width() <= 1.0 {
            return false;
        }
        if let Some(point) = pan_press.filter(|point| rect.contains(*point))
            && self.zoom_frames.is_some()
        {
            self.pan = Some((point.x, self.visible_frames.clone()));
        }
        if let Some((origin, range)) = self.pan.clone() {
            if let Some(point) = position {
                let count = range.end - range.start;
                let offset = ((origin - point.x) / audio.width() * count as f32).round() as i64;
                self.set_visible_frames(range.start.saturating_add_signed(offset), count);
            }
            if !pan_down {
                self.pan = None;
            }
            return true;
        }
        if steps != 0.0
            && let Some(point) = hover.filter(|point| {
                rect.contains(*point)
                    && !self
                        .lane_scroll_zones
                        .iter()
                        .any(|(_, zone)| zone.contains(*point))
            })
        {
            let requested = self.zoom_frames.as_ref().unwrap_or(&self.history_frames);
            let count = requested.end - requested.start;
            let total = self.history_frames.end - self.history_frames.start;
            let minimum = waveform::minimum_zoom_frames(audio.width(), total).min(count);
            let new_count = ((count as f64 * 1.22f64.powf(f64::from(-steps.clamp(-12.0, 12.0))))
                .round() as u64)
                .clamp(minimum, total);
            let fraction_at = |bounds: Rect| {
                f64::from(((point.x - bounds.left()) / bounds.width().max(1.0)).clamp(0.0, 1.0))
            };
            let anchor_rect = if requested != &self.history_frames {
                self.timeline_rect(rect)
            } else {
                audio
            };
            let anchor = requested.start + (count as f64 * fraction_at(anchor_rect)).round() as u64;
            let fraction = fraction_at(self.timeline_rect(rect));
            self.set_visible_frames(
                anchor.saturating_sub((new_count as f64 * fraction).round() as u64),
                new_count,
            );
            return true;
        }
        false
    }

    fn set_visible_frames(&mut self, start: u64, count: u64) {
        let range = fit_frame_range(&self.history_frames, start, count);
        if self.zoom_frames.as_ref().unwrap_or(&self.history_frames) != &range {
            self.zoom_frames = (range != self.history_frames).then_some(range);
            self.live_revision = None;
            self.next_refresh = Instant::now();
            self.last_interaction = Instant::now();
            self.refresh_waveform();
        }
    }

    fn flag_endpoints(&self, audio: Rect, position: Pos2) -> Option<(u64, u64)> {
        let (flags, rows) = match (&self.selection, &self.flags) {
            (Some(selection), _) => (selection, self.selected_rows().unwrap_or_default()),
            (None, Some(flags)) => (&flags.frames, flags.rows.as_slice()),
            (None, None) => return None,
        };
        let area = self.flag_area(audio, rows);
        let (fixed, moved) = flag_handle_at(
            area,
            self.visible_frames.clone(),
            flags,
            position,
            FLAG_REACH,
        )?;
        Some((
            fixed.clamp(self.history_frames.start, self.history_frames.end),
            moved,
        ))
    }

    /// The frame under `x` without snapping, so a fade follows the pointer smoothly.
    fn free_frame_at(&self, audio: Rect, x: f32) -> u64 {
        let visible = &self.visible_frames;
        let length = visible.end.saturating_sub(visible.start);
        let fraction = f64::from((x - audio.left()) / audio.width().max(1.0)).clamp(0.0, 1.0);
        visible.start + ((length as f64 * fraction).round() as u64).min(length)
    }

    fn frame_at(&self, audio: Rect, x: f32, snap: bool) -> u64 {
        let mut flags: Vec<_> = self
            .flags
            .iter()
            .flat_map(|flags| [flags.frames.start, flags.frames.end])
            .collect();
        if let Some(Gesture::Select { anchor, .. }) = self.gesture {
            // The stationary flag is visible; the moving cursor must not attract itself.
            flags.push(anchor);
        } else if let Some(selection) = &self.selection {
            flags.extend([selection.start, selection.end]);
        }
        let available = self
            .editing
            .as_ref()
            .map_or_else(|| self.history_frames.clone(), FrozenEdit::frames);
        frame_from_pointer(
            audio,
            x,
            self.visible_frames.clone(),
            snap,
            &flags,
            &self.visible_guides(audio),
        )
        .clamp(available.start, available.end)
    }

    fn draw_waveform(&self, ui: &egui::Ui, rect: Rect, preview_hover: bool) {
        let theme = &self.theme;
        if self.waveform_border {
            let color = theme.border(
                self.accent,
                self.waveform_border_color,
                self.waveform_border_follows_background,
                self.transparency,
            );
            ui.painter_at(rect).rect_stroke(
                rect,
                if self.waveform_only {
                    0.0
                } else {
                    theme.corner_radius
                },
                Stroke::new(self.border_inset(), color),
                egui::StrokeKind::Inside,
            );
        }
        let rect = rect.shrink(self.border_inset());
        let painter = ui.painter_at(rect);
        if self.waveform_only {
            painter.rect_filled(
                rect,
                0.0,
                theme.translucent_plot_background(self.transparency),
            );
        } else {
            // The panel already supplies the chosen alpha when translucent.
            if self.transparency == 0 {
                painter.rect_filled(rect, theme.corner_radius, theme.plot_background);
            }
        }
        let audio = self.audio_rect(rect);
        if audio.width() <= 0.0 {
            if !self.waveform_only {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    NO_AUDIO_HINT,
                    egui::FontId::proportional(13.0),
                    Color32::GRAY,
                );
            } else if self.capture_problem().is_none() {
                // Dimmed, and left out when a source problem is already written on the plot.
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    NO_AUDIO_HINT,
                    egui::FontId::proportional(12.0),
                    Color32::from_gray(104),
                );
            }
            return;
        }

        let selection = match self.gesture {
            Some(Gesture::Select { anchor, cursor, .. }) => {
                Some(anchor.min(cursor)..anchor.max(cursor))
            }
            _ => self.selection.clone(),
        };
        // A selection being drawn has no fades yet; one being resized keeps its own.
        let fades = match self.gesture {
            Some(Gesture::Select {
                resizing: false, ..
            }) => Fades::NONE,
            _ => self.fades,
        };
        let pointer = ui
            .input(|input| input.pointer.hover_pos())
            .filter(|pos| rect.contains(*pos));
        let dragging = matches!(self.gesture, Some(Gesture::Export { .. }));
        let full_preview = !self.waveform_only
            && selection.is_none()
            && self.selection.is_none()
            && (dragging || preview_hover && pointer.is_some());
        let separate = self.rows_separate();
        let boxes = self.lane_boxes(audio.height());
        // While rows are separate, a drag takes only the row it starts in, and a selection
        // covers only its own rows.
        let preview_rows = if !separate {
            None
        } else if dragging {
            self.drag_row.map(|row| rows_between(&self.lanes, row, row))
        } else {
            pointer.and_then(|pointer| self.rows_at(audio, pointer.y))
        };
        let selected_rows = self.selected_rows();
        if full_preview {
            let area = match &preview_rows {
                Some(rows) => self.rows_rect(audio, rows),
                None => Some(audio),
            };
            if let Some(area) = area {
                painter.rect_filled(area, 2.0, theme.preview_highlight(self.accent));
            }
        }

        let visible = self.visible_frames.clone();
        self.paint_time_lines(&painter, audio);
        let highlight = selection.as_ref().and_then(|range| {
            let start = waveform::flag_fraction(range.start.max(visible.start), visible.clone())?;
            let end = waveform::flag_fraction(range.end.min(visible.end), visible.clone())?;
            Some(start as f32..end as f32)
        });
        let stacked = self.stacked();
        let rows: usize = boxes.iter().map(|lane| lane.shown).sum();
        let dual = self.dual_source();
        // Colouring by frequency: the band colours, once for every row.
        let palette = self.waveform_palette();
        for (plan, lane_box) in self.lanes.iter().zip(&boxes) {
            for index in lane_box.visible() {
                let top = audio.top() + lane_box.row_top(index).unwrap_or_default();
                let row = Rect::from_min_size(
                    Pos2::new(audio.left(), top),
                    Vec2::new(audio.width(), lane_box.row_height),
                );
                let source = plan.row(index);
                if !self.waveform_only {
                    // The pair is set apart from the channel rows below it by a faint backing
                    // and a rule, both fainter than the line between two lanes.
                    if matches!(source, RowSource::Pair(..)) {
                        painter.rect_filled(
                            Rect::from_x_y_ranges(rect.left()..=audio.right(), row.y_range()),
                            0.0,
                            PAIR_BACKING,
                        );
                    }
                    painter.hline(
                        row.x_range(),
                        row.center().y,
                        Stroke::new(1.0, theme.center_line),
                    );
                    if source == RowSource::Pair(plan.lane, PairSide::Right)
                        && index + 1 < lane_box.first + lane_box.shown
                    {
                        painter.hline(
                            rect.left() + 1.0..=audio.right(),
                            row.bottom(),
                            Stroke::new(1.0, PAIR_DIVIDER),
                        );
                    }
                    // Mono and stereo lanes are labelled as before; a stacked lane names
                    // each channel.
                    let label = match (&plan.kind, source) {
                        (LaneKind::Stack { .. }, _) => match plan.group_channel(index) {
                            Some((group, channel)) => Some(stack::short_label(
                                group.labels.get(channel).map_or("", String::as_str),
                            ))
                            .filter(|label| !label.is_empty())
                            .or_else(|| Some((channel + 1).to_string())),
                            None => Some(self.row_name(source)),
                        },
                        (_, RowSource::View(lane, view)) if rows == 2 || stacked => {
                            Some(match view {
                                _ if dual => lane_name(lane).to_owned(),
                                WaveformView::Left => "L".to_owned(),
                                WaveformView::Right => "R".to_owned(),
                                _ => String::new(),
                            })
                        }
                        _ => None,
                    };
                    if let Some(label) = label.filter(|label| {
                        !label.is_empty() && lane_box.row_height >= stack::LABELLED_ROW_HEIGHT
                    }) {
                        painter.text(
                            Pos2::new(rect.left() + 1.0, row.center().y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::monospace(8.0),
                            theme.row_label,
                        );
                    }
                }
                // A selection of some rows is drawn on those rows only.
                let selected = selected_rows.is_none_or(|rows| rows.contains(&source));
                let row_selection = selection.as_ref().filter(|_| selected);
                if let Some(peaks) = self
                    .peak_rows
                    .iter()
                    .position(|row| *row == source)
                    .and_then(|position| self.peaks.get(position))
                {
                    let tapered;
                    let peaks = match row_selection {
                        // The waveform tapers the way the exported clip does.
                        Some(range) if !fades.is_none() => {
                            let mut faded = peaks.clone();
                            fade_waveform_bins(
                                &mut faded.low,
                                &mut faded.high,
                                visible.clone(),
                                range,
                                fades,
                            );
                            tapered = faded;
                            &tapered
                        }
                        _ => peaks,
                    };
                    // A look can colour a channel's row by the channel's label.
                    let channel = plan
                        .group_channel(index)
                        .and_then(|(group, channel)| group.labels.get(channel))
                        .map(String::as_str);
                    let colors = palette
                        .as_ref()
                        .map(|palette| self.row_column_colors(source, peaks.low.len(), palette));
                    self.paint_waveform(
                        &painter,
                        row,
                        peaks,
                        highlight.as_ref().filter(|_| selected),
                        channel,
                        colors.as_deref().map(|colors| ColumnColors {
                            colors,
                            selection_shown: highlight.is_some(),
                        }),
                    );
                }
                if let Some(range) = row_selection {
                    // Full scale is drawn at the height paint_waveform_geometry uses.
                    paint_fades(
                        &painter,
                        row,
                        row.center().y,
                        row.height() * 0.47,
                        &visible,
                        range,
                        fades,
                        theme.flag,
                    );
                }
            }
        }
        if stacked {
            self.paint_lane_furniture(ui, &painter, rect, audio, &boxes);
        }
        if separate && let [_, lane_b] = boxes.as_slice() {
            // Rows are apart: a line runs between lanes A and B for as long as they are, below
            // however many rows lane A stacks.
            painter.hline(
                audio.x_range(),
                audio.top() + lane_b.top,
                Stroke::new(1.0, theme.lane_divider(self.accent)),
            );
        }
        self.paint_transport(ui, &painter, rect, &boxes);
        self.paint_flags(&painter, audio);
        if let Some(player) = &self.preview {
            paint_playhead(&painter, audio, &visible, player.playhead(), theme.playhead);
        }

        let rate = f64::from(self.sample_rate.max(1));
        // The label in full, and the length alone for when the full label would cover a flag.
        let labels = match self.gesture {
            Some(Gesture::Fade {
                end: FadeEnd::In, ..
            }) => Some(vec![format!(
                "Fade in {:.2} s",
                fades.fade_in as f64 / rate
            )]),
            Some(Gesture::Fade {
                end: FadeEnd::Out, ..
            }) => Some(vec![format!(
                "Fade out {:.2} s",
                fades.fade_out as f64 / rate
            )]),
            _ => {
                // A length that covers some rows says which.
                let rows = if selection.is_some() {
                    selected_rows
                } else {
                    preview_rows.as_deref()
                };
                selection
                    .or_else(|| full_preview.then_some(visible))
                    .map(|range| {
                        let seconds = (range.end - range.start) as f64 / rate;
                        let length = format!("{seconds:.2} s");
                        match rows {
                            Some(rows) => {
                                vec![format!("{} · {length}", self.rows_name(rows)), length]
                            }
                            None => vec![length],
                        }
                    })
            }
        };
        if !self.waveform_only
            && let Some(labels) = labels
        {
            let flags: Vec<Rect> = self
                .flag_marks(audio)
                .into_iter()
                .map(|(x, y, height, direction)| {
                    Rect::from_two_pos(Pos2::new(x, y), Pos2::new(x + direction * 6.0, y + height))
                        .expand(1.0)
                })
                .collect();
            let font = egui::FontId::monospace(11.0);
            let galleys: Vec<_> = labels
                .into_iter()
                .map(|label| painter.layout_no_wrap(label, font.clone(), Color32::WHITE))
                .collect();
            let (galley, place) = length_label_place(
                audio,
                &galleys
                    .iter()
                    .map(|galley| galley.size())
                    .collect::<Vec<_>>(),
                &flags,
            );
            painter.galley(place, Arc::clone(&galleys[galley]), Color32::WHITE);
        }
    }

    /// The line between two lanes, the scroll bars and the group switches of stacked lanes.
    fn paint_lane_furniture(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: Rect,
        audio: Rect,
        boxes: &[LaneBox],
    ) {
        let hover = ui.input(|input| input.pointer.hover_pos());
        for lane_box in boxes.iter().skip(1) {
            painter.hline(
                rect.left() + 1.0..=audio.right(),
                audio.top() + lane_box.top,
                Stroke::new(1.0, Color32::from_white_alpha(36)),
            );
        }
        for (plan, lane_box) in self.lanes.iter().zip(boxes) {
            if !lane_box.scrolls() {
                continue;
            }
            let track = self.scroll_track(rect, lane_box);
            let x = track.center().x - 1.5..=track.center().x + 1.5;
            painter.rect_filled(
                Rect::from_x_y_ranges(x.clone(), track.y_range()),
                1.5,
                Color32::from_white_alpha(18),
            );
            let thumb = lane_box.thumb();
            let active = self.scroll_drag.is_some_and(|(lane, _)| lane == plan.lane)
                || hover.is_some_and(|point| track.contains(point));
            painter.rect_filled(
                Rect::from_x_y_ranges(x, audio.top() + thumb.start..=audio.top() + thumb.end),
                1.5,
                self.theme.scroll_thumb(active),
            );
        }
        // The same dark backing as the waveform-only status mark keeps the switch readable.
        let chips = self.group_chips(ui, rect, boxes);
        for lane in [0, 1] {
            let lane_chips: Vec<_> = chips.iter().filter(|chip| chip.lane == lane).collect();
            let (Some(start), Some(end)) = (lane_chips.first(), lane_chips.last()) else {
                continue;
            };
            painter.rect_filled(start.rect.union(end.rect), 3.0, self.theme.chip_background);
            for chip in lane_chips {
                let hovered = hover.is_some_and(|point| chip.rect.contains(point));
                if chip.current {
                    painter.rect_filled(
                        chip.rect.shrink(1.0),
                        2.5,
                        self.theme.chip_current(self.accent),
                    );
                }
                painter.text(
                    chip.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &chip.label,
                    egui::FontId::proportional(10.0),
                    if chip.current || hovered {
                        Color32::WHITE
                    } else {
                        self.theme.chip_text
                    },
                );
            }
        }
    }

    fn visible_guides(&self, audio: Rect) -> waveform::TimelineGuides {
        let bars = self
            .editing
            .as_ref()
            .map_or(self.bar_markers.as_slice(), |edit| edit.bars.as_slice());
        waveform::timeline_guides(
            self.visible_frames.clone(),
            self.sample_rate,
            u32::from(self.second_interval),
            bars,
            self.bar_lines,
            self.second_lines,
            self.transparency < 100,
            &self.display_policy(audio, self.visible_frames.clone()),
        )
    }

    fn paint_time_lines(&self, painter: &egui::Painter, audio: Rect) {
        let guides = self.visible_guides(audio);
        let policy = self.display_policy(audio, self.visible_frames.clone());
        let draw = |frames: &[u64], color, stroke| {
            paint_guide_lines(
                painter,
                audio,
                &self.visible_frames,
                frames,
                Stroke::new(stroke, color),
            );
        };
        draw(
            &guides.seconds,
            self.theme.second_line(self.transparency),
            policy.seconds_stroke,
        );
        draw(
            &guides.bars,
            self.theme.bar_line(self.transparency),
            policy.bar_stroke,
        );
    }

    fn paint_flags(&self, painter: &egui::Painter, audio: Rect) {
        let painter = painter.with_clip_rect(audio);
        for (x, y, height, direction) in self.flag_marks(audio) {
            painter.line_segment(
                [Pos2::new(x, y), Pos2::new(x, y + height)],
                Stroke::new(1.0, self.theme.flag),
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(x, y),
                    Pos2::new(x + direction * 6.0, y + height * 0.28),
                    Pos2::new(x, y + height * 0.56),
                ],
                self.theme.flag,
                Stroke::NONE,
            ));
        }
    }

    /// Where the selection's flags stand in `audio`: the x of each pole, its top, its height
    /// and which way its flag points.
    fn flag_marks(&self, audio: Rect) -> Vec<(f32, f32, f32, f32)> {
        let visible = self.visible_frames.clone();
        let current = match self.gesture {
            Some(Gesture::Select { anchor, cursor, .. }) => {
                Some(anchor.min(cursor)..anchor.max(cursor))
            }
            _ => self.selection.clone(),
        };
        let accepted = self.flags.as_ref().map(|flags| &flags.frames);
        // While rows are separate, flags stand on top of the rows they belong to.
        let accepted_area = self
            .flags
            .as_ref()
            .map_or(audio, |flags| self.flag_area(audio, &flags.rows));
        let current_area = self.flag_area(audio, self.selected_rows().unwrap_or_default());
        let ranges = accepted
            .map(|range| (range, accepted_area))
            .into_iter()
            .chain(
                current
                    .as_ref()
                    .filter(|range| Some(*range) != accepted)
                    .map(|range| (range, current_area)),
            );
        let mut marks = Vec::new();
        for (flags, area) in ranges {
            let flag_height = area.height().min(11.0);
            for (position, direction) in [(flags.start, 1.0), (flags.end, -1.0)] {
                let Some(fraction) = waveform::flag_fraction(position, visible.clone()) else {
                    continue;
                };
                let x = audio.left() + fraction as f32 * audio.width();
                let inset = (audio.width() * 0.25).min(0.25);
                let x = x.clamp(audio.left() + inset, audio.right() - inset);
                marks.push((x, area.top(), flag_height, direction));
            }
        }
        marks
    }

    fn paint_waveform(
        &self,
        painter: &egui::Painter,
        audio: Rect,
        peaks: &WaveformPeaks,
        selection: Option<&Range<f32>>,
        channel: Option<&str>,
        colors: Option<ColumnColors<'_>>,
    ) {
        let policy = self.display_policy(audio, self.visible_frames.clone());
        let geometry =
            waveform::waveform_geometry(&peaks.low, &peaks.high, self.visible_frames.clone());
        paint_colored_row_waveform(
            painter,
            audio,
            &geometry,
            policy.waveform_stroke,
            selection,
            &self.theme,
            self.accent,
            channel,
            colors,
        );
    }
}

/// A row's colours while the waveform is coloured by frequency.
#[derive(Clone, Copy)]
struct ColumnColors<'a> {
    /// One for each bin the row's geometry was made from; `None` draws that column in the
    /// single colour.
    colors: &'a [Option<Color32>],
    /// A selection is on screen, on this row or another: what lies outside it is dimmed.
    selection_shown: bool,
}

/// Paints one row's waveform `geometry` across `audio` as `theme` draws it for this accent:
/// in the colour of the row's `channel` (its label) or the waveform's, and in the selected
/// colour across `selection`, given as fractions of the row's width.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
fn paint_row_waveform(
    painter: &egui::Painter,
    audio: Rect,
    geometry: &waveform::WaveformGeometry,
    stroke: f32,
    selection: Option<&Range<f32>>,
    theme: &Theme,
    accent: Color32,
    channel: Option<&str>,
) {
    paint_colored_row_waveform(
        painter, audio, geometry, stroke, selection, theme, accent, channel, None,
    );
}

/// [`paint_row_waveform`], with each column in its frequency colour when `colors` has one for
/// it. That colour wins over the look's waveform and row colours; inside `selection` it is
/// drawn in full and, while a selection is shown, dimmed everywhere else
/// (`spectrum::unselected_color`). Columns without one take the row's single colour, treated
/// the same way.
#[allow(clippy::too_many_arguments)]
fn paint_colored_row_waveform(
    painter: &egui::Painter,
    audio: Rect,
    geometry: &waveform::WaveformGeometry,
    stroke: f32,
    selection: Option<&Range<f32>>,
    theme: &Theme,
    accent: Color32,
    channel: Option<&str>,
    colors: Option<ColumnColors<'_>>,
) {
    let color = theme.row_waveform(channel, accent);
    let single = color;
    let selected = theme.waveform_selected(accent, SELECTION_CONTRAST);
    let painter = painter.with_clip_rect(audio);
    let outside: fn(Color32) -> Color32 = match colors {
        Some(colors) if colors.selection_shown => spectrum::unselected_color,
        _ => std::convert::identity,
    };
    let paint_range = |range: Range<f32>, color, tint: fn(Color32) -> Color32| {
        if range.is_empty() {
            return;
        }
        let clip = Rect::from_x_y_ranges(
            audio.left() + range.start * audio.width()..=audio.left() + range.end * audio.width(),
            audio.y_range(),
        );
        let painter = painter.with_clip_rect(clip);
        match colors {
            // A column without its colour yet takes the waveform's single colour, dimmed or
            // not like its neighbours, so the selection reads the same across it.
            Some(colors) => paint_colored_waveform_geometry(
                &painter,
                audio,
                geometry,
                stroke,
                color,
                theme,
                |x| {
                    let column = trace_bin(x, colors.colors.len());
                    let own = colors.colors.get(column).copied().flatten();
                    Some(tint(own.unwrap_or(single)))
                },
            ),
            None => paint_waveform_geometry(&painter, audio, geometry, stroke, color, theme),
        }
    };
    if let Some(range) = selection.filter(|range| range.start < range.end) {
        paint_range(0.0..range.start, color, outside);
        paint_range(range.clone(), selected, spectrum::selected_color);
        paint_range(range.end..1.0, color, outside);
    } else {
        paint_range(0.0..1.0, color, outside);
    }
}

/// Where the plot's length label goes so that it covers no flag (`flags`, their rectangles):
/// which of the labels (`sizes`, longest first) and its top-left corner. The full label stays
/// at the top right when it fits; else the shorter ones there, then the shortest at the top
/// left, then the shortest at the top right just below the flags.
fn length_label_place(audio: Rect, sizes: &[Vec2], flags: &[Rect]) -> (usize, Pos2) {
    let clear = |rect: Rect| !flags.iter().any(|flag| flag.intersects(rect));
    let top_right = |size: Vec2| Pos2::new(audio.right() - 8.0 - size.x, audio.top());
    for (index, &size) in sizes.iter().enumerate() {
        if clear(Rect::from_min_size(top_right(size), size)) {
            return (index, top_right(size));
        }
    }
    let shortest = sizes.len().saturating_sub(1);
    let size = sizes.get(shortest).copied().unwrap_or_default();
    let top_left = Pos2::new(audio.left() + 8.0, audio.top());
    if clear(Rect::from_min_size(top_left, size)) {
        return (shortest, top_left);
    }
    let below = flags
        .iter()
        .map(|flag| flag.max.y)
        .fold(audio.top(), f32::max)
        + 1.0;
    let y = below.min(audio.bottom() - size.y).max(audio.top());
    (shortest, Pos2::new(top_right(size).x, y))
}

/// What the Sources page calls `route`. The capture code's name for All channels holds both
/// languages; the window shows its own.
fn route_label(route: ChannelRoute, language: Language) -> &'static str {
    match route {
        ChannelRoute::AllChannels => language.all_channels(),
        route => route.label(),
    }
}

/// Whether the capture status mark is drawn for this state under the user's choice.
fn status_mark_shown(mark: StatusMark, indicator: CaptureIndicator, problem: bool) -> bool {
    match mark {
        StatusMark::Always => true,
        StatusMark::Attention => {
            problem
                || matches!(
                    indicator,
                    CaptureIndicator::Paused | CaptureIndicator::Unavailable
                )
        }
        StatusMark::Never => false,
    }
}

/// Paints the waveform-only status mark in the top-left corner of the plot: a dot coloured by
/// state, or pause bars, with the source problem written beside it when there is one.
/// Returns the area whose tooltip explains the mark.
/// See [`Recorder::window_border_inset`].
fn window_border_inset(waveform_border: bool, width: u8) -> f32 {
    if waveform_border { f32::from(width) } else { 0.0 }
}

/// Grows or shrinks the waveform-only window by the change from `old_inset` to `new_inset` on
/// each side, so its plot keeps its height.
fn fit_window_border(ctx: &egui::Context, waveform_only: bool, old_inset: f32, new_inset: f32) {
    if waveform_only && new_inset != old_inset {
        let size = ctx.viewport_rect().size();
        let height = size.y + (new_inset - old_inset) * 2.0;
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(
            Vec2::new(size.x, height).max(min_window_size(true)),
        ));
    }
}

fn paint_compact_status(
    painter: &egui::Painter,
    rect: Rect,
    indicator: CaptureIndicator,
    lit: bool,
    problem: Option<&str>,
) -> Rect {
    let color = if problem.is_some() {
        ERROR_LED
    } else {
        indicator.compact_color(lit)
    };
    let center = rect.left_top() + COMPACT_STATUS_OFFSET;
    // The backing keeps the mark readable over a loud waveform or a transparent window.
    let backing = Color32::from_black_alpha(168);
    painter.circle_filled(center, 6.5, backing);
    if indicator == CaptureIndicator::Paused && problem.is_none() {
        for x in [-2.0, 2.0] {
            painter.rect_filled(
                Rect::from_center_size(center + Vec2::new(x, 0.0), Vec2::new(2.0, 7.0)),
                0.5,
                color,
            );
        }
    } else {
        painter.circle_filled(center, 3.5, color);
    }

    let mut hover = Rect::from_center_size(center, Vec2::splat(15.0));
    let left = center.x + 11.0;
    let width = rect.right() - left - 8.0;
    if let Some(problem) = problem.filter(|_| width > 0.0) {
        let mut job = egui::text::LayoutJob::simple_singleline(
            problem.to_owned(),
            egui::FontId::proportional(12.0),
            ERROR_LED,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(width);
        let galley = painter.layout_job(job);
        let text = Rect::from_min_size(
            Pos2::new(left, center.y - galley.size().y * 0.5),
            galley.size(),
        );
        painter.rect_filled(text.expand2(Vec2::new(4.0, 1.5)), 3.0, backing);
        painter.galley(text.min, galley, ERROR_LED);
        hover = hover.union(text);
    }
    hover.intersect(rect)
}

fn paint_waveform_geometry(
    painter: &egui::Painter,
    audio: Rect,
    geometry: &waveform::WaveformGeometry,
    stroke: f32,
    color: Color32,
    theme: &Theme,
) {
    let position = |x, value| {
        Pos2::new(
            audio.left() + x * audio.width(),
            audio.center().y - value * audio.height() * 0.47,
        )
    };
    let stroke = theme.waveform_stroke(stroke);
    paint_waveform_trace(
        painter,
        geometry,
        position,
        &TraceStyle {
            stroke: Stroke::new(stroke, color),
            fill: theme.waveform_fill(color),
            lone_sample: LoneSample::Dot(stroke * 0.55),
        },
    );
}

/// [`paint_waveform_geometry`] with the colour `color_at` gives each column and sample, and
/// `color` where it gives none.
fn paint_colored_waveform_geometry(
    painter: &egui::Painter,
    audio: Rect,
    geometry: &waveform::WaveformGeometry,
    stroke: f32,
    color: Color32,
    theme: &Theme,
    color_at: impl Fn(f32) -> Option<Color32>,
) {
    let position = |x, value| {
        Pos2::new(
            audio.left() + x * audio.width(),
            audio.center().y - value * audio.height() * 0.47,
        )
    };
    let stroke = theme.waveform_stroke(stroke);
    paint_colored_waveform_trace(
        painter,
        geometry,
        position,
        &TraceStyle {
            stroke: Stroke::new(stroke, color),
            fill: theme.waveform_fill(color),
            lone_sample: LoneSample::Dot(stroke * 0.55),
        },
        color_at,
        |color| theme.waveform_fill(color),
    );
}

impl eframe::App for Recorder {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if input.viewport_id != egui::ViewportId::ROOT {
            return;
        }
        if let Some(harness) = &mut self.harness {
            harness.drop_on_main(input);
        }
        let viewport = input.viewport();
        if viewport.minimized == Some(true) || viewport.fullscreen == Some(true) || !input.focused {
            self.window_drag = None;
            self.pan = None;
            return;
        }
        let can_resize = viewport.maximized != Some(true);
        let menu_open = egui::Popup::is_any_open(ctx);
        let selection_active =
            self.selection.is_some() || matches!(self.gesture, Some(Gesture::Select { .. }));
        if !menu_open && (selection_active || self.gesture.is_some()) {
            let cancel_key = |event: &egui::Event| is_cancel_key(event, &self.hotkeys);
            if input.events.iter().any(cancel_key) {
                input.events.retain(|event| !cancel_key(event));
                self.cancel_selection();
                return;
            }
        }
        if input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::PointerButton { pressed: true, .. }))
        {
            self.last_interaction = Instant::now();
        }
        if self.gesture.is_some() || menu_open {
            return;
        }
        let bounds = input.screen_rect.unwrap_or_else(|| ctx.viewport_rect());
        let audio = self.audio_rect(self.plot_rect);
        for event in &mut input.events {
            let egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            } = *event
            else {
                continue;
            };
            if pressed {
                // Alt on a flag of the selection drags its fade instead of the window.
                let fading = modifiers.alt && self.fade_flag(audio, pos).is_some();
                self.window_drag = window_interaction(bounds, pos, modifiers.alt, can_resize)
                    .filter(|_| !fading)
                    .map(|interaction| WindowDrag {
                        interaction,
                        press_position: pos,
                        origin: None,
                        released: false,
                    });
            }
            if let Some(drag) = &mut self.window_drag {
                drag.released = !pressed;
                // Keep the position but prevent the reserved press from reaching any widget.
                *event = egui::Event::PointerMoved(pos);
            }
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.updates.poll() {
            self.quit(ctx);
            return;
        }
        self.update_tray(ctx, frame);
        self.poll_snapshot_hotkey();
        self.collect_source_list();
        self.addons_page
            .poll(self.controller.addons(), Language::current());
        self.refresh_look();
        let in_use = |id: &str| {
            self.controller.sources().iter().flatten().any(|settings| {
                matches!(&settings.source, CaptureSource::Addon { addon_id, .. } if addon_id == id)
            })
        };
        if let Some(message) =
            self.addon_sources
                .poll(self.controller.addons(), Language::current(), in_use)
        {
            self.message = message;
        }
        let now = Instant::now();
        self.watch_pin(frame, now);
        if let Some(drag) = &mut self.window_drag
            && frame
                .winit_window()
                .and_then(|window| {
                    drag.update(
                        window,
                        ctx.pixels_per_point(),
                        min_window_size(self.waveform_only),
                    )
                })
                .is_none()
        {
            self.window_drag = None;
        }
        if let Some(window) = frame.winit_window() {
            self.observe_window_position(window, now);
        }
        if !ctx.input(|input| input.pointer.any_down()) {
            self.save_accent();
            self.save_waveform_gradient();
        }
        self.controller_busy = !self.controller.poll();
        self.sync_generation();
        self.collect_editing();
        self.collect_preview();
        if let Some(error) = self
            .preference_writer
            .as_ref()
            .and_then(PreferenceWriter::poll_error)
        {
            self.message = format!("Could not save settings: {error}");
        }
        if !self.controller.is_recording() {
            self.recording_started = None;
        }
        self.collect_export();
        self.collect_snapshot_saves();
        self.clean_output_folder();
        if self
            .auto_resume_remaining()
            .is_some_and(|remaining| remaining.is_zero())
        {
            self.resume();
        }
        // The window is not always drawn after this, so ask for the next pass here too.
        self.schedule_wake(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let panel = egui::Frame::central_panel(ui.style());
        let frame_style = if self.waveform_only {
            panel.fill(Color32::TRANSPARENT).inner_margin(0)
        } else {
            panel.fill(
                self.theme
                    .panel_background(ui.visuals().panel_fill, self.transparency),
            )
        };
        // Menu and keyboard commands wait until the frame is laid out, like the Options window's
        // changes: some of them resize the window or switch its mode.
        let commands_allowed = self.window_drag.is_none() && self.gesture.is_none();
        let open_menu = commands_allowed
            && ui.input(|input| input.pointer.button_clicked(PointerButton::Secondary));
        // A focused widget (a text field, or a button reached with Tab) owns the keyboard, and
        // keys pressed in the Options window never reach this viewport's input at all.
        let shortcut = (commands_allowed
            && !egui::Popup::is_any_open(ui.ctx())
            && ui.memory(|memory| memory.focused().is_none()))
        .then(|| ui.input(|input| window_shortcut(&input.events, &self.hotkeys)))
        .flatten();
        if self.lane_key.is_none() {
            self.lane_key = frame
                .winit_window()
                .and_then(|window| lane_key::LaneKey::attach(window));
        }
        // A tap during a gesture is dropped rather than kept for later. It runs only what the
        // Hotkeys page gave it, which by default is "Edit separately".
        let tapped = self
            .lane_key
            .as_ref()
            .is_some_and(lane_key::LaneKey::take_tap);
        let tapped = self
            .harness
            .as_mut()
            .is_some_and(harness::Harness::take_tap)
            || tapped;
        let lane_tap = tapped
            .then(|| tap_command(&self.hotkeys))
            .flatten()
            .filter(|_| commands_allowed);
        egui::CentralPanel::default()
            .frame(frame_style)
            .show(ui, |ui| {
                let window_drag = self.window_drag.as_ref().map(|drag| drag.interaction);
                let interaction_blocked = window_drag.is_some();
                if !self.waveform_only {
                    self.controls(ui, interaction_blocked);
                }
                // Alt over a flag of the selection offers its fade, not a window move.
                let fade_hover = !interaction_blocked
                    && self.gesture.is_none()
                    && ui.input(|input| {
                        input.modifiers.alt
                            && input.pointer.hover_pos().is_some_and(|position| {
                                self.fade_flag(self.audio_rect(self.plot_rect), position)
                                    .is_some()
                            })
                    });
                let window_cursor = if let Some(interaction) = window_drag {
                    Some(interaction.cursor())
                } else if !interaction_blocked && self.gesture.is_none() && !fade_hover {
                    ui.input(|input| {
                        if input.viewport().fullscreen == Some(true) {
                            return None;
                        }
                        window_interaction(
                            input.viewport_rect(),
                            input.pointer.hover_pos()?,
                            input.modifiers.alt,
                            input.viewport().maximized != Some(true),
                        )
                        .map(WindowInteraction::cursor)
                    })
                } else {
                    None
                };
                let height = if self.waveform_only {
                    ui.available_height()
                } else {
                    (ui.available_height() - 26.0).max(46.0)
                };
                let size = Vec2::new(ui.available_width(), height);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                let plot = rect.shrink(self.border_inset());
                if self.plot_rect.size() != plot.size() {
                    self.next_refresh = Instant::now();
                }
                self.plot_rect = plot;
                self.update_activity();
                self.update_lanes();
                self.fit_window_to_rows(ui.ctx(), frame);
                self.refresh_waveform();
                self.refresh_waveform_colors(ui.ctx());
                if !self.waveform_only {
                    self.full_waveform_height = rect.height() - 2.0 * WAVEFORM_INSET;
                    self.full_window_size = ui.ctx().viewport_rect().size();
                }
                let mut lane_tip = None;
                if !interaction_blocked {
                    lane_tip = self.interact_lanes(ui, rect.shrink(self.border_inset()));
                    let transport_tip =
                        self.interact_transport(ui, rect.shrink(self.border_inset()));
                    lane_tip = transport_tip.or(lane_tip);
                    self.interact_waveform(ui, frame, rect.shrink(self.border_inset()));
                }
                self.draw_waveform(ui, rect, !interaction_blocked && window_cursor.is_none());
                if self.waveform_only {
                    self.draw_compact_status(ui, rect);
                } else {
                    let status = self.status_text();
                    if !status.is_empty() {
                        ui.add(egui::Label::new(&status).truncate())
                            .on_hover_text(&status);
                    }
                }
                let response = match lane_tip {
                    Some(tip) if window_cursor.is_none() && self.gesture.is_none() => {
                        response.on_hover_text_at_pointer(tip)
                    }
                    _ => response,
                };
                if let Some(cursor) = window_cursor {
                    ui.ctx().set_cursor_icon(cursor);
                } else if fade_hover || matches!(self.gesture, Some(Gesture::Fade { .. })) {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                } else if !interaction_blocked
                    && self.lane_controls.iter().all(|control| {
                        !ui.input(|input| input.pointer.hover_pos())
                            .is_some_and(|point| control.contains(point))
                    })
                {
                    response.on_hover_cursor(CursorIcon::Grab);
                }
            });
        if self.gesture.is_some() {
            // A press in this same frame started a waveform gesture, and that comes first.
        } else if open_menu {
            self.show_context_menu(ui.ctx(), frame);
        } else if let Some(command) = lane_tap.or(shortcut) {
            // The clear key with a selection on screen drops the selection, as the cancel keys
            // do, and the pause key plays it or stops it.
            match key_effect(command, self.selection.is_some()) {
                KeyEffect::Run(command) => self.run_command(command, ui.ctx(), frame),
                KeyEffect::TogglePreview => self.toggle_preview(ui.ctx()),
                KeyEffect::CancelSelection => self.cancel_selection(),
            }
        }
        // A package dropped on this window opens its confirmation in Options. Other files are
        // left alone, and a clip dragged out never drops here (`file_drag` cancels a drop on
        // the app's own windows).
        let dropped = dropped_paths(ui.ctx());
        if let Some(package) = addons_page::dropped_package(&dropped) {
            self.review_addon(package);
            if self.options_open {
                self.options_focus_requested = true;
            } else {
                self.open_options(ui.ctx());
            }
        } else {
            self.route_dropped_files(ui.ctx(), &dropped, true);
        }
        addons_page::paint_drop_hint(ui.ctx(), self.accent, Language::current());
        self.draw_options_window(ui.ctx(), frame);
        // A row of the Hotkeys page records only while this pass drew the page: not on another
        // page, nor with the Options window closed or hidden.
        self.hotkeys_page
            .end_pass(hotkeys_page::global(self.hotkey.as_ref()));
        // Drawing can start a gesture, an export or the recording blink.
        self.watch_pin(frame, Instant::now());
        self.schedule_wake(ui.ctx(), frame);
        self.drive_harness(ui.ctx(), frame);
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.save_accent();
        self.save_waveform_gradient();
        self.save_window_position();
        if let Some(request) = self.edit_request.take()
            && !request.saves.is_empty()
        {
            self.pending_edit_save_reads.push(request);
        }
        // An explicitly requested save survives closing the UI before its read reply arrives.
        // Waiting is confined to shutdown; frame/update paths only use try_recv.
        for read in self.pending_edit_save_reads.drain(..) {
            if let Ok(Some(snapshot)) = read.result.recv()
                && edit_snapshot_matches(&snapshot, read.generation)
            {
                for spec in read.saves {
                    if let Some(clip) = spec.frozen_clip(&snapshot) {
                        self.pending_snapshot_saves.push_back(clip);
                    }
                }
            }
        }
        for read in self.pending_snapshot_reads.drain(..) {
            if let Ok(Some(snapshot)) = read.result.recv() {
                for spec in read.saves {
                    if let Some(clip) = spec.frozen_clip(&snapshot) {
                        self.pending_snapshot_saves.push_back(clip);
                    }
                }
            }
        }
        if let Some(job) = self.export.take() {
            let _ = job.worker.join();
        }
        if let Some(worker) = self.snapshot_save.take() {
            let _ = worker.join();
        }
        for clip in self.pending_snapshot_saves.drain(..) {
            let _ = clip.write(true);
        }
        if let Some(writer) = &self.preference_writer
            && let Err(error) = writer.flush()
        {
            eprintln!("Could not save settings on exit: {error}");
        }
    }
}

/// Shown when the frozen audio for a selection cannot be read or played.
const SELECTION_UNAVAILABLE: &str = "The selected audio is unavailable.";

/// One lane that Space plays.
#[derive(Clone, Debug, PartialEq)]
struct PreviewPart {
    /// The lane's frozen snapshot, by its index.
    index: usize,
    route: ChannelRoute,
    /// The group heard of a lane wider than stereo; `None` for a mono or stereo lane.
    group: Option<PairGroup>,
    /// The lane-native channels of `group`; empty for a mono or stereo lane.
    channels: Range<usize>,
    /// The lane-native channels heard; `None` for the whole group, or the whole narrow lane.
    selected: Option<Vec<usize>>,
    /// Whether the lane's frozen positions go with it, so objects move as they do on export.
    positions: bool,
}

/// What Space plays of a frozen selection, lane by lane in snapshot order. `rows` are the rows
/// chosen while rows are separate; `None` plays every lane whole. A mono or stereo lane plays
/// as it always did, or one side of a split stereo lane alone. A wide lane plays the pair
/// render of its rows: the whole Channels group for the pair, or for the whole lane while the
/// Channels group is on show; the chosen channels of one group, with positions when they are
/// objects; the whole group on show of a lane chosen whole. `groups` and `shown` are frozen
/// with the audio, by logical lane.
fn preview_plan(
    snapshots: &[FrozenTrack],
    lanes: &[usize],
    groups: &[Vec<LaneGroup>; 2],
    shown: &[Option<String>; 2],
    routes: [ChannelRoute; 2],
    rows: Option<&[RowSource]>,
) -> Result<Vec<PreviewPart>, &'static str> {
    if lanes.len() != snapshots.len() || lanes.iter().any(|&lane| lane > 1) {
        return Err(SELECTION_UNAVAILABLE);
    }
    let routes = physical_routes(routes, lanes);
    let chosen = rows.and_then(chosen_rows);
    let mut parts = Vec::new();
    for (index, (track, &lane)) in snapshots.iter().zip(lanes).enumerate() {
        let lane_rows: Option<Vec<RowSource>> = rows.map(|rows| {
            rows.iter()
                .copied()
                .filter(|row| row.lane() == lane)
                .collect()
        });
        if lane_rows.as_ref().is_some_and(Vec::is_empty) {
            continue;
        }
        let channels = track.channels();
        let native = chosen
            .as_ref()
            .and_then(|chosen| chosen.native[lane].clone());
        let pair = chosen.as_ref().is_some_and(|chosen| chosen.pair[lane]);
        let route = routes[index];
        if channels <= 2 {
            // Both sides, or a merged row, play the lane as it always played.
            let selected = native.filter(|native| native.len() == 1 && channels == 2);
            parts.push(PreviewPart {
                index,
                route,
                group: None,
                channels: 0..0,
                selected,
                positions: false,
            });
            continue;
        }
        let lane_groups = &groups[lane];
        let whole = |group: LaneGroup| -> Result<PreviewPart, &'static str> {
            Ok(PreviewPart {
                index,
                route,
                group: Some(
                    PairGroup::new(group.channels.start, &group.labels)
                        .ok_or(SELECTION_UNAVAILABLE)?,
                ),
                positions: is_objects(&group),
                channels: group.channels,
                selected: None,
            })
        };
        let part = match native {
            // The lane chosen whole plays the group on show; the pair plays its Channels group.
            None if !pair => whole(group_on_show(lane_groups, shown[lane].as_deref(), channels).0)?,
            Some(native) if !pair && !native.is_empty() => {
                let group = lane_groups
                    .iter()
                    .filter(|group| group.channels.end <= channels)
                    .find(|group| {
                        native
                            .iter()
                            .all(|channel| group.channels.contains(channel))
                    })
                    .cloned()
                    .or_else(|| lane_groups.is_empty().then(|| numbered_group(channels)));
                match group {
                    Some(group) => PreviewPart {
                        selected: Some(native),
                        ..whole(group)?
                    },
                    // Rows of two groups at once play the pair.
                    None => whole(channels_group(lane_groups, channels))?,
                }
            }
            _ => whole(channels_group(lane_groups, channels))?,
        };
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(SELECTION_UNAVAILABLE);
    }
    Ok(parts)
}

/// Whether `positions` move any of the channels `part` plays within `frames`, which makes its
/// render binaural whatever the pair preference is.
fn moves_in(part: &PreviewPart, positions: &FrozenPositionTrack, frames: &Range<u64>) -> bool {
    if !part.positions {
        return false;
    }
    let heard: Vec<usize> = match &part.selected {
        Some(selected) => selected.clone(),
        None => part.channels.clone().collect(),
    };
    heard
        .into_iter()
        .any(|channel| !positions.render_keys(channel, frames.clone()).is_empty())
}

/// What `source` is called in file names: the name the user gave it, or its own name when
/// they gave none or one that leaves nothing a file name can hold.
fn source_file_name<'a>(names: &'a SourceNames, source: &'a CaptureSource) -> &'a str {
    names
        .get(source)
        .filter(|name| file_name_source(name).is_some())
        .unwrap_or_else(|| source.default_file_name())
}

fn lane_name(lane: usize) -> &'static str {
    if lane == 0 { "A" } else { "B" }
}

/// The files dropped on the window being drawn, in this pass.
fn dropped_paths(ctx: &egui::Context) -> Vec<PathBuf> {
    ctx.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .map(|file| egui::DroppedFile::path(file.as_ref()).to_path_buf())
            .collect()
    })
}

fn source_picker_label<'a>(
    source: &'a CaptureSource,
    choices: &[CaptureSource],
) -> std::borrow::Cow<'a, str> {
    if let CaptureSource::Sender { id, name } = source
        && choices
            .iter()
            .filter(|choice| choice.label() == name)
            .count()
            > 1
    {
        let suffix = id.get(id.len().saturating_sub(6)..).unwrap_or(id);
        format!("{name} ({suffix})").into()
    } else {
        source.label().into()
    }
}

fn window_interaction(
    bounds: Rect,
    pos: Pos2,
    alt: bool,
    can_resize: bool,
) -> Option<WindowInteraction> {
    if !bounds.contains(pos) {
        return None;
    }
    if alt {
        return Some(WindowInteraction::Move);
    }
    if !can_resize {
        return None;
    }

    let left = pos.x - bounds.left();
    let right = bounds.right() - pos.x;
    let top = pos.y - bounds.top();
    let bottom = bounds.bottom() - pos.y;
    let direction = match () {
        _ if left.min(top) <= RESIZE_EDGE && left.max(top) <= RESIZE_CORNER => {
            ResizeDirection::NorthWest
        }
        _ if right.min(top) <= RESIZE_EDGE && right.max(top) <= RESIZE_CORNER => {
            ResizeDirection::NorthEast
        }
        _ if left.min(bottom) <= RESIZE_EDGE && left.max(bottom) <= RESIZE_CORNER => {
            ResizeDirection::SouthWest
        }
        _ if right.min(bottom) <= RESIZE_EDGE && right.max(bottom) <= RESIZE_CORNER => {
            ResizeDirection::SouthEast
        }
        _ if left <= RESIZE_EDGE => ResizeDirection::West,
        _ if right <= RESIZE_EDGE => ResizeDirection::East,
        _ if top <= RESIZE_EDGE => ResizeDirection::North,
        _ if bottom <= RESIZE_EDGE => ResizeDirection::South,
        _ => return None,
    };
    Some(WindowInteraction::Resize(direction))
}

fn drag_bounds(
    interaction: WindowInteraction,
    mut bounds: RECT,
    start: POINT,
    cursor: POINT,
    min_width: i32,
    min_height: i32,
) -> RECT {
    use ResizeDirection::{East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West};

    let dx = cursor.x - start.x;
    let dy = cursor.y - start.y;
    let WindowInteraction::Resize(direction) = interaction else {
        bounds.left += dx;
        bounds.right += dx;
        bounds.top += dy;
        bounds.bottom += dy;
        return bounds;
    };

    if matches!(direction, West | NorthWest | SouthWest) {
        bounds.left = (bounds.left + dx).min(bounds.right - min_width);
    } else if matches!(direction, East | NorthEast | SouthEast) {
        bounds.right = (bounds.right + dx).max(bounds.left + min_width);
    }
    if matches!(direction, North | NorthWest | NorthEast) {
        bounds.top = (bounds.top + dy).min(bounds.bottom - min_height);
    } else if matches!(direction, South | SouthWest | SouthEast) {
        bounds.bottom = (bounds.bottom + dy).max(bounds.top + min_height);
    }
    bounds
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::hotkeys::{BindingError, ReservedShortcut};

    #[test]
    fn every_row_maps_a_point_to_the_same_time() {
        // One selection crosses all rows: the time comes from x alone, whichever row is hit.
        let audio = eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::new(36.0, 40.0),
            eframe::egui::Vec2::new(566.0, 150.0),
        );
        let specs = [
            stack::LaneRows {
                rows: 12,
                stacked: true,
            },
            stack::LaneRows {
                rows: 1,
                stacked: false,
            },
        ];
        let boxes = stack::lay_out(audio.height(), &specs, &[0, 0]);
        let guides = backtracking_core::waveform::TimelineGuides::default();
        let mut times = Vec::new();
        for lane in &boxes {
            for row in lane.visible() {
                let y = audio.top() + lane.row_top(row).unwrap() + lane.row_height / 2.0;
                assert!(stack::row_at(&boxes, y - audio.top()).is_some());
                times.push(frame_from_pointer(
                    audio,
                    300.0,
                    0..480_000,
                    false,
                    &[],
                    &guides,
                ));
            }
        }
        assert_eq!(times.len(), 10);
        assert!(times.iter().all(|&time| time == times[0]));
        assert_eq!(
            times[0],
            ((300.0 - 36.0) / 566.0 * 480_000.0f64).round() as u64
        );
    }

    fn painted_waveform(
        minimum: &[f32],
        maximum: &[f32],
        visible: Range<u64>,
        audio: Rect,
    ) -> Vec<egui::epaint::ClippedShape> {
        let context = egui::Context::default();
        let geometry = waveform::waveform_geometry(minimum, maximum, visible);
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0))),
                ..Default::default()
            },
            |ui| {
                paint_waveform_geometry(
                    &ui.painter().with_clip_rect(audio),
                    audio,
                    &geometry,
                    1.5,
                    Color32::GREEN,
                    &Theme::default(),
                );
            },
        );
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        shapes
    }

    /// The colours and widths of what one row's waveform paints: (outline colour, width) of
    /// each path and the colours of each fill.
    fn painted_row(theme: &Theme, channel: Option<&str>) -> (Vec<(Color32, f32)>, Vec<Color32>) {
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
        let mut lines = Vec::new();
        let mut fills = Vec::new();
        for shape in shapes {
            match shape.shape {
                egui::Shape::Path(path) => match path.stroke.color {
                    egui::epaint::ColorMode::Solid(color) => lines.push((color, path.stroke.width)),
                    other => panic!("unexpected stroke {other:?}"),
                },
                egui::Shape::Mesh(mesh) => {
                    fills.extend(mesh.vertices.iter().map(|vertex| vertex.color));
                }
                other => panic!("unexpected shape {other:?}"),
            }
        }
        fills.dedup();
        (lines, fills)
    }

    const ACCENT: Color32 = Color32::from_rgb(103, 231, 181);

    #[test]
    fn the_default_look_paints_the_waveform_as_it_always_has() {
        let selected = backtracking_ui::selected_waveform_color(ACCENT, 80);
        let (lines, fills) = painted_row(&Theme::default(), Some("LFE"));
        // Before, inside and after the selection: two outlines each.
        assert_eq!(
            lines,
            [ACCENT, ACCENT, selected, selected, ACCENT, ACCENT].map(|color| (color, 1.5))
        );
        assert_eq!(
            fills,
            [ACCENT, selected, ACCENT].map(|color| color.linear_multiply(0.4))
        );
    }

    #[test]
    fn a_look_colours_the_waveform_its_selection_and_a_channels_row() {
        let waveform = Color32::from_rgb(10, 20, 200);
        let selected = Color32::from_rgb(240, 230, 10);
        let lfe = Color32::from_rgb(200, 40, 40);
        let mut theme = Theme {
            waveform: Some(waveform),
            waveform_selected: Some(selected),
            waveform_stroke_scale: 2.0,
            waveform_fill_opacity: 0.9,
            ..Theme::default()
        };
        let (lines, fills) = painted_row(&theme, None);
        assert_eq!(
            lines,
            [waveform, waveform, selected, selected, waveform, waveform].map(|color| (color, 3.0))
        );
        assert_eq!(
            fills,
            [waveform, selected, waveform].map(|color| color.linear_multiply(0.9))
        );

        // A row whose channel the look colours takes that colour outside the selection.
        theme.row_colors.insert("LFE".into(), lfe);
        let (lines, _) = painted_row(&theme, Some("LFE"));
        assert_eq!(
            lines.iter().map(|line| line.0).collect::<Vec<_>>(),
            [lfe, lfe, selected, selected, lfe, lfe]
        );
        let (lines, _) = painted_row(&theme, Some("C"));
        assert_eq!(
            lines[0].0, waveform,
            "other channels keep the waveform's colour"
        );
    }

    #[test]
    fn sample_waveform_connects_signed_samples_without_stretching_the_last_frame() {
        let audio = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(120.0, 100.0));
        let shapes = painted_waveform(&[0.75, -0.5, 0.25], &[0.75, -0.5, 0.25], 100..103, audio);
        let paths: Vec<_> = shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Path(path) => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(
            paths.len(),
            1,
            "Sample zoom must connect the actual signed sample sequence"
        );
        let points = &paths[0].points;
        assert_eq!(points.len(), 3);
        for (point, (x, sample)) in points
            .iter()
            .zip([(10.0, 0.75), (50.0, -0.5), (90.0, 0.25)])
        {
            assert!((point.x - x).abs() < 0.001);
            assert!((point.y - (audio.center().y - sample * audio.height() * 0.47)).abs() < 0.001);
        }
        assert!(
            points[2].x < audio.right(),
            "The exclusive frame boundary has no invented sample"
        );
        assert!(
            !shapes
                .iter()
                .any(|shape| matches!(&shape.shape, egui::Shape::Mesh(_)))
        );
    }

    #[test]
    fn overview_envelope_fills_between_columns_without_comb_gaps() {
        let audio = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(120.0, 100.0));
        let shapes = painted_waveform(&[-0.6; 3], &[0.8; 3], 100..400, audio);
        let meshes: Vec<_> = shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh),
                _ => None,
            })
            .collect();
        assert_eq!(meshes.len(), 1);
        let mesh = meshes[0];
        let inside = |point: Pos2, triangle: &[u32; 3]| {
            let [a, b, c] =
                std::array::from_fn(|index| mesh.vertices[triangle[index] as usize].pos);
            let cross = |a: Vec2, b: Vec2| a.x * b.y - a.y * b.x;
            let sides = [
                cross(b - a, point - a),
                cross(c - b, point - b),
                cross(a - c, point - c),
            ];
            sides.iter().all(|side| *side >= -0.001) || sides.iter().all(|side| *side <= 0.001)
        };
        for pixel in 0..120 {
            let point = Pos2::new(audio.left() + pixel as f32 + 0.5, audio.center().y);
            assert!(
                mesh.indices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .any(|triangle| inside(point, triangle)),
                "An overview must retain a continuous filled envelope at x={}",
                point.x
            );
        }
        let outlines: Vec<_> = shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Path(path) => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(outlines.len(), 2);
        assert!(
            outlines
                .iter()
                .all(|path| path.points.first().unwrap().x == audio.left()
                    && path.points.last().unwrap().x == audio.right())
        );
    }

    #[test]
    fn editing_snapshot_remains_the_source_after_live_audio_has_rolled_out() {
        let retained: Arc<[FrozenTrack]> =
            vec![FrozenTrack::from_interleaved(48_000, 1, 100, &[0.1, 0.2, 0.3, 0.4]).unwrap()]
                .into();
        let newer: Arc<[FrozenTrack]> =
            vec![FrozenTrack::from_interleaved(48_000, 1, 500, &[0.9; 4]).unwrap()].into();
        let source = snapshot_source(Some(&retained), false, || Some(newer)).unwrap();
        assert!(Arc::ptr_eq(&source, &retained));
        let clip = WavClip::new(
            source,
            &[0],
            Some(&(101..103)),
            Fades::NONE,
            PathBuf::new(),
            ClipFormat::unnamed([ChannelRoute::Merged; 2], false),
            None,
        )
        .unwrap();
        assert_eq!(clip.frames, 1..3);
        assert_eq!(
            clip.snapshots[0].snapshot().frames(clip.frames.clone()),
            Some(&[0.2, 0.3][..])
        );
    }

    #[test]
    fn pending_edit_never_exports_a_different_live_snapshot() {
        let mut live_was_read = false;
        let source = snapshot_source(None, true, || {
            live_was_read = true;
            None
        });
        assert!(source.is_none());
        assert!(
            !live_was_read,
            "An unfinished edit must wait for its own retained material"
        );
    }

    #[test]
    fn capture_replacement_is_rejected_even_when_its_absolute_frames_overlap() {
        let make_reply = |generation, second_first| EditSnapshot {
            generation,
            bar_markers: vec![101],
            lanes: vec![0, 1],
            groups: std::array::from_fn(|_| Vec::new()),
            shown_groups: [None, None],
            positions: std::array::from_fn(|_| backtracking_core::PositionTrack::new().freeze()),
            snapshots: vec![
                FrozenTrack::from_interleaved(48000, 1, 100, &[0.1; 4]).unwrap(),
                FrozenTrack::from_interleaved(48000, 1, second_first, &[-0.1; 4]).unwrap(),
            ]
            .into(),
        };
        assert!(
            !edit_snapshot_matches(&make_reply(2, 100), 1),
            "A reconnected source must not replace the material being selected"
        );
        assert!(
            !edit_snapshot_matches(&make_reply(1, 101), 1),
            "A/B editing material must have exactly the same frame boundaries"
        );
        let accepted = make_reply(1, 100);
        assert!(edit_snapshot_matches(&accepted, 1));
        assert_eq!(accepted.snapshots[0].frame_range(), 100..104);
        assert_eq!(accepted.bar_markers, [101]);
    }

    #[test]
    fn diagnostic_window_size_accepts_only_finite_reasonable_dimensions() {
        assert_eq!(
            initial_window_size(false, Some("1500x500")),
            Vec2::new(1500.0, 500.0)
        );
        assert_eq!(
            initial_window_size(true, Some("1500x500")),
            Vec2::new(1500.0, 500.0)
        );
        for input in [
            "NaNx500",
            "1500xinf",
            "500x500",
            "1500x100",
            "9000x500",
            "1500x9000",
            "1500x500x1",
            "invalid",
        ] {
            assert_eq!(
                initial_window_size(false, Some(input)),
                Vec2::new(610.0, 144.0),
                "{input}"
            );
        }
        assert_eq!(initial_window_size(false, None), Vec2::new(610.0, 144.0));
        assert_eq!(
            initial_window_size(true, None),
            Vec2::new(610.0, DEFAULT_WAVEFORM_HEIGHT)
        );
    }

    #[test]
    fn auto_resume_choices_are_labelled_by_what_they_do() {
        let labels = AutoResumeDelay::ALL.map(auto_resume_label);
        assert_eq!(labels, ["Off", "5 seconds", "10 seconds"]);
    }

    fn milliseconds(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn an_idle_window_only_runs_its_safety_tick() {
        assert_eq!(next_wake(&Waits::default()), SAFETY_TICK);
        // Hidden or not, with nothing pending there is nothing sooner to do.
        assert_eq!(
            next_wake(&Waits {
                hidden: true,
                ..Waits::default()
            }),
            SAFETY_TICK
        );
    }

    #[test]
    fn work_in_progress_keeps_the_frame_cadence_it_had() {
        assert_eq!(
            next_wake(&Waits {
                busy: true,
                ..Waits::default()
            }),
            BUSY_REFRESH
        );
        assert_eq!(
            next_wake(&Waits {
                polling: true,
                ..Waits::default()
            }),
            REFRESH
        );
        assert_eq!(
            next_wake(&Waits {
                busy: true,
                polling: true,
                hidden: true,
                ..Waits::default()
            }),
            BUSY_REFRESH,
        );
    }

    #[test]
    fn deadlines_wake_the_window_when_they_are_due_and_never_spin_it() {
        let position = Waits {
            position_save: Some(milliseconds(120)),
            ..Waits::default()
        };
        assert_eq!(next_wake(&position), milliseconds(120));
        let overdue = Waits {
            position_save: Some(Duration::ZERO),
            ..Waits::default()
        };
        assert_eq!(next_wake(&overdue), BUSY_REFRESH);
        // A tray that cannot be added is retried, but not in a tight loop.
        let tray = Waits {
            tray_retry: Some(Duration::ZERO),
            ..Waits::default()
        };
        assert_eq!(next_wake(&tray), RETRY_FLOOR);
        let tray = Waits {
            tray_retry: Some(milliseconds(1_400)),
            ..Waits::default()
        };
        assert_eq!(next_wake(&tray), milliseconds(1_400));
        let scan = Waits {
            options_scan: Some(milliseconds(900)),
            ..Waits::default()
        };
        assert_eq!(next_wake(&scan), milliseconds(900));
        let stuck = Waits {
            options_scan: Some(Duration::ZERO),
            ..Waits::default()
        };
        assert_eq!(next_wake(&stuck), RETRY_FLOOR);
    }

    #[test]
    fn the_resume_countdown_wakes_once_per_shown_second_and_at_the_resume() {
        assert_eq!(countdown_tick(milliseconds(4_300)), milliseconds(302));
        // "4" is already showing at exactly four seconds; "3" follows a second later.
        assert_eq!(countdown_tick(Duration::from_secs(4)), milliseconds(1_002));
        assert_eq!(countdown_tick(milliseconds(700)), milliseconds(702));
        for remaining in [4_300, 4_000, 700, 1] {
            let remaining = milliseconds(remaining);
            let before = paused_status(Some(remaining));
            let after = remaining.saturating_sub(countdown_tick(remaining));
            assert!(after.is_zero() || paused_status(Some(after)) != before);
        }

        let shown = Waits {
            auto_resume: Some(milliseconds(4_300)),
            countdown_shown: true,
            ..Waits::default()
        };
        assert_eq!(next_wake(&shown), milliseconds(302));
        // Unseen, the countdown costs nothing until capture actually resumes.
        let unseen = Waits {
            countdown_shown: false,
            ..shown.clone()
        };
        assert_eq!(next_wake(&unseen), SAFETY_TICK);
        let hidden = Waits {
            hidden: true,
            auto_resume: Some(milliseconds(900)),
            ..shown
        };
        assert_eq!(next_wake(&hidden), milliseconds(900));
    }

    #[test]
    fn the_recording_blink_wakes_at_each_half_second_while_it_is_visible() {
        assert_eq!(blink_tick(0.0), milliseconds(502));
        assert_eq!(blink_tick(0.2), milliseconds(302));
        assert_eq!(blink_tick(0.75).as_millis(), 252);
        assert_eq!(blink_tick(12.9).as_millis(), 102);
        // The LED really is in its other half when that wake-up arrives.
        for seconds in [0.0, 0.2, 0.499, 0.5, 0.75, 12.9] {
            let lit = |time: f64| time.rem_euclid(1.0) < 0.5;
            assert_ne!(
                lit(seconds),
                lit(seconds + blink_tick(seconds).as_secs_f64())
            );
        }
        let recording = Waits {
            blink: Some(0.2),
            ..Waits::default()
        };
        assert_eq!(next_wake(&recording), milliseconds(302));
        assert_eq!(
            next_wake(&Waits {
                hidden: true,
                ..recording
            }),
            SAFETY_TICK
        );
    }

    #[test]
    fn display_only_waits_stop_while_the_window_is_hidden() {
        let shown = Waits {
            meters_live: true,
            fine_edit: Some(milliseconds(100)),
            waveform: Some(milliseconds(40)),
            options_scan: Some(milliseconds(600)),
            ..Waits::default()
        };
        assert_eq!(next_wake(&shown), milliseconds(40));
        assert_eq!(
            next_wake(&Waits {
                waveform: None,
                ..shown.clone()
            }),
            milliseconds(151)
        );
        assert_eq!(
            next_wake(&Waits {
                waveform: None,
                fine_edit: None,
                ..shown.clone()
            }),
            METER_STALE_CHECK,
        );
        // A waveform that is already due is drawn on the next frame, not in a spin.
        assert_eq!(
            next_wake(&Waits {
                waveform: Some(Duration::ZERO),
                ..shown.clone()
            }),
            BUSY_REFRESH
        );
        assert_eq!(
            next_wake(&Waits {
                hidden: true,
                ..shown
            }),
            SAFETY_TICK
        );
    }

    #[test]
    fn paused_status_counts_down_to_the_resume_and_stays_plain_when_there_is_none() {
        assert_eq!(paused_status(None), "Paused");
        let remaining = |idle| AutoResumeDelay::Seconds10.remaining(Duration::from_millis(idle));
        assert_eq!(paused_status(remaining(0)), "Paused · resumes in 10 s");
        assert_eq!(paused_status(remaining(2_100)), "Paused · resumes in 8 s");
        assert_eq!(paused_status(remaining(9_950)), "Paused · resumes in 1 s");
        // The frame on which the delay runs out resumes capture and shows no zero.
        assert_eq!(paused_status(remaining(10_000)), "Paused · resumes in 1 s");
        assert_eq!(
            paused_status(AutoResumeDelay::Off.remaining(Duration::from_secs(60))),
            "Paused"
        );

        assert_eq!(idle_status(None, ""), "");
        assert_eq!(
            idle_status(None, "Saved snapshot a.wav"),
            "Saved snapshot a.wav"
        );
        assert_eq!(idle_status(Some("Paused".into()), ""), "Paused");
        assert_eq!(
            idle_status(
                Some(paused_status(remaining(3_000))),
                "Saved snapshot a.wav"
            ),
            "Paused · resumes in 7 s · Saved snapshot a.wav"
        );
    }

    #[test]
    fn the_status_mark_follows_the_users_choice() {
        use CaptureIndicator::{Paused, Recording, Unavailable, Waiting};
        for indicator in [Paused, Recording, Unavailable, Waiting] {
            for problem in [false, true] {
                assert!(status_mark_shown(StatusMark::Always, indicator, problem));
                assert!(!status_mark_shown(StatusMark::Never, indicator, problem));
            }
        }
        // The quiet choice hides the mark while all is well and brings it back for the rest.
        assert!(!status_mark_shown(StatusMark::Attention, Recording, false));
        assert!(!status_mark_shown(StatusMark::Attention, Waiting, false));
        assert!(status_mark_shown(StatusMark::Attention, Paused, false));
        assert!(status_mark_shown(StatusMark::Attention, Unavailable, false));
        assert!(status_mark_shown(StatusMark::Attention, Recording, true));
    }

    #[test]
    fn capture_indicator_ranks_failure_over_pause_over_recording_over_waiting() {
        use CaptureIndicator::{Paused, Recording, Unavailable, Waiting};
        assert_eq!(CaptureIndicator::new(true, true, true), Unavailable);
        assert_eq!(CaptureIndicator::new(false, true, true), Paused);
        assert_eq!(CaptureIndicator::new(false, false, true), Recording);
        assert_eq!(CaptureIndicator::new(false, false, false), Waiting);
        assert_eq!(
            [Unavailable, Paused, Recording, Waiting].map(CaptureIndicator::label),
            [
                "Capture unavailable",
                "Paused",
                "Recording",
                "Waiting for sound"
            ]
        );

        // In waveform-only mode colour alone has to tell the four states apart, in both halves
        // of the recording blink, against the dark plot. A look's plot backing is held to the
        // same test (`Theme::legible`).
        let plot = Color32::from_rgb(22, 26, 29);
        for lit in [false, true] {
            let colors =
                [Unavailable, Paused, Recording, Waiting].map(|state| state.compact_color(lit));
            for (index, color) in colors.iter().enumerate() {
                assert!(
                    backtracking_ui::distinguishable(*color, plot),
                    "{color:?} is lost on the plot"
                );
                for other in &colors[index + 1..] {
                    assert!(
                        backtracking_ui::distinguishable(*color, *other),
                        "{color:?} is too close to {other:?}"
                    );
                }
            }
        }
        assert_eq!(Recording.compact_color(true), Recording.led_color(true));
        assert_eq!(Unavailable.compact_color(false), ERROR_LED);
    }

    #[test]
    fn compact_status_reports_the_failing_source_and_keeps_the_latest_message() {
        assert_eq!(capture_problem(None, [None, None], true), None);
        assert_eq!(
            capture_problem(Some("No audio device"), [Some("unplugged"), None], true).as_deref(),
            Some("No audio device")
        );
        // One lane can fail while the other keeps recording, so name the lane.
        assert_eq!(
            capture_problem(None, [None, Some("unplugged")], true).as_deref(),
            Some("Source B: unplugged")
        );
        assert_eq!(
            capture_problem(None, [Some("no center channel"), None], false).as_deref(),
            Some("no center channel")
        );

        assert_eq!(compact_status_text("Recording", None, ""), "Recording");
        assert_eq!(
            compact_status_text("Paused · resumes in 7 s", None, "Saved snapshot a.wav"),
            "Paused · resumes in 7 s · Saved snapshot a.wav"
        );
        assert_eq!(
            compact_status_text(
                "Capture unavailable",
                Some("No audio device"),
                "No audio device"
            ),
            "Capture unavailable · No audio device"
        );
    }

    #[test]
    fn compact_status_mark_fits_the_smallest_window_and_truncates_a_long_problem() {
        let plot = Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(WAVEFORM_MIN_WIDTH, WAVEFORM_MIN_HEIGHT),
        );
        let paint = |indicator, problem: Option<&str>| {
            let context = egui::Context::default();
            let mut hover = Rect::NOTHING;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(plot),
                    ..Default::default()
                },
                |ui| {
                    hover =
                        paint_compact_status(&ui.painter_at(plot), plot, indicator, true, problem);
                },
            );
            let shapes: Vec<_> = std::mem::take(&mut output.shapes)
                .into_iter()
                .map(|clipped| clipped.shape)
                .collect();
            output.drop_without_applying_deltas();
            (hover, shapes)
        };
        let fill = |shape: &egui::Shape| match shape {
            egui::Shape::Circle(circle) => Some(circle.fill),
            egui::Shape::Rect(rect) => Some(rect.fill),
            _ => None,
        };
        let written = |shapes: &[egui::Shape]| {
            shapes.iter().find_map(|shape| match shape {
                egui::Shape::Text(text) => Some(text.clone()),
                _ => None,
            })
        };

        for indicator in [
            CaptureIndicator::Recording,
            CaptureIndicator::Waiting,
            CaptureIndicator::Unavailable,
        ] {
            let (hover, shapes) = paint(indicator, None);
            assert!(
                plot.contains_rect(hover) && hover.width() >= 12.0,
                "{indicator:?} {hover:?}"
            );
            assert!(
                shapes
                    .iter()
                    .filter_map(fill)
                    .any(|color| color == indicator.compact_color(true))
            );
            assert!(written(&shapes).is_none());
        }
        // Paused is two bars, so it differs from the other states by shape as well as colour.
        let (_, shapes) = paint(CaptureIndicator::Paused, None);
        let bars = shapes
            .iter()
            .filter_map(fill)
            .filter(|color| *color == COMPACT_PAUSED)
            .count();
        assert_eq!(bars, 2);

        // A source problem turns the mark amber whatever else capture is doing, and its text
        // stays on one line inside the plot however long the message is.
        let problem =
            "Source B: the audio device was unplugged or is in use by another application "
                .repeat(4);
        let (hover, shapes) = paint(CaptureIndicator::Recording, Some(&problem));
        assert!(
            shapes
                .iter()
                .filter_map(fill)
                .any(|color| color == ERROR_LED)
        );
        assert!(
            !shapes
                .iter()
                .filter_map(fill)
                .any(|color| color == RECORDING_LIT)
        );
        let text = written(&shapes).expect("the problem is written on the plot");
        assert_eq!(text.galley.rows.len(), 1);
        assert!(text.galley.elided);
        assert!(text.pos.x + text.galley.size().x <= plot.right());
        assert!(
            plot.contains_rect(hover) && hover.width() > 150.0,
            "{hover:?}"
        );

        let (hover, shapes) = paint(CaptureIndicator::Unavailable, Some("No audio device"));
        let text = written(&shapes).unwrap();
        assert_eq!(text.galley.text(), "No audio device");
        assert!(!text.galley.elided);
        assert!(hover.right() >= text.pos.x + text.galley.size().x - 0.5);
    }

    #[test]
    fn window_keys_run_commands_once_and_leave_other_chords_alone() {
        let key = |key, modifiers, pressed, repeat| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers,
        };
        let none = egui::Modifiers::NONE;
        let defaults = Hotkeys::defaults();
        let press =
            |code, modifiers| window_shortcut(&[key(code, modifiers, true, false)], &defaults);
        assert_eq!(
            press(egui::Key::Space, none),
            Some(WindowCommand::ToggleCapture)
        );
        assert_eq!(press(egui::Key::Delete, none), Some(WindowCommand::Clear));
        // Ctrl+S stays free: snapshots have the global shortcut, and a second key for the same
        // save only invites duplicate files. Windows reports Ctrl as both `ctrl` and `command`.
        let ctrl = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
        assert_eq!(press(egui::Key::S, egui::Modifiers::COMMAND), None);
        assert_eq!(press(egui::Key::S, ctrl), None);
        assert_eq!(press(egui::Key::S, none), None);
        // Alt+Space belongs to the system menu and Alt is the window-move modifier.
        assert_eq!(press(egui::Key::Space, egui::Modifiers::ALT), None);
        assert_eq!(press(egui::Key::Delete, egui::Modifiers::SHIFT), None);
        // Backspace and Escape keep their existing job of cancelling a selection.
        assert_eq!(press(egui::Key::Backspace, none), None);
        assert_eq!(press(egui::Key::Escape, none), None);

        // A held key repeats, and releasing it is not a press: neither toggles capture again.
        assert_eq!(
            window_shortcut(&[key(egui::Key::Space, none, true, true)], &defaults),
            None
        );
        assert_eq!(
            window_shortcut(&[key(egui::Key::Space, none, false, false)], &defaults),
            None
        );
        assert_eq!(
            window_shortcut(
                &[
                    egui::Event::PointerMoved(Pos2::ZERO),
                    key(egui::Key::Space, none, true, true),
                    key(egui::Key::Delete, none, true, false),
                ],
                &defaults
            ),
            Some(WindowCommand::Clear)
        );
    }

    fn chord(modifiers: Modifiers, key: crate::hotkeys::Key) -> Binding {
        Binding::Chord { modifiers, key }
    }

    /// A key press as egui-winit reports it on a US layout.
    fn pressed(modifiers: Modifiers, key: crate::hotkeys::Key) -> egui::Event {
        hotkeys_page::key_event(modifiers, key, true)
    }

    #[test]
    fn rebound_window_keys_run_their_new_command_and_free_the_old_key() {
        use crate::hotkeys::Key as K;
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        };
        let none = Modifiers::NONE;
        let press = |hotkeys: &Hotkeys, modifiers, key| {
            window_shortcut(&[pressed(modifiers, key)], hotkeys)
        };
        let mut hotkeys = Hotkeys::defaults();
        hotkeys
            .set(Action::ToggleCapture, &[chord(none, K::F6)])
            .unwrap();
        assert_eq!(
            press(&hotkeys, none, K::F6),
            Some(WindowCommand::ToggleCapture)
        );
        assert_eq!(press(&hotkeys, none, K::Space), None);
        // The whole chord has to match.
        assert_eq!(press(&hotkeys, ctrl, K::F6), None);
        hotkeys.clear(Action::ToggleCapture);
        assert_eq!(press(&hotkeys, none, K::F6), None);
        // An action without a default works once it has a key.
        assert_eq!(press(&hotkeys, ctrl, K::O), None);
        hotkeys
            .set(Action::OpenOptions, &[chord(ctrl, K::O)])
            .unwrap();
        assert_eq!(press(&hotkeys, ctrl, K::O), Some(WindowCommand::Options));
        for (action, command) in [
            (Action::SaveSnapshot, WindowCommand::SaveSnapshot),
            (Action::WaveformOnly, WindowCommand::WaveformOnly),
            (Action::KeepOnTop, WindowCommand::KeepOnTop),
            (Action::Quit, WindowCommand::Quit),
        ] {
            hotkeys.set(action, &[chord(ctrl, K::F7)]).unwrap();
            assert_eq!(press(&hotkeys, ctrl, K::F7), Some(command), "{action:?}");
            hotkeys.clear(action);
        }
        // The system-wide chord is not the window's, and a cancel key is no command.
        let alt = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(press(&hotkeys, alt, K::KeyLeftOf1), None);
        assert_eq!(press(&hotkeys, none, K::Escape), None);
        // A held key still runs nothing more.
        let held = egui::Event::Key {
            key: egui::Key::O,
            physical_key: Some(egui::Key::O),
            pressed: true,
            repeat: true,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        };
        assert_eq!(window_shortcut(&[held], &hotkeys), None);
    }

    #[test]
    fn edit_separately_follows_its_binding_and_the_tap_follows_its_action() {
        use crate::hotkeys::Key as K;
        let mut hotkeys = Hotkeys::defaults();
        assert_eq!(tap_command(&hotkeys), Some(WindowCommand::SeparateLanes));
        hotkeys
            .set(Action::SeparateLanes, &[chord(Modifiers::NONE, K::E)])
            .unwrap();
        assert_eq!(
            window_shortcut(&[pressed(Modifiers::NONE, K::E)], &hotkeys),
            Some(WindowCommand::SeparateLanes)
        );
        // Replaced, the tap does nothing.
        assert_eq!(tap_command(&hotkeys), None);
        // Only Edit separately can have the tap; another action is refused and keeps its keys.
        for action in [Action::KeepOnTop, Action::CancelSelection] {
            let before = hotkeys.clone();
            assert_eq!(
                hotkeys.set(action, &[Binding::RightCtrlTap]),
                Err(BindingError::Reserved(ReservedShortcut::RightCtrlTap)),
                "{action:?}"
            );
            assert_eq!(hotkeys, before);
            assert_eq!(tap_command(&hotkeys), None);
        }
        // Back on Edit separately, as its second binding, the tap runs it again.
        hotkeys
            .set(
                Action::SeparateLanes,
                &[chord(Modifiers::NONE, K::E), Binding::RightCtrlTap],
            )
            .unwrap();
        assert_eq!(tap_command(&hotkeys), Some(WindowCommand::SeparateLanes));
    }

    /// The menu's texts as `context_menu::show` writes them.
    fn menu_texts(language: Language, hotkeys: &Hotkeys) -> Vec<String> {
        let state = context_menu::State {
            transport: context_menu::Transport::Pause,
            capture_controls: true,
            waveform_only: false,
            keep_on_top: false,
            several_rows: true,
            separate_rows: false,
            separate_label: "Edit separately",
        };
        menu_entries(state, language, hotkeys)
            .iter()
            .map(|entry| match entry {
                context_menu::Entry::Separator => "-".to_owned(),
                context_menu::Entry::Item {
                    label,
                    shortcut: Some(shortcut),
                    ..
                } => format!("{label}	{shortcut}"),
                context_menu::Entry::Item { label, .. } => (*label).to_owned(),
            })
            .collect()
    }

    #[test]
    fn menu_shortcuts_follow_changed_and_cleared_bindings() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        };
        let defaults = [
            "Pause	Space",
            "Clear	Delete",
            "Save snapshot",
            "-",
            "Edit separately	Right Ctrl",
            "Show waveform only",
            "Keep on top",
            "-",
            "Options…",
            "Quit",
        ];
        for language in [Language::English, Language::TraditionalChinese] {
            assert_eq!(menu_texts(language, &Hotkeys::defaults()), defaults);
        }
        let mut hotkeys = Hotkeys::defaults();
        hotkeys.clear(Action::ToggleCapture);
        hotkeys
            .set(
                Action::Clear,
                &[
                    chord(ctrl, crate::hotkeys::Key::K),
                    chord(Modifiers::NONE, crate::hotkeys::Key::Delete),
                ],
            )
            .unwrap();
        hotkeys
            .set(
                Action::SeparateLanes,
                &[chord(Modifiers::NONE, crate::hotkeys::Key::E)],
            )
            .unwrap();
        hotkeys
            .set(
                Action::OpenOptions,
                &[chord(ctrl, crate::hotkeys::Key::Comma)],
            )
            .unwrap();
        assert_eq!(
            menu_texts(Language::English, &hotkeys),
            [
                "Pause",
                // The menu names the first of two.
                "Clear	Ctrl + K",
                "Save snapshot",
                "-",
                "Edit separately	E",
                "Show waveform only",
                "Keep on top",
                "-",
                "Options…	Ctrl + ,",
                "Quit",
            ]
        );
    }

    #[test]
    fn a_selection_turns_the_pause_and_clear_keys_into_preview_and_cancel() {
        use crate::hotkeys::Key as K;
        for command in [
            WindowCommand::ToggleCapture,
            WindowCommand::Clear,
            WindowCommand::SaveSnapshot,
            WindowCommand::SeparateLanes,
            WindowCommand::Options,
        ] {
            assert_eq!(key_effect(command, false), KeyEffect::Run(command));
        }
        assert_eq!(
            key_effect(WindowCommand::ToggleCapture, true),
            KeyEffect::TogglePreview
        );
        assert_eq!(
            key_effect(WindowCommand::Clear, true),
            KeyEffect::CancelSelection
        );
        assert_eq!(
            key_effect(WindowCommand::SaveSnapshot, true),
            KeyEffect::Run(WindowCommand::SaveSnapshot)
        );
        // The rules go with the action, whatever key it has.
        let mut hotkeys = Hotkeys::defaults();
        hotkeys
            .set(Action::ToggleCapture, &[chord(Modifiers::NONE, K::P)])
            .unwrap();
        hotkeys
            .set(Action::Clear, &[chord(Modifiers::NONE, K::X)])
            .unwrap();
        let effect = |key| {
            window_shortcut(&[pressed(Modifiers::NONE, key)], &hotkeys)
                .map(|command| key_effect(command, true))
        };
        assert_eq!(effect(K::P), Some(KeyEffect::TogglePreview));
        assert_eq!(effect(K::X), Some(KeyEffect::CancelSelection));
        assert_eq!(effect(K::Space), None);
        assert_eq!(effect(K::Delete), None);
    }

    #[test]
    fn cancel_keys_take_any_modifiers_when_bound_bare_and_follow_rebinding() {
        use crate::hotkeys::Key as K;
        let shift = Modifiers {
            shift: true,
            ..Modifiers::NONE
        };
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        };
        let defaults = Hotkeys::defaults();
        let raw = |key, modifiers, pressed, repeat| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers,
        };
        let none = egui::Modifiers::NONE;
        // As before the Hotkeys page: Escape and Backspace, with modifiers held and repeats too.
        for event in [
            raw(egui::Key::Escape, none, true, false),
            raw(egui::Key::Backspace, none, true, false),
            raw(egui::Key::Escape, egui::Modifiers::SHIFT, true, false),
            raw(egui::Key::Backspace, egui::Modifiers::CTRL, true, true),
        ] {
            assert!(is_cancel_key(&event, &defaults), "{event:?}");
        }
        for event in [
            raw(egui::Key::Escape, none, false, false),
            raw(egui::Key::Delete, none, true, false),
            raw(egui::Key::Space, none, true, false),
        ] {
            assert!(!is_cancel_key(&event, &defaults), "{event:?}");
        }
        let mut hotkeys = Hotkeys::defaults();
        hotkeys
            .set(
                Action::CancelSelection,
                &[chord(Modifiers::NONE, K::Q), chord(ctrl, K::Z)],
            )
            .unwrap();
        assert!(is_cancel_key(&pressed(shift, K::Q), &hotkeys));
        assert!(is_cancel_key(&pressed(ctrl, K::Z), &hotkeys));
        // A binding with modifiers needs exactly those.
        assert!(!is_cancel_key(&pressed(Modifiers::NONE, K::Z), &hotkeys));
        assert!(!is_cancel_key(&pressed(shift, K::Z), &hotkeys));
        assert!(!is_cancel_key(
            &pressed(Modifiers::NONE, K::Escape),
            &hotkeys
        ));
        hotkeys.clear(Action::CancelSelection);
        assert!(!is_cancel_key(
            &pressed(Modifiers::NONE, K::Escape),
            &hotkeys
        ));
    }

    #[test]
    fn selection_and_unread_snapshot_cleanup_run_off_the_calling_thread() {
        struct ObservedDrop<T> {
            _value: T,
            observed: mpsc::Sender<thread::ThreadId>,
        }
        impl<T> Drop for ObservedDrop<T> {
            fn drop(&mut self) {
                let _ = self.observed.send(thread::current().id());
            }
        }
        let retirement = UiRetirement::new().unwrap();
        let caller = thread::current().id();
        let snapshots: Arc<[FrozenTrack]> =
            vec![FrozenTrack::from_interleaved(48000, 1, 1, &[0.1; 4]).unwrap()].into();
        let first = Arc::downgrade(&snapshots);
        let (observed, drops) = mpsc::channel();
        assert!(
            retirement
                .retire(ObservedDrop {
                    _value: FrozenEdit {
                        snapshots,
                        lanes: vec![0],
                        bars: vec![1],
                        retention_seconds: 600.0,
                        export_options: ExportOptions::default(),
                        groups: Default::default(),
                        shown_groups: Default::default(),
                        positions: no_positions(),
                        activity: [None; 2],
                        activity_job: None,
                    },
                    observed: observed.clone()
                })
                .is_ok()
        );
        assert_ne!(drops.recv_timeout(Duration::from_secs(2)).unwrap(), caller);

        let snapshots: Arc<[FrozenTrack]> =
            vec![FrozenTrack::from_interleaved(48000, 1, 1, &[0.2; 4]).unwrap()].into();
        let second = Arc::downgrade(&snapshots);
        let (result, unread) = mpsc::channel();
        result.send(Some(snapshots)).unwrap();
        drop(result);
        assert!(
            retirement
                .retire(ObservedDrop {
                    _value: unread,
                    observed
                })
                .is_ok()
        );
        assert_ne!(drops.recv_timeout(Duration::from_secs(2)).unwrap(), caller);
        drop(retirement);
        assert!(first.upgrade().is_none());
        assert!(
            second.upgrade().is_none(),
            "Discarding an unread reply must release its last audio reference on cleanup"
        );
    }

    #[test]
    fn an_unstarted_wav_job_keeps_its_audio_available_for_background_retirement() {
        let snapshots: Arc<[FrozenTrack]> =
            vec![FrozenTrack::from_interleaved(48000, 1, 1, &[0.1; 4]).unwrap()].into();
        let weak = Arc::downgrade(&snapshots);
        let clip = WavClip::new(
            snapshots,
            &[0],
            None,
            Fades::NONE,
            PathBuf::new(),
            ClipFormat::unnamed([ChannelRoute::Merged; 2], false),
            None,
        )
        .unwrap();
        let (payload, job) = wav_job(clip, false);
        drop(job); // Thread creation failure drops the closure before it starts.
        let retained = payload.lock().unwrap().take().unwrap();
        drop(payload);
        assert!(weak.upgrade().is_some());
        let retirement = UiRetirement::new().unwrap();
        assert!(retirement.retire(retained).is_ok());
        drop(retirement);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn desktop_snap_uses_audio_pixels_and_visible_marker_priority() {
        // The retained audio occupies only the rightmost 100 pixels of a larger panel.
        let audio = Rect::from_min_size(Pos2::new(900.0, 40.0), Vec2::new(100.0, 80.0));
        let guides = waveform::TimelineGuides {
            bars: vec![2000],
            seconds: vec![2500],
        };
        assert_eq!(
            frame_from_pointer(audio, 915.0, 1000..11000, true, &[1900], &guides),
            1900
        );
        assert_eq!(
            frame_from_pointer(audio, 915.0, 1000..11000, true, &[1600], &guides),
            2000
        );
        assert_eq!(
            frame_from_pointer(
                audio,
                915.0,
                1000..11000,
                true,
                &[],
                &waveform::TimelineGuides {
                    bars: vec![],
                    seconds: vec![2500]
                }
            ),
            2500
        );
        assert_eq!(
            frame_from_pointer(
                audio,
                914.5,
                1000..11000,
                true,
                &[],
                &waveform::TimelineGuides::default()
            ),
            2450
        );
        assert_eq!(
            frame_from_pointer(audio, 914.5, 1000..11000, false, &[1900], &guides),
            2450
        );
        assert_eq!(
            frame_from_pointer(
                audio,
                915.0,
                1000..11000,
                true,
                &[1900],
                &waveform::TimelineGuides::default()
            ),
            1900,
            "Transparent backgrounds still show flags"
        );
    }

    #[test]
    fn pinned_preview_preserves_absolute_frames_and_independent_channels() {
        let stereo = FrozenTrack::from_interleaved(
            48000,
            2,
            1000,
            &[0.1, 0.7, 0.2, 0.6, 0.3, 0.5, 0.4, 0.4],
        )
        .unwrap();
        let key = EditPreviewKey {
            frames: 1001..1003,
            rows: vec![
                RowSource::View(0, WaveformView::Left),
                RowSource::View(0, WaveformView::Right),
            ],
            pairs: [None, None],
            bins: 2,
        };
        let peaks = edit_preview(&[stereo], &[0], &key);
        assert_eq!(peaks[0].low, [0.2, 0.3]);
        assert_eq!(peaks[0].high, [0.2, 0.3]);
        assert_eq!(peaks[1].low, [0.6, 0.5]);
        let tracks = [
            FrozenTrack::from_interleaved(48000, 1, 1000, &[0.1, 0.2, 0.3, 0.4]).unwrap(),
            FrozenTrack::from_interleaved(48000, 1, 1000, &[-0.1, -0.2, -0.3, -0.4]).unwrap(),
        ];
        let key = EditPreviewKey {
            rows: vec![
                RowSource::View(0, WaveformView::Merged),
                RowSource::View(1, WaveformView::Merged),
            ],
            ..key
        };
        let peaks = edit_preview(&tracks, &[0, 1], &key);
        assert_eq!(peaks[0].low, [0.2, 0.3]);
        assert_eq!(peaks[1].high, [-0.2, -0.3]);
        let peaks = edit_preview(&tracks[1..], &[1], &key);
        assert_eq!(
            peaks[0].low,
            [0.0, 0.0],
            "Unavailable lane A must stay empty"
        );
        assert_eq!(
            peaks[1].high,
            [-0.2, -0.3],
            "Lane B must retain its logical row"
        );
    }

    #[test]
    fn options_typography_is_readable_without_changing_parent_ui() {
        let context = egui::Context::default();
        configure_style(&context, Color32::WHITE);
        context
            .run_ui(egui::RawInput::default(), |ui| {
                let original = ui.style().text_styles.clone();
                options_list(ui, |ui| {
                    assert!(ui.style().text_styles[&egui::TextStyle::Body].size >= 15.0);
                    assert!(ui.style().text_styles[&egui::TextStyle::Button].size >= 15.0);
                    assert!(ui.style().text_styles[&egui::TextStyle::Small].size >= 13.0);
                });
                assert_eq!(
                    ui.style().text_styles,
                    original,
                    "Options typography must not enlarge the main waveform controls"
                );
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn options_popup_capacity_stops_at_ten_rows_and_short_menus_do_not_reserve_extra_rows() {
        let context = egui::Context::default();
        configure_style(&context, Color32::WHITE);
        context
            .run_ui(egui::RawInput::default(), |ui| {
                options_list(ui, |ui| {
                    let short = options_menu_height(ui, 3);
                    let ten = options_menu_height(ui, 10);
                    assert!(
                        short < ten,
                        "Three choices should not reserve the full menu height"
                    );
                    assert_eq!(ten, options_menu_height(ui, 11));
                    assert_eq!(ten, options_menu_height(ui, 200));
                    let row = options_menu_height(ui, 1);
                    let spacing = 4.0;
                    assert!((ten - (row * 10.0 + spacing * 9.0)).abs() < 0.01);
                });
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn options_menu_height_fits_real_rows_and_scrolls_after_the_tenth() {
        for count in [1, 3, 10, 15] {
            let context = egui::Context::default();
            configure_style(&context, Color32::WHITE);
            context.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 700.0))),
                ..Default::default()
            }, |ui| {
                options_list(ui, |ui| {
                    let height = options_menu_height(ui, count);
                    let scroll = ui.scope(|ui| {
                        egui::ScrollArea::vertical().max_height(height).show(ui, |ui| {
                            configure_options_menu(ui, 380.0);
                            (0..count).map(|index| {
                                ui.selectable_label(false, format!("Choice {index}")).rect
                            }).collect::<Vec<_>>()
                        })
                    });
                    let output = scroll.inner;
                    // ScrollAreaOutput.inner_rect is captured before auto-shrink and
                    // can still report the default 64px minimum for one 32px row.
                    // The surrounding scope measures the final space actually used.
                    let viewport = scroll.response.rect;
                    for row in output.inner.iter().take(10) {
                        assert!(row.top() >= viewport.top() - 0.1);
                        assert!(row.bottom() <= viewport.bottom() + 0.1,
                            "Menu clipped one of its first ten rows: {row:?} in {viewport:?}");
                    }
                    if count > 10 {
                        assert!(output.inner[10].bottom() > viewport.bottom());
                        assert!(output.content_size.y > viewport.height());
                    } else {
                        assert!((output.content_size.y - viewport.height()).abs() <= 0.1,
                            "Short menus should shrink to their actual content: count={count}, limit={height}, content={:?}, viewport={:?}, rows={:?}",
                            output.content_size, viewport, output.inner);
                    }
                });
            }).drop_without_applying_deltas();
        }
    }

    #[test]
    fn options_list_stacks_controls_and_scrolls_without_horizontal_overflow() {
        for width in [280.0, 360.0, 480.0] {
            let context = egui::Context::default();
            configure_style(&context, Color32::WHITE);
            for _ in 0..3 {
                context.run_ui(egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 150.0))),
                    ..Default::default()
                }, |ui| {
                    ui.horizontal_top(|ui| {
                        let mut rows = Vec::new();
                        let output = egui::ScrollArea::vertical().auto_shrink([false, false])
                            .show(ui, |ui| options_list(ui, |ui| {
                                let mut enabled = true;
                                for label in ["Show waveform only", "Keep on top", "Show bar lines"] {
                                    rows.push(ui.checkbox(&mut enabled, label).rect);
                                }
                                let mut retention = DurationChoice::Minutes10;
                                options_field(ui, "Retain the latest audio", |ui| {
                                    options_choice(ui, "test-history", &mut retention,
                                        DurationChoice::ALL.into_iter().map(|value| (value, history_label(value))));
                                });
                                let mut source = 0;
                                options_field(ui, "Audio source", |ui| {
                                    options_choice(ui, "test-source", &mut source,
                                        [(0, "A long sender name that must stay inside the dropdown ".repeat(5))]);
                                });
                                ui.add(egui::Label::new("A long output folder path ".repeat(12)).truncate());
                            }));
                        assert!(rows.windows(2).all(|pair| pair[1].top() >= pair[0].bottom()),
                            "Setting rows must extend down even inside a horizontal parent: {rows:?}");
                        assert!(output.content_size.x <= output.inner_rect.width() + 1.0,
                            "Form overflow at width {width}: {:?}", output.content_size);
                        assert!(output.content_size.y > output.inner_rect.height(),
                            "A long settings list must scroll vertically");
                    });
                }).drop_without_applying_deltas();
            }
        }
    }

    #[test]
    fn dual_drag_exports_selected_routes_and_cleans_up_canceled_files() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "Backtracking-dual-drag-{}-{nonce}",
            std::process::id()
        ));
        let snapshots: Arc<[FrozenTrack]> = vec![
            FrozenTrack::from_interleaved(48_000, 1, 10, &[0.0, 0.5, 1.0]).unwrap(),
            FrozenTrack::from_interleaved(48_000, 1, 10, &[0.0, -0.5, -1.0]).unwrap(),
        ]
        .into();
        let prepare = |force_mono| {
            WavClip::new(
                snapshots.clone(),
                &[0, 1],
                Some(&(11..12)),
                Fades::NONE,
                folder.clone(),
                ClipFormat::unnamed([ChannelRoute::Left, ChannelRoute::Side], force_mono),
                None,
            )
            .unwrap()
            .write(false)
            .unwrap()
        };
        let assert_samples = |paths: &[PathBuf], expected: [&[f32]; 2]| {
            assert_eq!(paths.len(), 2);
            for (path, samples) in paths.iter().zip(expected) {
                let wav = std::fs::read(path).unwrap();
                let channels = u16::from_le_bytes(wav[22..24].try_into().unwrap());
                assert_eq!(usize::from(channels), samples.len());
                let expected: Vec<_> = samples
                    .iter()
                    .flat_map(|sample| sample.to_le_bytes())
                    .collect();
                assert_eq!(&wav[wav.len() - expected.len()..], expected);
            }
        };

        let rejected = prepare(false);
        assert_samples(&rejected.paths, [&[0.5, 0.0], &[-0.5, 0.5]]);
        let rejected_paths = rejected.paths.clone();
        drop(rejected);
        assert!(rejected_paths.iter().all(|path| !path.exists()));

        let mut accepted = prepare(true);
        assert_samples(&accepted.paths, [&[0.5], &[-0.5]]);
        let accepted_paths = accepted.paths.clone();
        accepted.keep = true;
        drop(accepted);
        assert!(accepted_paths.iter().all(|path| path.is_file()));
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn snapshot_export_keeps_the_group_labels_frozen_with_its_audio() {
        use crate::controller::LaneGroup;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "Backtracking-frozen-group-{}-{nonce}",
            std::process::id()
        ));
        let snapshot = EditSnapshot {
            snapshots: vec![
                FrozenTrack::from_interleaved(48_000, 6, 100, &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6])
                    .unwrap(),
            ]
            .into(),
            generation: 1,
            lanes: vec![1],
            bar_markers: Vec::new(),
            groups: [
                Vec::new(),
                vec![
                    LaneGroup {
                        id: "bed".into(),
                        positioned: false,
                        label: "Bed".into(),
                        channels: 0..2,
                        labels: vec!["L".into(), "R".into()],
                    },
                    LaneGroup {
                        id: "objects".into(),
                        positioned: true,
                        label: "Objects".into(),
                        channels: 2..6,
                        labels: vec![
                            "Object 1".into(),
                            "Object 2".into(),
                            "Object 3".into(),
                            "Object 4".into(),
                        ],
                    },
                ],
            ],
            shown_groups: [None, Some("objects".into())],
            positions: std::array::from_fn(|_| backtracking_core::PositionTrack::new().freeze()),
        };
        let spec = SnapshotSpec {
            selection: None,
            fades: Fades::NONE,
            output: folder.clone(),
            format: ClipFormat::unnamed([ChannelRoute::Merged; 2], false),
            flags: None,
            export_options: ExportOptions::default(),
            rows: Vec::new(),
            whole: [false; 2],
        };
        let clip = spec.frozen_clip(&snapshot).unwrap();
        drop(snapshot);
        let files = clip.write(false).unwrap();
        // The objects on show, which have no positions and so are written in mono and say so,
        // then the pair, made from the bed frozen with them.
        assert_eq!(files.paths.len(), 5);
        assert!(files.objects_in_mono);
        assert!(
            files.paths[4].to_string_lossy().ends_with("-Stereo.wav"),
            "{:?}",
            files.paths
        );
        for (index, path) in files.paths[..4].iter().enumerate() {
            assert!(
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(&format!("Object {}", index + 1))
            );
            let wav = std::fs::read(path).unwrap();
            assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
            let sample = f32::from_le_bytes(wav[wav.len() - 4..].try_into().unwrap());
            assert!((sample - [0.3, 0.4, 0.5, 0.6][index]).abs() < 0.000001);
        }
        drop(files);
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn snapshot_export_uses_lane_b_route_when_lane_a_is_unavailable() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "Backtracking-lane-b-route-{}-{nonce}",
            std::process::id()
        ));
        let snapshot = EditSnapshot {
            snapshots: vec![FrozenTrack::from_interleaved(48_000, 1, 100, &[0.5]).unwrap()].into(),
            generation: 1,
            lanes: vec![1],
            bar_markers: Vec::new(),
            groups: [Vec::new(), Vec::new()],
            shown_groups: [None, None],
            positions: std::array::from_fn(|_| backtracking_core::PositionTrack::new().freeze()),
        };
        let spec = SnapshotSpec {
            selection: None,
            fades: Fades::NONE,
            output: folder.clone(),
            format: ClipFormat::unnamed([ChannelRoute::Left, ChannelRoute::Right], false),
            flags: None,
            export_options: ExportOptions::default(),
            rows: Vec::new(),
            whole: [false; 2],
        };
        let files = spec.frozen_clip(&snapshot).unwrap().write(false).unwrap();
        assert_eq!(files.paths.len(), 1);
        let wav = std::fs::read(&files.paths[0]).unwrap();
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 2);
        assert_eq!(
            f32::from_le_bytes(wav[wav.len() - 8..wav.len() - 4].try_into().unwrap()),
            0.0
        );
        assert_eq!(
            f32::from_le_bytes(wav[wav.len() - 4..].try_into().unwrap()),
            0.5
        );
        drop(files);
        std::fs::remove_dir(folder).unwrap();
    }

    fn group(id: &str, channels: Range<usize>) -> LaneGroup {
        LaneGroup {
            id: id.into(),
            positioned: false,
            label: id.to_uppercase(),
            labels: channels
                .clone()
                .map(|channel| format!("ch{channel}"))
                .collect(),
            channels,
        }
    }

    #[test]
    fn a_wide_lane_shows_its_chosen_group_or_the_first_or_every_channel_numbered() {
        let groups = [group("bed", 0..12), group("objects", 12..28)];
        let (shown, choices) = group_on_show(&groups, Some("objects"), 28);
        assert_eq!(shown, groups[1]);
        assert_eq!(
            choices,
            [
                ("bed".into(), "BED".into()),
                ("objects".into(), "OBJECTS".into())
            ]
        );
        assert_eq!(group_on_show(&groups, Some("gone"), 28).0, groups[0]);
        assert_eq!(group_on_show(&groups, None, 28).0, groups[0]);
        // A group the audio is too narrow for, or no groups at all, falls back to numbers.
        let (numbered, choices) = group_on_show(&groups[1..], None, 12);
        assert_eq!((numbered.channels, numbered.labels.len()), (0..12, 12));
        assert_eq!(numbered.labels[11], "12");
        assert_eq!(choices.len(), 1);
        assert_eq!(group_on_show(&[], None, 6).0.channels, 0..6);
    }

    #[test]
    fn a_stacked_lane_draws_and_asks_for_each_channel_of_its_group_in_order() {
        let plan = LanePlan::stack(1, group("objects", 12..28), Vec::new());
        assert!(plan.stacked());
        assert_eq!(plan.rows(), 16);
        assert_eq!(plan.row(0), RowSource::Channel(1, 12));
        assert_eq!(plan.row(15), RowSource::Channel(1, 27));
        let stereo = LanePlan {
            lane: 0,
            kind: LaneKind::Views(vec![WaveformView::Left, WaveformView::Right]),
        };
        assert!(!stereo.stacked());
        assert_eq!(stereo.row(1), RowSource::View(0, WaveformView::Right));
        // Only rows on screen reach the preview: three channels of a scrolled 12-channel lane.
        let track = FrozenTrack::from_interleaved(
            48000,
            12,
            0,
            &(0..24)
                .map(|sample| sample as f32 / 100.0)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let key = EditPreviewKey {
            frames: 0..2,
            rows: (4..7)
                .map(|channel| RowSource::Channel(0, channel))
                .collect(),
            pairs: [None, None],
            bins: 2,
        };
        let peaks = edit_preview(&[track], &[0], &key);
        assert_eq!(peaks.len(), 3);
        assert_eq!(peaks[0].high, [0.04, 0.16]);
        assert_eq!(peaks[2].low, [0.06, 0.18]);
    }

    #[test]
    fn wide_lanes_keep_peak_and_vu_but_show_no_loudness_number() {
        let wide = LaneMeterReadings {
            peak_dbfs: -12.0,
            vu: -3.0,
            momentary_lufs: None,
        };
        assert_eq!(meter_value(MeterMode::Dbfs, wide), Some(-12.0));
        assert_eq!(meter_value(MeterMode::Vu, wide), Some(-3.0));
        assert_eq!(meter_value(MeterMode::Lufs, wide), None);
        assert_eq!(meter_number(meter_value(MeterMode::Lufs, wide), false), "—");
        let stereo = LaneMeterReadings {
            momentary_lufs: Some(-20.0),
            ..wide
        };
        assert_eq!(meter_value(MeterMode::Lufs, stereo), Some(-20.0));
    }

    #[test]
    fn clips_name_their_files_after_each_lane_source() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "Backtracking-named-clips-{}-{nonce}",
            std::process::id()
        ));
        let lane = || FrozenTrack::from_interleaved(48_000, 1, 10, &[0.0, 0.5, 1.0]).unwrap();
        let pair: Arc<[FrozenTrack]> = vec![lane(), lane()].into();
        let single: Arc<[FrozenTrack]> = vec![lane()].into();
        let named = |lane| ClipFormat {
            names: ["System audio".into(), "Vocal mic".into()],
            lane,
            ..ClipFormat::unnamed([ChannelRoute::Merged; 2], false)
        };
        let file_names = |snapshots: &Arc<[FrozenTrack]>, format| {
            // Not kept, so the files go away when the result is dropped.
            let lanes = &[0, 1][..snapshots.len()];
            let file = WavClip::new(
                Arc::clone(snapshots),
                lanes,
                None,
                Fades::NONE,
                folder.clone(),
                format,
                None,
            )
            .unwrap()
            .write(false)
            .unwrap();
            file.paths
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        let both = file_names(&pair, named(None));
        assert!(
            both[0].starts_with("Backtracking_System audio_A_2"),
            "{both:?}"
        );
        assert!(
            both[1].starts_with("Backtracking_Vocal mic_B_2"),
            "{both:?}"
        );
        // One lane of two keeps its letter.
        let lane_b = file_names(&pair, named(Some(1)));
        assert_eq!(lane_b.len(), 1);
        assert!(
            lane_b[0].starts_with("Backtracking_Vocal mic_B_2"),
            "{lane_b:?}"
        );
        // One source shows no letter, and has no second lane to pick.
        let one = file_names(&single, named(None));
        assert!(one[0].starts_with("Backtracking_System audio_2"), "{one:?}");
        assert!(
            WavClip::new(
                single,
                &[0],
                None,
                Fades::NONE,
                folder.clone(),
                named(Some(1)),
                None
            )
            .is_none()
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn two_lanes_split_the_plot_at_its_middle() {
        let lanes = [
            views_plan(0, &[WaveformView::Merged]),
            views_plan(1, &[WaveformView::Merged]),
        ];
        let audio = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(300.0, 80.0));
        let boxes = stack::lay_out(
            audio.height(),
            &lanes.each_ref().map(LanePlan::spec),
            &[0, 0],
        );
        let lane_at = |y: f32| {
            row_at_height(&lanes, &boxes, y - audio.top())
                .unwrap()
                .lane()
        };
        assert_eq!(lane_at(20.0), 0);
        assert_eq!(lane_at(59.9), 0);
        assert_eq!(lane_at(60.0), 1);
        assert_eq!(lane_at(100.0), 1);
        // Presses just outside the plot still land on the nearer lane.
        assert_eq!(lane_at(12.0), 0);
        assert_eq!(lane_at(104.0), 1);
        let lane_rect = |lane: usize| rows_rect(&lanes, &boxes, audio, &[lanes[lane].row(0)]);
        assert_eq!(
            lane_rect(0),
            Some(Rect::from_min_max(
                Pos2::new(10.0, 20.0),
                Pos2::new(310.0, 60.0)
            ))
        );
        assert_eq!(
            lane_rect(1),
            Some(Rect::from_min_max(
                Pos2::new(10.0, 60.0),
                Pos2::new(310.0, 100.0)
            ))
        );
    }

    const BED_714: [&str; 12] = [
        "L", "R", "C", "LFE", "Ls", "Rs", "Lrs", "Rrs", "Ltf", "Rtf", "Ltr", "Rtr",
    ];

    fn bed_group() -> LaneGroup {
        LaneGroup {
            id: "7.1.4".into(),
            positioned: false,
            label: "7.1.4".into(),
            channels: 0..12,
            labels: BED_714.iter().map(|&label| label.to_owned()).collect(),
        }
    }

    fn wide_plan(lane: usize, group: LaneGroup) -> LanePlan {
        LanePlan::stack(lane, group, Vec::new())
    }

    /// A wide lane as the window plans it: the pair on top, then the channels of `group` in
    /// `shown`.
    fn paired_plan(lane: usize, group: LaneGroup, shown: Vec<usize>) -> LanePlan {
        LanePlan {
            lane,
            kind: LaneKind::Stack {
                lead: RowSource::pair(lane).to_vec(),
                pair: PairGroup::new(group.channels.start, &group.labels),
                group,
                shown,
                groups: Vec::new(),
            },
        }
    }

    fn no_positions() -> [FrozenPositionTrack; 2] {
        std::array::from_fn(|_| backtracking_core::PositionTrack::new().freeze())
    }

    fn views_plan(lane: usize, views: &[WaveformView]) -> LanePlan {
        LanePlan {
            lane,
            kind: LaneKind::Views(views.to_vec()),
        }
    }

    /// A clip whose channel `n` holds `(n + 1) / 100` in every frame.
    fn numbered(channels: usize) -> FrozenTrack {
        FrozenTrack::from_interleaved(
            48_000,
            channels,
            100,
            &(0..8 * channels)
                .map(|sample| (sample % channels + 1) as f32 / 100.0)
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    /// One file a clip wrote: its name, its channel count and its samples, frame by frame.
    #[derive(Debug)]
    struct Written {
        name: String,
        channels: usize,
        samples: Vec<f32>,
    }

    /// Writes frames 101..105 of `snapshots` as a selection of `rows` and reads the files back.
    fn write_rows(
        snapshots: &Arc<[FrozenTrack]>,
        lanes: &[usize],
        rows: &[RowSource],
        groups: &[Vec<LaneGroup>; 2],
        fades: Fades,
        wide_mode: crate::preferences::WideExportMode,
    ) -> Vec<Written> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "Backtracking-row-clips-{}-{nonce}",
            std::process::id()
        ));
        let format = ClipFormat {
            names: ["Tone".into(), "Music".into()],
            ..ClipFormat::unnamed([ChannelRoute::Merged; 2], false)
        };
        let mut clip = WavClip::new(
            Arc::clone(snapshots),
            lanes,
            Some(&(101..105)),
            fades,
            folder.clone(),
            format,
            None,
        )
        .unwrap();
        // The group on show is the first, as it is on screen.
        clip.export_options.groups = std::array::from_fn(|index| {
            lanes
                .get(index)
                .and_then(|&lane| groups[lane].first().cloned())
        });
        clip.export_options.wide_mode = wide_mode;
        let files = clip.with_rows(rows, groups).unwrap().write(false).unwrap();
        let written = files
            .paths
            .iter()
            .map(|path| {
                let wav = std::fs::read(path).unwrap();
                let channels = usize::from(u16::from_le_bytes(wav[22..24].try_into().unwrap()));
                Written {
                    name: path.file_name().unwrap().to_string_lossy().into_owned(),
                    channels,
                    samples: wav[wav.len() - 4 * channels * 4..]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|&bytes| f32::from_le_bytes(bytes))
                        .collect(),
                }
            })
            .collect();
        // Not kept, so dropping the result takes the files away again.
        drop(files);
        let _ = std::fs::remove_dir_all(folder);
        written
    }

    #[test]
    fn a_row_drag_covers_its_start_row_to_the_row_under_the_pointer_and_back() {
        let lanes = [
            wide_plan(0, bed_group()),
            views_plan(1, &[WaveformView::Left, WaveformView::Right]),
        ];
        let a = |channel| RowSource::Channel(0, channel);
        let (b_left, b_right) = (
            RowSource::View(1, WaveformView::Left),
            RowSource::View(1, WaveformView::Right),
        );
        // Fourteen rows of 20 points: A's twelve, then B's L and R.
        let boxes = stack::lay_out(280.0, &lanes.each_ref().map(LanePlan::spec), &[0, 0]);
        let under = |y: f32| row_at_height(&lanes, &boxes, y).unwrap();
        let drag = |from: f32, to: f32| rows_between(&lanes, under(from), under(to));
        // A press in row 5 (Ls), moved down two rows, then up past where it started, then back.
        assert_eq!(drag(85.0, 85.0), [a(4)]);
        assert_eq!(drag(85.0, 125.0), [a(4), a(5), a(6)]);
        assert_eq!(drag(85.0, 45.0), [a(2), a(3), a(4)]);
        assert_eq!(drag(85.0, 81.0), [a(4)]);
        // On down into B: every row between, both lanes.
        let across = drag(85.0, 250.0);
        assert_eq!(across[..8], (4..12).map(a).collect::<Vec<_>>()[..]);
        assert_eq!(across[8..], [b_left]);
        // Past the bottom, or above the top, the nearest row counts.
        assert_eq!(drag(85.0, 400.0).last(), Some(&b_right));
        assert_eq!(drag(85.0, -10.0), (0..5).map(a).collect::<Vec<_>>());
        // A drag that starts in B and goes up into A.
        assert_eq!(drag(270.0, 230.0), [a(11), b_left, b_right]);
        assert_eq!(drag(270.0, 250.0), [b_left, b_right]);
        // Any row on its own is a range of one.
        for y in (0..14).map(|row| row as f32 * 20.0 + 10.0) {
            assert_eq!(drag(y, y).len(), 1, "{y}");
        }
    }

    #[test]
    fn rows_in_front_of_a_lanes_channels_are_selected_as_one() {
        // The L/R pair a wide lane draws above its channel rows.
        let lead = RowSource::pair(0).to_vec();
        let plan = paired_plan(0, bed_group(), (0..12).collect());
        let a = |channel| RowSource::Channel(0, channel);
        assert_eq!(plan.rows(), 14);
        assert_eq!(
            (plan.row(1), plan.row(2), plan.row(13)),
            (lead[1], a(0), a(11))
        );
        assert!(plan.group_channel(1).is_none());
        assert_eq!(plan.group_channel(2).map(|(_, channel)| channel), Some(0));
        let lanes = [plan];
        assert_eq!(rows_between(&lanes, lead[1], lead[1]), lead);
        assert_eq!(
            rows_between(&lanes, a(1), lead[1]),
            [lead[0], lead[1], a(0), a(1)]
        );
        assert_eq!(rows_between(&lanes, a(1), a(0)), [a(0), a(1)]);
    }

    /// The labels Windows gives a 7.1 device.
    fn windows_71() -> LaneGroup {
        LaneGroup {
            id: String::new(),
            positioned: false,
            label: String::new(),
            channels: 0..8,
            labels: ["FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs"]
                .map(str::to_owned)
                .to_vec(),
        }
    }

    #[test]
    fn the_pair_tops_every_wide_lane_at_any_height_with_or_without_hidden_rows() {
        let heights = [20.0, 47.0, 100.0, 167.0, 300.0, 559.0];
        // Mono and stereo lanes get no pair: their one or two rows split the height as before.
        for channels in [1usize, 2] {
            let views = if channels == 1 {
                vec![WaveformView::Merged]
            } else {
                vec![WaveformView::Left, WaveformView::Right]
            };
            let plan = views_plan(0, &views);
            assert!(plan.pair_group().is_none());
            for height in heights {
                let lane = stack::lay_out(height, &[plan.spec()], &[0])[0];
                assert_eq!((lane.rows, lane.shown), (channels, channels));
                assert!((lane.row_height - height / channels as f32).abs() < 0.001);
            }
        }
        for channels in [6usize, 12, 16, 40] {
            let group = numbered_group(channels);
            let every: Vec<usize> = (0..channels).collect();
            let some: Vec<usize> = (0..channels).step_by(3).collect();
            for shown in [every, some, Vec::new()] {
                let plan = paired_plan(0, group.clone(), shown.clone());
                let rows = 2 + shown.len();
                assert_eq!(plan.rows(), rows);
                assert!(plan.pair_group().is_some());
                assert_eq!(plan.row(0), RowSource::Pair(0, PairSide::Left));
                assert_eq!(plan.row(1), RowSource::Pair(0, PairSide::Right));
                assert!(plan.group_channel(1).is_none());
                for (index, &channel) in shown.iter().enumerate() {
                    assert_eq!(plan.row(2 + index), RowSource::Channel(0, channel));
                    assert_eq!(plan.group_channel(2 + index).unwrap().1, channel);
                }
                for height in heights {
                    // Every row that fits at the least height shows; the rest scroll.
                    let fits = ((height / stack::MIN_ROW_HEIGHT).floor() as usize).max(1);
                    let expected = if rows as f32 * stack::MIN_ROW_HEIGHT > height {
                        fits.min(rows)
                    } else {
                        rows
                    };
                    let lane = stack::lay_out(height, &[plan.spec()], &[0])[0];
                    assert_eq!(
                        (lane.rows, lane.shown, lane.first),
                        (rows, expected, 0),
                        "{channels} channels, {} shown, {height}",
                        shown.len()
                    );
                    assert!((lane.height - height).abs() < 0.001);
                    // The pair scrolls away with the lane, and the last row can be reached.
                    let last = stack::lay_out(height, &[plan.spec()], &[rows])[0];
                    assert_eq!(last.first + last.shown, rows);
                    // A mono lane B below keeps its row.
                    let both = [plan.clone(), views_plan(1, &[WaveformView::Merged])];
                    let boxes =
                        stack::lay_out(height, &both.each_ref().map(LanePlan::spec), &[0, 0]);
                    assert_eq!(boxes[1].shown, 1);
                    assert!(boxes[0].shown >= 1);
                    assert_eq!(
                        row_at_height(&both, &boxes, 0.0),
                        Some(RowSource::Pair(0, PairSide::Left))
                    );
                }
                // The window grows for the pair and the rows shown, never for hidden ones.
                assert_eq!(
                    stack::comfortable_height(&[plan.spec()]),
                    Some(rows as f32 * stack::COMFORTABLE_ROW_HEIGHT)
                );
            }
        }
        // A lane too wide for the pair (more than 64 channels) has no pair rows.
        assert!(pair_group(&[], 65).is_none());
        assert!(pair_group(&[], 64).is_some());
    }

    #[test]
    fn hidden_rows_follow_which_channels_are_active() {
        const NONE: [usize; 0] = [];
        let group = windows_71();
        // Hiding off: every row.
        assert_eq!(shown_channels(&group, None), (0..8).collect::<Vec<_>>());
        // Nothing heard yet, or only front left and right: the pair alone.
        assert_eq!(shown_channels(&group, Some(0)), NONE);
        assert_eq!(shown_channels(&group, Some(0b11)), NONE);
        assert_eq!(shown_channels(&group, Some(0b01)), NONE);
        // Anything beyond them: every active channel, front left and right included.
        assert_eq!(
            shown_channels(&group, Some(0xff)),
            (0..8).collect::<Vec<_>>()
        );
        assert_eq!(shown_channels(&group, Some(0b101)), [0, 2]);
        assert_eq!(shown_channels(&group, Some(0b1000)), [3]);
        assert_eq!(shown_channels(&group, Some(0b1100_0011)), [0, 1, 6, 7]);
        // Bits past the lane's channels mean nothing.
        assert_eq!(shown_channels(&group, Some(1 << 40)), NONE);
        // Numbered channels have no front pair of their own.
        let numbered = numbered_group(3);
        assert_eq!(shown_channels(&numbered, Some(0b11)), [0, 1]);
        // A group further into the lane counts its bits from the lane's first channel.
        let later = group_with(12..14, &["Object 1", "Object 2"]);
        assert_eq!(shown_channels(&later, Some(1 << 13)), [13]);
        assert_eq!(shown_channels(&later, Some(0b11)), NONE);
    }

    fn group_with(channels: Range<usize>, labels: &[&str]) -> LaneGroup {
        LaneGroup {
            id: "objects".into(),
            positioned: true,
            label: "Objects".into(),
            channels,
            labels: labels.iter().map(|&label| label.to_owned()).collect(),
        }
    }

    #[test]
    fn a_row_appears_at_once_and_goes_only_after_two_silent_answers() {
        let mut activity = ChannelActivity::default();
        assert_eq!(activity.shown, None);
        activity.answer(0b111);
        assert_eq!(activity.shown, Some(0b111));
        // One silent answer is not enough for channel 2 to lose its row; two are.
        activity.answer(0b011);
        assert_eq!(activity.shown, Some(0b111));
        activity.answer(0b011);
        assert_eq!(activity.shown, Some(0b011));
        // Back at once.
        activity.answer(0b111);
        assert_eq!(activity.shown, Some(0b111));
        // A channel that comes and goes every answer keeps its row.
        for _ in 0..10 {
            activity.answer(0b011);
            assert_eq!(activity.shown, Some(0b111));
            activity.answer(0b111);
            assert_eq!(activity.shown, Some(0b111));
        }
        // Everything gone quiet: after two answers only the pair is left.
        activity.answer(0);
        activity.answer(0);
        assert_eq!(activity.shown, Some(0));
        // The top channel of a 64-channel lane works like any other.
        activity.answer(1 << 63);
        assert_eq!(activity.shown, Some(1 << 63));
    }

    #[test]
    fn the_pair_is_one_unit_of_a_row_selection_and_is_kept_by_identity() {
        let group = windows_71();
        let pair = RowSource::pair(0);
        let a = |channel| RowSource::Channel(0, channel);
        // Rows of FC, Ls and Rs shown below the pair; the others are hidden.
        let lanes = [
            paired_plan(0, group.clone(), vec![2, 6, 7]),
            views_plan(1, &[WaveformView::Left, WaveformView::Right]),
        ];
        for side in pair {
            assert_eq!(rows_between(&lanes, side, side), pair);
            assert_eq!(clip_rows(&lanes, true, None, Some(side)), pair);
        }
        assert_eq!(
            rows_between(&lanes, pair[1], a(6)),
            [pair[0], pair[1], a(2), a(6)]
        );
        assert_eq!(
            rows_between(&lanes, a(7), pair[0]),
            [pair[0], pair[1], a(2), a(6), a(7)]
        );
        // Into lane B, and from B back up into the pair.
        let b_left = RowSource::View(1, WaveformView::Left);
        assert_eq!(rows_between(&lanes, a(7), b_left), [a(7), b_left]);
        assert_eq!(rows_between(&lanes, b_left, pair[1]).len(), 6);
        // A press on either pair row takes the pair.
        let boxes = stack::lay_out(140.0, &lanes.each_ref().map(LanePlan::spec), &[0, 0]);
        let under = |y: f32| row_at_height(&lanes, &boxes, y).unwrap();
        assert_eq!(rows_between(&lanes, under(5.0), under(5.0)), pair);
        assert_eq!(rows_between(&lanes, under(25.0), under(25.0)), pair);
        assert_eq!(under(45.0), a(2));
        // A hidden row stands for itself; when more rows show, the selection keeps its audio.
        assert_eq!(rows_between(&lanes, a(3), a(3)), [a(3)]);
        let more = [paired_plan(0, group, vec![2, 3, 6, 7])];
        let chosen = rows_between(&lanes, pair[0], a(6));
        assert_eq!(
            rows_between(&more, chosen[0], *chosen.last().unwrap()),
            [pair[0], pair[1], a(2), a(3), a(6)]
        );
        // What the rows write and play: the pair, and the lane-native channels.
        let export = chosen_rows(&[pair[0], pair[1], a(6), a(2)]).unwrap();
        assert_eq!(export.lane, Some(0));
        assert_eq!(export.pair, [true, false]);
        assert_eq!(export.native[0], Some(vec![2, 6]));
        let channels = chosen_rows(&[a(6), a(2)]).unwrap();
        assert_eq!(
            (channels.pair, channels.native[0].clone()),
            ([false, false], Some(vec![2, 6]))
        );
        let only_pair = chosen_rows(&pair).unwrap();
        assert_eq!(only_pair.native[0], Some(Vec::new()));
        let right = chosen_rows(&[RowSource::View(1, WaveformView::Right)]).unwrap();
        assert_eq!(
            (right.lane, right.native[1].clone()),
            (Some(1), Some(vec![1]))
        );
    }

    #[test]
    fn a_selection_names_the_pair_apart_from_channels_labelled_l_and_r() {
        // A lane whose first two channels are themselves called L and R, as an add-on's are.
        let name = |row: RowSource| match row {
            RowSource::Pair(_, PairSide::Left) | RowSource::Channel(_, 0) => "L".to_owned(),
            RowSource::Pair(_, PairSide::Right) | RowSource::Channel(_, 1) => "R".to_owned(),
            RowSource::Channel(_, channel) => format!("C{channel}"),
            RowSource::View(lane, _) => lane_name(lane).to_owned(),
        };
        let pair = RowSource::pair(0);
        let a = |channel| RowSource::Channel(0, channel);
        assert_eq!(rows_label(&pair, false, name), "L/R");
        assert_eq!(
            rows_label(&[pair[0], pair[1], a(0), a(1)], false, name),
            "L/R–R"
        );
        assert_eq!(rows_label(&[a(0), a(1)], false, name), "L–R");
        assert_eq!(rows_label(&[a(1)], false, name), "R");
        // With two sources the lane's letter comes first, once for rows of one lane.
        assert_eq!(rows_label(&pair, true, name), "A L/R");
        assert_eq!(
            rows_label(&[pair[0], pair[1], a(0), a(2)], true, name),
            "A L/R–C2"
        );
        let b = RowSource::pair(1);
        assert_eq!(rows_label(&[a(2), b[0], b[1]], true, name), "A C2–B L/R");
        assert_eq!(rows_label(&[], false, name), "");
    }

    #[test]
    fn the_frozen_edit_names_the_chosen_rows_by_snapshot_for_the_export() {
        let edit = |lanes: Vec<usize>| FrozenEdit {
            snapshots: lanes.iter().map(|_| tone(8)).collect::<Vec<_>>().into(),
            lanes,
            bars: Vec::new(),
            retention_seconds: 20.0,
            export_options: ExportOptions::default(),
            groups: [vec![windows_71()], Vec::new()],
            shown_groups: Default::default(),
            positions: no_positions(),
            activity: [None; 2],
            activity_job: None,
        };
        let pair = RowSource::pair(1);
        let rows = [
            pair[0],
            pair[1],
            RowSource::Channel(1, 5),
            RowSource::Channel(1, 3),
        ];
        // Snapshot 0 holds lane B here, as it does when only B records.
        let only_b = edit(vec![1]);
        let choice = only_b.row_choice(&rows);
        assert_eq!(
            choice[0],
            RowChoice {
                channels: Some(vec![3, 5]),
                pair: true
            }
        );
        assert_eq!(choice[1], RowChoice::default());
        let both = edit(vec![0, 1]);
        let choice = both.row_choice(&rows);
        assert_eq!(
            (choice[0].clone(), choice[1].pair),
            (RowChoice::default(), true)
        );
        // No rows chosen is every row of every lane.
        assert_eq!(
            both.row_choice(&[]),
            [RowChoice::default(), RowChoice::default()]
        );
    }

    #[test]
    fn the_length_label_never_covers_a_flag() {
        let audio = Rect::from_min_size(Pos2::new(28.0, 0.0), Vec2::new(470.0, 80.0));
        let full = Vec2::new(150.0, 13.0);
        let short = Vec2::new(50.0, 13.0);
        let flag =
            |x: f32| Rect::from_min_size(Pos2::new(x, 0.0), Vec2::new(7.0, 11.0)).expand(1.0);
        // Flags far from the corner: the full label at the top right.
        let (index, place) = length_label_place(audio, &[full, short], &[flag(60.0), flag(100.0)]);
        assert_eq!(
            (index, place),
            (0, Pos2::new(audio.right() - 8.0 - 150.0, 0.0))
        );
        // An end flag under the full label: the length alone.
        let (index, place) = length_label_place(audio, &[full, short], &[flag(60.0), flag(400.0)]);
        assert_eq!((index, place.y), (1, 0.0));
        // Under the short one too: the length alone at the top left.
        let end = audio.right() - 30.0;
        let (index, place) = length_label_place(audio, &[full, short], &[flag(200.0), flag(end)]);
        assert_eq!((index, place), (1, Pos2::new(audio.left() + 8.0, 0.0)));
        // Flags at both corners: the length alone below the flags.
        let flags = [flag(audio.left() + 2.0), flag(end)];
        let (index, place) = length_label_place(audio, &[full, short], &flags);
        let label = Rect::from_min_size(place, short);
        assert_eq!(index, 1);
        assert!(flags.iter().all(|flag| !flag.intersects(label)));
        assert!(audio.contains_rect(label));
        // A fade label has only one form.
        assert_eq!(length_label_place(audio, &[full], &[]).0, 0);
    }

    #[test]
    fn the_frozen_pair_rows_are_the_fold_down_of_their_group() {
        let track = numbered(12);
        let group = bed_group();
        let pair = PairGroup::new(0, &group.labels).unwrap();
        let key = EditPreviewKey {
            frames: 100..108,
            rows: vec![
                RowSource::Pair(0, PairSide::Left),
                RowSource::Pair(0, PairSide::Right),
                RowSource::Channel(0, 2),
            ],
            pairs: [Some(pair.clone()), None],
            bins: 4,
        };
        let peaks = edit_preview(std::slice::from_ref(&track), &[0], &key);
        for (row, side) in [PairSide::Left, PairSide::Right].into_iter().enumerate() {
            let (mut low, mut high) = (vec![0.0; 4], vec![0.0; 4]);
            track.pair_waveform(&pair, side, 100..108, &mut low, &mut high);
            assert_eq!((&peaks[row].low, &peaks[row].high), (&low, &high));
            assert!(high.iter().all(|&value| value > 0.0), "{side:?}");
        }
        assert_eq!(peaks[2].high, [0.03; 4]);
        // Without its group a pair row stays flat rather than show other audio.
        let lost = EditPreviewKey {
            pairs: [None, None],
            ..key
        };
        assert_eq!(edit_preview(&[track], &[0], &lost)[0].high, [0.0; 4]);
    }

    #[test]
    fn a_drag_past_the_edge_of_a_scrolling_lane_scrolls_it_a_row_at_a_time_then_moves_on() {
        let lanes = [
            wide_plan(0, bed_group()),
            views_plan(1, &[WaveformView::Merged]),
        ];
        let specs = lanes.each_ref().map(LanePlan::spec);
        // 70 points hold five rows of 14: four of A's twelve, then B's one at 56 to 70.
        let boxes = |first: usize| stack::lay_out(70.0, &specs, &[first, 0]);
        assert_eq!((boxes(0)[0].shown, boxes(0)[1].top), (4, 56.0));
        let a = |channel| RowSource::Channel(0, channel);
        let b = RowSource::View(1, WaveformView::Merged);
        let start = a(1);
        // The pointer goes down into B and stays there. A scrolls a row each step, and the
        // drag takes the row that comes into view, until A's last row is on screen.
        let (mut first, mut at, mut scrolls) = (0, start, 0);
        loop {
            match row_drag(&lanes, &boxes(first), at, 60.0).unwrap() {
                RowDrag::Scroll {
                    lane: 0,
                    down: true,
                } => {
                    first += 1;
                    let shown = boxes(first)[0];
                    at = lanes[0].row(shown.first + shown.shown - 1);
                    scrolls += 1;
                    assert_eq!(at, a(first + 3));
                    assert_eq!(
                        rows_between(&lanes, start, at),
                        (1..first + 4).map(a).collect::<Vec<_>>()
                    );
                }
                RowDrag::To(row) => {
                    at = row;
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!((scrolls, first, at), (8, 8, b));
        assert_eq!(rows_between(&lanes, start, at).len(), 12);
        // Back up above the plot: first to A's top row on screen, then A scrolls back up.
        let at = match row_drag(&lanes, &boxes(first), at, -5.0) {
            Some(RowDrag::To(row)) => row,
            other => panic!("{other:?}"),
        };
        assert_eq!(at, a(8));
        assert_eq!(
            row_drag(&lanes, &boxes(first), at, -5.0),
            Some(RowDrag::Scroll {
                lane: 0,
                down: false
            })
        );
        // Inside the lane nothing scrolls, and a lane at its end does not scroll further.
        assert_eq!(
            row_drag(&lanes, &boxes(first), at, 20.0),
            Some(RowDrag::To(a(9)))
        );
        assert_eq!(
            row_drag(&lanes, &boxes(0), a(0), -5.0),
            Some(RowDrag::To(a(0)))
        );
        // A lane with room for every row never scrolls.
        let tall = stack::lay_out(400.0, &specs, &[0, 0]);
        assert_eq!(row_drag(&lanes, &tall, a(11), 500.0), Some(RowDrag::To(b)));
    }

    #[test]
    fn selected_rows_stay_the_same_rows_across_scrolling_and_a_group_switch() {
        let objects = LaneGroup {
            id: "objects".into(),
            positioned: true,
            label: "Objects".into(),
            channels: 12..16,
            labels: (1..=4).map(|object| format!("Object {object}")).collect(),
        };
        let groups = [vec![bed_group(), objects.clone()], Vec::new()];
        let bed_lanes = [wide_plan(0, bed_group())];
        // Rows 3 to 5, as a drag from C to Ls picks them.
        let selected = rows_between(
            &bed_lanes,
            RowSource::Channel(0, 2),
            RowSource::Channel(0, 4),
        );
        assert_eq!(
            selected,
            (2..5)
                .map(|channel| RowSource::Channel(0, channel))
                .collect::<Vec<_>>()
        );
        // Four rows of 14 points on screen. Scrolling moves the highlight with the rows.
        let audio = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(300.0, 56.0));
        let spec = [bed_lanes[0].spec()];
        let drawn = |lanes: &[LanePlan], first: usize| {
            rows_rect(
                lanes,
                &stack::lay_out(56.0, &spec, &[first]),
                audio,
                &selected,
            )
            .map(|rect| (rect.top(), rect.bottom()))
        };
        assert_eq!(
            drawn(&bed_lanes, 0),
            Some((48.0, 76.0)),
            "rows 3 and 4 of rows 1 to 4"
        );
        assert_eq!(
            drawn(&bed_lanes, 2),
            Some((20.0, 62.0)),
            "all three at the top"
        );
        assert_eq!(drawn(&bed_lanes, 4), Some((20.0, 34.0)), "row 5 alone");
        assert_eq!(drawn(&bed_lanes, 5), None, "scrolled out, still selected");
        // Another group on show draws none of them; switching back draws them again.
        let object_lanes = [wide_plan(0, objects)];
        assert_eq!(drawn(&object_lanes, 0), None);
        assert_eq!(drawn(&bed_lanes, 2), Some((20.0, 62.0)));
        // And they write the same channels, named by their own group, whatever is on show.
        let snapshots: Arc<[FrozenTrack]> = vec![numbered(16)].into();
        let files = write_rows(
            &snapshots,
            &[0],
            &selected,
            &[vec![groups[0][1].clone(), groups[0][0].clone()], Vec::new()],
            Fades::NONE,
            crate::preferences::WideExportMode::MonoFiles,
        );
        let names: Vec<_> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        for (name, label) in names
            .iter()
            .zip(["-Ch03_C.wav", "-Ch04_LFE.wav", "-Ch05_Ls.wav"])
        {
            assert!(name.ends_with(label), "{names:?}");
        }
    }

    #[test]
    fn each_row_selection_writes_only_its_own_rows() {
        use crate::preferences::WideExportMode;

        let none: [Vec<LaneGroup>; 2] = [Vec::new(), Vec::new()];
        let mono_files = WideExportMode::MonoFiles;
        let shape = |files: &[Written]| {
            files
                .iter()
                .map(|file| {
                    (
                        file.channels,
                        file.samples[file.channels..2 * file.channels].to_vec(),
                    )
                })
                .collect::<Vec<_>>()
        };

        // Two stereo sources, both split into L and R.
        let split = [
            views_plan(0, &[WaveformView::Left, WaveformView::Right]),
            views_plan(1, &[WaveformView::Left, WaveformView::Right]),
        ];
        let view = |lane, view| RowSource::View(lane, view);
        let stereo: Arc<[FrozenTrack]> = vec![numbered(2), numbered(2)].into();
        let rows = |from, to| rows_between(&split, from, to);
        // R of A alone: one mono file, under A's own take name.
        let r = write_rows(
            &stereo,
            &[0, 1],
            &rows(view(0, WaveformView::Right), view(0, WaveformView::Right)),
            &none,
            Fades::NONE,
            mono_files,
        );
        assert_eq!(shape(&r), [(1, vec![0.02])]);
        assert!(
            r[0].name.starts_with("Backtracking_Tone_A_2") && !r[0].name.contains("-Ch"),
            "{r:?}"
        );
        // L and R of A: A's stereo file, as today.
        let a = write_rows(
            &stereo,
            &[0, 1],
            &rows(view(0, WaveformView::Left), view(0, WaveformView::Right)),
            &none,
            Fades::NONE,
            mono_files,
        );
        assert_eq!(shape(&a), [(2, vec![0.01, 0.02])]);
        assert!(a[0].name.starts_with("Backtracking_Tone_A_2"), "{a:?}");
        // From R of A to L of B: one side of each.
        let across = write_rows(
            &stereo,
            &[0, 1],
            &rows(view(0, WaveformView::Right), view(1, WaveformView::Left)),
            &none,
            Fades::NONE,
            mono_files,
        );
        assert_eq!(shape(&across), [(1, vec![0.02]), (1, vec![0.01])]);
        assert!(
            across[1].name.starts_with("Backtracking_Music_B_2"),
            "{across:?}"
        );
        // Every row, or none named: both stereo files, as today.
        let all = rows(view(0, WaveformView::Left), view(1, WaveformView::Right));
        assert_eq!(all.len(), 4);
        for rows in [&all[..], &[]] {
            let both = write_rows(&stereo, &[0, 1], rows, &none, Fades::NONE, mono_files);
            assert_eq!(shape(&both), [(2, vec![0.01, 0.02]), (2, vec![0.01, 0.02])]);
        }

        // One 7.1.4 lane: rows 3 to 5 write those three channels, in either wide mode.
        let bed = [vec![bed_group()], Vec::new()];
        let wide: Arc<[FrozenTrack]> = vec![numbered(12)].into();
        let lanes = [wide_plan(0, bed_group())];
        let rows_3_to_5 = rows_between(&lanes, RowSource::Channel(0, 2), RowSource::Channel(0, 4));
        let files = write_rows(&wide, &[0], &rows_3_to_5, &bed, Fades::NONE, mono_files);
        assert_eq!(
            shape(&files),
            [(1, vec![0.03]), (1, vec![0.04]), (1, vec![0.05])]
        );
        for (file, label) in files
            .iter()
            .zip(["-Ch03_C.wav", "-Ch04_LFE.wav", "-Ch05_Ls.wav"])
        {
            assert!(file.name.starts_with("Backtracking_Tone_2"), "{files:?}");
            assert!(file.name.ends_with(label), "{files:?}");
        }
        let one = write_rows(
            &wide,
            &[0],
            &rows_3_to_5,
            &bed,
            Fades::NONE,
            WideExportMode::Multichannel,
        );
        assert_eq!(shape(&one), [(3, vec![0.03, 0.04, 0.05])]);
        assert!(!one[0].name.contains("-Ch"), "{one:?}");

        // 7.1.4 in A above stereo in B, from row 10 of A down into B.
        let lanes = [
            wide_plan(0, bed_group()),
            views_plan(1, &[WaveformView::Merged]),
        ];
        let pair: Arc<[FrozenTrack]> = vec![numbered(12), numbered(2)].into();
        let crossing = rows_between(
            &lanes,
            RowSource::Channel(0, 9),
            view(1, WaveformView::Merged),
        );
        assert_eq!(crossing.len(), 4);
        let files = write_rows(&pair, &[0, 1], &crossing, &bed, Fades::NONE, mono_files);
        assert_eq!(
            shape(&files),
            [
                (1, vec![0.10]),
                (1, vec![0.11]),
                (1, vec![0.12]),
                (2, vec![0.01, 0.02])
            ]
        );
        for (file, label) in files
            .iter()
            .zip(["-Ch10_Rtf.wav", "-Ch11_Ltr.wav", "-Ch12_Rtr.wav"])
        {
            assert!(file.name.starts_with("Backtracking_Tone_A_2"), "{files:?}");
            assert!(file.name.ends_with(label), "{files:?}");
        }
        assert!(
            files[3].name.starts_with("Backtracking_Music_B_2") && !files[3].name.contains("-Ch"),
            "{files:?}"
        );
        // Rows of A alone leave B out; B alone leaves A out.
        let a_only = write_rows(
            &pair,
            &[0, 1],
            &crossing[..2],
            &bed,
            Fades::NONE,
            mono_files,
        );
        assert_eq!(shape(&a_only), [(1, vec![0.10]), (1, vec![0.11])]);
        let b_only = write_rows(
            &pair,
            &[0, 1],
            &crossing[3..],
            &bed,
            Fades::NONE,
            mono_files,
        );
        assert_eq!(shape(&b_only), [(2, vec![0.01, 0.02])]);
    }

    #[test]
    fn fades_ramp_only_the_selected_rows_and_their_handles_sit_on_those_rows() {
        let lanes = [wide_plan(0, bed_group())];
        let selected = rows_between(&lanes, RowSource::Channel(0, 2), RowSource::Channel(0, 4));
        let fades = Fades::new(2, 1);
        let wide: Arc<[FrozenTrack]> = vec![numbered(12)].into();
        let files = write_rows(
            &wide,
            &[0],
            &selected,
            &[vec![bed_group()], Vec::new()],
            fades,
            crate::preferences::WideExportMode::MonoFiles,
        );
        assert_eq!(files.len(), 3);
        for (file, level) in files.iter().zip([0.03, 0.04, 0.05]) {
            for (frame, &sample) in file.samples.iter().enumerate() {
                let expected = level * fades.gain(frame as u64, 4);
                assert!(
                    (sample - expected).abs() < 1e-6,
                    "{} {:?}",
                    file.name,
                    file.samples
                );
            }
            assert_eq!(file.samples[0], 0.0);
            assert!(file.samples[2] > 0.0);
        }
        // The flags stand on top of the rows, so a press there takes a fade handle and one at
        // the top of the plot, over rows 1 and 2, does not.
        let audio = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(400.0, 240.0));
        let boxes = stack::lay_out(audio.height(), &[lanes[0].spec()], &[0]);
        let area = rows_rect(&lanes, &boxes, audio, &selected).unwrap();
        assert_eq!((area.top(), area.bottom()), (40.0, 100.0));
        let visible = 0..400;
        let selection = 100..300;
        let at = |x: f32, y: f32| {
            fade_end_at(
                area,
                visible.clone(),
                &selection,
                Pos2::new(x, y),
                FLAG_REACH,
            )
        };
        assert_eq!(at(100.0, 44.0), Some(FadeEnd::In));
        assert_eq!(at(300.0, 44.0), Some(FadeEnd::Out));
        assert_eq!(at(100.0, 4.0), None);
        assert_eq!(
            fade_end_at(
                audio,
                visible,
                &selection,
                Pos2::new(100.0, 4.0),
                FLAG_REACH
            ),
            Some(FadeEnd::In),
            "with the mode off"
        );
    }

    #[test]
    fn a_drag_out_without_a_selection_takes_the_row_under_the_pointer() {
        let lanes = [
            wide_plan(0, bed_group()),
            views_plan(1, &[WaveformView::Left, WaveformView::Right]),
        ];
        let boxes = stack::lay_out(280.0, &lanes.each_ref().map(LanePlan::spec), &[0, 0]);
        let under = |y: f32| row_at_height(&lanes, &boxes, y);
        // A press on row 4 (LFE) with no selection writes LFE alone, for the whole capture.
        let rows = clip_rows(&lanes, true, None, under(70.0));
        assert_eq!(rows, [RowSource::Channel(0, 3)]);
        let pair: Arc<[FrozenTrack]> = vec![numbered(12), numbered(2)].into();
        let groups = [vec![bed_group()], Vec::new()];
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "Backtracking-drag-row-{}-{nonce}",
            std::process::id()
        ));
        let mut clip = WavClip::new(
            Arc::clone(&pair),
            &[0, 1],
            None,
            Fades::NONE,
            folder.clone(),
            ClipFormat::unnamed([ChannelRoute::Merged; 2], false),
            None,
        )
        .unwrap();
        // This test reads the row's own samples, so it asks for mono files.
        clip.export_options.wide_mode = crate::preferences::WideExportMode::MonoFiles;
        let files = clip
            .with_rows(&rows, &groups)
            .unwrap()
            .write(false)
            .unwrap();
        assert_eq!(files.paths.len(), 1);
        let name = files.paths[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(name.ends_with("-Ch04_LFE.wav"), "{name}");
        let wav = std::fs::read(&files.paths[0]).unwrap();
        // Every frame held: eight frames of one 32-bit channel.
        assert_eq!(
            u32::from_le_bytes(wav[wav.len() - 36..wav.len() - 32].try_into().unwrap()),
            32
        );
        drop(files);
        let _ = std::fs::remove_dir_all(folder);
        // R of B, the same way.
        assert_eq!(
            clip_rows(&lanes, true, None, under(270.0)),
            [RowSource::View(1, WaveformView::Right)]
        );
        // A selection's own rows win over the row the drag starts in; with the mode off, or
        // a selection of every row, every row is written as before.
        let selected = [RowSource::Channel(0, 5)];
        assert_eq!(
            clip_rows(&lanes, true, Some(&selected), under(70.0)),
            selected
        );
        assert_eq!(clip_rows(&lanes, true, Some(&[]), under(70.0)), []);
        assert_eq!(clip_rows(&lanes, false, None, under(70.0)), []);
        assert_eq!(clip_rows(&lanes, false, Some(&selected), under(70.0)), []);
    }

    #[test]
    fn file_names_use_the_name_given_to_the_device_or_its_own() {
        let chrome = CaptureSource::Application {
            path: r"C:\Apps\chrome.exe".into(),
            name: "chrome.exe".into(),
        };
        let sender = CaptureSource::Sender {
            id: "a-1".into(),
            name: format!("{}Drums", crate::controller::SENDER_MARK),
        };
        let mut names = SourceNames::default();
        assert_eq!(source_file_name(&names, &chrome), "chrome");
        assert_eq!(source_file_name(&names, &sender), "Drums");
        assert_eq!(
            source_file_name(&names, &CaptureSource::System),
            "System audio"
        );
        assert_eq!(
            source_file_name(&names, &CaptureSource::DefaultMicrophone),
            "Default microphone"
        );
        names.set(&chrome, "Browser");
        names.set(&CaptureSource::System, "???");
        assert_eq!(source_file_name(&names, &chrome), "Browser");
        // A name with nothing a file name can hold leaves the source its own.
        assert_eq!(
            source_file_name(&names, &CaptureSource::System),
            "System audio"
        );
    }

    fn tone(channels: usize) -> FrozenTrack {
        FrozenTrack::from_interleaved(48_000, channels, 100, &vec![0.25; channels * 8]).unwrap()
    }

    #[test]
    fn space_plays_mono_and_stereo_as_before_and_a_wide_lane_by_its_rows() {
        let routes = [ChannelRoute::Merged, ChannelRoute::Right];
        let objects = LaneGroup {
            id: "objects".into(),
            positioned: true,
            label: "Objects".into(),
            channels: 12..14,
            labels: vec!["Object 1".into(), "Object 2".into()],
        };
        let groups = [vec![bed_group(), objects.clone()], Vec::new()];
        let on_show = [None, None];
        let wide_and_stereo = [tone(14), tone(2)];
        // A stereo lane B is captured merged; Right is the route of a mono one.
        let merged = [ChannelRoute::Merged; 2];
        let plan = |rows: Option<&[RowSource]>| {
            preview_plan(&wide_and_stereo, &[0, 1], &groups, &on_show, merged, rows)
        };
        // Edited together: both lanes, A as its pair (the whole Channels group), B as before.
        let together = plan(None).unwrap();
        assert_eq!(
            together,
            [
                PreviewPart {
                    index: 0,
                    route: ChannelRoute::Merged,
                    group: PairGroup::new(0, &bed_group().labels),
                    channels: 0..12,
                    selected: None,
                    positions: false,
                },
                PreviewPart {
                    index: 1,
                    route: ChannelRoute::Merged,
                    group: None,
                    channels: 0..0,
                    selected: None,
                    positions: false,
                },
            ]
        );
        // The pair, alone or with channel rows, plays the whole Channels group, of A only.
        let pair = RowSource::pair(0);
        assert_eq!(plan(Some(&pair)).unwrap(), together[..1]);
        assert_eq!(
            plan(Some(&[pair[0], pair[1], RowSource::Channel(0, 2)])).unwrap(),
            together[..1]
        );
        // Channel rows play only their own channels, within their group.
        let rows = [RowSource::Channel(0, 4), RowSource::Channel(0, 2)];
        assert_eq!(
            plan(Some(&rows)).unwrap(),
            [PreviewPart {
                selected: Some(vec![2, 4]),
                ..together[0].clone()
            }]
        );
        // Object rows play their group, with its positions.
        let object_rows = [RowSource::Channel(0, 13)];
        assert_eq!(
            plan(Some(&object_rows)).unwrap(),
            [PreviewPart {
                index: 0,
                route: ChannelRoute::Merged,
                group: PairGroup::new(12, &objects.labels),
                channels: 12..14,
                selected: Some(vec![13]),
                positions: true,
            }]
        );
        // Rows of two groups at once play the pair.
        assert_eq!(
            plan(Some(&[RowSource::Channel(0, 2), RowSource::Channel(0, 12)])).unwrap(),
            together[..1]
        );
        // A lane chosen whole while Objects is on show plays every object, with positions.
        let objects_on_show = [Some("objects".to_owned()), None];
        let whole = preview_plan(
            &wide_and_stereo,
            &[0, 1],
            &groups,
            &objects_on_show,
            merged,
            None,
        )
        .unwrap();
        assert_eq!(
            (
                whole[0].channels.clone(),
                whole[0].selected.clone(),
                whole[0].positions
            ),
            (12..14, None, true)
        );
        // Only the R row of a split stereo lane: only R.
        let right = [RowSource::View(1, WaveformView::Right)];
        assert_eq!(
            plan(Some(&right)).unwrap(),
            [PreviewPart {
                selected: Some(vec![1]),
                ..together[1].clone()
            }]
        );
        // Both of its rows: the lane as it always played.
        assert_eq!(
            plan(Some(&[RowSource::View(1, WaveformView::Left), right[0]])).unwrap(),
            together[1..]
        );
        // Only lane B recording: the one snapshot is B's, with B's route.
        let only_b = [tone(1)];
        let played = preview_plan(&only_b, &[1], &groups, &on_show, routes, None).unwrap();
        assert_eq!(
            (played.len(), played[0].route, played[0].group.is_none()),
            (1, ChannelRoute::Right, true)
        );
        assert!(preview_plan(&only_b, &[1], &groups, &on_show, routes, Some(&pair)).is_err());
        assert!(preview_plan(&only_b, &[0, 1], &groups, &on_show, routes, None).is_err());
        // Every plan makes a preview the core accepts, in stereo and binaural; none is played.
        let model = Arc::new(HeadModel::new(48_000).unwrap());
        for (rows, model) in [
            (None, None),
            (Some(&pair[..]), None),
            (Some(&pair[..]), Some(Arc::clone(&model))),
            (Some(&rows[..]), None),
            (Some(&rows[..]), Some(Arc::clone(&model))),
            (Some(&object_rows[..]), None),
            (Some(&right[..]), None),
        ] {
            let parts = plan(rows).unwrap();
            let lanes: Vec<_> = parts
                .iter()
                .map(|part| PreviewLane {
                    track: &wide_and_stereo[part.index],
                    route: part.route,
                    group: part.group.as_ref(),
                    selected: part.selected.as_deref(),
                })
                .collect();
            assert!(
                Preview::with_positions(&lanes, 101..105, model, false, Fades::NONE, &[None; 2])
                    .is_some(),
                "{rows:?}"
            );
        }
        // Mono and stereo played whole still take the preview they always did.
        let narrow = [tone(2), tone(1)];
        let parts = preview_plan(
            &narrow,
            &[0, 1],
            &[Vec::new(), Vec::new()],
            &on_show,
            routes,
            None,
        )
        .unwrap();
        assert!(
            parts
                .iter()
                .all(|part| part.group.is_none() && part.selected.is_none())
        );
        assert!(Preview::new(&narrow, 101..105, &routes, false, Fades::NONE).is_some());
    }

    #[test]
    fn every_row_of_a_stacked_lane_is_its_own_and_a_lane_selected_whole_is_all_of_them() {
        let lanes = [
            wide_plan(0, bed_group()),
            views_plan(1, &[WaveformView::Merged]),
        ];
        let audio = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(300.0, 260.0));
        let boxes = stack::lay_out(
            audio.height(),
            &lanes.each_ref().map(LanePlan::spec),
            &[0, 0],
        );
        let at = |y: f32| row_at_height(&lanes, &boxes, y - audio.top()).unwrap();
        // Twelve rows of A take 240 of the 260 points; the middle of the plot is a row of A.
        assert_eq!(at(audio.center().y), RowSource::Channel(0, 6));
        assert_eq!(at(audio.top() + 239.0), RowSource::Channel(0, 11));
        assert_eq!(
            at(audio.top() + 241.0),
            RowSource::View(1, WaveformView::Merged)
        );
        assert_eq!(at(audio.top() - 4.0), RowSource::Channel(0, 0));
        assert_eq!(
            at(audio.bottom() + 4.0),
            RowSource::View(1, WaveformView::Merged)
        );
        let lane_b = rows_rect(&lanes, &boxes, audio, &[lanes[1].row(0)]).unwrap();
        assert_eq!(
            (lane_b.top(), lane_b.bottom()),
            (audio.top() + 240.0, audio.bottom())
        );
        let lane_a = rows_between(&lanes, lanes[0].row(0), lanes[0].row(11));
        assert_eq!(
            rows_rect(&lanes, &boxes, audio, &lane_a).unwrap().height(),
            240.0
        );
        // One lane on screen owns every height.
        let one = [views_plan(1, &[WaveformView::Merged])];
        let plain = stack::lay_out(80.0, &[one[0].spec()], &[0]);
        assert_eq!(row_at_height(&one, &plain, 1.0).unwrap().lane(), 1);
        assert_eq!(row_at_height(&one, &plain, 79.0).unwrap().lane(), 1);
    }

    #[test]
    fn separate_lanes_export_every_channel_of_a_wide_lane_or_only_the_other_lane() {
        use crate::controller::LaneGroup;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "Backtracking-wide-separate-{}-{nonce}",
            std::process::id()
        ));
        let pair: Arc<[FrozenTrack]> = vec![tone(12), tone(2)].into();
        let labels = [
            "L", "R", "C", "LFE", "Ls", "Rs", "Lrs", "Rrs", "Ltf", "Rtf", "Ltr", "Rtr",
        ];
        let bed = LaneGroup {
            id: "7.1.4".into(),
            positioned: false,
            label: "7.1.4".into(),
            channels: 0..12,
            labels: labels.iter().map(|&label| label.to_owned()).collect(),
        };
        let file_names = |lane| {
            let format = ClipFormat {
                names: ["Test tone".into(), "System audio".into()],
                lane,
                ..ClipFormat::unnamed([ChannelRoute::Merged; 2], false)
            };
            let mut clip = WavClip::new(
                Arc::clone(&pair),
                &[0, 1],
                Some(&(101..105)),
                Fades::new(1, 1),
                folder.clone(),
                format,
                None,
            )
            .unwrap();
            clip.export_options.groups = [Some(bed.clone()), None];
            clip.export_options.wide_mode = crate::preferences::WideExportMode::MonoFiles;
            let files = clip.write(false).unwrap();
            files
                .paths
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        // A selection of A covers all twelve of its rows: one file per channel, after A's
        // source and letter, then the channel; and last the lane's L/R file.
        let a = file_names(Some(0));
        assert_eq!(a.len(), 13, "{a:?}");
        assert!(a[12].ends_with("-Stereo.wav"), "{a:?}");
        for ((name, label), channel) in a.iter().zip(labels).zip(1..) {
            assert!(name.starts_with("Backtracking_Test tone_A_2"), "{name}");
            assert!(
                name.ends_with(&format!("-Ch{channel:02}_{label}.wav")),
                "{name}"
            );
        }
        // A selection of B leaves A out.
        let b = file_names(Some(1));
        assert_eq!(b.len(), 1, "{b:?}");
        assert!(
            b[0].starts_with("Backtracking_System audio_B_2") && !b[0].contains("-Ch"),
            "{b:?}"
        );
        // Together, both lanes are written.
        assert_eq!(file_names(None).len(), 14);
        std::fs::remove_dir_all(folder).unwrap();
    }

    fn objects_group(channels: Range<usize>) -> LaneGroup {
        LaneGroup {
            id: "objects".into(),
            positioned: true,
            label: "Objects".into(),
            labels: (1..=channels.len())
                .map(|object| format!("Object {object}"))
                .collect(),
            channels,
        }
    }

    #[test]
    fn rows_become_what_the_export_call_writes_of_each_lane() {
        let bed = bed_group();
        let objects = objects_group(12..14);
        let groups = [bed.clone(), objects.clone()];
        let choice = |channels: Option<Vec<usize>>, pair| RowChoice { channels, pair };
        let write = |channels, pair, shown: &LaneGroup, bed: &LaneGroup| LaneWrite {
            channels,
            pair,
            shown: Some(shown.clone()),
            bed: Some(bed.clone()),
        };
        // The whole lane: the group on show and the pair.
        assert_eq!(
            lane_write(14, &RowChoice::default(), &groups, Some(&bed), None),
            write(None, true, &bed, &bed)
        );
        // Rows of the bed, the pair alone, and the pair with rows.
        assert_eq!(
            lane_write(
                14,
                &choice(Some(vec![2, 4]), false),
                &groups,
                Some(&bed),
                None
            ),
            write(Some(vec![2, 4]), false, &bed, &bed)
        );
        assert_eq!(
            lane_write(
                14,
                &choice(Some(Vec::new()), true),
                &groups,
                Some(&bed),
                None
            ),
            write(Some(Vec::new()), true, &bed, &bed)
        );
        assert_eq!(
            lane_write(14, &choice(Some(vec![2]), true), &groups, Some(&bed), None),
            write(Some(vec![2]), true, &bed, &bed)
        );
        // Object rows are named by the Objects group, with the bed for the pair, even when the
        // bed is on show again; the Objects group written whole takes the bed along as well.
        assert_eq!(
            lane_write(14, &choice(Some(vec![12]), true), &groups, Some(&bed), None),
            write(Some(vec![12]), true, &objects, &bed)
        );
        assert_eq!(
            lane_write(14, &RowChoice::default(), &groups, Some(&objects), None),
            write(None, true, &objects, &bed)
        );
        // A lane of objects only makes its pair from every channel, numbered.
        let only = objects_group(0..4);
        assert_eq!(
            lane_write(
                4,
                &RowChoice::default(),
                std::slice::from_ref(&only),
                Some(&only),
                None
            ),
            write(None, true, &only, &numbered_group(4))
        );
        // A groupless lane names its channels by number and folds all of them.
        assert_eq!(
            lane_write(6, &RowChoice::default(), &[], None, None),
            LaneWrite {
                pair: true,
                ..LaneWrite::default()
            }
        );
        // Stereo and mono: one side alone, or the whole lane.
        assert_eq!(
            lane_write(2, &choice(Some(vec![1]), false), &[], None, None).channels,
            Some(vec![1])
        );
        assert_eq!(
            lane_write(2, &choice(Some(vec![0, 1]), false), &[], None, None),
            LaneWrite::default()
        );
        assert_eq!(
            lane_write(1, &choice(Some(vec![0]), false), &[], None, None),
            LaneWrite::default()
        );
        // A lane that hides silent rows, written whole, writes its active channels, or only
        // the pair while nothing but the front left and right is active.
        let system = LaneGroup {
            id: String::new(),
            positioned: false,
            label: String::new(),
            channels: 0..8,
            labels: SYSTEM_71.iter().map(|&label| label.to_owned()).collect(),
        };
        let whole = |active| {
            lane_write(
                8,
                &RowChoice::default(),
                std::slice::from_ref(&system),
                Some(&system),
                Some(active),
            )
        };
        assert_eq!(whole(0b11), write(Some(Vec::new()), true, &system, &system));
        assert_eq!(
            whole(0b111),
            write(Some(vec![0, 1, 2]), true, &system, &system)
        );
        assert_eq!(whole(0xff).channels, Some((0..8).collect()));
    }

    /// The channel labels a 7.1 system device gets from its channel mask.
    const SYSTEM_71: [&str; 8] = ["FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs"];

    /// What a clip wrote: the file names, after the take name, and whether objects without
    /// positions were written in mono.
    fn written_names(clip: WavClip) -> (Vec<String>, bool) {
        let folder = clip.output.clone();
        let files = clip.write(false).unwrap();
        let names = files
            .paths
            .iter()
            .map(|path| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                // From the length of the take on: `…_0s-Ch01_L.wav` reads `-Ch01_L.wav`.
                name.split_once("s-")
                    .map_or(name.clone(), |(_, rest)| format!("-{rest}"))
            })
            .collect();
        let mono = files.objects_in_mono;
        drop(files);
        let _ = std::fs::remove_dir_all(folder);
        (names, mono)
    }

    fn clip_folder(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "Backtracking-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn a_lane_b_save_and_swapped_lanes_write_lane_bs_rows_groups_and_positions() {
        use crate::preferences::WideExportMode;

        let objects = objects_group(12..14);
        let groups = [Vec::new(), vec![bed_group(), objects.clone()]];
        let mut positions = backtracking_core::PositionTrack::new();
        for frame in [0, 200] {
            positions.insert(
                12,
                backtracking_core::PositionKey {
                    frame,
                    position: backtracking_core::Position {
                        x: 0.5,
                        y: 0.5,
                        z: 0.0,
                        size: 0.0,
                        gain: 1.0,
                    },
                },
            );
        }
        // By logical lane: only lane B, the wide one, has positions.
        let tracks = [
            backtracking_core::PositionTrack::new().freeze(),
            positions.freeze(),
        ];
        // Lane B's snapshot second, as captured, or first, as when lane A started later.
        for swapped in [false, true] {
            let (snapshots, lanes): (Arc<[FrozenTrack]>, [usize; 2]) = if swapped {
                (vec![numbered(14), numbered(2)].into(), [1, 0])
            } else {
                (vec![numbered(2), numbered(14)].into(), [0, 1])
            };
            let clip = |rows: &[RowSource], mode| {
                let format = ClipFormat {
                    names: ["Music".into(), "Tone".into()],
                    ..ClipFormat::unnamed([ChannelRoute::Merged; 2], false)
                };
                let mut clip = WavClip::new(
                    Arc::clone(&snapshots),
                    &lanes,
                    Some(&(101..105)),
                    Fades::NONE,
                    clip_folder("lane-b-rows"),
                    format,
                    None,
                )
                .unwrap();
                // By snapshot: the bed is on show in lane B.
                clip.export_options.groups = lanes.map(|lane| (lane == 1).then(bed_group));
                clip.export_options.wide_mode = mode;
                clip.with_rows(rows, &groups)
                    .unwrap()
                    .with_positions(&tracks)
            };
            // The pair and C of lane B: B alone, its C and its pair.
            let (names, mono) = written_names(clip(
                &[
                    RowSource::Pair(1, PairSide::Left),
                    RowSource::Pair(1, PairSide::Right),
                    RowSource::Channel(1, 2),
                ],
                WideExportMode::MonoFiles,
            ));
            assert_eq!(names, ["-Ch03_C.wav", "-Stereo.wav"], "{swapped} {names:?}");
            assert!(!mono);
            // Both objects of B: the moving one binaural with its positions, the other mono.
            let (names, mono) = written_names(clip(
                &[RowSource::Channel(1, 12), RowSource::Channel(1, 13)],
                WideExportMode::BinauralFiles,
            ));
            assert_eq!(
                names,
                [
                    "-Ch13_Object 1_binaural.wav",
                    "-Ch14_Object 2.wav",
                    "-Positions.json"
                ],
                "{swapped} {names:?}"
            );
            assert!(mono);
            // Lane A alone: its stereo file only.
            let (names, _) = written_names(clip(
                &[RowSource::View(0, WaveformView::Merged)],
                WideExportMode::BinauralFiles,
            ));
            assert_eq!(names.len(), 1, "{swapped} {names:?}");
            assert!(!names[0].contains("-Ch"), "{swapped} {names:?}");
            // Every row of B chosen is B written whole: the bed and the pair.
            let whole = clip(&[RowSource::Channel(1, 0)], WideExportMode::MonoFiles)
                .whole_lanes([false, true]);
            let (names, _) = written_names(whole);
            assert_eq!(names.len(), 13, "{swapped} {names:?}");
            assert_eq!(names[12], "-Stereo.wav");
        }
    }

    /// A 7.1 system lane whose channels in `active` hold a tone and the rest silence.
    fn system_71(active: &[usize]) -> FrozenTrack {
        FrozenTrack::from_interleaved(
            48_000,
            8,
            100,
            &(0..8 * 8)
                .map(|sample| {
                    let channel = sample % 8;
                    if active.contains(&channel) {
                        0.25 * (channel + 1) as f32 / 8.0
                    } else {
                        0.0
                    }
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    #[test]
    fn a_system_lane_that_hides_silent_rows_saves_what_it_shows() {
        use crate::preferences::WideExportMode;

        let system = LaneGroup {
            id: String::new(),
            positioned: false,
            label: String::new(),
            channels: 0..8,
            labels: SYSTEM_71.iter().map(|&label| label.to_owned()).collect(),
        };
        let names = |track: FrozenTrack, hiding: bool, mode| {
            let format = ClipFormat {
                names: ["System audio".into(), String::new()],
                hide_silent: [hiding, false],
                ..ClipFormat::unnamed([ChannelRoute::AllChannels; 2], false)
            };
            let mut clip = WavClip::new(
                vec![track].into(),
                &[0],
                None,
                Fades::NONE,
                clip_folder("hide-silent"),
                format,
                None,
            )
            .unwrap();
            clip.export_options.groups = [Some(system.clone()), None];
            clip.export_options.wide_mode = mode;
            let clip = clip
                .with_rows(&[], &[vec![system.clone()], Vec::new()])
                .unwrap();
            written_names(clip).0
        };
        for mode in [WideExportMode::BinauralFiles, WideExportMode::Multichannel] {
            // Stereo content on a 7.1 device: the pair file alone.
            assert_eq!(
                names(system_71(&[0, 1]), true, mode),
                ["-Stereo.wav"],
                "{mode:?}"
            );
            // Every channel active: the mode's files of all eight, and the pair.
            let all = names(system_71(&(0..8).collect::<Vec<_>>()), true, mode);
            assert_eq!(
                all.len(),
                if mode == WideExportMode::Multichannel {
                    2
                } else {
                    9
                },
                "{all:?}"
            );
            assert_eq!(all.last().unwrap(), "-Stereo.wav");
            // With hiding off every channel is written, active or not.
            for active in [vec![0, 1], (0..8).collect()] {
                assert_eq!(
                    names(system_71(&active), false, mode),
                    all,
                    "{mode:?} {active:?}"
                );
            }
        }
        // Three active channels: those three, in native order, and the pair.
        assert_eq!(
            names(system_71(&[0, 1, 6]), true, WideExportMode::MonoFiles),
            [
                "-Ch01_FL.wav",
                "-Ch02_FR.wav",
                "-Ch07_Ls.wav",
                "-Stereo.wav"
            ]
        );
        let format = ClipFormat {
            hide_silent: [true, false],
            ..ClipFormat::unnamed([ChannelRoute::AllChannels; 2], false)
        };
        // One multichannel file of a subset holds only those channels.
        let mut clip = WavClip::new(
            vec![system_71(&[0, 1, 6])].into(),
            &[0],
            None,
            Fades::NONE,
            clip_folder("hide-silent-multichannel"),
            format,
            None,
        )
        .unwrap();
        clip.export_options.groups = [Some(system.clone()), None];
        clip.export_options.wide_mode = WideExportMode::Multichannel;
        let folder = clip.output.clone();
        let files = clip.write(false).unwrap();
        assert_eq!(files.paths.len(), 2);
        let wav = std::fs::read(&files.paths[0]).unwrap();
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 3);
        let last: Vec<f32> = wav[wav.len() - 12..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&bytes| f32::from_le_bytes(bytes))
            .collect();
        assert_eq!(last, [0.25 / 8.0, 0.5 / 8.0, 0.25 * 7.0 / 8.0]);
        drop(files);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_lane_that_does_not_hide_rows_saves_its_silent_channels_too() {
        use crate::preferences::WideExportMode;

        // An add-on lane never hides rows, so its clip format does not ask for it.
        let format = ClipFormat {
            names: ["Test tone".into(), String::new()],
            ..ClipFormat::unnamed([ChannelRoute::Merged; 2], false)
        };
        assert_eq!(format.hide_silent, [false, false]);
        let mut clip = WavClip::new(
            vec![system_71(&[0, 1])].into(),
            &[0],
            None,
            Fades::NONE,
            clip_folder("addon-silent"),
            format,
            None,
        )
        .unwrap();
        let bed = LaneGroup {
            id: "channels".into(),
            positioned: false,
            label: "Channels".into(),
            channels: 0..8,
            labels: SYSTEM_71.iter().map(|&label| label.to_owned()).collect(),
        };
        clip.export_options.groups = [Some(bed.clone()), None];
        clip.export_options.wide_mode = WideExportMode::MonoFiles;
        let clip = clip.with_rows(&[], &[vec![bed], Vec::new()]).unwrap();
        let (names, _) = written_names(clip);
        assert_eq!(names.len(), 9, "{names:?}");
    }

    #[test]
    fn every_row_of_a_lane_chosen_is_that_lane_whole() {
        let lanes = [
            paired_plan(0, bed_group(), vec![0, 1, 2]),
            views_plan(1, &[WaveformView::Left, WaveformView::Right]),
        ];
        let every_a = rows_between(&lanes, lanes[0].row(0), lanes[0].row(4));
        assert_eq!(lanes_chosen_whole(&lanes, &every_a), [true, false]);
        assert_eq!(lanes_chosen_whole(&lanes, &every_a[..4]), [false, false]);
        let across = rows_between(&lanes, lanes[0].row(3), lanes[1].row(1));
        assert_eq!(lanes_chosen_whole(&lanes, &across), [false, true]);
        assert_eq!(lanes_chosen_whole(&lanes, &[]), [false, false]);
        // A lane that shows only its pair, while its other rows are hidden, is whole when the
        // pair is chosen; the export then asks the audio which channels have sound.
        let pair_only = [paired_plan(0, bed_group(), Vec::new())];
        assert_eq!(
            lanes_chosen_whole(&pair_only, &RowSource::pair(0)),
            [true, false]
        );
    }

    fn system_group() -> LaneGroup {
        LaneGroup {
            id: String::new(),
            positioned: false,
            label: String::new(),
            channels: 0..8,
            labels: SYSTEM_71.iter().map(|&label| label.to_owned()).collect(),
        }
    }

    #[test]
    fn a_drag_keeps_the_rows_it_began_with_and_the_window_its_size_until_it_ends() {
        let system = system_group();
        // A 7.1 system lane that hides silent rows, as an activity answer plans it.
        let plan = |active: u64| {
            vec![paired_plan(
                0,
                system.clone(),
                shown_channels(&system, Some(active)),
            )]
        };
        let specs = |lanes: &[LanePlan]| lanes.iter().map(LanePlan::spec).collect::<Vec<_>>();
        // The drag begins on the pair and the front four channels. The window grew for them a
        // while ago, and the user has not resized it since.
        let began = plan(0b1111);
        let answered = plan(0b100_1111);
        assert_ne!(began, answered);
        let mut lanes = began.clone();
        let mut first_row = [0; 2];
        let now = Instant::now();
        let later = now + Duration::from_secs(2);
        let window = 300.0;
        let current = stack::comfortable_height(&specs(&began)).unwrap();
        let mut growth = Some(WindowGrowth {
            before: 144.0,
            after: window,
            settles: now,
        });
        // The frozen view's answer lands while the drag is in progress: Ls has sound too. Every
        // pass works it out, and every lane keeps the rows the drag began with, so the rows
        // under the pointer stay where they are and the window does not grow.
        for _ in 0..3 {
            assert_eq!(
                take_plans(&mut lanes, &mut first_row, answered.clone(), true),
                None
            );
            assert_eq!(lanes, began);
            let fit = fit_window(
                &specs(&lanes),
                current,
                window,
                &mut growth,
                later,
                false,
                true,
                || MAX_GROWN_WINDOW_HEIGHT,
            );
            assert_eq!(fit, WindowFit::Stay);
            assert_eq!(growth.map(|growth| growth.after), Some(window));
        }
        // The drag ends: the answer shows, and the window grows by the new row.
        assert_eq!(
            take_plans(&mut lanes, &mut first_row, answered.clone(), false),
            Some(began.clone())
        );
        assert_eq!(lanes, answered);
        assert_eq!(first_row, [0; 2]);
        let wanted = stack::comfortable_height(&specs(&answered)).unwrap();
        let fit = fit_window(
            &specs(&lanes),
            current,
            window,
            &mut growth,
            later,
            false,
            false,
            || MAX_GROWN_WINDOW_HEIGHT,
        );
        assert_eq!(fit, WindowFit::Grow(window + wanted - current));

        // A row that appeared just before the drag began waits for the window to settle from
        // growing. The drag holds that growth too, until it ends.
        let mut growth = Some(WindowGrowth {
            before: 144.0,
            after: window,
            settles: now + Duration::from_millis(500),
        });
        for (window_drag, gesture) in [(false, true), (true, false)] {
            let fit = fit_window(
                &specs(&answered),
                current,
                window,
                &mut growth,
                later,
                window_drag,
                gesture,
                || MAX_GROWN_WINDOW_HEIGHT,
            );
            assert_eq!(fit, WindowFit::Stay, "{window_drag} {gesture}");
            assert_eq!(growth.map(|growth| growth.after), Some(window));
        }
        let fit = fit_window(
            &specs(&answered),
            current,
            window,
            &mut growth,
            later,
            false,
            false,
            || MAX_GROWN_WINDOW_HEIGHT,
        );
        assert_eq!(fit, WindowFit::Grow(window + wanted - current));
    }

    #[test]
    fn a_lane_selected_whole_stays_whole_as_its_rows_appear_or_go() {
        use crate::preferences::WideExportMode;

        let system = system_group();
        let plan = |shown: Vec<usize>| vec![paired_plan(0, system.clone(), shown)];
        // What a save or drag-out of `rows` writes from `track`, a lane that hides silent rows.
        let saved = |track: FrozenTrack, rows: &[RowSource], whole: [bool; 2]| {
            let format = ClipFormat {
                names: ["System audio".into(), String::new()],
                hide_silent: [true, false],
                ..ClipFormat::unnamed([ChannelRoute::AllChannels; 2], false)
            };
            let mut clip = WavClip::new(
                vec![track].into(),
                &[0],
                None,
                Fades::NONE,
                clip_folder("whole-rows"),
                format,
                None,
            )
            .unwrap();
            clip.export_options.groups = [Some(system.clone()), None];
            clip.export_options.wide_mode = WideExportMode::MonoFiles;
            let clip = clip
                .with_rows(rows, &[vec![system.clone()], Vec::new()])
                .unwrap()
                .whole_lanes(whole);
            written_names(clip).0
        };

        // Every row is selected while the lane shows the pair, the centre and the LFE.
        let made = plan(vec![2, 3]);
        let every = rows_between(&made, made[0].row(0), made[0].row(3));
        let mut whole = lanes_chosen_whole(&made, &every);
        assert_eq!(whole, [true, false]);
        // Then Ls is found active and its row appears. The lane stays whole and covers it.
        let appeared = plan(vec![2, 3, 6]);
        let rows = carry_rows(&made, &appeared, &every, &mut whole);
        assert_eq!(whole, [true, false]);
        assert_eq!(
            rows,
            rows_between(&appeared, appeared[0].row(0), appeared[0].row(4))
        );
        // Saved whole: every active channel, Ls included, and the pair.
        assert_eq!(
            saved(system_71(&[2, 3, 6]), &rows, whole),
            [
                "-Ch03_FC.wav",
                "-Ch04_LFE.wav",
                "-Ch07_Ls.wav",
                "-Stereo.wav"
            ]
        );
        // A row that goes leaves the lane whole too.
        let went = plan(vec![2]);
        let rows = carry_rows(&appeared, &went, &rows, &mut whole);
        assert_eq!(whole, [true, false]);
        assert_eq!(rows, rows_between(&went, went[0].row(0), went[0].row(2)));

        // A selection of some rows keeps exactly those, even once they are all the lane shows:
        // the pair, FL and FR, after FC goes and with it every row but the pair.
        let made = plan(vec![0, 1, 2]);
        let some = rows_between(&made, made[0].row(0), made[0].row(3));
        let mut whole = lanes_chosen_whole(&made, &some);
        assert_eq!(whole, [false, false]);
        let pair_only = plan(Vec::new());
        let rows = carry_rows(&made, &pair_only, &some, &mut whole);
        assert_eq!(whole, [false, false]);
        assert_eq!(rows, some);
        assert_eq!(
            saved(system_71(&[0, 1]), &rows, whole),
            ["-Ch01_FL.wav", "-Ch02_FR.wav", "-Stereo.wav"]
        );

        // Another group on show keeps a lane held whole to the rows it held, as every
        // selection is kept across a group switch, and switching back does not widen it.
        let objects = objects_group(12..16);
        let bed = [wide_plan(0, bed_group())];
        let shown_objects = [wide_plan(0, objects)];
        let all_bed = rows_between(&bed, bed[0].row(0), bed[0].row(11));
        let mut whole = lanes_chosen_whole(&bed, &all_bed);
        assert_eq!(whole, [true, false]);
        let rows = carry_rows(&bed, &shown_objects, &all_bed, &mut whole);
        assert_eq!(whole, [false, false]);
        assert_eq!(rows, all_bed);
        let rows = carry_rows(&shown_objects, &bed, &rows, &mut whole);
        assert_eq!(whole, [false, false]);
        assert_eq!(rows, all_bed);
    }

    #[test]
    fn the_saved_message_counts_wav_and_positions_files_and_says_when_objects_are_mono() {
        let folder = PathBuf::from(r"D:\takes");
        let file = |names: &[&str], objects_in_mono| ExportedWav {
            paths: names.iter().map(|name| folder.join(name)).collect(),
            objects_in_mono,
            keep: true,
            flags: None,
        };
        let english = Language::English;
        assert_eq!(
            file(&["a.wav"], false).saved_message(english, true),
            r"Saved snapshot D:\takes\a.wav"
        );
        assert_eq!(
            file(&["a.wav", "b.wav", "c-Positions.json"], false).saved_message(english, false),
            r"Saved 2 WAV files and a positions file in D:\takes"
        );
        assert_eq!(
            file(&["a.wav", "b.wav", "c-Positions.json"], true).saved_message(english, true),
            r"Objects without positions were written in mono. Saved snapshot 2 WAV files and a positions file in D:\takes"
        );
        assert_eq!(
            file(&["a.wav", "b.wav"], true).saved_message(Language::TraditionalChinese, true),
            r"沒有位置資料的物件已匯出為單聲道。已儲存快照：2 個 WAV 檔，位於 D:\takes"
        );
    }
}

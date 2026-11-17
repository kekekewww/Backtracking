use std::collections::VecDeque;
use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::context_menu::{self, WindowCommand};
use crate::controller::{
    CaptureSource, Controller, EditSnapshot, LaneGroup, LaneMeterReadings, SourceSettings,
};
use crate::clip_export::{ExportOptions, export_clip};
use crate::file_drag;
use crate::preferences::{
    BORDER_WIDTHS, DEFAULT_ACCENT, Key, MeterMode, PreferenceWriter, Preferences, Settings,
    StatusMark, WideExportMode, serialize_sources, wav_sample_format_preference,
};
use crate::snapshot_hotkey::{self, SnapshotHotkey};
use crate::startup;
use crate::tray::{self, Tray};
use crate::updater::UpdateOptions;
use crate::window_order;
use backtracking_core::{
    AutoResumeDelay, ChannelRoute, DurationChoice, FrozenTrack, WavExportProfile, WaveformView,
    cleanup, waveform,
};
use backtracking_ui::{
    CENTER_LINE, FlagReach, LoneSample, OPTIONS_NAVIGATION_WIDTH, PLOT_BACKGROUND, RECORDING_DIM,
    RECORDING_LIT, TraceStyle, TransportGlyph, WAV_SAMPLE_FORMATS, WAV_SAMPLE_RATES,
    accent_color, add_system_font_fallbacks, configure_options_menu, configure_options_style,
    fit_frame_range, flag_handle_at, icon_button, meter_number, muted_accent, options_choice,
    options_control_width, options_field, options_group_heading, options_heading_text,
    options_list, options_menu_height, options_navigation_item, options_note, options_value,
    paint_guide_lines, paint_transport_glyph, paint_waveform_trace, remeasure_options_menu,
    selected_waveform_color, status_led, waveform_press,
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

mod addons_page;
mod harness;
mod stack;
#[cfg(test)]
mod test_profile;
mod text;

use addons_page::{AddonsPage, PageRequest};
use stack::LaneBox;
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
    let next = if part.is_zero() { Duration::from_secs(1) } else { part };
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
        by(waits.fine_edit.map(|since| {
            FINE_EDIT_SETTLE.saturating_sub(since) + Duration::from_millis(1)
        }));
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
    play: [Vec2::new(-3.5, -5.5), Vec2::new(5.0, 0.0), Vec2::new(-3.5, 5.5)],
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
const FLAG_REACH: FlagReach = FlagReach { above: 2.0, below: 13.0, radius: FLAG_SNAP_DISTANCE as f32 };
/// Alt is taken: dragging with it moves the window.
const ALT_SELECTS: bool = false;
const FLAG_COLOR: Color32 = Color32::from_rgb(255, 201, 109);
/// The selected waveform takes the accent's complement when it differs by at least this much.
const SELECTION_CONTRAST: u8 = 80;
const PREPARING_WAV: &str = "Preparing WAV…";

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
            .with_inner_size(initial_window_size(settings.waveform_only, test_size.as_deref()))
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
        if primary { "Backtracking" } else { "Backtracking Capture" },
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
            recorder.options_in_taskbar = show_options_on_start;
            if show_options_on_start {
                recorder.open_options(&context.egui_ctx);
            }
            // A test copy leaves the shortcut, the tray, the startup entry and updates to the
            // installed app.
            if primary && !test_copy {
                match SnapshotHotkey::start(context.egui_ctx.clone()) {
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
                    Err(error) => recorder.message = format!("Could not read startup setting: {error}"),
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
    let fallback = Vec2::new(610.0, if waveform_only { DEFAULT_WAVEFORM_HEIGHT } else { 144.0 });
    let Some((width, height)) = diagnostic_size.and_then(|size| size.split_once('x')) else { return fallback; };
    let (Ok(width), Ok(height)) = (width.parse::<f32>(), height.parse::<f32>()) else { return fallback; };
    let minimum = min_window_size(waveform_only);
    if width.is_finite() && height.is_finite()
        && (minimum.x..=4096.0).contains(&width) && (minimum.y..=2160.0).contains(&height)
    { Vec2::new(width, height) } else { fallback }
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
    meter_modes: [MeterMode; 2],
    split_waveform: bool,
    wav_profile: WavExportProfile,
    wav_force_mono: bool,
    wide_export: WideExportMode,
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
    flags: Option<FlagUpdate>,
    flag_order: u64,
    gesture: Option<Gesture>,
    window_drag: Option<WindowDrag>,
    export: Option<ExportJob>,
    snapshot_save: Option<JoinHandle<io::Result<ExportedWav>>>,
    pending_snapshot_saves: VecDeque<WavClip>,
    hotkey: Option<SnapshotHotkey>,
    hotkey_error: Option<String>,
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
    Select { anchor: u64, cursor: u64, origin_x: f32, moved: bool },
    Export { preparing: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum OptionsSection {
    Actions,
    Capture,
    Sources,
    Appearance,
    Export,
    Addons,
    Updates,
}

impl OptionsSection {
    const ALL: [(Self, &'static str); 7] = [
        // One short word each, so the list never wraps and a page is easy to name.
        (Self::Actions, "Actions"),
        (Self::Capture, "Capture"),
        (Self::Sources, "Sources"),
        (Self::Appearance, "Appearance"),
        (Self::Export, "Export"),
        (Self::Addons, "Add-ons"),
        (Self::Updates, "Updates"),
    ];

    fn title(self) -> &'static str {
        // The Add-ons page is the first one written in both languages (see `text`).
        if self == Self::Addons {
            return Language::current().addons_section();
        }
        Self::ALL.iter().find(|(section, _)| *section == self).unwrap().1
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

/// The recorder window's own keys. Held keys do not repeat a command.
fn window_shortcut(events: &[egui::Event]) -> Option<WindowCommand> {
    events.iter().find_map(|event| {
        let egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } = event else {
            return None;
        };
        match key {
            egui::Key::Space if modifiers.is_none() => Some(WindowCommand::ToggleCapture),
            egui::Key::Delete if modifiers.is_none() => Some(WindowCommand::Clear),
            _ => None,
        }
    })
}

fn history_label(value: DurationChoice) -> String {
    let seconds = value.seconds();
    if seconds <= 60 { format!("{seconds} seconds") }
    else { format!("{} minutes", seconds / 60) }
}

#[derive(Clone)]
struct WaveformPeaks {
    low: Vec<f32>,
    high: Vec<f32>,
}

impl WaveformPeaks {
    fn empty(bins: usize) -> Self {
        Self { low: vec![0.0; bins], high: vec![0.0; bins] }
    }
}

/// What one waveform row draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowSource {
    /// A mono or stereo lane, merged or one side, as such lanes always were.
    View(usize, WaveformView),
    /// One channel of a lane wider than two channels: the lane, then the channel within it.
    Channel(usize, usize),
}

impl RowSource {
    fn lane(self) -> usize {
        match self {
            Self::View(lane, _) | Self::Channel(lane, _) => lane,
        }
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
    /// `groups` names every group the lane can show, as (id, label), in the source's order.
    Stack {
        group: LaneGroup,
        groups: Vec<(String, String)>,
    },
}

impl LanePlan {
    fn stacked(&self) -> bool {
        matches!(self.kind, LaneKind::Stack { .. })
    }

    fn rows(&self) -> usize {
        match &self.kind {
            LaneKind::Views(views) => views.len(),
            LaneKind::Stack { group, .. } => group.channels.len(),
        }
    }

    fn row(&self, row: usize) -> RowSource {
        match &self.kind {
            LaneKind::Views(views) => RowSource::View(self.lane, views[row]),
            LaneKind::Stack { group, .. } => {
                RowSource::Channel(self.lane, group.channels.start + row)
            }
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
        .unwrap_or_else(|| LaneGroup {
            id: String::new(),
            label: String::new(),
            channels: 0..channels,
            labels: (1..=channels).map(|channel| channel.to_string()).collect(),
        });
    let choices = groups
        .iter()
        .map(|group| (group.id.clone(), group.label.clone()))
        .collect();
    (group, choices)
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
    /// Channel groups frozen with the audio, by logical lane.
    groups: [Vec<LaneGroup>; 2],
    shown_groups: [Option<String>; 2],
}

impl FrozenEdit {
    fn track(&self, lane: usize) -> Option<&FrozenTrack> {
        self.lanes
            .iter()
            .position(|&logical| logical == lane)
            .and_then(|index| self.snapshots.get(index))
    }

    fn frames(&self) -> Range<u64> {
        self.snapshots.first().map_or(0..0, FrozenTrack::frame_range)
    }
}

struct EditRequest {
    result: mpsc::Receiver<Option<EditSnapshot>>,
    generation: u64,
    retention_seconds: f32,
    saves: Vec<SnapshotSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditPreviewKey {
    frames: Range<u64>,
    /// The rows on screen, in drawing order.
    rows: Vec<RowSource>,
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
    output: PathBuf,
    profile: WavExportProfile,
    routes: [ChannelRoute; 2],
    force_mono: bool,
    flags: Option<FlagUpdate>,
    export_options: ExportOptions,
}

type RetiredValue = Box<dyn std::any::Any + Send>;

struct UiRetirement {
    sender: Option<mpsc::Sender<RetiredValue>>,
    worker: Option<JoinHandle<()>>,
}

impl UiRetirement {
    fn new() -> io::Result<Self> {
        let (sender, requests) = mpsc::channel::<RetiredValue>();
        let worker = thread::Builder::new().name("backtracking-selection-cleanup".into())
            .spawn(move || { for retired in requests { drop(retired); } })?;
        Ok(Self { sender: Some(sender), worker: Some(worker) })
    }

    fn retire<T: Send + 'static>(&self, value: T) -> Result<(), mpsc::SendError<RetiredValue>> {
        self.sender.as_ref().expect("cleanup sender lives until shutdown").send(Box::new(value))
    }
}

impl Drop for UiRetirement {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

impl SnapshotSpec {
    fn frozen_clip(mut self, snapshot: &EditSnapshot) -> Option<WavClip> {
        self.export_options.groups = snapshot_groups(snapshot);
        self.clip(Arc::clone(&snapshot.snapshots), &snapshot.lanes)
    }

    fn clip(mut self, snapshots: Arc<[FrozenTrack]>, lanes: &[usize]) -> Option<WavClip> {
        if lanes.len() != snapshots.len() || lanes.iter().any(|&lane| lane > 1) {
            return None;
        }
        if let Some(flags) = &mut self.flags {
            flags.frames = snapshots.first()?.frame_range();
        }
        WavClip::new(
            snapshots,
            self.selection.as_ref(),
            self.output,
            self.profile,
            physical_routes(self.routes, lanes),
            self.force_mono,
            self.flags,
        )
        .map(|mut clip| {
            clip.export_options = self.export_options;
            clip
        })
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
            }
            peaks
        })
        .collect()
}

struct FlagUpdate {
    generation: u64,
    order: u64,
    frames: Range<u64>,
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
        fraction = waveform::snap_to_visible_markers(fraction, visible.clone(), flags,
            guides, FLAG_SNAP_DISTANCE / width);
    }
    let length = visible.end.saturating_sub(visible.start);
    visible.start.saturating_add(((length as f64 * fraction.clamp(0.0, 1.0)).round() as u64).min(length))
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
    frames: Range<usize>,
    output: PathBuf,
    profile: WavExportProfile,
    routes: [ChannelRoute; 2],
    force_mono: bool,
    flags: Option<FlagUpdate>,
    export_options: ExportOptions,
}

impl WavClip {
    fn new(
        snapshots: Arc<[FrozenTrack]>,
        selection: Option<&Range<u64>>,
        output: PathBuf,
        profile: WavExportProfile,
        routes: [ChannelRoute; 2],
        force_mono: bool,
        flags: Option<FlagUpdate>,
    ) -> Option<Self> {
        if snapshots.len() > routes.len() {
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
            frames: start..end,
            output,
            profile,
            routes,
            force_mono,
            flags,
            export_options: ExportOptions::default(),
        })
    }

    fn write(self, keep: bool) -> io::Result<ExportedWav> {
        let paths = export_clip(
            &self.output,
            &self.snapshots,
            self.frames,
            self.profile,
            &self.routes[..self.snapshots.len()],
            self.force_mono,
            &self.export_options,
        )?;
        let mut result = ExportedWav {
            paths,
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
    paths: Vec<PathBuf>,
    keep: bool,
    flags: Option<FlagUpdate>,
}

type WavPayload = Arc<std::sync::Mutex<Option<WavClip>>>;

fn wav_job(clip: WavClip, keep: bool) -> (WavPayload, impl FnOnce() -> io::Result<ExportedWav> + Send + 'static) {
    // The caller keeps a recovery reference until spawning succeeds. If the OS
    // rejects a new thread, dropping its closure cannot free the large audio on UI.
    let payload = Arc::new(std::sync::Mutex::new(Some(clip)));
    let work = Arc::clone(&payload);
    (payload, move || {
        let clip = work.lock().unwrap_or_else(|error| error.into_inner())
            .take().expect("one WAV writer owns the payload");
        clip.write(keep)
    })
}

fn spawn_wav_write(name: &str, clip: WavClip, keep: bool)
    -> Result<JoinHandle<io::Result<ExportedWav>>, (io::Error, WavClip)>
{
    let (payload, job) = wav_job(clip, keep);
    match thread::Builder::new().name(name.into()).spawn(job) {
        Ok(worker) => Ok(worker),
        Err(error) => {
            let clip = payload.lock().unwrap_or_else(|error| error.into_inner())
                .take().expect("an unstarted WAV job still owns its payload");
            Err((error, clip))
        }
    }
}

impl ExportedWav {
    fn description(&self) -> String {
        match self.paths.as_slice() {
            [file] => file.display().to_string(),
            [first, ..] => format!(
                "{} WAV files in {}",
                self.paths.len(),
                first.parent().unwrap_or(first).display(),
            ),
            [] => String::new(),
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
            meter_modes: [settings.meter_mode, settings.meter_b_mode],
            split_waveform: settings.split_waveform,
            wav_profile: settings.wav_profile,
            wav_force_mono: settings.wav_force_mono,
            wide_export: settings.wide_export,
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
            flags: None,
            flag_order: 0,
            gesture: None,
            window_drag: None,
            export: None,
            snapshot_save: None,
            pending_snapshot_saves: VecDeque::new(),
            hotkey: None,
            hotkey_error: None,
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

    fn save_preference(&mut self, key: Key, value: impl AsRef<str>) {
        if let Err(error) = self.write_preference(key, value.as_ref()) {
            self.message = format!("Could not save settings: {error}");
        }
    }

    fn write_preference(&self, key: Key, value: &str) -> io::Result<()> {
        if self.primary {
            self.preference_writer.as_ref()
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
        if !self.options_in_taskbar { tray::suppress_taskbar(window); }
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

    fn current_snapshots(&self) -> Option<Arc<[FrozenTrack]>> {
        snapshot_source(self.editing.as_ref().map(|edit| &edit.snapshots),
            self.edit_request.is_some(), || None)
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

    fn accept_edit_snapshot(&mut self, snapshot: EditSnapshot, generation: u64) -> Option<EditSnapshot> {
        if edit_snapshot_matches(&snapshot, generation) { Some(snapshot) }
        else { self.retire(snapshot); None }
    }

    fn begin_editing(&mut self) {
        if self.editing.is_some() || self.edit_request.is_some() { return; }
        self.edit_request = Some(EditRequest {
            result: self.controller.request_edit_snapshot(),
            generation: self.generation,
            retention_seconds: self.plot_retention_seconds(),
            saves: Vec::new(),
        });
        self.live_revision = None;
        self.next_refresh = Instant::now();
    }

    fn discard_editing(&mut self) {
        if let Some(request) = self.edit_request.take() {
            if !request.saves.is_empty() {
                self.pending_edit_save_reads.push(request);
            } else { self.retire(request); }
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
        });
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
            for peaks in &mut self.peaks { peaks.low.fill(0.0); peaks.high.fill(0.0); }
            self.lane_first_row = [0; 2];
            self.scroll_drag = None;
        }
    }

    fn flag_update(&mut self, frames: Range<u64>) -> FlagUpdate {
        self.flag_order += 1;
        FlagUpdate {
            generation: self.generation,
            order: self.flag_order,
            frames,
        }
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

    fn background_color(&self, color: Color32) -> Color32 {
        let alpha = ((100 - u16::from(self.transparency)) * 255 + 50) / 100;
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha as u8)
    }

    fn border_inset(&self) -> f32 {
        if self.waveform_border { f32::from(self.waveform_border_width) } else { 0.0 }
    }

    fn invalidate_export(&mut self) {
        self.clip_revision = self.clip_revision.wrapping_add(1);
        self.exported = None;
        if self.message == PREPARING_WAV {
            self.message.clear();
        }
    }

    fn refresh_waveform(&mut self) {
        if self.edit_request.is_some() { return; }
        if self.editing.is_some() {
            self.refresh_edit_preview();
            return;
        }
        let revision = self.controller.revision();
        let now = Instant::now();
        let bins = self.display_policy(self.audio_rect(self.plot_rect), self.visible_frames.clone()).bins;
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
                        channels.push((lane, channel));
                        channel_traces.push(trace);
                    }
                    RowSource::View(lane, view) => {
                        views.push((lane, view));
                        view_traces.push(trace);
                    }
                }
            }
            // Only the channels on screen are read. A second lane drawn the old way reads
            // the same frames as the first.
            let mut zoom = self.zoom_frames.clone();
            let mut rendered = None;
            let mut busy = false;
            if !channels.is_empty() {
                match self.controller.try_render_channel_waveforms(
                    &channels,
                    zoom.clone(),
                    &mut channel_traces,
                ) {
                    Some(result) => {
                        zoom = Some(result.1.clone());
                        rendered = Some(result);
                    }
                    None => busy = true,
                }
            }
            if !busy && (!views.is_empty() || rendered.is_none()) {
                match self
                    .controller
                    .try_render_waveforms(&views, zoom, &mut view_traces)
                {
                    Some(result) => {
                        rendered.get_or_insert(result);
                    }
                    None => busy = true,
                }
            }
            rendered.filter(|_| !busy)
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
        self.gesture.is_some() || self.pan.is_some()
            || self.last_interaction.elapsed() < FINE_EDIT_SETTLE
    }

    /// Keeps a pinned window above other topmost windows. The check runs on a
    /// native timer, so it does not need a repaint.
    fn watch_pin(&mut self, frame: &eframe::Frame, now: Instant) {
        // A hidden window cannot be covered. Showing it runs a pass that re-arms this.
        let wanted = self.pinned
            && !self.options_open
            && !frame.winit_window().is_some_and(|window| window_is_hidden(window));
        if wanted && self.pin_watch.is_none() {
            self.pin_watch = frame.winit_window().and_then(|window| window_order::PinWatch::attach(window));
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
                || !self.pending_plugin_snapshots.is_empty(),
            // Add-on manager replies arrive without a repaint of their own.
            polling: self.source_list_worker.is_some() || self.addons_page.waiting(),
            tray_retry: if self.options_in_taskbar || self.test_copy {
                None
            } else if let Some(tray) = &self.tray {
                tray.retry_in()
            } else {
                Some(self.next_tray_attach.saturating_duration_since(now))
            },
            // A minimized window keeps its unsaved position until it is restored.
            position_save: self.window_position_changed.filter(|_| !hidden).map(|changed| {
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
                .filter(|_| self.status_mark == StatusMark::Always && self.controller.is_recording())
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
        waveform::DisplayPolicy::for_view(audio.width(), audio.height(),
            visible.end.saturating_sub(visible.start), self.sample_rate, self.is_fine_editing())
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
            bins: policy.bins,
        };
        if let Some(job) = &self.edit_preview {
            if !job.worker.is_finished() {
                return;
            }
        }
        if let Some(job) = self.edit_preview.take() {
            if let Ok(peaks) = job.worker.join() {
                if job.key == key {
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
                }
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
                    let (group, groups) = group_on_show(groups, shown, channels);
                    return LanePlan {
                        lane,
                        kind: LaneKind::Stack { group, groups },
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

    /// Works out this pass's lanes. A lane whose rows changed starts again at its first row.
    fn update_lanes(&mut self) {
        let lanes = self.lane_plans();
        if lanes != self.lanes {
            for plan in &lanes {
                let same = self
                    .lanes
                    .iter()
                    .find(|old| old.lane == plan.lane)
                    .is_some_and(|old| match (&old.kind, &plan.kind) {
                        (
                            LaneKind::Stack { group: old, .. },
                            LaneKind::Stack { group: new, .. },
                        ) => old.id == new.id && old.channels == new.channels,
                        _ => false,
                    });
                if !same {
                    self.lane_first_row[plan.lane] = 0;
                }
            }
            self.lanes = lanes;
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

    /// Grows the window once when a lane starts stacking rows, so they need not scroll, and
    /// puts it back when no lane stacks any more unless the user resized it meanwhile.
    fn fit_window_to_rows(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if !self.auto_grow || self.waveform_only || self.window_drag.is_some() {
            return;
        }
        let window = ctx.viewport_rect().size();
        let now = Instant::now();
        let specs: Vec<_> = self.lanes.iter().map(LanePlan::spec).collect();
        match (stack::comfortable_height(&specs), self.growth) {
            (Some(wanted), None) => {
                let current = self.timeline_rect(self.plot_rect).height();
                let screen = frame
                    .winit_window()
                    .and_then(|window| window.current_monitor())
                    .map(|monitor| monitor.size().height as f32 / ctx.pixels_per_point() * 0.85);
                let cap = screen.map_or(MAX_GROWN_WINDOW_HEIGHT, |screen| {
                    screen.min(MAX_GROWN_WINDOW_HEIGHT)
                });
                let target = (window.y + wanted - current).min(cap).max(window.y);
                self.growth = Some(WindowGrowth {
                    before: window.y,
                    after: target,
                    settles: now + Duration::from_secs(1),
                });
                if target > window.y + 0.5 {
                    let size = Vec2::new(window.x, target);
                    // A test copy stays where it was put, off every screen.
                    if !self.test_copy {
                        fit_expanded_window(ctx, frame, size);
                    }
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
                }
            }
            (None, Some(growth)) => {
                self.growth = None;
                let untouched = (window.y - growth.after).abs() < 1.0 || now < growth.settles;
                if untouched && growth.after > growth.before + 0.5 {
                    let size = Vec2::new(window.x, growth.before).max(min_window_size(false));
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
                }
            }
            _ => {}
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
        if !status_mark_shown(self.status_mark, indicator, self.controller.error().is_some()) {
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
        capture_problem(
            self.controller.error(),
            [0, 1].map(|lane| self.controller.source_status(lane)),
            self.dual_source(),
        )
    }

    /// The status line under the waveform in the full window.
    fn status_text(&self) -> String {
        if let Some(error) = self.controller.error() {
            return error.to_owned();
        }
        let paused = self.controller.is_paused();
        if self.edit_request.is_some() {
            "Preparing selection…".into()
        } else if self.editing.is_some() && self.export.is_none() && self.snapshot_save.is_none() {
            if paused { "Selection / Paused" } else { "Selection" }.into()
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
            &ui.painter_at(rect), rect, indicator, lit, problem.as_deref(),
        );

        let state = if indicator == CaptureIndicator::Paused {
            paused_status(self.auto_resume_remaining())
        } else {
            indicator.label().to_owned()
        };
        let status = compact_status_text(&state, problem.as_deref(), &self.message);
        let response = ui.interact(hover, egui::Id::unique("capture-status"), egui::Sense::hover());
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
        let command = context_menu::show(window, &context_menu::entries(self.menu_state()));
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
            Vec2::new(size.x, self.full_waveform_height + self.border_inset() * 2.0).max(minimum)
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
        let old_border_inset = self.border_inset();
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
            // A package dropped here opens its confirmation on the Add-ons page.
            if let Some(dropped) = addons_page::dropped_package(&dropped_paths(ui.ctx())) {
                self.review_addon(dropped);
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
        } else if self.waveform_only && self.border_inset() != old_border_inset {
            let size = ctx.viewport_rect().size();
            let height = size.y + (self.border_inset() - old_border_inset) * 2.0;
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(
                Vec2::new(size.x, height).max(min_window_size(true)),
            ));
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
                    if !self.primary && section == OptionsSection::Updates { continue; }
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
                .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(22, 8)))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(&self.message).truncate()).on_hover_text(&self.message);
                });
        }
        let action = egui::CentralPanel::default()
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
                    .show(ui, |ui| options_list(ui, |ui| {
                        match self.options_section {
                            OptionsSection::Actions => {
                                self.capture_action_options(ui);
                                ui.separator();
                                self.snapshot_options(ui);
                                ui.separator();
                                self.app_options(ui, frame)
                            }
                            OptionsSection::Capture => { self.capture_options(ui); None }
                            OptionsSection::Sources => { self.source_options(ui); None }
                            OptionsSection::Appearance => { self.appearance_options(ui); None }
                            OptionsSection::Export => {
                                self.wav_settings(ui);
                                ui.separator();
                                self.folder_options(ui)
                            }
                            OptionsSection::Addons => self.addon_options(ui),
                            OptionsSection::Updates => self.updates.show(ui).then_some(OptionsAction::Quit),
                        }
                    })).inner
            }).inner;
        action
    }

    fn snapshot_options(&mut self, ui: &mut egui::Ui) {
        options_group_heading(ui, "Snapshots");
        let (save, new_window) = ui.horizontal_wrapped(|ui| {
            (ui.button("Save snapshot").clicked(), ui.button("New capture window").clicked())
        }).inner;
        if save { self.save_snapshot(); }
        if new_window {
            let launched = std::env::current_exe().and_then(|exe| {
                Command::new(exe).arg("--new-capture-window").spawn()
            });
            if let Err(error) = launched {
                self.message = format!("Could not open capture window: {error}");
            }
        }
        if !self.primary { return; }
        options_field(ui, "Global shortcut", |ui| {
            options_value(ui, "Alt + the key left of 1");
            if let Some(error) = &self.hotkey_error {
                options_note(ui, format!("Could not register the shortcut: {error}"));
            }
        });
        if Instant::now() >= self.snapshot_target_scan {
            self.snapshot_targets = backtracking_core::discover_snapshot_targets();
            self.snapshot_target_scan = Instant::now() + Duration::from_secs(1);
        }
        let current = self.snapshot_target.as_deref().map_or("Desktop recorder", |id| {
            self.snapshot_targets.iter().find(|target| target.id == id)
                .map_or("Plugin unavailable", |target| target.name.as_str())
        }).to_owned();
        let mut target = self.snapshot_target.clone();
        options_field(ui, "Shortcut target", |ui| {
            let width = options_control_width(ui);
            egui::ComboBox::from_id_salt("snapshot-target")
                .width(width)
                .height(options_menu_height(ui, self.snapshot_targets.len() + 1))
                .selected_text(&current).truncate()
                .show_ui(ui, |ui| {
                    configure_options_menu(ui, width);
                    ui.selectable_value(&mut target, None, "Desktop recorder");
                    for plugin in &self.snapshot_targets {
                        let suffix = plugin.id.get(plugin.id.len().saturating_sub(6)..).unwrap_or(&plugin.id);
                        ui.selectable_value(&mut target, Some(plugin.id.clone()), format!("{} ({suffix})", plugin.name));
                    }
                }).response.on_hover_text(current);
        });
        if self.snapshot_target != target {
            self.save_preference(Key::SnapshotTarget, target.as_deref().unwrap_or(""));
            self.snapshot_target = target;
        }
    }

    fn capture_action_options(&mut self, ui: &mut egui::Ui) {
        options_group_heading(ui, "Capture controls");
        let can_toggle = self.gesture.is_none();
        let transport = if self.controller.error().is_some() { "Retry capture" }
            else if self.controller.is_paused() { "Play" } else { "Pause" };
        let has_selection = self.selection.is_some();
        let (toggle, cancel, clear) = ui.horizontal_wrapped(|ui| {
            (
                ui.add_enabled(can_toggle, egui::Button::new(transport)).clicked(),
                ui.add_enabled(has_selection, egui::Button::new("Cancel selection")).clicked(),
                ui.add_enabled(can_toggle, egui::Button::new("Clear capture")).clicked(),
            )
        }).inner;
        if toggle { self.toggle_capture(); }
        if cancel { self.cancel_selection(); }
        if clear { self.clear_capture(); }
    }

    fn capture_options(&mut self, ui: &mut egui::Ui) {
        options_field(ui, "Retain the latest audio", |ui| {
            let mut retention = self.controller.retention();
            if options_choice(ui, "history", &mut retention,
                DurationChoice::ALL.into_iter().map(|value| (value, history_label(value)))) {
                self.controller.set_retention(retention);
                self.invalidate_export();
                self.live_revision = None;
                self.next_refresh = Instant::now();
                self.save_preference(Key::History, retention.seconds().to_string());
            }
        });
        // Wide lanes fill memory sooner; say how much of the setting is really kept.
        let requested = f64::from(self.controller.retention().seconds());
        if let Some(effective) = self.controller.effective_retention_seconds()
            && effective.is_finite()
            && effective < requested - 0.5
        {
            let channels: usize = self
                .controller
                .active_lanes()
                .iter()
                .filter_map(|&lane| self.controller.track_channels(lane))
                .sum();
            options_note(ui, Language::current().shorter_history(channels, effective));
        }
        let mut retain_silence = self.controller.retain_silence();
        if ui.checkbox(&mut retain_silence, "Keep quiet sections").changed() {
            self.controller.set_retain_silence(retain_silence);
            self.save_preference(Key::RetainSilence, u8::from(retain_silence).to_string());
        }
        options_field(ui, "Auto-resume delay", |ui| {
            if options_choice(ui, "auto-resume", &mut self.idle_resume,
                AutoResumeDelay::ALL.into_iter().map(|value| (value, auto_resume_label(value)))) {
                self.save_preference(Key::IdleResume, self.idle_resume.seconds().to_string());
            }
        });
    }

    fn appearance_options(&mut self, ui: &mut egui::Ui) {
        options_group_heading(ui, "Window");
        ui.checkbox(&mut self.waveform_only, "Show waveform only");
        ui.checkbox(&mut self.pinned, "Keep on top");
        options_field(ui, "Background transparency", |ui| {
            if options_choice(ui, "transparency", &mut self.transparency,
                TRANSPARENCIES.into_iter().map(|value| (value, format!("{value}%")))) {
                self.save_preference(Key::Transparency, self.transparency.to_string());
            }
        });
        self.accent_settings(ui);
        ui.separator();
        options_group_heading(ui, "Show");
        if ui.checkbox(&mut self.show_title, "App name in the toolbar").changed() {
            self.save_preference(Key::ShowTitle, u8::from(self.show_title).to_string());
        }
        if ui.checkbox(&mut self.show_meters, "Level meters").changed() {
            self.save_preference(Key::ShowMeters, u8::from(self.show_meters).to_string());
        }
        options_field(ui, "Capture status mark", |ui| {
            if options_choice(ui, "status-mark", &mut self.status_mark,
                StatusMark::ALL.into_iter().map(|mark| (mark, mark.label().to_owned()))) {
                self.save_preference(Key::StatusMark, self.status_mark.preference());
            }
        });
        ui.separator();
        options_group_heading(ui, "Timeline guides");
        if ui.checkbox(&mut self.bar_lines, "Show bar lines").changed() {
            self.save_preference(Key::BarLines, u8::from(self.bar_lines).to_string());
        }
        if ui.checkbox(&mut self.second_lines, "Show seconds lines").changed() {
            self.save_preference(Key::SecondsLines, u8::from(self.second_lines).to_string());
        }
        options_field(ui, "Seconds interval", |ui| {
            ui.add_enabled_ui(self.second_lines, |ui| {
                if options_choice(ui, "seconds-interval", &mut self.second_interval,
                    [1, 5, 10].into_iter().map(|value| {
                        (value, if value == 1 { "1 second".to_owned() } else { format!("{value} seconds") })
                    })) {
                    self.save_preference(Key::SecondsInterval, self.second_interval.to_string());
                }
            });
        });
        ui.separator();
        options_group_heading(ui, "Waveform border");
        if ui.checkbox(&mut self.waveform_border, "Show a border").changed() {
            self.save_preference(Key::WaveformBorder, u8::from(self.waveform_border).to_string());
        }
        ui.add_enabled_ui(self.waveform_border, |ui| {
            options_field(ui, "Border width", |ui| {
                if options_choice(ui, "border-width", &mut self.waveform_border_width,
                    BORDER_WIDTHS.into_iter().map(|width| (width, format!("{width} px")))) {
                    self.save_preference(Key::WaveformBorderWidth, self.waveform_border_width.to_string());
                }
            });
            let mut theme = self.waveform_border_color.is_none();
            let theme_changed = ui.checkbox(&mut theme, "Use theme color").changed();
            let [r, g, b, _] = self.waveform_border_color.unwrap_or(self.accent).to_array();
            let mut color = [r, g, b];
            let color_changed = options_field(ui, "Custom border color", |ui| {
                ui.add_enabled_ui(!theme, |ui| ui.color_edit_button_srgb(&mut color)).inner.changed()
            });
            if theme_changed || color_changed {
                self.waveform_border_color = (!theme).then(|| accent_color(color));
                let [r, g, b] = color;
                self.save_preference(Key::WaveformBorderColor,
                    if theme { String::new() } else { format!("#{r:02x}{g:02x}{b:02x}") });
            }
            if ui.checkbox(&mut self.waveform_border_follows_background, "Follow background opacity").changed() {
                self.save_preference(Key::WaveformBorderFollowsBackground,
                    u8::from(self.waveform_border_follows_background).to_string());
            }
        });
        ui.separator();
        options_group_heading(ui, "Display");
        options_field(ui, "Single-source waveform", |ui| {
            ui.add_enabled_ui(!self.dual_source(), |ui| {
                if options_choice(ui, "waveform-layout", &mut self.split_waveform,
                    [(false, "Merged".to_owned()), (true, "Left / Right".to_owned())]) {
                    self.save_preference(Key::SplitWaveform, if self.split_waveform { "1" } else { "0" });
                    self.live_revision = None;
                    self.next_refresh = Instant::now();
                }
            });
        });
        let dual = self.dual_source();
        for lane in 0..if dual { 2 } else { 1 } {
            let label = if dual { format!("Meter {}", lane_name(lane)) } else { "Meter".to_owned() };
            ui.push_id(lane, |ui| options_field(ui, &label, |ui| {
                if options_choice(ui, "meter", &mut self.meter_modes[lane],
                    MeterMode::OPTIONS.into_iter().map(|mode| (mode, mode.unit().to_owned()))) {
                    self.save_preference(if lane == 0 { Key::Meter } else { Key::MeterB }, self.meter_modes[lane].unit());
                }
            }));
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

    fn source_options(&mut self, ui: &mut egui::Ui) {
        // The list opened on a spinner and was measured for that; give the scanned rows room.
        if std::mem::take(&mut self.source_menu_outdated) && let Some(menu) = self.source_menu {
            remeasure_options_menu(ui.ctx(), menu);
        }
        let choices = self.available_sources.as_slice();
        let scanning = self.source_list_worker.is_some();
        let mut source_menu_opened = false;
        let mut open_menu = None;
        let mut sources = self.controller.sources().clone();
        ui.add_enabled_ui(self.gesture.is_none(), |ui| {
            for (lane, settings) in sources.iter_mut().enumerate() {
                ui.push_id(lane, |ui| {
                    options_group_heading(ui, &format!("Source {}", lane_name(lane)));
                    let configured = settings.clone();
                    let mut source = configured.as_ref().map(|settings| settings.source.clone());
                    options_field(ui, "Audio source", |ui| {
                        let source_menu = "source";
                        let was_open = egui::ComboBox::is_open(ui.ctx(), ui.make_persistent_id(egui::IdSalt::new(source_menu)));
                        let label = source.as_ref().map_or_else(
                            || std::borrow::Cow::Borrowed("Off"),
                            |source| source_picker_label(choices.iter().find(|choice| choice.same_source(source)).unwrap_or(source), choices),
                        ).into_owned();
                        let width = options_control_width(ui);
                        egui::ComboBox::from_id_salt(source_menu)
                            .selected_text(&label).width(width)
                            .height(options_menu_height(ui, choices.len() + usize::from(lane == 1)))
                            .truncate().show_ui(ui, |ui| {
                                configure_options_menu(ui, width);
                                open_menu = Some(ui.layer_id());
                                if lane == 1 { ui.selectable_value(&mut source, None, "Off"); }
                                source_menu_opened |= !was_open;
                                if !was_open || scanning { ui.spinner(); return; }
                                for choice in choices {
                                    let selected = source.as_ref().is_some_and(|source| choice.same_source(source));
                                    let label = source_picker_label(choice, choices);
                                    let row = ui.selectable_label(selected, label.as_ref());
                                    if row.clicked() && !selected { source = Some(choice.clone()); }
                                    match choice {
                                        CaptureSource::Application { path, .. } => { row.on_hover_text(path); }
                                        CaptureSource::Sender { id, .. } => { row.on_hover_text(format!("Sender instance {id}")); }
                                        _ => {}
                                    }
                                }
                            }).response.on_hover_text(label);
                    });
                    let same_source = source.as_ref().zip(configured.as_ref())
                        .is_some_and(|(source, configured)| source.same_source(&configured.source));
                    *settings = source.map(|source| SourceSettings {
                        source,
                        route: configured.filter(|_| same_source).map_or(ChannelRoute::Merged, |settings| settings.route),
                    });
                    if let Some(settings) = settings {
                        options_field(ui, "Channel route", |ui| {
                            let width = options_control_width(ui);
                            egui::ComboBox::from_id_salt("route")
                                .selected_text(settings.route.label()).width(width)
                                .height(options_menu_height(ui, ChannelRoute::ALL.len()))
                                .truncate().show_ui(ui, |ui| {
                                    configure_options_menu(ui, width);
                                    for route in ChannelRoute::ALL {
                                        let enabled = same_source && self.controller.route_available(lane, route);
                                        ui.add_enabled_ui(enabled, |ui| { ui.selectable_value(&mut settings.route, route, route.label()); });
                                    }
                                });
                        });
                    }
                    if let Some(status) = self.controller.source_status(lane) { options_note(ui, status); }
                    if lane == 0 { ui.separator(); }
                });
            }
        });
        self.source_menu = open_menu;
        if source_menu_opened { self.refresh_source_list(); }
        if sources != *self.controller.sources() {
            self.save_preference(Key::CaptureSources, serialize_sources(&sources));
            self.controller.set_sources(sources);
            self.sync_generation();
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
                self.exported.as_ref().map_or_else(Vec::new, |file| file.paths.clone()),
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
                if select { Some(OptionsAction::SelectSaveFolder) } else { open.then_some(OptionsAction::GoToSaveFolder) }
            }).inner
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
                if select { Some(OptionsAction::SelectSnapshotFolder) } else { open.then_some(OptionsAction::OpenSnapshotFolder) }
            }).inner
        });
        waveform.or(snapshot)
    }

    fn app_options(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) -> Option<OptionsAction> {
        options_group_heading(ui, "Application");
        let mut enabled = self.launch_at_login;
        if self.primary && ui.checkbox(&mut enabled, "Start with Windows").changed() {
            self.set_launch_at_login(enabled, frame);
        }
        let visible = frame.winit_window().is_none_or(|window| !window_is_hidden(window));
        let tray_available = self.tray_available;
        ui.horizontal_wrapped(|ui| {
            let visibility = if visible {
                ui.add_enabled(tray_available, egui::Button::new("Hide waveform")).clicked().then_some(OptionsAction::Hide)
            } else { ui.button("Show waveform").clicked().then_some(OptionsAction::Show) };
            let quit = ui.button("Quit Backtracking").clicked();
            visibility.or_else(|| quit.then_some(OptionsAction::Quit))
        }).inner
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
            options_choice(ui, "wav-rate", &mut self.wav_profile.sample_rate,
                WAV_SAMPLE_RATES.into_iter().map(|(rate, label)| (rate, label.to_owned())));
        });
        options_field(ui, "WAV encoding", |ui| {
            options_choice(ui, "wav-encoding", &mut self.wav_profile.sample_format,
                WAV_SAMPLE_FORMATS.into_iter().map(|(format, label)| (format, label.to_owned())));
        });
        let mono_changed = ui.checkbox(&mut self.wav_force_mono, "Export every file as mono").changed();
        let language = Language::current();
        options_field(ui, language.wide_export_field(), |ui| {
            if options_choice(
                ui,
                "wide-export",
                &mut self.wide_export,
                WideExportMode::ALL.into_iter().map(|mode| {
                    (
                        mode,
                        language
                            .wide_export_mode(mode == WideExportMode::Multichannel)
                            .to_owned(),
                    )
                }),
            ) {
                self.save_preference(Key::WideExport, self.wide_export.preference());
                self.invalidate_export();
            }
        });
        options_note(ui, language.wide_export_note());
        if self.wav_profile != previous || mono_changed {
            self.invalidate_export();
            self.message.clear();
            if mono_changed { self.save_preference(Key::WavForceMono, if self.wav_force_mono { "1" } else { "0" }); }
            if self.wav_profile.sample_rate != previous.sample_rate {
                self.save_preference(Key::WavSampleRate, self.wav_profile.sample_rate.to_string());
            }
            if self.wav_profile.sample_format != previous.sample_format {
                self.save_preference(Key::WavSampleFormat, wav_sample_format_preference(self.wav_profile.sample_format));
            }
        }
    }

    fn accent_settings(&mut self, ui: &mut egui::Ui) {
        options_field(ui, "Theme color", |ui| {
            let [r, g, b, _] = self.accent.to_array();
            let mut color = [r, g, b];
            let (changed, reset) = ui.horizontal(|ui| {
                (ui.color_edit_button_srgb(&mut color).changed(), ui.button("Reset").clicked())
            }).inner;
            let mut accent = accent_color(color);
            if reset { accent = accent_color(DEFAULT_ACCENT); }
            if (changed || reset) && accent != self.accent {
                self.accent = accent;
                apply_accent(ui.ctx(), accent);
            }
        });
    }

    fn poll_snapshot_hotkey(&mut self) {
        if self.plugin_snapshot_request.as_ref().is_some_and(|job| job.is_finished())
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
        if self.gesture.is_some() {
            return;
        }
        self.controller.poll();
        self.sync_generation();
        let has_selection = self.selection.is_some();
        let spec = SnapshotSpec {
            selection: self.selection.clone(),
            output: self.saved_output.clone(),
            profile: self.wav_profile,
            routes: self.controller.sources().each_ref().map(|settings| {
                settings
                    .as_ref()
                    .map_or(ChannelRoute::Merged, |settings| settings.route)
            }),
            force_mono: self.wav_force_mono,
            flags: (!has_selection).then(|| self.flag_update(0..0)),
            export_options: self.export_options(),
        };
        if let Some(edit) = &self.editing {
            if let Some(clip) = spec.clip(Arc::clone(&edit.snapshots), &edit.lanes) {
                self.pending_snapshot_saves.push_back(clip);
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
        let flags = if self.selection.is_none() {
            Some(self.flag_update(snapshots.first()?.frame_range()))
        } else {
            None
        };
        let routes = self.controller.sources().each_ref().map(|settings| {
            settings
                .as_ref()
                .map_or(ChannelRoute::Merged, |settings| settings.route)
        });
        let lanes = self.editing.as_ref().map_or_else(
            || self.controller.active_lanes(),
            |edit| edit.lanes.as_slice(),
        );
        WavClip::new(
            snapshots,
            self.selection.as_ref(),
            output,
            self.wav_profile,
            physical_routes(routes, lanes),
            self.wav_force_mono,
            flags,
        )
        .map(|mut clip| {
            clip.export_options = self.export_options();
            clip
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
                    format!("Saved snapshot {}", file.description())
                }
                Ok(Err(error)) => format!("Could not save snapshot: {error}"),
                Err(_) => "Snapshot worker stopped unexpectedly.".into(),
            };
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
        if self.editing.is_none() && self.edit_request.is_none() { self.begin_editing(); }
        if self.edit_request.is_some() {
            self.export_requested = true;
            self.message = PREPARING_WAV.into();
            return;
        }
        self.export_requested = false;
        let output = self.output.clone();
        let Some(wav) = self.current_snapshots().and_then(|snapshot| self.wav_clip(snapshot, output))
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
        let history_seconds =
            (self.history_frames.end - self.history_frames.start) as f32 / self.sample_rate.max(1) as f32;
        let retention = self.editing.as_ref().map(|edit| edit.retention_seconds)
            .or_else(|| self.edit_request.as_ref().map(|request| request.retention_seconds))
            .unwrap_or_else(|| self.plot_retention_seconds());
        let width = inner.width() * (history_seconds / retention.max(1.0)).clamp(0.0, 1.0);
        Rect::from_min_max(Pos2::new(inner.right() - width, inner.top()), inner.max)
    }

    fn interact_waveform(&mut self, ui: &egui::Ui, frame: &eframe::Frame, rect: Rect) {
        if self.history_frames.is_empty() { return; }
        if self.interact_timeline(ui, rect) {
            return;
        }
        let (position, press, down, snap) = ui.input(|input| {
            let press = waveform_press(&input.events, ALT_SELECTS);
            (
                input.pointer.interact_pos(),
                press,
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
            let resize = self.flag_endpoints(self.audio_rect(rect), position);
            if selecting || resize.is_some() {
                self.begin_editing();
                self.last_interaction = Instant::now();
                let start = self.frame_at(self.audio_rect(rect), position.x, snap_on_press);
                let (anchor, cursor) = resize.unwrap_or((start, start));
                self.gesture = Some(Gesture::Select {
                    anchor,
                    cursor,
                    origin_x: position.x,
                    moved: false,
                });
            } else {
                self.gesture = Some(Gesture::Export { preparing: false });
            }
        }

        let audio_rect = self.audio_rect(rect);
        if let Some(position) = position
            && matches!(self.gesture, Some(Gesture::Select { .. }))
        {
            let endpoint = self.frame_at(audio_rect, position.x, snap);
            if let Some(Gesture::Select { cursor, origin_x, moved, .. }) = &mut self.gesture {
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
            let keep_selection = match gesture {
                Gesture::Select { anchor, cursor, moved, .. } => {
                    let range = anchor.min(cursor)..anchor.max(cursor);
                    if moved && !range.is_empty() {
                        let flags = self.flag_update(range.clone());
                        self.apply_flags(flags);
                        self.selection = Some(range);
                        self.invalidate_export();
                    } else {
                        self.selection = None;
                    }
                    self.selection.is_some()
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
                    self.message = format!("Saved {}", file.description());
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
            let LaneKind::Stack { group, groups } = &plan.kind else {
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
        let LaneKind::Stack { group, .. } = &plan.kind else {
            return None;
        };
        let label = group
            .labels
            .get(row)
            .map_or_else(|| (row + 1).to_string(), Clone::clone);
        let mut text = language.channel(row + 1, group.channels.len(), &label);
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
                        pos, button: PointerButton::Middle, pressed: true, ..
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
            (input.pointer.hover_pos(), input.pointer.interact_pos(), press,
                input.pointer.button_down(PointerButton::Middle), steps)
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
                .round() as u64).clamp(minimum, total);
            let fraction_at = |bounds: Rect| {
                f64::from(((point.x - bounds.left()) / bounds.width().max(1.0)).clamp(0.0, 1.0))
            };
            let anchor_rect = if requested != &self.history_frames { self.timeline_rect(rect) } else { audio };
            let anchor = requested.start + (count as f64 * fraction_at(anchor_rect)).round() as u64;
            let fraction = fraction_at(self.timeline_rect(rect));
            self.set_visible_frames(
                anchor.saturating_sub((new_count as f64 * fraction).round() as u64), new_count
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
        let flags = self.selection.as_ref().or_else(|| self.flags.as_ref().map(|flags| &flags.frames))?;
        let (fixed, moved) =
            flag_handle_at(audio, self.visible_frames.clone(), flags, position, FLAG_REACH)?;
        Some((fixed.clamp(self.history_frames.start, self.history_frames.end), moved))
    }

    fn frame_at(&self, audio: Rect, x: f32, snap: bool) -> u64 {
        let mut flags: Vec<_> = self.flags.iter()
            .flat_map(|flags| [flags.frames.start, flags.frames.end]).collect();
        if let Some(Gesture::Select { anchor, .. }) = self.gesture {
            // The stationary flag is visible; the moving cursor must not attract itself.
            flags.push(anchor);
        } else if let Some(selection) = &self.selection {
            flags.extend([selection.start, selection.end]);
        }
        let available = self.editing.as_ref().map_or_else(|| self.history_frames.clone(), FrozenEdit::frames);
        frame_from_pointer(audio, x, self.visible_frames.clone(), snap, &flags, &self.visible_guides(audio))
            .clamp(available.start, available.end)
    }

    fn draw_waveform(&self, ui: &egui::Ui, rect: Rect, preview_hover: bool) {
        if self.waveform_border {
            let color = self.waveform_border_color.unwrap_or(self.accent);
            let color = if self.waveform_border_follows_background {
                self.background_color(color)
            } else {
                color
            };
            ui.painter_at(rect).rect_stroke(
                rect, if self.waveform_only { 0.0 } else { 6.0 },
                Stroke::new(self.border_inset(), color), egui::StrokeKind::Inside
            );
        }
        let rect = rect.shrink(self.border_inset());
        let painter = ui.painter_at(rect);
        if self.waveform_only {
            painter.rect_filled(rect, 0.0, self.background_color(PLOT_BACKGROUND));
        } else {
            // The panel already supplies the chosen alpha when translucent.
            if self.transparency == 0 {
                painter.rect_filled(rect, 6.0, PLOT_BACKGROUND);
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
        let full_preview = !self.waveform_only
            && selection.is_none()
            && self.selection.is_none()
            && (matches!(self.gesture, Some(Gesture::Export { .. }))
                || preview_hover
                    && ui.input(|input| {
                        input
                            .pointer
                            .hover_pos()
                            .is_some_and(|pos| rect.contains(pos))
                    }));
        if full_preview {
            painter.rect_filled(audio, 2.0, muted_accent(self.accent));
        }

        let visible = self.visible_frames.clone();
        self.paint_time_lines(&painter, audio);
        let highlight = selection.as_ref().and_then(|range| {
            let start = waveform::flag_fraction(range.start.max(visible.start), visible.clone())?;
            let end = waveform::flag_fraction(range.end.min(visible.end), visible.clone())?;
            Some(start as f32..end as f32)
        });
        let boxes = self.lane_boxes(audio.height());
        let stacked = self.stacked();
        let rows: usize = boxes.iter().map(|lane| lane.shown).sum();
        let dual = self.dual_source();
        for (plan, lane_box) in self.lanes.iter().zip(&boxes) {
            for index in lane_box.visible() {
                let top = audio.top() + lane_box.row_top(index).unwrap_or_default();
                let row = Rect::from_min_size(
                    Pos2::new(audio.left(), top),
                    Vec2::new(audio.width(), lane_box.row_height),
                );
                let source = plan.row(index);
                if !self.waveform_only {
                    painter.hline(row.x_range(), row.center().y, Stroke::new(1.0, CENTER_LINE));
                    // Mono and stereo lanes are labelled as before; a stacked lane names
                    // each channel.
                    let label = match (&plan.kind, source) {
                        (LaneKind::Stack { group, .. }, _) => Some(stack::short_label(
                            group.labels.get(index).map_or("", String::as_str),
                        ))
                        .filter(|label| !label.is_empty())
                        .or_else(|| Some((index + 1).to_string())),
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
                            Color32::from_gray(180),
                        );
                    }
                }
                if let Some(peaks) = self
                    .peak_rows
                    .iter()
                    .position(|row| *row == source)
                    .and_then(|position| self.peaks.get(position))
                {
                    self.paint_waveform(&painter, row, peaks, highlight.as_ref());
                }
            }
        }
        if stacked {
            self.paint_lane_furniture(ui, &painter, rect, audio, &boxes);
        }
        self.paint_flags(&painter, audio);

        if !self.waveform_only
            && let Some(range) = selection.or_else(|| full_preview.then_some(visible))
        {
            painter.text(
                audio.right_top() - Vec2::new(8.0, 0.0),
                egui::Align2::RIGHT_TOP,
                format!(
                    "{:.2} s",
                    (range.end - range.start) as f64 / f64::from(self.sample_rate.max(1)),
                ),
                egui::FontId::monospace(11.0),
                Color32::WHITE,
            );
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
                Color32::from_gray(if active { 200 } else { 128 }),
            );
        }
        // The same dark backing as the waveform-only status mark keeps the switch readable.
        let chips = self.group_chips(ui, rect, boxes);
        for lane in [0, 1] {
            let lane_chips: Vec<_> = chips.iter().filter(|chip| chip.lane == lane).collect();
            let (Some(start), Some(end)) = (lane_chips.first(), lane_chips.last()) else {
                continue;
            };
            painter.rect_filled(
                start.rect.union(end.rect),
                3.0,
                Color32::from_black_alpha(168),
            );
            for chip in lane_chips {
                let hovered = hover.is_some_and(|point| chip.rect.contains(point));
                if chip.current {
                    painter.rect_filled(chip.rect.shrink(1.0), 2.5, muted_accent(self.accent));
                }
                painter.text(
                    chip.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &chip.label,
                    egui::FontId::proportional(10.0),
                    if chip.current || hovered {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(180)
                    },
                );
            }
        }
    }

    fn visible_guides(&self, audio: Rect) -> waveform::TimelineGuides {
        let bars = self.editing.as_ref().map_or(self.bar_markers.as_slice(), |edit| edit.bars.as_slice());
        waveform::timeline_guides(self.visible_frames.clone(), self.sample_rate,
            u32::from(self.second_interval), bars, self.bar_lines, self.second_lines,
            self.transparency < 100, &self.display_policy(audio, self.visible_frames.clone()))
    }

    fn paint_time_lines(&self, painter: &egui::Painter, audio: Rect) {
        let guides = self.visible_guides(audio);
        let policy = self.display_policy(audio, self.visible_frames.clone());
        let draw = |frames: &[u64], alpha: u16, stroke| {
            let alpha = alpha * u16::from(100 - self.transparency) / 100;
            paint_guide_lines(painter, audio, &self.visible_frames, frames,
                Stroke::new(stroke, Color32::from_white_alpha(alpha as u8)));
        };
        draw(&guides.seconds, 52, policy.seconds_stroke);
        draw(&guides.bars, 104, policy.bar_stroke);
    }

    fn paint_flags(&self, painter: &egui::Painter, audio: Rect) {
        let painter = painter.with_clip_rect(audio);
        let visible = self.visible_frames.clone();
        let flag_height = audio.height().min(11.0);
        let current = match self.gesture {
            Some(Gesture::Select { anchor, cursor, .. }) => Some(anchor.min(cursor)..anchor.max(cursor)),
            _ => self.selection.clone(),
        };
        let accepted = self.flags.as_ref().map(|flags| &flags.frames);
        let ranges = accepted.into_iter().chain(current.as_ref().filter(|range| Some(*range) != accepted));
        for flags in ranges {
        for (position, direction) in [(flags.start, 1.0), (flags.end, -1.0)] {
            let Some(fraction) = waveform::flag_fraction(position, visible.clone()) else {
                continue;
            };
            let x = audio.left() + fraction as f32 * audio.width();
            let inset = (audio.width() * 0.25).min(0.25);
            let x = x.clamp(audio.left() + inset, audio.right() - inset);
            let y = audio.top();
            painter.line_segment(
                [Pos2::new(x, y), Pos2::new(x, y + flag_height)],
                Stroke::new(1.0, FLAG_COLOR),
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(x, y),
                    Pos2::new(x + direction * 6.0, y + flag_height * 0.28),
                    Pos2::new(x, y + flag_height * 0.56),
                ],
                FLAG_COLOR,
                Stroke::NONE,
            ));
        }
        }
    }

    fn paint_waveform(
        &self,
        painter: &egui::Painter,
        audio: Rect,
        peaks: &WaveformPeaks,
        selection: Option<&Range<f32>>,
    ) {
        let policy = self.display_policy(audio, self.visible_frames.clone());
        let geometry = waveform::waveform_geometry(&peaks.low, &peaks.high, self.visible_frames.clone());
        let selected = selected_waveform_color(self.accent, SELECTION_CONTRAST);
        let painter = painter.with_clip_rect(audio);
        let paint_range = |range: Range<f32>, color| {
            if range.is_empty() { return; }
            let clip = Rect::from_x_y_ranges(
                audio.left() + range.start * audio.width()..=audio.left() + range.end * audio.width(),
                audio.y_range(),
            );
            paint_waveform_geometry(&painter.with_clip_rect(clip), audio,
                &geometry, policy.waveform_stroke, color);
        };
        if let Some(range) = selection.filter(|range| range.start < range.end) {
            paint_range(0.0..range.start, self.accent);
            paint_range(range.clone(), selected);
            paint_range(range.end..1.0, self.accent);
        } else {
            paint_range(0.0..1.0, self.accent);
        }
    }


}

/// Whether the capture status mark is drawn for this state under the user's choice.
fn status_mark_shown(mark: StatusMark, indicator: CaptureIndicator, problem: bool) -> bool {
    match mark {
        StatusMark::Always => true,
        StatusMark::Attention => {
            problem || matches!(indicator, CaptureIndicator::Paused | CaptureIndicator::Unavailable)
        }
        StatusMark::Never => false,
    }
}

/// Paints the waveform-only status mark in the top-left corner of the plot: a dot coloured by
/// state, or pause bars, with the source problem written beside it when there is one.
/// Returns the area whose tooltip explains the mark.
fn paint_compact_status(
    painter: &egui::Painter,
    rect: Rect,
    indicator: CaptureIndicator,
    lit: bool,
    problem: Option<&str>,
) -> Rect {
    let color = if problem.is_some() { ERROR_LED } else { indicator.compact_color(lit) };
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
            problem.to_owned(), egui::FontId::proportional(12.0), ERROR_LED,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(width);
        let galley = painter.layout_job(job);
        let text = Rect::from_min_size(
            Pos2::new(left, center.y - galley.size().y * 0.5), galley.size(),
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
) {
    let position = |x, value| Pos2::new(
        audio.left() + x * audio.width(),
        audio.center().y - value * audio.height() * 0.47,
    );
    paint_waveform_trace(painter, geometry, position, &TraceStyle {
        stroke: Stroke::new(stroke, color),
        fill: color.linear_multiply(0.4),
        lone_sample: LoneSample::Dot(stroke * 0.55),
    });
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
            let cancel_key = |event: &egui::Event| match event {
                egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                } => true,
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    pressed: true,
                    ..
                } => true,
                _ => false,
            };
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
                self.window_drag =
                    window_interaction(bounds, pos, modifiers.alt, can_resize).map(|interaction| {
                        WindowDrag {
                            interaction,
                            press_position: pos,
                            origin: None,
                            released: false,
                        }
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
        }
        self.controller_busy = !self.controller.poll();
        self.sync_generation();
        self.collect_editing();
        if let Some(error) = self.preference_writer.as_ref().and_then(PreferenceWriter::poll_error) {
            self.message = format!("Could not save settings: {error}");
        }
        if !self.controller.is_recording() {
            self.recording_started = None;
        }
        self.collect_export();
        self.collect_snapshot_saves();
        self.clean_output_folder();
        if self.auto_resume_remaining().is_some_and(|remaining| remaining.is_zero()) {
            self.resume();
        }
        // The window is not always drawn after this, so ask for the next pass here too.
        self.schedule_wake(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let panel = egui::Frame::central_panel(ui.style());
        let frame_style = if self.waveform_only {
            panel
                .fill(Color32::TRANSPARENT)
                .inner_margin(0)
        } else {
            panel.fill(self.background_color(ui.visuals().panel_fill))
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
        .then(|| ui.input(|input| window_shortcut(&input.events)))
        .flatten();
        egui::CentralPanel::default()
            .frame(frame_style)
            .show(ui, |ui| {
                let window_drag = self.window_drag.as_ref().map(|drag| drag.interaction);
                let interaction_blocked = window_drag.is_some();
                if !self.waveform_only {
                    self.controls(ui, interaction_blocked);
                }
                let window_cursor = if let Some(interaction) = window_drag {
                    Some(interaction.cursor())
                } else if !interaction_blocked && self.gesture.is_none() {
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
                if self.plot_rect.size() != plot.size() { self.next_refresh = Instant::now(); }
                self.plot_rect = plot;
                self.update_lanes();
                self.fit_window_to_rows(ui.ctx(), frame);
                self.refresh_waveform();
                if !self.waveform_only {
                    self.full_waveform_height = rect.height() - 2.0 * WAVEFORM_INSET;
                    self.full_window_size = ui.ctx().viewport_rect().size();
                }
                let mut lane_tip = None;
                if !interaction_blocked {
                    lane_tip = self.interact_lanes(ui, rect.shrink(self.border_inset()));
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
        } else if let Some(command) = shortcut {
            // Delete with a selection on screen drops the selection, as Backspace and Escape do.
            // Wiping the whole capture takes a second press.
            if command == WindowCommand::Clear && self.selection.is_some() {
                self.cancel_selection();
            } else {
                self.run_command(command, ui.ctx(), frame);
            }
        }
        // A package dropped on this window opens its confirmation in Options. Other files are
        // left alone, and a clip dragged out never drops here (`file_drag` cancels a drop on
        // the app's own windows).
        if let Some(dropped) = addons_page::dropped_package(&dropped_paths(ui.ctx())) {
            self.review_addon(dropped);
            if self.options_open {
                self.options_focus_requested = true;
            } else {
                self.open_options(ui.ctx());
            }
        }
        addons_page::paint_drop_hint(ui.ctx(), self.accent, Language::current());
        self.draw_options_window(ui.ctx(), frame);
        // Drawing can start a gesture, an export or the recording blink.
        self.watch_pin(frame, Instant::now());
        self.schedule_wake(ui.ctx(), frame);
        self.drive_harness(ui.ctx(), frame);
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.save_accent();
        self.save_window_position();
        if let Some(request) = self.edit_request.take() {
            if !request.saves.is_empty() {
                self.pending_edit_save_reads.push(request);
            }
        }
        // An explicitly requested save survives closing the UI before its read reply arrives.
        // Waiting is confined to shutdown; frame/update paths only use try_recv.
        for read in self.pending_edit_save_reads.drain(..) {
            if let Ok(Some(snapshot)) = read.result.recv() {
                if edit_snapshot_matches(&snapshot, read.generation) {
                    for spec in read.saves {
                        if let Some(clip) = spec.frozen_clip(&snapshot) {
                            self.pending_snapshot_saves.push_back(clip);
                        }
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
        if let Some(writer) = &self.preference_writer {
            if let Err(error) = writer.flush() {
                eprintln!("Could not save settings on exit: {error}");
            }
        }
    }
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
        && choices.iter().filter(|choice| choice.label() == name).count() > 1
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

    fn painted_waveform(
        minimum: &[f32], maximum: &[f32], visible: Range<u64>, audio: Rect,
    ) -> Vec<egui::epaint::ClippedShape> {
        let context = egui::Context::default();
        let geometry = waveform::waveform_geometry(minimum, maximum, visible);
        let mut output = context.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0))),
            ..Default::default()
        }, |ui| {
            paint_waveform_geometry(&ui.painter().with_clip_rect(audio), audio,
                &geometry, 1.5, Color32::GREEN);
        });
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        shapes
    }

    #[test]
    fn sample_waveform_connects_signed_samples_without_stretching_the_last_frame() {
        let audio = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(120.0, 100.0));
        let shapes = painted_waveform(&[0.75, -0.5, 0.25], &[0.75, -0.5, 0.25], 100..103, audio);
        let paths: Vec<_> = shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Path(path) => Some(path),
            _ => None,
        }).collect();
        assert_eq!(paths.len(), 1, "Sample zoom must connect the actual signed sample sequence");
        let points = &paths[0].points;
        assert_eq!(points.len(), 3);
        for (point, (x, sample)) in points.iter().zip([(10.0, 0.75), (50.0, -0.5), (90.0, 0.25)]) {
            assert!((point.x - x).abs() < 0.001);
            assert!((point.y - (audio.center().y - sample * audio.height() * 0.47)).abs() < 0.001);
        }
        assert!(points[2].x < audio.right(), "The exclusive frame boundary has no invented sample");
        assert!(!shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Mesh(_))));
    }

    #[test]
    fn overview_envelope_fills_between_columns_without_comb_gaps() {
        let audio = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(120.0, 100.0));
        let shapes = painted_waveform(&[-0.6; 3], &[0.8; 3], 100..400, audio);
        let meshes: Vec<_> = shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) => Some(mesh),
            _ => None,
        }).collect();
        assert_eq!(meshes.len(), 1);
        let mesh = meshes[0];
        let inside = |point: Pos2, triangle: &[u32]| {
            let [a, b, c] = std::array::from_fn(|index| mesh.vertices[triangle[index] as usize].pos);
            let cross = |a: Vec2, b: Vec2| a.x * b.y - a.y * b.x;
            let sides = [cross(b - a, point - a), cross(c - b, point - b), cross(a - c, point - c)];
            sides.iter().all(|side| *side >= -0.001) || sides.iter().all(|side| *side <= 0.001)
        };
        for pixel in 0..120 {
            let point = Pos2::new(audio.left() + pixel as f32 + 0.5, audio.center().y);
            assert!(mesh.indices.chunks_exact(3).any(|triangle| inside(point, triangle)),
                "An overview must retain a continuous filled envelope at x={}", point.x);
        }
        let outlines: Vec<_> = shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Path(path) => Some(path),
            _ => None,
        }).collect();
        assert_eq!(outlines.len(), 2);
        assert!(outlines.iter().all(|path| path.points.first().unwrap().x == audio.left()
            && path.points.last().unwrap().x == audio.right()));
    }

    #[test]
    fn editing_snapshot_remains_the_source_after_live_audio_has_rolled_out() {
        let retained: Arc<[FrozenTrack]> = vec![FrozenTrack::from_interleaved(48_000, 1, 100, &[0.1, 0.2, 0.3, 0.4]).unwrap()].into();
        let newer: Arc<[FrozenTrack]> = vec![FrozenTrack::from_interleaved(48_000, 1, 500, &[0.9; 4]).unwrap()].into();
        let source = snapshot_source(Some(&retained), false, || Some(newer)).unwrap();
        assert!(Arc::ptr_eq(&source, &retained));
        let clip = WavClip::new(source, Some(&(101..103)), PathBuf::new(),
            WavExportProfile::default(), [ChannelRoute::Merged; 2], false, None).unwrap();
        assert_eq!(clip.frames, 1..3);
        assert_eq!(clip.snapshots[0].snapshot().frames(clip.frames.clone()), Some(&[0.2, 0.3][..]));
    }

    #[test]
    fn pending_edit_never_exports_a_different_live_snapshot() {
        let mut live_was_read = false;
        let source = snapshot_source(None, true, || {
            live_was_read = true;
            None
        });
        assert!(source.is_none());
        assert!(!live_was_read, "An unfinished edit must wait for its own retained material");
    }

    #[test]
    fn capture_replacement_is_rejected_even_when_its_absolute_frames_overlap() {
        let make_reply = |generation, second_first| EditSnapshot {
            generation,
            bar_markers: vec![101],
            lanes: vec![0, 1],
            groups: std::array::from_fn(|_| Vec::new()),
            shown_groups: [None, None],
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
        assert_eq!(initial_window_size(false, Some("1500x500")), Vec2::new(1500.0, 500.0));
        assert_eq!(initial_window_size(true, Some("1500x500")), Vec2::new(1500.0, 500.0));
        for input in ["NaNx500", "1500xinf", "500x500", "1500x100", "9000x500", "1500x9000", "1500x500x1", "invalid"] {
            assert_eq!(initial_window_size(false, Some(input)), Vec2::new(610.0, 144.0), "{input}");
        }
        assert_eq!(initial_window_size(false, None), Vec2::new(610.0, 144.0));
        assert_eq!(initial_window_size(true, None), Vec2::new(610.0, DEFAULT_WAVEFORM_HEIGHT));
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
        assert_eq!(next_wake(&Waits { hidden: true, ..Waits::default() }), SAFETY_TICK);
    }

    #[test]
    fn work_in_progress_keeps_the_frame_cadence_it_had() {
        assert_eq!(next_wake(&Waits { busy: true, ..Waits::default() }), BUSY_REFRESH);
        assert_eq!(next_wake(&Waits { polling: true, ..Waits::default() }), REFRESH);
        assert_eq!(
            next_wake(&Waits { busy: true, polling: true, hidden: true, ..Waits::default() }),
            BUSY_REFRESH,
        );
    }

    #[test]
    fn deadlines_wake_the_window_when_they_are_due_and_never_spin_it() {
        let position = Waits { position_save: Some(milliseconds(120)), ..Waits::default() };
        assert_eq!(next_wake(&position), milliseconds(120));
        let overdue = Waits { position_save: Some(Duration::ZERO), ..Waits::default() };
        assert_eq!(next_wake(&overdue), BUSY_REFRESH);
        // A tray that cannot be added is retried, but not in a tight loop.
        let tray = Waits { tray_retry: Some(Duration::ZERO), ..Waits::default() };
        assert_eq!(next_wake(&tray), RETRY_FLOOR);
        let tray = Waits { tray_retry: Some(milliseconds(1_400)), ..Waits::default() };
        assert_eq!(next_wake(&tray), milliseconds(1_400));
        let scan = Waits { options_scan: Some(milliseconds(900)), ..Waits::default() };
        assert_eq!(next_wake(&scan), milliseconds(900));
        let stuck = Waits { options_scan: Some(Duration::ZERO), ..Waits::default() };
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
        let unseen = Waits { countdown_shown: false, ..shown.clone() };
        assert_eq!(next_wake(&unseen), SAFETY_TICK);
        let hidden = Waits { hidden: true, auto_resume: Some(milliseconds(900)), ..shown };
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
            assert_ne!(lit(seconds), lit(seconds + blink_tick(seconds).as_secs_f64()));
        }
        let recording = Waits { blink: Some(0.2), ..Waits::default() };
        assert_eq!(next_wake(&recording), milliseconds(302));
        assert_eq!(next_wake(&Waits { hidden: true, ..recording }), SAFETY_TICK);
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
        assert_eq!(next_wake(&Waits { waveform: None, ..shown.clone() }), milliseconds(151));
        assert_eq!(
            next_wake(&Waits { waveform: None, fine_edit: None, ..shown.clone() }),
            METER_STALE_CHECK,
        );
        // A waveform that is already due is drawn on the next frame, not in a spin.
        assert_eq!(next_wake(&Waits { waveform: Some(Duration::ZERO), ..shown.clone() }), BUSY_REFRESH);
        assert_eq!(next_wake(&Waits { hidden: true, ..shown }), SAFETY_TICK);
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
        assert_eq!(idle_status(None, "Saved snapshot a.wav"), "Saved snapshot a.wav");
        assert_eq!(idle_status(Some("Paused".into()), ""), "Paused");
        assert_eq!(
            idle_status(Some(paused_status(remaining(3_000))), "Saved snapshot a.wav"),
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
            ["Capture unavailable", "Paused", "Recording", "Waiting for sound"]
        );

        // In waveform-only mode colour alone has to tell the four states apart, in both halves
        // of the recording blink, against the dark plot.
        let plot = Color32::from_rgb(22, 26, 29);
        let distance = |a: Color32, b: Color32| {
            a.to_array().into_iter().zip(b.to_array()).take(3)
                .map(|(a, b)| u32::from(a.abs_diff(b))).sum::<u32>()
        };
        for lit in [false, true] {
            let colors = [Unavailable, Paused, Recording, Waiting].map(|state| state.compact_color(lit));
            for (index, color) in colors.iter().enumerate() {
                assert!(distance(*color, plot) >= 90, "{color:?} is lost on the plot");
                for other in &colors[index + 1..] {
                    assert!(distance(*color, *other) >= 90, "{color:?} is too close to {other:?}");
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
            compact_status_text("Capture unavailable", Some("No audio device"), "No audio device"),
            "Capture unavailable · No audio device"
        );
    }

    #[test]
    fn compact_status_mark_fits_the_smallest_window_and_truncates_a_long_problem() {
        let plot = Rect::from_min_size(Pos2::ZERO, Vec2::new(WAVEFORM_MIN_WIDTH, WAVEFORM_MIN_HEIGHT));
        let paint = |indicator, problem: Option<&str>| {
            let context = egui::Context::default();
            let mut hover = Rect::NOTHING;
            let mut output = context.run_ui(egui::RawInput {
                screen_rect: Some(plot),
                ..Default::default()
            }, |ui| {
                hover = paint_compact_status(&ui.painter_at(plot), plot, indicator, true, problem);
            });
            let shapes: Vec<_> = std::mem::take(&mut output.shapes)
                .into_iter().map(|clipped| clipped.shape).collect();
            output.drop_without_applying_deltas();
            (hover, shapes)
        };
        let fill = |shape: &egui::Shape| match shape {
            egui::Shape::Circle(circle) => Some(circle.fill),
            egui::Shape::Rect(rect) => Some(rect.fill),
            _ => None,
        };
        let written = |shapes: &[egui::Shape]| shapes.iter().find_map(|shape| match shape {
            egui::Shape::Text(text) => Some(text.clone()),
            _ => None,
        });

        for indicator in [CaptureIndicator::Recording, CaptureIndicator::Waiting, CaptureIndicator::Unavailable] {
            let (hover, shapes) = paint(indicator, None);
            assert!(plot.contains_rect(hover) && hover.width() >= 12.0, "{indicator:?} {hover:?}");
            assert!(shapes.iter().filter_map(fill).any(|color| color == indicator.compact_color(true)));
            assert!(written(&shapes).is_none());
        }
        // Paused is two bars, so it differs from the other states by shape as well as colour.
        let (_, shapes) = paint(CaptureIndicator::Paused, None);
        let bars = shapes.iter().filter_map(fill).filter(|color| *color == COMPACT_PAUSED).count();
        assert_eq!(bars, 2);

        // A source problem turns the mark amber whatever else capture is doing, and its text
        // stays on one line inside the plot however long the message is.
        let problem = "Source B: the audio device was unplugged or is in use by another application ".repeat(4);
        let (hover, shapes) = paint(CaptureIndicator::Recording, Some(&problem));
        assert!(shapes.iter().filter_map(fill).any(|color| color == ERROR_LED));
        assert!(!shapes.iter().filter_map(fill).any(|color| color == RECORDING_LIT));
        let text = written(&shapes).expect("the problem is written on the plot");
        assert_eq!(text.galley.rows.len(), 1);
        assert!(text.galley.elided);
        assert!(text.pos.x + text.galley.size().x <= plot.right());
        assert!(plot.contains_rect(hover) && hover.width() > 150.0, "{hover:?}");

        let (hover, shapes) = paint(CaptureIndicator::Unavailable, Some("No audio device"));
        let text = written(&shapes).unwrap();
        assert_eq!(text.galley.text(), "No audio device");
        assert!(!text.galley.elided);
        assert!(hover.right() >= text.pos.x + text.galley.size().x - 0.5);
    }

    #[test]
    fn window_keys_run_commands_once_and_leave_other_chords_alone() {
        let key = |key, modifiers, pressed, repeat| egui::Event::Key {
            key, physical_key: None, pressed, repeat, modifiers,
        };
        let none = egui::Modifiers::NONE;
        let press = |code, modifiers| window_shortcut(&[key(code, modifiers, true, false)]);
        assert_eq!(press(egui::Key::Space, none), Some(WindowCommand::ToggleCapture));
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
        assert_eq!(window_shortcut(&[key(egui::Key::Space, none, true, true)]), None);
        assert_eq!(window_shortcut(&[key(egui::Key::Space, none, false, false)]), None);
        assert_eq!(
            window_shortcut(&[
                egui::Event::PointerMoved(Pos2::ZERO),
                key(egui::Key::Space, none, true, true),
                key(egui::Key::Delete, none, true, false),
            ]),
            Some(WindowCommand::Clear)
        );
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
        let snapshots: Arc<[FrozenTrack]> = vec![FrozenTrack::from_interleaved(48000, 1, 1, &[0.1; 4]).unwrap()].into();
        let weak = Arc::downgrade(&snapshots);
        let clip = WavClip::new(snapshots, None, PathBuf::new(), WavExportProfile::default(),
            [ChannelRoute::Merged; 2], false, None).unwrap();
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
        let guides = waveform::TimelineGuides { bars: vec![2000], seconds: vec![2500] };
        assert_eq!(frame_from_pointer(audio, 915.0, 1000..11000, true, &[1900], &guides), 1900);
        assert_eq!(frame_from_pointer(audio, 915.0, 1000..11000, true, &[1600], &guides), 2000);
        assert_eq!(frame_from_pointer(audio, 915.0, 1000..11000, true, &[],
            &waveform::TimelineGuides { bars: vec![], seconds: vec![2500] }), 2500);
        assert_eq!(frame_from_pointer(audio, 914.5, 1000..11000, true, &[],
            &waveform::TimelineGuides::default()), 2450);
        assert_eq!(frame_from_pointer(audio, 914.5, 1000..11000, false, &[1900], &guides), 2450);
        assert_eq!(frame_from_pointer(audio, 915.0, 1000..11000, true, &[1900],
            &waveform::TimelineGuides::default()), 1900, "Transparent backgrounds still show flags");
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
        context.run_ui(egui::RawInput::default(), |ui| {
            let original = ui.style().text_styles.clone();
            options_list(ui, |ui| {
                assert!(ui.style().text_styles[&egui::TextStyle::Body].size >= 15.0);
                assert!(ui.style().text_styles[&egui::TextStyle::Button].size >= 15.0);
                assert!(ui.style().text_styles[&egui::TextStyle::Small].size >= 13.0);
            });
            assert_eq!(ui.style().text_styles, original,
                "Options typography must not enlarge the main waveform controls");
        }).drop_without_applying_deltas();
    }

    #[test]
    fn options_popup_capacity_stops_at_ten_rows_and_short_menus_do_not_reserve_extra_rows() {
        let context = egui::Context::default();
        configure_style(&context, Color32::WHITE);
        context.run_ui(egui::RawInput::default(), |ui| {
            options_list(ui, |ui| {
                let short = options_menu_height(ui, 3);
                let ten = options_menu_height(ui, 10);
                assert!(short < ten, "Three choices should not reserve the full menu height");
                assert_eq!(ten, options_menu_height(ui, 11));
                assert_eq!(ten, options_menu_height(ui, 200));
                let row = options_menu_height(ui, 1);
                let spacing = 4.0;
                assert!((ten - (row * 10.0 + spacing * 9.0)).abs() < 0.01);
            });
        }).drop_without_applying_deltas();
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
                Some(&(11..12)),
                folder.clone(),
                WavExportProfile::default(),
                [ChannelRoute::Left, ChannelRoute::Side],
                force_mono,
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
                        label: "Bed".into(),
                        channels: 0..2,
                        labels: vec!["L".into(), "R".into()],
                    },
                    LaneGroup {
                        id: "objects".into(),
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
        };
        let spec = SnapshotSpec {
            selection: None,
            output: folder.clone(),
            profile: WavExportProfile::default(),
            routes: [ChannelRoute::Merged; 2],
            force_mono: false,
            flags: None,
            export_options: ExportOptions::default(),
        };
        let clip = spec.frozen_clip(&snapshot).unwrap();
        drop(snapshot);
        let files = clip.write(false).unwrap();
        assert_eq!(files.paths.len(), 4);
        for (index, path) in files.paths.iter().enumerate() {
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
        };
        let spec = SnapshotSpec {
            selection: None,
            output: folder.clone(),
            profile: WavExportProfile::default(),
            routes: [ChannelRoute::Left, ChannelRoute::Right],
            force_mono: false,
            flags: None,
            export_options: ExportOptions::default(),
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
        let plan = LanePlan {
            lane: 1,
            kind: LaneKind::Stack {
                group: group("objects", 12..28),
                groups: Vec::new(),
            },
        };
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
}

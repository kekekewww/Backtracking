use std::collections::VecDeque;
use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::controller::{CaptureSource, Controller, EditSnapshot, SourceSettings};
use crate::file_drag;
use crate::preferences::{
    DEFAULT_ACCENT, Key, MeterMode, PreferenceWriter, Preferences, Settings, serialize_sources,
    wav_sample_format_preference,
};
use crate::snapshot_hotkey::{self, SnapshotHotkey};
use crate::startup;
use crate::tray::{self, Tray};
use crate::updater::UpdateOptions;
use crate::window_order;
use backtracking_core::{
    AutoResumeDelay, ChannelRoute, DurationChoice, MeterReadings, Snapshot, WavExportProfile,
    WavSampleFormat, WaveformView, export_routed_tracks_with_profile, waveform,
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

const DEFAULT_BINS: usize = 384;
const REFRESH: Duration = Duration::from_millis(80);
const PIN_RETRY: Duration = Duration::from_millis(320);
const WINDOW_POSITION_SAVE_DELAY: Duration = Duration::from_millis(250);
const TRANSPARENCIES: [u8; 5] = [0, 25, 50, 75, 100];
const WAV_SAMPLE_RATES: [(u32, &str); 3] =
    [(44_100, "44.1 kHz"), (48_000, "48 kHz"), (96_000, "96 kHz")];
const WAV_SAMPLE_FORMATS: [(WavSampleFormat, &str); 3] = [
    (WavSampleFormat::Pcm16, "16-bit PCM"),
    (WavSampleFormat::Pcm24, "24-bit PCM"),
    (WavSampleFormat::Float32, "32-bit float"),
];
const RECORDING_LIT: Color32 = Color32::from_rgb(249, 78, 88);
const RECORDING_DIM: Color32 = Color32::from_rgb(103, 44, 48);
const WAITING_LED: Color32 = Color32::from_rgb(76, 86, 88);
const PAUSED_LED: Color32 = Color32::from_rgb(54, 57, 60);
const ERROR_LED: Color32 = Color32::from_rgb(246, 196, 118);
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
const FLAG_COLOR: Color32 = Color32::from_rgb(255, 201, 109);
const PREPARING_WAV: &str = "Preparing WAV…";

pub(crate) fn run(controller: Controller, primary: bool) -> eframe::Result<()> {
    let show_options_on_start = std::env::args_os().any(|arg| arg == "--show-options");
    let test_size = show_options_on_start.then(|| std::env::var("BACKTRACKING_TEST_WINDOW_SIZE").ok()).flatten();
    let preferences = Preferences::for_current_user();
    let settings = preferences.load();
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/backtracking-icon.png"))
            .expect("decode the Backtracking icon");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
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
            if primary && let (Some(window), Some(position)) =
                (context.winit_window(), settings.window_position)
            {
                window_order::restore_position(window, position);
            }
            let mut recorder = Recorder::new(controller, preferences, settings, primary);
            recorder.options_in_taskbar = show_options_on_start;
            if show_options_on_start {
                recorder.open_options(&context.egui_ctx);
            }
            if primary {
                match SnapshotHotkey::start(context.egui_ctx.clone()) {
                    Ok(hotkey) => recorder.hotkey = Some(hotkey),
                    Err(error) => recorder.hotkey_error = Some(error.to_string()),
                }
            }
            if !show_options_on_start && let Some(window) = context.winit_window() {
                recorder.attach_tray(window, &context.egui_ctx);
            }
            if primary {
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
    ctx.set_fonts(fonts);
}

fn accent_color([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn muted_accent(color: Color32) -> Color32 {
    Color32::from_rgb(color.r() / 3, color.g() / 3, color.b() / 3)
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
    saved_output: PathBuf,
    pinned: bool,
    next_pin_check: Instant,
    waveform_only: bool,
    full_waveform_height: f32,
    full_window_size: Vec2,
    waveform_border: bool,
    waveform_border_color: Option<Color32>,
    waveform_border_follows_background: bool,
    bar_lines: bool,
    second_lines: bool,
    second_interval: u8,
    options_open: bool,
    options_in_taskbar: bool,
    options_focus_requested: bool,
    options_section: OptionsSection,
    available_sources: Vec<CaptureSource>,
    source_list_worker: Option<JoinHandle<Vec<CaptureSource>>>,
    source_list_refresh_pending: bool,
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
    last_interaction: Instant,
    recording_started: Option<f64>,
    peaks: [WaveformPeaks; 2],
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
    Updates,
}

impl OptionsSection {
    const ALL: [(Self, &'static str); 6] = [
        (Self::Actions, "Actions & shortcuts"),
        (Self::Capture, "Capture"),
        (Self::Sources, "Sources & connections"),
        (Self::Appearance, "Waveform & appearance"),
        (Self::Export, "Export"),
        (Self::Updates, "Updates"),
    ];

    fn title(self) -> &'static str {
        Self::ALL.iter().find(|(section, _)| *section == self).unwrap().1
    }
}

fn configure_options_style(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(13.0));
    style.text_styles.insert(egui::TextStyle::Heading,
        egui::FontId::new(22.0, egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into())));
    style.spacing.interact_size.y = 32.0;
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.spacing.item_spacing.y = 12.0;
}

fn options_heading_text(label: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(label).font(egui::FontId::new(
        size, egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into()),
    ))
}

fn options_group_heading(ui: &mut egui::Ui, label: &str) {
    ui.add_space(4.0);
    ui.label(options_heading_text(label, 18.0));
}

fn options_list<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
        configure_options_style(ui);
        ui.set_width(ui.available_width());
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        contents(ui)
    }).inner
}

fn options_menu_row_height(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y.max(
        ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y,
    )
}

fn options_menu_height(ui: &egui::Ui, item_count: usize) -> f32 {
    let rows = item_count.clamp(1, 10) as f32;
    rows * options_menu_row_height(ui) + (rows - 1.0) * 4.0
}

fn configure_options_menu(ui: &mut egui::Ui, width: f32) {
    configure_options_style(ui);
    ui.set_max_width(width);
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
}

fn options_field<R>(ui: &mut egui::Ui, label: &str, control: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.vertical(|ui| {
        ui.label(options_heading_text(label, 16.0));
        let result = control(ui);
        ui.add_space(9.0);
        result
    }).inner
}

fn options_choice<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    selected: &mut T,
    choices: impl IntoIterator<Item = (T, String)>,
) -> bool {
    let choices: Vec<_> = choices.into_iter().collect();
    let text = choices.iter().find(|(value, _)| value == selected)
        .map_or_else(String::new, |(_, text)| text.clone());
    let before = *selected;
    let width = ui.available_width().min(680.0);
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .height(options_menu_height(ui, choices.len()))
        .selected_text(&text)
        .truncate()
        .show_ui(ui, |ui| {
            configure_options_menu(ui, width);
            for (value, label) in choices {
                ui.selectable_value(selected, value, label);
            }
        }).response.on_hover_text(text);
    *selected != before
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

struct FrozenEdit {
    snapshots: Arc<[Snapshot]>,
    bars: Vec<u64>,
    retention_seconds: u32,
}

impl FrozenEdit {
    fn frames(&self) -> Range<u64> {
        self.snapshots.first().map_or(0..0, Snapshot::frame_range)
    }
}

struct EditRequest {
    result: mpsc::Receiver<Option<EditSnapshot>>,
    generation: u64,
    retention_seconds: u32,
    saves: Vec<SnapshotSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditPreviewKey {
    frames: Range<u64>,
    views: [(usize, WaveformView); 2],
    rows: usize,
    bins: usize,
}

struct EditPreviewJob {
    key: EditPreviewKey,
    worker: JoinHandle<[WaveformPeaks; 2]>,
}

struct SnapshotRead {
    result: mpsc::Receiver<Option<Arc<[Snapshot]>>>,
    saves: Vec<SnapshotSpec>,
}

struct SnapshotSpec {
    selection: Option<Range<u64>>,
    output: PathBuf,
    profile: WavExportProfile,
    routes: [ChannelRoute; 2],
    force_mono: bool,
    flags: Option<FlagUpdate>,
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
    fn clip(mut self, snapshots: Arc<[Snapshot]>) -> Option<WavClip> {
        if let Some(flags) = &mut self.flags {
            flags.frames = snapshots.first()?.frame_range();
        }
        WavClip::new(snapshots, self.selection.as_ref(), self.output,
            self.profile, self.routes, self.force_mono, self.flags)
    }
}

fn edit_preview(snapshots: &[Snapshot], key: &EditPreviewKey) -> [WaveformPeaks; 2] {
    std::array::from_fn(|row| {
        let mut peaks = WaveformPeaks::empty(key.bins);
        if row >= key.rows { return peaks; }
        let (lane, view) = key.views[row];
        let Some(snapshot) = snapshots.get(lane) else { return peaks; };
        let frames = snapshot.frame_range();
        let start = key.frames.start.max(frames.start);
        let end = key.frames.end.min(frames.end).max(start);
        let Some(samples) = snapshot.frames((start - frames.start) as usize..(end - frames.start) as usize)
        else { return peaks; };
        waveform::view_peaks(samples, snapshot.channels, view, &mut peaks.low, &mut peaks.high);
        peaks
    })
}

struct FlagUpdate {
    generation: u64,
    order: u64,
    frames: Range<u64>,
}

fn snapshot_source(
    editing: Option<&Arc<[Snapshot]>>,
    pending: bool,
    live: impl FnOnce() -> Option<Arc<[Snapshot]>>,
) -> Option<Arc<[Snapshot]>> {
    if let Some(editing) = editing {
        Some(Arc::clone(editing))
    } else if pending {
        None
    } else {
        live()
    }
}

fn edit_snapshot_matches(snapshot: &EditSnapshot, generation: u64) -> bool {
    snapshot.generation == generation && snapshot.snapshots.first().is_some_and(|first| {
        !first.frame_range().is_empty() && snapshot.snapshots.iter().all(|next|
            next.frame_range() == first.frame_range() && next.sample_rate == first.sample_rate)
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

fn waveform_press(events: &[egui::Event]) -> Option<(Pos2, bool, bool)> {
    events.iter().rev().find_map(|event| match event {
        egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers } =>
            Some((*pos, modifiers.ctrl || modifiers.shift, modifiers.shift)),
        _ => None,
    })
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
    Hide,
    Show,
    Quit,
    Close,
}

impl MeterMode {
    const OPTIONS: [Self; 3] = [Self::Dbfs, Self::Vu, Self::Lufs];

    fn value(self, readings: MeterReadings) -> f32 {
        match self {
            Self::Dbfs => readings.peak_dbfs,
            Self::Vu => readings.vu,
            Self::Lufs => readings.momentary_lufs,
        }
    }

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
    snapshots: Arc<[Snapshot]>,
    frames: Range<usize>,
    output: PathBuf,
    profile: WavExportProfile,
    routes: [ChannelRoute; 2],
    force_mono: bool,
    flags: Option<FlagUpdate>,
}

impl WavClip {
    fn new(
        snapshots: Arc<[Snapshot]>,
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
        })
    }

    fn write(self, keep: bool) -> io::Result<ExportedWav> {
        let paths = export_routed_tracks_with_profile(
            &self.output,
            &self.snapshots,
            self.frames,
            self.profile,
            &self.routes[..self.snapshots.len()],
            self.force_mono,
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
        } else { (None, String::new()) };
        let retirement = match UiRetirement::new() {
            Ok(retirement) => Some(retirement),
            Err(error) => {
                if !preference_error.is_empty() { preference_error.push_str("; "); }
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
            saved_output: settings.saved_output,
            pinned: settings.pinned,
            next_pin_check: Instant::now(),
            waveform_only: settings.waveform_only,
            full_waveform_height: DEFAULT_WAVEFORM_HEIGHT,
            full_window_size: Vec2::new(610.0, 144.0),
            waveform_border: settings.waveform_border,
            waveform_border_color: settings.waveform_border_color.map(accent_color),
            waveform_border_follows_background: settings.waveform_border_follows_background,
            bar_lines: settings.bar_lines,
            second_lines: settings.second_lines,
            second_interval: settings.second_interval,
            options_open: false,
            options_in_taskbar: false,
            options_focus_requested: false,
            options_section: OptionsSection::Actions,
            available_sources: vec![CaptureSource::System, CaptureSource::DefaultMicrophone],
            source_list_worker: None,
            source_list_refresh_pending: false,
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
            last_interaction: Instant::now(),
            recording_started: None,
            peaks: std::array::from_fn(|_| WaveformPeaks::empty(DEFAULT_BINS)),
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
        if self.options_in_taskbar { return; }
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

    fn current_snapshots(&self) -> Option<Arc<[Snapshot]>> {
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
            retention_seconds: self.controller.retention().seconds(),
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
        let result = self.edit_request.as_ref().and_then(|request| match request.result.try_recv() {
            Ok(snapshots) => Some(snapshots),
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
            Err(mpsc::TryRecvError::Empty) => None,
        });
        let Some(snapshots) = result else { return; };
        let request = self.edit_request.take().expect("edit request was observed");
        let Some(snapshot) = snapshots.and_then(|snapshot| self.accept_edit_snapshot(snapshot, request.generation)) else {
            self.selection = None;
            self.gesture = None;
            self.export_requested = false;
            self.message = "No audio available for this selection.".into();
            return;
        };
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
        for save in request.saves {
            if let Some(clip) = save.clip(Arc::clone(&snapshots)) {
                self.pending_snapshot_saves.push_back(clip);
            }
        }
        self.editing = Some(FrozenEdit { snapshots, bars: snapshot.bar_markers,
            retention_seconds: request.retention_seconds });
        self.next_refresh = Instant::now();
        self.live_revision = None;
        if self.export_requested { self.start_export(); }
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
        if self.waveform_border { 1.0 } else { 0.0 }
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
        let resolution_changed = self.peaks[0].low.len() != bins;
        if !resolution_changed && (self.live_revision == Some(revision) || now < self.next_refresh) {
            return;
        }

        let (views, rows) = self.waveform_layout();
        let mut peaks = self.peaks.clone();
        for peak in &mut peaks {
            peak.low.resize(bins, 0.0);
            peak.high.resize(bins, 0.0);
        }
        let mut traces = peaks.each_mut().map(|peaks| {
            (peaks.low.as_mut_slice(), peaks.high.as_mut_slice())
        });
        let Some((history, visible, rate, bars)) =
            self.controller.try_render_waveforms(
                &views[..rows],
                self.zoom_frames.clone(),
                &mut traces[..rows],
            ) else {
                self.next_refresh = now + Duration::from_millis(16);
                return;
            };
        self.peaks = peaks;
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
        self.next_refresh = now + REFRESH;
    }

    fn is_fine_editing(&self) -> bool {
        self.gesture.is_some() || self.pan.is_some()
            || self.last_interaction.elapsed() < Duration::from_millis(250)
    }

    fn display_policy(&self, audio: Rect, visible: Range<u64>) -> waveform::DisplayPolicy {
        waveform::DisplayPolicy::for_view(audio.width(), audio.height(),
            visible.end.saturating_sub(visible.start), self.sample_rate, self.is_fine_editing())
    }

    fn refresh_edit_preview(&mut self) {
        let edit = self.editing.as_ref().expect("editing preview has immutable material");
        let history = edit.frames();
        let visible = self.zoom_frames.as_ref().map_or_else(|| history.clone(), |zoom|
            fit_frame_range(&history, zoom.start, zoom.end.saturating_sub(zoom.start)));
        let inner = self.timeline_rect(self.plot_rect);
        let width = if visible != history { inner.width() } else {
            let seconds = (history.end - history.start) as f32 / self.sample_rate.max(1) as f32;
            inner.width() * (seconds / edit.retention_seconds.max(1) as f32).clamp(0.0, 1.0)
        };
        let policy = self.display_policy(Rect::from_min_size(inner.min, Vec2::new(width, inner.height())), visible.clone());
        let (views, rows) = self.waveform_layout();
        let key = EditPreviewKey { frames: visible, views, rows, bins: policy.bins };
        if let Some(job) = &self.edit_preview {
            if !job.worker.is_finished() { return; }
        }
        if let Some(job) = self.edit_preview.take() {
            if let Ok(peaks) = job.worker.join() {
                if job.key == key {
                    // Publish the immutable material's range and its peaks together.
                    // A partially filled live view must keep its old right-aligned
                    // pointer geometry until this preview is actually ready.
                    let edit = self.editing.as_ref().expect("material remains available");
                    self.history_frames = edit.frames();
                    self.sample_rate = edit.snapshots[0].sample_rate;
                    self.bar_markers.clone_from(&edit.bars);
                    self.peaks = peaks;
                    self.visible_frames = key.frames.clone();
                    self.seconds = (key.frames.end - key.frames.start) as f32 / self.sample_rate.max(1) as f32;
                    self.edit_preview_key = Some(key.clone());
                }
            }
        }
        if self.edit_preview_key.as_ref() == Some(&key) { return; }
        let snapshots = Arc::clone(&self.editing.as_ref().expect("material remains available").snapshots);
        let render_key = key.clone();
        match thread::Builder::new().name("backtracking-selection-peaks".into())
            .spawn(move || edit_preview(&snapshots, &render_key))
        {
            Ok(worker) => self.edit_preview = Some(EditPreviewJob { key, worker }),
            Err(error) => self.message = format!("Could not prepare selection display: {error}"),
        }
    }

    fn waveform_layout(&self) -> ([(usize, WaveformView); 2], usize) {
        if self.dual_source() {
            ([(0, WaveformView::Merged), (1, WaveformView::Merged)], 2)
        } else if self.split_waveform && self.editing.as_ref()
            .and_then(|edit| edit.snapshots.first()).map(|snapshot| snapshot.channels)
            .or_else(|| self.controller.track_channels(0)) == Some(2) {
            ([(0, WaveformView::Left), (0, WaveformView::Right)], 2)
        } else {
            ([(0, WaveformView::Merged); 2], 1)
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui, interaction_blocked: bool) {
        ui.horizontal(|ui| {
            ui.heading("Backtracking");
            self.transport_button(ui, interaction_blocked);
            self.recording_indicator(ui);
            self.clear_button(ui, interaction_blocked);
            self.pin_button(ui, interaction_blocked);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.options_button(ui, !interaction_blocked && self.gesture.is_none());
                if self.dual_source() {
                    self.meter_readout(ui, 1);
                }
                self.meter_readout(ui, 0);
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
        let response = ui.add_enabled(can_toggle, icon_button());
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
        if paused {
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    center + Vec2::new(-3.5, -5.5),
                    center + Vec2::new(5.0, 0.0),
                    center + Vec2::new(-3.5, 5.5),
                ],
                color,
                Stroke::NONE,
            ));
        } else {
            for x in [-2.4, 2.4] {
                ui.painter().rect_filled(
                    Rect::from_center_size(center + Vec2::new(x, 0.0), Vec2::new(2.4, 10.0)),
                    0.5,
                    color,
                );
            }
        }
    }

    fn recording_indicator(&mut self, ui: &mut egui::Ui) {
        let (status, color) = if self.controller.error().is_some() {
            ("Capture unavailable", ERROR_LED)
        } else if self.controller.is_paused() {
            ("Paused", PAUSED_LED)
        } else if self.controller.is_recording() {
            let time = ui.input(|input| input.time);
            let started = *self.recording_started.get_or_insert(time);
            let lit = (time - started).rem_euclid(1.0) < 0.5;
            ("Recording", if lit { RECORDING_LIT } else { RECORDING_DIM })
        } else {
            ("Waiting for sound", WAITING_LED)
        };
        if !self.controller.is_recording() {
            self.recording_started = None;
        }

        let (rect, response) = ui.allocate_exact_size(Vec2::new(15.0, 24.0), egui::Sense::hover());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, status));
        response.on_hover_text(status);
        ui.painter().circle_filled(rect.center(), 4.5, color);
    }

    fn clear_button(&mut self, ui: &mut egui::Ui, menu_open: bool) {
        let enabled = !menu_open && self.gesture.is_none();
        let response = ui.add_enabled(enabled, icon_button());
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
        let value = self
            .controller
            .track_meter_readings(lane)
            .map(|readings| mode.value(readings));
        let number = if let Some(value) = value.filter(|value| value.is_finite()) {
            if mode == MeterMode::Vu {
                format!("{value:+.1}")
            } else {
                format!("{value:.1}")
            }
        } else {
            "—".into()
        };
        let text = if self.dual_source() {
            format!("{} {number} {}", lane_name(lane), mode.unit())
        } else {
            format!("{number} {}", mode.unit())
        };
        let width = if self.dual_source() { 91.0 } else { 102.0 };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 24.0), egui::Sense::hover());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, &text));
        response.on_hover_text(mode.description());
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
        let response = ui.add_enabled(!menu_open, icon_button());
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
            .add_enabled(enabled, icon_button())
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
        let title = if self.primary {
            tray::OPTIONS_TITLE.to_owned()
        } else {
            "Backtracking Capture Options".to_owned()
        };
        let builder = egui::ViewportBuilder::default()
            .with_title(&title)
            .with_inner_size([820.0, 640.0])
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
        let action = ctx.show_viewport_immediate(options_viewport(), builder, |ui, class| {
            if ui.input(|input| input.viewport().close_requested()) {
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
                ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
            }
            if ui.input(|input| input.pointer.any_pressed()) {
                self.last_interaction = Instant::now();
            }
            self.options_contents(ui, frame)
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
        let close = egui::Panel::bottom("options-footer")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(12))
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = ui.add_sized([96.0, 36.0], egui::Button::new("Close")).clicked();
                    if !self.message.is_empty() {
                        ui.add(egui::Label::new(&self.message).truncate()).on_hover_text(&self.message);
                    }
                    close
                }).inner
            }).inner;
        egui::Panel::left("options-navigation")
            .exact_size(190.0)
            .resizable(false)
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(12))
            .show(ui, |ui| {
                ui.label(options_heading_text("Backtracking", 18.0));
                ui.add_space(14.0);
                for (section, label) in OptionsSection::ALL {
                    if !self.primary && section == OptionsSection::Updates { continue; }
                    if ui.add_sized(
                        [ui.available_width(), 46.0],
                        egui::Button::new(if self.options_section == section {
                            options_heading_text(label, 15.0)
                        } else { egui::RichText::new(label).size(15.0) })
                            .selected(self.options_section == section).wrap(),
                    ).clicked() { self.options_section = section; }
                }
            });
        let action = egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(22))
            .show(ui, |ui| {
                ui.label(options_heading_text(self.options_section.title(), 22.0));
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(14.0);
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
                            OptionsSection::Updates => self.updates.show(ui).then_some(OptionsAction::Quit),
                        }
                    })).inner
            }).inner;
        if close { Some(OptionsAction::Close) } else { action }
    }

    fn snapshot_options(&mut self, ui: &mut egui::Ui) {
        options_group_heading(ui, "Snapshots");
        if ui.button("Save snapshot").clicked() { self.save_snapshot(); }
        if ui.button("New capture window").clicked() {
            let launched = std::env::current_exe().and_then(|exe| {
                Command::new(exe).arg("--new-capture-window").spawn()
            });
            if let Err(error) = launched {
                self.message = format!("Could not open capture window: {error}");
            }
        }
        if !self.primary { return; }
        ui.add_space(6.0);
        options_field(ui, "Global shortcut", |ui| {
            ui.label("Alt + the key left of 1");
            if let Some(error) = &self.hotkey_error {
                ui.add(egui::Label::new(format!("Could not register the shortcut: {error}")).wrap());
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
            let width = ui.available_width().min(680.0);
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
        if ui.add_enabled(can_toggle, egui::Button::new(transport)).clicked() { self.toggle_capture(); }
        if ui.add_enabled(self.selection.is_some(), egui::Button::new("Cancel selection")).clicked() {
            self.cancel_selection();
        }
        if ui.add_enabled(can_toggle, egui::Button::new("Clear capture")).clicked() { self.clear_capture(); }
        ui.add_space(6.0);
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
        let mut retain_silence = self.controller.retain_silence();
        if ui.checkbox(&mut retain_silence, "Keep quiet sections").changed() {
            self.controller.set_retain_silence(retain_silence);
            self.save_preference(Key::RetainSilence, u8::from(retain_silence).to_string());
        }
        options_field(ui, "Auto-resume delay", |ui| {
            if options_choice(ui, "auto-resume", &mut self.idle_resume,
                AutoResumeDelay::ALL.into_iter().map(|value| (value, format!("{} seconds", value.seconds())))) {
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
                    [1, 5, 10].into_iter().map(|value| (value, format!("{value} seconds")))) {
                    self.save_preference(Key::SecondsInterval, self.second_interval.to_string());
                }
            });
        });
        ui.separator();
        options_group_heading(ui, "Waveform border");
        if ui.checkbox(&mut self.waveform_border, "Show a thin border").changed() {
            self.save_preference(Key::WaveformBorder, u8::from(self.waveform_border).to_string());
        }
        ui.add_enabled_ui(self.waveform_border, |ui| {
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
        }
    }

    fn source_options(&mut self, ui: &mut egui::Ui) {
        let choices = self.available_sources.as_slice();
        let scanning = self.source_list_worker.is_some();
        let mut source_menu_opened = false;
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
                        let width = ui.available_width().min(680.0);
                        egui::ComboBox::from_id_salt(source_menu)
                            .selected_text(&label).width(width)
                            .height(options_menu_height(ui, choices.len() + usize::from(lane == 1)))
                            .truncate().show_ui(ui, |ui| {
                                configure_options_menu(ui, width);
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
                            let width = ui.available_width().min(680.0);
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
                    if let Some(status) = self.controller.source_status(lane) { ui.small(status); }
                    ui.add_space(8.0);
                    if lane == 0 { ui.separator(); }
                });
            }
        });
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
        match thread::Builder::new()
            .name("backtracking-sources".into())
            .spawn(Controller::available_sources)
        {
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
                Ok(sources) => self.available_sources = sources,
                Err(_) => self.message = "Source list worker stopped unexpectedly.".into(),
            }
            if std::mem::take(&mut self.source_list_refresh_pending) {
                self.refresh_source_list();
            }
        }
    }

    fn folder_options(&self, ui: &mut egui::Ui) -> Option<OptionsAction> {
        let waveform = options_field(ui, "Drag-and-drop folder", |ui| {
            ui.add(egui::Label::new(self.output.display().to_string()).truncate()).on_hover_text(self.output.display().to_string());
            ui.horizontal_wrapped(|ui| {
                let select = ui.button("Choose folder...").clicked();
                let open = ui.button("Open folder").clicked();
                if select { Some(OptionsAction::SelectSaveFolder) } else { open.then_some(OptionsAction::GoToSaveFolder) }
            }).inner
        });
        let snapshot = options_field(ui, "Saved snapshots folder", |ui| {
            ui.add(egui::Label::new(self.saved_output.display().to_string()).truncate()).on_hover_text(self.saved_output.display().to_string());
            ui.horizontal_wrapped(|ui| {
                let select = ui.button("Choose folder...").clicked();
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
        let visibility = if visible {
            ui.add_enabled(self.tray_available, egui::Button::new("Hide waveform")).clicked().then_some(OptionsAction::Hide)
        } else { ui.button("Show waveform").clicked().then_some(OptionsAction::Show) };
        let quit = ui.button("Quit Backtracking").clicked();
        visibility.or_else(|| quit.then_some(OptionsAction::Quit))
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
        self.message = match saved {
            Ok(()) => format!("Snapshot folder: {}", self.saved_output.display()),
            Err(error) => format!("Could not save settings: {error}"),
        };
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
        ui.add_space(8.0);
    }

    fn accent_settings(&mut self, ui: &mut egui::Ui) {
        options_field(ui, "Theme color", |ui| {
            let [r, g, b, _] = self.accent.to_array();
            let mut color = [r, g, b];
            let changed = ui.color_edit_button_srgb(&mut color).changed();
            let mut accent = accent_color(color);
            let reset = ui.button("Reset theme color").clicked();
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
            routes: self.controller.sources().each_ref().map(|settings| settings.as_ref()
                .map_or(ChannelRoute::Merged, |settings| settings.route)),
            force_mono: self.wav_force_mono,
            flags: (!has_selection).then(|| self.flag_update(0..0)),
        };
        if let Some(edit) = &self.editing {
            if let Some(clip) = spec.clip(Arc::clone(&edit.snapshots)) {
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
                result: self.controller.request_snapshots(None), saves: vec![spec],
            });
            self.message = "Preparing snapshot…".into();
        }
        if has_selection { self.clear_snapshot(); }
        self.collect_snapshot_saves();
    }

    fn wav_clip(&mut self, snapshots: Arc<[Snapshot]>, output: PathBuf) -> Option<WavClip> {
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
        WavClip::new(
            snapshots,
            self.selection.as_ref(),
            output,
            self.wav_profile,
            routes,
            self.wav_force_mono,
            flags,
        )
    }

    fn collect_snapshot_saves(&mut self) {
        let mut index = 0;
        while index < self.pending_edit_save_reads.len() {
            let result = match self.pending_edit_save_reads[index].result.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Disconnected) => None,
                Err(mpsc::TryRecvError::Empty) => { index += 1; continue; }
            };
            let read = self.pending_edit_save_reads.remove(index);
            if let Some(snapshot) = result.and_then(|snapshot| self.accept_edit_snapshot(snapshot, read.generation)) {
                for spec in read.saves {
                    if let Some(clip) = spec.clip(Arc::clone(&snapshot.snapshots)) {
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
                Err(mpsc::TryRecvError::Empty) => { index += 1; continue; }
            };
            let read = self.pending_snapshot_reads.remove(index);
            if let Some(snapshots) = result {
                for spec in read.saves {
                    if let Some(clip) = spec.clip(Arc::clone(&snapshots)) {
                        self.pending_snapshot_saves.push_back(clip);
                    } else {
                        self.message = "The selected audio is unavailable.".into();
                    }
                }
                self.retire(snapshots);
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
            .unwrap_or_else(|| self.controller.retention().seconds());
        let width = inner.width() * (history_seconds / retention.max(1) as f32)
            .clamp(0.0, 1.0);
        Rect::from_min_max(Pos2::new(inner.right() - width, inner.top()), inner.max)
    }

    fn interact_waveform(&mut self, ui: &egui::Ui, frame: &eframe::Frame, rect: Rect) {
        if self.history_frames.is_empty() { return; }
        if self.interact_timeline(ui, rect) {
            return;
        }
        let (position, press, down, snap) = ui.input(|input| {
            let press = waveform_press(&input.events);
            (
                input.pointer.interact_pos(),
                press,
                input.pointer.button_down(PointerButton::Primary),
                input.modifiers.shift,
            )
        });

        if self.gesture.is_none() && let Some((position, selecting, snap_on_press)) =
            press.filter(|(point, _, _)| rect.contains(*point))
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
        if steps != 0.0 && let Some(point) = hover.filter(|point| rect.contains(*point)) {
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
        if position.y < audio.top() - 2.0 || position.y > audio.top() + 13.0 {
            return None;
        }
        let flags = self.selection.as_ref().or_else(|| self.flags.as_ref().map(|flags| &flags.frames))?.clone();
        [(flags.start, flags.end), (flags.end, flags.start)]
            .into_iter()
            .filter_map(|(moved, fixed)| {
                let fraction = waveform::flag_fraction(moved, self.visible_frames.clone())?;
                let x = audio.left() + fraction as f32 * audio.width();
                let distance = (x - position.x).abs();
                (distance <= FLAG_SNAP_DISTANCE as f32).then_some((distance, fixed, moved))
            })
            .min_by(|(a, _, _), (b, _, _)| a.total_cmp(b))
            .map(|(_, fixed, moved)| {
                (fixed.clamp(self.history_frames.start, self.history_frames.end), moved)
            })
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
                Stroke::new(1.0, color), egui::StrokeKind::Inside
            );
        }
        let rect = rect.shrink(self.border_inset());
        let painter = ui.painter_at(rect);
        if self.waveform_only {
            painter.rect_filled(rect, 0.0, self.background_color(Color32::from_rgb(22, 26, 29)));
        } else {
            // The panel already supplies the chosen alpha when translucent.
            if self.transparency == 0 {
                painter.rect_filled(rect, 6.0, Color32::from_rgb(22, 26, 29));
            }
        }
        let audio = self.audio_rect(rect);
        if audio.width() <= 0.0 {
            if !self.waveform_only {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No captured audio",
                    egui::FontId::proportional(13.0),
                    Color32::GRAY,
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
        let (_, rows) = self.waveform_layout();
        let row_height = audio.height() / rows as f32;
        for (index, peaks) in self.peaks.iter().take(rows).enumerate() {
            let top = audio.top() + index as f32 * row_height;
            let row = Rect::from_min_size(
                Pos2::new(audio.left(), top),
                Vec2::new(audio.width(), row_height),
            );
            if !self.waveform_only {
                painter.hline(
                    row.x_range(),
                    row.center().y,
                    Stroke::new(1.0, Color32::from_rgb(47, 60, 64)),
                );
                if rows == 2 {
                    let label = if self.dual_source() {
                        lane_name(index)
                    } else if index == 0 {
                        "L"
                    } else {
                        "R"
                    };
                    painter.text(
                        Pos2::new(rect.left() + 1.0, row.center().y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::monospace(8.0),
                        Color32::from_gray(180),
                    );
                }
            }
            self.paint_waveform(&painter, row, peaks, highlight.as_ref());
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

    fn visible_guides(&self, audio: Rect) -> waveform::TimelineGuides {
        let bars = self.editing.as_ref().map_or(self.bar_markers.as_slice(), |edit| edit.bars.as_slice());
        waveform::timeline_guides(self.visible_frames.clone(), self.sample_rate,
            u32::from(self.second_interval), bars, self.bar_lines, self.second_lines,
            self.transparency < 100, &self.display_policy(audio, self.visible_frames.clone()))
    }

    fn paint_time_lines(&self, painter: &egui::Painter, audio: Rect) {
        let guides = self.visible_guides(audio);
        let policy = self.display_policy(audio, self.visible_frames.clone());
        let draw = |frame, alpha: u16, stroke| {
            if let Some(fraction) = waveform::flag_fraction(frame, self.visible_frames.clone()) {
                let x = audio.left() + fraction as f32 * audio.width();
                let alpha = alpha * u16::from(100 - self.transparency) / 100;
                painter.vline(x, audio.y_range(), Stroke::new(stroke, Color32::from_white_alpha(alpha as u8)));
            }
        };
        for frame in guides.seconds {
            draw(frame, 52, policy.seconds_stroke);
        }
        for frame in guides.bars {
            draw(frame, 104, policy.bar_stroke);
        }
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
        let [r, g, b, _] = self.accent.to_array();
        let selected = if [r, g, b]
            .into_iter()
            .any(|channel| channel.abs_diff(255 - channel) >= 80)
        {
            Color32::from_rgb(255 - r, 255 - g, 255 - b)
        } else {
            Color32::WHITE
        };
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
    match geometry {
        waveform::WaveformGeometry::Empty => {}
        waveform::WaveformGeometry::Samples(runs) => {
            for run in runs {
                let points: Vec<_> = run.iter().map(|point| position(point.x, point.value)).collect();
                if let [point] = points.as_slice() {
                    painter.circle_filled(*point, stroke * 0.55, color);
                } else if !points.is_empty() {
                    painter.add(egui::Shape::line(points, Stroke::new(stroke, color)));
                }
            }
        }
        waveform::WaveformGeometry::Envelope(runs) => {
            for run in runs {
                let mut mesh = egui::Mesh::default();
                let mut upper = Vec::with_capacity(run.len());
                let mut lower = Vec::with_capacity(run.len());
                let fill = color.linear_multiply(0.4);
                for (index, column) in run.iter().enumerate() {
                    let top = position(column.x, column.maximum);
                    let bottom = position(column.x, column.minimum);
                    mesh.colored_vertex(top, fill);
                    mesh.colored_vertex(bottom, fill);
                    upper.push(top);
                    lower.push(bottom);
                    if index > 0 {
                        let vertex = index as u32 * 2;
                        mesh.add_triangle(vertex - 2, vertex - 1, vertex);
                        mesh.add_triangle(vertex, vertex - 1, vertex + 1);
                    }
                }
                painter.add(egui::Shape::mesh(mesh));
                painter.add(egui::Shape::line(upper, Stroke::new(stroke, color)));
                painter.add(egui::Shape::line(lower, Stroke::new(stroke, color)));
            }
        }
    }
}

impl eframe::App for Recorder {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if input.viewport_id != egui::ViewportId::ROOT {
            return;
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
        let now = Instant::now();
        if self.pinned && !self.options_open && now >= self.next_pin_check {
            let attempted = frame
                .winit_window()
                .is_some_and(|window| window_order::raise_if_covered(window));
            self.next_pin_check = now + if attempted { PIN_RETRY } else { REFRESH };
        }
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
        self.controller.poll();
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
        if self.controller.is_paused()
            && self.gesture.is_none()
            && self.selection.is_none()
            && self.idle_resume != AutoResumeDelay::Immediate
            && self.last_interaction.elapsed()
                >= Duration::from_secs(u64::from(self.idle_resume.seconds()))
        {
            self.resume();
        }
        ctx.request_repaint_after(
            if self.window_drag.is_some()
                || self.gesture.is_some()
                || self.pan.is_some()
                || self.export.is_some()
                || self.edit_request.is_some()
                || self.edit_preview.is_some()
                || !self.pending_snapshot_reads.is_empty()
                || !self.pending_edit_save_reads.is_empty()
                || self.snapshot_save.is_some()
                || self.plugin_snapshot_request.is_some()
            {
                Duration::from_millis(16)
            } else {
                REFRESH
            },
        );
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
        egui::CentralPanel::default()
            .frame(frame_style)
            .show(ui, |ui| {
                let window_drag = self.window_drag.as_ref().map(|drag| drag.interaction);
                let interaction_blocked = window_drag.is_some();
                if !interaction_blocked
                    && self.gesture.is_none()
                    && ui.input(|input| input.pointer.button_clicked(PointerButton::Secondary))
                {
                    self.open_options(ui.ctx());
                }
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
                self.refresh_waveform();
                if !self.waveform_only {
                    self.full_waveform_height = rect.height() - 2.0 * WAVEFORM_INSET;
                    self.full_window_size = ui.ctx().viewport_rect().size();
                }
                if !interaction_blocked {
                    self.interact_waveform(ui, frame, rect.shrink(self.border_inset()));
                }
                self.draw_waveform(ui, rect, !interaction_blocked && window_cursor.is_none());
                let status = self.controller.error().unwrap_or_else(|| {
                    if self.edit_request.is_some() { "Preparing selection…" }
                    else if self.editing.is_some() && self.export.is_none() && self.snapshot_save.is_none() {
                        if self.controller.is_paused() { "Selection / Paused" }
                        else { "Selection" }
                    } else { &self.message }
                });
                if !self.waveform_only && !status.is_empty() {
                    ui.add(egui::Label::new(status).truncate())
                        .on_hover_text(status);
                }
                if let Some(cursor) = window_cursor {
                    ui.ctx().set_cursor_icon(cursor);
                } else if !interaction_blocked {
                    response.on_hover_cursor(CursorIcon::Grab);
                }
            });
        self.draw_options_window(ui.ctx(), frame);
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
                        if let Some(clip) = spec.clip(Arc::clone(&snapshot.snapshots)) {
                            self.pending_snapshot_saves.push_back(clip);
                        }
                    }
                }
            }
        }
        for read in self.pending_snapshot_reads.drain(..) {
            if let Ok(Some(snapshots)) = read.result.recv() {
                for spec in read.saves {
                    if let Some(clip) = spec.clip(Arc::clone(&snapshots)) {
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
            if let Err(error) = writer.flush() { eprintln!("Could not save settings on exit: {error}"); }
        }
    }
}

pub(crate) fn fit_frame_range(history: &Range<u64>, start: u64, count: u64) -> Range<u64> {
    let count = count.min(history.end - history.start);
    let start = start.clamp(history.start, history.end - count);
    start..start + count
}

fn lane_name(lane: usize) -> &'static str {
    if lane == 0 { "A" } else { "B" }
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

fn icon_button() -> egui::Button<'static> {
    egui::Button::new("").min_size(Vec2::splat(24.0))
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
        let retained: Arc<[Snapshot]> = vec![Snapshot {
            first_frame: 100,
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.1, 0.2, 0.3, 0.4],
        }].into();
        let newer: Arc<[Snapshot]> = vec![Snapshot {
            first_frame: 500,
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.9; 4],
        }].into();
        let source = snapshot_source(Some(&retained), false, || Some(newer)).unwrap();
        assert!(Arc::ptr_eq(&source, &retained));
        let clip = WavClip::new(source, Some(&(101..103)), PathBuf::new(),
            WavExportProfile::default(), [ChannelRoute::Merged; 2], false, None).unwrap();
        assert_eq!(clip.frames, 1..3);
        assert_eq!(clip.snapshots[0].frames(clip.frames.clone()), Some(&[0.2, 0.3][..]));
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
            generation, bar_markers: vec![101],
            snapshots: vec![
                Snapshot { first_frame: 100, sample_rate: 48000, channels: 1, samples: vec![0.1; 4] },
                Snapshot { first_frame: second_first, sample_rate: 48000, channels: 1, samples: vec![-0.1; 4] },
            ].into(),
        };
        assert!(!edit_snapshot_matches(&make_reply(2, 100), 1),
            "A reconnected source must not replace the material being selected");
        assert!(!edit_snapshot_matches(&make_reply(1, 101), 1),
            "A/B editing material must have exactly the same frame boundaries");
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
    fn selection_and_unread_snapshot_cleanup_run_off_the_calling_thread() {
        struct ObservedDrop<T> { _value: T, observed: mpsc::Sender<thread::ThreadId> }
        impl<T> Drop for ObservedDrop<T> {
            fn drop(&mut self) { let _ = self.observed.send(thread::current().id()); }
        }
        let retirement = UiRetirement::new().unwrap();
        let caller = thread::current().id();
        let snapshots: Arc<[Snapshot]> = vec![Snapshot { first_frame: 1, sample_rate: 48000,
            channels: 1, samples: vec![0.1; 4] }].into();
        let first = Arc::downgrade(&snapshots);
        let (observed, drops) = mpsc::channel();
        assert!(retirement.retire(ObservedDrop { _value: FrozenEdit { snapshots,
            bars: vec![1], retention_seconds: 600 }, observed: observed.clone() }).is_ok());
        assert_ne!(drops.recv_timeout(Duration::from_secs(2)).unwrap(), caller);

        let snapshots: Arc<[Snapshot]> = vec![Snapshot { first_frame: 1, sample_rate: 48000,
            channels: 1, samples: vec![0.2; 4] }].into();
        let second = Arc::downgrade(&snapshots);
        let (result, unread) = mpsc::channel();
        result.send(Some(snapshots)).unwrap();
        drop(result);
        assert!(retirement.retire(ObservedDrop { _value: unread, observed }).is_ok());
        assert_ne!(drops.recv_timeout(Duration::from_secs(2)).unwrap(), caller);
        drop(retirement);
        assert!(first.upgrade().is_none());
        assert!(second.upgrade().is_none(), "Discarding an unread reply must release its last audio reference on cleanup");
    }

    #[test]
    fn an_unstarted_wav_job_keeps_its_audio_available_for_background_retirement() {
        let snapshots: Arc<[Snapshot]> = vec![Snapshot { first_frame: 1, sample_rate: 48000,
            channels: 1, samples: vec![0.1; 4] }].into();
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
    fn shift_pointer_input_selects_and_snaps_while_ctrl_remains_free() {
        for (ctrl, shift, selecting, snap) in [
            (false, true, true, true), (true, false, true, false),
            (true, true, true, true), (false, false, false, false),
        ] {
            let context = egui::Context::default();
            let modifiers = egui::Modifiers { ctrl, shift, ..Default::default() };
            let point = Pos2::new(17.0, 27.0);
            context.run_ui(egui::RawInput {
                events: vec![egui::Event::PointerMoved(point), egui::Event::PointerButton {
                    pos: point, button: PointerButton::Primary, pressed: true, modifiers,
                }],
                ..Default::default()
            }, |ui| {
                let press = ui.input(|input| waveform_press(&input.events));
                assert_eq!(press, Some((point, selecting, snap)));
            }).drop_without_applying_deltas();
        }
    }

    #[test]
    fn pinned_preview_preserves_absolute_frames_and_independent_channels() {
        let stereo = Snapshot { first_frame: 1000, sample_rate: 48000, channels: 2,
            samples: vec![0.1, 0.7, 0.2, 0.6, 0.3, 0.5, 0.4, 0.4] };
        let key = EditPreviewKey { frames: 1001..1003,
            views: [(0, WaveformView::Left), (0, WaveformView::Right)], rows: 2, bins: 2 };
        let peaks = edit_preview(&[stereo], &key);
        assert_eq!(peaks[0].low, [0.2, 0.3]);
        assert_eq!(peaks[0].high, [0.2, 0.3]);
        assert_eq!(peaks[1].low, [0.6, 0.5]);
        let tracks = [
            Snapshot { first_frame: 1000, sample_rate: 48000, channels: 1, samples: vec![0.1,0.2,0.3,0.4] },
            Snapshot { first_frame: 1000, sample_rate: 48000, channels: 1, samples: vec![-0.1,-0.2,-0.3,-0.4] },
        ];
        let key = EditPreviewKey { views: [(0, WaveformView::Merged), (1, WaveformView::Merged)], ..key };
        let peaks = edit_preview(&tracks, &key);
        assert_eq!(peaks[0].low, [0.2, 0.3]);
        assert_eq!(peaks[1].high, [-0.2, -0.3]);
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
    fn waveform_pan_clamps_to_retained_frames_and_preserves_view_length() {
        assert_eq!(fit_frame_range(&(100..500), 170, 120), 170..290);
        assert_eq!(fit_frame_range(&(100..500), 0, 120), 100..220);
        assert_eq!(fit_frame_range(&(100..500), 900, 120), 380..500);
        assert_eq!(fit_frame_range(&(220..270), 200, 120), 220..270);
        assert_eq!(fit_frame_range(&(220..220), 900, 120), 220..220);
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
        let snapshots: Arc<[Snapshot]> = vec![
            Snapshot {
                first_frame: 10,
                sample_rate: 48_000,
                channels: 1,
                samples: vec![0.0, 0.5, 1.0],
            },
            Snapshot {
                first_frame: 10,
                sample_rate: 48_000,
                channels: 1,
                samples: vec![0.0, -0.5, -1.0],
            },
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
}

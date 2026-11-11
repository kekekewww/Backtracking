use std::collections::VecDeque;
use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::controller::Controller;
use crate::preferences::{
    DEFAULT_ACCENT, Key, MeterMode, Preferences, Settings, wav_sample_format_preference,
};
use crate::startup;
use crate::tray::{self, Tray};
use crate::window_order;
use backtracking_core::{
    AutoResumeDelay, DurationChoice, MeterReadings, Snapshot, WavExportProfile, WavSampleFormat,
    export_wav_with_profile, waveform,
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

const BINS: usize = 384;
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
const WRITING_WAV: &str = "Writing WAV…";

pub(crate) fn run(controller: Controller) -> eframe::Result<()> {
    let preferences = Preferences::for_current_user();
    let settings = preferences.load();
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/backtracking-icon.png"))
            .expect("decode the Backtracking icon");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([610.0, 144.0])
            .with_min_inner_size(min_window_size(settings.waveform_only))
            .with_icon(icon)
            .with_taskbar(false)
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
        "Backtracking",
        options,
        Box::new(|context| {
            configure_style(&context.egui_ctx, accent_color(settings.accent));
            if settings.waveform_only {
                set_waveform_frame(context.winit_window(), true);
            }
            if let (Some(window), Some(position)) =
                (context.winit_window(), settings.window_position)
            {
                window_order::restore_position(window, position);
            }
            let mut recorder = Recorder::new(controller, preferences, settings);
            if let Some(window) = context.winit_window() {
                recorder.attach_tray(window, &context.egui_ctx);
            }
            match startup::enabled() {
                Ok(enabled) => recorder.launch_at_login = enabled,
                Err(error) => recorder.message = format!("Could not read startup setting: {error}"),
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
    preferences: Preferences,
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
    options_open: bool,
    options_focus_requested: bool,
    window_position: Option<[i32; 2]>,
    window_position_changed: Option<Instant>,
    idle_resume: AutoResumeDelay,
    transparency: u8,
    accent: Color32,
    saved_accent: Color32,
    meter_mode: MeterMode,
    wav_profile: WavExportProfile,
    last_interaction: Instant,
    recording_started: Option<f64>,
    low: [f32; BINS],
    high: [f32; BINS],
    seconds: f32,
    live_revision: Option<u64>,
    next_refresh: Instant,
    snapshot: Option<Arc<Snapshot>>,
    selection: Option<Range<f32>>,
    gesture: Option<Gesture>,
    window_drag: Option<WindowDrag>,
    export: Option<ExportJob>,
    pending_exports: VecDeque<ExportClip>,
    snapshot_save: Option<JoinHandle<io::Result<PathBuf>>>,
    pending_snapshot_saves: VecDeque<WavClip>,
    clip_revision: u64,
    exported: Option<ExportedWav>,
    message: String,
}

enum Gesture {
    Select { anchor: f32, cursor: f32 },
    Export { origin: Pos2, drag_requested: bool },
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

struct ExportClip {
    revision: u64,
    wav: WavClip,
    immediate: bool,
    provisional: bool,
}

impl ExportClip {
    fn write(self) -> io::Result<ExportedWav> {
        Ok(ExportedWav {
            path: self.wav.write()?,
            provisional: self.provisional,
        })
    }
}

struct WavClip {
    snapshot: Arc<Snapshot>,
    frames: Range<usize>,
    output: PathBuf,
    profile: WavExportProfile,
}

impl WavClip {
    fn new(
        snapshot: Arc<Snapshot>,
        selection: Option<&Range<f32>>,
        output: PathBuf,
        profile: WavExportProfile,
    ) -> Option<Self> {
        let frames = snapshot.frame_count();
        if frames == 0 {
            return None;
        }
        let frames = selection.map_or(0..frames, |selection| {
            let start = ((selection.start * frames as f32).floor() as usize).min(frames - 1);
            let end = ((selection.end * frames as f32).ceil() as usize).clamp(start + 1, frames);
            start..end
        });
        Some(Self {
            snapshot,
            frames,
            output,
            profile,
        })
    }

    fn write(self) -> io::Result<PathBuf> {
        let samples = self
            .snapshot
            .frames(self.frames)
            .ok_or_else(|| io::Error::other("invalid audio selection"))?;
        export_wav_with_profile(
            &self.output,
            self.snapshot.sample_rate,
            self.snapshot.channels,
            samples,
            self.profile,
        )
    }
}

struct ExportedWav {
    path: PathBuf,
    provisional: bool,
}

impl Drop for ExportedWav {
    fn drop(&mut self) {
        // A selected drag needs a real file path before OLE starts. Keep it only after a drop.
        if self.provisional {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Recorder {
    fn new(mut controller: Controller, preferences: Preferences, settings: Settings) -> Self {
        controller.set_retention(settings.history);
        Self {
            controller,
            preferences,
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
            options_open: false,
            options_focus_requested: false,
            window_position: settings.window_position,
            window_position_changed: None,
            idle_resume: settings.idle_resume,
            transparency: settings.transparency,
            accent: accent_color(settings.accent),
            saved_accent: accent_color(settings.accent),
            meter_mode: settings.meter_mode,
            wav_profile: settings.wav_profile,
            last_interaction: Instant::now(),
            recording_started: None,
            low: [0.0; BINS],
            high: [0.0; BINS],
            seconds: 0.0,
            live_revision: None,
            next_refresh: Instant::now(),
            snapshot: None,
            selection: None,
            gesture: None,
            window_drag: None,
            export: None,
            pending_exports: VecDeque::new(),
            snapshot_save: None,
            pending_snapshot_saves: VecDeque::new(),
            clip_revision: 0,
            exported: None,
            message: String::new(),
        }
    }

    fn save_preference(&mut self, key: Key, value: impl AsRef<str>) {
        if let Err(error) = self.preferences.save(key, value.as_ref()) {
            self.message = format!("Could not save settings: {error}");
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
            self.resume();
        }
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    fn attach_tray(&mut self, window: &Window, ctx: &egui::Context) {
        self.next_tray_attach = Instant::now() + Duration::from_secs(2);
        match Tray::attach(window, ctx) {
            Ok(tray) => {
                self.tray_available = tray.is_active();
                self.tray = Some(tray);
            }
            Err(error) => self.message = format!("System tray unavailable: {error}"),
        }
        tray::suppress_taskbar(window);
    }

    fn update_tray(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
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
                let enabled = startup::enabled().unwrap_or(self.launch_at_login);
                self.set_launch_at_login(!enabled, frame);
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
        self.last_interaction = Instant::now();
        if !self.controller.is_paused() {
            self.clear_snapshot();
        }
    }

    fn freeze(&mut self) {
        self.controller.pause();
        self.last_interaction = Instant::now();
        if self.snapshot.is_none()
            && let Some(snapshot) = self.controller.snapshot()
        {
            self.advance_clip_revision();
            self.seconds = snapshot.frame_count() as f32 / snapshot.sample_rate.max(1) as f32;
            waveform::peaks(
                &snapshot.samples,
                snapshot.channels,
                &mut self.low,
                &mut self.high,
            );
            self.snapshot = Some(Arc::new(snapshot));
        }
    }

    fn clear_snapshot(&mut self) {
        self.reset_snapshot();
        self.invalidate_export();
    }

    fn reset_snapshot(&mut self) {
        self.snapshot = None;
        self.selection = None;
        self.live_revision = None;
        self.next_refresh = Instant::now();
    }

    fn finish_waveform_gesture(&mut self, keep_selection: bool) {
        self.last_interaction = Instant::now();
        if self.idle_resume == AutoResumeDelay::Immediate {
            self.controller.resume();
            if !self.controller.is_paused() && !keep_selection {
                // Let the current WAV finish. A new snapshot will invalidate its cache.
                self.reset_snapshot();
            }
        }
    }

    fn clear_capture(&mut self) {
        self.controller.clear();
        self.clear_snapshot();
        self.message.clear();
        self.last_interaction = Instant::now();
    }

    fn cancel_selection(&mut self) {
        self.gesture = None;
        self.clear_snapshot();
        self.message.clear();
        self.controller.resume();
        self.last_interaction = Instant::now();
    }

    fn background_color(&self, color: Color32) -> Color32 {
        let alpha = ((100 - u16::from(self.transparency)) * 255 + 50) / 100;
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha as u8)
    }

    fn invalidate_export(&mut self) {
        self.advance_clip_revision();
        self.pending_exports
            .retain(|clip| clip.immediate && !clip.provisional);
    }

    fn advance_clip_revision(&mut self) {
        self.clip_revision = self.clip_revision.wrapping_add(1);
        self.exported = None;
        if self.message == WRITING_WAV {
            self.message.clear();
        }
    }

    fn refresh_waveform(&mut self) {
        let revision = self.controller.revision();
        let now = Instant::now();
        if self.snapshot.is_none()
            && self.live_revision != Some(revision)
            && now >= self.next_refresh
        {
            let (frames, sample_rate) = self.controller.waveform(&mut self.low, &mut self.high);
            self.seconds = frames as f32 / sample_rate.max(1) as f32;
            self.live_revision = Some(revision);
            self.next_refresh = now + REFRESH;
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
                self.meter_readout(ui);
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

    fn meter_readout(&self, ui: &mut egui::Ui) {
        let value = self
            .controller
            .meter_readings()
            .map(|readings| self.meter_mode.value(readings));
        let number = if let Some(value) = value.filter(|value| value.is_finite()) {
            if self.meter_mode == MeterMode::Vu {
                format!("{value:+.1}")
            } else {
                format!("{value:.1}")
            }
        } else {
            "—".into()
        };
        let text = format!("{number} {}", self.meter_mode.unit());
        let (rect, response) = ui.allocate_exact_size(Vec2::new(102.0, 24.0), egui::Sense::hover());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, &text));
        response.on_hover_text(self.meter_mode.description());
        let painter = ui.painter();
        painter.rect_filled(rect, 5.0, ui.visuals().widgets.inactive.bg_fill);
        painter.text(
            rect.right_center() - Vec2::new(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            text,
            egui::FontId::monospace(12.0),
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
        if !self.waveform_only {
            let size = ctx.viewport_rect().size();
            let expanded = size.max(minimum);
            if expanded != size {
                fit_expanded_window(ctx, frame, expanded);
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(expanded));
            }
        }
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
        let builder = egui::ViewportBuilder::default()
            .with_title(tray::OPTIONS_TITLE)
            .with_inner_size([465.0, 540.0])
            .with_min_inner_size([375.0, 300.0])
            .with_maximize_button(false)
            .with_taskbar(false)
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
                tray::prepare_options_window();
                ui.ctx().send_viewport_cmd(ViewportCommand::Visible(true));
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Minimized(false));
                ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
            }
            self.handle_snapshot_shortcut(ui.ctx());
            if ui.input(|input| input.pointer.any_pressed()) {
                self.last_interaction = Instant::now();
            }
            self.options_contents(ui, frame)
        });
        if self.waveform_only != waveform_only {
            self.apply_waveform_mode(ctx, frame);
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
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(16))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::CollapsingHeader::new("Capture")
                            .default_open(true)
                            .show(ui, |ui| {
                                self.capture_options(ui);
                            });
                        egui::CollapsingHeader::new("Appearance")
                            .default_open(true)
                            .show(ui, |ui| {
                                self.appearance_options(ui);
                            });
                        egui::CollapsingHeader::new("WAV export")
                            .default_open(true)
                            .show(ui, |ui| {
                                egui::Grid::new("wav-options").show(ui, |ui| self.wav_settings(ui));
                                if ui.button("Save snapshot (Ctrl+S)").clicked() {
                                    self.save_snapshot();
                                }
                            });
                        let folders = egui::CollapsingHeader::new("Save folders")
                            .default_open(true)
                            .show(ui, |ui| self.folder_options(ui))
                            .body_returned
                            .flatten();
                        let app = egui::CollapsingHeader::new("App")
                            .default_open(true)
                            .show(ui, |ui| self.app_options(ui, frame))
                            .body_returned
                            .flatten();
                        if !self.message.is_empty() {
                            ui.label(&self.message);
                        }
                        folders.or(app)
                    })
                    .inner
            })
            .inner
    }

    fn capture_options(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let can_toggle = self.gesture.is_none();
            let transport = if self.controller.error().is_some() {
                "Retry capture"
            } else if self.controller.is_paused() {
                "Play"
            } else {
                "Pause"
            };
            if ui
                .add_enabled(can_toggle, egui::Button::new(transport))
                .clicked()
            {
                self.toggle_capture();
            }
            if ui
                .add_enabled(can_toggle, egui::Button::new("Clear capture"))
                .clicked()
            {
                self.clear_capture();
            }
            if ui
                .add_enabled(
                    self.selection.is_some(),
                    egui::Button::new("Cancel selection"),
                )
                .clicked()
            {
                self.cancel_selection();
            }
        });
        egui::Grid::new("capture-options").show(ui, |ui| {
            ui.label("History");
            ui.horizontal(|ui| {
                let mut retention = self.controller.retention();
                for value in DurationChoice::ALL {
                    ui.selectable_value(&mut retention, value, format!("{} s", value.seconds()));
                }
                if retention != self.controller.retention() {
                    self.controller.set_retention(retention);
                    self.clear_snapshot();
                    self.save_preference(Key::History, retention.seconds().to_string());
                }
            });
            ui.end_row();
            ui.label("Auto-resume");
            ui.horizontal(|ui| {
                let before = self.idle_resume;
                for value in AutoResumeDelay::ALL {
                    ui.selectable_value(
                        &mut self.idle_resume,
                        value,
                        format!("{} s", value.seconds()),
                    );
                }
                if self.idle_resume != before {
                    self.save_preference(Key::IdleResume, self.idle_resume.seconds().to_string());
                }
            });
            ui.end_row();
        });
    }

    fn appearance_options(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.waveform_only, "Show waveform only");
        ui.checkbox(&mut self.pinned, "Keep on top");
        egui::Grid::new("appearance-options").show(ui, |ui| {
            ui.label("Background transparency");
            ui.horizontal(|ui| {
                let before = self.transparency;
                for value in TRANSPARENCIES {
                    ui.selectable_value(&mut self.transparency, value, format!("{value}%"));
                }
                if self.transparency != before {
                    self.save_preference(Key::Transparency, self.transparency.to_string());
                }
            });
            ui.end_row();
            ui.label("Meter");
            ui.horizontal(|ui| {
                let before = self.meter_mode;
                for mode in MeterMode::OPTIONS {
                    ui.selectable_value(&mut self.meter_mode, mode, mode.unit())
                        .on_hover_text(mode.description());
                }
                if self.meter_mode != before {
                    self.save_preference(Key::Meter, self.meter_mode.unit());
                }
            });
            ui.end_row();
        });
        self.accent_settings(ui);
    }

    fn folder_options(&self, ui: &mut egui::Ui) -> Option<OptionsAction> {
        let waveform = ui
            .horizontal(|ui| {
                let select = ui.button("Select save folder").clicked();
                let open = ui
                    .button("Go to save folder")
                    .on_hover_text(self.output.display().to_string())
                    .clicked();
                if select {
                    Some(OptionsAction::SelectSaveFolder)
                } else {
                    open.then_some(OptionsAction::GoToSaveFolder)
                }
            })
            .inner;
        let snapshot = ui
            .horizontal(|ui| {
                let select = ui.button("Select snapshot folder").clicked();
                let open = ui
                    .button("Go to snapshot folder")
                    .on_hover_text(self.saved_output.display().to_string())
                    .clicked();
                if select {
                    Some(OptionsAction::SelectSnapshotFolder)
                } else {
                    open.then_some(OptionsAction::OpenSnapshotFolder)
                }
            })
            .inner;
        waveform.or(snapshot)
    }

    fn app_options(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) -> Option<OptionsAction> {
        let mut enabled = self.launch_at_login;
        if ui
            .checkbox(&mut enabled, "Start with Windows")
            .on_hover_text("Launch Backtracking from its current location when you sign in")
            .changed()
        {
            self.set_launch_at_login(enabled, frame);
        }
        let visible = frame
            .winit_window()
            .is_none_or(|window| !window_is_hidden(window));
        ui.horizontal(|ui| {
            let visibility = if visible {
                ui.add_enabled(self.tray_available, egui::Button::new("Hide waveform"))
                    .clicked()
                    .then_some(OptionsAction::Hide)
            } else {
                ui.button("Show waveform")
                    .clicked()
                    .then_some(OptionsAction::Show)
            };
            let quit = ui.button("Quit Backtracking").clicked();
            let close = ui.button("Close Options").clicked();
            visibility
                .or_else(|| quit.then_some(OptionsAction::Quit))
                .or_else(|| close.then_some(OptionsAction::Close))
        })
        .inner
    }

    fn select_save_folder(&mut self, frame: &eframe::Frame) {
        let was_paused = self.controller.is_paused();
        self.controller.pause();
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
                .and_then(|path| self.preferences.save(Key::Output, path));
            self.message = match saved {
                Ok(()) => format!("Output: {}", self.output.display()),
                Err(error) => format!("Could not save settings: {error}"),
            };
        }
        self.last_interaction = Instant::now();
        if !was_paused && self.idle_resume == AutoResumeDelay::Immediate {
            self.controller.resume();
        }
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
            .and_then(|path| self.preferences.save(Key::SavedSnapshots, path));
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
        ui.label("WAV rate");
        ui.horizontal(|ui| {
            for (rate, label) in WAV_SAMPLE_RATES {
                ui.selectable_value(&mut self.wav_profile.sample_rate, rate, label);
            }
        });
        ui.end_row();
        ui.label("WAV encoding");
        ui.horizontal(|ui| {
            for (format, label) in WAV_SAMPLE_FORMATS {
                ui.selectable_value(&mut self.wav_profile.sample_format, format, label);
            }
        });
        ui.end_row();

        if self.wav_profile != previous {
            self.invalidate_export();
            self.message.clear();
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
        ui.collapsing("Theme color", |ui| {
            ui.spacing_mut().slider_width = 112.0;
            let mut accent = self.accent;
            let changed = egui::color_picker::color_picker_color32(
                ui,
                &mut accent,
                egui::color_picker::Alpha::Opaque,
            );
            if ui.button("Reset color").clicked() {
                accent = accent_color(DEFAULT_ACCENT);
            } else if !changed {
                return;
            }
            if accent != self.accent {
                self.accent = accent;
                apply_accent(ui.ctx(), accent);
            }
        });
    }

    fn handle_snapshot_shortcut(&mut self, ctx: &egui::Context) {
        let requests = ctx.input_mut(|input| {
            if !input.focused {
                return 0;
            }
            let mut requests = 0;
            input.events.retain(|event| {
                if !snapshot_save_key(event) {
                    return true;
                }
                requests += usize::from(matches!(event, egui::Event::Key { repeat: false, .. }));
                false
            });
            requests
        });
        for _ in 0..requests {
            self.save_snapshot();
        }
    }

    fn save_snapshot(&mut self) {
        if self.gesture.is_some() {
            return;
        }
        let selection = self.selection.as_ref();
        let snapshot = if selection.is_some() {
            self.snapshot.clone()
        } else {
            self.controller.poll();
            self.controller.snapshot().map(Arc::new)
        };
        let Some(clip) = snapshot.and_then(|snapshot| {
            WavClip::new(
                snapshot,
                selection,
                self.saved_output.clone(),
                self.wav_profile,
            )
        }) else {
            self.message = "No audio captured yet.".into();
            return;
        };
        self.pending_snapshot_saves.push_back(clip);
        if selection.is_some() || self.controller.is_paused() {
            self.resume();
        }
        self.collect_snapshot_saves();
    }

    fn collect_snapshot_saves(&mut self) {
        if self
            .snapshot_save
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            return;
        }
        if let Some(worker) = self.snapshot_save.take() {
            self.message = match worker.join() {
                Ok(Ok(path)) => format!("Saved snapshot {}", path.display()),
                Ok(Err(error)) => format!("Could not save snapshot: {error}"),
                Err(_) => "Snapshot worker stopped unexpectedly.".into(),
            };
        }
        if let Some(clip) = self.pending_snapshot_saves.pop_front() {
            match thread::Builder::new()
                .name("backtracking-snapshot".into())
                .spawn(move || clip.write())
            {
                Ok(worker) => {
                    self.snapshot_save = Some(worker);
                    self.message = "Saving snapshot…".into();
                }
                Err(error) => self.message = format!("Could not start snapshot save: {error}"),
            }
        }
    }

    fn start_export(&mut self, provisional: bool) {
        let revision = self.clip_revision;
        if self.exported.is_some()
            || self
                .export
                .as_ref()
                .is_some_and(|job| job.revision == revision)
            || self
                .pending_exports
                .iter()
                .any(|clip| clip.revision == revision)
        {
            return;
        }
        let Some(wav) = self.snapshot.clone().and_then(|snapshot| {
            WavClip::new(
                snapshot,
                self.selection.as_ref(),
                self.output.clone(),
                self.wav_profile,
            )
        }) else {
            self.message = "No audio captured yet.".into();
            return;
        };
        let clip = ExportClip {
            revision,
            wav,
            immediate: self.idle_resume == AutoResumeDelay::Immediate,
            provisional,
        };
        if self.export.is_none() && self.pending_exports.is_empty() {
            self.spawn_export(clip);
        } else {
            if !clip.immediate {
                self.pending_exports.retain(|clip| clip.immediate);
            }
            self.pending_exports.push_back(clip);
        }
    }

    fn spawn_export(&mut self, clip: ExportClip) {
        let revision = clip.revision;
        match thread::Builder::new()
            .name("backtracking-wav".into())
            .spawn(move || clip.write())
        {
            Ok(worker) => {
                self.export = Some(ExportJob { worker, revision });
                if revision == self.clip_revision {
                    self.message = WRITING_WAV.into();
                }
            }
            Err(error) => self.message = format!("Could not start export: {error}"),
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
                    Ok(Ok(file)) => {
                        if file.provisional {
                            self.message.clear();
                        } else {
                            self.message = format!("Saved {}", file.path.display());
                        }
                        self.exported = Some(file);
                    }
                    Ok(Err(error)) => self.message = format!("Could not save WAV: {error}"),
                    Err(_) => self.message = "WAV export worker stopped unexpectedly.".into(),
                }
            }
        }
        if let Some(clip) = self.pending_exports.pop_front() {
            self.spawn_export(clip);
        }
    }

    fn audio_rect(&self, rect: Rect) -> Rect {
        let inner = if self.waveform_only {
            let inner = rect.shrink2(Vec2::new(WAVEFORM_INSET, 2.0));
            Rect::from_center_size(
                inner.center(),
                Vec2::new(inner.width(), self.full_waveform_height.min(inner.height())),
            )
        } else {
            rect.shrink(WAVEFORM_INSET)
        };
        let width = inner.width()
            * (self.seconds / self.controller.retention().seconds() as f32).clamp(0.0, 1.0);
        Rect::from_min_max(Pos2::new(inner.right() - width, inner.top()), inner.max)
    }

    fn interact_waveform(
        &mut self,
        ui: &egui::Ui,
        frame: &eframe::Frame,
        response: &egui::Response,
    ) {
        let rect = response.rect;
        let (position, pressed, down, selecting) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.button_pressed(PointerButton::Primary),
                input.pointer.button_down(PointerButton::Primary),
                input.modifiers.ctrl,
            )
        });

        if pressed
            && response.contains_pointer()
            && let Some(position) = position
        {
            if selecting && !self.controller.is_paused() {
                self.reset_snapshot();
            }
            self.freeze();
            if selecting {
                let start = fraction(self.audio_rect(rect), position.x);
                self.gesture = Some(Gesture::Select {
                    anchor: start,
                    cursor: start,
                });
            } else {
                if self.selection.is_none() {
                    self.start_export(false);
                }
                self.gesture = Some(Gesture::Export {
                    origin: position,
                    drag_requested: false,
                });
            }
        }

        let audio_rect = self.audio_rect(rect);
        if let Some(Gesture::Select { cursor, .. }) = &mut self.gesture
            && let Some(position) = position
        {
            *cursor = fraction(audio_rect, position.x);
        }

        if down {
            if let (
                Some(Gesture::Export {
                    origin,
                    drag_requested,
                }),
                Some(position),
            ) = (&mut self.gesture, position)
                && !*drag_requested
                && position.distance(*origin) >= 4.0
            {
                *drag_requested = true;
                if self.selection.is_some() {
                    self.start_export(true);
                }
            }
            if matches!(
                self.gesture,
                Some(Gesture::Export {
                    drag_requested: true,
                    ..
                })
            ) && let Some(file) = &mut self.exported
            {
                let dropped = Arc::new(AtomicBool::new(false));
                let drop_outcome = Arc::clone(&dropped);
                // OLE holds the UI thread until the mouse is released.
                let result = drag::start_drag(
                    frame,
                    drag::DragItem::Files(vec![file.path.clone()]),
                    drag::Image::Raw(DRAG_ICON.to_vec()),
                    move |result, _| {
                        drop_outcome.store(
                            matches!(result, drag::DragResult::Dropped),
                            Ordering::Relaxed,
                        );
                    },
                    drag::Options::default(),
                );
                self.gesture = None;
                if let Err(error) = result {
                    let keep_selection = self.selection.is_some();
                    if keep_selection {
                        self.invalidate_export();
                    }
                    self.finish_waveform_gesture(keep_selection);
                    self.message = format!("Could not start file drag: {error}");
                } else if dropped.load(Ordering::Relaxed) {
                    file.provisional = false;
                    self.message = format!("Saved {}", file.path.display());
                    self.finish_waveform_gesture(false);
                } else if self.selection.is_some() {
                    self.cancel_selection();
                } else {
                    self.finish_waveform_gesture(false);
                }
            }
        } else if let Some(gesture) = self.gesture.take() {
            let keep_selection = match gesture {
                Gesture::Select { anchor, cursor } => {
                    let range = anchor.min(cursor)..anchor.max(cursor);
                    if range.start < range.end {
                        self.selection = Some(range);
                        self.invalidate_export();
                    }
                    self.selection.is_some()
                }
                Gesture::Export { drag_requested, .. } if self.selection.is_some() => {
                    if drag_requested {
                        // The button was released before the WAV was ready to drag.
                        self.invalidate_export();
                    } else {
                        self.start_export(false);
                    }
                    drag_requested
                }
                Gesture::Export { .. } => false,
            };
            self.finish_waveform_gesture(keep_selection);
        }
    }

    fn draw_waveform(&self, ui: &egui::Ui, rect: Rect, preview_hover: bool) {
        let painter = ui.painter_at(rect);
        if !self.waveform_only {
            // The panel already supplies the chosen alpha when translucent.
            if self.transparency == 0 {
                painter.rect_filled(rect, 6.0, Color32::from_rgb(22, 26, 29));
            }
            let inner = rect.shrink(WAVEFORM_INSET);
            painter.hline(
                inner.x_range(),
                inner.center().y,
                Stroke::new(1.0, Color32::from_rgb(47, 60, 64)),
            );
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
            Some(Gesture::Select { anchor, cursor }) => {
                Some(anchor.min(cursor)..anchor.max(cursor))
            }
            _ => self.selection.clone(),
        };
        let full_preview = !self.waveform_only
            && selection.is_none()
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

        self.paint_waveform(&painter, audio, selection.as_ref());

        if !self.waveform_only
            && let Some(range) = selection.or_else(|| full_preview.then_some(0.0..1.0))
        {
            painter.text(
                audio.right_top(),
                egui::Align2::RIGHT_TOP,
                format!("{:.2} s", (range.end - range.start) * self.seconds),
                egui::FontId::monospace(11.0),
                Color32::WHITE,
            );
        }
    }

    fn paint_waveform(&self, painter: &egui::Painter, audio: Rect, selection: Option<&Range<f32>>) {
        let middle = audio.center().y;
        let extent = audio.height() * 0.47;
        let [r, g, b, _] = self.accent.to_array();
        let selected = if [r, g, b]
            .into_iter()
            .any(|channel| channel.abs_diff(255 - channel) >= 80)
        {
            Color32::from_rgb(255 - r, 255 - g, 255 - b)
        } else {
            Color32::WHITE
        };
        for (bin, (&low, &high)) in self.low.iter().zip(&self.high).enumerate() {
            let fraction = (bin as f32 + 0.5) / BINS as f32;
            let position = audio.left() + fraction * audio.width();
            let color = if selection.is_some_and(|range| range.contains(&fraction)) {
                selected
            } else {
                self.accent
            };
            let top = middle - high.clamp(-1.0, 1.0) * extent;
            let bottom = middle - low.clamp(-1.0, 1.0) * extent;
            painter.line_segment(
                [
                    Pos2::new(position, top),
                    Pos2::new(position, bottom.max(top + 0.4)),
                ],
                Stroke::new(1.0, color),
            );
        }
    }

    fn draw_action_hint(&self, ui: &egui::Ui, response: &egui::Response) {
        if self.seconds <= 0.0 || !response.contains_pointer() {
            return;
        }
        let (position, modifier) =
            ui.input(|input| (input.pointer.hover_pos(), input.modifiers.ctrl));
        let Some(position) = position.filter(|point| response.rect.contains(*point)) else {
            return;
        };
        let hint = match self.gesture {
            Some(Gesture::Select { .. }) => "Drag to Select",
            Some(Gesture::Export { .. }) => "Drag and Drop",
            None if modifier => "Drag to Select",
            None if self.exported.is_some() || self.selection.is_some() => {
                "Drag and Drop   ·   Ctrl + Click"
            }
            None => "Click   ·   Ctrl + Click",
        };
        let bounds = ui.ctx().content_rect().shrink(5.0);
        let painter = ui.painter().with_clip_rect(bounds);
        let foreground = Color32::from_rgb(234, 243, 240);
        let text = painter.layout_no_wrap(
            hint.to_owned(),
            egui::FontId::new(
                12.0,
                egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into()),
            ),
            foreground,
        );
        let padding = Vec2::new(9.0, 5.0);
        let size = text.size() + padding * 2.0;
        let [x, y] = waveform::hint_position(
            [position.x, position.y],
            [size.x, size.y],
            [bounds.left(), bounds.top(), bounds.right(), bounds.bottom()],
        );
        let bubble = Rect::from_min_size(Pos2::new(x, y), size);
        painter.rect(
            bubble,
            5.0,
            Color32::from_rgb(37, 43, 46),
            Stroke::new(1.0, muted_accent(self.accent)),
            egui::StrokeKind::Inside,
        );
        painter.galley(bubble.min + padding, text, foreground);
    }
}

impl eframe::App for Recorder {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if input.focused && self.selection.is_some() && input.events.iter().any(snapshot_save_key) {
            self.last_interaction = Instant::now();
        }
        if input.viewport_id != egui::ViewportId::ROOT {
            return;
        }
        let viewport = input.viewport();
        if viewport.minimized == Some(true) || viewport.fullscreen == Some(true) || !input.focused {
            self.window_drag = None;
            return;
        }
        let can_resize = viewport.maximized != Some(true);
        let menu_open = egui::Popup::is_any_open(ctx);
        if !menu_open
            && (self.selection.is_some() || matches!(self.gesture, Some(Gesture::Select { .. })))
        {
            let cancel_key = |event: &egui::Event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Escape | egui::Key::Backspace,
                        pressed: true,
                        ..
                    }
                )
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
        self.update_tray(ctx, frame);
        let now = Instant::now();
        if self.pinned && now >= self.next_pin_check {
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
        if !self.controller.is_recording() {
            self.recording_started = None;
        }
        self.collect_export();
        self.collect_snapshot_saves();
        if self.controller.is_paused()
            && self.gesture.is_none()
            && self.idle_resume != AutoResumeDelay::Immediate
            && self.last_interaction.elapsed()
                >= Duration::from_secs(u64::from(self.idle_resume.seconds()))
        {
            self.resume();
        }
        ctx.request_repaint_after(
            if self.window_drag.is_some()
                || self.gesture.is_some()
                || self.export.is_some()
                || self.snapshot_save.is_some()
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
                .inner_margin(egui::Margin::symmetric(8, 4))
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
                self.refresh_waveform();
                let height = if self.waveform_only {
                    ui.available_height()
                } else {
                    (ui.available_height() - 26.0).max(46.0)
                };
                let size = Vec2::new(ui.available_width(), height);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                if !self.waveform_only {
                    self.full_waveform_height = rect.height() - 2.0 * WAVEFORM_INSET;
                }
                if !interaction_blocked {
                    self.interact_waveform(ui, frame, &response);
                }
                self.handle_snapshot_shortcut(ui.ctx());
                self.draw_waveform(ui, rect, !interaction_blocked && window_cursor.is_none());
                let status = self.controller.error().unwrap_or(&self.message);
                if !self.waveform_only && !status.is_empty() {
                    ui.add(egui::Label::new(status).truncate())
                        .on_hover_text(status);
                }
                if let Some(cursor) = window_cursor {
                    ui.ctx().set_cursor_icon(cursor);
                } else if !interaction_blocked {
                    if !self.waveform_only {
                        self.draw_action_hint(ui, &response);
                    }
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
        if let Some(job) = self.export.take() {
            let _ = job.worker.join();
        }
        for clip in self
            .pending_exports
            .drain(..)
            .filter(|clip| clip.immediate && !clip.provisional)
        {
            let _ = clip.write();
        }
        if let Some(worker) = self.snapshot_save.take() {
            let _ = worker.join();
        }
        for clip in self.pending_snapshot_saves.drain(..) {
            let _ = clip.write();
        }
    }
}

fn snapshot_save_key(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key {
            key: egui::Key::S,
            pressed: true,
            modifiers,
            ..
        } if modifiers.ctrl && !modifiers.alt && !modifiers.shift && !modifiers.mac_cmd
    )
}

fn fraction(rect: Rect, x: f32) -> f32 {
    ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0)
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

const DRAG_ICON: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0x18, 0x08, 0x06, 0x00, 0x00, 0x00, 0xed, 0x69, 0x4d,
    0xbf, 0x00, 0x00, 0x00, 0x60, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x10, 0x93, 0x92, 0xfd,
    0x3f, 0x10, 0x98, 0x61, 0xd4, 0x62, 0x62, 0x71, 0xfa, 0xf3, 0xad, 0x23, 0xc4, 0x62, 0x98, 0x85,
    0x83, 0xce, 0x62, 0x62, 0x1d, 0xc4, 0x40, 0x6a, 0x90, 0xa2, 0x5b, 0x8c, 0x4b, 0x7e, 0xc0, 0x2c,
    0x26, 0xe4, 0x00, 0x06, 0x52, 0x83, 0x14, 0x97, 0xc5, 0x84, 0x1c, 0x44, 0xb6, 0xc5, 0xe4, 0xd2,
    0x44, 0x5b, 0x4c, 0x2d, 0x0b, 0x09, 0x39, 0x60, 0xd4, 0xe2, 0x81, 0x8b, 0xe3, 0x01, 0x4f, 0xd5,
    0x03, 0x96, 0x8f, 0x07, 0xbc, 0xe4, 0x1a, 0xb0, 0xb2, 0x7a, 0xd0, 0xd4, 0x4e, 0x23, 0xaf, 0x21,
    0x30, 0xe4, 0x1b, 0x7b, 0x00, 0x7b, 0x1f, 0x56, 0x37, 0xd2, 0x94, 0x7f, 0xd1, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

use std::io::{self, Cursor};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    AutoResumeDelay, DurationChoice, MeterReadings, Snapshot, WavExportProfile, WavSampleFormat,
    export_wav_with_profile, waveform,
};
use egui::{Color32, CursorIcon, PointerButton, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};
use nice_plug::context::gui::GuiContext;
use nice_plug_egui::baseview::dpi::LogicalSize;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, Frame, NiceEguiApp, RepaintNotifier,
    create_egui_editor,
};

use crate::session::Session;
use crate::settings::{DEFAULT_ACCENT, MeterMode, SessionSettings, TRANSPARENCY_LEVELS};

#[cfg(target_os = "windows")]
mod win_drag;

const BINS: usize = 512;
const REFRESH: Duration = Duration::from_millis(80);
const RECORDING_BLINK: Duration = Duration::from_millis(500);
const TOOLBAR_CONTROL_HEIGHT: f32 = 25.0;
const MIN_WAVEFORM_HEIGHT: f32 = 60.0;
const PRODUCT_ICON: &[u8] = include_bytes!("../../../assets/backtracking-icon.png");

pub type CaptureEditor = EguiEditor<CaptureUi>;

pub(crate) fn create(session: Arc<Session>) -> Option<CaptureEditor> {
    let editor_state = EguiEditorState::from_size(LogicalSize::new(640.0, 212.0), 1.0);
    create_egui_editor(
        editor_state.clone(),
        RepaintNotifier::new(),
        EguiNiceSettings::new().with_tile("Backtracking"),
        CaptureUi {
            session,
            editor_state,
            product_icon: None,
            minimum: [0.0; BINS],
            maximum: [0.0; BINS],
            seconds: 0.0,
            next_refresh: Instant::now(),
            recording_since: None,
            displayed_settings: None,
            options_open: false,
            options_tab: OptionsTab::Actions,
            full_waveform_height: None,
            frozen: None,
            selection: None,
            selection_ready_for_drag: false,
            gesture: None,
            drag_preparation: None,
            snapshot_writer: None,
            native_dragging: Arc::new(AtomicBool::new(false)),
            drag_results: None,
            message: String::new(),
        },
    )
}

pub struct CaptureUi {
    session: Arc<Session>,
    editor_state: Arc<EguiEditorState>,
    product_icon: Option<TextureHandle>,
    minimum: [f32; BINS],
    maximum: [f32; BINS],
    seconds: f32,
    next_refresh: Instant,
    recording_since: Option<Instant>,
    displayed_settings: Option<SessionSettings>,
    options_open: bool,
    options_tab: OptionsTab,
    full_waveform_height: Option<f32>,
    frozen: Option<Arc<Snapshot>>,
    selection: Option<Range<f32>>,
    selection_ready_for_drag: bool,
    gesture: Option<Gesture>,
    drag_preparation: Option<DragPreparation>,
    snapshot_writer: Option<SnapshotWriter>,
    native_dragging: Arc<AtomicBool>,
    drag_results: Option<Receiver<Result<PathBuf, String>>>,
    message: String,
}

enum Gesture {
    Select {
        anchor: f32,
        current: f32,
    },
    Drag {
        origin: Pos2,
        preparation_started: bool,
    },
}

enum FolderAction {
    Select(FolderKind),
    Open(FolderKind),
}

#[derive(Clone, Copy)]
enum FolderKind {
    Export,
    Saved,
}

impl FolderKind {
    fn label(self) -> &'static str {
        match self {
            Self::Export => "Export folder",
            Self::Saved => "Saved snapshots",
        }
    }

    fn path(self, settings: &SessionSettings) -> &Path {
        match self {
            Self::Export => &settings.output_dir,
            Self::Saved => &settings.saved_dir,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OptionsTab {
    Actions,
    Capture,
    Appearance,
    Folders,
}

impl MeterMode {
    fn reading(self, readings: MeterReadings) -> f32 {
        match self {
            Self::Dbfs => readings.peak_dbfs,
            Self::Vu => readings.vu,
            Self::Lufs => readings.momentary_lufs,
        }
    }
}

struct DragPreparation {
    result: Arc<Mutex<Option<io::Result<PreparedDrag>>>>,
    cancel: Option<Sender<()>>,
    worker: JoinHandle<()>,
}

struct PreparedDrag {
    path: PathBuf,
    accepted: bool,
}

impl Drop for PreparedDrag {
    fn drop(&mut self) {
        if !self.accepted {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct ActiveDrag {
    file: PreparedDrag,
    session: Arc<Session>,
    active: Arc<AtomicBool>,
    results: Sender<Result<PathBuf, String>>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl ActiveDrag {
    fn accept(&mut self) {
        self.file.accepted = true;
        let _ = self.results.send(Ok(self.file.path.clone()));
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for ActiveDrag {
    fn drop(&mut self) {
        self.session.end_waveform_gesture();
        self.active.store(false, Ordering::Release);
    }
}

struct WavClip {
    profile: WavExportProfile,
    output: PathBuf,
    snapshot: Arc<Snapshot>,
    range: Range<usize>,
}

impl WavClip {
    fn write(self) -> io::Result<PathBuf> {
        let samples = self
            .snapshot
            .frames(self.range)
            .ok_or_else(|| io::Error::other("invalid audio selection"))?;
        let path = export_wav_with_profile(
            &self.output,
            self.snapshot.sample_rate,
            self.snapshot.channels,
            samples,
            self.profile,
        )?;
        Ok(path.canonicalize().unwrap_or(path))
    }
}

struct SnapshotWriter {
    requests: Sender<WavClip>,
    results: Receiver<io::Result<PathBuf>>,
    worker: JoinHandle<()>,
}

impl SnapshotWriter {
    fn start() -> io::Result<Self> {
        let (requests, incoming) = mpsc::channel::<WavClip>();
        let (completed, results) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("backtracking-snapshot".into())
            .spawn(move || {
                for clip in incoming {
                    let _ = completed.send(clip.write());
                }
            })?;
        Ok(Self {
            requests,
            results,
            worker,
        })
    }

    fn finish(self) {
        drop(self.requests);
        let _ = self.worker.join();
    }
}

fn product_icon(context: &egui::Context) -> Option<TextureHandle> {
    let mut decoder = png::Decoder::new(Cursor::new(PRODUCT_ICON));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    let size = [info.width as usize, info.height as usize];
    let pixels = &pixels[..info.buffer_size()];
    let image = match info.color_type {
        png::ColorType::Rgba => egui::ColorImage::from_rgba_unmultiplied(size, pixels),
        png::ColorType::Rgb => egui::ColorImage::from_rgb(size, pixels),
        _ => return None,
    };
    Some(context.load_texture(
        "backtracking-icon",
        image,
        egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear)),
    ))
}

impl NiceEguiApp for CaptureUi {
    fn build(
        &mut self,
        context: egui::Context,
        _: GuiContext,
        _frame: &mut Frame,
    ) -> Result<(), nice_plug_egui::baseview::HandlerError> {
        self.editor_closed();
        self.product_icon = product_icon(&context);
        // Baseview still sends ignored keys to egui. Let AppKit forward Cmd+S to the DAW.
        #[cfg(target_os = "macos")]
        _frame.set_key_capture(nice_plug_egui::KeyCapture::IgnoreKeys(vec![
            nice_plug_egui::Key::Character("s".into()),
            nice_plug_egui::Key::Character("S".into()),
        ]));
        context.add_font(egui::FontInsert {
            name: waveform::HINT_BOLD_FAMILY.to_owned(),
            data: egui::FontData::from_static(waveform::HINT_BOLD_FONT),
            families: vec![egui::InsertFontFamily {
                family: egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into()),
                priority: egui::FontPriority::Highest,
            }],
        });
        Ok(())
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame) {
        if let Some(results) = &self.drag_results {
            use std::sync::mpsc::TryRecvError;

            match results.try_recv() {
                Ok(Ok(path)) => {
                    self.selection_ready_for_drag = false;
                    self.message = format!("Saved {}", path.display());
                    self.drag_results = None;
                }
                Ok(Err(error)) => {
                    self.message = format!("Could not drag file: {error}");
                    self.drag_results = None;
                }
                Err(TryRecvError::Disconnected) => self.drag_results = None,
                Err(TryRecvError::Empty) => (),
            }
        }
        let mut settings = self.session.settings();
        if let Some(previous) = &self.displayed_settings {
            let retention_changed = previous.retention != settings.retention;
            let output_changed = previous.output_dir != settings.output_dir;
            let format_changed = previous.wav_profile != settings.wav_profile;
            if retention_changed {
                self.reset_frozen();
            } else if output_changed || format_changed {
                self.discard_drag_preparation();
            }
            if retention_changed || output_changed || format_changed {
                self.message.clear();
                if self.gesture.take().is_some() {
                    self.session.end_waveform_gesture();
                }
            }
        }
        self.reap_drag_preparation();
        self.collect_saved_snapshots();
        let save_shortcut = take_save_shortcut(ui) && frame.baseview_window().has_focus();
        let save_after_gesture = save_shortcut && !self.can_toggle_capture();
        if save_shortcut && !save_after_gesture {
            self.save_snapshot(&settings);
        }
        let canceled_selection = !self.options_open
            && self.can_cancel_selection()
            && ui.input_mut(|input| {
                let event_count = input.events.len();
                input.events.retain(|event| {
                    !matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Escape | egui::Key::Backspace,
                            pressed: true,
                            ..
                        }
                    )
                });
                input.events.len() != event_count
            });
        if canceled_selection {
            self.cancel_selection();
        }
        let mut paused = self.session.is_paused();
        self.refresh_waveform(paused, settings.idle_resume);
        ui.visuals_mut().selection.bg_fill = muted_accent(settings.accent);

        if ui.input(|input| input.pointer.any_pressed()) {
            self.session.mark_interaction();
        }

        let waveform_only = settings.show_waveform_only;
        // An embedded host can still composite this against its own opaque backing.
        frame.set_clear_color(if waveform_only {
            egui::Rgba::TRANSPARENT
        } else {
            egui::Rgba::BLACK
        });
        let can_toggle = self.can_toggle_capture();
        let canvas = ui.max_rect().intersect(ui.clip_rect());
        if can_toggle
            && ui.input(|input| {
                input.pointer.secondary_clicked()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|position| canvas.contains(position))
            })
        {
            self.options_open = true;
        }
        let controls_enabled = can_toggle && !self.options_open;
        let gear = if waveform_only {
            self.recording_since = None;
            None
        } else {
            Some(
                ui.horizontal(|ui| {
                    if let Some(icon) = &self.product_icon {
                        ui.image((icon.id(), Vec2::splat(TOOLBAR_CONTROL_HEIGHT)));
                    }
                    ui.heading("Backtracking");
                    ui.separator();
                    if capture_button(ui, paused, controls_enabled).clicked() {
                        self.toggle_capture(ui, paused);
                        paused = self.session.is_paused();
                    }
                    self.recording_indicator(ui, paused);
                    if clear_button(ui, controls_enabled && self.seconds > 0.0).clicked() {
                        self.clear_capture();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let gear = settings_button(ui);
                        self.meter_readout(ui, settings.meter_mode);
                        gear
                    })
                    .inner
                })
                .inner,
            )
        };

        let dropped = self.session.dropped_frames();
        let status_is_visible = !waveform_only && (dropped != 0 || !self.message.is_empty());
        let status_height = if status_is_visible {
            ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y
        } else {
            0.0
        };
        let available_height = ui.available_height();
        let wave_height = if waveform_only {
            available_height
        } else {
            let height = (available_height - status_height).max(MIN_WAVEFORM_HEIGHT);
            self.full_waveform_height = Some(height);
            height
        };
        let wave_size = Vec2::new(ui.available_width(), wave_height);
        let (rect, response) = ui.allocate_exact_size(wave_size, Sense::click_and_drag());
        if can_toggle && gear.is_some_and(|response| response.clicked()) {
            self.options_open = true;
        }
        let settings_open = self.options_open;
        if settings_open {
            self.show_options(ui, frame, &mut settings);
        }
        let configured_seconds = settings.retention.seconds() as f32;
        if !canceled_selection {
            self.interact_waveform(
                ui,
                frame,
                &response,
                configured_seconds,
                settings.idle_resume,
                !settings_open,
            );
        }
        if canceled_selection {
            response.surrender_focus();
        }
        if save_after_gesture && self.can_toggle_capture() {
            self.save_snapshot(&settings);
        }
        // Keep the transparent overlay's entire gesture area and center only the drawing.
        let draw_rect = if waveform_only {
            let height = self.full_waveform_height.unwrap_or_else(|| {
                let spacing = ui.spacing();
                let toolbar_height = TOOLBAR_CONTROL_HEIGHT
                    .max(spacing.interact_size.y)
                    .max(ui.text_style_height(&egui::TextStyle::Heading));
                (available_height - toolbar_height - spacing.item_spacing.y)
                    .max(MIN_WAVEFORM_HEIGHT)
            });
            Rect::from_center_size(
                rect.center(),
                Vec2::new(rect.width(), height.min(rect.height())),
            )
        } else {
            rect
        };
        self.draw_waveform(ui, draw_rect, configured_seconds, !settings_open, &settings);
        if !settings_open {
            if !waveform_only {
                self.draw_action_hint(ui, rect);
            }
            response.on_hover_cursor(CursorIcon::Grab);
        }
        if !waveform_only && dropped != 0 {
            let mut status = format!("Dropped {dropped} audio frames");
            if !self.message.is_empty() {
                status.push_str(" · ");
                status.push_str(&self.message);
            }
            ui.add(egui::Label::new(&status).truncate())
                .on_hover_text(status);
        } else if status_is_visible {
            ui.add(egui::Label::new(&self.message).truncate())
                .on_hover_text(&self.message);
        }
        self.displayed_settings = Some(settings);
        ui.ctx().request_repaint_after(
            if self.gesture.is_some() || self.drag_preparation.is_some() {
                Duration::from_millis(16)
            } else {
                REFRESH
            },
        );
    }

    fn editor_closed(&mut self) {
        self.gesture = None;
        self.recording_since = None;
        self.options_open = false;
        self.discard_drag_preparation();
        if !self.native_dragging.load(Ordering::Acquire) {
            self.session.end_waveform_gesture();
        }
    }
}

impl Drop for CaptureUi {
    fn drop(&mut self) {
        if !self.native_dragging.load(Ordering::Acquire) {
            self.session.end_waveform_gesture();
        }
        if let Some(writer) = self.snapshot_writer.take() {
            writer.finish();
        }
        if let Some(job) = self.drag_preparation.take() {
            // Close the worker's cancellation channel before waiting for it.
            drop(job.cancel);
            let _ = job.worker.join();
        }
    }
}

impl CaptureUi {
    fn meter_readout(&self, ui: &mut egui::Ui, mode: MeterMode) {
        let value = mode.reading(self.session.meter_readings());
        let text = if value.is_finite() {
            format!("{value:.1} {}", mode.label())
        } else {
            format!("— {}", mode.label())
        };
        let description = match mode {
            MeterMode::Dbfs => "Sample peak level",
            MeterMode::Vu => "Volume units",
            MeterMode::Lufs => "Momentary loudness (400 ms)",
        };
        ui.add_sized(
            [90.0, TOOLBAR_CONTROL_HEIGHT],
            egui::Label::new(egui::RichText::new(text).monospace()).truncate(),
        )
        .on_hover_text(description);
    }

    fn recording_indicator(&mut self, ui: &mut egui::Ui, paused: bool) {
        let (status, color) = if !paused && self.session.is_recording() {
            let started = self.recording_since.get_or_insert_with(Instant::now);
            let phase = started.elapsed().as_millis() / RECORDING_BLINK.as_millis();
            let color = if phase.is_multiple_of(2) {
                Color32::from_rgb(249, 78, 88)
            } else {
                Color32::from_rgb(103, 44, 48)
            };
            ("Recording", color)
        } else {
            self.recording_since = None;
            if paused {
                ("Paused", Color32::from_rgb(51, 56, 61))
            } else {
                ("Waiting for sound", Color32::from_rgb(74, 81, 88))
            }
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, status));
        response.on_hover_text(status);
    }

    fn show_options(&mut self, ui: &egui::Ui, frame: &mut Frame, settings: &mut SessionSettings) {
        let bounds = ui.ctx().viewport_rect();
        let mut open =
            !ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut edited = settings.clone();
        let mut folder_action = None;
        egui::Window::new("Options")
            .id(egui::Id::unique("backtracking-options"))
            .open(&mut open)
            .default_pos(bounds.center() - Vec2::new(230.0, 80.0))
            .default_width(460.0)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (OptionsTab::Actions, "Actions"),
                        (OptionsTab::Capture, "Capture"),
                        (OptionsTab::Appearance, "Appearance"),
                        (OptionsTab::Folders, "Folders"),
                    ] {
                        ui.selectable_value(&mut self.options_tab, tab, label);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("options-content")
                    .max_height((bounds.height() - 80.0).max(42.0))
                    .show(ui, |ui| {
                        match self.options_tab {
                            OptionsTab::Actions => self.capture_actions(ui, settings),
                            OptionsTab::Capture => capture_options(ui, &mut edited),
                            OptionsTab::Appearance => appearance_options(ui, &mut edited),
                            OptionsTab::Folders => folder_action = folder_options(ui, settings),
                        }
                        if !self.message.is_empty() {
                            ui.add(egui::Label::new(&self.message).truncate())
                                .on_hover_text(&self.message);
                        }
                    });
            });
        self.options_open = open;
        self.apply_options(settings, edited);
        if let Some(action) = folder_action {
            self.apply_folder_action(frame, settings, action);
        }
    }

    fn capture_actions(&mut self, ui: &mut egui::Ui, settings: &SessionSettings) {
        let paused = self.session.is_paused();
        let can_toggle = self.can_toggle_capture();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    can_toggle,
                    egui::Button::new(if paused { "Play" } else { "Pause" }),
                )
                .clicked()
            {
                self.toggle_capture(ui, paused);
            }
            if ui
                .add_enabled(
                    can_toggle && self.seconds > 0.0,
                    egui::Button::new("Clear capture"),
                )
                .clicked()
            {
                self.clear_capture();
            }
            if ui
                .add_enabled(can_toggle, egui::Button::new("Save snapshot"))
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                self.save_snapshot(settings);
            }
            if ui
                .add_enabled(
                    self.can_cancel_selection(),
                    egui::Button::new("Cancel selection"),
                )
                .on_hover_text("Esc / Backspace")
                .clicked()
            {
                self.cancel_selection();
            }
        });
    }

    fn apply_options(&mut self, settings: &mut SessionSettings, edited: SessionSettings) {
        if edited.retention != settings.retention {
            self.session.set_retention(edited.retention);
            self.reset_frozen();
            self.message.clear();
        }
        if edited.idle_resume != settings.idle_resume {
            self.session.set_idle_resume(edited.idle_resume);
        }
        if edited.panel_transparency != settings.panel_transparency {
            self.session
                .set_panel_transparency(edited.panel_transparency);
        }
        if edited.accent != settings.accent {
            self.session.set_accent(edited.accent);
        }
        if edited.meter_mode != settings.meter_mode {
            self.session.set_meter_mode(edited.meter_mode);
        }
        if edited.wav_profile != settings.wav_profile {
            self.session.set_wav_profile(edited.wav_profile);
            self.discard_drag_preparation();
            self.message.clear();
        }
        if edited.show_waveform_only != settings.show_waveform_only {
            self.session
                .set_show_waveform_only(edited.show_waveform_only);
        }
        *settings = edited;
    }

    fn apply_folder_action(
        &mut self,
        frame: &Frame,
        settings: &mut SessionSettings,
        action: FolderAction,
    ) {
        match action {
            FolderAction::Select(kind) => {
                self.session.set_gesture_active(true);
                let mut dialog = rfd::FileDialog::new()
                    .set_title(kind.label())
                    .set_parent(frame.baseview_window());
                if kind.path(settings).is_dir() {
                    dialog = dialog.set_directory(kind.path(settings));
                }
                let selection = dialog.pick_folder();
                self.session.set_gesture_active(false);
                if let Some(folder) = selection {
                    self.message = format!("{}: {}", kind.label(), folder.display());
                    match kind {
                        FolderKind::Export => {
                            self.session.set_output_dir(folder.clone());
                            settings.output_dir = folder;
                            self.discard_drag_preparation();
                        }
                        FolderKind::Saved => {
                            self.session.set_saved_dir(folder.clone());
                            settings.saved_dir = folder;
                        }
                    }
                }
            }
            FolderAction::Open(kind) => {
                let folder = kind.path(settings);
                if let Err(error) =
                    std::fs::create_dir_all(folder).and_then(|()| open::that_detached(folder))
                {
                    self.message = format!("Could not open {}: {error}", kind.label());
                }
            }
        }
    }

    fn refresh_waveform(&mut self, paused: bool, idle_resume: AutoResumeDelay) {
        if self.frozen.is_some() {
            if paused {
                return;
            }
            if idle_resume == AutoResumeDelay::Immediate {
                // The selected preview stays linked to its snapshot while capture continues.
                if self.selection_ready_for_drag {
                    return;
                }
                self.clear_frozen_preview();
            } else {
                self.reset_frozen();
            }
        }
        if Instant::now() < self.next_refresh {
            return;
        }
        self.next_refresh = Instant::now() + REFRESH;
        let (frames, rate) = self.session.waveform(&mut self.minimum, &mut self.maximum);
        self.seconds = frames as f32 / rate.max(1) as f32;
    }

    fn update_peaks(&mut self, snapshot: &Snapshot) {
        self.seconds = snapshot.frame_count() as f32 / snapshot.sample_rate as f32;
        waveform::peaks(
            &snapshot.samples,
            snapshot.channels,
            &mut self.minimum,
            &mut self.maximum,
        );
    }

    fn can_toggle_capture(&self) -> bool {
        self.gesture.is_none() && !self.native_dragging.load(Ordering::Acquire)
    }

    fn toggle_capture(&mut self, ui: &egui::Ui, paused: bool) {
        if paused {
            self.session.resume();
            self.reset_frozen();
        } else {
            self.session.pause();
            self.cache_frozen_snapshot();
        }
        ui.ctx().request_repaint();
    }

    fn cache_frozen_snapshot(&mut self) {
        if self.frozen.is_none()
            && let Some(snapshot) = self.session.snapshot()
        {
            self.update_peaks(&snapshot);
            self.frozen = Some(Arc::new(snapshot));
        }
    }

    fn clear_capture(&mut self) {
        self.session.clear();
        self.reset_frozen();
        self.minimum.fill(0.0);
        self.maximum.fill(0.0);
        self.seconds = 0.0;
        self.recording_since = None;
        self.message.clear();
    }

    fn reset_frozen(&mut self) {
        self.clear_frozen_preview();
        self.discard_drag_preparation();
    }

    fn clear_frozen_preview(&mut self) {
        self.frozen = None;
        self.selection = None;
        self.selection_ready_for_drag = false;
        self.next_refresh = Instant::now();
    }

    fn can_cancel_selection(&self) -> bool {
        !self.native_dragging.load(Ordering::Acquire)
            && (self.selection_ready_for_drag
                || matches!(self.gesture, Some(Gesture::Select { .. })))
    }

    fn cancel_selection(&mut self) {
        self.gesture = None;
        self.reset_frozen();
        self.message.clear();
        self.session.cancel_waveform_selection();
    }

    fn discard_drag_preparation(&mut self) {
        if let Some(preparation) = &mut self.drag_preparation {
            preparation.cancel = None;
        }
    }

    fn audio_rect(&self, rect: Rect, seconds: f32) -> Rect {
        let inner = rect.shrink(8.0);
        let width = inner.width() * (self.seconds / seconds).clamp(0.0, 1.0);
        Rect::from_min_max(Pos2::new(inner.right() - width, inner.top()), inner.max)
    }

    fn draw_waveform(
        &self,
        ui: &egui::Ui,
        rect: Rect,
        seconds: f32,
        allow_hover: bool,
        settings: &SessionSettings,
    ) {
        let waveform_only = settings.show_waveform_only;
        let painter = ui.painter_at(rect);
        let inner = rect.shrink(8.0);
        let middle = inner.center().y;
        let extent = inner.height().max(0.0) * 0.47;
        if !waveform_only {
            let opacity = ((100 - u16::from(settings.panel_transparency)) * 255 / 100) as u8;
            painter.rect_filled(
                rect,
                6.0,
                Color32::from_rgba_unmultiplied(22, 26, 29, opacity),
            );
            painter.hline(
                inner.x_range(),
                middle,
                Stroke::new(1.0, Color32::from_rgb(47, 60, 64)),
            );
        }
        let audio_rect = self.audio_rect(rect, seconds);
        let width = audio_rect.width();
        if width <= 0.0 {
            return;
        }

        let explicit_range = match &self.gesture {
            Some(Gesture::Select { anchor, current }) => {
                Some(anchor.min(*current)..anchor.max(*current))
            }
            _ => self.selection.clone(),
        };
        let hover_preview = !waveform_only
            && explicit_range.is_none()
            && match &self.gesture {
                Some(Gesture::Select { .. }) => false,
                Some(Gesture::Drag { .. }) => true,
                None => {
                    allow_hover
                        && ui.input(|input| {
                            input
                                .pointer
                                .hover_pos()
                                .is_some_and(|position| rect.contains(position))
                        })
                }
            };
        if hover_preview {
            painter.rect_filled(audio_rect, 2.0, muted_accent(settings.accent));
        }

        let [r, g, b] = settings.accent;
        let color = Color32::from_rgb(r, g, b);
        let complement = [255 - r, 255 - g, 255 - b];
        let selected_color = if settings
            .accent
            .into_iter()
            .zip(complement)
            .all(|(a, b)| a.abs_diff(b) < 64)
        {
            Color32::WHITE
        } else {
            Color32::from_rgb(complement[0], complement[1], complement[2])
        };
        for (bin, (&low, &high)) in self.minimum.iter().zip(&self.maximum).enumerate() {
            let fraction = (bin as f32 + 0.5) / BINS as f32;
            let x = audio_rect.left() + width * fraction;
            let top = middle - high.clamp(-1.0, 1.0) * extent;
            let bottom = middle - low.clamp(-1.0, 1.0) * extent;
            let color = if explicit_range
                .as_ref()
                .is_some_and(|range| range.contains(&fraction))
            {
                selected_color
            } else {
                color
            };
            painter.line_segment(
                [Pos2::new(x, top), Pos2::new(x, bottom.max(top + 0.4))],
                Stroke::new(1.0, color),
            );
        }
    }

    fn draw_action_hint(&self, ui: &egui::Ui, rect: Rect) {
        if self.seconds <= 0.0 || self.native_dragging.load(Ordering::Acquire) {
            return;
        }
        let (cursor, down, modifier) = ui.input(|input| {
            (
                input.pointer.hover_pos(),
                input.pointer.button_down(PointerButton::Primary),
                input.modifiers.alt || input.modifiers.ctrl,
            )
        });
        let visible_waveform = rect.intersect(ui.clip_rect());
        let Some(cursor) = cursor.filter(|cursor| visible_waveform.contains(*cursor)) else {
            return;
        };
        let hint = match &self.gesture {
            Some(Gesture::Select { .. }) => "Drag to Select".into(),
            Some(Gesture::Drag { .. }) => "Drag and Drop".into(),
            None if down => return,
            None if modifier => "Drag to Select".into(),
            None if self.selection_ready_for_drag
                && cfg!(any(target_os = "macos", target_os = "windows")) =>
            {
                "Drag and Drop\nEsc / Backspace to cancel".into()
            }
            None => {
                let action = if cfg!(any(target_os = "macos", target_os = "windows")) {
                    "Drag and Drop"
                } else {
                    "Ctrl + S"
                };
                let selection = if cfg!(target_os = "macos") {
                    "Option + Drag"
                } else {
                    "Ctrl + Drag"
                };
                format!("{action}\n{selection}")
            }
        };

        let bounds = visible_waveform.shrink(2.0);
        let painter = ui.painter_at(bounds);
        let color = Color32::from_rgb(232, 242, 238);
        let font = egui::FontId::new(
            12.0,
            egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into()),
        );
        let label = painter.layout_no_wrap(hint, font, color);
        let padding = Vec2::new(8.0, 5.0);
        let size = label.size() + 2.0 * padding;
        let [left, top] = waveform::hint_position(
            [cursor.x, cursor.y],
            [size.x, size.y],
            [bounds.left(), bounds.top(), bounds.right(), bounds.bottom()],
        );
        let background = Rect::from_min_size(Pos2::new(left, top), size);
        painter.rect_filled(
            background,
            5.0,
            Color32::from_rgba_unmultiplied(33, 41, 46, 245),
        );
        painter.rect_stroke(
            background,
            5.0,
            Stroke::new(1.0, Color32::from_rgb(65, 81, 82)),
            egui::StrokeKind::Inside,
        );
        painter.galley(background.min + padding, label, color);
    }

    fn interact_waveform(
        &mut self,
        ui: &egui::Ui,
        frame: &Frame,
        waveform: &egui::Response,
        seconds: f32,
        idle_resume: AutoResumeDelay,
        allow_new_gesture: bool,
    ) {
        let rect = waveform.rect;
        let (position, press, down) = ui.input(|input| {
            let press = input.events.iter().rev().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers,
                } => Some((*pos, modifiers.alt || modifiers.ctrl)),
                _ => None,
            });
            (
                input.pointer.interact_pos(),
                press,
                input.pointer.button_down(PointerButton::Primary),
            )
        });
        if self.native_dragging.load(Ordering::Acquire) {
            return;
        }

        if allow_new_gesture
            && let Some((position, selecting)) =
                press.filter(|(position, _)| rect.contains(*position))
        {
            self.discard_drag_preparation();
            self.session.begin_waveform_gesture();
            if idle_resume == AutoResumeDelay::Immediate
                && selecting
                && self.selection_ready_for_drag
            {
                self.clear_frozen_preview();
            }
            self.cache_frozen_snapshot();
            if selecting {
                waveform.request_focus();
                let anchor = position_in_audio(self.audio_rect(rect, seconds), position.x);
                self.gesture = Some(Gesture::Select {
                    anchor,
                    current: anchor,
                });
            } else {
                self.gesture = Some(Gesture::Drag {
                    origin: position,
                    preparation_started: false,
                });
            }
        }

        let audio_rect = self.audio_rect(rect, seconds);
        if let Some(Gesture::Select { current, .. }) = &mut self.gesture
            && let Some(position) = position
        {
            *current = position_in_audio(audio_rect, position.x);
        }

        let dragging = matches!(self.gesture, Some(Gesture::Drag { .. }));
        let drag_canceled = dragging && ui.input(|input| input.key_pressed(egui::Key::Escape));
        let down = if dragging && cfg!(any(target_os = "macos", target_os = "windows")) {
            primary_button_pressed()
        } else {
            down
        };
        if down && !drag_canceled {
            if let Some(Gesture::Drag {
                origin,
                preparation_started,
            }) = self.gesture
                && pointer_outside_editor(frame)
            {
                if !preparation_started {
                    if self.drag_preparation.is_none() {
                        self.gesture = Some(Gesture::Drag {
                            origin,
                            preparation_started: true,
                        });
                        self.begin_drag_preparation();
                    }
                } else if let Some(file) = self.take_prepared_drag() {
                    self.gesture = None;
                    self.message.clear();
                    self.start_native_drag(frame, file, origin);
                }
            }
        } else if let Some(gesture) = self.gesture.take() {
            if let Gesture::Select { anchor, current } = gesture {
                let range = anchor.min(current)..anchor.max(current);
                self.selection = (range.end > range.start).then_some(range);
                self.selection_ready_for_drag = self.selection.is_some();
            }
            self.discard_drag_preparation();
            self.session.end_waveform_gesture();
        }
    }

    fn begin_drag_preparation(&mut self) {
        let settings = self.session.settings();
        let Some(snapshot) = self
            .frozen
            .as_ref()
            .filter(|snapshot| snapshot.frame_count() > 0)
            .cloned()
        else {
            self.message = "No captured audio to save yet.".into();
            return;
        };
        let clip = WavClip {
            profile: settings.wav_profile,
            output: settings.output_dir,
            range: export_frame_range(snapshot.frame_count(), self.selection.as_ref()),
            snapshot,
        };
        let (cancel, canceled) = mpsc::channel();
        let result = Arc::new(Mutex::new(None));
        let pending = result.clone();
        let editor_state = self.editor_state.clone();
        let worker = thread::Builder::new()
            .name("backtracking-drag".into())
            .spawn(move || {
                use std::sync::mpsc::{RecvTimeoutError, TryRecvError};

                let can_drag = || editor_state.is_open() && primary_button_pressed();
                if !can_drag() || !matches!(canceled.try_recv(), Err(TryRecvError::Empty)) {
                    return;
                }
                let file = clip.write().map(|path| PreparedDrag {
                    path,
                    accepted: false,
                });
                *pending.lock().unwrap() = Some(file);
                // The editor can disappear between frames while the DAW remains open.
                // Keep custody here until the editor claims the WAV or the gesture ends.
                while can_drag()
                    && matches!(
                        canceled.recv_timeout(Duration::from_millis(16)),
                        Err(RecvTimeoutError::Timeout)
                    )
                {}
                pending.lock().unwrap().take();
            });
        match worker {
            Ok(worker) => {
                self.drag_preparation = Some(DragPreparation {
                    result,
                    cancel: Some(cancel),
                    worker,
                });
            }
            Err(error) => self.message = format!("Could not prepare file drag: {error}"),
        }
    }

    fn reap_drag_preparation(&mut self) {
        if self
            .drag_preparation
            .as_ref()
            .is_some_and(|job| job.worker.is_finished())
            && let Some(job) = self.drag_preparation.take()
        {
            let _ = job.worker.join();
        }
    }

    fn take_prepared_drag(&mut self) -> Option<PreparedDrag> {
        let preparation = self.drag_preparation.as_mut()?;
        preparation.cancel.as_ref()?;
        let result = preparation.result.lock().unwrap().take()?;
        preparation.cancel = None;
        match result {
            Ok(file) => Some(file),
            Err(error) => {
                self.message = format!("Could not prepare file drag: {error}");
                None
            }
        }
    }

    fn save_snapshot(&mut self, settings: &SessionSettings) {
        let selected = self.selection.is_some();
        let snapshot = if selected {
            self.frozen.clone()
        } else {
            self.session.snapshot_live().map(Arc::new)
        };
        let Some(snapshot) = snapshot.filter(|snapshot| snapshot.frame_count() > 0) else {
            self.message = "No captured audio to save yet.".into();
            return;
        };
        let range = export_frame_range(snapshot.frame_count(), self.selection.as_ref());
        if selected || self.session.is_paused() {
            self.cancel_selection();
        }
        let result = self.queue_saved_snapshot(WavClip {
            profile: settings.wav_profile,
            output: settings.saved_dir.clone(),
            range,
            snapshot,
        });
        self.message = match result {
            Ok(()) => "Saving snapshot…".into(),
            Err(error) => format!("Could not save snapshot: {error}"),
        };
    }

    fn queue_saved_snapshot(&mut self, clip: WavClip) -> io::Result<()> {
        let writer = match &self.snapshot_writer {
            Some(writer) => writer,
            None => self.snapshot_writer.insert(SnapshotWriter::start()?),
        };
        writer
            .requests
            .send(clip)
            .map_err(|_| io::Error::other("snapshot writer stopped unexpectedly"))
    }

    fn collect_saved_snapshots(&mut self) {
        if let Some(writer) = &self.snapshot_writer {
            for result in writer.results.try_iter() {
                self.message = match result {
                    Ok(path) => format!("Saved snapshot {}", path.display()),
                    Err(error) => format!("Could not save snapshot: {error}"),
                };
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn start_native_drag(&mut self, frame: &Frame, file: PreparedDrag, _origin: Pos2) {
        let (results, receiver) = mpsc::channel();
        self.native_dragging.store(true, Ordering::Release);
        let drag = ActiveDrag {
            file,
            session: Arc::clone(&self.session),
            active: Arc::clone(&self.native_dragging),
            results,
        };
        #[cfg(target_os = "windows")]
        let started = win_drag::schedule(frame.baseview_window(), drag);
        #[cfg(target_os = "macos")]
        let started = mac_drag::start(frame, _origin, drag);
        match started {
            Ok(()) => self.drag_results = Some(receiver),
            Err(error) => self.message = format!("Could not start file drag: {error}"),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn start_native_drag(&mut self, _frame: &Frame, _file: PreparedDrag, _origin: Pos2) {
        self.session.end_waveform_gesture();
    }
}

fn pointer_outside_editor(_frame: &Frame) -> bool {
    #[cfg(target_os = "windows")]
    return win_drag::pointer_outside_editor(_frame.baseview_window());
    #[cfg(target_os = "macos")]
    return mac_drag::pointer_outside_editor(_frame);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    false
}

fn primary_button_pressed() -> bool {
    #[cfg(target_os = "windows")]
    return win_drag::primary_button_pressed();
    #[cfg(target_os = "macos")]
    return mac_drag::primary_button_pressed();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    false
}

fn muted_accent([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r / 3, g / 3, b / 3)
}

fn take_save_shortcut(ui: &egui::Ui) -> bool {
    ui.input_mut(|input| {
        let event_count = input.events.len();
        input.events.retain(|event| {
            !matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::S,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if modifiers.ctrl && !modifiers.alt && !modifiers.shift && !modifiers.mac_cmd
            )
        });
        input.events.len() != event_count
    })
}

fn export_frame_range(frames: usize, selection: Option<&Range<f32>>) -> Range<usize> {
    selection.map_or(0..frames, |range| {
        let first = ((range.start * frames as f32).floor() as usize).min(frames - 1);
        let last = (range.end * frames as f32).ceil() as usize;
        first..last.clamp(first + 1, frames)
    })
}

fn capture_options(ui: &mut egui::Ui, settings: &mut SessionSettings) {
    egui::Grid::new("capture-options").show(ui, |ui| {
        ui.label("History");
        ui.horizontal(|ui| {
            for duration in DurationChoice::ALL {
                ui.selectable_value(
                    &mut settings.retention,
                    duration,
                    format!("{} s", duration.seconds()),
                );
            }
        });
        ui.end_row();
        ui.label("Auto-resume");
        ui.horizontal(|ui| {
            for duration in AutoResumeDelay::ALL {
                ui.selectable_value(
                    &mut settings.idle_resume,
                    duration,
                    format!("{} s", duration.seconds()),
                );
            }
        });
        ui.end_row();
        wav_format_controls(ui, &mut settings.wav_profile);
    });
}

fn appearance_options(ui: &mut egui::Ui, settings: &mut SessionSettings) {
    ui.checkbox(&mut settings.show_waveform_only, "Show waveform only");
    egui::Grid::new("appearance-options").show(ui, |ui| {
        ui.label("Panel transparency")
            .on_hover_text("Fades the panel fill. The DAW may keep the editor backing opaque.");
        ui.horizontal(|ui| {
            for percent in TRANSPARENCY_LEVELS {
                ui.selectable_value(
                    &mut settings.panel_transparency,
                    percent,
                    format!("{percent}%"),
                );
            }
        });
        ui.end_row();
        ui.label("Meter");
        ui.horizontal(|ui| {
            for mode in MeterMode::ALL {
                ui.selectable_value(&mut settings.meter_mode, mode, mode.label());
            }
        });
        ui.end_row();
        ui.label("Theme color");
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut settings.accent);
            if ui.button("Reset color").clicked() {
                settings.accent = DEFAULT_ACCENT;
            }
        });
        ui.end_row();
    });
}

fn folder_options(ui: &mut egui::Ui, settings: &SessionSettings) -> Option<FolderAction> {
    let mut action = None;
    egui::Grid::new("folder-options").show(ui, |ui| {
        for kind in [FolderKind::Export, FolderKind::Saved] {
            let folder = kind.path(settings).display().to_string();
            ui.label(kind.label()).on_hover_text(&folder);
            if ui.button("Select folder").on_hover_text(&folder).clicked() {
                action = Some(FolderAction::Select(kind));
            }
            if ui.button("Open folder").on_hover_text(folder).clicked() {
                action = Some(FolderAction::Open(kind));
            }
            ui.end_row();
        }
    });
    action
}

fn wav_format_controls(ui: &mut egui::Ui, profile: &mut WavExportProfile) {
    ui.label("WAV sample rate");
    ui.horizontal(|ui| {
        for (rate, label) in WavExportProfile::SAMPLE_RATES
            .into_iter()
            .zip(["44.1 kHz", "48 kHz", "96 kHz"])
        {
            ui.selectable_value(&mut profile.sample_rate, rate, label);
        }
    });
    ui.end_row();
    ui.label("WAV sample format");
    ui.horizontal(|ui| {
        for (format, label) in [
            (WavSampleFormat::Pcm16, "16-bit PCM"),
            (WavSampleFormat::Pcm24, "24-bit PCM"),
            (WavSampleFormat::Float32, "32-bit float"),
        ] {
            ui.selectable_value(&mut profile.sample_format, format, label);
        }
    });
    ui.end_row();
}

fn capture_button(ui: &mut egui::Ui, paused: bool, enabled: bool) -> egui::Response {
    let button = egui::Button::new("").min_size(Vec2::splat(TOOLBAR_CONTROL_HEIGHT));
    let response = ui.add_enabled(enabled, button);
    let label = if paused {
        "Start capture"
    } else {
        "Pause capture"
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let center = response.rect.center();
    let color = ui.style().interact(&response).fg_stroke.color;
    if paused {
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                center + Vec2::new(-4.0, -6.0),
                center + Vec2::new(6.0, 0.0),
                center + Vec2::new(-4.0, 6.0),
            ],
            color,
            Stroke::NONE,
        ));
    } else {
        for offset in [-2.7, 2.7] {
            ui.painter().rect_filled(
                Rect::from_center_size(center + Vec2::new(offset, 0.0), Vec2::new(2.6, 11.0)),
                0.5,
                color,
            );
        }
    }
    response.on_hover_text(label)
}

fn settings_button(ui: &mut egui::Ui) -> egui::Response {
    let response = ui.add(egui::Button::new("").min_size(Vec2::splat(TOOLBAR_CONTROL_HEIGHT)));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Options"));
    let center = response.rect.center();
    let stroke = ui.style().interact(&response).fg_stroke;
    let outline = (0..32)
        .map(|step| {
            let angle = std::f32::consts::TAU * step as f32 / 32.0;
            let radius = if matches!(step % 4, 1 | 2) { 7.5 } else { 5.7 };
            center + Vec2::angled(angle) * radius
        })
        .collect();
    ui.painter().add(egui::Shape::closed_line(outline, stroke));
    ui.painter().circle_stroke(center, 2.1, stroke);
    response.on_hover_text("Options")
}

fn clear_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let button = egui::Button::new("").min_size(Vec2::splat(TOOLBAR_CONTROL_HEIGHT));
    let response = ui.add_enabled(enabled, button);
    let label = "Clear captured audio";
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let center = response.rect.center();
    let stroke = ui.style().interact(&response).fg_stroke;
    let painter = ui.painter();
    painter.rect_stroke(
        Rect::from_center_size(center + Vec2::new(0.0, 2.0), Vec2::new(9.0, 10.0)),
        1.2,
        stroke,
        egui::StrokeKind::Inside,
    );
    painter.hline(center.x - 6.0..=center.x + 6.0, center.y - 4.0, stroke);
    painter.line_segment(
        [
            center + Vec2::new(-2.2, -6.2),
            center + Vec2::new(2.2, -6.2),
        ],
        stroke,
    );
    response.on_hover_text(label)
}

fn position_in_audio(rect: Rect, x: f32) -> f32 {
    ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_accepted_drag_files_are_retained() {
        let output = std::env::temp_dir().join(format!(
            "backtracking-plugin-drag-test-{}",
            std::process::id()
        ));
        let snapshot = Arc::new(Snapshot {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.25; 32],
        });
        let prepare_file = || {
            WavClip {
                profile: WavExportProfile::default(),
                output: output.clone(),
                snapshot: snapshot.clone(),
                range: 0..32,
            }
            .write()
            .unwrap()
        };
        let canceled = prepare_file();
        let accepted = prepare_file();
        drop(PreparedDrag {
            path: canceled.clone(),
            accepted: false,
        });
        drop(PreparedDrag {
            path: accepted.clone(),
            accepted: true,
        });
        assert!(!canceled.exists());
        assert!(accepted.exists());
        std::fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn saved_snapshots_finish_without_editor_frames() {
        let output = std::env::temp_dir().join(format!(
            "backtracking-plugin-snapshot-test-{}",
            std::process::id()
        ));
        let writer = SnapshotWriter::start().unwrap();
        let snapshot = Arc::new(Snapshot {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.25; 32],
        });
        for range in [0..16, 0..32] {
            writer
                .requests
                .send(WavClip {
                    profile: WavExportProfile::default(),
                    output: output.clone(),
                    snapshot: snapshot.clone(),
                    range,
                })
                .unwrap();
        }
        let first = writer
            .results
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let second = writer
            .results
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        writer.finish();
        assert_ne!(first, second);
        assert_eq!(
            second.metadata().unwrap().len() - first.metadata().unwrap().len(),
            64
        );
        std::fs::remove_dir_all(output).unwrap();
    }
}

// A small RGBA PNG waveform preview for the native file-drag cursor.
#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
mod mac_drag {
    use std::ffi::{CString, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::ptr::{NonNull, null_mut};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use objc2::rc::{Retained, Weak};
    use objc2::runtime::{
        AnyClass, AnyObject, AnyProtocol, ClassBuilder, NSObject, ProtocolObject, Sel,
    };
    use objc2::{AnyThread, ClassType, MainThreadMarker, msg_send, sel};
    use objc2_app_kit::{
        NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSEvent,
        NSEventModifierFlags, NSEventType, NSImage, NSView,
    };
    use objc2_foundation::{NSArray, NSData, NSPoint, NSPointInRect, NSRect, NSURL};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::{ActiveDrag, DRAG_ICON, Frame, Pos2};

    static NEXT_CLASS: AtomicUsize = AtomicUsize::new(0);
    const COMPLETION: &std::ffi::CStr = c"backtrackingCompletion";

    struct Completion {
        drag: ActiveDrag,
        editor: Weak<NSView>,
        initial_bounds: NSRect,
    }

    fn editor_bounds(view: &NSView) -> Option<NSRect> {
        let window = view.window()?;
        Some(window.convertRectToScreen(view.convertRect_toView(view.bounds(), None)))
    }

    pub(super) fn primary_button_pressed() -> bool {
        NSEvent::pressedMouseButtons() & 1 != 0
    }

    pub(super) fn pointer_outside_editor(frame: &Frame) -> bool {
        let Ok(window) = frame.baseview_window().window_handle() else {
            return false;
        };
        let RawWindowHandle::AppKit(handle) = window.as_raw() else {
            return false;
        };
        // The WindowHandle borrow keeps the view valid during this geometry check.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        editor_bounds(view).is_some_and(|bounds| !NSPointInRect(NSEvent::mouseLocation(), bounds))
    }

    pub(super) fn start(frame: &Frame, origin: Pos2, drag: ActiveDrag) -> Result<(), &'static str> {
        let _main = MainThreadMarker::new().ok_or("AppKit file drag requires the main thread")?;
        if !primary_button_pressed() || !pointer_outside_editor(frame) {
            return Ok(());
        }
        let handle = frame
            .baseview_window()
            .window_handle()
            .map_err(|_| "editor window closed")?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err("editor has no AppKit view");
        };
        // The WindowHandle borrow keeps the native view valid for the duration of this call.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = view.window().ok_or("editor window detached")?;
        let initial_bounds = editor_bounds(view).ok_or("editor bounds unavailable")?;
        let location = window.mouseLocationOutsideOfEventStream();
        let timestamp = window.currentEvent().map_or(0.0, |event| event.timestamp());
        let press =
            view.convertPoint_toView(NSPoint::new(f64::from(origin.x), f64::from(origin.y)), None);
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDown, press, NSEventModifierFlags::empty(), timestamp,
            window.windowNumber(), None, 0, 1, 1.0,
        ).ok_or("could not create the drag event")?;

        let native_path =
            CString::new(drag.file.path.as_os_str().as_bytes()).map_err(|_| "invalid WAV path")?;
        let path_ptr = NonNull::new(native_path.as_ptr().cast_mut()).ok_or("invalid WAV path")?;
        // AppKit reads the NUL-terminated native path before returning the NSURL.
        let url = unsafe {
            NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
                path_ptr, false, None,
            )
        };
        let image = NSImage::initWithData(NSImage::alloc(), &NSData::from_vec(DRAG_ICON.to_vec()))
            .ok_or("could not load the drag preview")?;
        let item = NSDraggingItem::initWithPasteboardWriter(
            NSDraggingItem::alloc(),
            &ProtocolObject::from_retained(url),
        );
        let preview_center = view.convertPoint_fromView(location, None);
        let size = image.size();
        let origin = NSPoint::new(
            preview_center.x - size.width * 0.5,
            preview_center.y - size.height * 0.5,
        );
        // The preview contents are an NSImage, as required by AppKit.
        unsafe { item.setDraggingFrame_contents(NSRect::new(origin, size), Some(&*image)) };
        let items = NSArray::from_retained_slice(&[item]);
        let source = make_source(Completion {
            drag,
            editor: Weak::new(view),
            initial_bounds,
        })?;

        // Accept a nil result as an error. AppKit retains the source during a started session.
        let drag: Option<Retained<NSDraggingSession>> = unsafe {
            msg_send![view, beginDraggingSessionWithItems: &*items, event: &*event, source: &*source]
        };
        drag.map(|_| ()).ok_or("AppKit did not start the file drag")
    }

    fn make_source(completion: Completion) -> Result<Retained<AnyObject>, &'static str> {
        // Objective-C classes are process-global. Give each drag its own class so VST3, CLAP,
        // and other plugin binaries can coexist in the host. Dealloc disposes it after use.
        let sequence = NEXT_CLASS.fetch_add(1, Ordering::Relaxed);
        let name = CString::new(format!(
            "BacktrackingDrag_{:x}_{sequence}",
            &NEXT_CLASS as *const AtomicUsize as usize
        ))
        .map_err(|_| "invalid drag class name")?;
        let mut builder = ClassBuilder::new(&name, NSObject::class())
            .ok_or("could not create the drag source class")?;
        let protocol = AnyProtocol::get(c"NSDraggingSource")
            .ok_or("AppKit drag source protocol unavailable")?;
        builder.add_protocol(protocol);
        builder.add_ivar::<*mut c_void>(COMPLETION);
        // Match the AppKit protocol signatures and NSObject's dealloc signature exactly.
        unsafe {
            builder.add_method(
                sel!(draggingSession:sourceOperationMaskForDraggingContext:),
                source_mask as extern "C-unwind" fn(_, _, _, _) -> _,
            );
            builder.add_method(
                sel!(draggingSession:endedAtPoint:operation:),
                drag_ended as extern "C-unwind" fn(_, _, _, _, _),
            );
            builder.add_method(sel!(dealloc), dealloc as extern "C-unwind" fn(_, _));
        }
        let class = builder.register();
        let source: Option<Retained<AnyObject>> = unsafe { msg_send![class, new] };
        let Some(source) = source else {
            unsafe { dispose(class) };
            return Err("could not allocate a drag source");
        };
        let ivar = class
            .instance_variable(COMPLETION)
            .ok_or("drag source storage unavailable")?;
        // The source was just created using this class, and it is touched only on the main thread.
        unsafe {
            ivar.load_ptr::<*mut c_void>(&source)
                .write(Box::into_raw(Box::new(completion)).cast())
        };
        Ok(source)
    }

    extern "C-unwind" fn source_mask(
        _source: &AnyObject,
        _selector: Sel,
        _session: &NSDraggingSession,
        _context: NSDraggingContext,
    ) -> NSDragOperation {
        NSDragOperation::Copy
    }

    extern "C-unwind" fn drag_ended(
        source: &AnyObject,
        _selector: Sel,
        _session: &NSDraggingSession,
        location: NSPoint,
        operation: NSDragOperation,
    ) {
        if let Some(mut completion) = take_completion(source, source.class()) {
            let bounds = completion
                .editor
                .load()
                .as_deref()
                .and_then(editor_bounds)
                .unwrap_or(completion.initial_bounds);
            if !operation.is_empty() && !NSPointInRect(location, bounds) {
                completion.drag.accept();
            }
        }
    }

    extern "C-unwind" fn dealloc(source: &mut AnyObject, _selector: Sel) {
        let class = source.class();
        drop(take_completion(source, class));
        // This dynamically registered class has exactly one instance and is never subclassed.
        unsafe {
            let _: () = msg_send![super(source, NSObject::class()), dealloc];
            dispose(class);
        }
    }

    fn take_completion(source: &AnyObject, class: &AnyClass) -> Option<Box<Completion>> {
        let ivar = class.instance_variable(COMPLETION)?;
        // The callback and dealloc both run on the main thread. Take ownership at most once.
        let pointer = unsafe { ivar.load_ptr::<*mut c_void>(source).replace(null_mut()) };
        NonNull::new(pointer.cast::<Completion>())
            .map(|pointer| unsafe { Box::from_raw(pointer.as_ptr()) })
    }

    unsafe fn dispose(class: &AnyClass) {
        // Called after the sole source instance has been deallocated, or on allocation failure.
        unsafe { objc2::ffi::objc_disposeClassPair(std::ptr::from_ref(class).cast_mut()) };
    }
}

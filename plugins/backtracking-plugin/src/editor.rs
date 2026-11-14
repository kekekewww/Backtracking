use std::io::{self, Cursor};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    AutoResumeDelay, ChannelRoute, DurationChoice, FrozenTrack, MeterReadings, WavExportProfile,
    export_routed_tracks_with_tempo, waveform,
};
use backtracking_ui::{
    CENTER_LINE, FlagReach, LoneSample, PLOT_BACKGROUND, RECORDING_DIM, RECORDING_LIT,
    TraceStyle, TransportGlyph, WAV_SAMPLE_FORMATS, WAV_SAMPLE_RATES, accent_color,
    combo_popup_height, fit_frame_range, flag_handle_at, icon_button, meter_number, muted_accent,
    paint_guide_lines, paint_transport_glyph, paint_waveform_trace, selected_waveform_color,
    status_led, waveform_press,
};
use egui::{Color32, CursorIcon, PointerButton, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};
use nice_plug::context::gui::GuiContext;
use nice_plug_egui::baseview::dpi::LogicalSize;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, Frame, NiceEguiApp, RepaintNotifier,
    create_egui_editor,
};

use crate::session::{EditingSnapshot, Session, WaveformPeaks, WaveformTimeline, waveform_views};
use crate::settings::{
    CaptureInput, DEFAULT_ACCENT, MeterMode, SessionSettings, TRANSPARENCY_LEVELS, WaveformLayout,
};

#[cfg(target_os = "windows")]
mod win_drag;
mod preview;
use preview::{PreviewKey, PreviewRequest, PreviewResult, PreviewWorker};

const REFRESH: Duration = Duration::from_millis(80);
const RECORDING_BLINK: Duration = Duration::from_millis(500);
const TOOLBAR_CONTROL_HEIGHT: f32 = 25.0;
const TRANSPORT_GLYPH: TransportGlyph = TransportGlyph {
    play: [Vec2::new(-4.0, -6.0), Vec2::new(6.0, 0.0), Vec2::new(-4.0, 6.0)],
    pause_offset: 2.7,
    pause_bar: Vec2::new(2.6, 11.0),
};
const MIN_WAVEFORM_HEIGHT: f32 = 60.0;
const OPTIONS_EDITOR_SIZE: Vec2 = Vec2::new(680.0, 480.0);
const FLAG_SNAP_RADIUS: f32 = 8.0;
const FLAG_REACH: FlagReach = FlagReach { above: 4.0, below: 14.0, radius: FLAG_SNAP_RADIUS };
/// Alt starts a free selection, as Ctrl does.
const ALT_SELECTS: bool = true;
/// The selected waveform takes the accent's complement when it differs by at least this much.
const SELECTION_CONTRAST: u8 = 64;
const PRODUCT_ICON: &[u8] = include_bytes!("../../../assets/backtracking-icon.png");

pub type CaptureEditor = EguiEditor<CaptureUi>;

pub(crate) fn create(session: Arc<Session>) -> Option<CaptureEditor> {
    let editor_state = EguiEditorState::from_size(LogicalSize::new(640.0, 212.0), 1.0);
    create_egui_editor(
        editor_state.clone(),
        RepaintNotifier::new(),
        EguiNiceSettings::new().with_tile("Backtracking"),
        CaptureUi::new(session, editor_state),
    )
}

pub struct CaptureUi {
    session: Arc<Session>,
    editor_state: Arc<EguiEditorState>,
    product_icon: Option<TextureHandle>,
    peaks: [WaveformPeaks; 2],
    peak_layout: WaveformLayout,
    timeline: WaveformTimeline,
    markers: MarkerHistory,
    seconds: f32,
    next_refresh: Instant,
    display_size: Vec2,
    preview_worker: Option<PreviewWorker>,
    preview_requested: Option<PreviewKey>,
    editing_seconds: Option<f32>,
    displayed_zoomed: bool,
    recording_since: Option<Instant>,
    displayed_settings: Option<SessionSettings>,
    options_open: bool,
    options_tab: OptionsTab,
    editor_sizing: EditorSizing,
    view: Option<Range<u64>>,
    peak_frames: Range<u64>,
    selection: Option<Range<u64>>,
    gesture: Option<Gesture>,
    drag_preparation: Option<DragPreparation>,
    snapshot_writer: Option<SnapshotWriter>,
    native_dragging: Arc<AtomicBool>,
    drag_results: Option<Receiver<Result<SavedWav, String>>>,
    message: String,
}

enum Gesture {
    Select {
        anchor: u64,
        current: u64,
    },
    ResizeSelection {
        fixed: u64,
        current: u64,
    },
    Drag {
        origin: Pos2,
        preparation_started: bool,
    },
}

#[derive(Clone)]
struct MarkerUpdate {
    epoch: u64,
    sequence: u64,
    range: Range<u64>,
}

#[derive(Default)]
struct MarkerHistory {
    epoch: u64,
    sequence: u64,
    last: Option<MarkerUpdate>,
}

impl MarkerHistory {
    fn reset(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.sequence = 0;
        self.last = None;
    }

    fn issue(&mut self, range: Range<u64>) -> MarkerUpdate {
        self.sequence += 1;
        MarkerUpdate {
            epoch: self.epoch,
            sequence: self.sequence,
            range,
        }
    }

    fn accept(&mut self, update: MarkerUpdate) {
        if update.epoch == self.epoch
            && self
                .last
                .as_ref()
                .is_none_or(|last| last.sequence < update.sequence)
        {
            self.last = Some(update);
        }
    }

    fn range(&self) -> Option<&Range<u64>> {
        self.last.as_ref().map(|last| &last.range)
    }
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
    Sources,
    Appearance,
    Export,
}

impl OptionsTab {
    const ALL: [Self; 5] = [
        Self::Actions,
        Self::Capture,
        Self::Sources,
        Self::Appearance,
        Self::Export,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Actions => "Actions & shortcuts",
            Self::Capture => "Capture",
            Self::Sources => "Sources & connections",
            Self::Appearance => "Waveform & appearance",
            Self::Export => "Export",
        }
    }
}

#[derive(Default)]
struct EditorSizing {
    options_size: Option<Vec2>,
    full_height: Option<f32>,
    waveform_only: bool,
}

impl EditorSizing {
    fn requested_size(
        &mut self,
        current: Vec2,
        options_open: bool,
        waveform_only: bool,
        waveform_height: f32,
        toolbar_height: f32,
    ) -> Option<Vec2> {
        if options_open {
            if self.options_size.is_none() {
                self.options_size = Some(current);
                let size = current.max(OPTIONS_EDITOR_SIZE);
                return (size != current).then_some(size);
            }
            return None;
        }
        if let Some(mut size) = self.options_size.take() {
            if self.waveform_only != waveform_only {
                self.waveform_only = waveform_only;
                size.y = if waveform_only {
                    self.full_height = Some(size.y);
                    size.y - toolbar_height
                } else {
                    self.full_height.take().unwrap_or(212.0)
                };
            }
            size.y = size.y.max(MIN_WAVEFORM_HEIGHT);
            // Host resizing is asynchronous. Apply a mode changed in Options using
            // the saved dimensions, never the temporary expanded viewport.
            return (size != current).then_some(size);
        }
        if self.waveform_only == waveform_only {
            return None;
        }
        self.waveform_only = waveform_only;
        let height = if waveform_only {
            self.full_height = Some(current.y);
            if waveform_height >= current.y - 1.0 {
                current.y - toolbar_height
            } else {
                waveform_height
            }
        } else {
            self.full_height.take().unwrap_or(212.0)
        }.max(MIN_WAVEFORM_HEIGHT);
        ((height - current.y).abs() >= 1.0).then_some(Vec2::new(current.x, height))
    }
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
    wav: SavedWav,
    accepted: bool,
}

impl Drop for PreparedDrag {
    fn drop(&mut self) {
        if !self.accepted {
            for path in &self.wav.paths {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct ActiveDrag {
    file: PreparedDrag,
    session: Arc<Session>,
    active: Arc<AtomicBool>,
    results: Sender<Result<SavedWav, String>>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl ActiveDrag {
    fn accept(&mut self) {
        self.file.accepted = true;
        let _ = self.results.send(Ok(self.file.wav.clone()));
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for ActiveDrag {
    fn drop(&mut self) {
        self.session.end_waveform_gesture(false);
        self.session.cancel_waveform_selection();
        self.active.store(false, Ordering::Release);
    }
}

struct WavClip {
    profile: WavExportProfile,
    force_mono: bool,
    output: PathBuf,
    snapshots: Arc<[FrozenTrack]>,
    range: Range<usize>,
    marker: MarkerUpdate,
    /// The host tempo when the clip was taken, if the host reports one. Shown in the file name.
    tempo: Option<f64>,
}

#[derive(Clone)]
struct SavedWav {
    paths: Vec<PathBuf>,
    marker: MarkerUpdate,
}

impl SavedWav {
    fn description(&self) -> String {
        self.paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl WavClip {
    fn write(self) -> io::Result<SavedWav> {
        let paths = export_routed_tracks_with_tempo(
            &self.output,
            &self.snapshots,
            self.range,
            self.profile,
            &[ChannelRoute::Merged; 2][..self.snapshots.len()],
            self.force_mono,
            self.tempo,
        )?;
        Ok(SavedWav {
            paths: paths
                .into_iter()
                .map(|path| path.canonicalize().unwrap_or(path))
                .collect(),
            marker: self.marker,
        })
    }
}

struct SnapshotWriter {
    requests: Sender<WavClip>,
    results: Receiver<io::Result<SavedWav>>,
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
    fn track_info_changed(&mut self, info: nice_plug::prelude::TrackInfo) {
        self.session.set_host_track_name(Some(info.name()));
    }

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
        self.sync_timeline(self.session.timeline_epoch());
        while let Some((epoch, range, result)) = self.session.take_snapshot_result() {
            if epoch != self.timeline.epoch {
                continue;
            }
            self.message = match result {
                Ok(paths) => {
                    let marker = self.markers.issue(range);
                    let wav = SavedWav { paths, marker };
                    self.markers.accept(wav.marker.clone());
                    format!("Saved snapshot {}", wav.description())
                }
                Err(error) => format!("Could not save snapshot: {error}"),
            };
        }
        if let Some(results) = &self.drag_results {
            use std::sync::mpsc::TryRecvError;

            match results.try_recv() {
                Ok(Ok(wav)) => {
                    let description = wav.description();
                    self.markers.accept(wav.marker);
                    self.reset_selection_preview();
                    self.message = format!("Saved {description}");
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
            let format_changed = previous.wav_profile != settings.wav_profile
                || previous.force_mono != settings.force_mono;
            if retention_changed {
                self.next_refresh = Instant::now();
            }
            if retention_changed || output_changed || format_changed {
                self.discard_drag_preparation();
            }
            if retention_changed || output_changed || format_changed {
                self.message.clear();
                if self.gesture.take().is_some() {
                    self.session.end_waveform_gesture(self.selection.is_some());
                }
            }
        }
        self.reap_drag_preparation();
        self.collect_saved_snapshots();
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
        self.refresh_waveform(paused, settings.waveform_layout);
        ui.visuals_mut().selection.bg_fill = muted_accent(accent_color(settings.accent));

        if ui.input(|input| input.pointer.any_pressed()) {
            self.session.mark_interaction();
        }

        let waveform_only = settings.show_waveform_only && !self.options_open;
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
            (available_height - status_height).max(MIN_WAVEFORM_HEIGHT)
        };
        let wave_size = Vec2::new(ui.available_width(), wave_height);
        let (rect, response) = ui.allocate_exact_size(wave_size, Sense::click_and_drag());
        self.display_size = self.audio_rect(rect, settings.retention.seconds() as f32).size();
        self.refresh_waveform(paused, settings.waveform_layout);
        if can_toggle && gear.is_some_and(|response| response.clicked()) {
            self.options_open = true;
        }
        let settings_open = self.options_open;
        if settings_open {
            self.show_options(ui, frame, &mut settings);
        }
        self.apply_waveform_size(ui, rect.height(), settings.show_waveform_only);
        let configured_seconds = settings.retention.seconds() as f32;
        if !canceled_selection {
            self.interact_waveform(ui, frame, &response, &settings, !settings_open);
        }
        if canceled_selection {
            response.surrender_focus();
        }
        self.draw_waveform(ui, rect, configured_seconds, !settings_open, &settings);
        if !settings_open {
            let cursor = ui.input(|input| input.pointer.hover_pos())
                .filter(|position| self.selection_handle(self.audio_rect(rect, configured_seconds), *position).is_some())
                .map_or(CursorIcon::Grab, |_| CursorIcon::ResizeHorizontal);
            response.on_hover_cursor(cursor);
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
            self.session.end_waveform_gesture(self.selection.is_some());
        }
    }
}

impl Drop for CaptureUi {
    fn drop(&mut self) {
        if !self.native_dragging.load(Ordering::Acquire) {
            self.session.end_waveform_gesture(self.selection.is_some());
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
    fn new(session: Arc<Session>, editor_state: Arc<EguiEditorState>) -> Self {
        Self {
            session, editor_state,
            product_icon: None,
            peaks: std::array::from_fn(|_| WaveformPeaks::default()),
            peak_layout: WaveformLayout::default(),
            timeline: WaveformTimeline::default(),
            markers: MarkerHistory::default(),
            seconds: 0.0,
            next_refresh: Instant::now(),
            display_size: Vec2::new(624.0, 140.0),
            preview_worker: PreviewWorker::start().ok(),
            preview_requested: None,
            editing_seconds: None,
            displayed_zoomed: false,
            recording_since: None,
            displayed_settings: None,
            options_open: false,
            options_tab: OptionsTab::Actions,
            editor_sizing: EditorSizing::default(),
            view: None,
            peak_frames: 0..0,
            selection: None,
            gesture: None,
            drag_preparation: None,
            snapshot_writer: None,
            native_dragging: Arc::new(AtomicBool::new(false)),
            drag_results: None,
            message: String::new(),
        }
    }

    fn meter_readout(&self, ui: &mut egui::Ui, mode: MeterMode) {
        let value = mode.reading(self.session.meter_readings());
        let text = format!("{} {}", meter_number(Some(value), false), mode.label());
        ui.add_sized(
            [90.0, TOOLBAR_CONTROL_HEIGHT],
            egui::Label::new(egui::RichText::new(text).monospace()).truncate(),
        );
    }

    fn recording_indicator(&mut self, ui: &mut egui::Ui, paused: bool) {
        let (status, color) = if !paused && self.session.is_recording() {
            let started = self.recording_since.get_or_insert_with(Instant::now);
            let phase = started.elapsed().as_millis() / RECORDING_BLINK.as_millis();
            let color = if phase.is_multiple_of(2) { RECORDING_LIT } else { RECORDING_DIM };
            ("Recording", color)
        } else {
            self.recording_since = None;
            if paused {
                ("Paused", Color32::from_rgb(51, 56, 61))
            } else {
                ("Waiting for sound", Color32::from_rgb(74, 81, 88))
            }
        };
        status_led(ui, Vec2::splat(16.0), 4.0, color, status);
    }

    fn show_options(&mut self, ui: &egui::Ui, frame: &mut Frame, settings: &mut SessionSettings) {
        let bounds = ui.ctx().viewport_rect();
        let mut open =
            !ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut edited = settings.clone();
        let mut folder_action = None;
        let mut selected_tab = self.options_tab;
        let window_size = (bounds.size() - Vec2::splat(16.0))
            .min(OPTIONS_EDITOR_SIZE - Vec2::splat(16.0))
            .max(Vec2::new(220.0, 100.0));
        egui::Window::new("Options")
            .id(egui::Id::unique("backtracking-options"))
            .open(&mut open)
            .fixed_rect(Rect::from_center_size(bounds.center(), window_size))
            .constrain_to(bounds)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                options_layout(ui, &mut selected_tab, |ui, tab| {
                    match tab {
                        OptionsTab::Actions => self.capture_actions(ui, settings),
                        OptionsTab::Capture => capture_options(ui, &mut edited),
                        OptionsTab::Sources => source_options(
                            ui,
                            &mut edited,
                            self.session.sidechain_available(),
                            self.session.host_track_name().as_deref(),
                        ),
                        OptionsTab::Appearance => appearance_options(ui, &mut edited),
                        OptionsTab::Export => folder_action = export_options(ui, &mut edited),
                    }
                    if !self.message.is_empty() {
                        ui.separator();
                        ui.add(egui::Label::new(&self.message).wrap())
                            .on_hover_text(&self.message);
                    }
                });
            });
        self.options_tab = selected_tab;
        self.options_open = open;
        self.apply_options(settings, edited);
        if let Some(action) = folder_action {
            self.apply_folder_action(frame, settings, action);
        }
    }

    fn capture_actions(&mut self, ui: &mut egui::Ui, settings: &SessionSettings) {
        let paused = self.session.is_paused();
        let can_toggle = self.can_toggle_capture();
        ui.vertical(|ui| {
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
                .clicked()
            {
                self.save_snapshot(settings);
            }
            if ui
                .add_enabled(
                    self.can_cancel_selection(),
                    egui::Button::new("Cancel selection"),
                )
                .clicked()
            {
                self.cancel_selection();
            }
        });
    }

    fn apply_options(&mut self, settings: &mut SessionSettings, edited: SessionSettings) {
        if edited == *settings {
            return;
        }
        self.session.update_editor_settings(edited.clone());
        if edited.wav_profile != settings.wav_profile || edited.force_mono != settings.force_mono {
            self.discard_drag_preparation();
            self.message.clear();
        }
        if edited.waveform_layout != settings.waveform_layout {
            self.refresh_waveform(self.session.is_paused(), edited.waveform_layout);
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

    fn refresh_waveform(&mut self, _paused: bool, layout: WaveformLayout) {
        let now = Instant::now();
        let policy = waveform::DisplayPolicy::for_view(
            self.display_size.x, self.display_size.y,
            self.peak_frames.end.saturating_sub(self.peak_frames.start),
            self.timeline.sample_rate,
            self.gesture.is_some() || self.native_dragging.load(Ordering::Acquire),
        );
        let bins = policy.bins.max(1);
        if self.peak_layout == layout && now < self.next_refresh
            && self.peaks[0].minimum.len() == bins
        {
            return;
        }
        if self.session.editing_pending() {
            // Pin the view under the pointer without interrupting the producer.
            self.next_refresh = now + Duration::from_millis(16);
            return;
        }
        if let Some(editing) = self.session.editing_snapshot() {
            let range = clamped_view(self.view.clone(), &editing.timeline.frames);
            let key = PreviewKey {
                epoch: editing.timeline.epoch,
                editing_token: editing.token,
                range: range.clone(),
                layout,
                bins,
            };
            self.sync_timeline(editing.timeline.epoch);
            self.clamp_edit_selection(&editing.timeline.frames);
            if self.preview_requested.as_ref() != Some(&key) {
                if let Some(worker) = &self.preview_worker {
                    worker.request(PreviewRequest { key: key.clone(), snapshots: editing.snapshots.clone() });
                    self.preview_requested = Some(key.clone());
                } else {
                    self.message = "Waveform preview worker unavailable.".into();
                }
            }
            let result = self.preview_worker.as_ref().and_then(PreviewWorker::try_take);
            self.apply_edit_preview(&editing, &key, result);
            self.next_refresh = now + Duration::from_millis(16);
            return;
        }
        self.preview_requested = None;
        self.editing_seconds = None;
        if self.selection.is_some() && self.gesture.is_none() {
            self.reset_selection_preview();
        }
        let mut peaks = self.peaks.clone();
        let requested = self.view.clone();
        let Some(timeline) = self.session.try_waveform_in_range_with_bins(
            layout, requested.clone(), bins, &mut peaks,
        ) else {
            self.next_refresh = now + Duration::from_millis(16);
            return;
        };
        self.sync_timeline(timeline.epoch);
        self.seconds = timeline.duration();
        self.peak_frames = clamped_view(requested, &timeline.frames);
        if self.peak_frames == timeline.frames {
            self.view = None;
        }
        self.displayed_zoomed = self.view.is_some();
        if let Some(range) = &mut self.selection {
            let clipped = range.start.max(timeline.frames.start)..range.end.min(timeline.frames.end);
            if clipped.is_empty() {
                self.cancel_selection();
            } else if *range != clipped {
                *range = clipped;
                self.session.set_snapshot_selection(self.selection.clone());
            }
        }
        self.timeline = timeline;
        self.peak_layout = layout;
        self.peaks = peaks;
        self.next_refresh = now + REFRESH;
    }

    fn clamp_edit_selection(&mut self, bounds: &Range<u64>) {
        match &mut self.gesture {
            Some(Gesture::Select { anchor, current }) => {
                *anchor = (*anchor).clamp(bounds.start, bounds.end);
                *current = (*current).clamp(bounds.start, bounds.end);
            }
            Some(Gesture::ResizeSelection { fixed, current }) => {
                *fixed = (*fixed).clamp(bounds.start, bounds.end);
                *current = (*current).clamp(bounds.start, bounds.end);
            }
            _ => {}
        }
        if let Some(range) = &self.selection {
            let clipped = range.start.max(bounds.start)..range.end.min(bounds.end);
            let clipped = (!clipped.is_empty()).then_some(clipped);
            if self.selection != clipped {
                self.selection = clipped;
                self.session.set_snapshot_selection(self.selection.clone());
            }
        }
    }

    fn apply_edit_preview(
        &mut self,
        editing: &EditingSnapshot,
        key: &PreviewKey,
        result: Option<PreviewResult>,
    ) -> bool {
        let Some(result) = result.filter(|result| &result.key == key) else { return false; };
        // Apply geometry and data together: pending scans must never reinterpret
        // old peaks against a newly requested zoom or pan range.
        self.timeline = editing.timeline.as_ref().clone();
        self.seconds = self.timeline.duration();
        self.peak_frames = result.key.range;
        self.displayed_zoomed = self.peak_frames != self.timeline.frames;
        self.peaks = result.peaks;
        self.peak_layout = key.layout;
        true
    }

    fn sync_timeline(&mut self, epoch: u64) {
        if self.timeline.epoch == epoch {
            return;
        }
        if self.gesture.take().is_some() {
            self.session.end_waveform_gesture(false);
        }
        self.reset_selection_preview();
        self.view = None;
        self.displayed_zoomed = false;
        self.peak_frames = 0..0;
        self.markers.reset(epoch);
        self.timeline = WaveformTimeline {
            epoch,
            ..WaveformTimeline::default()
        };
        self.peaks.fill(WaveformPeaks::default());
        self.seconds = 0.0;
    }

    fn can_toggle_capture(&self) -> bool {
        self.gesture.is_none() && !self.native_dragging.load(Ordering::Acquire)
    }

    fn toggle_capture(&mut self, ui: &egui::Ui, paused: bool) {
        if paused {
            self.session.resume();
        } else {
            self.session.pause();
            self.next_refresh = Instant::now();
        }
        ui.ctx().request_repaint();
    }

    fn clear_capture(&mut self) {
        self.session.clear();
        self.sync_timeline(self.session.timeline_epoch());
        self.recording_since = None;
        self.message.clear();
    }

    fn reset_selection_preview(&mut self) {
        self.clear_selection_preview();
        self.discard_drag_preparation();
    }

    fn clear_selection_preview(&mut self) {
        self.selection = None;
        self.preview_requested = None;
        self.editing_seconds = None;
        self.session.set_snapshot_selection(None);
        self.next_refresh = Instant::now();
    }

    fn can_cancel_selection(&self) -> bool {
        !self.native_dragging.load(Ordering::Acquire)
            && (self.selection.is_some()
                || matches!(self.gesture, Some(Gesture::Select { .. } | Gesture::ResizeSelection { .. })))
    }

    fn cancel_selection(&mut self) {
        self.gesture = None;
        self.reset_selection_preview();
        self.message.clear();
        self.session.cancel_waveform_selection();
    }

    fn discard_drag_preparation(&mut self) {
        if let Some(preparation) = &mut self.drag_preparation {
            preparation.cancel = None;
        }
    }

    fn apply_waveform_size(
        &mut self,
        ui: &egui::Ui,
        wave_height: f32,
        waveform_only: bool,
    ) {
        let current = ui.ctx().viewport_rect().size();
        let margin = (current.y - ui.max_rect().height()).max(0.0);
        let toolbar_height = TOOLBAR_CONTROL_HEIGHT
            .max(ui.text_style_height(&egui::TextStyle::Heading)) + ui.spacing().item_spacing.y;
        if let Some(size) = self.editor_sizing.requested_size(
            current,
            self.options_open,
            waveform_only,
            wave_height + margin,
            toolbar_height,
        ) {
            ui.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
    }

    fn audio_rect(&self, rect: Rect, seconds: f32) -> Rect {
        let inner = rect.shrink(8.0);
        let seconds = self.editing_seconds.unwrap_or(seconds);
        let width = if self.displayed_zoomed && !self.peak_frames.is_empty() {
            inner.width()
        } else {
            inner.width() * (self.seconds / seconds).clamp(0.0, 1.0)
        };
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
        if inner.width() <= 0.0 || inner.height() <= 0.0 {
            return;
        }
        let opacity = ((100 - u16::from(settings.panel_transparency)) * 255 / 100) as u8;
        let panel = if settings.border_enabled { rect.shrink(1.0) } else { rect };
        painter.rect_filled(
            panel,
            6.0,
            Color32::from_rgba_unmultiplied(
                PLOT_BACKGROUND.r(), PLOT_BACKGROUND.g(), PLOT_BACKGROUND.b(), opacity),
        );
        if settings.border_enabled {
            let [r, g, b] = settings.border_color.unwrap_or(settings.accent);
            let alpha = if settings.border_follows_background { opacity } else { 255 };
            painter.rect_stroke(
                panel,
                6.0,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(r, g, b, alpha)),
                egui::StrokeKind::Outside,
            );
        }
        let audio_rect = self.audio_rect(rect, seconds);
        let width = audio_rect.width();
        let policy = self.display_policy(audio_rect);

        let explicit_range = match &self.gesture {
            Some(Gesture::Select { anchor, current } | Gesture::ResizeSelection { fixed: anchor, current }) => {
                Some((*anchor).min(*current)..(*anchor).max(*current))
            }
            _ => self.selection.clone(),
        };
        let hover_preview = !waveform_only
            && explicit_range.is_none()
            && match &self.gesture {
                Some(Gesture::Select { .. } | Gesture::ResizeSelection { .. }) => false,
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
        let color = accent_color(settings.accent);
        if hover_preview && width > 0.0 {
            painter.rect_filled(audio_rect, 2.0, muted_accent(color));
        }

        let selected_color = selected_waveform_color(color, SELECTION_CONTRAST);
        if width > 0.0 && opacity > 0 {
            self.draw_reference_lines(&painter, audio_rect, settings, opacity);
        }
        let rows = if self.timeline.b_channels.is_some() {
            2
        } else {
            waveform_views(self.peak_layout, self.timeline.channels).len()
        };
        let row_height = inner.height() / rows as f32;
        let extent = row_height * 0.47;
        let selection_clip = explicit_range.as_ref().and_then(|range|
            waveform_selection_clip(audio_rect, &self.peak_frames, range));
        for (row, peaks) in self.peaks[..rows].iter().enumerate() {
            let middle = inner.top() + (row as f32 + 0.5) * row_height;
            if !waveform_only {
                painter.hline(
                    inner.x_range(),
                    middle,
                    Stroke::new(policy.seconds_stroke, CENTER_LINE),
                );
            }
            if width <= 0.0 {
                continue;
            }
            let geometry = waveform::waveform_geometry(
                &peaks.minimum, &peaks.maximum, self.peak_frames.clone());
            paint_waveform_geometry(&painter, &geometry, audio_rect, middle, extent,
                policy.waveform_stroke, color);
            if let Some(clip) = selection_clip {
                paint_waveform_geometry(&painter.with_clip_rect(clip), &geometry, audio_rect,
                    middle, extent, policy.waveform_stroke, selected_color);
            }
        }
        if width > 0.0 {
            for (flags, muted) in [
                (self.markers.range(), explicit_range.is_some()),
                (explicit_range.as_ref(), false),
            ] {
                if let Some(flags) = flags {
                    for (boundary, direction) in [(flags.start, 1.0), (flags.end, -1.0)] {
                        if let Some(fraction) =
                            waveform::flag_fraction(boundary, self.peak_frames.clone())
                        {
                            paint_flag(&painter, audio_rect, fraction as f32, direction, muted);
                        }
                    }
                }
            }
        }
    }

    fn draw_reference_lines(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        settings: &SessionSettings,
        opacity: u8,
    ) {
        let policy = self.display_policy(rect);
        let guides = self.visible_guides(rect, settings);
        let draw = |frames: &[u64], color, alpha: u8, stroke: f32| {
            let [r, g, b] = color;
            let alpha = (u16::from(alpha) * u16::from(opacity) / 255) as u8;
            paint_guide_lines(painter, rect, &self.peak_frames, frames,
                Stroke::new(stroke, Color32::from_rgba_unmultiplied(r, g, b, alpha)));
        };
        draw(&guides.seconds, [158, 170, 175], 76, policy.seconds_stroke);
        draw(&guides.bars, settings.accent, 144, policy.bar_stroke);
    }

    fn display_policy(&self, rect: Rect) -> waveform::DisplayPolicy {
        waveform::DisplayPolicy::for_view(
            rect.width(), rect.height(),
            self.peak_frames.end.saturating_sub(self.peak_frames.start),
            self.timeline.sample_rate,
            self.gesture.is_some() || self.native_dragging.load(Ordering::Acquire),
        )
    }

    fn visible_guides(&self, rect: Rect, settings: &SessionSettings) -> waveform::TimelineGuides {
        waveform::timeline_guides(
            self.peak_frames.clone(), self.timeline.sample_rate, u32::from(settings.seconds_interval),
            &self.timeline.bar_markers, settings.show_bar_lines, settings.show_seconds_lines,
            settings.panel_transparency < 100, &self.display_policy(rect),
        )
    }

    fn selection_handle(&self, rect: Rect, position: Pos2) -> Option<u64> {
        let flags = self.selection.as_ref().or_else(|| self.markers.range())?;
        flag_handle_at(rect, self.peak_frames.clone(), flags, position, FLAG_REACH)
            .map(|(fixed, _)| fixed)
    }

    fn frame_at_pointer(&self, rect: Rect, x: f32, snap: bool, settings: &SessionSettings) -> u64 {
        let mut flags = Vec::new();
        let guides = if snap {
            if let Some(range) = self.markers.range() { flags.extend([range.start, range.end]); }
            match &self.gesture {
                Some(Gesture::ResizeSelection { fixed, .. }) => flags.push(*fixed),
                Some(Gesture::Select { anchor, .. }) => flags.push(*anchor),
                _ => if let Some(range) = &self.selection { flags.extend([range.start, range.end]); },
            }
            self.visible_guides(rect, settings)
        } else { waveform::TimelineGuides::default() };
        frame_at_pointer_with_guides(rect, x, self.peak_frames.clone(), &flags, &guides)
    }

    fn interact_viewport(&mut self, ui: &egui::Ui, response: &egui::Response, settings: &SessionSettings) {
        let rect = response.rect;
        let (pointer, wheel) =
            ui.input(|input| (input.pointer.hover_pos(), input.smooth_scroll_delta().y));
        let pan = if response.dragged_by(PointerButton::Middle) {
            response.drag_delta().x
        } else {
            0.0
        };
        let full = self.timeline.frames.clone();
        let count = full.end - full.start;
        if count <= 1 || (wheel == 0.0 && pan == 0.0) {
            return;
        }
        let current = clamped_view(self.view.clone(), &full);
        let current_len = current.end - current.start;
        let next = if let Some(pointer) = pointer.filter(|pointer| rect.contains(*pointer)) && wheel != 0.0 {
            let audio_rect = self.audio_rect(rect, settings.retention.seconds() as f32);
            Some(wheel_zoom_view(&full, &current, wheel, pointer.x, audio_rect, rect.shrink(8.0)))
        } else if self.view.is_some() && pan != 0.0 {
            let delta = (-f64::from(pan) / f64::from(rect.width().max(1.0)) * current_len as f64).round() as i64;
            Some(fit_frame_range(&full, current.start.saturating_add_signed(delta), current_len))
        } else {
            None
        };
        if let Some(next) = next {
            self.view = (next != full).then_some(next);
            self.next_refresh = Instant::now();
            self.refresh_waveform(self.session.is_paused(), settings.waveform_layout);
        }
    }

    fn interact_waveform(
        &mut self,
        ui: &egui::Ui,
        frame: &Frame,
        waveform: &egui::Response,
        settings: &SessionSettings,
        allow_new_gesture: bool,
    ) {
        let rect = waveform.rect;
        let (position, press, down, shift) = ui.input(|input| {
            let press = waveform_press(&input.events, ALT_SELECTS);
            (
                input.pointer.interact_pos(),
                press,
                input.pointer.button_down(PointerButton::Primary),
                input.modifiers.shift,
            )
        });
        if self.native_dragging.load(Ordering::Acquire) {
            return;
        }
        if allow_new_gesture {
            self.interact_viewport(ui, waveform, settings);
        }

        if allow_new_gesture
            && let Some((position, selecting, snap)) =
                press.filter(|(position, _, _)| rect.contains(*position))
        {
            self.discard_drag_preparation();
            self.next_refresh = Instant::now();
            self.refresh_waveform(self.session.is_paused(), settings.waveform_layout);
            let audio_rect = self.audio_rect(rect, settings.retention.seconds() as f32);
            if let Some(fixed) = self.selection_handle(audio_rect, position) {
                self.editing_seconds.get_or_insert(settings.retention.seconds() as f32);
                self.session.begin_waveform_gesture();
                waveform.request_focus();
                let current = self.frame_at_pointer(audio_rect, position.x, snap, settings);
                self.gesture = Some(Gesture::ResizeSelection { fixed, current });
            } else if selecting {
                self.editing_seconds.get_or_insert(settings.retention.seconds() as f32);
                self.session.begin_waveform_gesture();
                waveform.request_focus();
                let anchor = self.frame_at_pointer(audio_rect, position.x, snap, settings);
                self.gesture = Some(Gesture::Select { anchor, current: anchor });
            } else {
                self.gesture = Some(Gesture::Drag {
                    origin: position,
                    preparation_started: false,
                });
            }
        }

        if matches!(self.gesture, Some(Gesture::Select { .. } | Gesture::ResizeSelection { .. }))
            && let Some(position) = position
            && !press.is_some_and(|(press_position, _, _)| press_position == position)
        {
            let audio_rect = self.audio_rect(rect, settings.retention.seconds() as f32);
            let frame = self.frame_at_pointer(audio_rect, position.x, shift, settings);
            if let Some(Gesture::Select { current, .. } | Gesture::ResizeSelection { current, .. }) = &mut self.gesture {
                *current = frame;
            }
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
                        if self.session.editing_snapshot().is_none() && !self.session.editing_pending() {
                            self.editing_seconds.get_or_insert(settings.retention.seconds() as f32);
                            self.session.begin_waveform_gesture();
                            self.message = "Preparing selected audio…".into();
                        } else if self.session.editing_pending() {
                            self.message = "Preparing selected audio…".into();
                        } else {
                            self.gesture = Some(Gesture::Drag {
                                origin,
                                preparation_started: true,
                            });
                            self.begin_drag_preparation();
                        }
                    }
                } else if let Some(file) = self.take_prepared_drag() {
                    self.gesture = None;
                    self.selection = None;
                    self.session.set_snapshot_selection(None);
                    self.message.clear();
                    self.start_native_drag(frame, file, origin);
                }
            }
        } else if let Some(gesture) = self.gesture.take() {
            if let Gesture::Select { anchor, current } | Gesture::ResizeSelection { fixed: anchor, current } = gesture {
                let range = anchor.min(current)..anchor.max(current);
                self.selection = (!range.is_empty()).then_some(range);
                self.session.set_snapshot_selection(self.selection.clone());
            } else {
                self.clear_selection_preview();
            }
            self.discard_drag_preparation();
            self.session.end_waveform_gesture(self.selection.is_some());
            if self.selection.is_none() {
                self.session.cancel_waveform_selection();
            }
        }
    }

    fn begin_drag_preparation(&mut self) {
        let settings = self.session.settings();
        let Some((epoch, snapshots)) = self.session.positioned_snapshots(false)
            .filter(|(_, snapshots)| snapshots.first().is_some_and(|snapshot| snapshot.frame_count() > 0))
        else {
            self.message = "No captured audio to save yet.".into();
            return;
        };
        self.sync_timeline(epoch);
        let Some(clip) = self.wav_clip(
            snapshots.into(),
            settings.output_dir,
            settings.wav_profile,
            settings.force_mono,
        ) else {
            self.message = "The selected audio is no longer available.".into();
            return;
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
                let file = clip.write().map(|wav| PreparedDrag {
                    wav,
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
        if !selected {
            if self.seconds <= 0.0 {
                self.message = "No captured audio to save yet.".into();
                return;
            }
            self.session.request_snapshot();
            self.message = "Saving snapshot…".into();
            return;
        }
        if selected && self.session.editing_pending() {
            self.message = "Preparing selected audio…".into();
            return;
        }
        let snapshots = self.session.positioned_snapshots(false);
        let Some((epoch, snapshots)) = snapshots.filter(|(_, snapshots)|
            snapshots.first().is_some_and(|snapshot| snapshot.frame_count() > 0))
        else {
            self.message = "No captured audio to save yet.".into();
            return;
        };
        self.sync_timeline(epoch);
        let Some(clip) = self.wav_clip(
            snapshots.into(),
            settings.saved_dir.clone(),
            settings.wav_profile,
            settings.force_mono,
        ) else {
            self.message = "The selected audio is no longer available.".into();
            return;
        };
        if selected {
            self.cancel_selection();
        }
        let result = self.queue_saved_snapshot(clip);
        self.message = match result {
            Ok(()) => "Saving snapshot…".into(),
            Err(error) => format!("Could not save snapshot: {error}"),
        };
    }

    fn wav_clip(
        &mut self,
        snapshots: Arc<[FrozenTrack]>,
        output: PathBuf,
        profile: WavExportProfile,
        force_mono: bool,
    ) -> Option<WavClip> {
        let first = snapshots.first()?;
        let selected = self.selection.clone().unwrap_or_else(|| first.frame_range());
        let range = export_frame_range(first, &selected)?;
        Some(WavClip {
            profile,
            force_mono,
            output,
            snapshots,
            range,
            marker: self.markers.issue(selected),
            tempo: self.session.host_tempo(),
        })
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
                    Ok(wav) => {
                        let description = wav.description();
                        self.markers.accept(wav.marker);
                        format!("Saved snapshot {description}")
                    }
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
        self.session.end_waveform_gesture(false);
        self.session.cancel_waveform_selection();
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

fn export_frame_range(snapshot: &FrozenTrack, selection: &Range<u64>) -> Option<Range<usize>> {
    let bounds = snapshot.frame_range();
    if selection.is_empty() || selection.start < bounds.start || selection.end > bounds.end {
        return None;
    }
    Some((selection.start - bounds.start) as usize..(selection.end - bounds.start) as usize)
}

fn options_layout<R>(
    ui: &mut egui::Ui,
    selected: &mut OptionsTab,
    content: impl FnOnce(&mut egui::Ui, OptionsTab) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    ui.scope(|ui| {
        // Options uses a readable type scale without increasing the compact transport.
        // `RichText::strong` only changes color in egui; use the bundled bold face
        // for the actual heading weight. Headless contexts can use the default face.
        let bold_family = egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into());
        let bold_family = if ui.fonts(|fonts| fonts.families().contains(&bold_family)) {
            bold_family
        } else {
            egui::FontFamily::Proportional
        };
        let style = ui.style_mut();
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(13.0));
        style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::new(21.0, bold_family.clone()));
        style.text_styles.insert(egui::TextStyle::Name("options-field".into()), egui::FontId::new(16.0, bold_family.clone()));
        style.spacing.interact_size.y = 28.0;
        style.spacing.button_padding = Vec2::new(8.0, 4.0);
        style.spacing.item_spacing.y = 6.0;
        let size = ui.available_size();
        let navigation_width = (size.x * 0.3).clamp(104.0, 170.0).min(size.x * 0.42);
        ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(navigation_width, size.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(navigation_width);
                egui::ScrollArea::vertical()
                    .id_salt("options-navigation")
                    .auto_shrink([false, false])
                    .max_height(size.y)
                    .show(ui, |ui| {
                        for tab in OptionsTab::ALL {
                            if ui.add_sized(
                                [ui.available_width(), 34.0],
                                egui::Button::new(egui::RichText::new(tab.label())
                                    .font(egui::FontId::new(16.0, bold_family.clone())))
                                    .selected(*selected == tab)
                                    .frame(false)
                                    .wrap(),
                            ).on_hover_text(tab.label()).clicked() {
                                *selected = tab;
                            }
                        }
                    });
            },
        );
        ui.separator();
        let width = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(width, size.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(width);
                egui::ScrollArea::vertical()
                    .id_salt(("options-content", *selected as u8))
                    .auto_shrink([false, false])
                    .max_width(width)
                    .max_height(size.y)
                    .show(ui, |ui| {
                        // A ScrollArea inherits its parent's layout. Keep the form explicitly
                        // vertical so each setting advances down, including narrow host editors.
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.set_width(ui.available_width());
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            ui.add(egui::Label::new(egui::RichText::new(selected.label()).heading().strong()).wrap());
                            ui.separator();
                            content(ui, *selected)
                        }).inner
                    })
            },
        ).inner
        }).inner
    }).inner
}

fn option_row(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label)
            .text_style(egui::TextStyle::Name("options-field".into()))
            .strong());
        content(ui);
    });
    ui.add_space(10.0);
}

fn option_combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut T,
    choices: impl IntoIterator<Item = (T, String)>,
) {
    let choices: Vec<_> = choices.into_iter().collect();
    let selected = choices.iter().find(|(choice, _)| choice == value)
        .map_or("", |(_, label)| label.as_str());
    option_row(ui, label, |ui| {
        let height = combo_popup_height(ui, choices.len(), ui.spacing().item_spacing.y);
        egui::ComboBox::from_id_salt(label)
            .selected_text(selected)
            .width(ui.available_width())
            .height(height)
            .popup_style(ui.style().as_ref().clone().into())
            .truncate()
            .show_ui(ui, |ui| {
                for (choice, label) in &choices {
                    ui.selectable_value(value, *choice, label.as_str());
                }
            });
    });
}

fn capture_options(ui: &mut egui::Ui, settings: &mut SessionSettings) {
    option_combo(ui, "History", &mut settings.retention,
        DurationChoice::ALL.map(|duration| (duration, duration_label(duration.seconds()))));
    option_combo(ui, "Auto-resume", &mut settings.idle_resume,
        AutoResumeDelay::ALL.map(|delay| (delay, auto_resume_label(delay))));
    ui.checkbox(&mut settings.retain_silence, "Keep quiet sections");
}

/// "Off" leaves a paused capture paused. It is stored as 0 seconds, which never meant "at once".
fn auto_resume_label(delay: AutoResumeDelay) -> String {
    match delay.delay() {
        Some(delay) => format!("{} s", delay.as_secs()),
        None => "Off".into(),
    }
}

fn duration_label(seconds: u32) -> String {
    if seconds >= 60 {
        format!("{} min", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}

fn source_options(
    ui: &mut egui::Ui,
    settings: &mut SessionSettings,
    sidechain_available: bool,
    host_track_name: Option<&str>,
) {
    let mut input = if sidechain_available { settings.capture_input } else { CaptureInput::Main };
    ui.add_enabled_ui(sidechain_available, |ui| {
        option_combo(ui, "Capture input", &mut input,
            [CaptureInput::Main, CaptureInput::Sidechain, CaptureInput::Both]
                .map(|input| (input, input.label().to_owned())));
    });
    if sidechain_available {
        settings.capture_input = input;
    }
    ui.checkbox(&mut settings.sender_enabled, "Send to Backtracking desktop");
    if settings.sender_enabled {
        if let Some(name) = host_track_name {
            option_row(ui, "Sender name", |ui| {
                ui.add(egui::Label::new(name).truncate()).on_hover_text(name);
            });
        } else {
            option_row(ui, "Sender name", |ui| {
                ui.add(egui::TextEdit::singleline(&mut settings.sender_name)
                    .desired_width(ui.available_width())
                    .char_limit(80)
                    .hint_text("Sender name"));
            });
        }
    }
}

fn export_options(ui: &mut egui::Ui, settings: &mut SessionSettings) -> Option<FolderAction> {
    wav_format_controls(ui, &mut settings.wav_profile);
    option_combo(ui, "WAV channels", &mut settings.force_mono,
        [(false, "Original channels".to_owned()), (true, "Mono".to_owned())]);
    ui.separator();
    folder_options(ui, settings)
}

fn appearance_options(ui: &mut egui::Ui, settings: &mut SessionSettings) {
    ui.checkbox(&mut settings.show_waveform_only, "Show waveform only");
    ui.checkbox(&mut settings.show_bar_lines, "Bar lines");
    ui.checkbox(&mut settings.show_seconds_lines, "Seconds lines");
    if settings.show_seconds_lines {
        option_combo(ui, "Seconds interval", &mut settings.seconds_interval,
            [1, 5, 10].map(|seconds| (seconds, format!("{seconds} s"))));
    }
    ui.checkbox(&mut settings.border_enabled, "Waveform border");
    if settings.border_enabled {
        ui.checkbox(&mut settings.border_follows_background, "Fade with background");
        let mut custom = settings.border_color.is_some();
        if ui.checkbox(&mut custom, "Custom border color").changed() {
            settings.border_color = custom.then_some(settings.accent);
        }
        if let Some(color) = &mut settings.border_color {
            option_row(ui, "Border color", |ui| { ui.color_edit_button_srgb(color); });
        }
    }
    option_combo(ui, "Waveform display", &mut settings.waveform_layout,
        WaveformLayout::ALL.map(|layout| (layout, layout.label().to_owned())));
    option_combo(ui, "Panel transparency", &mut settings.panel_transparency,
        TRANSPARENCY_LEVELS.map(|percent| (percent, format!("{percent}%"))));
    option_combo(ui, "Meter", &mut settings.meter_mode,
        MeterMode::ALL.map(|mode| (mode, mode.label().to_owned())));
    option_row(ui, "Theme color", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.color_edit_button_srgb(&mut settings.accent);
            if ui.button("Reset color").clicked() {
                settings.accent = DEFAULT_ACCENT;
            }
        });
    });
}

fn folder_options(ui: &mut egui::Ui, settings: &SessionSettings) -> Option<FolderAction> {
    let mut action = None;
    for kind in [FolderKind::Export, FolderKind::Saved] {
        let folder = kind.path(settings).display().to_string();
        option_row(ui, kind.label(), |ui| {
            ui.add(egui::Label::new(&folder).truncate()).on_hover_text(&folder);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Select folder").on_hover_text(&folder).clicked() {
                    action = Some(FolderAction::Select(kind));
                }
                if ui.button("Open folder").on_hover_text(&folder).clicked() {
                    action = Some(FolderAction::Open(kind));
                }
            });
        });
    }
    action
}

fn wav_format_controls(ui: &mut egui::Ui, profile: &mut WavExportProfile) {
    option_combo(ui, "WAV sample rate", &mut profile.sample_rate,
        WAV_SAMPLE_RATES.map(|(rate, label)| (rate, label.to_owned())));
    option_combo(ui, "WAV sample format", &mut profile.sample_format,
        WAV_SAMPLE_FORMATS.map(|(format, label)| (format, label.to_owned())));
}

fn capture_button(ui: &mut egui::Ui, paused: bool, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(enabled, icon_button(TOOLBAR_CONTROL_HEIGHT));
    let label = if paused {
        "Start capture"
    } else {
        "Pause capture"
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let center = response.rect.center();
    let color = ui.style().interact(&response).fg_stroke.color;
    paint_transport_glyph(ui.painter(), center, paused, color, &TRANSPORT_GLYPH);
    response.on_hover_text(label)
}

fn settings_button(ui: &mut egui::Ui) -> egui::Response {
    let response = ui.add(icon_button(TOOLBAR_CONTROL_HEIGHT));
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
    let response = ui.add_enabled(enabled, icon_button(TOOLBAR_CONTROL_HEIGHT));
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

fn clamped_view(view: Option<Range<u64>>, full: &Range<u64>) -> Range<u64> {
    view.map(|view| view.start.max(full.start)..view.end.min(full.end))
        .filter(|view| !view.is_empty())
        .unwrap_or_else(|| full.clone())
}

fn wheel_zoom_view(
    full: &Range<u64>, current: &Range<u64>, wheel: f32, pointer_x: f32,
    audio_rect: Rect, viewport_rect: Rect,
) -> Range<u64> {
    let count = full.end.saturating_sub(full.start);
    let current_len = current.end.saturating_sub(current.start);
    if count <= 1 || current_len == 0 {
        return current.clone();
    }
    // A wider editor must not make an already finer view zoom out on wheel-in.
    let minimum = waveform::minimum_zoom_frames(audio_rect.width(), count).min(current_len).max(1);
    let anchor = f64::from(((pointer_x - audio_rect.left()) / audio_rect.width().max(1.0)).clamp(0.0, 1.0));
    let factor = 2.0_f64.powf(-f64::from(wheel.clamp(-250.0, 250.0)) / 100.0);
    let length = ((current_len as f64 * factor).round() as u64).clamp(minimum, count);
    let anchor_frame = current.start.saturating_add((current_len as f64 * anchor).round() as u64);
    let next_anchor = f64::from(((pointer_x - viewport_rect.left()) / viewport_rect.width().max(1.0)).clamp(0.0, 1.0));
    let start = anchor_frame.saturating_sub((length as f64 * next_anchor).round() as u64);
    fit_frame_range(full, start, length)
}

fn waveform_selection_clip(rect: Rect, visible: &Range<u64>, selection: &Range<u64>) -> Option<Rect> {
    let frames = visible.end.saturating_sub(visible.start);
    let start = selection.start.max(visible.start);
    let end = selection.end.min(visible.end);
    if frames == 0 || start >= end || rect.width() <= 0.0 {
        return None;
    }
    let x = |frame: u64| rect.left() + rect.width()
        * ((frame - visible.start) as f64 / frames as f64) as f32;
    Some(Rect::from_min_max(Pos2::new(x(start), rect.top()), Pos2::new(x(end), rect.bottom())))
}

fn paint_waveform_geometry(
    painter: &egui::Painter, geometry: &waveform::WaveformGeometry, rect: Rect,
    middle: f32, extent: f32, stroke_width: f32, color: Color32,
) {
    let position = |x: f32, value: f32| Pos2::new(rect.left() + rect.width() * x, middle - value * extent);
    paint_waveform_trace(painter, geometry, position, &TraceStyle {
        stroke: Stroke::new(stroke_width, color),
        fill: Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 96),
        lone_sample: LoneSample::Dash(stroke_width.max(1.0)),
    });
}

#[cfg(test)]
fn frame_at_pointer(rect: Rect, x: f32, visible: Range<u64>, flags: Option<&Range<u64>>) -> u64 {
    frame_at_pointer_with_bars(rect, x, visible, flags, [])
}

#[cfg(test)]
fn frame_at_pointer_with_bars(
    rect: Rect,
    x: f32,
    visible: Range<u64>,
    flags: Option<&Range<u64>>,
    bars: impl IntoIterator<Item = u64>,
) -> u64 {
    let flags: Vec<_> = flags.into_iter().flat_map(|range| [range.start, range.end]).collect();
    let guides = waveform::TimelineGuides { bars: bars.into_iter().collect(), seconds: Vec::new() };
    frame_at_pointer_with_guides(rect, x, visible, &flags, &guides)
}

fn frame_at_pointer_with_guides(
    rect: Rect,
    x: f32,
    visible: Range<u64>,
    flags: &[u64],
    guides: &waveform::TimelineGuides,
) -> u64 {
    let width = f64::from(rect.width());
    if width <= 0.0 || visible.is_empty() {
        return visible.start;
    }
    let fraction = ((f64::from(x) - f64::from(rect.left())) / width).clamp(0.0, 1.0);
    let tolerance = f64::from(FLAG_SNAP_RADIUS) / width;
    let fraction = waveform::snap_to_visible_markers(fraction, visible.clone(), flags, guides, tolerance);
    visible.start + ((visible.end - visible.start) as f64 * fraction).round() as u64
}

fn paint_flag(painter: &egui::Painter, rect: Rect, fraction: f32, direction: f32, muted: bool) {
    let x = rect.left() + rect.width() * fraction;
    let y = rect.top() + 1.0;
    let size = rect.height().min(12.0);
    let color = if muted {
        Color32::from_rgb(164, 131, 79)
    } else {
        Color32::from_rgb(255, 199, 103)
    };
    let outline = Stroke::new(0.75, Color32::from_gray(35));
    painter.line_segment(
        [Pos2::new(x, y), Pos2::new(x, y + size)],
        Stroke::new(1.25, color),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(x, y),
            Pos2::new(x + direction * size * 0.62, y + size * 0.25),
            Pos2::new(x, y + size * 0.5),
        ],
        color,
        outline,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_resume_choices_are_labelled_by_what_they_do() {
        let labels = AutoResumeDelay::ALL.map(auto_resume_label);
        assert_eq!(labels, ["Off", "5 s", "10 s"]);
    }

    fn rendered_waveform(
        minimum: &[f32], maximum: &[f32], visible: Range<u64>, selection: Option<Range<u64>>,
    ) -> egui::FullOutput {
        let session = Arc::new(Session::new(crate::settings::SettingsStore::default()));
        let editor = EguiEditorState::from_size(LogicalSize::new(640.0, 212.0), 1.0);
        let mut capture = CaptureUi::new(session, editor);
        capture.timeline = WaveformTimeline { frames: visible.clone(), sample_rate: 48_000,
            channels: 1, ..WaveformTimeline::default() };
        capture.peak_frames = visible;
        capture.displayed_zoomed = true;
        capture.peaks[0] = WaveformPeaks { minimum: minimum.to_vec(), maximum: maximum.to_vec() };
        capture.selection = selection;
        let settings = SessionSettings { show_waveform_only: true, ..SessionSettings::default() };
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 200.0))),
            ..Default::default()
        }, |ui| capture.draw_waveform(ui,
            Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(240.0, 100.0)), 1.0, false, &settings));
        output.textures_delta.clear();
        output
    }

    #[test]
    fn rendered_waveform_overview_has_a_filled_continuous_strip_and_two_outlines() {
        let output = rendered_waveform(&[-0.8, -0.2, -0.5], &[0.4, 0.9, 0.6], 100..1_100, None);
        let mesh = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) => Some(mesh), _ => None,
        }).expect("an overview must fill a connected envelope instead of isolated bars");
        assert!(mesh.indices.len() >= 12);
        assert_eq!(mesh.vertices.first().unwrap().pos.x, 18.0);
        assert_eq!(mesh.vertices.last().unwrap().pos.x, 242.0);
        let paths: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Path(path) if !path.closed => Some(path), _ => None,
        }).collect();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|path| path.points.first().unwrap().x == 18.0
            && path.points.last().unwrap().x == 242.0));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn rendered_waveform_samples_form_a_path_at_original_frame_positions() {
        let samples = [-0.25, 0.5, -0.75, 1.0];
        let output = rendered_waveform(&samples, &samples, 1_000_000..1_000_004, None);
        let paths: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Path(path) if !path.closed => Some(path), _ => None,
        }).collect();
        assert_eq!(paths.len(), 1, "individual sample dots must become one connected trace");
        assert_eq!(paths[0].points.iter().map(|point| point.x).collect::<Vec<_>>(),
            [18.0, 74.0, 130.0, 186.0]);
        assert!(paths[0].points[0].y > paths[0].points[1].y);
        assert!(paths[0].points[2].y > paths[0].points[3].y);
        assert!(!output.shapes.iter().any(|shape| matches!(shape.shape, egui::Shape::Circle(_))));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn rendered_waveform_selection_recolors_the_same_trace_at_exact_frame_boundaries() {
        let output = rendered_waveform(&[-0.8, -0.2, -0.5], &[0.4, 0.9, 0.6],
            100..1_100, Some(350..850));
        let meshes: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) => Some((shape.clip_rect, mesh)), _ => None,
        }).collect();
        assert_eq!(meshes.len(), 2, "selection should clip a recolored copy of the whole envelope");
        assert_eq!(meshes[0].1.indices, meshes[1].1.indices);
        assert_eq!(meshes[0].1.vertices.iter().map(|vertex| vertex.pos).collect::<Vec<_>>(),
            meshes[1].1.vertices.iter().map(|vertex| vertex.pos).collect::<Vec<_>>());
        assert_ne!(meshes[0].1.vertices[0].color, meshes[1].1.vertices[0].color);
        assert_eq!(meshes[1].0.left(), 74.0);
        assert_eq!(meshes[1].0.right(), 186.0);
        output.drop_without_applying_deltas();
    }

    #[test]
    fn rendered_waveform_never_connects_across_unknown_samples_or_envelope_bins() {
        for (minimum, maximum, frames) in [
            (vec![0.0, 0.5, f32::NAN, -0.5, 0.25], vec![0.0, 0.5, f32::NAN, -0.5, 0.25], 5),
            (vec![-0.4, -0.1, f32::NAN, -0.5, -0.2], vec![0.1, 0.5, f32::NAN, 0.2, 0.4], 100),
        ] {
            let output = rendered_waveform(&minimum, &maximum, 0..frames, None);
            let paths: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Path(path) if !path.closed => Some(path), _ => None,
            }).collect();
            assert!(!paths.is_empty());
            for path in paths {
                assert!(path.points.last().unwrap().x <= 107.6 || path.points.first().unwrap().x >= 152.4,
                    "a finite path must stay on one side of the missing third bin: {:?}", path.points);
            }
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn wheel_zoom_reaches_original_samples_and_keeps_the_pointer_anchor() {
        let full = 100..48_100;
        let rect = Rect::from_min_size(Pos2::new(18.0, 28.0), Vec2::new(224.0, 84.0));
        let mut current = full.clone();
        for _ in 0..20 {
            current = wheel_zoom_view(&full, &current, 100.0, rect.center().x, rect, rect);
        }
        assert_eq!(current.end - current.start, 56,
            "the view should reach four logical pixels per real sample, rather than a 100ms floor");
        assert_eq!((current.start + current.end) / 2, (full.start + full.end) / 2);
        let policy = waveform::DisplayPolicy::for_view(rect.width(), rect.height(), 56, 48_000, false);
        assert_eq!(policy.bins, 56, "idle must keep the true sample trace at this zoom");
    }

    #[test]
    fn wheel_in_after_a_resize_never_expands_an_existing_finer_view() {
        let full = 100..48_100;
        let current = 20_000..20_008;
        let wider = Rect::from_min_size(Pos2::ZERO, Vec2::new(1_600.0, 200.0));
        let next = wheel_zoom_view(&full, &current, 100.0, wider.center().x, wider, wider);
        assert_eq!(next, current);
        let out = wheel_zoom_view(&full, &current, -100.0, wider.center().x, wider, wider);
        assert_eq!(out.end - out.start, 16);
        assert_eq!(out.start + 8, current.start + 4);
    }

    #[test]
    fn saving_unselected_audio_dispatches_to_the_capture_worker() {
        let session = Arc::new(Session::new(crate::settings::SettingsStore::default()));
        let editor = EguiEditorState::from_size(LogicalSize::new(640.0, 212.0), 1.0);
        let settings = session.settings();
        let mut ui = CaptureUi::new(session.clone(), editor);
        ui.seconds = 1.0;
        ui.save_snapshot(&settings);
        assert!(ui.message.starts_with("Saving snapshot"));
        assert!(ui.snapshot_writer.is_none(), "the UI must not own the unselected save job");
        assert!(session.take_snapshot_result().is_none());
    }

    #[test]
    fn an_edit_preview_applies_frames_geometry_and_peaks_only_as_a_matching_group() {
        let session = Arc::new(Session::new(crate::settings::SettingsStore::default()));
        let editor = EguiEditorState::from_size(LogicalSize::new(640.0, 212.0), 1.0);
        let mut ui = CaptureUi::new(session, editor);
        ui.timeline = WaveformTimeline { epoch: 7, frames: 0..100, sample_rate: 100, channels: 1,
            ..WaveformTimeline::default() };
        ui.peak_frames = 0..100;
        ui.seconds = 1.0;
        ui.peaks[0].minimum = vec![0.25; 4];
        ui.peaks[0].maximum = vec![0.25; 4];
        ui.view = Some(25..75);
        ui.selection = Some(0..20);
        ui.gesture = Some(Gesture::ResizeSelection { fixed: 20, current: 0 });
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 80.0));
        let old_geometry = ui.audio_rect(rect, 10.0);
        let editing = EditingSnapshot {
            token: 1,
            timeline: Arc::new(WaveformTimeline { epoch: 7, frames: 5..105, sample_rate: 100,
                channels: 1, ..WaveformTimeline::default() }),
            snapshots: vec![FrozenTrack::from_interleaved(100, 1, 5, &[0.5; 100]).unwrap()].into(),
        };
        let key = PreviewKey { epoch: 7, editing_token: 1, range: 25..75,
            layout: WaveformLayout::Merged, bins: 2 };
        assert!(!ui.apply_edit_preview(&editing, &key, None));
        assert_eq!(ui.peak_frames, 0..100);
        assert_eq!(ui.audio_rect(rect, 10.0), old_geometry);
        assert_eq!(ui.peaks[0].maximum, [0.25; 4]);
        let mut stale = key.clone();
        stale.editing_token = 0;
        assert!(!ui.apply_edit_preview(&editing, &key, Some(PreviewResult {
            key: stale, peaks: std::array::from_fn(|_| WaveformPeaks::default()),
        })));
        assert_eq!(ui.peak_frames, 0..100);
        ui.clamp_edit_selection(&editing.timeline.frames);
        assert_eq!(ui.selection, Some(5..20));
        assert!(matches!(ui.gesture, Some(Gesture::ResizeSelection { fixed: 20, current: 5 })));
        assert!(ui.apply_edit_preview(&editing, &key, Some(PreviewResult {
            key: key.clone(), peaks: std::array::from_fn(|_| WaveformPeaks {
                minimum: vec![0.5; 2], maximum: vec![0.5; 2],
            }),
        })));
        assert_eq!(ui.peak_frames, 25..75);
        assert_eq!(ui.peaks[0].maximum, [0.5; 2]);
        assert!(ui.audio_rect(rect, 10.0).width() > old_geometry.width());
    }

    #[test]
    fn options_resize_restores_each_editor_mode_even_when_host_resize_is_delayed() {
        let normal = Vec2::new(640.0, 212.0);
        let waveform = Vec2::new(640.0, 180.0);
        let mut sizing = EditorSizing::default();
        assert_eq!(sizing.requested_size(normal, true, false, 180.0, 32.0), Some(OPTIONS_EDITOR_SIZE));
        assert_eq!(sizing.requested_size(OPTIONS_EDITOR_SIZE, true, false, 448.0, 32.0), None);
        assert_eq!(sizing.requested_size(OPTIONS_EDITOR_SIZE, false, false, 448.0, 32.0), Some(normal));

        assert_eq!(sizing.requested_size(normal, false, true, 212.0, 32.0), Some(waveform));
        assert_eq!(sizing.requested_size(waveform, true, true, 180.0, 32.0), Some(OPTIONS_EDITOR_SIZE));
        assert_eq!(sizing.requested_size(OPTIONS_EDITOR_SIZE, false, true, 480.0, 32.0), Some(waveform));
        assert_eq!(sizing.full_height, Some(normal.y));

        assert_eq!(sizing.requested_size(waveform, true, true, 180.0, 32.0), Some(OPTIONS_EDITOR_SIZE));
        assert_eq!(sizing.requested_size(OPTIONS_EDITOR_SIZE, false, false, 448.0, 32.0), Some(normal));
        assert_eq!(sizing.requested_size(normal, true, false, 180.0, 32.0), Some(OPTIONS_EDITOR_SIZE));
        assert_eq!(sizing.requested_size(OPTIONS_EDITOR_SIZE, false, true, 480.0, 32.0), Some(waveform));
        // A paint can arrive before the host adopts the close-time resize request.
        assert_eq!(sizing.requested_size(OPTIONS_EDITOR_SIZE, false, true, 480.0, 32.0), None);
        assert_eq!(sizing.full_height, Some(normal.y));
        assert_eq!(sizing.requested_size(waveform, false, false, 148.0, 32.0), Some(normal));
    }

    #[test]
    fn options_typography_is_readable_without_enlarging_the_waveform_toolbar() {
        let context = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(waveform::HINT_BOLD_FAMILY.into(),
            Arc::new(egui::FontData::from_static(waveform::HINT_BOLD_FONT)));
        fonts.families.insert(egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into()),
            vec![waveform::HINT_BOLD_FAMILY.into()]);
        context.set_fonts(fonts);
        let mut tab = OptionsTab::Capture;
        let mut output = context.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(680.0, 600.0))),
            ..Default::default()
        }, |ui| {
            let toolbar_body = ui.style().text_styles[&egui::TextStyle::Body].size;
            options_layout(ui, &mut tab, |ui, _| {
                assert!(ui.style().text_styles[&egui::TextStyle::Body].size >= 15.0);
                option_row(ui, "Readable field", |ui| { ui.label("Readable body"); });
            });
            assert_eq!(ui.style().text_styles[&egui::TextStyle::Body].size, toolbar_body);
        });
        output.textures_delta.clear();
        let font_size = |text: &str| output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Text(shape) if shape.galley.job.text == text =>
                shape.galley.job.sections.first().map(|section| section.format.font_id.size),
            _ => None,
        }).max_by(f32::total_cmp).unwrap();
        assert!(font_size("Capture") >= 20.0);
        assert!(font_size("Readable field") > font_size("Readable body"));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(shape) if shape.galley.job.text == "Readable field"
                && shape.galley.job.sections[0].format.font_id.family
                    == egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into()))));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn options_combo_shows_ten_rows_then_scrolls_and_short_lists_fit() {
        for count in [3, 10, 13] {
            let context = egui::Context::default();
            let mut selected = 0;
            let mut render = |events| {
                let mut output = context.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 1000.0))),
                events,
                ..Default::default()
            }, |ui| {
                let mut tab = OptionsTab::Capture;
                options_layout(ui, &mut tab, |ui, _| {
                    option_combo(ui, "Menu under test", &mut selected,
                        (0..count).map(|index| (index, format!("Entry {index:02}"))));
                });
                });
                output.textures_delta.clear();
                output
            };
            let first = render(vec![]);
            let pointer = first.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Entry 00" =>
                    Some(text.visual_bounding_rect().center()),
                _ => None,
            }).unwrap();
            first.drop_without_applying_deltas();
            render(vec![egui::Event::PointerMoved(pointer), egui::Event::PointerButton {
                pos: pointer, button: PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE,
            }]).drop_without_applying_deltas();
            render(vec![egui::Event::PointerButton {
                pos: pointer, button: PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE,
            }]).drop_without_applying_deltas();
            let output = render(vec![]);
            let visible_entries = |output: &egui::FullOutput| {
                output.shapes.iter().filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text)
                        if text.galley.job.text.starts_with("Entry ")
                            && shape.clip_rect.contains_rect(text.visual_bounding_rect()) =>
                        Some(text.galley.job.text.clone()),
                    _ => None,
                }).collect::<std::collections::BTreeSet<_>>()
            };
            let entries = visible_entries(&output);
            assert_eq!(entries.len(), count.min(10), "Visible rows for {count} choices: {entries:?}");
            if count > 10 {
                let pointer = output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == "Entry 01" =>
                        Some(text.visual_bounding_rect().center()),
                    _ => None,
                }).unwrap();
                output.drop_without_applying_deltas();
                render(vec![egui::Event::PointerMoved(pointer), egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -400.0),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: egui::Modifiers::NONE,
                }]).drop_without_applying_deltas();
                let scrolled = render(vec![]);
                assert!(visible_entries(&scrolled).contains("Entry 12"));
                scrolled.drop_without_applying_deltas();
            } else {
                output.drop_without_applying_deltas();
            }
        }
    }

    #[test]
    fn options_forms_fit_narrow_editors_and_scroll_down() {
        for width in [320.0, 480.0, 640.0] {
            for mut tab in [OptionsTab::Capture, OptionsTab::Sources, OptionsTab::Appearance, OptionsTab::Export] {
                for host_name in [Some("An unusually long host track name repeated across the source connection row"), None] {
                    let context = egui::Context::default();
                    let mut settings = SessionSettings {
                        sender_enabled: true,
                        sender_name: "A long sender name that must remain inside its text field".into(),
                        capture_input: CaptureInput::Both,
                        show_seconds_lines: true,
                        border_enabled: true,
                        border_color: Some(DEFAULT_ACCENT),
                        output_dir: PathBuf::from("A very long export directory name ".repeat(8)),
                        saved_dir: PathBuf::from("A very long saved snapshot directory ".repeat(8)),
                        ..SessionSettings::default()
                    };
                    for _ in 0..3 {
                        context.run_ui(egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 160.0))),
                            ..Default::default()
                        }, |ui| {
                            let available = ui.max_rect();
                            let output = options_layout(ui, &mut tab, |ui, tab| match tab {
                                OptionsTab::Capture => capture_options(ui, &mut settings),
                                OptionsTab::Sources => source_options(ui, &mut settings, true, host_name),
                                OptionsTab::Appearance => appearance_options(ui, &mut settings),
                                OptionsTab::Export => { export_options(ui, &mut settings); }
                                OptionsTab::Actions => unreachable!(),
                            });
                            assert!(output.content_size.x <= output.inner_rect.width() + 1.0,
                                "{} overflow at {width}: content {:?}, viewport {:?}",
                                tab.label(), output.content_size, output.inner_rect);
                            assert!(ui.min_rect().right() <= available.right() + 1.0);
                            assert!(ui.min_rect().bottom() <= available.bottom() + 1.0);
                            if matches!(tab, OptionsTab::Appearance | OptionsTab::Export) {
                                assert!(output.content_size.y > output.inner_rect.height(),
                                    "{} must extend down into the scroll area", tab.label());
                            }
                        }).drop_without_applying_deltas();
                    }
                }
            }
        }
    }

    #[test]
    fn only_accepted_drag_files_are_retained() {
        let output = std::env::temp_dir().join(format!(
            "backtracking-plugin-drag-test-{}",
            std::process::id()
        ));
        let main = FrozenTrack::from_interleaved(48_000, 1, 0, &[0.25; 32]).unwrap();
        let sidechain = FrozenTrack::from_interleaved(48_000, 1, 0, &[-0.25; 32]).unwrap();
        let snapshots: Arc<[FrozenTrack]> = vec![main, sidechain].into();
        let prepare_file = || {
            WavClip {
                profile: WavExportProfile::default(),
                force_mono: false,
                output: output.clone(),
                snapshots: snapshots.clone(),
                range: 0..32,
                marker: MarkerHistory::default().issue(0..32),
                tempo: None,
            }
            .write()
            .unwrap()
        };
        let canceled = prepare_file();
        let accepted = prepare_file();
        drop(PreparedDrag {
            wav: canceled.clone(),
            accepted: false,
        });
        drop(PreparedDrag {
            wav: accepted.clone(),
            accepted: true,
        });
        assert_eq!(canceled.paths.len(), 2);
        assert_eq!(accepted.paths.len(), 2);
        assert!(canceled.paths.iter().all(|path| !path.exists()));
        assert!(accepted.paths.iter().all(|path| path.exists()));
        std::fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn saved_snapshots_finish_without_editor_frames() {
        let output = std::env::temp_dir().join(format!(
            "backtracking-plugin-snapshot-test-{}",
            std::process::id()
        ));
        let writer = SnapshotWriter::start().unwrap();
        let mut markers = MarkerHistory::default();
        let snapshots: Arc<[FrozenTrack]> = vec![
            FrozenTrack::from_interleaved(48_000, 2, 0, &[0.75, -0.25].repeat(32)).unwrap(),
        ].into();
        for (range, force_mono) in [(0..16, true), (0..32, false)] {
            writer
                .requests
                .send(WavClip {
                    profile: WavExportProfile::default(),
                    force_mono,
                    output: output.clone(),
                    snapshots: snapshots.clone(),
                    marker: markers.issue(range.start as u64..range.end as u64),
                    range,
                    tempo: Some(120.0),
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
        assert_eq!(first.marker.range, 0..16);
        assert_eq!(second.marker.range, 0..32);
        assert_ne!(first.paths, second.paths);
        for (saved, channels, expected) in [
            (first, 1, vec![0.25; 16]),
            (second, 2, [0.75, -0.25].repeat(32)),
        ] {
            // The clip name a DAW shows carries the host tempo the clip was taken at.
            let name = saved.paths[0].file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with("Backtracking_") && name.contains("s_120bpm"), "{name}");
            let wav = std::fs::read(&saved.paths[0]).unwrap();
            assert_eq!(
                u16::from_le_bytes(wav[22..24].try_into().unwrap()),
                channels
            );
            let data = wav.windows(4).position(|chunk| chunk == b"data").unwrap() + 8;
            let samples: Vec<_> = wav[data..]
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
                .collect();
            assert_eq!(samples, expected);
        }
        std::fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn accepted_markers_keep_the_most_recent_completed_range() {
        let mut history = MarkerHistory::default();
        history.reset(4);
        let slow_save = history.issue(300..700);
        let completed_selection = history.issue(500..600);
        history.accept(completed_selection);
        history.accept(slow_save);
        assert_eq!(history.range(), Some(&(500..600)));

        let old_epoch = history.issue(550..580);
        history.reset(5);
        history.accept(old_epoch);
        assert_eq!(history.range(), None);
        let new_epoch = history.issue(0..16);
        history.accept(new_epoch);
        assert_eq!(history.range(), Some(&(0..16)));
    }

    #[test]
    fn snapped_frame_boundaries_export_exact_snapshot_samples() {
        let start = u64::from(u32::MAX) + 123;
        let samples: Vec<f32> = (0..128).map(|sample| sample as f32).collect();
        let snapshot = FrozenTrack::from_interleaved(48_000, 1, start, &samples).unwrap();
        let marked = start + 25..start + 109;
        let rect = Rect::from_min_size(Pos2::new(10.0, 0.0), Vec2::new(256.0, 80.0));
        let visible = snapshot.frame_range();
        assert_eq!(
            frame_at_pointer(rect, 63.0, visible.clone(), None),
            start + 27
        );
        let from = frame_at_pointer(rect, 63.0, visible.clone(), Some(&marked));
        let to = frame_at_pointer(rect, 224.0, visible, Some(&marked));
        let frames = export_frame_range(&snapshot, &(from..to)).unwrap();
        assert_eq!(frames, 25..109);
        assert_eq!(snapshot.snapshot().frames(frames).unwrap(), &samples[25..109]);
        assert!(export_frame_range(&snapshot, &(start - 1..start + 20)).is_none());
        let after_scroll = start + 32..start + 160;
        assert_eq!(
            frame_at_pointer(rect, 14.0, after_scroll, Some(&marked)),
            start + 34
        );
    }

    #[test]
    fn snapping_prioritizes_flags_over_a_closer_visible_bar() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 80.0));
        let visible = 400..600;
        let flags = 420..570;
        assert_eq!(
            frame_at_pointer_with_bars(rect, 24.0, visible.clone(), Some(&flags), [425, 570]),
            420,
        );
        assert_eq!(
            frame_at_pointer_with_bars(rect, 18.0, visible.clone(), Some(&flags), [425, 570]),
            420,
        );
        assert_eq!(
            frame_at_pointer_with_bars(rect, 2.0, visible, None, [399, 425]),
            402,
        );
    }

    #[test]
    fn snapping_uses_seconds_only_when_the_painted_guides_include_them() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 80.0));
        let visible = 400..600;
        let guides = waveform::TimelineGuides { bars: vec![425], seconds: vec![424, 500] };
        assert_eq!(frame_at_pointer_with_guides(rect, 24.0, visible.clone(), &[420], &guides), 420);
        assert_eq!(frame_at_pointer_with_guides(rect, 24.0, visible.clone(), &[], &guides), 425);
        let seconds = waveform::TimelineGuides { bars: vec![], seconds: vec![500] };
        assert_eq!(frame_at_pointer_with_guides(rect, 103.0, visible.clone(), &[], &seconds), 500);
        assert_eq!(frame_at_pointer_with_guides(rect, 103.0, visible, &[], &waveform::TimelineGuides::default()), 503);
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

        let image = NSImage::initWithData(NSImage::alloc(), &NSData::from_vec(DRAG_ICON.to_vec()))
            .ok_or("could not load the drag preview")?;
        let preview_center = view.convertPoint_fromView(location, None);
        let size = image.size();
        let mut items = Vec::with_capacity(drag.file.wav.paths.len());
        for (index, path) in drag.file.wav.paths.iter().enumerate() {
            let native_path = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| "invalid WAV path")?;
            let path_ptr = NonNull::new(native_path.as_ptr().cast_mut()).ok_or("invalid WAV path")?;
            // AppKit reads the NUL-terminated native path before returning the NSURL.
            let url = unsafe {
                NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
                    path_ptr, false, None,
                )
            };
            let item = NSDraggingItem::initWithPasteboardWriter(
                NSDraggingItem::alloc(),
                &ProtocolObject::from_retained(url),
            );
            let offset = index as f64 * 4.0;
            let origin = NSPoint::new(
                preview_center.x - size.width * 0.5 + offset,
                preview_center.y - size.height * 0.5 + offset,
            );
            // The preview contents are an NSImage, as required by AppKit.
            unsafe { item.setDraggingFrame_contents(NSRect::new(origin, size), Some(&*image)) };
            items.push(item);
        }
        let items = NSArray::from_retained_slice(&items);
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

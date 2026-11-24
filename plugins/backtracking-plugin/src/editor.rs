use std::io::{self, Cursor};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    AutoResumeDelay, ChannelRoute, DurationChoice, Fades, FrozenTrack, MeterReadings, Preview,
    WavExportProfile, cleanup,
    spatial::HeadModel,
    waveform,
    waveform::{PairSide, PreviewLane},
};
use backtracking_ui::stack::{self, LaneBox};
use backtracking_ui::{
    CENTER_LINE, FadeEnd, FlagReach, LoneSample, OPTIONS_NAVIGATION_WIDTH, PLOT_BACKGROUND,
    RECORDING_DIM, RECORDING_LIT, SpectrumPalette, TraceStyle, TransportGlyph, WAV_SAMPLE_FORMATS,
    WAV_SAMPLE_RATES, accent_color, configure_options_style, dragged_fades, fade_end_at,
    fade_waveform_bins, fit_frame_range, flag_handle_at, icon_button, meter_number, muted_accent,
    options_choice, options_control_width, options_field, options_group_heading,
    options_heading_text, options_list, options_navigation_item, options_note, options_value,
    paint_colored_waveform_trace, paint_fades, paint_guide_lines, paint_playhead,
    paint_transport_glyph, paint_waveform_trace, selected_waveform_color, status_led, trace_bin,
    waveform_press,
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
    BORDER_WIDTHS, CaptureInput, DEFAULT_ACCENT, MeterMode, PairRender, SessionSettings,
    StatusMark, TRANSPARENCY_LEVELS, WIDE_EXPORT_MODES, WaveformColoring, WaveformLayout,
    wide_export_label,
};
use crate::surround::{WideLayout, WideRow, WideSave};

#[cfg(target_os = "windows")]
mod win_drag;
#[cfg(target_os = "windows")]
mod win_ime;
mod preview;
use preview::{
    PreviewKey, PreviewRequest, PreviewResult, PreviewWorker, RowsKey, RowsRequest, RowsResult,
    RowsWorker,
};
mod spectrum;
#[cfg(test)]
mod spectrum_tests;
use spectrum::{ColorRow, WaveformColors};

const REFRESH: Duration = Duration::from_millis(80);
const RECORDING_BLINK: Duration = Duration::from_millis(500);
const TOOLBAR_CONTROL_HEIGHT: f32 = 25.0;
const TRANSPORT_GLYPH: TransportGlyph = TransportGlyph {
    play: [Vec2::new(-4.0, -6.0), Vec2::new(6.0, 0.0), Vec2::new(-4.0, 6.0)],
    pause_offset: 2.7,
    pause_bar: Vec2::new(2.6, 11.0),
};
const MIN_WAVEFORM_HEIGHT: f32 = 60.0;
/// Room for the Options form with a setting's name beside its control.
const OPTIONS_EDITOR_SIZE: Vec2 = Vec2::new(760.0, 520.0);
const FLAG_SNAP_RADIUS: f32 = 8.0;
const FLAG_REACH: FlagReach = FlagReach { above: 4.0, below: 14.0, radius: FLAG_SNAP_RADIUS };
/// Alt starts a free selection, as Ctrl does, except on a flag of the selection, where it drags
/// that end's fade.
const ALT_SELECTS: bool = true;
/// The selection's flags and the fades drawn from them.
const FLAG_COLOR: Color32 = Color32::from_rgb(255, 199, 103);
/// The line that follows a playing preview across the selection.
const PLAYHEAD_COLOR: Color32 = Color32::from_gray(245);
/// How long a preview may wait for its first block before the editor says why it is silent.
const PREVIEW_WAIT: Duration = Duration::from_millis(600);
/// The selected waveform takes the accent's complement when it differs by at least this much.
const SELECTION_CONTRAST: u8 = 64;
const PRODUCT_ICON: &[u8] = include_bytes!("../../../assets/backtracking-icon.png");
/// The L and R rows of a capture wider than stereo are set apart from its channel rows by a faint
/// backing and a rule, both fainter than the line between two lanes, as on the desktop.
const PAIR_BACKING: Color32 = Color32::from_rgba_premultiplied(7, 7, 7, 7);
const PAIR_DIVIDER: Color32 = Color32::from_rgba_premultiplied(22, 22, 22, 22);
/// The tallest the editor grows by itself for the rows of a capture wider than stereo.
const MAX_GROWN_HEIGHT: f32 = 560.0;
const NO_LOUDNESS: &str = "Loudness is measured for mono and stereo sources only.";
const SCROLL_HINT: &str = "Scroll here or drag the bar on the right to see the other channels.";

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
    /// The selection's fade in and fade out, in frames from its flags.
    fades: Fades,
    /// When Space last started a preview, until the host has played some of it.
    preview_started: Option<Instant>,
    gesture: Option<Gesture>,
    drag_preparation: Option<DragPreparation>,
    snapshot_writer: Option<SnapshotWriter>,
    native_dragging: Arc<AtomicBool>,
    drag_results: Option<Receiver<Result<SavedWav, String>>>,
    output_cleanup: cleanup::Schedule,
    message: String,
    /// The rows on screen of a capture wider than stereo, top to bottom, whose peaks are in
    /// `row_peaks`. Empty for mono and stereo, which draw `peaks`.
    rows: Vec<WideRow>,
    row_peaks: Vec<WaveformPeaks>,
    rows_worker: Option<RowsWorker>,
    rows_requested: Option<RowsKey>,
    /// The first row on screen of a wide capture's lane; the ones above are scrolled out.
    first_row: usize,
    /// Where the scroll thumb was grabbed, while it is dragged.
    scroll_drag: Option<f32>,
    /// This frame's scroll track in the right margin, then the label gutter beside it, while
    /// a wide capture's rows scroll.
    scroll_zones: Vec<Rect>,
    /// Whether the waveform is drawn alone, without its labels and margins' furniture.
    waveform_only: bool,
    /// Made the first time a binaural preview needs it.
    head_model: Option<Arc<HeadModel>>,
    /// The colors read for the rows on screen, while the waveform is colored by frequency.
    waveform_colors: WaveformColors,
    /// Keeps a Chinese, Japanese or Korean input method from swallowing Space and the other keys.
    #[cfg(target_os = "windows")]
    input_method: win_ime::InputMethod,
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
    /// Alt on a flag of the selection: drags that end's fade, starting from `initial`.
    Fade {
        end: FadeEnd,
        press: u64,
        origin_x: f32,
        initial: Fades,
        moved: bool,
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
            Self::Export => "Drag-and-drop folder",
            Self::Saved => "Saved snapshots folder",
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
            Self::Actions => "Actions",
            Self::Capture => "Capture",
            Self::Sources => "Sources",
            Self::Appearance => "Appearance",
            Self::Export => "Export",
        }
    }
}

#[derive(Default)]
struct EditorSizing {
    options_size: Option<Vec2>,
    full_height: Option<f32>,
    waveform_only: bool,
    /// The heights before and after the editor grew for a wide capture's rows.
    grown: Option<(f32, f32)>,
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

    /// What the editor `current` big does for rows that want `wanted` points of a plot that has
    /// `plot`, as the desktop window does: it grows once when a wide capture's rows appear, at
    /// most to `MAX_GROWN_HEIGHT`, and goes back when they go unless it was resized meanwhile.
    /// While `busy` (Options, a gesture or another resize under way) it stays as it is.
    fn fit_rows(
        &mut self,
        current: Vec2,
        wanted: Option<f32>,
        plot: f32,
        busy: bool,
    ) -> Option<Vec2> {
        if busy {
            return None;
        }
        match (wanted, self.grown) {
            (Some(wanted), None) => {
                let target = (current.y + wanted - plot)
                    .min(MAX_GROWN_HEIGHT)
                    .max(current.y);
                self.grown = Some((current.y, target));
                (target > current.y + 0.5).then_some(Vec2::new(current.x, target))
            }
            (None, Some((before, after))) => {
                self.grown = None;
                ((current.y - after).abs() < 1.0 && after > before + 0.5)
                    .then_some(Vec2::new(current.x, before))
            }
            _ => None,
        }
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
    mask: u32,
    profile: WavExportProfile,
    force_mono: bool,
    output: PathBuf,
    snapshots: Arc<[FrozenTrack]>,
    range: Range<usize>,
    marker: MarkerUpdate,
    /// The host tempo when the clip was taken, if the host reports one. Shown in the file name.
    tempo: Option<f64>,
    /// The selection's ramps, counted from the ends of `range`.
    fades: Fades,
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
    /// Writes a clip that has no wide channels to lay out.
    #[cfg(test)]
    fn write(self) -> io::Result<SavedWav> {
        ClipJob::from(self).write()
    }
}

/// A clip and, for a capture wider than stereo, how its channels are written.
struct ClipJob {
    clip: WavClip,
    wide: Option<WideSave>,
}

impl From<WavClip> for ClipJob {
    fn from(clip: WavClip) -> Self {
        Self { clip, wide: None }
    }
}

impl ClipJob {
    fn write(self) -> io::Result<SavedWav> {
        let Self { clip, wide } = self;
        let paths = crate::surround::export_clip(
            &clip.output,
            &clip.snapshots,
            clip.range,
            clip.profile,
            clip.force_mono,
            clip.tempo,
            clip.fades,
            clip.mask,
            wide.as_ref(),
        )?;
        Ok(SavedWav {
            paths: paths
                .into_iter()
                .map(|path| path.canonicalize().unwrap_or(path))
                .collect(),
            marker: clip.marker,
        })
    }
}

/// Takes plain clips as well as wide ones.
struct ClipSender(Sender<ClipJob>);

impl ClipSender {
    /// Fails only once the writer has stopped.
    fn send(&self, job: impl Into<ClipJob>) -> io::Result<()> {
        self.0
            .send(job.into())
            .map_err(|_| io::Error::other("snapshot writer stopped unexpectedly"))
    }
}

struct SnapshotWriter {
    requests: ClipSender,
    results: Receiver<io::Result<SavedWav>>,
    worker: JoinHandle<()>,
}

impl SnapshotWriter {
    fn start() -> io::Result<Self> {
        let (requests, incoming) = mpsc::channel::<ClipJob>();
        let (completed, results) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("backtracking-snapshot".into())
            .spawn(move || {
                for clip in incoming {
                    let _ = completed.send(clip.write());
                }
            })?;
        Ok(Self {
            requests: ClipSender(requests),
            results,
            worker,
        })
    }

    fn finish(self) {
        drop(self.requests);
        let _ = self.worker.join();
    }
}

/// The editor's typefaces: egui's own, the bundled bold face for headings and, on Windows,
/// the system fonts for text in any other script.
fn editor_fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let name = waveform::HINT_BOLD_FAMILY;
    fonts.font_data.insert(
        name.to_owned(),
        Arc::new(egui::FontData::from_static(waveform::HINT_BOLD_FONT)),
    );
    fonts
        .families
        .insert(egui::FontFamily::Name(name.into()), vec![name.to_owned()]);
    // Folder paths and track names can be in any language.
    #[cfg(target_os = "windows")]
    backtracking_ui::add_system_font_fallbacks(&mut fonts);
    fonts
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
        context.set_fonts(editor_fonts());
        // Deleting is permanent, so a pass waits a while after the editor opens.
        self.output_cleanup.restart(Instant::now());
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
                || previous.force_mono != settings.force_mono
                || previous.pair_render != settings.pair_render
                || previous.wide_export != settings.wide_export;
            let folders_changed = output_changed || previous.saved_dir != settings.saved_dir;
            if retention_changed {
                self.next_refresh = Instant::now();
            }
            if retention_changed || output_changed || format_changed {
                self.discard_drag_preparation();
            }
            if folders_changed {
                self.output_cleanup.restart(Instant::now());
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
        self.clean_output_folder(&settings);
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
        // Space with a selection plays it from its start flag to its end flag, or stops it.
        // Without one the key goes on to the host.
        let toggled_preview = !self.options_open
            && self.selection.is_some()
            && self.gesture.is_none()
            && !self.native_dragging.load(Ordering::Acquire)
            && ui.input_mut(take_space_press);
        if toggled_preview {
            self.toggle_preview(&settings);
        }
        self.explain_waiting_preview();
        self.waveform_only = settings.show_waveform_only;
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
                    if settings.show_title {
                        if let Some(icon) = &self.product_icon {
                            ui.image((icon.id(), Vec2::splat(TOOLBAR_CONTROL_HEIGHT)));
                        }
                        ui.heading("Backtracking");
                        ui.separator();
                    }
                    if capture_button(ui, paused, controls_enabled).clicked() {
                        self.toggle_capture(ui, paused);
                        paused = self.session.is_paused();
                    }
                    if self.timeline.channels > 2 {
                        // The plug-in's window is in English, like its other labels.
                        ui.label(format!("{} ch", self.timeline.channels));
                    }
                    self.recording_indicator(ui, paused, settings.status_mark);
                    if clear_button(ui, controls_enabled && self.seconds > 0.0).clicked() {
                        self.clear_capture();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let gear = settings_button(ui);
                        if settings.show_meter {
                            self.meter_readout(ui, settings.meter_mode);
                        }
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
        self.refresh_waveform_colors(settings.waveform_coloring);
        let configured_seconds = settings.retention.seconds() as f32;
        let row_tip = self.interact_lanes(ui, rect, !settings_open);
        if !canceled_selection {
            self.interact_waveform(ui, frame, &response, &settings, !settings_open);
        }
        if canceled_selection {
            response.surrender_focus();
        }
        self.draw_waveform(ui, rect, configured_seconds, !settings_open, &settings);
        let response = match row_tip {
            Some(text) => response.on_hover_text(text),
            None => response,
        };
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
        // Only a text field being edited takes keys through the input method; Space and the
        // other shortcuts come straight to the editor.
        #[cfg(target_os = "windows")]
        self.input_method.follow(
            frame.baseview_window(),
            ui.ctx().output(|output| output.ime.is_some()),
        );
        ui.ctx().request_repaint_after(
            // The playhead moves every frame.
            if self.gesture.is_some() || self.drag_preparation.is_some() || self.playhead().is_some() {
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
        // The next editor window starts with the input method Windows gave it.
        #[cfg(target_os = "windows")]
        {
            self.input_method = win_ime::InputMethod::default();
        }
        // Nothing would show the preview or stop it.
        self.stop_preview();
        self.discard_drag_preparation();
        // Nothing would draw the colors; the next window reads them again.
        self.waveform_colors.stop();
        if !self.native_dragging.load(Ordering::Acquire) {
            self.session.end_waveform_gesture(self.selection.is_some());
        }
    }
}

impl Drop for CaptureUi {
    fn drop(&mut self) {
        self.stop_preview();
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
        let window = editor_state.clone();
        Self {
            waveform_colors: WaveformColors::new(Arc::new(move || window.is_open())),
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
            fades: Fades::NONE,
            preview_started: None,
            gesture: None,
            drag_preparation: None,
            snapshot_writer: None,
            native_dragging: Arc::new(AtomicBool::new(false)),
            drag_results: None,
            // The project's choice arrives later, with the rest of the restored settings.
            output_cleanup: cleanup::Schedule::new(None, Instant::now()),
            message: String::new(),
            rows: Vec::new(),
            row_peaks: Vec::new(),
            rows_worker: None,
            rows_requested: None,
            first_row: 0,
            scroll_drag: None,
            scroll_zones: Vec::new(),
            waveform_only: false,
            head_model: None,
            #[cfg(target_os = "windows")]
            input_method: win_ime::InputMethod::default(),
        }
    }

    fn meter_readout(&self, ui: &mut egui::Ui, mode: MeterMode) {
        let value = mode.reading(self.session.meter_readings());
        let text = format!("{} {}", meter_number(Some(value), false), mode.label());
        let response = ui.add_sized(
            [90.0, TOOLBAR_CONTROL_HEIGHT],
            egui::Label::new(egui::RichText::new(text).monospace()).truncate(),
        );
        // A capture wider than stereo has a peak and a VU reading but no loudness, as on the
        // desktop, so LUFS reads "—".
        if mode == MeterMode::Lufs && self.timeline.wide.is_some() {
            response.on_hover_text(NO_LOUDNESS);
        }
    }

    fn recording_indicator(&mut self, ui: &mut egui::Ui, paused: bool, mark: StatusMark) {
        let size = Vec2::splat(16.0);
        if !mark.shown(paused) {
            self.recording_since = None;
            if mark != StatusMark::Never {
                // The light keeps its place, so the buttons do not move when it comes on.
                ui.allocate_space(size);
            }
            return;
        }
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
        status_led(ui, size, 4.0, color, status);
    }

    /// Deletes the plug-in's own old WAV files from the drag-and-drop folder when the
    /// project asks for it. Only an open editor does this.
    fn clean_output_folder(&mut self, settings: &SessionSettings) {
        let now = Instant::now();
        if self.output_cleanup.age() != settings.output_cleanup {
            self.output_cleanup.set_age(settings.output_cleanup, now);
        }
        if self.output_cleanup.age().is_none() {
            return;
        }
        // The folder check reads the disk, so it waits until a pass is due. Saved snapshots
        // are never deleted, so a folder that also holds them is left alone.
        let folder = (self.output_cleanup.due(now)
            && !cleanup::same_folder(&settings.output_dir, &settings.saved_dir))
        .then(|| settings.output_dir.clone());
        // A file being offered for a drag was written a moment ago, so no age reaches it.
        match self.output_cleanup.poll(folder.as_deref(), &[], now) {
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
        let history_note = self.short_history_note(settings.retention);
        let wide = self.timeline.wide.is_some();
        egui::Window::new("Options")
            .id(egui::Id::unique("backtracking-options"))
            .open(&mut open)
            .fixed_rect(Rect::from_center_size(bounds.center(), window_size))
            .constrain_to(bounds)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let accent = accent_color(edited.accent);
                options_layout(ui, &mut selected_tab, accent, |ui, tab| {
                    match tab {
                        OptionsTab::Actions => self.capture_actions(ui, settings),
                        OptionsTab::Capture => {
                            capture_page(ui, &mut edited, history_note.as_deref());
                        }
                        OptionsTab::Sources => source_options(
                            ui,
                            &mut edited,
                            self.session.sidechain_available(),
                            self.session.host_track_name().as_deref(),
                        ),
                        OptionsTab::Appearance => appearance_page(ui, &mut edited, wide),
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
        options_group_heading(ui, "Capture controls");
        let (toggle, cancel, clear) = ui.horizontal_wrapped(|ui| {
            (
                ui.add_enabled(can_toggle, egui::Button::new(if paused { "Play" } else { "Pause" }))
                    .clicked(),
                ui.add_enabled(self.can_cancel_selection(), egui::Button::new("Cancel selection"))
                    .clicked(),
                ui.add_enabled(can_toggle && self.seconds > 0.0, egui::Button::new("Clear capture"))
                    .clicked(),
            )
        }).inner;
        if toggle { self.toggle_capture(ui, paused); }
        if cancel { self.cancel_selection(); }
        if clear { self.clear_capture(); }
        ui.separator();
        options_group_heading(ui, "Snapshots");
        if ui.add_enabled(can_toggle, egui::Button::new("Save snapshot")).clicked() {
            self.save_snapshot(settings);
        }
    }

    fn apply_options(&mut self, settings: &mut SessionSettings, edited: SessionSettings) {
        if edited == *settings {
            return;
        }
        self.session.update_editor_settings(edited.clone());
        if edited.wav_profile != settings.wav_profile
            || edited.force_mono != settings.force_mono
            || edited.pair_render != settings.pair_render
            || edited.wide_export != settings.wide_export
        {
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
        let current = match &self.timeline.wide {
            // A wide capture's rows are current while the same rows are on screen.
            Some(wide) => {
                self.row_peaks
                    .first()
                    .is_none_or(|peaks| peaks.minimum.len() == bins)
                    && self.rows == self.rows_for(wide, self.timeline.b_channels.is_some())
            }
            None => self.peaks[0].minimum.len() == bins,
        };
        if self.peak_layout == layout && now < self.next_refresh && current {
            return;
        }
        if self.session.editing_pending() {
            // Pin the view under the pointer without interrupting the producer.
            self.next_refresh = now + Duration::from_millis(16);
            return;
        }
        if let Some(editing) = self.session.editing_snapshot() {
            let range = clamped_view(self.view.clone(), &editing.timeline.frames);
            if let Some(wide) = editing.timeline.wide.clone() {
                self.refresh_edit_rows(&editing, wide, range, bins);
                self.next_refresh = now + Duration::from_millis(16);
                return;
            }
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
                    worker.request(PreviewRequest {
                        wide: editing.timeline.wide.clone(),
                        key: key.clone(),
                        snapshots: editing.snapshots.clone(),
                    });
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
        self.rows_requested = None;
        self.editing_seconds = None;
        if self.selection.is_some() && self.gesture.is_none() {
            self.reset_selection_preview();
        }
        let requested = self.view.clone();
        if let Some(wide) = self.timeline.wide.clone() {
            let rows = self.rows_for(&wide, self.timeline.b_channels.is_some());
            let mut peaks = self.row_peaks.clone();
            let Some(timeline) =
                self.session
                    .try_wide_rows(&rows, requested.clone(), bins, &mut peaks)
            else {
                self.next_refresh = now + Duration::from_millis(16);
                return;
            };
            // A capture that is no longer wide is drawn the usual way below.
            if timeline.wide.is_some() {
                self.adopt_live_timeline(timeline, requested);
                self.rows = rows;
                self.row_peaks = peaks;
                self.peak_layout = layout;
                self.next_refresh = now + REFRESH;
                return;
            }
        }
        self.rows.clear();
        self.row_peaks.clear();
        let mut peaks = self.peaks.clone();
        let Some(timeline) = self.session.try_waveform_in_range_with_bins(
            layout, requested.clone(), bins, &mut peaks,
        ) else {
            self.next_refresh = now + Duration::from_millis(16);
            return;
        };
        self.adopt_live_timeline(timeline, requested);
        self.peak_layout = layout;
        self.peaks = peaks;
        self.next_refresh = now + REFRESH;
    }

    /// Takes the live capture's `timeline` and the view `requested` of it.
    fn adopt_live_timeline(&mut self, timeline: WaveformTimeline, requested: Option<Range<u64>>) {
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
                self.fades = self.fades.fitted(clipped.end - clipped.start);
                *range = clipped;
                self.stop_preview();
                self.publish_selection();
            }
        }
        self.timeline = timeline;
    }

    /// The row `row` of a mono or stereo capture is drawn from, top to bottom.
    fn color_row(&self, row: usize) -> ColorRow {
        if self.timeline.b_channels.is_some() {
            return ColorRow::View(row, waveform::WaveformView::Merged);
        }
        let views = waveform_views(self.peak_layout, self.timeline.channels);
        ColorRow::View(0, views.get(row).copied().unwrap_or_default())
    }

    /// The rows drawn now and the bins each is drawn in.
    fn color_rows(&self) -> (Vec<ColorRow>, usize) {
        if self.timeline.wide.is_some() {
            let rows = self.rows.iter().map(|&row| wide_color_row(row)).collect();
            return (rows, self.row_peaks.first().map_or(0, |peaks| peaks.minimum.len()));
        }
        let count = if self.timeline.b_channels.is_some() {
            2
        } else {
            waveform_views(self.peak_layout, self.timeline.channels).len()
        };
        ((0..count).map(|row| self.color_row(row)).collect(), self.peaks[0].minimum.len())
    }

    /// Asks for the colors of the rows drawn now, once per pass, from the pinned edit while a
    /// selection is edited and from the live capture otherwise. With a single color this only
    /// lets go of what coloring by frequency had.
    fn refresh_waveform_colors(&mut self, coloring: WaveformColoring) {
        if coloring != WaveformColoring::Frequency {
            self.waveform_colors.update(coloring, None, |_| None);
            return;
        }
        let editing = self.session.editing_snapshot();
        let (rows, bins) = self.color_rows();
        let wanted = (bins > 0 && !rows.is_empty() && !self.peak_frames.is_empty()).then(|| {
            spectrum::Request {
                epoch: self.timeline.epoch,
                edit: editing.as_ref().map(|editing| editing.token),
                frames: self.peak_frames.clone(),
                bins,
                rows,
                group: self.timeline.wide.as_ref().map(|wide| wide.group.clone()),
            }
        });
        let session = &self.session;
        self.waveform_colors.update(coloring, wanted, |request| match &editing {
            Some(editing) => {
                (editing.timeline.epoch == request.epoch).then(|| editing.snapshots.clone())
            }
            None => session
                .try_freeze_frames(request.frames.clone())
                .filter(|(epoch, _)| *epoch == request.epoch)
                .map(|(_, tracks)| tracks),
        });
    }

    /// Asks for the rows on screen of a pinned wide capture and shows them once they are read.
    fn refresh_edit_rows(
        &mut self,
        editing: &EditingSnapshot,
        wide: WideLayout,
        range: Range<u64>,
        bins: usize,
    ) {
        let key = RowsKey {
            epoch: editing.timeline.epoch,
            editing_token: editing.token,
            range,
            rows: self.rows_for(&wide, editing.timeline.b_channels.is_some()),
            bins,
        };
        self.sync_timeline(editing.timeline.epoch);
        self.clamp_edit_selection(&editing.timeline.frames);
        if self.rows_requested.as_ref() != Some(&key) {
            if self.rows_worker.is_none() {
                self.rows_worker = RowsWorker::start().ok();
            }
            if let Some(worker) = &self.rows_worker {
                worker.request(RowsRequest {
                    wide,
                    key: key.clone(),
                    snapshots: editing.snapshots.clone(),
                });
                self.rows_requested = Some(key.clone());
            } else {
                self.message = "Waveform preview worker unavailable.".into();
            }
        }
        let result = self.rows_worker.as_ref().and_then(RowsWorker::try_take);
        self.apply_edit_rows(editing, &key, result);
    }

    /// Shows the rows `key` asked for, when `result` is for them.
    fn apply_edit_rows(
        &mut self,
        editing: &EditingSnapshot,
        key: &RowsKey,
        result: Option<RowsResult>,
    ) -> bool {
        let Some(result) = result.filter(|result| &result.key == key) else {
            return false;
        };
        // Geometry and data change together, as for a pinned mono or stereo view.
        self.timeline = editing.timeline.as_ref().clone();
        self.seconds = self.timeline.duration();
        self.peak_frames = result.key.range;
        self.displayed_zoomed = self.peak_frames != self.timeline.frames;
        self.rows = result.key.rows;
        self.row_peaks = result.peaks;
        true
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
                self.fades = clipped.as_ref()
                    .map_or(Fades::NONE, |range| self.fades.fitted(range.end - range.start));
                self.selection = clipped;
                self.stop_preview();
                self.publish_selection();
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
        self.rows.clear();
        self.row_peaks.clear();
        self.first_row = 0;
        self.scroll_drag = None;
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
        self.fades = Fades::NONE;
        // The preview plays the selection, so it ends with it.
        self.stop_preview();
        self.preview_requested = None;
        self.rows_requested = None;
        self.editing_seconds = None;
        self.session.set_snapshot_selection(None);
        self.next_refresh = Instant::now();
    }

    /// Tells the session the selection and its fades, which a snapshot of it is written with.
    fn publish_selection(&self) {
        self.session.set_snapshot_edit(self.selection.clone(), self.fades);
    }

    /// Space with a selection: plays it from its start flag to its end flag, or stops it.
    fn toggle_preview(&mut self, settings: &SessionSettings) {
        self.session.mark_interaction();
        if self.preview_playing() {
            self.stop_preview();
            return;
        }
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let Some(editing) = self.session.editing_snapshot() else {
            self.message = "Preparing selected audio…".into();
            return;
        };
        // Keep the Capture page's L/R render ready for a bus with a different layout.
        let binaural = editing.timeline.wide.is_some() && settings.pair_render.binaural();
        let model = if binaural {
            let Some(model) = self.head_model(editing.timeline.sample_rate) else {
                self.message = "The selected audio is no longer available.".into();
                return;
            };
            Some(model)
        } else {
            None
        };
        let lanes: Vec<_> = editing
            .snapshots
            .iter()
            .map(|track| PreviewLane {
                track,
                route: ChannelRoute::Merged,
                selected: None,
                group: editing
                    .timeline
                    .wide
                    .as_ref()
                    .filter(|_| track.channels() > 2)
                    .map(|w| &w.group),
            })
            .collect();
        let preview = match &editing.timeline.wide {
            Some(wide) if !settings.preview_as_lr => Preview::for_bus(
                &lanes,
                selection,
                wide.speaker_mask,
                model,
                settings.force_mono,
                self.fades,
            ),
            _ => Preview::with_channels(&lanes, selection, model, settings.force_mono, self.fades),
        };
        match preview {
            Some(preview) => {
                self.session.preview.play(preview);
                self.preview_started = Some(Instant::now());
            }
            None => self.message = "The selected audio is no longer available.".into(),
        }
    }

    fn stop_preview(&mut self) {
        self.preview_started = None;
        self.session.preview.stop();
    }

    /// Whether a preview is playing and has not been asked to stop.
    fn preview_playing(&self) -> bool {
        self.session.preview
            .with_current(|preview| !preview.is_finished() && !preview.stop_requested())
            .unwrap_or(false)
    }

    /// The frame of the timeline the playing preview has reached. None once it is asked to stop.
    fn playhead(&self) -> Option<u64> {
        self.session.preview
            .with_current(|preview| {
                (!preview.is_finished() && !preview.stop_requested())
                    .then(|| preview.frames().start + preview.rendered())
            })
            .flatten()
    }

    /// The plug-in plays a preview only while the host processes audio, which some hosts stop
    /// doing while their transport is stopped. Says so when a preview has not begun.
    fn explain_waiting_preview(&mut self) {
        let Some(started) = self.preview_started else {
            return;
        };
        let waiting = self.session.preview
            .with_current(|preview| preview.rendered() == 0 && !preview.stop_requested())
            .unwrap_or(false);
        if !waiting {
            self.preview_started = None;
        } else if started.elapsed() >= PREVIEW_WAIT {
            self.preview_started = None;
            self.message = "The preview plays once the host processes audio.".into();
        }
    }

    /// The ramp whose flag a press at `position` picks up, while the selection is on screen.
    fn fade_flag(&self, audio_rect: Rect, position: Pos2) -> Option<FadeEnd> {
        let selection = self.selection.as_ref()?;
        if self.peak_frames.is_empty() || audio_rect.width() <= 1.0 {
            return None;
        }
        fade_end_at(audio_rect, self.peak_frames.clone(), selection, position, FLAG_REACH)
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
            return;
        }
        // The rows of a wide capture ask for room, as on the desktop. Mono and stereo never do.
        let wanted = self.timeline.wide.as_ref().and_then(|wide| {
            stack::comfortable_height(&wide_lanes(wide, self.timeline.b_channels.is_some()))
        });
        // A cleared or restarted capture shows an empty timeline until the session answers. Its
        // rows have not gone away yet, so the size waits for that answer.
        let busy = self.options_open
            || self.editor_sizing.options_size.is_some()
            || waveform_only
            || self.gesture.is_some()
            || self.native_dragging.load(Ordering::Acquire)
            || self.timeline.sample_rate == 0;
        if let Some(size) = self
            .editor_sizing
            .fit_rows(current, wanted, self.display_size.y, busy)
        {
            ui.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
    }

    /// Whether the capture shows one row per channel under L and R.
    fn stacked(&self) -> bool {
        self.timeline.wide.is_some()
    }

    /// Where the rows go in the waveform's `rect`: inside its margins, and right of the label
    /// gutter while a wide capture's channels are named beside their rows.
    fn timeline_rect(&self, rect: Rect) -> Rect {
        let inner = rect.shrink(8.0);
        if self.stacked() && !self.waveform_only {
            Rect::from_min_max(
                Pos2::new(
                    (rect.left() + stack::LABEL_GUTTER).min(inner.right()),
                    inner.top(),
                ),
                inner.max,
            )
        } else {
            inner
        }
    }

    /// Where each lane's rows go in a plot `height` points high.
    fn lane_boxes(&self, height: f32) -> Vec<LaneBox> {
        self.timeline.wide.as_ref().map_or_else(Vec::new, |wide| {
            let lanes = wide_lanes(wide, self.timeline.b_channels.is_some());
            stack::lay_out(height, &lanes, &[self.first_row, 0])
        })
    }

    /// The rows of `wide` on screen, with the sidechain's below when it is captured too.
    fn rows_for(&self, wide: &WideLayout, sidechain: bool) -> Vec<WideRow> {
        let boxes = stack::lay_out(
            self.display_size.y,
            &wide_lanes(wide, sidechain),
            &[self.first_row, 0],
        );
        rows_on_screen(wide, &boxes)
    }

    fn audio_rect(&self, rect: Rect, seconds: f32) -> Rect {
        let inner = self.timeline_rect(rect);
        let mut seconds = self.editing_seconds.unwrap_or(seconds);
        let kept = self.timeline.retention_seconds as f32;
        if self.stacked() && kept.is_finite() && kept > 0.0 {
            // More channels keep less history; the plot's width is what memory holds.
            seconds = seconds.min(kept).max(1.0);
        }
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
        let border = f32::from(settings.border_width);
        let panel = if settings.border_enabled { rect.shrink(border) } else { rect };
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
                Stroke::new(border, Color32::from_rgba_unmultiplied(r, g, b, alpha)),
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
                Some(Gesture::Select { .. } | Gesture::ResizeSelection { .. } | Gesture::Fade { .. }) => false,
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
        // A selection being drawn has no fades yet; one being resized keeps its own.
        let fades = match &self.gesture {
            Some(Gesture::Select { .. }) => Fades::NONE,
            _ if explicit_range.is_some() => self.fades,
            _ => Fades::NONE,
        };
        let color = accent_color(settings.accent);
        if hover_preview && width > 0.0 {
            painter.rect_filled(audio_rect, 2.0, muted_accent(color));
        }

        let selected_color = selected_waveform_color(color, SELECTION_CONTRAST);
        if width > 0.0 && opacity > 0 {
            self.draw_reference_lines(&painter, audio_rect, settings, opacity);
        }
        let selection_clip = explicit_range.as_ref().and_then(|range|
            waveform_selection_clip(audio_rect, &self.peak_frames, range));
        // Coloring by frequency: the band colors, once for every row.
        let palette = spectrum::waveform_palette(settings);
        let trace = RowTrace {
            audio_rect,
            range: explicit_range.as_ref(),
            fades,
            stroke: policy.waveform_stroke,
            color,
            selected_color,
            selection_clip,
            palette: palette.as_ref(),
        };
        if let Some(wide) = &self.timeline.wide {
            let hover = ui.input(|input| input.pointer.hover_pos());
            self.draw_rows(&painter, rect, wide, &trace, policy.seconds_stroke, hover);
        } else {
            let rows = if self.timeline.b_channels.is_some() {
                2
            } else {
                waveform_views(self.peak_layout, self.timeline.channels).len()
            };
            let row_height = inner.height() / rows as f32;
            let extent = row_height * 0.47;
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
                self.paint_row(&painter, &trace, peaks, middle, extent, self.color_row(row));
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
            if let Some(frame) = self.playhead() {
                paint_playhead(&painter, audio_rect, &self.peak_frames, frame, PLAYHEAD_COLOR);
            }
        }
        if !waveform_only && let Some(Gesture::Fade { end, .. }) = &self.gesture {
            let rate = f64::from(self.timeline.sample_rate.max(1));
            let (name, frames) = match end {
                FadeEnd::In => ("Fade in", fades.fade_in),
                FadeEnd::Out => ("Fade out", fades.fade_out),
            };
            painter.text(
                audio_rect.right_bottom() - Vec2::new(4.0, 2.0),
                egui::Align2::RIGHT_BOTTOM,
                format!("{name} {:.2} s", frames as f64 / rate),
                egui::FontId::monospace(11.0),
                Color32::WHITE,
            );
        }
    }

    /// One row's waveform about `middle`, tapered the way the clip is written, with its
    /// selection drawn over it. While the waveform is colored by frequency, each column takes
    /// the color read for `row` there.
    fn paint_row(
        &self,
        painter: &egui::Painter,
        trace: &RowTrace<'_>,
        peaks: &WaveformPeaks,
        middle: f32,
        extent: f32,
        row: ColorRow,
    ) {
        let tapered;
        let peaks = match trace.range {
            // The waveform tapers the way the exported clip does.
            Some(range) if !trace.fades.is_none() => {
                let mut faded = peaks.clone();
                fade_waveform_bins(
                    &mut faded.minimum,
                    &mut faded.maximum,
                    self.peak_frames.clone(),
                    range,
                    trace.fades,
                );
                tapered = faded;
                &tapered
            }
            _ => peaks,
        };
        let geometry =
            waveform::waveform_geometry(&peaks.minimum, &peaks.maximum, self.peak_frames.clone());
        if let Some(palette) = trace.palette {
            let colors = self.waveform_colors.row_colors(
                row,
                &self.peak_frames,
                peaks.minimum.len(),
                palette,
            );
            paint_colored_row(painter, trace, &geometry, middle, extent, &colors);
        } else {
            paint_waveform_geometry(
                painter,
                &geometry,
                trace.audio_rect,
                middle,
                extent,
                trace.stroke,
                trace.color,
            );
            if let Some(clip) = trace.selection_clip {
                paint_waveform_geometry(
                    &painter.with_clip_rect(clip),
                    &geometry,
                    trace.audio_rect,
                    middle,
                    extent,
                    trace.stroke,
                    trace.selected_color,
                );
            }
        }
        if let Some(range) = trace.range {
            // Full scale is drawn at the height paint_waveform_geometry uses.
            paint_fades(
                painter,
                trace.audio_rect,
                middle,
                extent,
                &self.peak_frames,
                range,
                trace.fades,
                FLAG_COLOR,
            );
        }
    }

    /// The rows of a capture wider than stereo, as on the desktop: L and R on a faint backing
    /// with a rule below, a row per channel named in the gutter, and the sidechain's row below
    /// a lane line when it is captured too. A lane whose rows do not all fit has a scroll bar
    /// in the right margin. `line` is the centre lines' width.
    fn draw_rows(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        wide: &WideLayout,
        trace: &RowTrace<'_>,
        line: f32,
        hover: Option<Pos2>,
    ) {
        let inner = self.timeline_rect(rect);
        let audio = trace.audio_rect;
        let boxes = self.lane_boxes(inner.height());
        for (lane, lane_box) in boxes.iter().enumerate() {
            for index in lane_box.visible() {
                let Some(source) = wide_row(wide, lane, index) else {
                    continue;
                };
                let top = inner.top() + lane_box.row_top(index).unwrap_or_default();
                let row = Rect::from_min_size(
                    Pos2::new(inner.left(), top),
                    Vec2::new(inner.width(), lane_box.row_height),
                );
                let middle = row.center().y;
                if !self.waveform_only {
                    if matches!(source, WideRow::Pair(_)) {
                        painter.rect_filled(
                            Rect::from_x_y_ranges(rect.left()..=audio.right(), row.y_range()),
                            0.0,
                            PAIR_BACKING,
                        );
                    }
                    painter.hline(inner.x_range(), middle, Stroke::new(line, CENTER_LINE));
                    if source == WideRow::Pair(PairSide::Right)
                        && index + 1 < lane_box.first + lane_box.shown
                    {
                        painter.hline(
                            rect.left() + 1.0..=audio.right(),
                            row.bottom(),
                            Stroke::new(1.0, PAIR_DIVIDER),
                        );
                    }
                    if lane_box.row_height >= stack::LABELLED_ROW_HEIGHT {
                        painter.text(
                            Pos2::new(rect.left() + 1.0, middle),
                            egui::Align2::LEFT_CENTER,
                            row_label(wide, source),
                            egui::FontId::monospace(8.0),
                            Color32::from_gray(180),
                        );
                    }
                }
                if audio.width() <= 0.0 {
                    continue;
                }
                // Rows just scrolled into view wait for their peaks.
                if let Some(peaks) = self
                    .rows
                    .iter()
                    .position(|shown| *shown == source)
                    .and_then(|position| self.row_peaks.get(position))
                {
                    let extent = lane_box.row_height * 0.47;
                    self.paint_row(painter, trace, peaks, middle, extent, wide_color_row(source));
                }
            }
        }
        if self.waveform_only {
            return;
        }
        for lane_box in boxes.iter().skip(1) {
            painter.hline(
                rect.left() + 1.0..=audio.right(),
                inner.top() + lane_box.top,
                Stroke::new(1.0, Color32::from_white_alpha(36)),
            );
        }
        if let Some(lane_box) = boxes.first().filter(|lane_box| lane_box.scrolls()) {
            let track = scroll_track(rect, inner, lane_box);
            let x = track.center().x - 1.5..=track.center().x + 1.5;
            painter.rect_filled(
                Rect::from_x_y_ranges(x.clone(), track.y_range()),
                1.5,
                Color32::from_white_alpha(18),
            );
            let thumb = lane_box.thumb();
            let active =
                self.scroll_drag.is_some() || hover.is_some_and(|point| track.contains(point));
            painter.rect_filled(
                Rect::from_x_y_ranges(x, inner.top() + thumb.start..=inner.top() + thumb.end),
                1.5,
                Color32::from_gray(if active { 200 } else { 128 }),
            );
        }
    }

    /// The scroll bar and the wheel over the margins of a wide capture's lane, as on the desktop.
    /// Returns the tooltip of the row beside the pointer, in the gutter or the right margin.
    fn interact_lanes(&mut self, ui: &egui::Ui, rect: Rect, enabled: bool) -> Option<String> {
        self.scroll_zones.clear();
        let Some(wide) = self
            .timeline
            .wide
            .clone()
            .filter(|_| enabled && !self.waveform_only)
        else {
            self.scroll_drag = None;
            return None;
        };
        let inner = self.timeline_rect(rect);
        let boxes = self.lane_boxes(inner.height());
        let lane = *boxes.first()?;
        let track = scroll_track(rect, inner, &lane);
        if lane.scrolls() {
            self.scroll_zones.push(track);
            self.scroll_zones.push(Rect::from_x_y_ranges(
                rect.left()..=inner.left(),
                track.y_range(),
            ));
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
        if lane.scrolls()
            && self.gesture.is_none()
            && let Some(point) = press.filter(|point| track.contains(*point))
        {
            let thumb = lane.thumb();
            let (top, bottom) = (inner.top() + thumb.start, inner.top() + thumb.end);
            self.scroll_drag = Some(if (top..=bottom).contains(&point.y) {
                point.y - top
            } else {
                // A press beside the thumb brings it there.
                (bottom - top) / 2.0
            });
        }
        if let Some(grab) = self.scroll_drag {
            if down && let Some(point) = position {
                self.scroll_rows_to(lane.first_for_thumb(point.y - grab - inner.top()));
            } else if !down {
                self.scroll_drag = None;
            }
        }
        if steps != 0.0
            && let Some(point) = hover
            && self.scroll_zones.iter().any(|zone| zone.contains(point))
        {
            // At least one row for each turn of the wheel; up shows earlier channels.
            let rows = steps.abs().ceil().max(1.0) as usize;
            self.scroll_rows_to(if steps > 0.0 {
                lane.first.saturating_sub(rows)
            } else {
                lane.first + rows
            });
        }
        let point = hover.filter(|point| {
            rect.contains(*point) && !(inner.left()..inner.right()).contains(&point.x)
        })?;
        let (index, row) = stack::row_at(&boxes, point.y - inner.top())?;
        let mut text = row_tooltip(&wide, wide_row(&wide, index, row)?)?;
        if boxes[index].scrolls() {
            text.push('\n');
            text.push_str(SCROLL_HINT);
        }
        Some(text)
    }

    /// Shows the wide lane's rows from `first`, kept so that the last row ends the lane.
    fn scroll_rows_to(&mut self, first: usize) {
        let first = self
            .lane_boxes(self.display_size.y)
            .first()
            .map_or(0, |lane| first.min(lane.rows - lane.shown));
        if first != self.first_row {
            self.first_row = first;
            self.next_refresh = Instant::now();
            self.session.mark_interaction();
        }
    }

    /// Whether a press at `position` belongs to the scroll bar rather than the waveform.
    fn on_scroll_track(&self, position: Pos2) -> bool {
        self.scroll_drag.is_some()
            || self
                .scroll_zones
                .first()
                .is_some_and(|track| track.contains(position))
    }

    /// Under the history setting, when the capture's width leaves room for less than was asked
    /// for, says how much memory keeps, as on the desktop.
    fn short_history_note(&self, retention: DurationChoice) -> Option<String> {
        let requested = f64::from(retention.seconds());
        let kept = self.timeline.retention_seconds;
        if !kept.is_finite() || kept <= 0.0 || kept >= requested - 0.5 {
            return None;
        }
        let channels = self.timeline.channels + self.timeline.b_channels.unwrap_or(0);
        Some(format!(
            "With {channels} channels, memory holds the latest {}.",
            kept_duration(kept)
        ))
    }

    /// The head model for a binaural preview at `rate`, made the first time it is needed.
    fn head_model(&mut self, rate: u32) -> Option<Arc<HeadModel>> {
        if let Some(model) = self
            .head_model
            .as_ref()
            .filter(|model| model.sample_rate() == rate)
        {
            return Some(Arc::clone(model));
        }
        let model = Arc::new(HeadModel::new(rate).ok()?);
        self.head_model = Some(Arc::clone(&model));
        Some(model)
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
        // Over a wide lane's margins the wheel scrolls its rows instead.
        let over_rows =
            |pointer: &Pos2| self.scroll_zones.iter().any(|zone| zone.contains(*pointer));
        let next = if let Some(pointer) =
            pointer.filter(|pointer| rect.contains(*pointer) && !over_rows(pointer))
            && wheel != 0.0
        {
            let audio_rect = self.audio_rect(rect, settings.retention.seconds() as f32);
            Some(wheel_zoom_view(
                &full,
                &current,
                wheel,
                pointer.x,
                audio_rect,
                self.timeline_rect(rect),
            ))
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
        let (position, press, alt_press, down, shift) = ui.input(|input| {
            let press = waveform_press(&input.events, ALT_SELECTS);
            // Whether Alt was held at that press: on a flag of the selection it drags a fade.
            let alt_press = input.events.iter().rev().find_map(|event| match event {
                egui::Event::PointerButton {
                    button: PointerButton::Primary, pressed: true, modifiers, ..
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
        if self.native_dragging.load(Ordering::Acquire) {
            return;
        }
        if allow_new_gesture {
            self.interact_viewport(ui, waveform, settings);
        }

        if allow_new_gesture
            && let Some((position, selecting, snap)) = press.filter(|(position, _, _)| {
                rect.contains(*position) && !self.on_scroll_track(*position)
            })
        {
            self.discard_drag_preparation();
            self.next_refresh = Instant::now();
            self.refresh_waveform(self.session.is_paused(), settings.waveform_layout);
            let audio_rect = self.audio_rect(rect, settings.retention.seconds() as f32);
            if let Some(end) = alt_press.then(|| self.fade_flag(audio_rect, position)).flatten() {
                // The selection and a playing preview stay; only the ramp changes.
                self.session.set_gesture_active(true);
                waveform.request_focus();
                self.gesture = Some(Gesture::Fade {
                    end,
                    press: self.frame_at_pointer(audio_rect, position.x, false, settings),
                    origin_x: position.x,
                    initial: self.fades,
                    moved: false,
                });
            } else if let Some(fixed) = self.selection_handle(audio_rect, position) {
                // A resized selection replaces the one that was playing.
                self.stop_preview();
                self.editing_seconds.get_or_insert(settings.retention.seconds() as f32);
                self.session.begin_waveform_gesture();
                waveform.request_focus();
                let current = self.frame_at_pointer(audio_rect, position.x, snap, settings);
                self.gesture = Some(Gesture::ResizeSelection { fixed, current });
            } else if selecting {
                self.stop_preview();
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

        if let Some(position) = position
            && let Some(Gesture::Fade { end, press: pressed, origin_x, initial, moved }) = self.gesture
            && let Some(selection) = self.selection.clone()
        {
            let moved = moved || (position.x - origin_x).abs() >= 2.0;
            if moved {
                let audio_rect = self.audio_rect(rect, settings.retention.seconds() as f32);
                let pointer = self.frame_at_pointer(audio_rect, position.x, false, settings);
                self.fades = dragged_fades(end, &selection, initial, pressed, pointer);
                // A playing preview takes the new ramps from its next block.
                let fades = self.fades;
                self.session.preview.with_current(|preview| preview.set_fades(fades));
            }
            self.gesture = Some(Gesture::Fade { end, press: pressed, origin_x, initial, moved });
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
                    self.fades = Fades::NONE;
                    self.stop_preview();
                    self.session.set_snapshot_selection(None);
                    self.message.clear();
                    self.start_native_drag(frame, file, origin);
                }
            }
        } else if let Some(gesture) = self.gesture.take() {
            if let Gesture::Fade { .. } = gesture {
                // Only the ramps changed. The selection and its material stay as they were.
                self.publish_selection();
                self.session.set_gesture_active(false);
                return;
            }
            let resized = matches!(gesture, Gesture::ResizeSelection { .. });
            if let Gesture::Select { anchor, current } | Gesture::ResizeSelection { fixed: anchor, current } = gesture {
                let range = anchor.min(current)..anchor.max(current);
                // A resized selection keeps the ramps at its flags; a new one has none.
                self.fades = if resized { self.fades.fitted(range.end - range.start) } else { Fades::NONE };
                self.selection = (!range.is_empty()).then_some(range);
                if self.selection.is_none() {
                    self.fades = Fades::NONE;
                }
                self.publish_selection();
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
        let Some(material) = self.session.export_snapshots(false).filter(|s| {
            s.tracks
                .first()
                .is_some_and(|snapshot| snapshot.frame_count() > 0)
        }) else {
            self.message = "No captured audio to save yet.".into();
            return;
        };
        self.sync_timeline(material.epoch);
        let wide = wide_save(material.wide, &settings);
        let Some(clip) = self.wav_clip(
            material.tracks,
            material.mask,
            settings.output_dir,
            settings.wav_profile,
            settings.force_mono,
        ) else {
            self.message = "The selected audio is no longer available.".into();
            return;
        };
        let clip = ClipJob { clip, wide };
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
        let snapshots = self.session.export_snapshots(false);
        let Some(material) = snapshots.filter(|s| {
            s.tracks
                .first()
                .is_some_and(|snapshot| snapshot.frame_count() > 0)
        }) else {
            self.message = "No captured audio to save yet.".into();
            return;
        };
        self.sync_timeline(material.epoch);
        let wide = wide_save(material.wide, settings);
        let Some(clip) = self.wav_clip(
            material.tracks,
            material.mask,
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
        let result = self.queue_saved_snapshot(ClipJob { clip, wide });
        self.message = match result {
            Ok(()) => "Saving snapshot…".into(),
            Err(error) => format!("Could not save snapshot: {error}"),
        };
    }

    fn wav_clip(
        &mut self,
        snapshots: Arc<[FrozenTrack]>,
        mask: u32,
        output: PathBuf,
        profile: WavExportProfile,
        force_mono: bool,
    ) -> Option<WavClip> {
        let first = snapshots.first()?;
        let selected = self.selection.clone().unwrap_or_else(|| first.frame_range());
        let range = export_frame_range(first, &selected)?;
        Some(WavClip {
            mask,
            profile,
            force_mono,
            output,
            snapshots,
            range,
            marker: self.markers.issue(selected),
            tempo: self.session.host_tempo(),
            fades: if self.selection.is_some() { self.fades } else { Fades::NONE },
        })
    }

    fn queue_saved_snapshot(&mut self, clip: ClipJob) -> io::Result<()> {
        let writer = match &self.snapshot_writer {
            Some(writer) => writer,
            None => self.snapshot_writer.insert(SnapshotWriter::start()?),
        };
        writer.requests.send(clip)
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

/// Takes the presses of Space without modifiers, held repeats included, and returns whether one
/// was new. A held key does not repeat the command, as on the desktop.
fn take_space_press(input: &mut egui::InputState) -> bool {
    let mut pressed = false;
    input.events.retain(|event| match event {
        egui::Event::Key { key: egui::Key::Space, pressed: true, repeat, modifiers, .. }
            if modifiers.is_none() =>
        {
            pressed |= !repeat;
            false
        }
        _ => true,
    });
    pressed
}

fn export_frame_range(snapshot: &FrozenTrack, selection: &Range<u64>) -> Option<Range<usize>> {
    let bounds = snapshot.frame_range();
    if selection.is_empty() || selection.start < bounds.start || selection.end > bounds.end {
        return None;
    }
    Some((selection.start - bounds.start) as usize..(selection.end - bounds.start) as usize)
}

/// How a capture with the wide main input `layout` is saved and dragged out, by `settings`.
fn wide_save(layout: Option<WideLayout>, settings: &SessionSettings) -> Option<WideSave> {
    layout.map(|layout| WideSave {
        layout,
        mode: settings.wide_export,
        pair_binaural: settings.pair_render.binaural(),
    })
}

/// How the rows of one frame are drawn: where the audio goes, the selection and its fades.
struct RowTrace<'a> {
    audio_rect: Rect,
    range: Option<&'a Range<u64>>,
    fades: Fades,
    stroke: f32,
    color: Color32,
    selected_color: Color32,
    selection_clip: Option<Rect>,
    /// The band colors while the waveform is colored by frequency.
    palette: Option<&'a SpectrumPalette>,
}

/// The row of a capture wider than stereo that the colors of `row` are read for.
fn wide_color_row(row: WideRow) -> ColorRow {
    match row {
        WideRow::Pair(side) => ColorRow::Pair(0, side),
        WideRow::Channel(channel) => ColorRow::Channel(0, channel),
        WideRow::Sidechain => ColorRow::View(1, waveform::WaveformView::Merged),
    }
}

/// The lanes of a capture wider than stereo: L, R and one row per channel, which scroll when
/// they do not fit, then the sidechain's one row when it is captured too.
fn wide_lanes(wide: &WideLayout, sidechain: bool) -> Vec<stack::LaneRows> {
    let mut lanes = vec![stack::LaneRows {
        rows: wide.labels.len() + 2,
        stacked: true,
    }];
    if sidechain {
        lanes.push(stack::LaneRows {
            rows: 1,
            stacked: false,
        });
    }
    lanes
}

/// Row `row` of lane `lane` of [`wide_lanes`].
fn wide_row(wide: &WideLayout, lane: usize, row: usize) -> Option<WideRow> {
    match (lane, row) {
        (0, row) => wide.rows().nth(row),
        (1, 0) => Some(WideRow::Sidechain),
        _ => None,
    }
}

/// The rows on screen in `boxes`, laid out from [`wide_lanes`], top to bottom.
fn rows_on_screen(wide: &WideLayout, boxes: &[LaneBox]) -> Vec<WideRow> {
    boxes
        .iter()
        .enumerate()
        .flat_map(|(lane, lane_box)| {
            lane_box
                .visible()
                .filter_map(move |row| wide_row(wide, lane, row))
        })
        .collect()
}

/// What the gutter says beside a row: L and R, the channel's short speaker name or else its
/// number, and B for the sidechain, as the desktop names its second lane.
fn row_label(wide: &WideLayout, row: WideRow) -> String {
    match row {
        WideRow::Pair(PairSide::Left) => "L".into(),
        WideRow::Pair(PairSide::Right) => "R".into(),
        WideRow::Channel(channel) => Some(stack::short_label(
            wide.labels.get(channel).map_or("", String::as_str),
        ))
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| (channel + 1).to_string()),
        WideRow::Sidechain => "B".into(),
    }
}

/// The tooltip of a row's label, in the desktop's English words. The sidechain's has none.
fn row_tooltip(wide: &WideLayout, row: WideRow) -> Option<String> {
    match row {
        WideRow::Pair(PairSide::Left) => {
            Some("L: the left side of all channels mixed to stereo".into())
        }
        WideRow::Pair(PairSide::Right) => {
            Some("R: the right side of all channels mixed to stereo".into())
        }
        WideRow::Channel(channel) => {
            let label = wide
                .labels
                .get(channel)
                .map_or_else(|| (channel + 1).to_string(), Clone::clone);
            Some(format!(
                "Channel {} of {}: {label}",
                channel + 1,
                wide.labels.len()
            ))
        }
        WideRow::Sidechain => None,
    }
}

/// The scroll bar's track in the right margin, as high as the lane.
fn scroll_track(rect: Rect, inner: Rect, lane: &LaneBox) -> Rect {
    Rect::from_x_y_ranges(
        inner.right()..=rect.right(),
        inner.top() + lane.top..=inner.top() + lane.top + lane.height,
    )
}

/// A length of history as the desktop says it: "1 minute 56 seconds".
fn kept_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).floor() as u64;
    let part =
        |value: u64, unit: &str| format!("{value} {unit}{}", if value == 1 { "" } else { "s" });
    match (total / 60, total % 60) {
        (0, seconds) => part(seconds, "second"),
        (minutes, 0) => part(minutes, "minute"),
        (minutes, seconds) => format!("{} {}", part(minutes, "minute"), part(seconds, "second")),
    }
}

/// The Options form: the list of pages on the left and the selected page on the right.
fn options_layout<R>(
    ui: &mut egui::Ui,
    selected: &mut OptionsTab,
    accent: Color32,
    content: impl FnOnce(&mut egui::Ui, OptionsTab) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    ui.scope(|ui| {
        // Options has its own readable type scale; the compact toolbar keeps its own.
        configure_options_style(ui);
        let size = ui.available_size();
        // A host can keep the editor small, so the list and the margins give way first.
        let navigation_width =
            (size.x * 0.3).clamp(104.0, OPTIONS_NAVIGATION_WIDTH).min(size.x * 0.42);
        let roomy = size.x >= 560.0;
        let title = |ui: &mut egui::Ui, label: &str| {
            let text = options_heading_text(ui, label, 20.0);
            ui.add(egui::Label::new(text).wrap());
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(6.0);
        };
        ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(navigation_width, size.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(navigation_width);
                ui.spacing_mut().item_spacing.y = 4.0;
                egui::ScrollArea::vertical()
                    .id_salt("options-navigation")
                    .auto_shrink([false, false])
                    .max_height(size.y)
                    .show(ui, |ui| {
                        for tab in OptionsTab::ALL {
                            if options_navigation_item(ui, tab.label(), *selected == tab, accent)
                                .clicked()
                            {
                                *selected = tab;
                            }
                        }
                    });
            },
        );
        ui.separator();
        // The page keeps clear of the list beside it, and its right edge is left to the
        // scroll handle, which would otherwise sit on top of the controls.
        ui.add_space(if roomy { 12.0 } else { 0.0 });
        let gutter: i8 = if roomy { 16 } else { 10 };
        let width = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(width, size.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(width);
                // The title stays in place above a tall form. A short editor scrolls it away
                // with the page, to keep what room there is for the settings.
                let fixed_title = size.y >= 280.0;
                if fixed_title {
                    ui.add_space(4.0);
                    ui.allocate_ui(Vec2::new(width - f32::from(gutter), 0.0), |ui| {
                        title(ui, selected.label());
                    });
                }
                egui::ScrollArea::vertical()
                    .id_salt(("options-content", *selected as u8))
                    .auto_shrink([false, false])
                    .max_width(width)
                    .max_height(ui.available_height())
                    .show(ui, |ui| {
                        // A ScrollArea inherits its parent's layout. The form is explicitly
                        // vertical so each setting advances down, including narrow editors.
                        let page = egui::Margin { left: 0, right: gutter, top: 0, bottom: 0 };
                        egui::Frame::NONE.inner_margin(page).show(ui, |ui| {
                            options_list(ui, |ui| {
                                if !fixed_title {
                                    title(ui, selected.label());
                                }
                                content(ui, *selected)
                            })
                        }).inner
                    })
            },
        ).inner
        }).inner
    }).inner
}

/// A named drop-down, one row of the form.
fn option_combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut T,
    choices: impl IntoIterator<Item = (T, String)>,
) {
    options_field(ui, label, |ui| {
        options_choice(ui, label, value, choices);
    });
}

#[cfg(test)]
fn capture_options(ui: &mut egui::Ui, settings: &mut SessionSettings) {
    capture_page(ui, settings, None);
}

/// The Capture page. `history_note` says how much history memory keeps when the capture's
/// width leaves room for less than the setting asks for.
fn capture_page(ui: &mut egui::Ui, settings: &mut SessionSettings, history_note: Option<&str>) {
    option_combo(ui, "Retain the latest audio", &mut settings.retention,
        DurationChoice::ALL.map(|duration| (duration, duration_label(duration.seconds()))));
    if let Some(note) = history_note {
        options_note(ui, note);
    }
    ui.checkbox(&mut settings.retain_silence, "Keep quiet sections");
    option_combo(ui, "Auto-resume delay", &mut settings.idle_resume,
        AutoResumeDelay::ALL.map(|delay| (delay, auto_resume_label(delay))));
    ui.separator();
    options_group_heading(ui, "Multichannel sources");
    option_combo(
        ui,
        "L/R of multichannel sources",
        &mut settings.pair_render,
        PairRender::ALL.map(|pair| (pair, pair.label().to_owned())),
    );
    options_note(
        ui,
        "How L and R sound on export and when preview plays as L/R. Binaural places each channel around your head, for headphones.",
    );
    option_combo(
        ui,
        "Preview of multichannel sources",
        &mut settings.preview_as_lr,
        [
            (false, "In their own channels".into()),
            (true, "As L/R".into()),
        ],
    );
    options_note(
        ui,
        "With a matching bus, each channel plays on the same channel for your DAW to monitor. As L/R plays the mix above on the first two channels.",
    );
}

/// "Off" leaves a paused capture paused. It is stored as 0 seconds, which never meant "at once".
fn auto_resume_label(delay: AutoResumeDelay) -> String {
    match delay.delay() {
        Some(delay) => format!("{} seconds", delay.as_secs()),
        None => "Off".into(),
    }
}

fn duration_label(seconds: u32) -> String {
    if seconds <= 60 {
        format!("{seconds} seconds")
    } else {
        format!("{} minutes", seconds / 60)
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
        options_field(ui, "Sender name", |ui| {
            if let Some(name) = host_track_name {
                options_value(ui, name);
            } else {
                ui.add(egui::TextEdit::singleline(&mut settings.sender_name)
                    .desired_width(options_control_width(ui))
                    .char_limit(80)
                    .hint_text("Sender name"));
            }
        });
    }
}

fn export_options(ui: &mut egui::Ui, settings: &mut SessionSettings) -> Option<FolderAction> {
    wav_format_controls(ui, &mut settings.wav_profile);
    ui.checkbox(&mut settings.force_mono, "Export every file as mono");
    option_combo(
        ui,
        "Multichannel sources",
        &mut settings.wide_export,
        WIDE_EXPORT_MODES.map(|mode| (mode, wide_export_label(mode).to_owned())),
    );
    options_note(
        ui,
        "Binaural files let you hear on headphones where each channel sits.",
    );
    ui.separator();
    folder_options(ui, settings)
}

#[cfg(test)]
fn appearance_options(ui: &mut egui::Ui, settings: &mut SessionSettings) {
    appearance_page(ui, settings, false);
}

/// The Appearance page. `wide` says the capture is wider than stereo, which has no loudness.
fn appearance_page(ui: &mut egui::Ui, settings: &mut SessionSettings, wide: bool) {
    options_group_heading(ui, "Window");
    ui.checkbox(&mut settings.show_waveform_only, "Show waveform only");
    option_combo(ui, "Background transparency", &mut settings.panel_transparency,
        TRANSPARENCY_LEVELS.map(|percent| (percent, format!("{percent}%"))));
    options_field(ui, "Theme color", |ui| {
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut settings.accent);
            if ui.button("Reset").clicked() {
                settings.accent = DEFAULT_ACCENT;
            }
        });
    });
    spectrum::waveform_color_options(ui, settings);
    ui.separator();
    options_group_heading(ui, "Show");
    ui.checkbox(&mut settings.show_title, "App name in the toolbar");
    ui.checkbox(&mut settings.show_meter, "Level meter");
    option_combo(ui, "Capture status mark", &mut settings.status_mark,
        StatusMark::ALL.map(|mark| (mark, mark.label().to_owned())));
    ui.separator();
    options_group_heading(ui, "Timeline guides");
    ui.checkbox(&mut settings.show_bar_lines, "Show bar lines");
    ui.checkbox(&mut settings.show_seconds_lines, "Show seconds lines");
    ui.add_enabled_ui(settings.show_seconds_lines, |ui| {
        option_combo(ui, "Seconds interval", &mut settings.seconds_interval,
            [1, 5, 10].map(|seconds| {
                (seconds, if seconds == 1 { "1 second".to_owned() } else { format!("{seconds} seconds") })
            }));
    });
    ui.separator();
    options_group_heading(ui, "Waveform border");
    ui.checkbox(&mut settings.border_enabled, "Show a border");
    ui.add_enabled_ui(settings.border_enabled, |ui| {
        option_combo(ui, "Border width", &mut settings.border_width,
            BORDER_WIDTHS.map(|width| (width, format!("{width} px"))));
        let mut theme = settings.border_color.is_none();
        let theme_changed = ui.checkbox(&mut theme, "Use theme color").changed();
        let mut color = settings.border_color.unwrap_or(settings.accent);
        let color_changed = options_field(ui, "Custom border color", |ui| {
            ui.add_enabled_ui(!theme, |ui| ui.color_edit_button_srgb(&mut color)).inner.changed()
        });
        if theme_changed || color_changed {
            settings.border_color = (!theme).then_some(color);
        }
        ui.checkbox(&mut settings.border_follows_background, "Follow background opacity");
    });
    ui.separator();
    options_group_heading(ui, "Display");
    option_combo(ui, "Waveform", &mut settings.waveform_layout,
        WaveformLayout::ALL.map(|layout| (layout, layout.label().to_owned())));
    option_combo(ui, "Meter", &mut settings.meter_mode,
        MeterMode::ALL.map(|mode| (mode, mode.label().to_owned())));
    if wide && settings.meter_mode == MeterMode::Lufs {
        options_note(ui, NO_LOUDNESS);
    }
}

fn folder_options(ui: &mut egui::Ui, settings: &mut SessionSettings) -> Option<FolderAction> {
    let mut action = None;
    let mut folder = |ui: &mut egui::Ui, kind: FolderKind, settings: &SessionSettings| {
        options_field(ui, kind.label(), |ui| {
            options_value(ui, &kind.path(settings).display().to_string());
            ui.horizontal_wrapped(|ui| {
                if ui.button("Choose folder…").clicked() {
                    action = Some(FolderAction::Select(kind));
                }
                if ui.button("Open folder").clicked() {
                    action = Some(FolderAction::Open(kind));
                }
            });
        });
    };
    folder(ui, FolderKind::Export, settings);
    output_cleanup_options(ui, settings);
    folder(ui, FolderKind::Saved, settings);
    action
}

fn output_cleanup_options(ui: &mut egui::Ui, settings: &mut SessionSettings) {
    let note = match settings.output_cleanup {
        None => "Files you drag into other programs stay in this folder.".to_owned(),
        Some(_) if cleanup::same_folder(&settings.output_dir, &settings.saved_dir) => {
            "Not active while this is also the saved snapshots folder.".to_owned()
        }
        Some(days) => format!(
            "Backtracking's WAV files in this folder are permanently deleted {days} days \
             after they were written. A project that still uses one loses it."
        ),
    };
    options_field(ui, "Delete old drag-and-drop files", |ui| {
        let choices = std::iter::once(None).chain(cleanup::AGE_CHOICES.map(Some));
        options_choice(ui, "output-cleanup", &mut settings.output_cleanup,
            choices.map(|age| (age, cleanup::age_label(age))));
        options_note(ui, note);
    });
}

fn wav_format_controls(ui: &mut egui::Ui, profile: &mut WavExportProfile) {
    option_combo(ui, "WAV sample rate", &mut profile.sample_rate,
        WAV_SAMPLE_RATES.map(|(rate, label)| (rate, label.to_owned())));
    option_combo(ui, "WAV encoding", &mut profile.sample_format,
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

/// [`paint_waveform_geometry`] with the color `color_at` gives each column and sample, and
/// `color` where it gives none. The fill under each is the one the theme color has.
#[allow(clippy::too_many_arguments)]
fn paint_colored_waveform_geometry(
    painter: &egui::Painter, geometry: &waveform::WaveformGeometry, rect: Rect,
    middle: f32, extent: f32, stroke_width: f32, color: Color32,
    color_at: impl Fn(f32) -> Option<Color32>,
) {
    let position = |x: f32, value: f32| Pos2::new(rect.left() + rect.width() * x, middle - value * extent);
    let style = TraceStyle {
        stroke: Stroke::new(stroke_width, color),
        fill: spectrum::trace_fill(color),
        lone_sample: LoneSample::Dash(stroke_width.max(1.0)),
    };
    paint_colored_waveform_trace(painter, geometry, position, &style, color_at, spectrum::trace_fill);
}

/// One row's `geometry` with each column in its frequency color, as on the desktop: in full
/// inside the selection and, while one is on screen, dimmed toward grey outside it. A column
/// whose color has not come in yet takes the theme color, treated the same way. `colors` has
/// one entry per bin the geometry was made from.
fn paint_colored_row(
    painter: &egui::Painter, trace: &RowTrace<'_>, geometry: &waveform::WaveformGeometry,
    middle: f32, extent: f32, colors: &[Option<Color32>],
) {
    let paint = |painter: &egui::Painter, tint: fn(Color32) -> Color32| {
        paint_colored_waveform_geometry(
            painter, geometry, trace.audio_rect, middle, extent, trace.stroke, trace.color,
            |x| {
                let own = colors.get(trace_bin(x, colors.len())).copied().flatten();
                Some(tint(own.unwrap_or(trace.color)))
            },
        );
    };
    let Some(clip) = trace.selection_clip else {
        paint(painter, std::convert::identity);
        return;
    };
    let whole = painter.clip_rect();
    let left = Rect::from_x_y_ranges(whole.left()..=clip.left(), whole.y_range());
    let right = Rect::from_x_y_ranges(clip.right()..=whole.right(), whole.y_range());
    paint(&painter.with_clip_rect(left), spectrum::unselected_color);
    paint(&painter.with_clip_rect(clip), std::convert::identity);
    paint(&painter.with_clip_rect(right), spectrum::unselected_color);
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
        FLAG_COLOR
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
        assert_eq!(labels, ["Off", "5 seconds", "10 seconds"]);
    }

    #[test]
    fn space_toggles_once_per_press_and_only_without_modifiers() {
        let space = |pressed, repeat, modifiers| egui::Event::Key {
            key: egui::Key::Space, physical_key: None, pressed, repeat, modifiers,
        };
        let mut input = egui::InputState::default();
        // A held key's repeats are taken without toggling again.
        input.events = vec![space(true, true, egui::Modifiers::NONE)];
        assert!(!take_space_press(&mut input));
        assert!(input.events.is_empty());
        // Shift+Space is left alone.
        input.events = vec![space(true, false, egui::Modifiers::SHIFT)];
        assert!(!take_space_press(&mut input));
        assert_eq!(input.events.len(), 1);
        input.events = vec![space(true, false, egui::Modifiers::NONE),
            space(false, false, egui::Modifiers::NONE)];
        assert!(take_space_press(&mut input));
        assert_eq!(input.events.len(), 1);
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
            options_layout(ui, &mut tab, Color32::WHITE, |ui, _| {
                assert!(ui.style().text_styles[&egui::TextStyle::Body].size >= 15.0);
                options_group_heading(ui, "Readable group");
                ui.label("Readable body");
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
        assert!(font_size("Readable body") >= 15.0);
        // The page title and a group heading are set in the bold face, not just brightened.
        for (heading, size) in [("Capture", 20.0), ("Readable group", 15.0)] {
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(shape) if shape.galley.job.text == heading
                    && shape.galley.job.sections[0].format.font_id
                        == egui::FontId::new(size, egui::FontFamily::Name(waveform::HINT_BOLD_FAMILY.into())))),
                "{heading}");
        }
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
                options_layout(ui, &mut tab, Color32::WHITE, |ui, _| {
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
                        output_cleanup: Some(30),
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
                            let output = options_layout(ui, &mut tab, Color32::WHITE, |ui, tab| match tab {
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
                mask: 0,
                profile: WavExportProfile::default(),
                force_mono: false,
                output: output.clone(),
                snapshots: snapshots.clone(),
                range: 0..32,
                marker: MarkerHistory::default().issue(0..32),
                tempo: None,
                fades: Fades::NONE,
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
                    mask: 0,
                    profile: WavExportProfile::default(),
                    force_mono,
                    output: output.clone(),
                    snapshots: snapshots.clone(),
                    marker: markers.issue(range.start as u64..range.end as u64),
                    range,
                    tempo: Some(120.0),
                    fades: Fades::NONE,
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
    fn a_clip_is_written_with_the_fades_of_its_selection() {
        let output = std::env::temp_dir().join(format!(
            "backtracking-plugin-fade-test-{}",
            std::process::id()
        ));
        let snapshots: Arc<[FrozenTrack]> =
            vec![FrozenTrack::from_interleaved(48_000, 1, 0, &[0.5; 8]).unwrap()].into();
        let saved = WavClip {
            mask: 0,
            profile: WavExportProfile::default(),
            force_mono: false,
            output: output.clone(),
            snapshots,
            range: 0..8,
            marker: MarkerHistory::default().issue(0..8),
            tempo: None,
            fades: Fades::new(2, 4),
        }
        .write()
        .unwrap();
        let wav = std::fs::read(&saved.paths[0]).unwrap();
        let data = wav.windows(4).position(|chunk| chunk == b"data").unwrap() + 8;
        let samples: Vec<_> = wav[data..]
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect();
        // Silent at both flags, full level between the ramps.
        assert_eq!(samples, [0.0, 0.25, 0.5, 0.5, 0.375, 0.25, 0.125, 0.0]);
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

    fn seven_one_four() -> WideLayout {
        let labels: Vec<String> = "FL FR FC LFE Lb Rb Ls Rs TFL TFR TBL TBR"
            .split(' ')
            .map(str::to_owned)
            .collect();
        WideLayout::new(12, &labels).unwrap()
    }

    fn row_names(wide: &WideLayout, rows: &[WideRow]) -> String {
        rows.iter()
            .map(|row| row_label(wide, *row))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn twelve_channel_lane_lays_out_l_r_and_every_speaker_with_and_without_a_sidechain() {
        let wide = seven_one_four();
        let speakers = "L R FL FR FC LFE Lb Rb Ls Rs TFL TFR TBL TBR";
        for sidechain in [false, true] {
            let lanes = wide_lanes(&wide, sidechain);
            let rows = 14 + usize::from(sidechain);
            assert_eq!(lanes[0].rows, 14);
            assert!(lanes[0].stacked);
            if sidechain {
                assert_eq!((lanes[1].rows, lanes[1].stacked), (1, false));
            }
            // The editor asks for room to give every row the comfortable height.
            let comfortable = stack::comfortable_height(&lanes).unwrap();
            assert_eq!(comfortable, rows as f32 * stack::COMFORTABLE_ROW_HEIGHT);
            let boxes = stack::lay_out(comfortable, &lanes, &[0, 0]);
            assert!(!boxes[0].scrolls());
            let all = rows_on_screen(&wide, &boxes);
            let expected = if sidechain {
                format!("{speakers} B")
            } else {
                speakers.to_owned()
            };
            assert_eq!(row_names(&wide, &all), expected);
            assert_eq!(
                all[..3],
                [
                    WideRow::Pair(PairSide::Left),
                    WideRow::Pair(PairSide::Right),
                    WideRow::Channel(0)
                ]
            );
            assert_eq!(all.last() == Some(&WideRow::Sidechain), sidechain);
            // A plot of 140 points shows ten rows of 14; the sidechain keeps one of them.
            let boxes = stack::lay_out(140.0, &lanes, &[0, 0]);
            assert!(boxes[0].scrolls());
            assert_eq!(boxes[0].row_height, stack::MIN_ROW_HEIGHT);
            assert_eq!(boxes[0].shown, if sidechain { 9 } else { 10 });
            if sidechain {
                assert_eq!((boxes[1].top, boxes[1].shown), (9.0 * 14.0, 1));
            }
            let shown = rows_on_screen(&wide, &boxes);
            assert_eq!(shown.first(), Some(&WideRow::Pair(PairSide::Left)));
            assert_eq!(shown.last() == Some(&WideRow::Sidechain), sidechain);
            // Scrolling past the end stops at the last speaker; the sidechain stays.
            let scrolled = stack::lay_out(140.0, &lanes, &[99, 0]);
            assert_eq!(scrolled[0].first, 14 - scrolled[0].shown);
            let names = row_names(&wide, &rows_on_screen(&wide, &scrolled));
            assert!(
                names.starts_with(if sidechain { "LFE " } else { "FC " }),
                "{names}"
            );
            assert!(
                names.ends_with(if sidechain { "TBR B" } else { "TBR" }),
                "{names}"
            );
        }
        // Unnamed channels, as under CLAP, are numbered.
        let unnamed = WideLayout::new(12, &[]).unwrap();
        assert_eq!(row_label(&unnamed, WideRow::Channel(11)), "12");
        assert_eq!(
            row_tooltip(&unnamed, WideRow::Channel(11)).unwrap(),
            "Channel 12 of 12: Channel 12"
        );
        assert_eq!(
            row_tooltip(&wide, WideRow::Channel(8)).unwrap(),
            "Channel 9 of 12: TFL"
        );
        assert_eq!(
            row_tooltip(&wide, WideRow::Pair(PairSide::Left)).unwrap(),
            "L: the left side of all channels mixed to stereo"
        );
        assert_eq!(row_tooltip(&wide, WideRow::Sidechain), None);
        // Mono and stereo ask for no room and have no rows.
        assert_eq!(
            stack::comfortable_height(&[stack::LaneRows {
                rows: 2,
                stacked: false
            }]),
            None
        );
        assert!(WideLayout::new(2, &["L".into(), "R".into()]).is_none());
    }

    #[test]
    fn editor_grows_once_for_wide_rows_and_shrinks_back_unless_resized_meanwhile() {
        let mut sizing = EditorSizing::default();
        let current = Vec2::new(640.0, 212.0);
        // Nothing to do for a stereo capture, or while busy.
        assert_eq!(sizing.fit_rows(current, None, 140.0, false), None);
        assert_eq!(sizing.fit_rows(current, Some(280.0), 140.0, true), None);
        assert_eq!(
            sizing.fit_rows(current, Some(280.0), 140.0, false),
            Some(Vec2::new(640.0, 352.0))
        );
        // Once grown it does not grow again, even when the host gave less.
        assert_eq!(
            sizing.fit_rows(Vec2::new(640.0, 300.0), Some(280.0), 228.0, false),
            None
        );
        assert_eq!(
            sizing.fit_rows(Vec2::new(640.0, 352.0), None, 280.0, false),
            Some(current)
        );
        // A window the user resized after it grew stays as it is.
        assert_eq!(
            sizing.fit_rows(current, Some(280.0), 140.0, false),
            Some(Vec2::new(640.0, 352.0))
        );
        assert_eq!(
            sizing.fit_rows(Vec2::new(640.0, 400.0), None, 328.0, false),
            None
        );
        // The growth stops at MAX_GROWN_HEIGHT and never shrinks a tall editor.
        let mut sizing = EditorSizing::default();
        assert_eq!(
            sizing.fit_rows(Vec2::new(640.0, 500.0), Some(300.0), 140.0, false),
            Some(Vec2::new(640.0, MAX_GROWN_HEIGHT))
        );
        let mut sizing = EditorSizing::default();
        assert_eq!(
            sizing.fit_rows(Vec2::new(640.0, 600.0), Some(300.0), 528.0, false),
            None
        );
    }

    #[test]
    fn clearing_a_wide_capture_keeps_the_grown_editor_until_the_session_answers() {
        let session = Arc::new(Session::new(crate::settings::SettingsStore::default()));
        let editor = EguiEditorState::from_size(LogicalSize::new(640.0, 324.0), 1.0);
        let mut capture = CaptureUi::new(session, editor);
        capture.timeline = WaveformTimeline {
            frames: 0..48_000,
            sample_rate: 48_000,
            channels: 12,
            wide: Some(seven_one_four()),
            retention_seconds: 20.0,
            ..WaveformTimeline::default()
        };
        capture.editor_sizing.grown = Some((212.0, 324.0));
        let context = egui::Context::default();
        let resizes = |capture: &mut CaptureUi| {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 324.0))),
                    ..Default::default()
                },
                |ui| capture.apply_waveform_size(ui, 280.0, false),
            );
            output.textures_delta.clear();
            output
                .viewport_output
                .values()
                .flat_map(|viewport| viewport.commands.iter())
                .filter_map(|command| match command {
                    egui::ViewportCommand::InnerSize(size) => Some(*size),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        // Clear swaps in an empty timeline of the next epoch. A frame that cannot read the
        // session yet keeps the window and its memory of the height before it grew.
        capture.sync_timeline(capture.timeline.epoch + 1);
        assert_eq!(resizes(&mut capture), Vec::<Vec2>::new());
        assert_eq!(capture.editor_sizing.grown, Some((212.0, 324.0)));
        // A capture that answers as stereo has really lost its rows: back to the old height.
        capture.timeline = WaveformTimeline {
            epoch: capture.timeline.epoch,
            frames: 0..48_000,
            sample_rate: 48_000,
            channels: 2,
            ..WaveformTimeline::default()
        };
        assert_eq!(resizes(&mut capture), [Vec2::new(640.0, 212.0)]);
    }

    fn rendered_lane(
        wide: &WideLayout,
        sidechain: bool,
        height: f32,
        first_row: usize,
    ) -> Vec<(Pos2, String)> {
        let session = Arc::new(Session::new(crate::settings::SettingsStore::default()));
        let editor = EguiEditorState::from_size(LogicalSize::new(640.0, 212.0), 1.0);
        let mut capture = CaptureUi::new(session, editor);
        capture.timeline = WaveformTimeline {
            frames: 0..48_000,
            sample_rate: 48_000,
            channels: 12,
            wide: Some(wide.clone()),
            b_channels: sidechain.then_some(2),
            retention_seconds: 20.0,
            ..WaveformTimeline::default()
        };
        capture.peak_frames = 0..48_000;
        capture.displayed_zoomed = true;
        capture.first_row = first_row;
        capture.rows = wide
            .rows()
            .chain(sidechain.then_some(WideRow::Sidechain))
            .collect();
        capture.row_peaks = capture
            .rows
            .iter()
            .map(|_| WaveformPeaks {
                minimum: vec![-0.5; 4],
                maximum: vec![0.5; 4],
            })
            .collect();
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(400.0, height + 16.0));
        let context = egui::Context::default();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0))),
                ..Default::default()
            },
            |ui| capture.draw_waveform(ui, rect, 1.0, false, &SessionSettings::default()),
        );
        output.textures_delta.clear();
        let mut labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.pos.x < rect.left() + stack::LABEL_GUTTER => {
                    Some((text.pos, text.galley.text().to_owned()))
                }
                _ => None,
            })
            .collect();
        labels.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
        output.drop_without_applying_deltas();
        labels
    }

    #[test]
    fn rendered_twelve_channel_lane_names_each_row_in_the_gutter() {
        let wide = seven_one_four();
        let names = |labels: &[(Pos2, String)]| {
            labels
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let speakers = "L R FL FR FC LFE Lb Rb Ls Rs TFL TFR TBL TBR";
        let labels = rendered_lane(&wide, false, 280.0, 0);
        assert_eq!(names(&labels), speakers);
        // Rows are 20 points apart and the first sits in the middle of the first row.
        assert!(
            labels
                .windows(2)
                .all(|pair| (pair[1].0.y - pair[0].0.y - 20.0).abs() < 1.0)
        );
        assert!(labels.iter().all(|(position, _)| position.x == 11.0));
        assert_eq!(
            names(&rendered_lane(&wide, true, 300.0, 0)),
            format!("{speakers} B")
        );
        // A short plot shows the top rows, and scrolled the last, with the sidechain below.
        assert_eq!(
            names(&rendered_lane(&wide, true, 140.0, 0)),
            "L R FL FR FC LFE Lb Rb Ls B"
        );
        assert_eq!(
            names(&rendered_lane(&wide, true, 140.0, 5)),
            "LFE Lb Rb Ls Rs TFL TFR TBL TBR B"
        );
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

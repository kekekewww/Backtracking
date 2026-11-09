use std::collections::VecDeque;
use std::io::{self, Cursor};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::settings::{MeterMode, SessionSettings, TRANSPARENCY_LEVELS};

const BINS: usize = 512;
const REFRESH: Duration = Duration::from_millis(80);
const RECORDING_BLINK: Duration = Duration::from_millis(500);
const DURATIONS: [DurationChoice; 3] = [
    DurationChoice::Seconds10,
    DurationChoice::Seconds20,
    DurationChoice::Seconds30,
];
const MINT: Color32 = Color32::from_rgb(103, 231, 181);
const INVERTED_MINT: Color32 = Color32::from_rgb(152, 24, 74);
const SETTINGS_POPUP_ID: &str = "backtracking-settings-popup";
const TOOLBAR_CONTROL_HEIGHT: f32 = 25.0;
const MIN_WAVEFORM_HEIGHT: f32 = 60.0;
const PRODUCT_ICON: &[u8] = include_bytes!("../../../assets/backtracking-icon.png");

pub type CaptureEditor = EguiEditor<CaptureUi>;

pub(crate) fn create(session: Arc<Session>) -> Option<CaptureEditor> {
    create_egui_editor(
        EguiEditorState::from_size(LogicalSize::new(640.0, 212.0), 1.0),
        RepaintNotifier::new(),
        EguiNiceSettings::new().with_tile("Backtracking"),
        CaptureUi {
            session,
            product_icon: None,
            minimum: [0.0; BINS],
            maximum: [0.0; BINS],
            seconds: 0.0,
            next_refresh: Instant::now(),
            recording_since: None,
            displayed_settings: None,
            context_menu_position: None,
            full_waveform_height: None,
            frozen: None,
            selection: None,
            selection_ready_for_drag: false,
            gesture: None,
            export: None,
            pending_exports: VecDeque::new(),
            exported: None,
            revision: 0,
            native_dragging: Arc::new(AtomicBool::new(false)),
            message: String::new(),
        },
    )
}

pub struct CaptureUi {
    session: Arc<Session>,
    product_icon: Option<TextureHandle>,
    minimum: [f32; BINS],
    maximum: [f32; BINS],
    seconds: f32,
    next_refresh: Instant,
    recording_since: Option<Instant>,
    displayed_settings: Option<SessionSettings>,
    context_menu_position: Option<Pos2>,
    full_waveform_height: Option<f32>,
    frozen: Option<Arc<Snapshot>>,
    selection: Option<Range<f32>>,
    selection_ready_for_drag: bool,
    gesture: Option<Gesture>,
    export: Option<ExportJob>,
    pending_exports: VecDeque<ExportRequest>,
    exported: Option<ExportedWav>,
    revision: u64,
    native_dragging: Arc<AtomicBool>,
    message: String,
}

enum Gesture {
    Select {
        anchor: f32,
        current: f32,
    },
    Drag {
        origin: Pos2,
    },
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    SaveOnly,
}

enum FolderAction {
    Select,
    Open,
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

struct ExportJob {
    revision: u64,
    profile: WavExportProfile,
    worker: JoinHandle<io::Result<PathBuf>>,
}

struct ExportRequest {
    revision: u64,
    profile: WavExportProfile,
    immediate: bool,
    output: PathBuf,
    snapshot: Arc<Snapshot>,
    range: Range<usize>,
}

impl ExportRequest {
    fn write(self) -> io::Result<PathBuf> {
        let samples = self
            .snapshot
            .frames(self.range)
            .ok_or_else(|| io::Error::other("invalid audio selection"))?;
        export_wav_with_profile(
            &self.output,
            self.snapshot.sample_rate,
            self.snapshot.channels,
            samples,
            self.profile,
        )?
        .canonicalize()
    }
}

struct ExportedWav {
    profile: WavExportProfile,
    path: PathBuf,
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
        _: &mut Frame,
    ) -> Result<(), nice_plug_egui::baseview::HandlerError> {
        self.product_icon = product_icon(&context);
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
        let mut settings = self.session.settings();
        if let Some(previous) = &self.displayed_settings {
            let retention_changed = previous.retention != settings.retention;
            let output_changed = previous.output_dir != settings.output_dir;
            let format_changed = previous.wav_profile != settings.wav_profile;
            if retention_changed {
                self.reset_frozen();
            } else if output_changed || format_changed {
                self.invalidate_export();
            }
            if retention_changed || output_changed || format_changed {
                self.message.clear();
                if self.gesture.take().is_some() {
                    self.session.end_waveform_gesture();
                }
            }
        }
        let mut paused = self.session.is_paused();
        self.collect_export();
        self.refresh_waveform(paused, settings.idle_resume);

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
        let context_at = ui.input(|input| {
            if can_toggle && input.pointer.secondary_clicked() {
                input
                    .pointer
                    .interact_pos()
                    .filter(|position| canvas.contains(*position))
            } else {
                None
            }
        });
        let controls_enabled = can_toggle
            && context_at.is_none()
            && !egui::Popup::is_id_open(ui.ctx(), egui::Id::unique(SETTINGS_POPUP_ID));
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
        let settings_open = self.settings_menu(
            frame,
            &mut settings,
            paused,
            gear.as_ref(),
            &response,
            context_at,
        );
        let configured_seconds = settings.retention.seconds() as f32;
        self.interact_waveform(
            ui,
            frame,
            rect,
            configured_seconds,
            settings.idle_resume,
            !settings_open,
        );
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
        self.draw_waveform(
            ui,
            draw_rect,
            configured_seconds,
            !settings_open,
            settings.panel_transparency,
            waveform_only,
        );
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
        ui.ctx()
            .request_repaint_after(if self.gesture.is_some() || self.export.is_some() {
                Duration::from_millis(16)
            } else {
                REFRESH
            });
    }

    fn editor_closed(&mut self) {
        self.gesture = None;
        self.recording_since = None;
        self.context_menu_position = None;
        if !self.native_dragging.load(Ordering::Acquire) {
            self.session.end_waveform_gesture();
        }
        if self.pending_exports.iter().any(|request| request.immediate) {
            self.finish_exports();
        }
    }
}

impl Drop for CaptureUi {
    fn drop(&mut self) {
        if !self.native_dragging.load(Ordering::Acquire) {
            self.session.end_waveform_gesture();
        }
        self.finish_exports();
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

    fn settings_menu(
        &mut self,
        frame: &mut Frame,
        settings: &mut SessionSettings,
        paused: bool,
        gear: Option<&egui::Response>,
        waveform: &egui::Response,
        context_at: Option<Pos2>,
    ) -> bool {
        let ctx = &waveform.ctx;
        let popup_id = egui::Id::unique(SETTINGS_POPUP_ID);
        let was_open = egui::Popup::is_id_open(ctx, popup_id);
        let open = if let Some(position) = context_at {
            self.context_menu_position = Some(position);
            Some(egui::SetOpenCommand::Bool(true))
        } else if gear.is_some_and(egui::Response::clicked) {
            let from_context = self.context_menu_position.take().is_some();
            Some(egui::SetOpenCommand::Bool(!was_open || from_context))
        } else {
            None
        };
        let popup = egui::Popup::menu(gear.unwrap_or(waveform))
            .id(popup_id)
            .accessible_name("Settings")
            .open_memory(open)
            .gap(4.0);
        let popup = if let Some(position) = self.context_menu_position {
            popup.at_position(position)
        } else if gear.is_some() {
            popup.align(egui::RectAlign::BOTTOM_END)
        } else {
            egui::Popup::close_id(ctx, popup_id);
            return was_open;
        };
        let close_behavior = if context_at.is_some() {
            egui::PopupCloseBehavior::IgnoreClicks
        } else {
            egui::PopupCloseBehavior::CloseOnClickOutside
        };
        let menu_content_height = (ctx.viewport_rect().height() - 68.0).max(60.0);
        let menu = popup.close_behavior(close_behavior).show(|ui| {
            let mut retention = settings.retention;
            let mut idle_resume = settings.idle_resume;
            let mut panel_transparency = settings.panel_transparency;
            let mut meter_mode = settings.meter_mode;
            let mut wav_profile = settings.wav_profile;
            let mut waveform_only = settings.show_waveform_only;
            egui::ScrollArea::vertical()
                .max_height(menu_content_height)
                .show(ui, |ui| {
                    if waveform_only {
                        let label = if paused { "Play" } else { "Pause" };
                        if ui
                            .add_enabled(self.can_toggle_capture(), egui::Button::new(label))
                            .clicked()
                        {
                            self.toggle_capture(ui, paused);
                            ui.close();
                        }
                    }
                    ui.checkbox(&mut waveform_only, "Show waveform only");
                    ui.separator();
                    egui::Grid::new("capture-settings")
                        .spacing([12.0, 3.0])
                        .show(ui, |ui| {
                            ui.label("History");
                            ui.horizontal(|ui| {
                                for duration in DURATIONS {
                                    ui.selectable_value(
                                        &mut retention,
                                        duration,
                                        format!("{} s", duration.seconds()),
                                    );
                                }
                            });
                            ui.end_row();
                            ui.label("Panel transparency").on_hover_text(
                                "Fades the panel fill. The DAW may keep the editor backing opaque.",
                            );
                            ui.horizontal(|ui| {
                                for percent in TRANSPARENCY_LEVELS {
                                    ui.selectable_value(
                                        &mut panel_transparency,
                                        percent,
                                        format!("{percent}%"),
                                    );
                                }
                            });
                            ui.end_row();
                            ui.label("Meter");
                            ui.horizontal(|ui| {
                                for mode in MeterMode::ALL {
                                    ui.selectable_value(&mut meter_mode, mode, mode.label());
                                }
                            });
                            ui.end_row();
                            ui.label("Auto-resume");
                            ui.horizontal(|ui| {
                                for duration in AutoResumeDelay::ALL {
                                    ui.selectable_value(
                                        &mut idle_resume,
                                        duration,
                                        format!("{} s", duration.seconds()),
                                    );
                                }
                            });
                            ui.end_row();
                            wav_format_controls(ui, &mut wav_profile);
                        });
                });
            if retention != settings.retention {
                self.session.set_retention(retention);
                settings.retention = retention;
                self.reset_frozen();
                self.message.clear();
                self.refresh_waveform(paused, settings.idle_resume);
            }
            if idle_resume != settings.idle_resume {
                self.session.set_idle_resume(idle_resume);
                settings.idle_resume = idle_resume;
            }
            if panel_transparency != settings.panel_transparency {
                self.session.set_panel_transparency(panel_transparency);
                settings.panel_transparency = panel_transparency;
            }
            if meter_mode != settings.meter_mode {
                self.session.set_meter_mode(meter_mode);
                settings.meter_mode = meter_mode;
            }
            if wav_profile != settings.wav_profile {
                self.session.set_wav_profile(wav_profile);
                settings.wav_profile = wav_profile;
                self.invalidate_export();
                self.message.clear();
            }
            if waveform_only != settings.show_waveform_only {
                self.session.set_show_waveform_only(waveform_only);
                settings.show_waveform_only = waveform_only;
            }
            ui.separator();
            let action = ui
                .horizontal(|ui| {
                    let folder = settings.output_dir.display().to_string();
                    let select = ui
                        .button("Select save folder")
                        .on_hover_text(&folder)
                        .clicked();
                    let open = ui
                        .button("Go to save folder")
                        .on_hover_text(folder)
                        .clicked();
                    if select {
                        Some(FolderAction::Select)
                    } else if open {
                        Some(FolderAction::Open)
                    } else {
                        None
                    }
                })
                .inner;
            if action.is_some() {
                ui.close();
            }
            action
        });
        if !egui::Popup::is_id_open(ctx, popup_id) {
            self.context_menu_position = None;
        }
        let shown = menu.is_some();
        match menu.and_then(|menu| menu.inner) {
            Some(FolderAction::Select) => {
                self.session.set_gesture_active(true);
                let selection = rfd::FileDialog::new()
                    .set_title("Select save folder")
                    .set_parent(frame.baseview_window())
                    .pick_folder();
                self.session.set_gesture_active(false);
                if let Some(folder) = selection {
                    self.session.set_output_dir(folder.clone());
                    self.invalidate_export();
                    self.message = format!("Output: {}", folder.display());
                    settings.output_dir = folder;
                }
            }
            Some(FolderAction::Open) => {
                if let Err(error) = std::fs::create_dir_all(&settings.output_dir)
                    .and_then(|()| open::that_detached(&settings.output_dir))
                {
                    self.message = format!("Could not open save folder: {error}");
                    if settings.show_waveform_only {
                        self.session.set_gesture_active(true);
                        rfd::MessageDialog::new()
                            .set_level(rfd::MessageLevel::Warning)
                            .set_title("Backtracking")
                            .set_description(&self.message)
                            .set_parent(frame.baseview_window())
                            .show();
                        self.session.set_gesture_active(false);
                    }
                }
            }
            None => {}
        }
        // A waveform click that dismisses the popup must not also start an export.
        was_open || shown
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
        self.invalidate_export();
    }

    fn clear_frozen_preview(&mut self) {
        self.frozen = None;
        self.selection = None;
        self.selection_ready_for_drag = false;
        self.next_refresh = Instant::now();
    }

    fn invalidate_export(&mut self) {
        self.next_export_revision();
        self.pending_exports.retain(|request| request.immediate);
    }

    fn next_export_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.exported = None;
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
        panel_transparency: u8,
        waveform_only: bool,
    ) {
        let painter = ui.painter_at(rect);
        let inner = rect.shrink(8.0);
        let middle = inner.center().y;
        let extent = inner.height().max(0.0) * 0.47;
        if !waveform_only {
            let opacity = ((100 - u16::from(panel_transparency)) * 255 / 100) as u8;
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
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                Some(Gesture::SaveOnly) => true,
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
            painter.rect_filled(audio_rect, 2.0, Color32::from_rgb(31, 79, 64));
        }

        for (bin, (&low, &high)) in self.minimum.iter().zip(&self.maximum).enumerate() {
            let fraction = (bin as f32 + 0.5) / BINS as f32;
            let x = audio_rect.left() + width * fraction;
            let top = middle - high.clamp(-1.0, 1.0) * extent;
            let bottom = middle - low.clamp(-1.0, 1.0) * extent;
            let color = if explicit_range
                .as_ref()
                .is_some_and(|range| range.contains(&fraction))
            {
                INVERTED_MINT
            } else {
                MINT
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
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            Some(Gesture::SaveOnly) => return,
            None if down => return,
            None if modifier => "Drag to Select".into(),
            None => {
                let action = if self.exported.is_some() || self.selection.is_some() {
                    "Drag and Drop"
                } else {
                    "Click"
                };
                let selection = if cfg!(target_os = "macos") {
                    "Option + Click"
                } else {
                    "Ctrl + Click"
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
        rect: Rect,
        seconds: f32,
        idle_resume: AutoResumeDelay,
        allow_new_gesture: bool,
    ) {
        let (position, pressed, down, selecting) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.button_pressed(PointerButton::Primary),
                input.pointer.button_down(PointerButton::Primary),
                input.modifiers.alt || input.modifiers.ctrl,
            )
        });
        if self.native_dragging.load(Ordering::Acquire) {
            return;
        }

        if allow_new_gesture
            && pressed
            && let Some(position) = position.filter(|position| rect.contains(*position))
        {
            self.session.begin_waveform_gesture();
            if idle_resume == AutoResumeDelay::Immediate {
                if selecting && self.selection_ready_for_drag {
                    self.clear_frozen_preview();
                }
                if self.frozen.is_none() {
                    self.next_export_revision();
                }
            }
            self.cache_frozen_snapshot();
            if selecting {
                let anchor = position_in_audio(self.audio_rect(rect, seconds), position.x);
                self.gesture = Some(Gesture::Select {
                    anchor,
                    current: anchor,
                });
            } else {
                self.selection_ready_for_drag = false;
                self.begin_export();
                self.gesture = Some(Gesture::Drag { origin: position });
            }
        }

        let audio_rect = self.audio_rect(rect, seconds);
        if let Some(Gesture::Select { current, .. }) = &mut self.gesture
            && let Some(position) = position
        {
            *current = position_in_audio(audio_rect, position.x);
        }

        if down {
            if let (Some(Gesture::Drag { origin }), Some(position), Some(file)) =
                (&self.gesture, position, self.exported.as_ref())
                && position.distance(*origin) >= 4.0
                && file.profile == self.session.settings().wav_profile
            {
                let origin = *origin;
                let path = file.path.clone();
                self.gesture = None;
                self.start_native_drag(frame, path, origin);
            }
        } else if let Some(gesture) = self.gesture.take() {
            if let Gesture::Select { anchor, current } = gesture {
                let range = anchor.min(current)..anchor.max(current);
                self.selection = (range.end > range.start).then_some(range);
                self.selection_ready_for_drag = self.selection.is_some();
                if idle_resume == AutoResumeDelay::Immediate {
                    self.next_export_revision();
                } else {
                    self.invalidate_export();
                }
                self.begin_export();
            }
            self.session.end_waveform_gesture();
        }
    }

    fn begin_export(&mut self) {
        let settings = self.session.settings();
        let profile = settings.wav_profile;
        if self
            .exported
            .as_ref()
            .is_some_and(|file| file.profile == profile)
            || self
                .export
                .as_ref()
                .is_some_and(|job| job.revision == self.revision && job.profile == profile)
            || self
                .pending_exports
                .iter()
                .any(|request| request.revision == self.revision && request.profile == profile)
        {
            return;
        }
        self.exported = None;
        let Some(snapshot) = self
            .frozen
            .as_ref()
            .filter(|snapshot| snapshot.frame_count() > 0)
            .cloned()
        else {
            self.message = "No captured audio to save yet.".into();
            return;
        };
        let frames = snapshot.frame_count();
        let range = self.selection.as_ref().map_or(0..frames, |range| {
            let first = (range.start * frames as f32).floor() as usize;
            let last = (range.end * frames as f32).ceil() as usize;
            first.min(frames - 1)..last.clamp(first.min(frames - 1) + 1, frames)
        });
        let request = ExportRequest {
            revision: self.revision,
            profile,
            immediate: settings.idle_resume == AutoResumeDelay::Immediate,
            output: settings.output_dir,
            snapshot,
            range,
        };
        if self.export.is_some() || !self.pending_exports.is_empty() {
            if !request.immediate {
                self.pending_exports.retain(|request| request.immediate);
            }
            self.pending_exports.push_back(request);
        } else {
            self.spawn_export(request);
        }
    }

    fn spawn_export(&mut self, request: ExportRequest) {
        let revision = request.revision;
        let profile = request.profile;
        let worker = thread::Builder::new()
            .name("backtracking-wav".into())
            .spawn(move || request.write());
        match worker {
            Ok(worker) => {
                self.export = Some(ExportJob {
                    revision,
                    profile,
                    worker,
                });
                if revision == self.revision {
                    self.message = "Writing WAV…".into();
                }
            }
            Err(error) if revision == self.revision => {
                self.message = format!("Could not start export: {error}");
            }
            Err(_) => {}
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
            if job.revision == self.revision && job.profile == self.session.settings().wav_profile {
                match result {
                    Ok(Ok(path)) => {
                        self.message = format!("Saved {}", path.display());
                        self.exported = Some(ExportedWav {
                            profile: job.profile,
                            path,
                        });
                    }
                    Ok(Err(error)) => self.message = format!("Could not save WAV: {error}"),
                    Err(_) => self.message = "WAV export worker stopped unexpectedly.".into(),
                }
            }
        }
        if let Some(request) = self.pending_exports.pop_front() {
            self.spawn_export(request);
        }
    }

    fn finish_exports(&mut self) {
        if let Some(job) = self.export.take() {
            // Do not leave plugin code running on a detached worker after library unload.
            let _ = job.worker.join();
        }
        for request in self
            .pending_exports
            .drain(..)
            .filter(|request| request.immediate)
        {
            let _ = request.write();
        }
    }

    #[cfg(target_os = "macos")]
    fn start_native_drag(&mut self, frame: &Frame, path: PathBuf, origin: Pos2) {
        self.native_dragging.store(true, Ordering::Release);
        let completion = mac_drag::Completion {
            session: Arc::clone(&self.session),
            dragging: Arc::clone(&self.native_dragging),
        };
        if let Err(error) = mac_drag::start(frame, &path, origin, completion) {
            self.message = format!("Could not start file drag: {error}");
        }
    }

    #[cfg(target_os = "windows")]
    fn start_native_drag(&mut self, frame: &Frame, path: PathBuf, _origin: Pos2) {
        let session = Arc::clone(&self.session);
        let dragging = Arc::clone(&self.native_dragging);
        dragging.store(true, Ordering::Release);
        let result = drag::start_drag(
            frame.baseview_window(),
            drag::DragItem::Files(vec![path]),
            drag::Image::Raw(DRAG_ICON.to_vec()),
            move |_, _| {
                session.end_waveform_gesture();
                dragging.store(false, Ordering::Release);
            },
            drag::Options::default(),
        );
        if let Err(error) = result {
            self.native_dragging.store(false, Ordering::Release);
            self.session.end_waveform_gesture();
            self.message = format!("Could not start file drag: {error}");
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn start_native_drag(&mut self, _frame: &Frame, path: PathBuf, _origin: Pos2) {
        self.gesture = Some(Gesture::SaveOnly);
        self.message = format!(
            "WAV saved. Drag the file from its folder: {}",
            path.display()
        );
    }
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
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Settings"));
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
    response.on_hover_text("Settings")
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

// A small RGBA PNG waveform preview for the native file-drag cursor.
#[cfg(any(target_os = "macos", target_os = "windows"))]
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
    use std::path::Path;
    use std::ptr::{NonNull, null_mut};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use objc2::rc::Retained;
    use objc2::runtime::{
        AnyClass, AnyObject, AnyProtocol, ClassBuilder, NSObject, ProtocolObject, Sel,
    };
    use objc2::{AnyThread, ClassType, MainThreadMarker, msg_send, sel};
    use objc2_app_kit::{
        NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSEvent,
        NSEventModifierFlags, NSEventType, NSImage, NSView,
    };
    use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSURL};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::{DRAG_ICON, Frame, Pos2, Session};

    static NEXT_CLASS: AtomicUsize = AtomicUsize::new(0);
    const COMPLETION: &std::ffi::CStr = c"backtrackingCompletion";

    pub(super) struct Completion {
        pub(super) session: Arc<Session>,
        pub(super) dragging: Arc<AtomicBool>,
    }

    impl Drop for Completion {
        fn drop(&mut self) {
            self.session.end_waveform_gesture();
            self.dragging.store(false, Ordering::Release);
        }
    }

    pub(super) fn start(
        frame: &Frame,
        path: &Path,
        origin: Pos2,
        completion: Completion,
    ) -> Result<(), &'static str> {
        let _main = MainThreadMarker::new().ok_or("AppKit file drag requires the main thread")?;
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
        let location = window.mouseLocationOutsideOfEventStream();
        let timestamp = window.currentEvent().map_or(0.0, |event| event.timestamp());
        let press =
            view.convertPoint_toView(NSPoint::new(f64::from(origin.x), f64::from(origin.y)), None);
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDown, press, NSEventModifierFlags::empty(), timestamp,
            window.windowNumber(), None, 0, 1, 1.0,
        ).ok_or("could not create the drag event")?;

        let native_path =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| "invalid WAV path")?;
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
        let source = make_source(completion)?;

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
        _location: NSPoint,
        _operation: NSDragOperation,
    ) {
        release_completion(source, source.class());
    }

    extern "C-unwind" fn dealloc(source: &mut AnyObject, _selector: Sel) {
        let class = source.class();
        release_completion(source, class);
        // This dynamically registered class has exactly one instance and is never subclassed.
        unsafe {
            let _: () = msg_send![super(source, NSObject::class()), dealloc];
            dispose(class);
        }
    }

    fn release_completion(source: &AnyObject, class: &AnyClass) {
        if let Some(ivar) = class.instance_variable(COMPLETION) {
            // The callback and dealloc both run on the main thread. Taking the pointer makes
            // completion idempotent for normal drops, cancellation, and failed drag starts.
            let pointer = unsafe { ivar.load_ptr::<*mut c_void>(source).replace(null_mut()) };
            if !pointer.is_null() {
                drop(unsafe { Box::from_raw(pointer.cast::<Completion>()) });
            }
        }
    }

    unsafe fn dispose(class: &AnyClass) {
        // Called after the sole source instance has been deallocated, or on allocation failure.
        unsafe { objc2::ffi::objc_disposeClassPair(std::ptr::from_ref(class).cast_mut()) };
    }
}

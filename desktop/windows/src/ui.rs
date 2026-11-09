use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{DurationChoice, MeterReadings, Snapshot, export_wav, waveform};
use eframe::egui::{
    self, Color32, CursorIcon, PointerButton, Pos2, Rect, ResizeDirection, Stroke, Vec2,
    ViewportCommand,
};

use crate::controller::Controller;
use crate::preferences::{Key, MeterMode, Preferences, Settings};

const BINS: usize = 384;
const REFRESH: Duration = Duration::from_millis(80);
const DURATIONS: [DurationChoice; 3] = [
    DurationChoice::Seconds10,
    DurationChoice::Seconds20,
    DurationChoice::Seconds30,
];
const TRANSPARENCIES: [u8; 5] = [0, 25, 50, 75, 100];
const MINT: Color32 = Color32::from_rgb(103, 231, 181);
const RECORDING_LIT: Color32 = Color32::from_rgb(249, 78, 88);
const RECORDING_DIM: Color32 = Color32::from_rgb(103, 44, 48);
const WAITING_LED: Color32 = Color32::from_rgb(76, 86, 88);
const PAUSED_LED: Color32 = Color32::from_rgb(54, 57, 60);
const ERROR_LED: Color32 = Color32::from_rgb(246, 196, 118);
const RESIZE_EDGE: f32 = 7.0;
const RESIZE_CORNER: f32 = 14.0;

pub(crate) fn run(controller: Controller) -> eframe::Result<()> {
    let preferences = Preferences::for_current_user();
    let settings = preferences.load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([610.0, 144.0])
            .with_min_inner_size([510.0, 132.0])
            .with_decorations(false)
            .with_resizable(true)
            .with_transparent(true)
            .with_has_shadow(false)
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
            configure_style(&context.egui_ctx);
            Ok(Box::new(Recorder::new(controller, preferences, settings)))
        }),
    )
}

fn configure_style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
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

struct Recorder {
    controller: Controller,
    preferences: Preferences,
    output: PathBuf,
    pinned: bool,
    idle_resume: Option<DurationChoice>,
    transparency: u8,
    meter_mode: MeterMode,
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
    window_command: Option<ViewportCommand>,
    export: Option<ExportJob>,
    export_again: bool,
    clip_revision: u64,
    exported: Option<PathBuf>,
    message: String,
}

enum Gesture {
    Select { anchor: f32, cursor: f32 },
    Export { origin: Pos2 },
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
    worker: JoinHandle<io::Result<PathBuf>>,
}

impl Recorder {
    fn new(mut controller: Controller, preferences: Preferences, settings: Settings) -> Self {
        controller.set_retention(settings.history);
        Self {
            controller,
            preferences,
            output: settings.output,
            pinned: settings.pinned,
            idle_resume: settings.idle_resume,
            transparency: settings.transparency,
            meter_mode: settings.meter_mode,
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
            window_command: None,
            export: None,
            export_again: false,
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
        self.snapshot = None;
        self.selection = None;
        self.live_revision = None;
        self.next_refresh = Instant::now();
        self.invalidate_export();
    }

    fn clear_capture(&mut self) {
        self.controller.clear();
        self.clear_snapshot();
        self.message.clear();
        self.last_interaction = Instant::now();
    }

    fn background_color(&self, color: Color32) -> Color32 {
        let alpha = ((100 - u16::from(self.transparency)) * 255 + 50) / 100;
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha as u8)
    }

    fn invalidate_export(&mut self) {
        self.clip_revision = self.clip_revision.wrapping_add(1);
        self.exported = None;
        self.export_again = false;
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

    fn controls(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) -> bool {
        ui.horizontal(|ui| {
            ui.heading("Backtracking");
            self.transport_button(ui);
            self.recording_indicator(ui);
            self.clear_button(ui);
            self.pin_button(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let settings_open = self.settings_button(ui, frame);
                self.meter_readout(ui);
                settings_open
            })
            .inner
        })
        .inner
    }

    fn transport_button(&mut self, ui: &mut egui::Ui) {
        let can_toggle = self.gesture.is_none();
        let response = ui.add_enabled(can_toggle, icon_button());
        if response.clicked() {
            if self.controller.is_paused() {
                self.resume();
            } else {
                self.freeze();
            }
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

    fn clear_button(&mut self, ui: &mut egui::Ui) {
        let enabled = self.gesture.is_none();
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

    fn pin_button(&mut self, ui: &mut egui::Ui) {
        let response = ui.add(icon_button());
        if response.clicked() {
            self.pinned = !self.pinned;
            self.save_preference(Key::AlwaysOnTop, if self.pinned { "1" } else { "0" });
            let level = if self.pinned {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::WindowLevel(level));
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::CheckBox, true, self.pinned, "Keep on top")
        });
        let response = response.on_hover_text(if self.pinned {
            "Disable keep on top"
        } else {
            "Keep on top"
        });
        let center = response.rect.center();
        let color = if self.pinned {
            MINT
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

    fn settings_button(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) -> bool {
        let response = ui.add(icon_button()).on_hover_text("Settings");
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Settings"));
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

        let popup = egui::Popup::menu(&response)
            .accessible_name("Settings")
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .gap(4.0);
        let was_open = popup.is_open();
        let settings_height = (ui.ctx().content_rect().height() - 59.0).max(48.0);
        let shown = popup.show(|ui| {
            ui.set_width(292.0);
            egui::ScrollArea::vertical()
                .id_salt("settings-options")
                .max_height(settings_height)
                .show(ui, |ui| {
                    ui.spacing_mut().interact_size.y = 15.0;
                    ui.spacing_mut().button_padding.y = 0.5;
                    egui::Grid::new("recording-settings")
                        .spacing([9.0, 1.0])
                        .show(ui, |ui| {
                            ui.label("History");
                            ui.horizontal(|ui| {
                                let mut retention = self.controller.retention();
                                for value in DURATIONS {
                                    ui.selectable_value(
                                        &mut retention,
                                        value,
                                        format!("{} s", value.seconds()),
                                    );
                                }
                                if retention != self.controller.retention() {
                                    self.controller.set_retention(retention);
                                    self.clear_snapshot();
                                    self.save_preference(
                                        Key::History,
                                        retention.seconds().to_string(),
                                    );
                                }
                            });
                            ui.end_row();
                            ui.label("Auto-resume");
                            ui.horizontal(|ui| {
                                let before = self.idle_resume;
                                ui.selectable_value(&mut self.idle_resume, None, "Off");
                                for value in DURATIONS {
                                    ui.selectable_value(
                                        &mut self.idle_resume,
                                        Some(value),
                                        format!("{} s", value.seconds()),
                                    );
                                }
                                if self.idle_resume != before {
                                    let seconds =
                                        self.idle_resume.map_or(0, DurationChoice::seconds);
                                    self.save_preference(Key::IdleResume, seconds.to_string());
                                }
                            });
                            ui.end_row();
                            ui.label("Background")
                                .on_hover_text("Background transparency");
                            ui.horizontal(|ui| {
                                let before = self.transparency;
                                for value in TRANSPARENCIES {
                                    ui.selectable_value(
                                        &mut self.transparency,
                                        value,
                                        format!("{value}%"),
                                    );
                                }
                                if self.transparency != before {
                                    self.save_preference(
                                        Key::Transparency,
                                        self.transparency.to_string(),
                                    );
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
                });

            ui.separator();
            let (choose_project, quit) = ui
                .horizontal(|ui| {
                    let choose_project = ui
                        .button("Project folder…")
                        .on_hover_text(self.output.display().to_string())
                        .clicked();
                    let quit = ui.button("Quit").clicked();
                    (choose_project, quit)
                })
                .inner;
            if quit {
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            }
            if choose_project || quit {
                ui.close();
            }
            choose_project && !quit
        });

        if shown.as_ref().is_some_and(|menu| menu.inner) {
            self.controller.pause();
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Choose the Backtracking project folder")
                .set_parent(frame)
                .pick_folder()
            {
                self.output = folder.join("output");
                self.invalidate_export();
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
        }
        was_open || shown.is_some()
    }

    fn start_export(&mut self) {
        if self.exported.is_some() {
            return;
        }
        if let Some(job) = &self.export {
            self.export_again = job.revision != self.clip_revision;
            return;
        }
        let Some(snapshot) = self
            .snapshot
            .as_ref()
            .filter(|s| s.frame_count() > 0)
            .cloned()
        else {
            self.message = "No audio captured yet.".into();
            return;
        };
        let frames = snapshot.frame_count();
        let frames_to_export = self.selection.as_ref().map_or(0..frames, |selection| {
            let start = ((selection.start * frames as f32).floor() as usize).min(frames - 1);
            let end = ((selection.end * frames as f32).ceil() as usize).clamp(start + 1, frames);
            start..end
        });
        let output = self.output.clone();
        match thread::Builder::new()
            .name("backtracking-wav".into())
            .spawn(move || {
                let samples = snapshot
                    .frames(frames_to_export)
                    .ok_or_else(|| io::Error::other("invalid audio selection"))?;
                export_wav(&output, snapshot.sample_rate, snapshot.channels, samples)
            }) {
            Ok(worker) => {
                self.export = Some(ExportJob {
                    worker,
                    revision: self.clip_revision,
                });
                self.message = "Writing WAV…".into();
            }
            Err(error) => self.message = format!("Could not start export: {error}"),
        }
    }

    fn collect_export(&mut self) {
        if self
            .export
            .as_ref()
            .is_none_or(|job| !job.worker.is_finished())
        {
            return;
        }
        let Some(job) = self.export.take() else {
            return;
        };
        let result = job.worker.join();
        if job.revision == self.clip_revision {
            match result {
                Ok(Ok(path)) => {
                    self.message = format!("Saved {}", path.display());
                    self.exported = Some(path);
                }
                Ok(Err(error)) => self.message = format!("Could not save WAV: {error}"),
                Err(_) => self.message = "WAV export worker stopped unexpectedly.".into(),
            }
        }
        if std::mem::take(&mut self.export_again) {
            self.start_export();
        }
    }

    fn audio_rect(&self, rect: Rect) -> Rect {
        let inner = rect.shrink(8.0);
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
            self.freeze();
            if selecting {
                let start = fraction(self.audio_rect(rect), position.x);
                self.gesture = Some(Gesture::Select {
                    anchor: start,
                    cursor: start,
                });
            } else {
                self.start_export();
                self.gesture = Some(Gesture::Export { origin: position });
            }
        }

        let audio_rect = self.audio_rect(rect);
        if let Some(Gesture::Select { cursor, .. }) = &mut self.gesture
            && let Some(position) = position
        {
            *cursor = fraction(audio_rect, position.x);
        }

        if down {
            if let (Some(Gesture::Export { origin }), Some(position), Some(file)) =
                (&self.gesture, position, self.exported.as_ref())
                && position.distance(*origin) >= 4.0
            {
                let file = file.clone();
                self.gesture = None;
                // OLE holds the UI thread until the mouse is released.
                if let Err(error) = drag::start_drag(
                    frame,
                    drag::DragItem::Files(vec![file]),
                    drag::Image::Raw(DRAG_ICON.to_vec()),
                    |_, _| {},
                    drag::Options::default(),
                ) {
                    self.message = format!("Could not start file drag: {error}");
                }
                self.last_interaction = Instant::now();
            }
        } else if let Some(gesture) = self.gesture.take() {
            self.last_interaction = Instant::now();
            if let Gesture::Select { anchor, cursor } = gesture {
                let range = anchor.min(cursor)..anchor.max(cursor);
                if range.start < range.end {
                    self.selection = Some(range);
                    self.invalidate_export();
                    self.start_export();
                }
            }
        }
    }

    fn draw_waveform(&self, ui: &egui::Ui, rect: Rect, preview_hover: bool) {
        let painter = ui.painter_at(rect);
        // The panel already supplies the chosen alpha. Skip a second background
        // layer when translucent so the waveform area uses the same opacity.
        if self.transparency == 0 {
            painter.rect_filled(rect, 6.0, Color32::from_rgb(22, 26, 29));
        }
        let inner = rect.shrink(8.0);
        let middle = inner.center().y;
        painter.hline(
            inner.x_range(),
            middle,
            Stroke::new(1.0, Color32::from_rgb(47, 60, 64)),
        );
        let audio = self.audio_rect(rect);
        if audio.width() <= 0.0 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No captured audio",
                egui::FontId::proportional(13.0),
                Color32::GRAY,
            );
            return;
        }

        let highlight = match self.gesture {
            Some(Gesture::Select { anchor, cursor }) => {
                Some(anchor.min(cursor)..anchor.max(cursor))
            }
            Some(Gesture::Export { .. }) => Some(self.selection.clone().unwrap_or(0.0..1.0)),
            None if self.selection.is_some() => self.selection.clone(),
            None if preview_hover
                && ui.input(|input| {
                    input
                        .pointer
                        .hover_pos()
                        .is_some_and(|pos| rect.contains(pos))
                }) =>
            {
                Some(0.0..1.0)
            }
            None => None,
        };
        let x = |fraction: f32| audio.left() + fraction * audio.width();
        if let Some(range) = &highlight {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(x(range.start), audio.top()),
                    Pos2::new(x(range.end), audio.bottom()),
                ),
                2.0,
                Color32::from_rgb(31, 79, 64),
            );
        }

        let extent = inner.height() * 0.47;
        for (bin, (&low, &high)) in self.low.iter().zip(&self.high).enumerate() {
            let position = x((bin as f32 + 0.5) / BINS as f32);
            let top = middle - high.clamp(-1.0, 1.0) * extent;
            let bottom = middle - low.clamp(-1.0, 1.0) * extent;
            painter.line_segment(
                [
                    Pos2::new(position, top),
                    Pos2::new(position, bottom.max(top + 0.4)),
                ],
                Stroke::new(1.0, MINT),
            );
        }
        if let Some(range) = highlight {
            painter.text(
                audio.right_top(),
                egui::Align2::RIGHT_TOP,
                format!("{:.2} s", (range.end - range.start) * self.seconds),
                egui::FontId::monospace(11.0),
                Color32::WHITE,
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
            Color32::from_rgb(36, 47, 47),
            Stroke::new(1.0, Color32::from_rgb(67, 89, 83)),
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
        if input.viewport().minimized == Some(true) {
            return;
        }
        if input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::PointerButton { pressed: true, .. }))
        {
            self.last_interaction = Instant::now();
        }
        if self.gesture.is_some() || egui::Popup::is_any_open(ctx) {
            return;
        }
        let bounds = input.screen_rect.unwrap_or_else(|| ctx.viewport_rect());
        let viewport = input.viewport();
        if viewport.fullscreen == Some(true) {
            return;
        }
        let can_resize = viewport.maximized != Some(true);
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
            // A queued release means the button was not held when the native command would run.
            self.window_command = None;
            if pressed
                && let Some((command, _)) =
                    window_interaction(bounds, pos, modifiers.alt, can_resize)
            {
                // Keep the position but prevent the reserved press from reaching any widget.
                *event = egui::Event::PointerMoved(pos);
                self.window_command = Some(command);
            }
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(command) = self.window_command.take() {
            ctx.send_viewport_cmd(command);
        }
        self.controller.poll();
        if !self.controller.is_recording() {
            self.recording_started = None;
        }
        self.collect_export();
        if self.controller.is_paused()
            && self.gesture.is_none()
            && self.idle_resume.is_some_and(|delay| {
                self.last_interaction.elapsed() >= Duration::from_secs(u64::from(delay.seconds()))
            })
        {
            self.resume();
        }
        ctx.request_repaint_after(if self.gesture.is_some() || self.export.is_some() {
            Duration::from_millis(16)
        } else {
            REFRESH
        });
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let fill = self.background_color(ui.visuals().panel_fill);
        let frame_style = egui::Frame::central_panel(ui.style()).fill(fill);
        egui::CentralPanel::default()
            .frame(frame_style)
            .show(ui, |ui| {
                let menu_open = self.controls(ui, frame);
                let window_cursor = if !menu_open && self.gesture.is_none() {
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
                        .map(|(_, cursor)| cursor)
                    })
                } else {
                    None
                };
                self.refresh_waveform();
                let size = Vec2::new(
                    ui.available_width(),
                    (ui.available_height() - 26.0).max(46.0),
                );
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                if !menu_open {
                    self.interact_waveform(ui, frame, &response);
                }
                self.draw_waveform(ui, rect, !menu_open && window_cursor.is_none());
                let status = self.controller.error().unwrap_or(&self.message);
                if !status.is_empty() {
                    ui.add(egui::Label::new(status).truncate())
                        .on_hover_text(status);
                }
                if let Some(cursor) = window_cursor {
                    ui.ctx().set_cursor_icon(cursor);
                } else if !menu_open {
                    self.draw_action_hint(ui, &response);
                    response.on_hover_cursor(CursorIcon::Grab);
                }
            });
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(job) = self.export.take() {
            let _ = job.worker.join();
        }
    }
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
) -> Option<(ViewportCommand, CursorIcon)> {
    if !bounds.contains(pos) {
        return None;
    }
    if alt {
        return Some((ViewportCommand::StartDrag, CursorIcon::Move));
    }
    if !can_resize {
        return None;
    }

    let left = pos.x - bounds.left();
    let right = bounds.right() - pos.x;
    let top = pos.y - bounds.top();
    let bottom = bounds.bottom() - pos.y;
    let (direction, cursor) = match () {
        _ if left.min(top) <= RESIZE_EDGE && left.max(top) <= RESIZE_CORNER => {
            (ResizeDirection::NorthWest, CursorIcon::ResizeNwSe)
        }
        _ if right.min(top) <= RESIZE_EDGE && right.max(top) <= RESIZE_CORNER => {
            (ResizeDirection::NorthEast, CursorIcon::ResizeNeSw)
        }
        _ if left.min(bottom) <= RESIZE_EDGE && left.max(bottom) <= RESIZE_CORNER => {
            (ResizeDirection::SouthWest, CursorIcon::ResizeNeSw)
        }
        _ if right.min(bottom) <= RESIZE_EDGE && right.max(bottom) <= RESIZE_CORNER => {
            (ResizeDirection::SouthEast, CursorIcon::ResizeNwSe)
        }
        _ if left <= RESIZE_EDGE => (ResizeDirection::West, CursorIcon::ResizeHorizontal),
        _ if right <= RESIZE_EDGE => (ResizeDirection::East, CursorIcon::ResizeHorizontal),
        _ if top <= RESIZE_EDGE => (ResizeDirection::North, CursorIcon::ResizeVertical),
        _ if bottom <= RESIZE_EDGE => (ResizeDirection::South, CursorIcon::ResizeVertical),
        _ => return None,
    };
    Some((ViewportCommand::BeginResize(direction), cursor))
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

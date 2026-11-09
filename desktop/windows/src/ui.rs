use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{DurationChoice, Snapshot, export_wav, waveform};
use eframe::egui::{
    self, Color32, CursorIcon, PointerButton, Pos2, Rect, ResizeDirection, Stroke, Vec2,
    ViewportCommand,
};

use crate::controller::Controller;

const BINS: usize = 384;
const REFRESH: Duration = Duration::from_millis(80);
const DURATIONS: [DurationChoice; 3] = [
    DurationChoice::Seconds10,
    DurationChoice::Seconds20,
    DurationChoice::Seconds30,
];
const MINT: Color32 = Color32::from_rgb(103, 231, 181);
const IDLE: Color32 = Color32::from_rgb(246, 196, 118);
const RESIZE_EDGE: f32 = 7.0;
const RESIZE_CORNER: f32 = 14.0;

pub(crate) fn run(controller: Controller) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([610.0, 144.0])
            .with_min_inner_size([510.0, 132.0])
            .with_decorations(false)
            .with_resizable(true)
            .with_always_on_top(),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Backtracking",
        options,
        Box::new(|context| {
            configure_style(&context.egui_ctx);
            Ok(Box::new(Recorder::new(controller)))
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
    output: PathBuf,
    pinned: bool,
    idle_resume: Option<DurationChoice>,
    last_interaction: Instant,
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

struct ExportJob {
    revision: u64,
    worker: JoinHandle<io::Result<PathBuf>>,
}

impl Recorder {
    fn new(controller: Controller) -> Self {
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            controller,
            output: home.join("Backtracking").join("output"),
            pinned: true,
            idle_resume: None,
            last_interaction: Instant::now(),
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

    fn resume(&mut self) {
        self.controller.resume();
        self.last_interaction = Instant::now();
        if !self.controller.is_paused() {
            self.clear_snapshot();
        }
    }

    fn clear_snapshot(&mut self) {
        self.snapshot = None;
        self.selection = None;
        self.live_revision = None;
        self.next_refresh = Instant::now();
        self.invalidate_export();
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
            let (status, color) = if self.controller.error().is_some() {
                ("Capture unavailable", IDLE)
            } else if self.controller.is_paused() {
                ("Paused", IDLE)
            } else if self.controller.is_recording() {
                ("Recording", MINT)
            } else {
                ("Waiting for sound", ui.visuals().weak_text_color())
            };
            ui.colored_label(color, status);
            if ui
                .add_enabled(
                    self.controller.is_paused() || self.controller.error().is_some(),
                    egui::Button::new("Resume"),
                )
                .clicked()
            {
                self.resume();
            }
            if ui.checkbox(&mut self.pinned, "Keep on top").changed() {
                let level = if self.pinned {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                };
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.settings_button(ui, frame)
            })
            .inner
        })
        .inner
    }

    fn settings_button(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) -> bool {
        let response = ui
            .add(egui::Button::new("").min_size(Vec2::splat(24.0)))
            .on_hover_text("Settings");
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
        let shown = popup.show(|ui| {
            ui.set_min_width(222.0);
            egui::Grid::new("recording-settings")
                .spacing([10.0, 3.0])
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
                        }
                    });
                    ui.end_row();
                    ui.label("Auto-resume");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.idle_resume, None, "Off");
                        for value in DURATIONS {
                            ui.selectable_value(
                                &mut self.idle_resume,
                                Some(value),
                                format!("{} s", value.seconds()),
                            );
                        }
                    });
                    ui.end_row();
                });

            ui.separator();
            let (choose_project, quit) = ui
                .horizontal(|ui| {
                    let choose_project = ui
                        .button("Project folder…")
                        .on_hover_text(self.output.display().to_string())
                        .clicked();
                    let quit = ui
                        .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.button("Quit").clicked()
                        })
                        .inner;
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
                self.message = format!("Output: {}", self.output.display());
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
        match job.worker.join() {
            Ok(Ok(path)) => {
                self.message = format!("Saved {}", path.display());
                if job.revision == self.clip_revision {
                    self.exported = Some(path);
                }
            }
            Ok(Err(error)) => self.message = format!("Could not save WAV: {error}"),
            Err(_) => self.message = "WAV export worker stopped unexpectedly.".into(),
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
            self.controller.pause();
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
        painter.rect_filled(rect, 6.0, Color32::from_rgb(22, 26, 29));
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
        egui::CentralPanel::default().show(ui, |ui| {
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

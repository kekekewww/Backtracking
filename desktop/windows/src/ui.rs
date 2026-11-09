use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{DurationChoice, Snapshot, export_wav, waveform};
use eframe::egui::{self, Color32, PointerButton, Pos2, Rect, Stroke, Vec2};

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

pub(crate) fn run(controller: Controller) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([610.0, 166.0])
            .with_min_inner_size([510.0, 152.0])
            .with_always_on_top(),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Backtracking",
        options,
        Box::new(|context| {
            context.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(Recorder::new(controller)))
        }),
    )
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
    clip: Option<Range<f32>>,
    explicit_clip: bool,
    gesture: Option<Gesture>,
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
            clip: None,
            explicit_clip: false,
            gesture: None,
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
        self.clip = None;
        self.explicit_clip = false;
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

    fn controls(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
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
        });

        ui.horizontal(|ui| {
            let mut retention = self.controller.retention();
            egui::ComboBox::from_id_salt("retention")
                .selected_text(format!("History: {} s", retention.seconds()))
                .show_ui(ui, |ui| {
                    for value in DURATIONS {
                        ui.selectable_value(
                            &mut retention,
                            value,
                            format!("{} s", value.seconds()),
                        );
                    }
                });
            if retention != self.controller.retention() {
                self.controller.set_retention(retention);
                self.clear_snapshot();
            }

            egui::ComboBox::from_id_salt("idle")
                .selected_text(match self.idle_resume {
                    Some(duration) => format!("Auto-resume: {} s", duration.seconds()),
                    None => "Auto-resume: off".into(),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.idle_resume, None, "Off");
                    for value in DURATIONS {
                        ui.selectable_value(
                            &mut self.idle_resume,
                            Some(value),
                            format!("{} s", value.seconds()),
                        );
                    }
                });

            if ui
                .button("Project folder…")
                .on_hover_text(self.output.display().to_string())
                .clicked()
            {
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
        });
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
        let Some(clip) = self.clip.clone() else {
            return;
        };
        let frames = snapshot.frame_count();
        let start = ((clip.start * frames as f32).floor() as usize).min(frames - 1);
        let end = ((clip.end * frames as f32).ceil() as usize).clamp(start + 1, frames);
        let output = self.output.clone();
        match thread::Builder::new()
            .name("backtracking-wav".into())
            .spawn(move || {
                let samples = snapshot
                    .frames(start..end)
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
                input.modifiers.ctrl || input.modifiers.alt,
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
            let start = fraction(self.audio_rect(rect), position.x);
            if selecting {
                self.gesture = Some(Gesture::Select {
                    anchor: start,
                    cursor: start,
                });
            } else {
                if !self.explicit_clip
                    && self.clip.as_ref().is_none_or(|range| range.start != start)
                {
                    self.clip = Some(start..1.0);
                    self.invalidate_export();
                }
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
                    self.clip = Some(range);
                    self.explicit_clip = true;
                    self.invalidate_export();
                    self.start_export();
                }
            }
        }
    }

    fn draw_waveform(&self, ui: &egui::Ui, rect: Rect) {
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
            Some(Gesture::Export { .. }) => self.clip.clone(),
            None if self.explicit_clip => self.clip.clone(),
            None => ui
                .input(|input| input.pointer.hover_pos())
                .filter(|position| rect.contains(*position))
                .map(|position| fraction(audio, position.x)..1.0),
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
        let (position, modifier) = ui.input(|input| {
            (
                input.pointer.hover_pos(),
                input.modifiers.ctrl || input.modifiers.alt,
            )
        });
        let Some(position) = position.filter(|point| response.rect.contains(*point)) else {
            return;
        };
        let hint = match self.gesture {
            Some(Gesture::Select { .. }) => "[Drag to Select]",
            Some(Gesture::Export { .. }) => "[Drag and Drop]",
            None if modifier => "[Drag to Select]",
            None if self.exported.is_some() || self.explicit_clip => {
                "[Drag and Drop]   [Ctrl + Click]"
            }
            None => "[Click]   [Ctrl + Click]",
        };
        let bounds = ui.ctx().content_rect().shrink(5.0);
        let painter = ui.painter().with_clip_rect(bounds);
        let foreground = Color32::from_rgb(234, 243, 240);
        let text = painter.layout_no_wrap(
            hint.to_owned(),
            egui::FontId::proportional(12.0),
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
    fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        if input.viewport().minimized != Some(true)
            && input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::PointerButton { pressed: true, .. }))
        {
            self.last_interaction = Instant::now();
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
            self.controls(ui, frame);
            self.refresh_waveform();
            let size = Vec2::new(
                ui.available_width(),
                (ui.available_height() - 26.0).max(46.0),
            );
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            self.interact_waveform(ui, frame, &response);
            self.draw_waveform(ui, rect);
            let status = self.controller.error().unwrap_or(&self.message);
            if !status.is_empty() {
                ui.add(egui::Label::new(status).truncate())
                    .on_hover_text(status);
            }
            self.draw_action_hint(ui, &response);
            response.on_hover_cursor(egui::CursorIcon::Grab);
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

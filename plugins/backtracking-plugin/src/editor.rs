use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{DurationChoice, Snapshot, export_wav, waveform};
use egui::{Color32, ComboBox, CursorIcon, PointerButton, Pos2, Rect, Sense, Stroke, Vec2};
use nice_plug_egui::baseview::dpi::LogicalSize;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, Frame, NiceEguiApp, RepaintNotifier,
    create_egui_editor,
};

use crate::Session;

const BINS: usize = 512;
const REFRESH: Duration = Duration::from_millis(80);
const DURATIONS: [DurationChoice; 3] = [
    DurationChoice::Seconds10,
    DurationChoice::Seconds20,
    DurationChoice::Seconds30,
];
const MINT: Color32 = Color32::from_rgb(103, 231, 181);

pub type CaptureEditor = EguiEditor<CaptureUi>;

pub(crate) fn create(session: Arc<Session>) -> Option<CaptureEditor> {
    create_egui_editor(
        EguiEditorState::from_size(LogicalSize::new(640.0, 212.0), 1.0),
        RepaintNotifier::new(),
        EguiNiceSettings::new().with_tile("Backtracking"),
        CaptureUi {
            session,
            minimum: [0.0; BINS],
            maximum: [0.0; BINS],
            seconds: 0.0,
            next_refresh: Instant::now(),
            frozen: None,
            selection: None,
            implicit_start: None,
            gesture: None,
            export: None,
            export_requested: false,
            exported: None,
            revision: 0,
            native_dragging: Arc::new(AtomicBool::new(false)),
            message: String::new(),
        },
    )
}

pub struct CaptureUi {
    session: Arc<Session>,
    minimum: [f32; BINS],
    maximum: [f32; BINS],
    seconds: f32,
    next_refresh: Instant,
    frozen: Option<Arc<Snapshot>>,
    selection: Option<Range<f32>>,
    implicit_start: Option<f32>,
    gesture: Option<Gesture>,
    export: Option<ExportJob>,
    export_requested: bool,
    exported: Option<PathBuf>,
    revision: u64,
    native_dragging: Arc<AtomicBool>,
    message: String,
}

enum Gesture {
    Select { anchor: f32, current: f32 },
    Drag { origin: Pos2 },
}

struct ExportJob {
    revision: u64,
    worker: JoinHandle<io::Result<PathBuf>>,
}

impl NiceEguiApp for CaptureUi {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame) {
        let mut settings = self.session.settings();
        let paused = self.session.is_paused();
        self.collect_export();
        self.refresh_waveform(paused);

        if ui.input(|input| input.pointer.any_pressed()) {
            self.session.mark_interaction();
        }

        ui.horizontal(|ui| {
            ui.heading("Backtracking");
            ui.separator();
            let (status, color) = if paused {
                ("Paused", Color32::from_rgb(246, 196, 118))
            } else if self.session.is_recording() {
                ("Recording", MINT)
            } else {
                ("Waiting for audio", ui.visuals().weak_text_color())
            };
            ui.colored_label(color, status);
            if ui
                .add_enabled(paused, egui::Button::new("Resume"))
                .clicked()
            {
                self.reset_frozen();
                self.session.resume();
            }
        });

        ui.horizontal(|ui| {
            let mut retention = settings.retention;
            ComboBox::from_id_salt("retention")
                .selected_text(format!("Buffer: {} s", retention.seconds()))
                .show_ui(ui, |ui| {
                    for duration in DURATIONS {
                        ui.selectable_value(
                            &mut retention,
                            duration,
                            format!("{} s", duration.seconds()),
                        );
                    }
                });
            if retention != settings.retention {
                self.session.set_retention(retention);
                settings.retention = retention;
                self.reset_frozen();
                self.refresh_waveform(paused);
            }

            let mut idle_resume = settings.idle_resume;
            ComboBox::from_id_salt("idle-resume")
                .selected_text(idle_resume.map_or_else(
                    || "Auto-resume: off".to_string(),
                    |duration| format!("Auto-resume: {} s", duration.seconds()),
                ))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut idle_resume, None, "Off");
                    for duration in DURATIONS {
                        ui.selectable_value(
                            &mut idle_resume,
                            Some(duration),
                            format!("{} s", duration.seconds()),
                        );
                    }
                });
            if idle_resume != settings.idle_resume {
                self.session.set_idle_resume(idle_resume);
            }

            if ui
                .button("Choose project…")
                .on_hover_text(settings.output_dir.display().to_string())
                .clicked()
            {
                self.session.set_gesture_active(true);
                let selection = rfd::FileDialog::new()
                    .set_title("Choose a project folder for Backtracking WAVs")
                    .set_parent(frame.baseview_window())
                    .pick_folder();
                self.session.set_gesture_active(false);
                if let Some(folder) = selection {
                    let output = folder.join("output");
                    self.session.set_output_dir(output.clone());
                    self.invalidate_export();
                    self.message = format!("Output: {}", output.display());
                }
            }
        });

        let dropped = self.session.dropped_frames();
        let status_is_visible = dropped != 0 || !self.message.is_empty();
        let status_height = if status_is_visible {
            ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y
        } else {
            0.0
        };
        let wave_size = Vec2::new(
            ui.available_width(),
            (ui.available_height() - status_height).max(60.0),
        );
        let (rect, response) = ui.allocate_exact_size(wave_size, Sense::click_and_drag());
        let configured_seconds = settings.retention.seconds() as f32;
        self.interact_waveform(ui, frame, rect, configured_seconds);
        self.draw_waveform(ui, rect, configured_seconds);
        self.draw_action_hint(ui, rect);
        response.on_hover_cursor(CursorIcon::Grab);

        if dropped != 0 {
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
        ui.ctx()
            .request_repaint_after(if self.gesture.is_some() || self.export.is_some() {
                Duration::from_millis(16)
            } else {
                REFRESH
            });
    }

    fn editor_closed(&mut self) {
        self.gesture = None;
        if !self.native_dragging.load(Ordering::Acquire) {
            self.session.set_gesture_active(false);
        }
    }
}

impl Drop for CaptureUi {
    fn drop(&mut self) {
        if let Some(job) = self.export.take() {
            // Do not leave plugin code running on a detached worker after library unload.
            let _ = job.worker.join();
        }
        self.session.set_gesture_active(false);
    }
}

impl CaptureUi {
    fn refresh_waveform(&mut self, paused: bool) {
        if paused && self.frozen.is_some() {
            return;
        }
        if !paused && self.frozen.is_some() {
            self.reset_frozen();
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

    fn reset_frozen(&mut self) {
        self.frozen = None;
        self.selection = None;
        self.implicit_start = None;
        self.invalidate_export();
        self.next_refresh = Instant::now();
    }

    fn invalidate_export(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.export_requested = false;
        self.exported = None;
    }

    fn audio_rect(&self, rect: Rect, seconds: f32) -> Rect {
        let inner = rect.shrink(8.0);
        let width = inner.width() * (self.seconds / seconds).clamp(0.0, 1.0);
        Rect::from_min_max(Pos2::new(inner.right() - width, inner.top()), inner.max)
    }

    fn draw_waveform(&self, ui: &egui::Ui, rect: Rect, seconds: f32) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, Color32::from_rgb(22, 26, 29));
        let inner = rect.shrink(8.0);
        let middle = inner.center().y;
        let extent = inner.height() * 0.47;
        let baseline = Stroke::new(1.0, Color32::from_rgb(47, 60, 64));
        painter.hline(inner.x_range(), middle, baseline);
        let audio_rect = self.audio_rect(rect, seconds);
        let width = audio_rect.width();
        if width <= 0.0 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No captured audio",
                egui::FontId::proportional(13.0),
                Color32::GRAY,
            );
            return;
        }

        let at = |fraction: f32| audio_rect.left() + width * fraction;
        let selecting = match &self.gesture {
            Some(Gesture::Select { anchor, current }) => {
                Some(anchor.min(*current)..anchor.max(*current))
            }
            Some(Gesture::Drag { .. }) => self
                .selection
                .clone()
                .or_else(|| self.implicit_start.map(|start| start..1.0)),
            None => self.selection.clone().or_else(|| {
                ui.input(|input| input.pointer.hover_pos())
                    .filter(|position| rect.contains(*position))
                    .map(|position| position_in_audio(audio_rect, position.x)..1.0)
            }),
        };
        if let Some(range) = selecting {
            let selection = Rect::from_min_max(
                Pos2::new(at(range.start), inner.top()),
                Pos2::new(at(range.end), inner.bottom()),
            );
            painter.rect_filled(selection, 2.0, Color32::from_rgb(31, 79, 64));
        }

        let wave_stroke = Stroke::new(1.0, MINT);
        for (bin, (&low, &high)) in self.minimum.iter().zip(&self.maximum).enumerate() {
            let x = at((bin as f32 + 0.5) / BINS as f32);
            let top = middle - high.clamp(-1.0, 1.0) * extent;
            let bottom = middle - low.clamp(-1.0, 1.0) * extent;
            painter.line_segment(
                [Pos2::new(x, top), Pos2::new(x, bottom.max(top + 0.4))],
                wave_stroke,
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
            Some(Gesture::Select { .. }) => "[Drag to Select]".into(),
            Some(Gesture::Drag { .. }) => "[Drag and Drop]".into(),
            None if down => return,
            None if modifier => "[Drag to Select]".into(),
            None => {
                let action = if self.exported.is_some() || self.selection.is_some() {
                    "[Drag and Drop]"
                } else {
                    "[Click]"
                };
                let selection = if cfg!(target_os = "macos") {
                    "[Option + Click]"
                } else {
                    "[Ctrl + Click]"
                };
                format!("{action}\n{selection}")
            }
        };

        let bounds = visible_waveform.shrink(2.0);
        let painter = ui.painter_at(bounds);
        let color = Color32::from_rgb(232, 242, 238);
        let label = painter.layout_no_wrap(hint, egui::FontId::proportional(12.0), color);
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

    fn interact_waveform(&mut self, ui: &egui::Ui, frame: &Frame, rect: Rect, seconds: f32) {
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

        if pressed && let Some(position) = position.filter(|position| rect.contains(*position)) {
            self.session.set_gesture_active(true);
            self.session.pause();
            if self.frozen.is_none()
                && let Some(snapshot) = self.session.snapshot()
            {
                self.update_peaks(&snapshot);
                self.frozen = Some(Arc::new(snapshot));
            }
            if selecting {
                let anchor = position_in_audio(self.audio_rect(rect, seconds), position.x);
                self.gesture = Some(Gesture::Select {
                    anchor,
                    current: anchor,
                });
            } else {
                if self.selection.is_none() {
                    let audio_rect = self.audio_rect(rect, seconds);
                    let start = position_in_audio(audio_rect, position.x);
                    let pixel = 1.0 / audio_rect.width().max(1.0);
                    if self
                        .implicit_start
                        .is_none_or(|old| (start - old).abs() > pixel)
                    {
                        self.implicit_start = Some(start);
                        self.invalidate_export();
                    }
                }
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
            if let (Some(Gesture::Drag { origin }), Some(position), Some(path)) =
                (&self.gesture, position, self.exported.clone())
                && position.distance(*origin) >= 4.0
            {
                let origin = *origin;
                self.gesture = None;
                self.start_native_drag(frame, path, origin);
            }
        } else if let Some(gesture) = self.gesture.take() {
            if let Gesture::Select { anchor, current } = gesture {
                let range = anchor.min(current)..anchor.max(current);
                self.selection = (range.end > range.start).then_some(range);
                self.implicit_start = Some(anchor);
                self.invalidate_export();
                self.begin_export();
            }
            self.session.set_gesture_active(false);
        }
    }

    fn begin_export(&mut self) {
        if self.exported.is_some() {
            return;
        }
        if self.export.is_some() {
            self.export_requested = true;
            return;
        }
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
        let selection = self
            .selection
            .clone()
            .or_else(|| self.implicit_start.map(|start| start..1.0));
        let range = selection.map_or(0..frames, |range| {
            let first = (range.start * frames as f32).floor() as usize;
            let last = (range.end * frames as f32).ceil() as usize;
            first.min(frames - 1)..last.clamp(first.min(frames - 1) + 1, frames)
        });
        let output = self.session.settings().output_dir;
        let worker = thread::Builder::new()
            .name("backtracking-wav".into())
            .spawn(move || {
                let samples = snapshot
                    .frames(range)
                    .ok_or_else(|| io::Error::other("invalid audio selection"))?;
                export_wav(&output, snapshot.sample_rate, snapshot.channels, samples)?
                    .canonicalize()
            });
        match worker {
            Ok(worker) => {
                self.export = Some(ExportJob {
                    revision: self.revision,
                    worker,
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
                if job.revision == self.revision {
                    self.exported = Some(path);
                }
            }
            Ok(Err(error)) => self.message = format!("Could not save WAV: {error}"),
            Err(_) => self.message = "WAV export worker stopped unexpectedly.".into(),
        }
        if std::mem::take(&mut self.export_requested) {
            self.begin_export();
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
                session.set_gesture_active(false);
                dragging.store(false, Ordering::Release);
            },
            drag::Options::default(),
        );
        if let Err(error) = result {
            self.native_dragging.store(false, Ordering::Release);
            self.session.set_gesture_active(false);
            self.message = format!("Could not start file drag: {error}");
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn start_native_drag(&mut self, _frame: &Frame, path: PathBuf, _origin: Pos2) {
        self.session.set_gesture_active(false);
        self.message = format!(
            "WAV saved. Drag the file from its folder: {}",
            path.display()
        );
    }
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
            self.session.set_gesture_active(false);
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

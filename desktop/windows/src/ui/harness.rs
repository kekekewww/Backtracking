//! A test copy of the app (see `startup::test_instance`) can put its window into a known
//! state, photograph it, write how long its frames took and quit. This lets a change to what
//! the window draws be checked without a person at the screen.
//!
//! - `BACKTRACKING_TEST_SCREENSHOT=<file.png>` turns it on.
//! - `BACKTRACKING_TEST_SCREENSHOT_DELAY_MS=<ms>` is how long the app runs first (default 4000).
//! - `BACKTRACKING_TEST_VIEW=<steps>` is done when the delay has passed, before the picture.
//!   Steps are separated by `;`. Times are seconds back from the newest audio:
//!   `zoom=<seconds>` shows only that much; `select=<from>-<to>` selects that stretch, as a
//!   drag would; `scroll=<lane>:<row>` scrolls a stacked lane (A is 0) to that row;
//!   `group=<lane>:<group id>` shows another channel group.
//!   `options=<page>[:<width>x<height>]` opens the Options window on that page (`actions`,
//!   `capture`, `sources`, `appearance`, `export`, `addons`, `updates`), optionally at that
//!   size, and makes the picture of the Options window instead of the main window.
//!   `drop=<file>` drops a file on the main window and `drop-options=<file>` on the Options
//!   window: the file reaches egui's input as a dropped file, as one from Explorer does.
//!   `remove=<add-on id>` opens the question that comes before removing that add-on.
//!   `install` presses Install on the confirmation, and `set=<add-on id>:<key>=<JSON value>`
//!   changes a setting as its control would, with any value. Each of these two waits until
//!   the steps before it have settled (a dropped package has been read, say).
//!   `format=<lane>:<format id>` chooses a format of the lane's add-on source,
//!   `open-format=<lane>` shows that lane's format list open, `open=<add-on id>:<file>` opens a
//!   file in an add-on, `transport=<lane>:<play|pause|stop|seek,<seconds>>` sends a transport
//!   command and `wait=<ms>` lets the steps before it run for that much longer before the next
//!   one. A test copy refuses `open=` (and any other way to open a file) unless the add-on's
//!   `play_through_speakers` setting is known to be off, so a test makes no sound; use
//!   `set=<add-on id>:play_through_speakers=false` first.
//!
//! The picture is `<file.png>`; `<file.png>.log` gets the frame times and any problem.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use backtracking_addon::protocol::TransportCommand;
use eframe::egui;

use super::{OptionsSection, Recorder};

pub(super) const SCREENSHOT: &str = "BACKTRACKING_TEST_SCREENSHOT";
const SCREENSHOT_DELAY: &str = "BACKTRACKING_TEST_SCREENSHOT_DELAY_MS";
const VIEW: &str = "BACKTRACKING_TEST_VIEW";
/// Where a test copy puts its window, in physical pixels: `<x>,<y>`.
pub(super) const WINDOW_POSITION: &str = "BACKTRACKING_TEST_WINDOW_POSITION";
/// Frames drawn while the app starts are left out of the timings.
const WARM_UP: Duration = Duration::from_millis(1500);
/// Time for a step's effect (a new waveform, a frozen selection) to reach the screen.
const SETTLE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Step {
    Zoom(f64),
    Select(f64, f64),
    Scroll(usize, usize),
    Group(usize, String),
    Options(OptionsSection, Option<egui::Vec2>),
    Drop(PathBuf),
    DropOnOptions(PathBuf),
    Remove(String),
    Install,
    Set(String, String, serde_json::Value),
    Format(usize, String),
    OpenFormatList(usize),
    Open(String, PathBuf),
    Transport(usize, TransportCommand, Option<f64>),
    Wait(u64),
}

impl Step {
    /// Steps that act on what earlier steps brought about, so they wait for it.
    fn waits(&self) -> bool {
        matches!(
            self,
            Self::Install
                | Self::Set(..)
                | Self::Format(..)
                | Self::Open(..)
                | Self::Transport(..)
                | Self::Wait(_)
        )
    }
}

fn options_page(name: &str) -> Result<OptionsSection, String> {
    let wanted: String = name.trim().to_ascii_lowercase().replace('-', "");
    OptionsSection::ALL
        .iter()
        .find(|(_, label)| label.to_ascii_lowercase().replace('-', "") == wanted)
        .map(|(section, _)| *section)
        .ok_or_else(|| format!("{name} is not an Options page"))
}

fn window_size(text: &str) -> Result<egui::Vec2, String> {
    let (width, height) = text
        .split_once('x')
        .ok_or_else(|| format!("{text} is not <width>x<height>"))?;
    let size = |text: &str| {
        text.trim()
            .parse::<f32>()
            .ok()
            .filter(|value| (200.0..=4000.0).contains(value))
            .ok_or_else(|| format!("{text} is not a size from 200 to 4000"))
    };
    Ok(egui::Vec2::new(size(width)?, size(height)?))
}

/// A file the harness drops, as egui-winit hands over one dropped from Explorer.
#[derive(Debug)]
struct DroppedForTest(PathBuf);

impl egui::DroppedFile for DroppedForTest {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        fs::read(&self.0).map_err(|error| error.to_string())
    }
}

type PictureResult = io::Result<[usize; 2]>;

/// Writes a window's pixels as a PNG and returns its size.
fn write_picture(output: &Path, image: &egui::ColorImage) -> PictureResult {
    let size = image.size;
    let pixels: Vec<u8> = image
        .pixels
        .iter()
        // The colours are premultiplied, so dropping alpha shows the window over black.
        .flat_map(|pixel| [pixel.r(), pixel.g(), pixel.b()])
        .collect();
    fs::File::create(output)
        .and_then(|mut file| file.write_all(&png_rgb(size[0], size[1], &pixels)))
        .map(|()| size)
}

pub(super) fn parse_steps(text: &str) -> Result<Vec<Step>, String> {
    text.split(';')
        .map(str::trim)
        .filter(|step| !step.is_empty())
        .map(|step| {
            if step == "install" {
                return Ok(Step::Install);
            }
            let (name, value) = step
                .split_once('=')
                .ok_or_else(|| format!("no '=' in {step}"))?;
            let number = |text: &str| {
                text.trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .ok_or_else(|| format!("{text} is not a number of seconds"))
            };
            fn pair<'a>(step: &str, text: &'a str) -> Result<(&'a str, &'a str), String> {
                text.split_once(':')
                    .ok_or_else(|| format!("{step} needs <lane>:<value>"))
            }
            let lane = |text: &str| match text.trim() {
                "0" | "A" | "a" => Ok(0),
                "1" | "B" | "b" => Ok(1),
                other => Err(format!("{other} is not a lane")),
            };
            match name.trim() {
                "zoom" => Ok(Step::Zoom(number(value)?)),
                "select" => {
                    let (from, to) = value
                        .split_once('-')
                        .ok_or_else(|| format!("{step} needs <from>-<to>"))?;
                    Ok(Step::Select(number(from)?, number(to)?))
                }
                "scroll" => {
                    let (lane_text, row) = pair(step, value)?;
                    let row = row
                        .trim()
                        .parse()
                        .map_err(|_| format!("{row} is not a row"))?;
                    Ok(Step::Scroll(lane(lane_text)?, row))
                }
                "group" => {
                    let (lane_text, id) = pair(step, value)?;
                    Ok(Step::Group(lane(lane_text)?, id.trim().to_owned()))
                }
                "options" => match value.split_once(':') {
                    Some((page, size)) => {
                        Ok(Step::Options(options_page(page)?, Some(window_size(size)?)))
                    }
                    None => Ok(Step::Options(options_page(value)?, None)),
                },
                "format" => {
                    let (lane_text, id) = pair(step, value)?;
                    Ok(Step::Format(lane(lane_text)?, id.trim().to_owned()))
                }
                "open-format" => Ok(Step::OpenFormatList(lane(value)?)),
                "open" => {
                    let (id, file) = pair(step, value)?;
                    Ok(Step::Open(id.trim().to_owned(), PathBuf::from(file.trim())))
                }
                "transport" => {
                    let (lane_text, command) = pair(step, value)?;
                    let (command, position) = match command.trim().split_once(',') {
                        Some(("seek", seconds)) => (TransportCommand::Seek, Some(number(seconds)?)),
                        _ => (
                            match command.trim() {
                                "play" => TransportCommand::Play,
                                "pause" => TransportCommand::Pause,
                                "stop" => TransportCommand::Stop,
                                other => return Err(format!("{other} is not a transport command")),
                            },
                            None,
                        ),
                    };
                    Ok(Step::Transport(lane(lane_text)?, command, position))
                }
                "wait" => {
                    Ok(Step::Wait(value.trim().parse().map_err(|_| {
                        format!("{value} is not a number of milliseconds")
                    })?))
                }
                "drop" => Ok(Step::Drop(PathBuf::from(value.trim()))),
                "drop-options" => Ok(Step::DropOnOptions(PathBuf::from(value.trim()))),
                "remove" => Ok(Step::Remove(value.trim().to_owned())),
                "set" => {
                    let (id, setting) = value
                        .split_once(':')
                        .ok_or_else(|| format!("{step} needs <add-on id>:<key>=<value>"))?;
                    let (key, json) = setting
                        .split_once('=')
                        .ok_or_else(|| format!("{step} needs <add-on id>:<key>=<value>"))?;
                    let json = serde_json::from_str(json.trim())
                        .map_err(|error| format!("{json} is not a JSON value: {error}"))?;
                    Ok(Step::Set(id.trim().to_owned(), key.trim().to_owned(), json))
                }
                other => Err(format!("unknown step {other}")),
            }
        })
        .collect()
}

pub(super) struct Harness {
    output: PathBuf,
    started: Instant,
    delay: Duration,
    steps: Vec<Step>,
    applied: Option<Instant>,
    result: Option<mpsc::Receiver<io::Result<[usize; 2]>>>,
    /// Seconds of work for each frame drawn after the warm-up, as eframe measured them.
    frame_seconds: Vec<f32>,
    first_timed: Option<Instant>,
    last_timed: Option<Instant>,
    notes: Vec<String>,
    /// The picture is of the Options window, at this size when one was asked for.
    options_picture: bool,
    options_size: Option<egui::Vec2>,
    main_drops: Vec<PathBuf>,
    options_drops: Vec<PathBuf>,
    /// Waits for the Options window's next pass to take its picture.
    options_shot: Option<mpsc::SyncSender<PictureResult>>,
    /// Extra time the last step asked for before the next one.
    hold: Duration,
}

impl Harness {
    /// The size the Options window is created at, when a step asked for one.
    pub(super) fn options_size(&self) -> Option<egui::Vec2> {
        self.options_size
    }

    /// Hands files the steps dropped on the main window to egui, in its raw input.
    pub(super) fn drop_on_main(&mut self, input: &mut egui::RawInput) {
        for path in self.main_drops.drain(..) {
            input.dropped_files.push(Arc::new(DroppedForTest(path)));
        }
    }

    /// Hands files the steps dropped on the Options window to its input, before it is read.
    pub(super) fn drop_on_options(&mut self, ctx: &egui::Context) {
        if self.options_drops.is_empty() {
            return;
        }
        let drops = std::mem::take(&mut self.options_drops);
        ctx.input_mut(|input| {
            for path in drops {
                input.raw.dropped_files.push(Arc::new(DroppedForTest(path)));
            }
        });
    }

    /// Takes the Options window's picture when it is due. eframe takes screenshots of the
    /// main window only, so the last shape drawn reads the finished frame back from OpenGL.
    pub(super) fn photograph_options(&mut self, ctx: &egui::Context) {
        let Some(send) = self.options_shot.take() else {
            return;
        };
        let job = Mutex::new(Some((self.output.clone(), send)));
        let wake = ctx.clone();
        let callback = eframe::egui_glow::CallbackFn::new(move |info, painter| {
            let Some((output, send)) = job.lock().ok().and_then(|mut job| job.take()) else {
                return;
            };
            let image = painter.read_screen_rgba(info.screen_size_px);
            let _ = send.send(write_picture(&output, &image));
            wake.request_repaint_of(egui::ViewportId::ROOT);
        });
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Debug,
            egui::Id::unique("harness-picture"),
        ))
        .add(egui::PaintCallback {
            rect: ctx.content_rect(),
            callback: Arc::new(callback),
        });
    }

    pub(super) fn from_environment() -> Option<Self> {
        let output = PathBuf::from(std::env::var_os(SCREENSHOT)?);
        let delay = std::env::var(SCREENSHOT_DELAY)
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .map_or(Duration::from_millis(4000), Duration::from_millis);
        let mut notes = Vec::new();
        let steps = match std::env::var(VIEW) {
            Ok(text) => parse_steps(&text).unwrap_or_else(|error| {
                notes.push(format!("{VIEW} ignored: {error}"));
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        let options_size = steps.iter().find_map(|step| match step {
            Step::Options(_, size) => *size,
            _ => None,
        });
        Some(Self {
            output,
            started: Instant::now(),
            delay,
            options_picture: steps.iter().any(|step| matches!(step, Step::Options(..))),
            options_size,
            steps,
            applied: None,
            result: None,
            frame_seconds: Vec::new(),
            first_timed: None,
            last_timed: None,
            notes,
            main_drops: Vec::new(),
            options_drops: Vec::new(),
            options_shot: None,
            hold: Duration::ZERO,
        })
    }

    fn write_log(&self, picture: &io::Result<[usize; 2]>) {
        let mut log = String::new();
        match picture {
            Ok([width, height]) => {
                log.push_str(&format!(
                    "picture {} ({width} x {height} px)\n",
                    self.output.display()
                ));
            }
            Err(error) => log.push_str(&format!("picture failed: {error}\n")),
        }
        let mut seconds = self.frame_seconds.clone();
        seconds.sort_by(f32::total_cmp);
        if let (Some(first), Some(last)) = (self.first_timed, self.last_timed)
            && !seconds.is_empty()
        {
            let span = last.duration_since(first).as_secs_f64();
            let at = |fraction: f64| {
                seconds[((seconds.len() - 1) as f64 * fraction).round() as usize] * 1000.0
            };
            let mean = seconds.iter().map(|&value| f64::from(value)).sum::<f64>()
                / seconds.len() as f64
                * 1000.0;
            log.push_str(&format!(
                "frames {} in {span:.2} s ({:.1} per second)\n\
                 frame work ms: mean {mean:.2}, median {:.2}, 95th percentile {:.2}, max {:.2}\n",
                seconds.len(),
                if span > 0.0 {
                    (seconds.len() - 1) as f64 / span
                } else {
                    0.0
                },
                at(0.5),
                at(0.95),
                at(1.0),
            ));
        } else {
            log.push_str("no frames were timed\n");
        }
        for note in &self.notes {
            log.push_str(note);
            log.push('\n');
        }
        let mut path = self.output.clone().into_os_string();
        path.push(".log");
        if let Err(error) = fs::write(&path, log) {
            eprintln!("Could not write {}: {error}", PathBuf::from(path).display());
        }
    }
}

impl Recorder {
    /// Runs the test harness once per drawn frame, after everything else.
    pub(super) fn drive_harness(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(harness) = self.harness.as_mut() else {
            return;
        };
        let now = Instant::now();
        let since_start = now.duration_since(harness.started);
        // Timing stops once the steps change what is drawn.
        if since_start >= WARM_UP
            && harness.applied.is_none()
            && let Some(seconds) = frame.info().cpu_usage
        {
            harness.frame_seconds.push(seconds);
            harness.first_timed.get_or_insert(now);
            harness.last_timed = Some(now);
        }
        if let Some(result) = &harness.result {
            match result.try_recv() {
                Ok(picture) => {
                    harness.write_log(&picture);
                    self.harness = None;
                    self.quit(ctx);
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(SETTLE),
                Err(mpsc::TryRecvError::Disconnected) => {
                    harness.write_log(&Err(io::Error::other("the screenshot was never taken")));
                    self.harness = None;
                    self.quit(ctx);
                }
            }
            return;
        }
        if since_start < harness.delay {
            // One wake at the end of the delay. It adds no frames to the ones measured.
            ctx.request_repaint_after(harness.delay - since_start);
            return;
        }
        let Some(applied) = harness.applied else {
            harness.applied = Some(now);
            self.apply_test_steps(ctx);
            return;
        };
        let options_picture = harness.options_picture;
        let settled = now.duration_since(applied) >= SETTLE + harness.hold
            && self.edit_request.is_none()
            && self.edit_preview.is_none()
            // Dropped files are read in the next pass, and the manager answers after that.
            && harness.main_drops.is_empty()
            && harness.options_drops.is_empty()
            && !self.addons_page.waiting();
        if !settled {
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        if !harness.steps.is_empty() {
            harness.applied = Some(now);
            self.apply_test_steps(ctx);
            return;
        }
        let (send, receive) = mpsc::sync_channel(1);
        harness.result = Some(receive);
        if options_picture {
            if !self.options_open {
                let _ = send.send(Err(io::Error::other("the Options window is not open")));
                return;
            }
            // The Options window draws during this window's next pass and takes it then.
            harness.options_shot = Some(send);
            ctx.request_repaint();
            return;
        }
        let output = harness.output.clone();
        let wake = ctx.clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
            egui::ScreenshotCallback::new(move |image| {
                let _ = send.send(write_picture(&output, &image));
                wake.request_repaint();
            }),
        ));
    }

    /// Does the steps that are left, up to the next one that waits for them to settle.
    fn apply_test_steps(&mut self, ctx: &egui::Context) {
        let Some(harness) = self.harness.as_mut() else {
            return;
        };
        let wait = harness
            .steps
            .iter()
            .skip(1)
            .position(Step::waits)
            .map_or(harness.steps.len(), |index| index + 1);
        harness.hold = Duration::ZERO;
        let steps: Vec<Step> = harness.steps.drain(..wait).collect();
        for step in steps {
            if let Err(error) = self.apply_test_step(&step, ctx)
                && let Some(harness) = &mut self.harness
            {
                harness.notes.push(format!("{step:?}: {error}"));
            }
        }
        ctx.request_repaint_after(SETTLE);
    }

    fn apply_test_step(&mut self, step: &Step, ctx: &egui::Context) -> Result<(), String> {
        let rate = u64::from(self.sample_rate.max(1));
        let frames = |seconds: f64| (seconds * rate as f64).round() as u64;
        let history = self.history_frames.clone();
        if history.is_empty() && matches!(step, Step::Zoom(_) | Step::Select(..)) {
            return Err("no audio yet".into());
        }
        match step {
            Step::Zoom(seconds) => {
                let count = frames(*seconds).max(1);
                self.set_visible_frames(history.end.saturating_sub(count), count);
            }
            &Step::Select(from, to) => {
                let start = history
                    .end
                    .saturating_sub(frames(from.max(to)))
                    .max(history.start);
                let end = history
                    .end
                    .saturating_sub(frames(from.min(to)))
                    .max(start + 1);
                // As a finished drag does: freeze the audio, then keep the range.
                self.begin_editing();
                self.last_interaction = Instant::now();
                let flags = self.flag_update(start..end);
                self.apply_flags(flags);
                self.selection = Some(start..end);
                self.invalidate_export();
            }
            &Step::Scroll(lane, row) => self.scroll_lane_to(lane, row),
            Step::Group(lane, id) => self.show_group(*lane, id)?,
            Step::Options(section, _) => {
                self.options_section = *section;
                if !self.options_open {
                    self.open_options(ctx);
                }
            }
            Step::Drop(path) | Step::DropOnOptions(path) => {
                if !path.is_file() {
                    return Err(format!("{} is not a file", path.display()));
                }
                let harness = self.harness.as_mut().ok_or("the harness has finished")?;
                if matches!(step, Step::Drop(_)) {
                    harness.main_drops.push(path.clone());
                } else {
                    harness.options_drops.push(path.clone());
                }
                ctx.request_repaint();
            }
            Step::Remove(id) => self.addons_page.ask_to_remove(id)?,
            Step::Install => {
                if !self.addons_page.press_install() {
                    return Err("no confirmation is ready to install".into());
                }
                ctx.request_repaint();
            }
            Step::Set(id, key, value) => {
                self.addons_page.change_setting(id, key, value.clone())?;
                ctx.request_repaint();
            }
            Step::Format(lane, format) => {
                let (addon, source) = self.lane_addon_source(*lane)?;
                let handle = self
                    .controller
                    .addons()
                    .cloned()
                    .ok_or("no add-on manager")?;
                self.addon_sources
                    .select_format(&handle, &addon, &source, format)?;
                ctx.request_repaint();
            }
            Step::OpenFormatList(lane) => {
                self.addon_sources.force_format_menu(*lane);
                ctx.request_repaint();
            }
            Step::Open(addon, path) => {
                if !path.is_file() {
                    return Err(format!("{} is not a file", path.display()));
                }
                let handle = self
                    .controller
                    .addons()
                    .cloned()
                    .ok_or("no add-on manager")?;
                self.addon_sources.open_file(&handle, addon, path.clone())?;
                ctx.request_repaint();
            }
            Step::Transport(lane, command, position) => {
                let (addon, source) = self.lane_addon_source(*lane)?;
                let handle = self
                    .controller
                    .addons()
                    .cloned()
                    .ok_or("no add-on manager")?;
                self.addon_sources
                    .send_transport(&handle, &addon, &source, *command, *position);
                ctx.request_repaint();
            }
            Step::Wait(milliseconds) => {
                if let Some(harness) = self.harness.as_mut() {
                    harness.hold = Duration::from_millis(*milliseconds);
                }
            }
        }
        Ok(())
    }

    /// The add-on and source id of the lane's add-on source.
    fn lane_addon_source(&self, lane: usize) -> Result<(String, String), String> {
        match self
            .controller
            .sources()
            .get(lane)
            .and_then(Option::as_ref)
            .map(|settings| &settings.source)
        {
            Some(crate::controller::CaptureSource::Addon {
                addon_id,
                source_id,
                ..
            }) => Ok((addon_id.clone(), source_id.clone())),
            _ => Err("the lane has no add-on source".into()),
        }
    }
}

/// Encodes 8-bit RGB pixels as a PNG. Each row takes whichever of the None, Sub and Up filters
/// leaves the smallest numbers, and the result is deflated with the fixed Huffman code and a
/// simple LZ77 search. A window picture is mostly flat colour, which this shrinks well.
pub(super) fn png_rgb(width: usize, height: usize, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(pixels.len(), width * height * 3, "three bytes per pixel");
    let stride = width * 3;
    let mut filtered = Vec::with_capacity((stride + 1) * height);
    let mut candidates: [Vec<u8>; 3] = std::array::from_fn(|_| vec![0; stride]);
    for y in 0..height {
        let row = &pixels[y * stride..][..stride];
        let above = (y > 0).then(|| &pixels[(y - 1) * stride..][..stride]);
        for x in 0..stride {
            let left = if x >= 3 { row[x - 3] } else { 0 };
            let up = above.map_or(0, |above| above[x]);
            candidates[0][x] = row[x];
            candidates[1][x] = row[x].wrapping_sub(left);
            candidates[2][x] = row[x].wrapping_sub(up);
        }
        let cost = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|&byte| u32::from((byte as i8).unsigned_abs()))
                .sum::<u32>()
        };
        let best = (0..3)
            .min_by_key(|&filter| cost(&candidates[filter]))
            .unwrap_or(0);
        filtered.push(best as u8);
        filtered.extend_from_slice(&candidates[best]);
    }

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    // Eight bits per sample, RGB, deflate, adaptive filtering, no interlace.
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    png_chunk(&mut png, b"IHDR", &header);
    png_chunk(&mut png, b"IDAT", &zlib(&filtered));
    png_chunk(&mut png, b"IEND", &[]);
    png
}

fn png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let crc = crc32(&png[start..]);
    png.extend_from_slice(&crc.to_be_bytes());
}

pub(super) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

pub(super) fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    (b << 16) | a
}

struct Bits {
    bytes: Vec<u8>,
    buffer: u64,
    count: u32,
}

impl Bits {
    /// Writes the low `count` bits of `value`, least significant first.
    fn put(&mut self, value: u32, count: u32) {
        self.buffer |= u64::from(value) << self.count;
        self.count += count;
        while self.count >= 8 {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= 8;
            self.count -= 8;
        }
    }

    /// Writes a Huffman code, which deflate stores most significant bit first.
    fn code(&mut self, code: u32, length: u32) {
        self.put(code.reverse_bits() >> (32 - length), length);
    }

    fn literal(&mut self, symbol: u32) {
        match symbol {
            0..=143 => self.code(0x30 + symbol, 8),
            144..=255 => self.code(0x190 + symbol - 144, 9),
            256..=279 => self.code(symbol - 256, 7),
            _ => self.code(0xC0 + symbol - 280, 8),
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push(self.buffer as u8);
        }
        self.bytes
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// A zlib stream holding one fixed-Huffman deflate block.
pub(super) fn zlib(data: &[u8]) -> Vec<u8> {
    const WINDOW: usize = 32_768;
    const HASH_BITS: u32 = 15;
    const CHAIN: usize = 24;
    let mut bits = Bits {
        bytes: vec![0x78, 0x01],
        buffer: 0,
        count: 0,
    };
    // The last block, compressed with the fixed codes.
    bits.put(1, 1);
    bits.put(1, 2);
    let hash = |at: usize| {
        let value =
            u32::from(data[at]) | u32::from(data[at + 1]) << 8 | u32::from(data[at + 2]) << 16;
        (value.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
    };
    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut previous = vec![usize::MAX; data.len()];
    let insert = |at: usize, head: &mut Vec<usize>, previous: &mut Vec<usize>| {
        if at + 3 <= data.len() {
            let slot = hash(at);
            previous[at] = head[slot];
            head[slot] = at;
        }
    };
    let mut at = 0;
    while at < data.len() {
        let mut best = (0usize, 0usize);
        if at + 3 <= data.len() {
            let mut candidate = head[hash(at)];
            let longest = (data.len() - at).min(258);
            for _ in 0..CHAIN {
                if candidate == usize::MAX || at - candidate > WINDOW {
                    break;
                }
                let length = data[candidate..]
                    .iter()
                    .zip(&data[at..at + longest])
                    .take_while(|(a, b)| a == b)
                    .count();
                if length > best.0 {
                    best = (length, at - candidate);
                    if length == longest {
                        break;
                    }
                }
                candidate = previous[candidate];
            }
        }
        if best.0 >= 3 {
            let (length, distance) = best;
            let code = LENGTH_BASE
                .iter()
                .rposition(|&base| usize::from(base) <= length)
                .unwrap();
            bits.literal(257 + code as u32);
            bits.put(
                (length - usize::from(LENGTH_BASE[code])) as u32,
                u32::from(LENGTH_EXTRA[code]),
            );
            let code = DISTANCE_BASE
                .iter()
                .rposition(|&base| usize::from(base) <= distance)
                .unwrap();
            bits.code(code as u32, 5);
            bits.put(
                (distance - usize::from(DISTANCE_BASE[code])) as u32,
                u32::from(DISTANCE_EXTRA[code]),
            );
            for position in at..at + length {
                insert(position, &mut head, &mut previous);
            }
            at += length;
        } else {
            bits.literal(u32::from(data[at]));
            insert(at, &mut head, &mut previous);
            at += 1;
        }
    }
    bits.literal(256);
    let mut stream = bits.finish();
    stream.extend_from_slice(&adler32(data).to_be_bytes());
    stream
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads a fixed-Huffman deflate stream back, enough to check the encoder.
    fn inflate_fixed(stream: &[u8]) -> Vec<u8> {
        assert_eq!(&stream[..2], &[0x78, 0x01]);
        let body = &stream[2..stream.len() - 4];
        let mut position = 0usize;
        let mut bit = || {
            let value = (body[position / 8] >> (position % 8)) & 1;
            position += 1;
            u32::from(value)
        };
        let read = |count: u32, bit: &mut dyn FnMut() -> u32| {
            (0..count).fold(0, |value, index| value | bit() << index)
        };
        let code = |bit: &mut dyn FnMut() -> u32| {
            let mut value = 0;
            for length in 1..=9 {
                value = value << 1 | bit();
                let symbol = match length {
                    7 if value <= 0b001_0111 => Some(value + 256),
                    8 if (0x30..=0xBF).contains(&value) => Some(value - 0x30),
                    8 if (0xC0..=0xC7).contains(&value) => Some(value - 0xC0 + 280),
                    9 if value >= 0x190 => Some(value - 0x190 + 144),
                    _ => None,
                };
                if let Some(symbol) = symbol {
                    return symbol;
                }
            }
            panic!("not a fixed code");
        };
        assert_eq!(read(1, &mut bit), 1);
        assert_eq!(read(2, &mut bit), 1);
        let mut output = Vec::new();
        loop {
            let symbol = code(&mut bit);
            match symbol {
                0..=255 => output.push(symbol as u8),
                256 => break,
                _ => {
                    let index = (symbol - 257) as usize;
                    let length = usize::from(LENGTH_BASE[index])
                        + read(u32::from(LENGTH_EXTRA[index]), &mut bit) as usize;
                    let distance_code = (0..5).fold(0, |value, _| value << 1 | bit()) as usize;
                    let distance = usize::from(DISTANCE_BASE[distance_code])
                        + read(u32::from(DISTANCE_EXTRA[distance_code]), &mut bit) as usize;
                    for _ in 0..length {
                        output.push(output[output.len() - distance]);
                    }
                }
            }
        }
        let adler = u32::from_be_bytes(stream[stream.len() - 4..].try_into().unwrap());
        assert_eq!(adler, adler32(&output));
        output
    }

    #[test]
    fn checksums_match_their_published_check_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&[]), 1);
    }

    #[test]
    fn deflate_round_trips_runs_repeats_and_noise() {
        let mut noise = 0x1234_5678u32;
        let mut data: Vec<u8> = (0..40_000)
            .map(|index| match index / 10_000 {
                0 => 7,
                1 => (index % 13) as u8,
                _ => {
                    noise ^= noise << 13;
                    noise ^= noise >> 17;
                    noise ^= noise << 5;
                    noise as u8
                }
            })
            .collect();
        data.extend_from_slice(&data[..5000].to_vec());
        for input in [&data[..], &[][..], &[1, 2][..], &[9; 300][..]] {
            assert_eq!(inflate_fixed(&zlib(input)), input);
        }
        assert!(zlib(&[0; 100_000]).len() < 1000, "flat colour must shrink");
    }

    #[test]
    fn png_has_the_signature_header_and_checked_chunks() {
        let png = png_rgb(2, 1, &[255, 0, 0, 0, 0, 255]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        assert_eq!(png[24..29], [8, 2, 0, 0, 0]);
        assert_eq!(&png[29..33], &crc32(&png[12..29]).to_be_bytes());
        let idat_length = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let rows = inflate_fixed(&png[41..41 + idat_length]);
        // One filter byte, then the row: the Sub filter stores the second pixel as a difference.
        assert_eq!(rows.len(), 7);
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn view_steps_are_read_in_order_and_bad_ones_are_named() {
        assert_eq!(
            parse_steps("zoom=2.5; select=1.5-0.25;scroll=A:4;group=b:objects").unwrap(),
            [
                Step::Zoom(2.5),
                Step::Select(1.5, 0.25),
                Step::Scroll(0, 4),
                Step::Group(1, "objects".into()),
            ]
        );
        assert_eq!(parse_steps(" ").unwrap(), []);
        assert_eq!(
            parse_steps(
                r"options=Add-ons:820x1100; drop=D:\a b\tone.btaddon;drop-options=C:\x.btaddon;remove=com.example.tone;options=export"
            )
            .unwrap(),
            [
                Step::Options(OptionsSection::Addons, Some(egui::Vec2::new(820.0, 1100.0))),
                Step::Drop(PathBuf::from(r"D:\a b\tone.btaddon")),
                Step::DropOnOptions(PathBuf::from(r"C:\x.btaddon")),
                Step::Remove("com.example.tone".into()),
                Step::Options(OptionsSection::Export, None),
            ]
        );
        assert_eq!(
            parse_steps("options=addons").unwrap(),
            [Step::Options(OptionsSection::Addons, None)]
        );
        let steps =
            parse_steps("install; set=com.example.tone:frequency=5 ;set=a:b=\"x=y\"").unwrap();
        assert_eq!(
            steps,
            [
                Step::Install,
                Step::Set(
                    "com.example.tone".into(),
                    "frequency".into(),
                    serde_json::json!(5)
                ),
                Step::Set("a".into(), "b".into(), serde_json::json!("x=y")),
            ]
        );
        assert!(steps.iter().all(Step::waits));
        let steps = parse_steps(
            r"format=B:7.1.4;open-format=a;open=com.example.player:C:\clips\joc.ec3;transport=A:play;transport=a:seek,12.5;transport=A:stop;wait=1500",
        )
        .unwrap();
        assert_eq!(
            steps,
            [
                Step::Format(1, "7.1.4".into()),
                Step::OpenFormatList(0),
                Step::Open(
                    "com.example.player".into(),
                    PathBuf::from(r"C:\clips\joc.ec3")
                ),
                Step::Transport(0, TransportCommand::Play, None),
                Step::Transport(0, TransportCommand::Seek, Some(12.5)),
                Step::Transport(0, TransportCommand::Stop, None),
                Step::Wait(1500),
            ]
        );
        // The step that only opens a list does not wait, as it changes nothing to settle on.
        assert!(!Step::OpenFormatList(0).waits());
        for bad in [
            "transport=A:rewind",
            "transport=A:seek,-1",
            "transport=A",
            "wait=soon",
            "open=nocolon",
            "format=C:x",
        ] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
        for bad in [
            "set=a:b",
            "set=a=1",
            "set=a:b=not json",
            "zoom",
            "zoom=-1",
            "select=2",
            "scroll=C:1",
            "spin=1",
            "options=plugins",
            "options=addons:820",
            "options=addons:20x20",
        ] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_picture_keeps_its_size_and_drops_the_premultiplied_alpha() {
        let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        std::fs::create_dir_all(&folder).unwrap();
        let output = folder.join(format!("harness-picture-{}.png", std::process::id()));
        let image = egui::ColorImage::new([3, 2], vec![egui::Color32::from_rgb(10, 20, 30); 6]);
        assert_eq!(write_picture(&output, &image).unwrap(), [3, 2]);
        let png = std::fs::read(&output).unwrap();
        std::fs::remove_file(&output).unwrap();
        assert_eq!(&png[16..24], &[0, 0, 0, 3, 0, 0, 0, 2]);
    }
}

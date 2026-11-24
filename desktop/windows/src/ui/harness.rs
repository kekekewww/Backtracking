//! A test copy of the app (see `startup::test_instance`) can put its window into a known
//! state, photograph it, write how long its frames took and quit. This lets a change to what
//! the window draws be checked without a person at the screen.
//!
//! - `BACKTRACKING_TEST_SCREENSHOT=<file.png>` turns it on.
//! - `BACKTRACKING_TEST_SCREENSHOT_DELAY_MS=<ms>` is how long the app runs first (default 4000).
//! - `BACKTRACKING_TEST_VIEW=<steps>` is done when the delay has passed, before the picture.
//!   Steps are separated by `;`. Times are seconds back from the newest audio, and a lane is
//!   named by its letter, `A` to `BL` in either case, or by its index from 0:
//!   `zoom=<seconds>` shows only that much; `select=<from>-<to>` selects that stretch, as a
//!   drag would; `scroll=<lane>:<row>` scrolls a stacked lane (A is 0) to that row, or, with
//!   more than two lanes, whose rows all scroll as one, the whole plot so that row is on top;
//!   `group=<lane>:<group id>` shows another channel group.
//!   `add-source=<name>` adds the source of that name (or the first whose name contains it,
//!   ignoring case, `Test tone` say) as the next lane, and `remove-source=<lane>` removes a
//!   lane, as the Sources page's Add source and Remove do; both wait until the steps before
//!   them have settled. `meters=open` opens the list of every lane's meter, as the "+N" button
//!   beside the meters does. `options-scroll=<points>` scrolls the Options page that far
//!   down, once the steps before it have settled; much past the end shows an empty page, so
//!   stay near the page's real length.
//!   `separate=<on|off>` edits rows on their own or together, as Right Ctrl does.
//!   `rows=<lane>:<first>-<last>` makes the next `select=` cover those rows of the lane, as a
//!   drag from the first row to the last does; rows count from 1, top down, as in the row
//!   tooltips, and `rows=A:10-B:1` runs from row 10 of lane A to row 1 of lane B.
//!   `select-lane=<lane>` makes it cover every row of that lane. Both need `separate=on`, and
//!   both wait until the steps before them have settled, so they count the rows a `group=`
//!   or `hide-silent=` before them leaves on screen.
//!   `save=<folder>` saves the selection (or everything, without one) into that folder, through
//!   the Save command's own code, waits until the files are written and lists them in the log.
//!   `options=<page>[:<width>x<height>]` opens the Options window on that page (`actions`,
//!   `hotkeys`, `capture`, `sources`, `appearance`, `export`, `addons`, `updates`), optionally
//!   at that size, and makes the picture of the Options window instead of the main window.
//!   `bind=<action id>:<binding>[,<binding>]` gives a shortcut action (named as in the
//!   `hotkeys-v1` settings file, `toggle-capture` say) those bindings through the Hotkeys
//!   page's own code, and keeps them as the page does; `bind=<action id>:none` clears it. A
//!   binding is modifiers and a key joined by `+`, the key named as in that file:
//!   `ctrl+shift+s`, `space`, `alt+key-left-of-1`, `f6`, or `right-ctrl-tap`.
//!   `key=<binding>` presses and releases those keys on the main window, through its input as
//!   a real key press arrives, once the steps before it have settled; `key=right-ctrl-tap` is a
//!   tap of Right Ctrl. A test copy's window never has the keyboard, and a selection's cancel
//!   keys are read only while it does, so they do nothing here.
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
//!   `pair=<stereo|binaural>` sets how the L/R pair of a wide lane sounds, and
//!   `hide-silent=<on|off>` whether system, application and microphone lanes hide the rows of
//!   silent channels, as their Options controls do (for this run only). `preview-plan` writes
//!   to the log what Space would play of the selection and whether it could be prepared; it
//!   never plays it.
//!   `export-mode=<binaural-files|mono-files|multichannel|binaural-mix>` chooses how the lanes
//!   wider than stereo are exported, as the Export page's Multichannel sources list does (for
//!   this run only), once the steps before it have settled, and `open-export-mode` shows that
//!   list open on the Options window.
//!   `drag-prepare=<folder>` writes the files a drag-out of the selection (or of the row the
//!   steps chose, or everything) would carry, through the drag's own code, waits until they are
//!   written, copies them into that folder and lists them in the log. The drag itself, which
//!   Windows runs, is not done; the selection stays, so a `save=` after it writes the same
//!   audio.
//!   `look=<add-on id|none>` chooses that add-on's look, or the window's own, as the
//!   "Look from add-on" row on the Appearance page does, and keeps the choice.
//!   `waveform-color=<single|frequency>` colours the waveform in one colour or by frequency, and
//!   `gradient=<#rrggbb@position,...|default>` makes those stops (2 to 8, positions from 0 for
//!   the lowest frequencies to 1 for the highest, as in the `waveform-gradient-v1` settings
//!   file) the user's own gradient, or puts the default back, as the Appearance page's
//!   "Waveform color" rows do; both keep the choice. The picture waits for the colours, unless
//!   `color-wait=off` came first: then it is taken while a read may be in flight, as a person
//!   watching the live view sees it (`color-wait=on` waits again).
//!
//! The picture is `<file.png>`; `<file.png>.log` gets the frame times and any problem.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use backtracking_addon::protocol::TransportCommand;
use backtracking_core::Lane;
use eframe::egui;

use super::commands::{Action as WindowAction, Ran};
use super::{OptionsSection, Recorder, hotkeys_page};
use crate::controller::{CaptureSource, Controller};
use crate::hotkeys::{Action, Binding};
use crate::preferences::{PairRender, WaveformColoring, WideExportMode, parse_gradient};

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
    /// Every row of a lane is what a selection made after it covers.
    SelectLane(usize),
    /// The rows a selection made after it covers: from row `.1` of lane `.0` to row `.3` of
    /// lane `.2`, counted from zero.
    Rows(usize, usize, usize, usize),
    /// Rows edited on their own, as Right Ctrl switches them.
    Separate(bool),
    /// Saves as the Save command does, into this folder.
    Save(PathBuf),
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
    /// How the pair of a wide lane sounds.
    Pair(PairRender),
    /// Whether lanes that may hide silent rows do.
    HideSilent(bool),
    /// Logs what Space would play, without playing it.
    PreviewPlan,
    /// How lanes wider than stereo are exported.
    ExportMode(WideExportMode),
    /// Shows the Multichannel sources list open on the Options window.
    OpenExportMode,
    /// Writes what a drag-out would carry and copies it into this folder.
    DragPrepare(PathBuf),
    /// Gives a shortcut action these bindings, as the Hotkeys page does.
    Bind(Action, Vec<Binding>),
    /// Presses these keys on the main window, or taps Right Ctrl.
    Key(Binding),
    /// The add-on whose look the window takes, or none.
    Look(Option<String>),
    /// How the waveform is coloured.
    WaveformColor(WaveformColoring),
    /// The user's own gradient for colouring by frequency, or none for the default.
    Gradient(Option<backtracking_ui::Spectrum>),
    /// Whether steps and the picture wait for the colours by frequency in flight.
    ColorWait(bool),
    /// Adds the source of this name as the next lane, as the Sources page's Add source does.
    AddSource(String),
    /// Removes this lane, as its Remove button on the Sources page does.
    RemoveSource(usize),
    /// Opens the list of every lane's meter, as the "+N" button beside the meters does.
    OpenMeters,
    /// Scrolls the Options page this many points down, as the wheel would.
    OptionsScroll(f32),
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
                | Self::Save(_)
                | Self::DragPrepare(_)
                // A drag being prepared is written in the mode it began with.
                | Self::ExportMode(_)
                | Self::Wait(_)
                | Self::PreviewPlan
                // Rows are counted in the lanes as drawn, so a `group=` or `hide-silent=`
                // before them has to reach the screen first.
                | Self::Rows(..)
                | Self::SelectLane(_)
                // Each press gets frames of its own, after what it acts on.
                | Self::Key(_)
                // The lanes change, so the steps after them count the new ones.
                | Self::AddSource(_)
                | Self::RemoveSource(_)
                // The page has to be on show before it can scroll.
                | Self::OptionsScroll(_)
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

/// The source of `choices` whose name is `name`, or else the first whose name contains it,
/// ignoring case.
fn source_named(choices: &[CaptureSource], name: &str) -> Option<CaptureSource> {
    let wanted = name.trim().to_lowercase();
    let label = |source: &&CaptureSource| source.label().to_lowercase();
    choices
        .iter()
        .find(|source| label(source) == wanted)
        .or_else(|| choices.iter().find(|source| label(source).contains(&wanted)))
        .cloned()
}

/// A lane named by its letter, A to BL in either case, or by its index from 0.
fn parse_lane(text: &str) -> Result<usize, String> {
    let text = text.trim();
    text.parse::<usize>()
        .ok()
        .and_then(Lane::from_index)
        .or_else(|| Lane::from_letter(&text.to_ascii_uppercase()))
        .map(Lane::index)
        .ok_or_else(|| format!("{text} is not a lane"))
}

pub(super) fn parse_steps(text: &str) -> Result<Vec<Step>, String> {
    text.split(';')
        .map(str::trim)
        .filter(|step| !step.is_empty())
        .map(|step| {
            if step == "install" {
                return Ok(Step::Install);
            }
            if step == "preview-plan" {
                return Ok(Step::PreviewPlan);
            }
            if step == "open-export-mode" {
                return Ok(Step::OpenExportMode);
            }
            if step == "meters=open" {
                return Ok(Step::OpenMeters);
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
            let lane = parse_lane;
            match name.trim() {
                "zoom" => Ok(Step::Zoom(number(value)?)),
                "select" => {
                    let (from, to) = value
                        .split_once('-')
                        .ok_or_else(|| format!("{step} needs <from>-<to>"))?;
                    Ok(Step::Select(number(from)?, number(to)?))
                }
                "select-lane" => Ok(Step::SelectLane(lane(value)?)),
                "rows" => {
                    let (first_lane, rows) = pair(step, value)?;
                    let (first, last) = rows
                        .split_once('-')
                        .ok_or_else(|| format!("{step} needs <lane>:<first>-<last>"))?;
                    let (last_lane, last) = match last.split_once(':') {
                        Some((last_lane, last)) => (lane(last_lane)?, last),
                        None => (lane(first_lane)?, last),
                    };
                    let row = |text: &str| {
                        text.trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|row| row.checked_sub(1))
                            .ok_or_else(|| format!("{text} is not a row counted from 1"))
                    };
                    Ok(Step::Rows(
                        lane(first_lane)?,
                        row(first)?,
                        last_lane,
                        row(last)?,
                    ))
                }
                "save" => Ok(Step::Save(PathBuf::from(value.trim()))),
                "drag-prepare" => Ok(Step::DragPrepare(PathBuf::from(value.trim()))),
                "export-mode" => WideExportMode::ALL
                    .into_iter()
                    .find(|mode| mode.preference() == value.trim())
                    .map(Step::ExportMode)
                    .ok_or_else(|| {
                        format!(
                            "{value} is not binaural-files, mono-files, multichannel or binaural-mix"
                        )
                    }),
                "separate" => match value.trim() {
                    "on" => Ok(Step::Separate(true)),
                    "off" => Ok(Step::Separate(false)),
                    other => Err(format!("{other} is not on or off")),
                },
                "hide-silent" => match value.trim() {
                    "on" => Ok(Step::HideSilent(true)),
                    "off" => Ok(Step::HideSilent(false)),
                    other => Err(format!("{other} is not on or off")),
                },
                "pair" => PairRender::from_preference(value.trim())
                    .map(Step::Pair)
                    .ok_or_else(|| format!("{value} is not stereo or binaural")),
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
                "bind" => {
                    let (id, bindings) = value
                        .split_once(':')
                        .ok_or_else(|| format!("{step} needs <action id>:<binding>"))?;
                    let action = hotkeys_page::parse_action(id)?;
                    let bindings = if bindings.trim() == "none" {
                        Vec::new()
                    } else {
                        bindings
                            .split(',')
                            .map(hotkeys_page::parse_binding)
                            .collect::<Result<_, _>>()?
                    };
                    Ok(Step::Bind(action, bindings))
                }
                "key" => Ok(Step::Key(hotkeys_page::parse_binding(value)?)),
                "look" => match value.trim() {
                    "" => Err(format!("{step} needs an add-on id or none")),
                    "none" => Ok(Step::Look(None)),
                    id => Ok(Step::Look(Some(id.to_owned()))),
                },
                "waveform-color" => WaveformColoring::from_preference(value.trim())
                    .map(Step::WaveformColor)
                    .ok_or_else(|| format!("{value} is not single or frequency")),
                "gradient" => match value.trim() {
                    "default" => Ok(Step::Gradient(None)),
                    stops => parse_gradient(stops)
                        .map(|gradient| Step::Gradient(Some(gradient)))
                        .ok_or_else(|| {
                            format!("{stops} is not 2 to 8 stops like #e8302c@0,#2c68ec@1")
                        }),
                },
                "color-wait" => match value.trim() {
                    "on" => Ok(Step::ColorWait(true)),
                    "off" => Ok(Step::ColorWait(false)),
                    other => Err(format!("{other} is not on or off")),
                },
                "add-source" => match value.trim() {
                    "" => Err(format!("{step} needs the name of a source")),
                    name => Ok(Step::AddSource(name.to_owned())),
                },
                "remove-source" => Ok(Step::RemoveSource(lane(value)?)),
                "options-scroll" => value
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|points| points.is_finite() && *points >= 0.0)
                    .map(Step::OptionsScroll)
                    .ok_or_else(|| format!("{value} is not a number of points")),
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
    /// Key presses and releases a `key=` step hands the main window.
    main_keys: Vec<egui::Event>,
    /// Taps of Right Ctrl a `key=right-ctrl-tap` step made, for the window to take.
    taps: usize,
    /// Waits for the Options window's next pass to take its picture.
    options_shot: Option<mpsc::SyncSender<PictureResult>>,
    /// Extra time the last step asked for before the next one.
    hold: Duration,
    /// A `drag-prepare=` waits for the drag's files and copies them here.
    drag_copy: Option<PathBuf>,
    /// Steps and the picture wait for the colours by frequency in flight (`color-wait=`).
    color_wait: bool,
}

impl Harness {
    /// The size the Options window is created at, when a step asked for one.
    pub(super) fn options_size(&self) -> Option<egui::Vec2> {
        self.options_size
    }

    /// Hands files the steps dropped on the main window, and keys they pressed, to egui, in its
    /// raw input.
    pub(super) fn drop_on_main(&mut self, input: &mut egui::RawInput) {
        for path in self.main_drops.drain(..) {
            input.dropped_files.push(Arc::new(DroppedForTest(path)));
        }
        input.events.append(&mut self.main_keys);
    }

    /// Whether a step tapped Right Ctrl since the last call, as `lane_key` reports a tap.
    pub(super) fn take_tap(&mut self) -> bool {
        let tapped = self.taps > 0;
        self.taps = self.taps.saturating_sub(1);
        tapped
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
            main_keys: Vec::new(),
            taps: 0,
            options_shot: None,
            hold: Duration::ZERO,
            drag_copy: None,
            color_wait: true,
        })
    }

    /// Lists the files a save wrote in the log, with what their headers say.
    pub(super) fn note_saved(&mut self, paths: &[PathBuf]) {
        for path in paths {
            let header = fs::read(path).ok().and_then(|bytes| wav_header(&bytes));
            self.notes.push(match header {
                Some(header) => format!(
                    "saved {}: {} channels, {} Hz, {} bits, {} frames ({:.3} s)",
                    path.display(),
                    header.channels,
                    header.rate,
                    header.bits,
                    header.frames,
                    header.frames as f64 / f64::from(header.rate.max(1)),
                ),
                None => format!("saved {} (not a WAV file this can read)", path.display()),
            });
        }
    }

    /// Puts a line in the log.
    pub(super) fn note(&mut self, text: String) {
        self.notes.push(text);
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
        // A `drag-prepare=` copies the drag's files once they are written.
        if harness.drag_copy.is_some()
            && self.export.is_none()
            && !self.export_requested
            && self.edit_request.is_none()
        {
            let folder = harness.drag_copy.take();
            if let Some(folder) = folder {
                self.copy_drag_files(&folder);
            }
            ctx.request_repaint();
            return;
        }
        let Some(harness) = self.harness.as_mut() else {
            return;
        };
        let settled = now.duration_since(applied) >= SETTLE + harness.hold
            && self.edit_request.is_none()
            && self.edit_preview.is_none()
            // Colours by frequency are read off the window's thread.
            && (!harness.color_wait || !self.waveform_colors.busy())
            // Dropped files are read in the next pass, and the manager answers after that.
            && harness.main_drops.is_empty()
            && harness.main_keys.is_empty()
            && harness.options_drops.is_empty()
            && !self.addons_page.waiting()
            // A `save=` waits for its files to be written.
            && self.snapshot_save.is_none()
            && self.pending_snapshot_saves.is_empty()
            && self.pending_snapshot_reads.is_empty()
            && self.pending_edit_save_reads.is_empty()
            // A `drag-prepare=` waits for the drag's files.
            && self.export.is_none()
            && !self.export_requested;
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
        // The steps that an add-on's control requests can also do go through one layer.
        match step {
            &Step::Zoom(seconds) => {
                self.run_action(WindowAction::ZoomNewest(seconds))?;
            }
            &Step::Select(from, to) => {
                self.run_action(WindowAction::SelectBack { from, to })?;
            }
            &Step::SelectLane(lane) => {
                self.run_action(WindowAction::ChooseLane(lane))?;
            }
            &Step::Rows(first_lane, first, last_lane, last) => {
                self.run_action(WindowAction::ChooseRows(first_lane, first, last_lane, last))?;
            }
            &Step::Separate(on) => {
                self.run_action(WindowAction::Separate(on))?;
            }
            Step::Save(folder) => {
                let saved = self.run_action(WindowAction::Save {
                    folder: folder.clone(),
                    tag: None,
                });
                // A folder that cannot be made is the step's error. A save that cannot start
                // says why on the status line, which the log keeps, as it always did.
                if let Err(refusal) = saved
                    && !folder.is_dir()
                {
                    return Err(refusal.message);
                }
                ctx.request_repaint();
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
            &Step::Pair(render) => {
                self.pair_render = render;
                self.invalidate_export();
            }
            &Step::ExportMode(mode) => {
                self.wide_export = mode;
                self.invalidate_export();
            }
            Step::OpenExportMode => {
                self.force_wide_export_menu = true;
                ctx.request_repaint();
            }
            Step::DragPrepare(folder) => {
                fs::create_dir_all(folder)
                    .map_err(|error| format!("{}: {error}", folder.display()))?;
                self.run_action(WindowAction::DragPrepare)?;
                if let Some(harness) = self.harness.as_mut() {
                    harness.drag_copy = Some(folder.clone());
                }
                ctx.request_repaint();
            }
            &Step::HideSilent(on) => {
                self.hide_silent = on;
                self.live_revision = None;
                ctx.request_repaint();
            }
            Step::Bind(action, bindings) => {
                let global = hotkeys_page::global(self.hotkey.as_ref());
                let result =
                    self.hotkeys_page
                        .set_bindings(*action, bindings, &mut self.hotkeys, global);
                result.map_err(|error| super::Language::current().hotkey_problem(error))?;
                self.save_hotkeys();
                ctx.request_repaint();
            }
            &Step::Key(binding) => {
                let harness = self.harness.as_mut().ok_or("the harness has finished")?;
                match binding {
                    Binding::RightCtrlTap => harness.taps += 1,
                    Binding::Chord { modifiers, key } => {
                        for pressed in [true, false] {
                            harness
                                .main_keys
                                .push(hotkeys_page::key_event(modifiers, key, pressed));
                        }
                    }
                }
                ctx.request_repaint();
            }
            Step::PreviewPlan => {
                if let Ran::Note(note) = self.run_action(WindowAction::PreviewPlan)?
                    && let Some(harness) = self.harness.as_mut()
                {
                    harness.notes.push(note);
                }
            }
            Step::Look(choice) => {
                self.choose_look(choice.clone());
                ctx.request_repaint();
            }
            &Step::WaveformColor(coloring) => {
                self.set_waveform_coloring(coloring);
                ctx.request_repaint();
            }
            Step::Gradient(gradient) => {
                self.set_waveform_gradient(gradient.clone());
                self.save_waveform_gradient();
                ctx.request_repaint();
            }
            &Step::ColorWait(wait) => {
                if let Some(harness) = self.harness.as_mut() {
                    harness.color_wait = wait;
                }
            }
            Step::AddSource(name) => {
                // The Sources page lists what it last scanned; a name it has not seen yet is
                // looked for in a fresh scan, as opening the list does.
                let source = match source_named(&self.available_sources, name) {
                    Some(source) => source,
                    None => {
                        let listed = match self.controller.addons() {
                            Some(addons) => {
                                Controller::available_sources_with_addons(&addons.snapshot())
                            }
                            None => Controller::available_sources(),
                        };
                        let found = source_named(&listed, name);
                        self.available_sources = listed;
                        found.ok_or_else(|| format!("no source is called {name}"))?
                    }
                };
                self.run_action(WindowAction::AddSource(source))?;
                ctx.request_repaint();
            }
            &Step::RemoveSource(lane) => {
                self.run_action(WindowAction::RemoveSource(lane))?;
                ctx.request_repaint();
            }
            &Step::OptionsScroll(points) => {
                if !self.options_open {
                    return Err("the Options window is not open".into());
                }
                self.options_scroll = Some(points);
                ctx.request_repaint();
            }
            Step::OpenMeters => {
                if self.lane_count() <= 2 {
                    return Err("the meters all show with one or two sources".into());
                }
                if !self.meter_list_open {
                    self.toggle_meter_list();
                }
                ctx.request_repaint();
            }
        }
        Ok(())
    }

    /// Copies the files a drag-out would carry into `folder` and lists them in the log, with
    /// the status line the drag would leave. The drag's own files are removed as usual when the
    /// prepared drag is dropped.
    fn copy_drag_files(&mut self, folder: &Path) {
        let Some(file) = self.exported.as_ref() else {
            let note = format!("drag-prepare: nothing was prepared ({})", self.message);
            if let Some(harness) = self.harness.as_mut() {
                harness.note(note);
            }
            return;
        };
        let mut copied = Vec::new();
        let mut notes = Vec::new();
        for path in &file.paths {
            let Some(name) = path.file_name() else {
                continue;
            };
            let target = folder.join(name);
            match fs::copy(path, &target) {
                Ok(_) => copied.push(target),
                Err(error) => notes.push(format!("drag-prepare: {}: {error}", path.display())),
            }
        }
        let status = file.saved_message(super::Language::current(), false);
        if let Some(harness) = self.harness.as_mut() {
            harness.note(format!("drag-prepare: {} files", file.paths.len()));
            harness.note_saved(&copied);
            for note in notes {
                harness.note(note);
            }
            harness.note(format!("drag status would be: {status}"));
        }
    }

    /// What Space would play now, for the log: the plan lane by lane, and whether the preview
    /// could be prepared. The preview made is dropped unplayed.
    pub(super) fn describe_preview(&mut self) -> String {
        let Some(selection) = self.selection.clone() else {
            return "preview: no selection".into();
        };
        let Some(edit) = self.editing.as_ref() else {
            return "preview: the selection is not frozen".into();
        };
        let plan = super::preview_plan(
            &edit.snapshots,
            &edit.lanes,
            &edit.groups,
            &edit.shown_groups,
            self.routes(),
            self.selected_rows(),
        );
        let parts = match plan {
            Ok(parts) => parts,
            Err(error) => return format!("preview: {error}"),
        };
        let described: Vec<String> = parts
            .iter()
            .map(|part| {
                format!(
                    "snapshot {} route {:?} group {:?} selected {:?} positions {}",
                    part.index, part.route, part.channels, part.selected, part.positions
                )
            })
            .collect();
        let prepared = match self.preview_of(&parts, selection.clone()) {
            Ok(preview) => {
                drop(preview);
                "prepared, not played"
            }
            Err(error) => error,
        };
        format!(
            "preview of {}..{} ({:?}): {}; {prepared}",
            selection.start,
            selection.end,
            self.pair_render,
            described.join("; ")
        )
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

#[derive(Debug, PartialEq, Eq)]
pub(super) struct WavHeader {
    pub(super) channels: u16,
    pub(super) rate: u32,
    pub(super) bits: u16,
    pub(super) frames: u64,
}

/// Reads the format and length of a RIFF or RF64 WAV file. A control `save` lists the files
/// it wrote from these headers too.
pub(super) fn wav_header(bytes: &[u8]) -> Option<WavHeader> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let u64_at = |at: usize| Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?));
    let riff = bytes.get(..4)?;
    if (riff != b"RIFF" && riff != b"RF64") || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut format, mut data, mut big_data) = (None, None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(at + 4)? as usize;
        match id {
            b"fmt " => format = Some((u16_at(at + 10)?, u32_at(at + 12)?, u16_at(at + 22)?)),
            b"ds64" => big_data = Some(u64_at(at + 16)?),
            b"data" => {
                data = Some(if size == u32::MAX as usize {
                    big_data?
                } else {
                    size as u64
                });
                break;
            }
            _ => {}
        }
        at += 8 + size + size % 2;
    }
    let (channels, rate, bits) = format?;
    let frame = u64::from(channels) * u64::from(bits / 8);
    Some(WavHeader {
        channels,
        rate,
        bits,
        frames: data? / frame.max(1),
    })
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
        data.extend_from_within(..5000);
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
    fn look_steps_choose_an_add_on_or_none() {
        assert_eq!(
            parse_steps("look=com.example.theme; look=none ;options=appearance").unwrap(),
            [
                Step::Look(Some("com.example.theme".into())),
                Step::Look(None),
                Step::Options(OptionsSection::Appearance, None),
            ]
        );
        assert!(!Step::Look(None).waits());
        for bad in ["look", "look=", "look= "] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn waveform_color_steps_choose_the_colouring_and_the_gradient() {
        use backtracking_ui::{Spectrum, SpectrumStop};
        let stop = |position, [r, g, b]: [u8; 3]| SpectrumStop {
            position,
            color: egui::Color32::from_rgb(r, g, b),
        };
        assert_eq!(
            parse_steps(
                "waveform-color=frequency; gradient=#e8302c@0,#28c85a@0.5,#2c68ec@1; \
                 gradient=default;waveform-color=single;color-wait=off;color-wait=on"
            )
            .unwrap(),
            [
                Step::WaveformColor(WaveformColoring::Frequency),
                Step::Gradient(Some(
                    Spectrum::new([
                        stop(0.0, [232, 48, 44]),
                        stop(0.5, [40, 200, 90]),
                        stop(1.0, [44, 104, 236]),
                    ])
                    .unwrap()
                )),
                Step::Gradient(None),
                Step::WaveformColor(WaveformColoring::Single),
                Step::ColorWait(false),
                Step::ColorWait(true),
            ]
        );
        assert!(!Step::Gradient(None).waits());
        for bad in [
            "waveform-color=rainbow",
            "gradient=",
            "gradient=#e8302c@0",
            "gradient=red@0,blue@1",
            "color-wait=maybe",
        ] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn lanes_are_named_by_letter_up_to_bl_or_by_index() {
        for (text, lane) in [
            ("A", 0),
            ("b", 1),
            ("C", 2),
            ("z", 25),
            ("AA", 26),
            ("ab", 27),
            ("BL", 63),
            (" bl ", 63),
            ("0", 0),
            ("2", 2),
            ("63", 63),
        ] {
            assert_eq!(parse_lane(text), Ok(lane), "{text}");
        }
        for bad in ["", "BM", "64", "-1", "A1", "Lane A", "ZZ"] {
            assert!(parse_lane(bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse_steps("separate=on;select-lane=E;rows=C:2-BL:1;scroll=AB:3").unwrap(),
            [
                Step::Separate(true),
                Step::SelectLane(4),
                Step::Rows(2, 1, 63, 0),
                Step::Scroll(27, 3),
            ]
        );
    }

    #[test]
    fn source_list_steps_add_and_remove_lanes_and_open_the_meters() {
        let steps = parse_steps(
            "add-source= Test tone ;remove-source=B;meters=open;options=sources;options-scroll=400",
        )
        .unwrap();
        assert_eq!(
            steps,
            [
                Step::AddSource("Test tone".into()),
                Step::RemoveSource(1),
                Step::OpenMeters,
                Step::Options(OptionsSection::Sources, None),
                Step::OptionsScroll(400.0),
            ]
        );
        assert!(steps[0].waits() && steps[1].waits() && steps[4].waits());
        assert!(!steps[2].waits());
        for bad in ["add-source=", "remove-source=BM", "options-scroll=-3", "meters=shut"] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
        let tone = CaptureSource::Addon {
            addon_id: "com.example.test-tone".into(),
            source_id: "main".into(),
            name: "Test tone".into(),
        };
        let choices = [CaptureSource::System, tone.clone()];
        assert_eq!(source_named(&choices, "test TONE"), Some(tone.clone()));
        assert_eq!(source_named(&choices, "tone"), Some(tone));
        assert_eq!(source_named(&choices, "microphone"), None);
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
        assert_eq!(
            parse_steps("separate=on;select-lane=B;select=3-1").unwrap(),
            [
                Step::Separate(true),
                Step::SelectLane(1),
                Step::Select(3.0, 1.0)
            ]
        );
        assert_eq!(
            parse_steps(r"rows=A:3-5;rows=a:10-B:1;save=D:\out\rows").unwrap(),
            [
                Step::Rows(0, 2, 0, 4),
                Step::Rows(0, 9, 1, 0),
                Step::Save(PathBuf::from(r"D:\out\rows")),
            ]
        );
        assert!(Step::Save(PathBuf::new()).waits());
        // Rows are counted as drawn, so they wait for a group switch before them.
        assert!(Step::Rows(0, 0, 0, 0).waits());
        assert!(Step::SelectLane(0).waits());
        for bad in [
            "rows=A:0-2",
            "rows=A:3",
            "rows=3-5",
            "rows=A:1-BM:2",
            "rows=A:x-2",
        ] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
        assert!(parse_steps("separate=maybe").is_err());
        assert_eq!(
            parse_steps("pair=binaural; hide-silent=off; pair=stereo;hide-silent=on;preview-plan")
                .unwrap(),
            [
                Step::Pair(PairRender::Binaural),
                Step::HideSilent(false),
                Step::Pair(PairRender::Stereo),
                Step::HideSilent(true),
                Step::PreviewPlan,
            ]
        );
        assert!(Step::PreviewPlan.waits());
        assert!(!Step::Pair(PairRender::Binaural).waits());
        for bad in ["pair=surround", "pair", "hide-silent=yes"] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse_steps(
                "export-mode=binaural-files;export-mode=mono-files; export-mode=multichannel;\
                 export-mode=binaural-mix;open-export-mode;drag-prepare=D:\\a b"
            )
            .unwrap(),
            [
                Step::ExportMode(WideExportMode::BinauralFiles),
                Step::ExportMode(WideExportMode::MonoFiles),
                Step::ExportMode(WideExportMode::Multichannel),
                Step::ExportMode(WideExportMode::BinauralMix),
                Step::OpenExportMode,
                Step::DragPrepare(PathBuf::from("D:\\a b")),
            ]
        );
        assert!(Step::ExportMode(WideExportMode::MonoFiles).waits());
        assert!(Step::DragPrepare(PathBuf::new()).waits());
        for bad in [
            "export-mode=mono",
            "export-mode",
            "export-mode=Multichannel",
        ] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
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
            "format=BM:x",
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
            "scroll=BM:1",
            "spin=1",
            "options=plugins",
            "options=addons:820",
            "options=addons:20x20",
        ] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn hotkey_steps_name_an_action_and_keys_as_the_settings_file_does() {
        use crate::hotkeys::{Key, Modifiers};
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        };
        let bare = |key| Binding::Chord {
            key,
            modifiers: Modifiers::NONE,
        };
        assert_eq!(
            parse_steps(
                "options=hotkeys; bind=toggle-capture:f6;bind=cancel-selection:escape, ctrl+backspace;\
                 bind=clear:none;key=f6;key=right-ctrl-tap;key=Alt+key-left-of-1"
            )
            .unwrap(),
            [
                Step::Options(OptionsSection::Hotkeys, None),
                Step::Bind(Action::ToggleCapture, vec![bare(Key::F6)]),
                Step::Bind(
                    Action::CancelSelection,
                    vec![
                        bare(Key::Escape),
                        Binding::Chord {
                            key: Key::Backspace,
                            modifiers: ctrl
                        }
                    ]
                ),
                Step::Bind(Action::Clear, Vec::new()),
                Step::Key(bare(Key::F6)),
                Step::Key(Binding::RightCtrlTap),
                Step::Key(Binding::Chord {
                    key: Key::KeyLeftOf1,
                    modifiers: Modifiers {
                        alt: true,
                        ..Modifiers::NONE
                    }
                }),
            ]
        );
        // A press acts on what came before it; a binding changes nothing that has to settle.
        assert!(Step::Key(bare(Key::F6)).waits());
        assert!(!Step::Bind(Action::Quit, Vec::new()).waits());
        for bad in [
            "bind=toggle-capture",
            "bind=toggle:f6",
            "bind=quit:ctrl+",
            "bind=quit:f6,",
            "key=banana",
            "key=",
            "key=hyper+a",
        ] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn saved_files_are_read_back_from_their_wav_headers() {
        let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
        // An odd-sized chunk before the format is padded to an even length.
        wav.extend_from_slice(b"LIST\x03\0\0\0abc\0");
        wav.extend_from_slice(b"fmt \x10\0\0\0");
        wav.extend_from_slice(&3u16.to_le_bytes());
        wav.extend_from_slice(&6u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&(48_000u32 * 24).to_le_bytes());
        wav.extend_from_slice(&24u16.to_le_bytes());
        wav.extend_from_slice(&32u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(24u32 * 10).to_le_bytes());
        wav.extend_from_slice(&[0; 240]);
        assert_eq!(
            wav_header(&wav),
            Some(WavHeader {
                channels: 6,
                rate: 48_000,
                bits: 32,
                frames: 10
            })
        );
        assert_eq!(wav_header(b"RIFF\0\0\0\0AVI "), None);
        assert_eq!(wav_header(&wav[..40]), None, "no data chunk");
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

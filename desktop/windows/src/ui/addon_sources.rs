//! What an add-on source adds to the Sources section of the Options window: its formats, a
//! file to open, transport, and what to do when it is not there. The main window's compact
//! transport reads from the same state.
//!
//! Every call to the add-on manager returns a receiver. This keeps the receivers and looks at
//! each one once per frame with `try_recv`, so nothing here waits on the UI thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use backtracking_addon::manifest::{Capability, OpenFiles};
use backtracking_addon::protocol::{
    MessageLevel, Source, Transport, TransportCommand, TransportState,
};
use backtracking_ui::{
    OPTIONS_VALUE_TEXT, configure_options_menu, options_control_width, options_field,
    options_menu_height, options_note,
};
use eframe::egui::{self, Color32, Stroke};

use super::addons_page::{
    ERROR_TEXT, PROBLEM, Tone, is_package, problem_line, state_line, state_words,
};
use super::text::Language;
use crate::addons::{AddonHandle, AddonInfo, AddonReply, AddonSnapshot, AddonState};

/// After a seek the add-on is silent for a moment while it prepares the new position. The
/// position that was asked for stays on screen for at most this long.
const SEEK_SETTLE: Duration = Duration::from_secs(3);
/// A chosen format stays on screen until the add-on describes it, for at most this long.
const FORMAT_SETTLE: Duration = Duration::from_secs(5);
/// The setting that makes the Atmos add-on play through the speakers. A test copy refuses to
/// open a file while it is on or unknown, so a test never makes sound.
const SPEAKER_SETTING: &str = "play_through_speakers";

/// An add-on and one of its sources.
type Slot = (String, String);

/// `1:05` or, from an hour on, `1:02:05`.
pub(super) fn clock(seconds: f64) -> String {
    let total = if seconds.is_finite() {
        seconds.max(0.0).floor() as u64
    } else {
        0
    };
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Where a lane's add-on source stands.
#[derive(Debug)]
pub(super) enum SourceState<'a> {
    Ready {
        addon: &'a AddonInfo,
        source: &'a Source,
    },
    NotInstalled,
    /// Installed but not running: off, starting, stopped, failed or not responding.
    NotRunning(&'a AddonInfo),
    /// The add-on runs but no longer offers this source.
    Gone(&'a AddonInfo),
}

pub(super) fn source_state<'a>(
    snapshot: &'a AddonSnapshot,
    addon_id: &str,
    source_id: &str,
) -> SourceState<'a> {
    let Some(addon) = snapshot.addon(addon_id) else {
        return SourceState::NotInstalled;
    };
    if addon.state != AddonState::Running {
        return SourceState::NotRunning(addon);
    }
    match addon.sources.iter().find(|source| source.id == source_id) {
        Some(source) => SourceState::Ready { addon, source },
        None => SourceState::Gone(addon),
    }
}

/// The extension of a path, lower case and without the dot.
fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

/// The file types an add-on opens, when it declared the capability.
pub(super) fn open_files(addon: &AddonInfo) -> Option<&OpenFiles> {
    let manifest = addon.manifest.as_ref()?;
    manifest
        .capabilities
        .contains(&Capability::OpenFiles)
        .then_some(manifest.open_files.as_ref())
        .flatten()
}

fn accepts(files: &OpenFiles, path: &Path) -> bool {
    extension(path).is_some_and(|wanted| {
        files.extensions.iter().any(|extension| {
            extension
                .trim_start_matches('.')
                .eq_ignore_ascii_case(&wanted)
        })
    })
}

/// A running add-on that opens a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Opener {
    pub(super) id: String,
    pub(super) name: String,
}

fn name_of(addon: &AddonInfo) -> String {
    addon
        .manifest
        .as_ref()
        .map_or_else(|| addon.id.clone(), |manifest| manifest.name.clone())
}

/// The running add-ons that open this file's type, in the manager's order.
pub(super) fn openers(snapshot: &AddonSnapshot, path: &Path) -> Vec<Opener> {
    snapshot
        .addons
        .iter()
        .filter(|addon| addon.state == AddonState::Running)
        .filter(|addon| open_files(addon).is_some_and(|files| accepts(files, path)))
        .map(|addon| Opener {
            id: addon.id.clone(),
            name: name_of(addon),
        })
        .collect()
}

/// What a dropped file leads to.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DropPlan {
    /// Not a file any add-on opens.
    Ignore,
    Open(Opener, PathBuf),
    /// More than one add-on opens it; the user picks.
    Choose(PathBuf, Vec<Opener>),
    /// An add-on opens it but is not running.
    NotRunning(String),
}

/// Decides what to do with the files dropped on a window. Add-on packages are the Add-ons
/// page's business and are skipped here. The first file some add-on opens decides.
pub(super) fn plan_drop(snapshot: &AddonSnapshot, paths: &[PathBuf]) -> DropPlan {
    for path in paths.iter().filter(|path| !is_package(path)) {
        let mut found = openers(snapshot, path);
        match found.len() {
            0 => {
                let idle = snapshot.addons.iter().find(|addon| {
                    addon.state != AddonState::Running
                        && open_files(addon).is_some_and(|files| accepts(files, path))
                });
                if let Some(addon) = idle {
                    return DropPlan::NotRunning(name_of(addon));
                }
            }
            1 => return DropPlan::Open(found.remove(0), path.clone()),
            _ => return DropPlan::Choose(path.clone(), found),
        }
    }
    DropPlan::Ignore
}

/// The add-on's setting that would play a file through the speakers, when it has one and it is
/// not known to be off.
fn speakers_may_be_on(addon: &AddonInfo) -> bool {
    match addon
        .settings
        .iter()
        .find(|setting| setting.declaration.key == SPEAKER_SETTING)
    {
        Some(setting) => setting.value != serde_json::Value::Bool(false),
        None => false,
    }
}

/// A position the user asked for, which the add-on takes a moment to reach.
#[derive(Clone, Copy, Debug)]
struct Seeking {
    target: f64,
    at: Instant,
}

impl Seeking {
    /// True until the add-on reports a position near the target, or the wait runs out.
    fn pending(&self, reported: f64, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < SEEK_SETTLE && (reported - self.target).abs() > 1.0
    }
}

/// The position a slider is dragged to. It is sent once, when the user lets go; a click or a
/// key press has no drag, so it is sent at once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SliderDraft(Option<f64>);

impl SliderDraft {
    /// `changed`: the slider moved this frame. `held`: the pointer is still down on it.
    fn update(&mut self, value: f64, changed: bool, held: bool) -> Option<f64> {
        if changed {
            self.0 = Some(value);
        }
        if held { None } else { self.0.take() }
    }
}

enum Purpose {
    Format { slot: Slot, label: String },
    Open { file: String, addon: String },
    Transport,
}

struct Request {
    addon_id: String,
    purpose: Purpose,
    reply: AddonReply<()>,
}

/// Which add-on is asked to open a dropped file.
pub(super) struct Question {
    path: PathBuf,
    openers: Vec<Opener>,
}

/// What the Sources section asks the window to do after it is drawn.
#[derive(Default)]
pub(super) struct LaneOutcome {
    /// The usual route control still applies to this source.
    pub(super) show_route: bool,
    /// "Open file…" was pressed, for this add-on.
    pub(super) open_file: Option<String>,
    /// "Open the Add-ons page" was pressed.
    pub(super) add_ons_page: bool,
    /// The source is not there, and this has said why: the capture status would repeat it.
    pub(super) covered: bool,
}

/// What a drop did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DropResult {
    Ignored,
    /// The file was sent to an add-on; the answer arrives with `poll`.
    Opening,
    /// The Sources section asks which add-on.
    Choose,
    Message(String),
}

/// The compact transport on the main window.
pub(super) struct Chip {
    pub(super) playing: bool,
    pub(super) text: String,
    pub(super) source: String,
}

#[derive(Default)]
pub(super) struct AddonSources {
    snapshot: Arc<AddonSnapshot>,
    requests: Vec<Request>,
    /// The latest refusal, by add-on id.
    problems: HashMap<String, String>,
    formats: HashMap<Slot, (String, Instant)>,
    seeks: HashMap<Slot, Seeking>,
    drafts: HashMap<Slot, SliderDraft>,
    question: Option<Question>,
    /// A test asked for this lane's format list to be open.
    force_format_menu: Option<usize>,
    /// A test copy never opens a file while the add-on may play it through the speakers.
    guard_speakers: bool,
}

fn reply_state<T>(reply: &AddonReply<T>) -> Option<Result<T, String>> {
    match reply.try_recv() {
        Ok(result) => Some(result),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => {
            Some(Err("The add-on manager stopped answering".into()))
        }
    }
}

impl AddonSources {
    pub(super) fn new(guard_speakers: bool) -> Self {
        Self {
            guard_speakers,
            ..Self::default()
        }
    }

    pub(super) fn waiting(&self) -> bool {
        !self.requests.is_empty()
    }

    pub(super) fn force_format_menu(&mut self, lane: usize) {
        self.force_format_menu = Some(lane);
    }

    /// Takes the newest snapshot and every reply that has arrived. Returns a line for the
    /// window's status when a file was opened or could not be. `in_use` says whether a lane
    /// already captures from the add-on.
    pub(super) fn poll(
        &mut self,
        handle: Option<&AddonHandle>,
        language: Language,
        in_use: impl Fn(&str) -> bool,
    ) -> Option<String> {
        if let Some(snapshot) = handle.and_then(AddonHandle::try_snapshot) {
            self.snapshot = snapshot;
        }
        let mut message = None;
        let mut still_waiting = Vec::with_capacity(self.requests.len());
        for request in std::mem::take(&mut self.requests) {
            let Some(result) = reply_state(&request.reply) else {
                still_waiting.push(request);
                continue;
            };
            let id = request.addon_id;
            match (request.purpose, result) {
                (Purpose::Format { .. } | Purpose::Transport, Ok(())) => {
                    self.problems.remove(&id);
                }
                (Purpose::Format { slot, label, .. }, Err(reason)) => {
                    self.formats.remove(&slot);
                    self.problems
                        .insert(id, language.could_not_choose_format(&label, &reason));
                }
                (Purpose::Transport, Err(reason)) => {
                    self.problems.insert(id, reason);
                }
                (Purpose::Open { file, addon }, Ok(())) => {
                    self.problems.remove(&id);
                    let mut text = language.opened_file(&file, &addon);
                    if !in_use(&id)
                        && let Some(source) = self
                            .snapshot
                            .addon(&id)
                            .and_then(|addon| addon.sources.first())
                    {
                        text.push(' ');
                        text.push_str(&language.choose_its_source(&source.name));
                    }
                    message = Some(text);
                }
                (Purpose::Open { file, .. }, Err(reason)) => {
                    let text = language.could_not_open_file(&file, &reason);
                    self.problems.insert(id, text.clone());
                    message = Some(text);
                }
            }
        }
        self.requests = still_waiting;
        // A format the add-on now reports needs no override.
        let snapshot = Arc::clone(&self.snapshot);
        self.formats.retain(|slot, (format, at)| {
            at.elapsed() < FORMAT_SETTLE
                && snapshot
                    .source(&slot.0, &slot.1)
                    .is_some_and(|source| source.format != *format)
        });
        message
    }

    pub(super) fn select_format(
        &mut self,
        handle: &AddonHandle,
        addon_id: &str,
        source_id: &str,
        format_id: &str,
    ) -> Result<(), String> {
        let source = self
            .snapshot
            .source(addon_id, source_id)
            .ok_or("The source is not available")?;
        let label = source
            .formats
            .iter()
            .find(|format| format.id == format_id)
            .map(|format| format.label.clone())
            .ok_or("The add-on does not offer this format")?;
        let slot = (addon_id.to_owned(), source_id.to_owned());
        self.problems.remove(addon_id);
        self.formats
            .insert(slot.clone(), (format_id.to_owned(), Instant::now()));
        self.requests.push(Request {
            addon_id: addon_id.to_owned(),
            purpose: Purpose::Format { slot, label },
            reply: handle.select_format(addon_id, source_id, format_id),
        });
        Ok(())
    }

    /// Sends a file to an add-on. A test copy first checks that the add-on will not play it
    /// through the speakers.
    pub(super) fn open_file(
        &mut self,
        handle: &AddonHandle,
        addon_id: &str,
        path: PathBuf,
    ) -> Result<(), String> {
        let addon = self
            .snapshot
            .addon(addon_id)
            .ok_or("The add-on is not installed")?;
        if self.guard_speakers && open_files(addon).is_some() && speakers_may_be_on(addon) {
            return Err(format!(
                "a test copy does not open files while {SPEAKER_SETTING} is on or not yet known"
            ));
        }
        let file = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let name = name_of(addon);
        self.problems.remove(addon_id);
        self.requests.push(Request {
            addon_id: addon_id.to_owned(),
            purpose: Purpose::Open { file, addon: name },
            reply: handle.open_file(addon_id, path),
        });
        Ok(())
    }

    /// Routes files dropped on a window to the add-on that opens them. Add-on packages are left
    /// to the Add-ons page.
    pub(super) fn drop_files(
        &mut self,
        handle: Option<&AddonHandle>,
        paths: &[PathBuf],
        language: Language,
    ) -> DropResult {
        let Some(handle) = handle else {
            return DropResult::Ignored;
        };
        match plan_drop(&self.snapshot, paths) {
            DropPlan::Ignore => DropResult::Ignored,
            DropPlan::NotRunning(name) => DropResult::Message(language.opener_not_running(&name)),
            DropPlan::Choose(path, openers) => {
                self.question = Some(Question { path, openers });
                DropResult::Choose
            }
            DropPlan::Open(opener, path) => {
                let file = path.display().to_string();
                match self.open_file(handle, &opener.id, path) {
                    Ok(()) => DropResult::Opening,
                    Err(reason) => {
                        DropResult::Message(language.could_not_open_file(&file, &reason))
                    }
                }
            }
        }
    }

    pub(super) fn send_transport(
        &mut self,
        handle: &AddonHandle,
        addon_id: &str,
        source_id: &str,
        command: TransportCommand,
        position: Option<f64>,
    ) {
        let slot = (addon_id.to_owned(), source_id.to_owned());
        match (command, position) {
            (TransportCommand::Seek, Some(target)) => {
                self.seeks.insert(
                    slot,
                    Seeking {
                        target,
                        at: Instant::now(),
                    },
                );
            }
            (TransportCommand::Stop, _) => {
                self.seeks.remove(&slot);
            }
            _ => {}
        }
        self.problems.remove(addon_id);
        self.requests.push(Request {
            addon_id: addon_id.to_owned(),
            purpose: Purpose::Transport,
            reply: handle.transport(addon_id, source_id, command, position),
        });
    }

    /// Play when it is not playing, pause when it is.
    pub(super) fn toggle_play(&mut self, handle: &AddonHandle, addon_id: &str, source_id: &str) {
        let Some(transport) = self
            .snapshot
            .source(addon_id, source_id)
            .and_then(|source| source.transport.as_ref())
        else {
            return;
        };
        let command = if transport.state == TransportState::Playing {
            TransportCommand::Pause
        } else {
            TransportCommand::Play
        };
        self.send_transport(handle, addon_id, source_id, command, None);
    }

    /// The position to show: the one the user asked for while the add-on is still moving
    /// there, otherwise the one it reports. The flag is true while it is moving.
    fn shown_position(&self, slot: &Slot, transport: &Transport, now: Instant) -> (f64, bool) {
        if let Some(seeking) = self.seeks.get(slot)
            && seeking.pending(transport.position, now)
        {
            return (seeking.target, true);
        }
        (transport.position, false)
    }

    /// What the main window's compact transport shows for this source.
    pub(super) fn chip(&self, addon_id: &str, source_id: &str, now: Instant) -> Option<Chip> {
        let source = self.snapshot.source(addon_id, source_id)?;
        let transport = source.transport.as_ref()?;
        let slot = (addon_id.to_owned(), source_id.to_owned());
        let (position, _) = self.shown_position(&slot, transport, now);
        let text = match transport.duration {
            Some(duration) => format!("{} / {}", clock(position), clock(duration)),
            None => clock(position),
        };
        Some(Chip {
            playing: transport.state == TransportState::Playing,
            text,
            source: source.name.clone(),
        })
    }

    /// The add-on's file types, for the file dialog.
    pub(super) fn open_files_of(&self, addon_id: &str) -> Option<(String, OpenFiles)> {
        let addon = self.snapshot.addon(addon_id)?;
        open_files(addon).map(|files| (name_of(addon), files.clone()))
    }

    /// The question shown when more than one add-on opens a dropped file. Returns the add-on
    /// the user picked and the file.
    pub(super) fn show_question(
        &mut self,
        ui: &mut egui::Ui,
        language: Language,
    ) -> Option<(String, PathBuf)> {
        let question = self.question.as_ref()?;
        let file = question
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let mut chosen = None;
        let mut cancel = false;
        egui::Frame::new()
            .fill(Color32::from_gray(34))
            .stroke(Stroke::new(1.0, backtracking_ui::OPTIONS_CONTROL_OUTLINE))
            .corner_radius(6)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(language.which_addon(&file));
                ui.horizontal_wrapped(|ui| {
                    for opener in &question.openers {
                        if ui.button(&opener.name).clicked() {
                            chosen = Some(opener.id.clone());
                        }
                    }
                    cancel = ui.button(language.cancel()).clicked();
                });
            });
        if let Some(id) = chosen {
            let path = question.path.clone();
            self.question = None;
            return Some((id, path));
        }
        if cancel {
            self.question = None;
        }
        None
    }

    /// Draws what an add-on source adds under its lane's source picker.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn show_lane(
        &mut self,
        ui: &mut egui::Ui,
        handle: Option<&AddonHandle>,
        language: Language,
        accent: Color32,
        lane: usize,
        (addon_id, source_id, source_name): (&str, &str, &str),
        channels: Option<usize>,
        history_note: Option<String>,
    ) -> LaneOutcome {
        let snapshot = Arc::clone(&self.snapshot);
        let mut outcome = LaneOutcome::default();
        outcome.covered = !matches!(
            source_state(&snapshot, addon_id, source_id),
            SourceState::Ready { .. }
        );
        match source_state(&snapshot, addon_id, source_id) {
            SourceState::Ready { addon, source } => self.ready(
                ui,
                handle,
                language,
                lane,
                (addon, source),
                channels,
                history_note,
                &mut outcome,
            ),
            SourceState::NotInstalled => {
                problem_line(ui, &language.source_not_installed(source_name));
                options_note(ui, language.source_not_installed_hint());
                outcome.add_ons_page = ui.button(language.open_addons_page()).clicked();
            }
            SourceState::NotRunning(addon) => {
                let (tone, words) = state_words(language, &addon.state, addon.enabled);
                let name = name_of(addon);
                if tone == Tone::Problem {
                    problem_line(ui, &language.source_not_running(&name, &words));
                } else {
                    state_line(
                        ui,
                        tone,
                        &language.source_not_running(&name, &words),
                        accent,
                    );
                }
                options_note(ui, language.source_not_running_hint());
                self.notice(ui, language, addon, true);
                outcome.add_ons_page = ui.button(language.open_addons_page()).clicked();
            }
            SourceState::Gone(addon) => {
                problem_line(ui, &language.source_gone(&name_of(addon), source_name));
                options_note(ui, language.source_gone_hint());
                self.notice(ui, language, addon, true);
                outcome.add_ons_page = ui.button(language.open_addons_page()).clicked();
            }
        }
        if let Some(problem) = self.problems.get(addon_id) {
            problem_line(ui, problem);
        }
        outcome
    }

    /// The add-on's latest message, in the colour of its level. An error also says what can be
    /// done about it, unless the caller has already said so (`has_hint`).
    fn notice(&self, ui: &mut egui::Ui, language: Language, addon: &AddonInfo, has_hint: bool) {
        let Some(notice) = &addon.message else {
            return;
        };
        let text = language.addon_message(notice.level, &notice.text);
        match notice.level {
            MessageLevel::Info => options_note(ui, text),
            MessageLevel::Warning => {
                ui.add(egui::Label::new(egui::RichText::new(text).color(PROBLEM)).wrap());
            }
            MessageLevel::Error => {
                ui.add(egui::Label::new(egui::RichText::new(text).color(ERROR_TEXT)).wrap());
                if !has_hint {
                    options_note(ui, language.addon_error_hint());
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ready(
        &mut self,
        ui: &mut egui::Ui,
        handle: Option<&AddonHandle>,
        language: Language,
        lane: usize,
        (addon, source): (&AddonInfo, &Source),
        channels: Option<usize>,
        history_note: Option<String>,
        outcome: &mut LaneOutcome,
    ) {
        let slot: Slot = (addon.id.clone(), source.id.clone());
        let busy = handle.is_none();
        // The channel count of the format in use: what the add-on says, else what capture sees.
        let channels = source
            .groups
            .as_ref()
            .map(|groups| groups.iter().map(|group| group.channels.len()).sum())
            .filter(|count: &usize| *count > 0)
            .or(channels);

        // Formats. A format just chosen stays shown until the add-on describes it.
        let shown = match self.formats.get(&slot) {
            Some((format, _)) => format.as_str(),
            None => source.format.as_str(),
        };
        let listed = source.formats.iter().position(|format| format.id == shown);
        let mut picked = None;
        options_field(ui, language.format_field(), |ui| {
            let width = options_control_width(ui);
            let text = listed.map_or(shown, |index| source.formats[index].label.as_str());
            let combo = ui.make_persistent_id(egui::IdSalt::new("format"));
            if self.force_format_menu == Some(lane) {
                self.force_format_menu = None;
                egui::Popup::open_id(ui.ctx(), combo.with("popup"));
            }
            egui::ComboBox::from_id_salt("format")
                .width(width)
                .height(options_menu_height(ui, source.formats.len()))
                .selected_text(text)
                .truncate()
                .show_ui(ui, |ui| {
                    configure_options_menu(ui, width);
                    for (index, format) in source.formats.iter().enumerate() {
                        let current = listed == Some(index);
                        if ui.selectable_label(current, &format.label).clicked() && !current {
                            picked = Some(index);
                        }
                    }
                })
                .response
                .on_hover_text(text);
        });
        if listed.is_none() {
            problem_line(ui, &language.format_unavailable(shown));
        }
        if let (Some(index), Some(handle)) = (picked, handle) {
            let format = source.formats[index].id.clone();
            let _ = self.select_format(handle, &addon.id, &source.id, &format);
        }
        if let Some(count) = channels {
            let mut line = language.channel_count(count);
            if count > 2 {
                line.push_str(". ");
                line.push_str(language.routes_only_for_narrow());
            }
            options_note(ui, line);
        }
        outcome.show_route = channels.is_some_and(|count| count <= 2);
        if let Some(note) = history_note {
            options_note(ui, note);
        }

        // A file to open.
        if let Some(files) = open_files(addon) {
            let extensions = files
                .extensions
                .iter()
                .map(|extension| format!(".{}", extension.trim_start_matches('.')))
                .collect::<Vec<_>>()
                .join(", ");
            options_field(ui, language.file_field(), |ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new(language.open_file_button()))
                    .clicked()
                {
                    outcome.open_file = Some(addon.id.clone());
                }
                options_note(ui, language.open_file_help(&files.description, &extensions));
            });
        }

        if let Some(transport) = &source.transport {
            self.transport_controls(ui, handle, language, &slot, transport);
        }
        self.notice(ui, language, addon, false);
    }

    /// Play or pause, stop, and a position slider with the elapsed and the total time.
    fn transport_controls(
        &mut self,
        ui: &mut egui::Ui,
        handle: Option<&AddonHandle>,
        language: Language,
        slot: &Slot,
        transport: &Transport,
    ) {
        let now = Instant::now();
        let (reported, moving) = self.shown_position(slot, transport, now);
        let draft = self.drafts.get(slot).copied().unwrap_or_default();
        let mut command = None;
        let mut released = None;
        let mut next_draft = draft;
        options_field(ui, language.playback_field(), |ui| {
            ui.horizontal(|ui| {
                let playing = transport.state == TransportState::Playing;
                let label = if playing {
                    language.pause()
                } else {
                    language.play()
                };
                if ui
                    .add_enabled(handle.is_some(), egui::Button::new(label))
                    .clicked()
                {
                    command = Some(if playing {
                        TransportCommand::Pause
                    } else {
                        TransportCommand::Play
                    });
                }
                if ui
                    .add_enabled(
                        handle.is_some() && transport.state != TransportState::Stopped,
                        egui::Button::new(language.stop()),
                    )
                    .clicked()
                {
                    command = Some(TransportCommand::Stop);
                }
                if moving {
                    ui.spinner();
                }
            });
            let shown = next_draft.0.unwrap_or(reported);
            let Some(duration) = transport.duration.filter(|duration| *duration > 0.0) else {
                ui.label(egui::RichText::new(clock(shown)).color(OPTIONS_VALUE_TEXT));
                return;
            };
            let mut value = shown.clamp(0.0, duration);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(clock(value))
                        .monospace()
                        .color(OPTIONS_VALUE_TEXT),
                );
                let width = (ui.available_width() - 64.0).clamp(80.0, 420.0);
                ui.spacing_mut().slider_width = width;
                let slider = ui.add_enabled(
                    handle.is_some(),
                    egui::Slider::new(&mut value, 0.0..=duration)
                        .show_value(false)
                        .trailing_fill(true),
                );
                let held = slider.dragged() || slider.is_pointer_button_down_on();
                released =
                    next_draft.update(value, slider.changed(), held && !slider.drag_stopped());
                ui.label(
                    egui::RichText::new(clock(duration))
                        .monospace()
                        .color(OPTIONS_VALUE_TEXT),
                );
            });
            if moving {
                options_note(ui, language.moving_to(&clock(reported)));
            }
        });
        self.drafts.insert(slot.clone(), next_draft);
        if moving {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        let Some(handle) = handle else {
            return;
        };
        if let Some(position) = released {
            self.send_transport(
                handle,
                &slot.0,
                &slot.1,
                TransportCommand::Seek,
                Some(position),
            );
        } else if let Some(command) = command {
            self.send_transport(handle, &slot.0, &slot.1, command, None);
        }
    }
}

/// Where the compact transport sits, and what a click does. The geometry is the window's.
pub(super) fn paint_chip(
    painter: &egui::Painter,
    rect: egui::Rect,
    chip: &Chip,
    hovered: bool,
    accent: Color32,
) {
    painter.rect_filled(rect, 3.0, Color32::from_black_alpha(168));
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 9.0, rect.center().y),
        egui::vec2(8.0, 8.0),
    );
    let color = if hovered { Color32::WHITE } else { accent };
    if chip.playing {
        for x in [icon.left(), icon.right() - 3.0] {
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, icon.top()), egui::vec2(3.0, 8.0)),
                0.5,
                color,
            );
        }
    } else {
        painter.add(egui::Shape::convex_polygon(
            vec![
                icon.left_top(),
                icon.left_bottom(),
                egui::pos2(icon.right(), icon.center().y),
            ],
            color,
            Stroke::NONE,
        ));
    }
    painter.text(
        egui::pos2(rect.left() + 18.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &chip.text,
        egui::FontId::monospace(9.0),
        if hovered {
            Color32::WHITE
        } else {
            Color32::from_gray(200)
        },
    );
}

/// The size of the compact transport for this text.
pub(super) fn chip_size(painter: &egui::Painter, text: &str) -> egui::Vec2 {
    let text = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::monospace(9.0),
        Color32::WHITE,
    );
    egui::vec2(text.size().x + 24.0, 14.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::{AddonNotice, AddonSetting};
    use backtracking_addon::manifest::Manifest;
    use backtracking_addon::protocol::{ChannelGroup, Format, Setting, SettingKind};

    fn manifest(open_files: bool) -> Manifest {
        let (capabilities, files) = if open_files {
            (
                r#"["source","open-files"]"#,
                r#","open_files":{"description":"E-AC-3 audio","extensions":["ec3","mp4"]}"#,
            )
        } else {
            (r#"["source"]"#, "")
        };
        Manifest::parse(&format!(
            r#"{{"contract":1,"id":"com.example.a","name":"Player","version":"1",
                "author":"E","description":"d","capabilities":{capabilities}{files},
                "run":{{"windows-x86_64":{{"program":"p.exe"}},
                        "windows-aarch64":{{"program":"p.exe"}}}}}}"#
        ))
        .unwrap()
    }

    fn source(id: &str) -> Source {
        Source {
            id: id.into(),
            name: "Stream".into(),
            sender: "ab-1".into(),
            formats: vec![
                Format {
                    id: "stereo".into(),
                    label: "Stereo".into(),
                },
                Format {
                    id: "5.1".into(),
                    label: "5.1".into(),
                },
            ],
            format: "5.1".into(),
            groups: Some(vec![ChannelGroup {
                id: "bed".into(),
                label: "Bed".into(),
                channels: vec![
                    "L".into(),
                    "R".into(),
                    "C".into(),
                    "LFE".into(),
                    "Ls".into(),
                    "Rs".into(),
                ],
                positioned: false,
            }]),
            transport: Some(Transport {
                state: TransportState::Playing,
                position: 12.5,
                duration: Some(60.0),
            }),
        }
    }

    fn addon(id: &str, open: bool, state: AddonState) -> AddonInfo {
        let mut manifest = manifest(open);
        manifest.id = id.into();
        AddonInfo {
            id: id.into(),
            manifest: Some(manifest),
            enabled: true,
            state,
            pid: None,
            message: None,
            sources: vec![source("main")],
            settings: Vec::new(),
            actions: Vec::new(),
        }
    }

    fn snapshot(addons: Vec<AddonInfo>) -> AddonSnapshot {
        AddonSnapshot {
            revision: 1,
            addons,
            error: None,
        }
    }

    #[test]
    fn a_clock_reads_minutes_and_seconds_and_hours_when_needed() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(5.9), "0:05");
        assert_eq!(clock(65.0), "1:05");
        assert_eq!(clock(3600.0), "1:00:00");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(-3.0), "0:00");
        assert_eq!(clock(f64::NAN), "0:00");
    }

    #[test]
    fn a_lane_knows_why_its_source_is_missing() {
        let all = snapshot(vec![
            addon("com.example.a", false, AddonState::Running),
            addon("com.example.b", false, AddonState::Failed("exited".into())),
        ]);
        assert!(matches!(
            source_state(&all, "com.example.a", "main"),
            SourceState::Ready { .. }
        ));
        assert!(matches!(
            source_state(&all, "com.example.a", "other"),
            SourceState::Gone(_)
        ));
        assert!(matches!(
            source_state(&all, "com.example.b", "main"),
            SourceState::NotRunning(_)
        ));
        assert!(matches!(
            source_state(&all, "com.example.c", "main"),
            SourceState::NotInstalled
        ));
    }

    #[test]
    fn only_running_add_ons_that_declared_the_type_open_a_file() {
        let all = snapshot(vec![
            addon("com.example.a", true, AddonState::Running),
            addon("com.example.b", false, AddonState::Running),
            addon("com.example.c", true, AddonState::Stopped),
        ]);
        let names = |path: &str| -> Vec<String> {
            openers(&all, Path::new(path))
                .into_iter()
                .map(|opener| opener.id)
                .collect()
        };
        assert_eq!(names(r"C:\clips\joc.ec3"), ["com.example.a"]);
        // Case does not matter, on either side.
        assert_eq!(names(r"C:\clips\FILM.Mp4"), ["com.example.a"]);
        assert!(names(r"C:\clips\take.wav").is_empty());
        assert!(names(r"C:\clips\noextension").is_empty());
    }

    #[test]
    fn a_drop_goes_to_one_add_on_asks_when_two_open_it_and_says_when_none_runs() {
        let paths = |names: &[&str]| names.iter().map(PathBuf::from).collect::<Vec<_>>();
        let one = snapshot(vec![addon("com.example.a", true, AddonState::Running)]);
        assert_eq!(
            plan_drop(&one, &paths(&[r"C:\x\a.wav", r"C:\x\a.ec3"])),
            DropPlan::Open(
                Opener {
                    id: "com.example.a".into(),
                    name: "Player".into()
                },
                PathBuf::from(r"C:\x\a.ec3")
            )
        );
        assert_eq!(plan_drop(&one, &paths(&[r"C:\x\a.wav"])), DropPlan::Ignore);
        assert_eq!(plan_drop(&one, &paths(&[])), DropPlan::Ignore);
        // Add-on packages are the Add-ons page's.
        assert_eq!(
            plan_drop(&one, &paths(&[r"C:\x\a.btaddon"])),
            DropPlan::Ignore
        );

        let two = snapshot(vec![
            addon("com.example.a", true, AddonState::Running),
            addon("com.example.b", true, AddonState::Running),
        ]);
        match plan_drop(&two, &paths(&[r"C:\x\a.ec3"])) {
            DropPlan::Choose(path, found) => {
                assert_eq!(path, PathBuf::from(r"C:\x\a.ec3"));
                assert_eq!(found.len(), 2);
            }
            other => panic!("{other:?}"),
        }

        let idle = snapshot(vec![addon("com.example.a", true, AddonState::Stopped)]);
        assert_eq!(
            plan_drop(&idle, &paths(&[r"C:\x\a.ec3"])),
            DropPlan::NotRunning("Player".into())
        );
    }

    #[test]
    fn a_test_copy_will_not_open_a_file_unless_the_speakers_are_known_to_be_off() {
        let mut addon = addon("com.example.a", true, AddonState::Running);
        // No such setting: nothing to guard.
        assert!(!speakers_may_be_on(&addon));
        let setting = |value: serde_json::Value| AddonSetting {
            declaration: Setting {
                key: SPEAKER_SETTING.into(),
                label: "Play through speakers".into(),
                help: None,
                kind: SettingKind::Toggle { default: true },
            },
            value,
        };
        addon.settings = vec![setting(serde_json::json!(true))];
        assert!(speakers_may_be_on(&addon));
        addon.settings = vec![setting(serde_json::json!(false))];
        assert!(!speakers_may_be_on(&addon));
        addon.settings = vec![setting(serde_json::json!("unexpected"))];
        assert!(speakers_may_be_on(&addon));
    }

    #[test]
    fn a_seek_keeps_the_asked_for_position_until_the_add_on_arrives_or_the_wait_ends() {
        let start = Instant::now();
        let seeking = Seeking {
            target: 30.0,
            at: start,
        };
        assert!(seeking.pending(12.0, start));
        assert!(seeking.pending(12.0, start + Duration::from_millis(2900)));
        // The add-on reports a position near the target.
        assert!(!seeking.pending(30.4, start + Duration::from_millis(500)));
        // It never does: the wait ends.
        assert!(!seeking.pending(12.0, start + SEEK_SETTLE));
    }

    #[test]
    fn dragging_the_slider_sends_one_seek_when_it_is_let_go() {
        let mut draft = SliderDraft::default();
        // Press and drag across many frames: nothing is sent.
        assert_eq!(draft.update(10.0, true, true), None);
        for value in [11.0, 12.0, 20.0, 20.5] {
            assert_eq!(draft.update(value, true, true), None);
        }
        // Held still: still nothing.
        assert_eq!(draft.update(20.5, false, true), None);
        // Let go: one position, the last one.
        assert_eq!(draft.update(20.5, false, false), Some(20.5));
        assert_eq!(draft.update(20.5, false, false), None);
        // A key press moves the slider with no pointer: sent at once, once.
        assert_eq!(draft.update(21.0, true, false), Some(21.0));
        assert_eq!(draft.update(21.0, false, false), None);
    }

    #[test]
    fn the_compact_transport_shows_the_position_and_total() {
        let mut sources = AddonSources::default();
        sources.snapshot = Arc::new(snapshot(vec![addon(
            "com.example.a",
            true,
            AddonState::Running,
        )]));
        let now = Instant::now();
        let chip = sources.chip("com.example.a", "main", now).unwrap();
        assert!(chip.playing);
        assert_eq!(chip.text, "0:12 / 1:00");
        assert_eq!(chip.source, "Stream");
        // While a seek is pending the target is shown.
        sources.seeks.insert(
            ("com.example.a".into(), "main".into()),
            Seeking {
                target: 45.0,
                at: now,
            },
        );
        assert_eq!(
            sources.chip("com.example.a", "main", now).unwrap().text,
            "0:45 / 1:00"
        );
        // A source without transport has no chip, nor has one that is not running.
        sources.snapshot = Arc::new(snapshot(vec![{
            let mut idle = addon("com.example.a", true, AddonState::Running);
            idle.sources[0].transport = None;
            idle
        }]));
        assert!(sources.chip("com.example.a", "main", now).is_none());
        assert!(sources.chip("com.example.zzz", "main", now).is_none());
    }

    #[test]
    fn an_endless_source_shows_only_the_position() {
        let mut sources = AddonSources::default();
        let mut endless = addon("com.example.a", false, AddonState::Running);
        endless.sources[0].transport.as_mut().unwrap().duration = None;
        sources.snapshot = Arc::new(snapshot(vec![endless]));
        let chip = sources
            .chip("com.example.a", "main", Instant::now())
            .unwrap();
        assert_eq!(chip.text, "0:12");
    }

    #[test]
    fn an_error_message_is_kept_apart_from_the_state() {
        let mut broken = addon("com.example.a", false, AddonState::Running);
        broken.message = Some(AddonNotice {
            level: MessageLevel::Error,
            text: "No object audio.".into(),
        });
        let all = snapshot(vec![broken]);
        let info = all.addon("com.example.a").unwrap();
        assert!(matches!(
            source_state(&all, "com.example.a", "main"),
            SourceState::Ready { .. }
        ));
        assert_eq!(info.message.as_ref().unwrap().level, MessageLevel::Error);
    }

    /// Draws the Sources parts of a source with no manager and returns what it asks for.
    fn draw_lane(sources: &mut AddonSources, channels: Option<usize>) -> LaneOutcome {
        let ctx = egui::Context::default();
        let mut outcome = None;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                outcome = Some(sources.show_lane(
                    ui,
                    None,
                    Language::English,
                    Color32::LIGHT_BLUE,
                    0,
                    ("com.example.a", "main", "Stream"),
                    channels,
                    None,
                ));
            },
        )
        .drop_without_applying_deltas();
        outcome.unwrap()
    }

    #[test]
    fn the_lane_draws_its_formats_file_button_and_transport_without_a_manager() {
        let mut sources = AddonSources::default();
        sources.snapshot = Arc::new(snapshot(vec![addon(
            "com.example.a",
            true,
            AddonState::Running,
        )]));
        let outcome = draw_lane(&mut sources, Some(6));
        // Six channels: each has its own row, so no route applies.
        assert!(!outcome.show_route);
        assert!(outcome.open_file.is_none());
        assert!(!outcome.add_ons_page);
    }

    #[test]
    fn a_stereo_format_keeps_the_usual_routes() {
        let mut stereo = addon("com.example.a", false, AddonState::Running);
        stereo.sources[0].format = "stereo".into();
        stereo.sources[0].groups = Some(vec![ChannelGroup {
            id: "channels".into(),
            label: "Channels".into(),
            channels: vec!["L".into(), "R".into()],
            positioned: false,
        }]);
        let mut sources = AddonSources::default();
        sources.snapshot = Arc::new(snapshot(vec![stereo]));
        assert!(draw_lane(&mut sources, None).show_route);
    }

    #[test]
    fn a_missing_source_has_no_route() {
        let mut sources = AddonSources::default();
        sources.snapshot = Arc::new(snapshot(vec![addon(
            "com.example.a",
            false,
            AddonState::Stopped,
        )]));
        assert!(!draw_lane(&mut sources, Some(2)).show_route);
        sources.snapshot = Arc::new(snapshot(Vec::new()));
        assert!(!draw_lane(&mut sources, Some(2)).show_route);
    }

    #[test]
    fn positioned_sources_are_excluded_from_the_pair_even_when_the_bed_is_named_objects() {
        use crate::ui::{LaneGroup, channels_group};
        let sources = LaneGroup {
            id: "sources".into(),
            positioned: true,
            label: "Sources".into(),
            channels: 0..2,
            labels: vec!["A".into(), "B".into()],
        };
        let bed = LaneGroup {
            id: "ordinary".into(),
            positioned: false,
            label: "Objects".into(),
            channels: 2..4,
            labels: vec!["L".into(), "R".into()],
        };
        assert_eq!(channels_group(&[sources, bed.clone()], 4), bed);
    }
}

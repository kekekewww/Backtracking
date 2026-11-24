//! One way to drive the window from outside it. [`Action`] is what can be done and
//! [`Recorder::run_action`] does it on the UI thread, through the same methods as the toolbar,
//! the keys and the Sources page. Two callers use it: add-ons with the `control` capability,
//! whose requests [`Recorder::run_control_calls`] turns into actions and answers, and the test
//! harness's `select=`, `save=`, `zoom=`, `rows=`, `select-lane=`, `separate=`,
//! `drag-prepare=`, `preview-plan`, `add-source=` and `remove-source=` steps.

use std::fs;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_addon::protocol::{
    ControlCommand, ControlError, ControlResult, ControlState, FrameSpan, LaneState, RowState,
    SavedFile, SelectionState, SourceInfo, control_codes as codes,
};
use eframe::egui;

use super::text::ControlNote;
use super::{
    CaptureSource, ChannelRoute, Controller, ExportedWav, Lane, Language, Recorder, RowSource,
    harness, lane_name, rows_between,
};
use crate::addons::ControlCall;

/// How long a reply waits for capture to say it paused or started, before it is sent anyway.
const CAPTURE_SETTLE: Duration = Duration::from_secs(2);
/// How long a `select` waits for its audio to be frozen.
const FREEZE_WITHIN: Duration = Duration::from_secs(8);

/// Something done to the window, as the user would do it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Action {
    StartCapture,
    PauseCapture,
    ClearHistory,
    /// Selects from `from` to `to` seconds back from the newest audio, as a drag would.
    SelectBack { from: f64, to: f64 },
    /// Selects these frames of the audio held.
    SelectFrames(Range<u64>),
    ClearSelection,
    /// "Edit separately" on or off, as Right Ctrl switches it.
    Separate(bool),
    /// Every row of this lane is what the selection covers.
    ChooseLane(usize),
    /// The rows from row `.1` of lane `.0` to row `.3` of lane `.2`, from 0, as a drag
    /// across them would choose.
    ChooseRows(usize, usize, usize, usize),
    /// Shows the newest this many seconds.
    ZoomNewest(f64),
    /// Shows these frames.
    View(Range<u64>),
    /// Shows everything held.
    ViewAll,
    /// Saves as the Save command does, into `folder`, made when missing.
    Save { folder: PathBuf, tag: Option<u64> },
    /// Writes the files a drag-out would carry, through the drag's own code.
    DragPrepare,
    /// Says what Space would play, without playing it.
    PreviewPlan,
    PlayPreview,
    StopPreview,
    AddSource(CaptureSource),
    RemoveSource(usize),
}

/// What an action left to wait for.
#[derive(Debug, PartialEq)]
pub(super) enum Ran {
    Done,
    /// The save is queued; its files are written later.
    SaveQueued,
    /// What `PreviewPlan` found.
    Note(String),
}

/// Why an action was not done: a stable code of the control contract and words for people.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Refusal {
    pub(super) code: &'static str,
    pub(super) message: String,
}

impl Refusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn error(self) -> ControlError {
        ControlError::new(self.code, self.message)
    }
}

impl From<Refusal> for String {
    fn from(refusal: Refusal) -> Self {
        refusal.message
    }
}

/// Control requests that wait for something before their reply.
#[derive(Default)]
pub(super) struct Control {
    waiting: Vec<(ControlCall, Wait)>,
    next_tag: u64,
    /// The window wakes when an add-on sends a request.
    subscribed: bool,
    context: Option<egui::Context>,
}

impl Control {
    /// Whether a reply waits on something that sends no repaint of its own.
    pub(super) fn waiting(&self) -> bool {
        !self.waiting.is_empty()
    }
}

enum Wait {
    /// Capture to report it is paused (true) or running (false).
    Capture { paused: bool, until: Instant },
    /// The selection's audio to be frozen.
    Freeze { until: Instant },
    /// The files of the save with this tag to be written.
    Save { tag: u64 },
    /// A scan of the sources that can be added: to list them, or to add the one with this id.
    Sources {
        scan: JoinHandle<Vec<CaptureSource>>,
        add: Option<String>,
    },
}

impl Recorder {
    /// Does `action` as the window's own controls would.
    pub(super) fn run_action(&mut self, action: Action) -> Result<Ran, Refusal> {
        let rate = u64::from(self.sample_rate.max(1));
        let frames = |seconds: f64| (seconds * rate as f64).round() as u64;
        let history = self.history_frames.clone();
        let needs_audio = matches!(
            action,
            Action::SelectBack { .. }
                | Action::SelectFrames(_)
                | Action::ZoomNewest(_)
                | Action::View(_)
        );
        if needs_audio && history.is_empty() {
            return Err(Refusal::new(codes::NO_AUDIO, "no audio yet"));
        }
        match action {
            Action::StartCapture => {
                if self.controller.is_paused() || self.controller.error().is_some() {
                    self.resume();
                }
            }
            Action::PauseCapture => {
                if !self.controller.is_paused() {
                    self.freeze();
                }
            }
            Action::ClearHistory => self.clear_capture(),
            Action::SelectBack { from, to } => {
                let start = history
                    .end
                    .saturating_sub(frames(from.max(to)))
                    .max(history.start);
                let end = history
                    .end
                    .saturating_sub(frames(from.min(to)))
                    .max(start + 1);
                self.select_frames(start..end);
            }
            Action::SelectFrames(range) => {
                if range.start < history.start || range.end > history.end || range.is_empty() {
                    return Err(Refusal::new(
                        codes::NOT_FOUND,
                        format!(
                            "frames {}..{} are not all in the audio held, {}..{}",
                            range.start, range.end, history.start, history.end
                        ),
                    ));
                }
                self.select_frames(range);
            }
            Action::ClearSelection => self.cancel_selection(),
            Action::Separate(on) => {
                if self.row_count() < 2 {
                    return Err(Refusal::new(
                        codes::FAILED,
                        "editing separately needs more than one row",
                    ));
                }
                if self.separate_rows != on {
                    self.toggle_separate_rows();
                }
            }
            Action::ChooseLane(lane) => {
                let plan = self
                    .lanes
                    .iter()
                    .find(|plan| plan.lane == lane)
                    .ok_or_else(|| {
                        Refusal::new(codes::NOT_FOUND, "the window does not show that lane")
                    })?;
                let last = plan
                    .rows()
                    .checked_sub(1)
                    .ok_or_else(|| Refusal::new(codes::NOT_FOUND, "the lane has no rows"))?;
                let rows = rows_between(&self.lanes, plan.row(0), plan.row(last));
                self.choose_shown_rows(rows)?;
            }
            Action::ChooseRows(first_lane, first, last_lane, last) => {
                let row = |lane: usize, row: usize| {
                    self.lanes
                        .iter()
                        .find(|plan| plan.lane == lane)
                        .filter(|plan| row < plan.rows())
                        .map(|plan| plan.row(row))
                        .ok_or_else(|| {
                            Refusal::new(
                                codes::NOT_FOUND,
                                format!("the window has no row {} in that lane", row + 1),
                            )
                        })
                };
                let rows =
                    rows_between(&self.lanes, row(first_lane, first)?, row(last_lane, last)?);
                self.choose_shown_rows(rows)?;
            }
            Action::ZoomNewest(seconds) => {
                let count = frames(seconds).max(1);
                self.set_visible_frames(history.end.saturating_sub(count), count);
            }
            Action::View(range) => {
                let start = range.start.max(history.start);
                let count = range.end.min(history.end).saturating_sub(start).max(1);
                self.set_visible_frames(start, count);
            }
            Action::ViewAll => {
                let count = history.end - history.start;
                if count > 0 {
                    self.set_visible_frames(history.start, count);
                }
            }
            Action::Save { folder, tag } => {
                fs::create_dir_all(&folder).map_err(|error| {
                    Refusal::new(codes::FAILED, format!("{}: {error}", folder.display()))
                })?;
                if !self.save_snapshot_tagged(folder, tag) {
                    let message = if self.message.is_empty() {
                        "Nothing could be saved now".to_owned()
                    } else {
                        self.message.clone()
                    };
                    return Err(Refusal::new(codes::FAILED, message));
                }
                return Ok(Ran::SaveQueued);
            }
            Action::DragPrepare => {
                // As a drag past the window's edge does: freeze, then write the clip.
                self.invalidate_export();
                self.start_export();
            }
            Action::PreviewPlan => return Ok(Ran::Note(self.describe_preview())),
            Action::PlayPreview => {
                if self.selection.is_none() {
                    return Err(Refusal::new(codes::NO_SELECTION, "there is no selection to play"));
                }
                if self.test_copy {
                    // A test copy makes no sound.
                    return Err(Refusal::new(
                        codes::FAILED,
                        "a test copy of Backtracking does not play audio",
                    ));
                }
                if self.preview.is_none() {
                    let Some(ctx) = self.control_context() else {
                        return Err(Refusal::new(codes::FAILED, "the window is not ready"));
                    };
                    self.toggle_preview(&ctx);
                    if self.preview.is_none() {
                        return Err(Refusal::new(codes::FAILED, self.message.clone()));
                    }
                }
            }
            Action::StopPreview => self.stop_preview(),
            Action::AddSource(source) => self.add_source(source).map_err(|error| {
                Refusal::new(codes::FAILED, error)
            })?,
            Action::RemoveSource(lane) => self
                .remove_source(lane)
                .map_err(|error| Refusal::new(codes::FAILED, error))?,
        }
        Ok(Ran::Done)
    }

    /// As a finished drag does: freeze the audio, then keep the range.
    fn select_frames(&mut self, range: Range<u64>) {
        self.begin_editing();
        self.last_interaction = Instant::now();
        let flags = self.flag_update(
            range.clone(),
            self.selection_rows.clone(),
            self.selection_whole.clone(),
        );
        self.apply_flags(flags);
        self.selection = Some(range);
        self.selection_by = None;
        self.invalidate_export();
    }

    /// Makes `rows` what the next selection covers, as the drag that crossed them would. A
    /// selection already made moves onto them.
    fn choose_shown_rows(&mut self, rows: Vec<RowSource>) -> Result<(), Refusal> {
        if !self.rows_separate() {
            return Err(Refusal::new(
                codes::FAILED,
                "rows are chosen only while rows are edited separately",
            ));
        }
        self.choose_rows(rows);
        if let Some(selection) = self.selection.clone() {
            let flags = self.flag_update(
                selection,
                self.selection_rows.clone(),
                self.selection_whole.clone(),
            );
            self.apply_flags(flags);
            self.stop_preview();
            self.invalidate_export();
        }
        Ok(())
    }

    /// The main window's context, to start a preview with.
    fn control_context(&self) -> Option<egui::Context> {
        self.control.context.clone()
    }

    /// Wakes the window when an add-on sends a control request.
    pub(super) fn wake_on_control(&mut self, ctx: &egui::Context) {
        let Some(addons) = self.controller.addons() else {
            return;
        };
        if !self.primary || self.control.subscribed {
            return;
        }
        self.control.subscribed = true;
        let ctx = ctx.clone();
        addons.subscribe_control(Arc::new(move || {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }));
    }

    /// Carries out the add-ons' control requests and answers them. Runs once a pass.
    pub(super) fn run_control_calls(&mut self, ctx: &egui::Context) {
        if !self.primary {
            return;
        }
        let Some(handle) = self.controller.addons().cloned() else {
            return;
        };
        if self.control.context.is_none() {
            self.control.context = Some(ctx.clone());
        }
        self.poll_control_waits(&handle);
        let calls = handle.take_control_calls();
        if calls.is_empty() {
            return;
        }
        let snapshot = handle.try_snapshot().unwrap_or_else(|| handle.snapshot());
        for call in calls {
            let allowed = snapshot
                .addon(&call.addon_id)
                .is_some_and(|addon| addon.control_allowed);
            if !allowed {
                handle.control_reply(
                    &call,
                    Err(ControlError::new(
                        codes::NOT_ALLOWED,
                        "The user has not let this add-on control Backtracking",
                    )),
                );
                continue;
            }
            self.start_control_call(&handle, call);
        }
    }

    fn start_control_call(&mut self, handle: &crate::addons::AddonHandle, call: ControlCall) {
        let quiet = matches!(
            call.command,
            ControlCommand::GetState | ControlCommand::ListSources | ControlCommand::StopPreview
        );
        if !quiet && let Some(reason) = self.control_busy() {
            let error = ControlError::new(
                codes::BUSY,
                format!("Backtracking is busy with {reason}; try again shortly"),
            );
            handle.control_reply(&call, Err(error));
            return;
        }
        let language = Language::current();
        let name = call.addon_name.clone();
        let result = match call.command.clone() {
            ControlCommand::GetState => Ok(None),
            ControlCommand::ListSources => {
                self.control_scan(call, None);
                return;
            }
            ControlCommand::StartCapture => self.run_action(Action::StartCapture).map(|_| {
                self.message = language.by_addon(language.control_note(ControlNote::Started), &name);
                Some(Wait::Capture {
                    paused: false,
                    until: Instant::now() + CAPTURE_SETTLE,
                })
            }),
            ControlCommand::PauseCapture => self.run_action(Action::PauseCapture).map(|_| {
                self.message = language.by_addon(language.control_note(ControlNote::Paused), &name);
                Some(Wait::Capture {
                    paused: true,
                    until: Instant::now() + CAPTURE_SETTLE,
                })
            }),
            ControlCommand::ClearHistory => self.run_action(Action::ClearHistory).map(|_| {
                self.message = language.by_addon(language.control_note(ControlNote::Cleared), &name);
                None
            }),
            ControlCommand::Select {
                from_seconds_ago,
                to_seconds_ago,
                start_frame,
                end_frame,
            } => {
                let action = match (from_seconds_ago, start_frame, end_frame) {
                    (Some(from), _, _) => Action::SelectBack {
                        from,
                        to: to_seconds_ago.unwrap_or(0.0),
                    },
                    (None, Some(start), Some(end)) => Action::SelectFrames(start..end),
                    _ => Action::SelectFrames(0..0),
                };
                self.run_action(action).map(|_| {
                    self.selection_by = Some(name.clone());
                    Some(Wait::Freeze {
                        until: Instant::now() + FREEZE_WITHIN,
                    })
                })
            }
            ControlCommand::ClearSelection => self.run_action(Action::ClearSelection).map(|_| None),
            ControlCommand::ChooseRows { lane, first, last } => {
                let lane_index = |letter: &str| self.control_lane(letter);
                let action = match (lane, first, last) {
                    (Some(lane), _, _) => lane_index(&lane).map(Action::ChooseLane),
                    (None, Some(first), Some(last)) => lane_index(&first.lane).and_then(|a| {
                        lane_index(&last.lane).map(|b| {
                            Action::ChooseRows(
                                a,
                                first.row.saturating_sub(1) as usize,
                                b,
                                last.row.saturating_sub(1) as usize,
                            )
                        })
                    }),
                    _ => Ok(Action::Separate(false)),
                };
                action.and_then(|action| {
                    if action != Action::Separate(false) && !self.rows_separate() {
                        self.run_action(Action::Separate(true))?;
                    }
                    if action == Action::Separate(false) && !self.separate_rows {
                        return Ok(None);
                    }
                    self.run_action(action).map(|_| None)
                })
            }
            ControlCommand::SetView {
                seconds,
                start_frame,
                end_frame,
            } => {
                let action = match (seconds, start_frame, end_frame) {
                    (Some(seconds), _, _) => Action::ZoomNewest(seconds),
                    (None, Some(start), Some(end)) => Action::View(start..end),
                    _ => Action::ViewAll,
                };
                self.run_action(action).map(|_| None)
            }
            ControlCommand::Save { folder } => {
                self.control.next_tag += 1;
                let tag = self.control.next_tag;
                let folder = folder.map_or_else(|| self.saved_output.clone(), PathBuf::from);
                self.run_action(Action::Save {
                    folder,
                    tag: Some(tag),
                })
                .map(|_| Some(Wait::Save { tag }))
            }
            ControlCommand::PlayPreview => self.run_action(Action::PlayPreview).map(|_| None),
            ControlCommand::StopPreview => self.run_action(Action::StopPreview).map(|_| None),
            ControlCommand::AddSource { source } => {
                if let Some(found) = self
                    .available_sources
                    .iter()
                    .find(|candidate| candidate.identity() == source)
                    .cloned()
                {
                    self.add_control_source(handle, call, found);
                } else {
                    self.control_scan(call, Some(source));
                }
                return;
            }
            ControlCommand::RemoveSource { lane } => self
                .control_lane(&lane)
                .and_then(|lane| {
                    if self.lane_count() < 2 {
                        return Err(Refusal::new(
                            codes::FAILED,
                            "the only lane cannot be removed; add another first",
                        ));
                    }
                    self.run_action(Action::RemoveSource(lane))
                })
                .map(|_| {
                    self.message = language
                        .by_addon(language.control_note(ControlNote::SourcesChanged), &name);
                    None
                }),
        };
        match result {
            Err(refusal) => handle.control_reply(&call, Err(refusal.error())),
            Ok(Some(wait)) => self.control.waiting.push((call, wait)),
            Ok(None) => {
                let restarted = matches!(call.command, ControlCommand::RemoveSource { .. });
                let result = ControlResult {
                    state: self.control_state(),
                    history_restarted: restarted,
                    ..ControlResult::default()
                };
                handle.control_reply(&call, Ok(result));
            }
        }
    }

    fn add_control_source(
        &mut self,
        handle: &crate::addons::AddonHandle,
        call: ControlCall,
        source: CaptureSource,
    ) {
        let language = Language::current();
        match self.run_action(Action::AddSource(source)) {
            Ok(_) => {
                self.message = language.by_addon(
                    language.control_note(ControlNote::SourcesChanged),
                    &call.addon_name,
                );
                let result = ControlResult {
                    state: self.control_state(),
                    history_restarted: true,
                    ..ControlResult::default()
                };
                handle.control_reply(&call, Ok(result));
            }
            Err(refusal) => handle.control_reply(&call, Err(refusal.error())),
        }
    }

    /// Scans the sources off this thread, as opening the Sources page's list does.
    fn control_scan(&mut self, call: ControlCall, add: Option<String>) {
        let snapshot = self.controller.addons().map(|addons| addons.snapshot());
        let scan = thread::Builder::new()
            .name("backtracking-control-sources".into())
            .spawn(move || match snapshot {
                Some(snapshot) => Controller::available_sources_with_addons(&snapshot),
                None => Controller::available_sources(),
            });
        match scan {
            Ok(scan) => self.control.waiting.push((call, Wait::Sources { scan, add })),
            Err(error) => {
                if let Some(handle) = self.controller.addons() {
                    handle.control_reply(
                        &call,
                        Err(ControlError::new(codes::FAILED, error.to_string())),
                    );
                }
            }
        }
    }

    /// Answers the requests whose wait is over.
    fn poll_control_waits(&mut self, handle: &crate::addons::AddonHandle) {
        let now = Instant::now();
        for (call, wait) in std::mem::take(&mut self.control.waiting) {
            match wait {
                Wait::Capture { paused, until } => {
                    if self.controller.is_paused() == paused
                        || self.controller.error().is_some()
                        || now >= until
                    {
                        let result = ControlResult {
                            state: self.control_state(),
                            ..ControlResult::default()
                        };
                        handle.control_reply(&call, Ok(result));
                    } else {
                        self.control.waiting.push((call, Wait::Capture { paused, until }));
                    }
                }
                Wait::Freeze { until } => {
                    if self.edit_request.is_some() && now < until {
                        self.control.waiting.push((call, Wait::Freeze { until }));
                    } else if self.editing.is_some() && self.selection.is_some() {
                        let result = ControlResult {
                            state: self.control_state(),
                            ..ControlResult::default()
                        };
                        handle.control_reply(&call, Ok(result));
                    } else {
                        let message = if self.edit_request.is_some() {
                            "the selection's audio was not ready in time".to_owned()
                        } else if self.message.is_empty() {
                            "no audio is available for this selection".to_owned()
                        } else {
                            self.message.clone()
                        };
                        handle.control_reply(
                            &call,
                            Err(ControlError::new(codes::NO_AUDIO, message)),
                        );
                    }
                }
                Wait::Save { tag } => {
                    if self.save_in_flight(tag) {
                        self.control.waiting.push((call, Wait::Save { tag }));
                    } else {
                        // It was dropped before its files were started: say what the status
                        // line says.
                        let message = if self.message.is_empty() {
                            "the audio could not be saved".to_owned()
                        } else {
                            self.message.clone()
                        };
                        handle.control_reply(&call, Err(ControlError::new(codes::FAILED, message)));
                    }
                }
                Wait::Sources { scan, add } => {
                    if !scan.is_finished() {
                        self.control.waiting.push((call, Wait::Sources { scan, add }));
                        continue;
                    }
                    let Ok(sources) = scan.join() else {
                        handle.control_reply(
                            &call,
                            Err(ControlError::new(codes::FAILED, "the source scan stopped")),
                        );
                        continue;
                    };
                    self.available_sources = sources;
                    match add {
                        None => {
                            let result = ControlResult {
                                state: self.control_state(),
                                sources: Some(
                                    self.available_sources.iter().map(source_info).collect(),
                                ),
                                ..ControlResult::default()
                            };
                            handle.control_reply(&call, Ok(result));
                        }
                        Some(id) => {
                            match self
                                .available_sources
                                .iter()
                                .find(|candidate| candidate.identity() == id)
                                .cloned()
                            {
                                Some(source) => self.add_control_source(handle, call, source),
                                None => handle.control_reply(
                                    &call,
                                    Err(ControlError::new(
                                        codes::NOT_FOUND,
                                        "no source with this id can be added now; list_sources \
                                         gives the ids",
                                    )),
                                ),
                            }
                        }
                    }
                }
            }
        }
    }

    /// Whether the save with `tag` is still queued or being written.
    fn save_in_flight(&self, tag: u64) -> bool {
        let marked = |saves: &[super::SnapshotSpec]| saves.iter().any(|spec| spec.tag == Some(tag));
        (self.snapshot_save_tag == Some(tag) && self.snapshot_save.is_some())
            || self.pending_snapshot_saves.iter().any(|clip| clip.tag == Some(tag))
            || self.pending_snapshot_reads.iter().any(|read| marked(&read.saves))
            || self.pending_edit_save_reads.iter().any(|read| marked(&read.saves))
            || self
                .edit_request
                .as_ref()
                .is_some_and(|request| marked(&request.saves))
    }

    /// The files of the save with `tag` are written, or could not be: answer its request.
    pub(super) fn finish_control_save(&mut self, tag: u64, outcome: Result<ExportedWav, String>) {
        let Some(index) = self
            .control
            .waiting
            .iter()
            .position(|(_, wait)| matches!(wait, Wait::Save { tag: waiting } if *waiting == tag))
        else {
            return;
        };
        let (call, _) = self.control.waiting.remove(index);
        let Some(handle) = self.controller.addons().cloned() else {
            return;
        };
        match outcome {
            Ok(file) => {
                let files = saved_files(&file);
                let folder = file
                    .paths
                    .first()
                    .and_then(|path| path.parent())
                    .map(|folder| folder.display().to_string());
                self.message = Language::current().by_addon(&self.message, &call.addon_name);
                let result = ControlResult {
                    state: self.control_state(),
                    folder,
                    files: Some(files),
                    ..ControlResult::default()
                };
                handle.control_reply(&call, Ok(result));
            }
            Err(message) => {
                handle.control_reply(&call, Err(ControlError::new(codes::FAILED, message)));
            }
        }
    }

    /// Why a command that changes things would wait now, or `None`.
    fn control_busy(&self) -> Option<&'static str> {
        if self.gesture.is_some() || self.window_drag.is_some() || self.pan.is_some() {
            Some("a drag in the window")
        } else if self.export.is_some() || self.export_requested {
            Some("a drag-out being written")
        } else if self.snapshot_save.is_some()
            || !self.pending_snapshot_saves.is_empty()
            || !self.pending_snapshot_reads.is_empty()
            || !self.pending_edit_save_reads.is_empty()
            || self
                .edit_request
                .as_ref()
                .is_some_and(|request| !request.saves.is_empty())
        {
            Some("a save being written")
        } else {
            None
        }
    }

    /// A lane named by its letter, which must exist.
    fn control_lane(&self, letter: &str) -> Result<usize, Refusal> {
        Lane::from_letter(&letter.to_ascii_uppercase())
            .map(Lane::index)
            .filter(|&lane| lane < self.lane_count())
            .ok_or_else(|| Refusal::new(codes::NOT_FOUND, format!("there is no lane {letter}")))
    }

    /// What every control reply reports.
    pub(super) fn control_state(&self) -> ControlState {
        let language = Language::current();
        let rate = self.sample_rate;
        let seconds = |frames: u64| {
            if rate == 0 {
                0.0
            } else {
                frames as f64 / f64::from(rate)
            }
        };
        let span = |range: &Range<u64>| FrameSpan {
            start_frame: range.start,
            end_frame: range.end,
            seconds: seconds(range.end.saturating_sub(range.start)),
        };
        let history = &self.history_frames;
        let lanes = self
            .controller
            .sources()
            .iter()
            .enumerate()
            .filter_map(|(lane, settings)| {
                let settings = settings.as_ref()?;
                Some(LaneState {
                    lane: lane_name(lane).into(),
                    source: source_info(&settings.source),
                    channels: self
                        .controller
                        .track_channels(lane)
                        .and_then(|channels| u32::try_from(channels).ok()),
                    route: route_name(settings.route).into(),
                    problem: self
                        .controller
                        .source_status(lane)
                        .map(|status| language.source_message(status).to_owned()),
                })
            })
            .collect();
        let selection = self.selection.as_ref().map(|range| SelectionState {
            start_frame: range.start,
            end_frame: range.end,
            seconds: seconds(range.end.saturating_sub(range.start)),
            from_seconds_ago: seconds(history.end.saturating_sub(range.start)),
            to_seconds_ago: seconds(history.end.saturating_sub(range.end)),
        });
        let chosen_rows = self
            .selected_rows()
            .unwrap_or_default()
            .iter()
            .filter_map(|&row| {
                let plan = self.lanes.iter().find(|plan| plan.lane == row.lane())?;
                let index = (0..plan.rows()).position(|index| plan.row(index) == row)?;
                Some(RowState {
                    lane: lane_name(row.lane()).into(),
                    row: u32::try_from(index + 1).ok()?,
                    label: self.row_name(row),
                })
            })
            .collect();
        ControlState {
            capturing: !self.controller.is_paused() && self.controller.error().is_none(),
            recording: self.controller.is_recording(),
            problem: self
                .controller
                .error()
                .map(|error| language.source_message(error).to_owned()),
            sample_rate: rate,
            lanes,
            history: span(history),
            selection,
            view: span(&self.visible_frames),
            separate_rows: self.rows_separate(),
            chosen_rows,
            preview_playing: self.preview.is_some(),
            save_folder: self.saved_output.display().to_string(),
            drag_folder: self.output.display().to_string(),
            busy: self.control_busy().map(str::to_owned),
        }
    }
}

fn route_name(route: ChannelRoute) -> &'static str {
    match route {
        ChannelRoute::Left => "left",
        ChannelRoute::Right => "right",
        ChannelRoute::Center => "center",
        ChannelRoute::Mid => "mid",
        ChannelRoute::Side => "side",
        ChannelRoute::Merged => "merged",
        ChannelRoute::AllChannels => "all",
    }
}

/// A source as `list_sources` and the lanes report it. `id` is the identity settings keep.
fn source_info(source: &CaptureSource) -> SourceInfo {
    let kind = match source {
        CaptureSource::System => "system",
        CaptureSource::DefaultMicrophone => "default-microphone",
        CaptureSource::Microphone { .. } => "microphone",
        CaptureSource::Application { .. } => "application",
        CaptureSource::Sender { .. } => "sender",
        CaptureSource::Addon { .. } => "addon",
    };
    SourceInfo {
        id: source.identity(),
        kind: kind.into(),
        name: source.label().into(),
    }
}

/// Every file a save wrote, read back from its header once it is complete.
fn saved_files(file: &ExportedWav) -> Vec<SavedFile> {
    file.paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let positions = path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
            let header = (!positions).then(|| read_header(path)).flatten();
            SavedFile {
                path: path.display().to_string(),
                kind: if positions { "positions" } else { "wav" }.into(),
                lane: file
                    .lanes
                    .get(index)
                    .copied()
                    .flatten()
                    .map(|lane| lane_name(lane).into()),
                channels: header.as_ref().map(|header| u32::from(header.channels)),
                sample_rate: header.as_ref().map(|header| header.rate),
                frames: header.as_ref().map(|header| header.frames),
            }
        })
        .collect()
}

/// The header of a WAV file, from its first megabyte: the data chunk starts well before.
fn read_header(path: &Path) -> Option<harness::WavHeader> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(1 << 20)
        .read_to_end(&mut bytes)
        .ok()?;
    harness::wav_header(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_and_sources_have_the_names_the_contract_lists() {
        let names: Vec<_> = ChannelRoute::ALL.into_iter().map(route_name).collect();
        assert_eq!(
            names,
            ["left", "right", "center", "mid", "side", "merged", "all"]
        );
        let system = source_info(&CaptureSource::System);
        assert_eq!(
            (system.id.as_str(), system.kind.as_str(), system.name.as_str()),
            ("system", "system", "System audio")
        );
        let addon = source_info(&CaptureSource::Addon {
            addon_id: "com.example.test-tone".into(),
            source_id: "main".into(),
            name: "Test tone".into(),
        });
        assert_eq!(addon.id, "addon\tcom.example.test-tone\tmain");
        assert_eq!(addon.kind, "addon");
    }

    #[test]
    fn a_refusal_keeps_its_code_and_reads_as_its_message() {
        let refusal = Refusal::new(codes::NO_AUDIO, "no audio yet");
        assert_eq!(String::from(refusal.clone()), "no audio yet");
        assert_eq!(refusal.error().code, "no_audio");
    }
}

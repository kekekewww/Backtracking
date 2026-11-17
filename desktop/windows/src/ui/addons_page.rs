//! The Add-ons page of the Options window, and the screen that is shown before an add-on is
//! installed.
//!
//! Every call to the add-on manager returns a receiver. The page keeps the receivers and looks
//! at each one once per frame with `try_recv`, so nothing here waits on the UI thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use backtracking_addon::manifest::Manifest;
use backtracking_addon::package::PackageInfo;
use backtracking_addon::protocol::{MessageLevel, Setting, SettingKind};
use backtracking_ui::{
    OPTIONS_CONTROL_OUTLINE, OPTIONS_VALUE_TEXT, muted_accent, options_choice,
    options_control_width, options_field, options_group_heading, options_heading_text,
    options_note, options_value,
};
use eframe::egui::{self, Color32, Stroke};
use serde_json::Value;

use super::text::Language;
use crate::addons::{AddonHandle, AddonInfo, AddonReply, AddonSnapshot, AddonState};

const PACKAGE_EXTENSION: &str = "btaddon";
/// The colour of a state that needs the user: not responding, failed or broken.
const PROBLEM: Color32 = Color32::from_rgb(246, 196, 118);
/// A request the add-on or the manager refused.
const ERROR_TEXT: Color32 = Color32::from_rgb(255, 160, 150);
const STARTING: Color32 = Color32::from_gray(170);
const OFF: Color32 = Color32::from_gray(110);
/// The longest value a text setting may hold (contract section 4.2).
const TEXT_SETTING_LIMIT: usize = 200;
/// The add-on's own key in `problems`, beside its settings' keys.
const ADDON_KEY: &str = "";

/// True for a file named like an add-on package. Its contents are checked by `inspect`.
pub(super) fn is_package(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PACKAGE_EXTENSION))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct DroppedPackage {
    pub(super) path: PathBuf,
    /// Further packages in the same drop, which are not opened.
    pub(super) others: usize,
}

/// The add-on package among dropped files. Files of any other kind are left alone, as they
/// were before add-ons existed.
pub(super) fn dropped_package(paths: &[PathBuf]) -> Option<DroppedPackage> {
    let mut packages = paths.iter().filter(|path| is_package(path));
    let path = packages.next()?.clone();
    Some(DroppedPackage {
        path,
        others: packages.count(),
    })
}

/// True while a package is being dragged over a window.
pub(super) fn package_hovered(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input
            .raw
            .hovered_files
            .iter()
            .any(|file| file.path.as_deref().is_some_and(is_package))
    })
}

/// Lays a quiet "drop here" sign over the window while a package is dragged across it.
pub(super) fn paint_drop_hint(ctx: &egui::Context, accent: Color32, language: Language) {
    if !package_hovered(ctx) {
        return;
    }
    let rect = ctx.content_rect().shrink(3.0);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("addon-drop-hint"),
    ));
    painter.rect(
        rect,
        6.0,
        Color32::from_black_alpha(200),
        Stroke::new(2.0, accent),
        egui::StrokeKind::Inside,
    );
    let font = egui::FontId::proportional(if rect.height() < 120.0 { 13.0 } else { 17.0 });
    let galley = painter.layout(
        language.drop_to_review().to_owned(),
        font,
        Color32::from_gray(235),
        (rect.width() - 24.0).max(40.0),
    );
    let position = rect.center() - galley.size() / 2.0;
    painter.galley(position, galley, Color32::WHITE);
}

/// A homepage is opened only when it is an https address with nothing a shell could misread.
pub(super) fn link_is_openable(link: &str) -> bool {
    link.starts_with("https://")
        && link.len() > "https://".len()
        && link.len() <= 2048
        && !link
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Running,
    Busy,
    Off,
    Problem,
}

/// What an add-on is doing, in plain words.
pub(super) fn state_words(language: Language, state: &AddonState, enabled: bool) -> (Tone, String) {
    match state {
        AddonState::Running => (Tone::Running, language.state_running().into()),
        AddonState::Starting => (Tone::Busy, language.state_starting().into()),
        AddonState::Stopping => (Tone::Busy, language.state_stopping().into()),
        AddonState::Stopped if enabled => (Tone::Off, language.state_stopped().into()),
        AddonState::Stopped => (Tone::Off, language.state_off().into()),
        AddonState::NotResponding => (Tone::Problem, language.state_not_responding().into()),
        AddonState::Failed(reason) => (Tone::Problem, language.state_failed(reason)),
        AddonState::BrokenPackage(reason) => (Tone::Problem, language.state_broken(reason)),
    }
}

/// One line for each capability the manifest declares.
pub(super) fn capability_lines(language: Language, manifest: &Manifest) -> Vec<String> {
    if manifest.capabilities.is_empty() {
        return vec![language.no_capabilities().to_owned()];
    }
    manifest
        .capabilities
        .iter()
        .map(|&capability| language.capability(capability, manifest.open_files.as_ref()))
        .collect()
}

/// What is installed under a package's id now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Installed {
    No,
    Version(String),
    Broken,
}

pub(super) fn installed(snapshot: &AddonSnapshot, id: &str) -> Installed {
    match snapshot.addon(id) {
        None => Installed::No,
        Some(addon) => addon
            .manifest
            .as_ref()
            .map_or(Installed::Broken, |manifest| {
                Installed::Version(manifest.version.clone())
            }),
    }
}

/// A setting's value as its widget holds it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SettingValue {
    Toggle(bool),
    Choice(String),
    Number(f64),
    Text(String),
}

impl SettingValue {
    /// The host's value for a declaration. A value that does not fit the declaration shows
    /// the declared default instead.
    pub(super) fn read(declaration: &Setting, value: &Value) -> Self {
        match &declaration.kind {
            SettingKind::Toggle { default } => Self::Toggle(value.as_bool().unwrap_or(*default)),
            SettingKind::Choice { default, options } => Self::Choice(
                value
                    .as_str()
                    .filter(|value| options.iter().any(|option| option.value == *value))
                    .unwrap_or(default)
                    .to_owned(),
            ),
            SettingKind::Number {
                default, min, max, ..
            } => Self::Number(
                value
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .unwrap_or(*default)
                    .clamp(*min, *max),
            ),
            SettingKind::Text { default } => {
                Self::Text(value.as_str().unwrap_or(default).to_owned())
            }
        }
    }

    /// What `set_setting` sends. A number lands on a whole step from the minimum and inside
    /// the range, and is sent as an integer when it is one; text is cut to the contract's limit.
    pub(super) fn to_json(&self, declaration: &Setting) -> Value {
        match (self, &declaration.kind) {
            (Self::Toggle(value), _) => Value::Bool(*value),
            (Self::Choice(value), _) => Value::String(value.clone()),
            (Self::Number(value), SettingKind::Number { min, max, step, .. }) => {
                number_json(snap_number(*value, *min, *max, *step))
            }
            (Self::Number(value), _) => number_json(*value),
            (Self::Text(value), _) => {
                Value::String(value.chars().take(TEXT_SETTING_LIMIT).collect())
            }
        }
    }
}

/// Puts a value on the nearest step counted from `min`, inside `min..=max`.
pub(super) fn snap_number(value: f64, min: f64, max: f64, step: f64) -> f64 {
    if !value.is_finite() {
        return min;
    }
    let mut snapped = if step > 0.0 {
        min + ((value - min) / step).round() * step
    } else {
        value
    };
    if snapped > max {
        // The top of the range need not be a whole step from the bottom.
        snapped = if step > 0.0 {
            min + ((max - min) / step).floor() * step
        } else {
            max
        };
    }
    // Steps such as 0.1 leave binary noise behind (0.30000000000000004).
    let snapped = (snapped * 1e9).round() / 1e9;
    snapped.clamp(min, max)
}

fn number_json(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        Value::from(value as i64)
    } else {
        serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
    }
}

/// How many decimals a number setting shows, from the size of its step.
pub(super) fn step_decimals(step: f64) -> usize {
    (0..=6)
        .find(|&decimals| {
            let scaled = step * 10f64.powi(decimals as i32);
            (scaled - scaled.round()).abs() < 1e-6
        })
        .unwrap_or(6)
}

/// Something the page asks the window to do, because it needs the window.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PageRequest {
    PickFile,
    OpenLink(String),
}

enum Purpose {
    Switch {
        on: bool,
    },
    Start,
    Change {
        key: String,
        label: String,
    },
    Action {
        action: String,
        label: String,
    },
    /// A new add-on is turned on once it is installed.
    EnableAfterInstall,
}

struct Request {
    id: String,
    purpose: Purpose,
    reply: AddonReply<()>,
}

struct Reviewed {
    info: PackageInfo,
    installed: Installed,
}

enum Stage {
    Reading(AddonReply<PackageInfo>),
    Refused(String),
    Ready(Box<Reviewed>),
    Installing(Box<Reviewed>, AddonReply<PackageInfo>),
    Failed(Box<Reviewed>, String),
}

struct Review {
    path: PathBuf,
    stage: Stage,
    /// Said above the screen, such as that other dropped packages were not opened.
    note: Option<String>,
}

struct Removal {
    id: String,
    name: String,
    delete_data: bool,
    reply: Option<AddonReply<()>>,
    error: Option<String>,
}

enum ReviewChoice {
    Install,
    Close,
    OpenLink(String),
}

/// What the user asked for in one frame. Drawing only records it; `show` passes it to the
/// manager afterwards, so the page can be drawn and tested without one.
#[derive(Debug, PartialEq)]
enum Intent {
    Switch {
        id: String,
        on: bool,
    },
    Start {
        id: String,
    },
    Change {
        id: String,
        key: String,
        label: String,
        value: SettingValue,
        json: Value,
    },
    Action {
        id: String,
        action: String,
        label: String,
    },
    Remove,
    Install,
}

#[derive(Default)]
pub(super) struct AddonsPage {
    snapshot: Arc<AddonSnapshot>,
    review: Option<Review>,
    removal: Option<Removal>,
    requests: Vec<Request>,
    intents: Vec<Intent>,
    /// Values the user has chosen that the host has not confirmed yet, or is still typing.
    drafts: HashMap<(String, String), SettingValue>,
    /// What went wrong, by add-on id and setting key; `ADDON_KEY` is the add-on itself, and
    /// `action:<id>` an action.
    problems: HashMap<(String, String), String>,
    notice: Option<String>,
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

impl AddonsPage {
    /// Takes the newest snapshot and every reply that has arrived. Call once per frame.
    pub(super) fn poll(&mut self, handle: Option<&AddonHandle>, language: Language) {
        if let Some(snapshot) = handle.and_then(AddonHandle::try_snapshot) {
            self.snapshot = snapshot;
        }
        let mut still_waiting = Vec::with_capacity(self.requests.len());
        for request in std::mem::take(&mut self.requests) {
            let Some(result) = reply_state(&request.reply) else {
                still_waiting.push(request);
                continue;
            };
            let id = request.id;
            match (request.purpose, result) {
                (Purpose::Switch { .. } | Purpose::Start | Purpose::EnableAfterInstall, Ok(())) => {
                    self.problems.remove(&(id, ADDON_KEY.into()));
                }
                (Purpose::Start, Err(reason)) => {
                    self.problems
                        .insert((id, ADDON_KEY.into()), language.could_not_start(&reason));
                }
                (Purpose::Switch { on }, Err(reason)) => {
                    self.problems.insert(
                        (id, ADDON_KEY.into()),
                        language.could_not_switch(on, &reason),
                    );
                }
                (Purpose::EnableAfterInstall, Err(reason)) => {
                    self.problems.insert(
                        (id, ADDON_KEY.into()),
                        language.could_not_switch(true, &reason),
                    );
                }
                (Purpose::Change { key, .. }, Ok(())) => {
                    self.drafts.remove(&(id.clone(), key.clone()));
                    self.problems.remove(&(id, key));
                }
                (Purpose::Change { key, label }, Err(reason)) => {
                    // The control goes back to the value the host holds.
                    self.drafts.remove(&(id.clone(), key.clone()));
                    self.problems
                        .insert((id, key), language.could_not_change(&label, &reason));
                }
                (Purpose::Action { action, .. }, Ok(())) => {
                    self.problems.remove(&(id, format!("action:{action}")));
                }
                (Purpose::Action { action, label }, Err(reason)) => {
                    self.problems.insert(
                        (id, format!("action:{action}")),
                        language.could_not_run(&label, &reason),
                    );
                }
            }
        }
        self.requests = still_waiting;

        let read = match self.review.as_ref().map(|review| &review.stage) {
            Some(Stage::Reading(reply)) => reply_state(reply),
            _ => None,
        };
        if let (Some(result), Some(review)) = (read, self.review.as_mut()) {
            review.stage = match result {
                Ok(info) => {
                    let installed = installed(&self.snapshot, &info.manifest.id);
                    Stage::Ready(Box::new(Reviewed { info, installed }))
                }
                Err(reason) => Stage::Refused(reason),
            };
        }
        let finished = match self.review.as_ref().map(|review| &review.stage) {
            Some(Stage::Installing(_, reply)) => reply_state(reply),
            _ => None,
        };
        if let Some(result) = finished
            && let Some(Review {
                path,
                stage: Stage::Installing(reviewed, _),
                note,
            }) = self.review.take()
        {
            match result {
                Ok(info) => {
                    let manifest = &info.manifest;
                    self.notice = Some(language.installed(&manifest.name, &manifest.version));
                    if reviewed.installed == Installed::No
                        && let Some(handle) = handle
                    {
                        self.requests.push(Request {
                            id: manifest.id.clone(),
                            purpose: Purpose::EnableAfterInstall,
                            reply: handle.set_enabled(&manifest.id, true),
                        });
                    }
                }
                Err(reason) => {
                    self.review = Some(Review {
                        path,
                        stage: Stage::Failed(reviewed, reason),
                        note,
                    });
                }
            }
        }

        if let Some(removal) = &mut self.removal
            && let Some(result) = removal.reply.as_ref().and_then(reply_state)
        {
            removal.reply = None;
            match result {
                Ok(()) => {
                    self.notice = Some(language.removed(&removal.name));
                    let id = removal.id.clone();
                    self.removal = None;
                    self.drafts.retain(|(addon, _), _| *addon != id);
                    self.problems.retain(|(addon, _), _| *addon != id);
                }
                Err(reason) => removal.error = Some(language.could_not_remove(&reason)),
            }
        }
    }

    /// True while a reply is still to come.
    pub(super) fn waiting(&self) -> bool {
        !self.requests.is_empty()
            || self
                .removal
                .as_ref()
                .is_some_and(|removal| removal.reply.is_some())
            || self.review.as_ref().is_some_and(|review| {
                matches!(review.stage, Stage::Reading(_) | Stage::Installing(..))
            })
    }

    /// Opens the confirmation for a package. Nothing is written until the user chooses Install.
    pub(super) fn review_package(
        &mut self,
        path: PathBuf,
        others: usize,
        handle: Option<&AddonHandle>,
        language: Language,
    ) {
        if self
            .review
            .as_ref()
            .is_some_and(|review| matches!(review.stage, Stage::Installing(..)))
        {
            self.notice = Some(language.wait_for_install().into());
            return;
        }
        let Some(handle) = handle else {
            self.notice = Some(language.addons_unavailable().into());
            return;
        };
        self.notice = None;
        self.review = Some(Review {
            stage: Stage::Reading(handle.inspect(path.clone())),
            path,
            note: (others > 0).then(|| language.other_drops_ignored(others)),
        });
    }

    /// Opens the question that comes before removing an add-on.
    pub(super) fn ask_to_remove(&mut self, id: &str) -> Result<(), String> {
        let addon = self
            .snapshot
            .addon(id)
            .ok_or_else(|| format!("{id} is not installed"))?;
        self.removal = Some(Removal {
            id: id.to_owned(),
            name: addon_name(addon).to_owned(),
            delete_data: false,
            reply: None,
            error: None,
        });
        Ok(())
    }

    /// Escape closes an open question before it closes the window. True when it did.
    pub(super) fn cancel_on_escape(&mut self) -> bool {
        if let Some(review) = &self.review {
            if matches!(review.stage, Stage::Installing(..)) {
                return true;
            }
            self.review = None;
            return true;
        }
        if let Some(removal) = &self.removal {
            if removal.reply.is_none() {
                self.removal = None;
            }
            return true;
        }
        false
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        handle: Option<&AddonHandle>,
        language: Language,
        accent: Color32,
    ) -> Option<PageRequest> {
        let Some(handle) = handle else {
            options_note(ui, language.addons_unavailable());
            return None;
        };
        let request = self.draw(ui, language, accent);
        for intent in std::mem::take(&mut self.intents) {
            self.carry_out(intent, handle);
        }
        request
    }

    /// Passes one thing the user asked for to the manager and keeps its reply.
    fn carry_out(&mut self, intent: Intent, handle: &AddonHandle) {
        match intent {
            Intent::Switch { id, on } => {
                self.problems.remove(&(id.clone(), ADDON_KEY.into()));
                let reply = handle.set_enabled(&id, on);
                self.requests.push(Request {
                    id,
                    purpose: Purpose::Switch { on },
                    reply,
                });
            }
            Intent::Start { id } => {
                self.problems.remove(&(id.clone(), ADDON_KEY.into()));
                let reply = handle.start(&id);
                self.requests.push(Request {
                    id,
                    purpose: Purpose::Start,
                    reply,
                });
            }
            Intent::Change {
                id,
                key,
                label,
                value,
                json,
            } => {
                let slot = (id.clone(), key.clone());
                self.problems.remove(&slot);
                // The control keeps showing the new value until the host confirms it.
                self.drafts.insert(slot, value);
                let reply = handle.set_setting(&id, &key, json);
                self.requests.push(Request {
                    id,
                    purpose: Purpose::Change { key, label },
                    reply,
                });
            }
            Intent::Action { id, action, label } => {
                self.problems
                    .remove(&(id.clone(), format!("action:{action}")));
                let reply = handle.run_action(&id, &action);
                self.requests.push(Request {
                    id,
                    purpose: Purpose::Action { action, label },
                    reply,
                });
            }
            Intent::Remove => {
                if let Some(removal) = &mut self.removal {
                    removal.error = None;
                    removal.reply = Some(handle.remove(&removal.id, removal.delete_data));
                }
            }
            Intent::Install => {
                if let Some(Review {
                    path,
                    stage: Stage::Ready(reviewed),
                    note,
                }) = self.review.take()
                {
                    let reply = handle.install(path.clone());
                    self.review = Some(Review {
                        path,
                        stage: Stage::Installing(reviewed, reply),
                        note,
                    });
                }
            }
        }
    }

    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        language: Language,
        accent: Color32,
    ) -> Option<PageRequest> {
        if let Some(review) = &self.review {
            return match review_screen(ui, review, language, accent) {
                Some(ReviewChoice::Install) => {
                    self.intents.push(Intent::Install);
                    None
                }
                Some(ReviewChoice::Close) => {
                    self.review = None;
                    None
                }
                Some(ReviewChoice::OpenLink(link)) => Some(PageRequest::OpenLink(link)),
                None => None,
            };
        }

        let mut request = None;
        if let Some(notice) = &self.notice {
            options_value(ui, notice);
        }
        let snapshot = Arc::clone(&self.snapshot);
        if let Some(error) = &snapshot.error {
            problem_line(ui, error);
        }
        let empty = snapshot.addons.is_empty();
        if empty {
            ui.label(egui::RichText::new(language.addons_empty()).color(OPTIONS_VALUE_TEXT));
            ui.label(language.addons_explained());
            ui.label(language.addons_how_to_add());
        }
        for addon in &snapshot.addons {
            ui.push_id(&addon.id, |ui| self.addon_card(ui, addon, language, accent));
        }
        if !empty {
            ui.separator();
        }
        if ui.button(language.install_from_file()).clicked() {
            request = Some(PageRequest::PickFile);
        }
        // The empty page has just said so.
        if !empty {
            options_note(ui, language.drop_hint());
        }
        request
    }

    /// Presses Install on a confirmation that is ready, as the user would. For the test harness.
    pub(super) fn press_install(&mut self) -> bool {
        let ready = self
            .review
            .as_ref()
            .is_some_and(|review| matches!(review.stage, Stage::Ready(_)));
        if ready {
            self.intents.push(Intent::Install);
        }
        ready
    }

    /// Changes a setting as its control would, with any value, so that a refusal can be seen.
    /// For the test harness; the change is sent when the page is next drawn.
    pub(super) fn change_setting(
        &mut self,
        id: &str,
        key: &str,
        json: Value,
    ) -> Result<(), String> {
        let setting = self
            .snapshot
            .addon(id)
            .and_then(|addon| {
                addon
                    .settings
                    .iter()
                    .find(|setting| setting.declaration.key == key)
            })
            .ok_or_else(|| format!("{id} has no setting {key} now"))?;
        let declaration = &setting.declaration;
        self.intents.push(Intent::Change {
            id: id.to_owned(),
            key: key.to_owned(),
            label: declaration.label.clone(),
            value: SettingValue::read(declaration, &json),
            json,
        });
        Ok(())
    }

    fn addon_card(
        &mut self,
        ui: &mut egui::Ui,
        addon: &AddonInfo,
        language: Language,
        accent: Color32,
    ) {
        egui::Frame::new()
            .stroke(Stroke::new(1.0, OPTIONS_CONTROL_OUTLINE))
            .corner_radius(6)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.0;
                let switching = self
                    .requests
                    .iter()
                    .find_map(|request| match request.purpose {
                        Purpose::Switch { on } if request.id == addon.id => Some(on),
                        _ => None,
                    });
                let mut enabled = switching.unwrap_or(addon.enabled);
                let can_switch = switching.is_none()
                    && (addon.manifest.is_some() || addon.enabled)
                    && self
                        .removal
                        .as_ref()
                        .is_none_or(|removal| removal.id != addon.id);
                let switched = ui
                    .horizontal(|ui| {
                        ui.label(options_heading_text(ui, addon_name(addon), 16.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_enabled(
                                can_switch,
                                egui::Checkbox::new(&mut enabled, language.enabled()),
                            )
                            .changed()
                        })
                        .inner
                    })
                    .inner;
                if switched {
                    self.intents.push(Intent::Switch {
                        id: addon.id.clone(),
                        on: enabled,
                    });
                }
                // A broken package has no manifest, and its id already stands in for its name.
                if let Some(manifest) = &addon.manifest {
                    options_note(ui, language.version_by(&manifest.version, &manifest.author));
                    if !manifest.description.is_empty() {
                        ui.label(&manifest.description);
                    }
                }

                let (tone, words) = state_words(language, &addon.state, addon.enabled);
                state_line(ui, tone, &words, accent);
                if matches!(addon.state, AddonState::BrokenPackage(_)) {
                    options_note(ui, language.broken_hint());
                }
                // The restart limit holds until the user starts it again; the saved switch stays.
                let starting = self.requests.iter().any(|request| {
                    request.id == addon.id && matches!(request.purpose, Purpose::Start)
                });
                if matches!(
                    addon.state,
                    AddonState::Failed(_) | AddonState::NotResponding
                ) && addon.enabled
                    && switching.is_none()
                    && ui
                        .add_enabled(!starting, egui::Button::new(language.start_again()))
                        .clicked()
                {
                    self.intents.push(Intent::Start {
                        id: addon.id.clone(),
                    });
                }
                if let Some(message) = &addon.message {
                    let color = match message.level {
                        MessageLevel::Info => OPTIONS_VALUE_TEXT,
                        MessageLevel::Warning => PROBLEM,
                        MessageLevel::Error => ERROR_TEXT,
                    };
                    ui.label(
                        egui::RichText::new(language.addon_message(message.level, &message.text))
                            .color(color),
                    );
                }
                if let Some(problem) = self.problems.get(&(addon.id.clone(), ADDON_KEY.into())) {
                    problem_line(ui, problem);
                }

                let described = !addon.settings.is_empty() || !addon.actions.is_empty();
                if addon.state == AddonState::Running {
                    self.settings_and_actions(ui, addon, language);
                } else if matches!(tone, Tone::Off | Tone::Busy)
                    && addon.manifest.as_ref().is_some_and(|manifest| {
                        manifest.capabilities.iter().any(|capability| {
                            matches!(
                                capability,
                                backtracking_addon::manifest::Capability::Settings
                                    | backtracking_addon::manifest::Capability::Actions
                            )
                        })
                    })
                {
                    options_note(ui, language.not_running_hint());
                }

                if addon.state == AddonState::Running && described {
                    // Removing is not one of the add-on's own buttons.
                    ui.separator();
                }
                self.removal_row(ui, addon, language);
            });
    }

    fn settings_and_actions(&mut self, ui: &mut egui::Ui, addon: &AddonInfo, language: Language) {
        if !addon.settings.is_empty() {
            ui.add_space(2.0);
            options_group_heading(ui, language.settings_heading());
        }
        for setting in &addon.settings {
            let declaration = &setting.declaration;
            let slot = (addon.id.clone(), declaration.key.clone());
            let pending = self.requests.iter().any(|request| {
                request.id == addon.id
                    && matches!(&request.purpose, Purpose::Change { key, .. } if *key == declaration.key)
            });
            let host = SettingValue::read(declaration, &setting.value);
            let shown = self
                .drafts
                .get(&slot)
                .cloned()
                .unwrap_or_else(|| host.clone());
            let (chosen, commit) = ui
                .push_id(&declaration.key, |ui| {
                    ui.add_enabled_ui(!pending, |ui| setting_control(ui, declaration, shown))
                        .inner
                })
                .inner;
            if let Some(help) = &declaration.help {
                options_note(ui, help);
            }
            if let Some(problem) = self.problems.get(&slot) {
                problem_line(ui, problem);
            }
            match (chosen, commit) {
                (Some(value), true) if value != host => {
                    let json = value.to_json(declaration);
                    // Shown as it will be stored: a dragged number lands on its step.
                    let value = SettingValue::read(declaration, &json);
                    self.drafts.insert(slot, value.clone());
                    self.intents.push(Intent::Change {
                        id: addon.id.clone(),
                        key: declaration.key.clone(),
                        label: declaration.label.clone(),
                        value,
                        json,
                    });
                }
                (Some(_), true) => {
                    self.drafts.remove(&slot);
                }
                (Some(value), false) => {
                    self.drafts.insert(slot, value);
                }
                (None, _) => {}
            }
        }
        if !addon.actions.is_empty() {
            ui.add_space(2.0);
            options_group_heading(ui, language.actions_heading());
            let pressed = ui
                .horizontal_wrapped(|ui| {
                    let mut pressed = None;
                    for action in &addon.actions {
                        let busy = self.requests.iter().any(|request| {
                            request.id == addon.id
                                && matches!(&request.purpose, Purpose::Action { action: id, .. } if *id == action.id)
                        });
                        if ui.add_enabled(!busy, egui::Button::new(&action.label)).clicked() {
                            pressed = Some(action);
                        }
                    }
                    pressed
                })
                .inner;
            if let Some(action) = pressed {
                self.intents.push(Intent::Action {
                    id: addon.id.clone(),
                    action: action.id.clone(),
                    label: action.label.clone(),
                });
            }
            for action in &addon.actions {
                if let Some(problem) = self
                    .problems
                    .get(&(addon.id.clone(), format!("action:{}", action.id)))
                {
                    problem_line(ui, problem);
                }
            }
        }
    }

    fn removal_row(&mut self, ui: &mut egui::Ui, addon: &AddonInfo, language: Language) {
        let Some(removal) = self
            .removal
            .as_mut()
            .filter(|removal| removal.id == addon.id)
        else {
            if ui.button(language.remove_button()).clicked() {
                self.removal = Some(Removal {
                    id: addon.id.clone(),
                    name: addon_name(addon).to_owned(),
                    delete_data: false,
                    reply: None,
                    error: None,
                });
            }
            return;
        };
        let mut close = false;
        let mut remove = false;
        egui::Frame::new()
            .fill(Color32::from_gray(34))
            .stroke(Stroke::new(1.0, OPTIONS_CONTROL_OUTLINE))
            .corner_radius(6)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(options_heading_text(
                    ui,
                    language.remove_question(&removal.name),
                    15.0,
                ));
                ui.label(language.remove_explained());
                let removing = removal.reply.is_some();
                ui.add_enabled(
                    !removing,
                    egui::Checkbox::new(&mut removal.delete_data, language.delete_data()),
                );
                if !removal.delete_data {
                    options_note(ui, language.keep_data_note());
                }
                if let Some(error) = &removal.error {
                    problem_line(ui, error);
                }
                ui.horizontal_wrapped(|ui| {
                    if removing {
                        ui.spinner();
                        ui.label(language.removing());
                        return;
                    }
                    remove = ui.button(language.remove()).clicked();
                    close = ui.button(language.cancel()).clicked();
                });
            });
        if remove {
            self.intents.push(Intent::Remove);
        } else if close {
            self.removal = None;
        }
    }
}

fn addon_name(addon: &AddonInfo) -> &str {
    addon
        .manifest
        .as_ref()
        .map_or(addon.id.as_str(), |manifest| manifest.name.as_str())
}

fn problem_line(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(ERROR_TEXT));
}

fn state_line(ui: &mut egui::Ui, tone: Tone, words: &str, accent: Color32) {
    let color = match tone {
        Tone::Running => accent,
        Tone::Busy => STARTING,
        Tone::Off => OFF,
        Tone::Problem => PROBLEM,
    };
    ui.horizontal_top(|ui| {
        let size = egui::Vec2::new(10.0, ui.text_style_height(&egui::TextStyle::Body));
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        let text_color = if tone == Tone::Problem {
            PROBLEM
        } else {
            OPTIONS_VALUE_TEXT
        };
        ui.add(egui::Label::new(egui::RichText::new(words).color(text_color)).wrap());
    });
}

/// Draws one setting's control. Returns the value it now shows, when the user touched it, and
/// whether that value is final (a click or a finished edit) rather than still being typed or
/// dragged.
fn setting_control(
    ui: &mut egui::Ui,
    declaration: &Setting,
    shown: SettingValue,
) -> (Option<SettingValue>, bool) {
    match (&declaration.kind, shown) {
        (SettingKind::Toggle { .. }, SettingValue::Toggle(mut value)) => {
            let changed = ui.checkbox(&mut value, &declaration.label).changed();
            (changed.then_some(SettingValue::Toggle(value)), true)
        }
        (SettingKind::Choice { options, .. }, SettingValue::Choice(value)) => {
            let before = options
                .iter()
                .position(|option| option.value == value)
                .unwrap_or(0);
            let mut index = before;
            options_field(ui, &declaration.label, |ui| {
                options_choice(
                    ui,
                    "choice",
                    &mut index,
                    options
                        .iter()
                        .enumerate()
                        .map(|(index, option)| (index, option.label.clone())),
                );
            });
            let chosen = (index != before)
                .then(|| {
                    options
                        .get(index)
                        .map(|option| SettingValue::Choice(option.value.clone()))
                })
                .flatten();
            (chosen, true)
        }
        (SettingKind::Number { min, max, step, .. }, SettingValue::Number(mut value)) => {
            let (min, max, step) = (*min, *max, *step);
            options_field(ui, &declaration.label, |ui| {
                ui.horizontal(|ui| {
                    let down = ui
                        .add_enabled(
                            value > min,
                            egui::Button::new("−").min_size(egui::vec2(32.0, 0.0)),
                        )
                        .clicked();
                    let field = ui.add(
                        egui::DragValue::new(&mut value)
                            .range(min..=max)
                            .speed(step)
                            .fixed_decimals(step_decimals(step))
                            .update_while_editing(false)
                            .min_size(egui::vec2(96.0, 0.0)),
                    );
                    let up = ui
                        .add_enabled(
                            value < max,
                            egui::Button::new("+").min_size(egui::vec2(32.0, 0.0)),
                        )
                        .clicked();
                    if down || up {
                        let moved = if up { value + step } else { value - step };
                        let value = snap_number(moved, min, max, step);
                        return (Some(SettingValue::Number(value)), true);
                    }
                    if field.drag_stopped() {
                        return (Some(SettingValue::Number(value)), true);
                    }
                    if field.changed() {
                        // A drag sends its value when it ends; a typed value is final.
                        return (Some(SettingValue::Number(value)), !field.dragged());
                    }
                    (None, false)
                })
                .inner
            })
        }
        (SettingKind::Text { .. }, SettingValue::Text(mut value)) => {
            options_field(ui, &declaration.label, |ui| {
                let width = options_control_width(ui);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut value)
                        .char_limit(TEXT_SETTING_LIMIT)
                        .desired_width(width),
                );
                if field.lost_focus() {
                    (Some(SettingValue::Text(value)), true)
                } else if field.changed() {
                    (Some(SettingValue::Text(value)), false)
                } else {
                    (None, false)
                }
            })
        }
        // A value read from the declaration always matches its kind.
        _ => (None, false),
    }
}

/// The confirmation, from reading the package to the end of the install.
fn review_screen(
    ui: &mut egui::Ui,
    review: &Review,
    language: Language,
    accent: Color32,
) -> Option<ReviewChoice> {
    let mut choice = None;
    ui.label(options_heading_text(ui, language.review_heading(), 17.0));
    if let Some(note) = &review.note {
        options_note(ui, note);
    }
    let file_name = review.path.file_name().map_or_else(
        || review.path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let reviewed = match &review.stage {
        Stage::Reading(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(language.reading(&file_name));
            });
            return None;
        }
        Stage::Refused(reason) => {
            ui.label(egui::RichText::new(language.refused_heading()).color(OPTIONS_VALUE_TEXT));
            problem_line(ui, reason);
            options_field(ui, language.package_file(), |ui| {
                options_value(ui, &review.path.display().to_string());
            });
            if ui.button(language.close()).clicked() {
                choice = Some(ReviewChoice::Close);
            }
            return choice;
        }
        Stage::Ready(reviewed) | Stage::Installing(reviewed, _) | Stage::Failed(reviewed, _) => {
            reviewed
        }
    };
    let info = &reviewed.info;
    let manifest = &info.manifest;
    ui.add_space(4.0);
    ui.label(options_heading_text(ui, &manifest.name, 20.0));
    options_note(ui, language.version_by(&manifest.version, &manifest.author));
    if !manifest.description.is_empty() {
        ui.label(egui::RichText::new(&manifest.description).color(OPTIONS_VALUE_TEXT));
    }
    match &reviewed.installed {
        Installed::No => {}
        Installed::Version(old) => {
            ui.label(egui::RichText::new(language.replaces(old, &manifest.version)).color(PROBLEM));
        }
        Installed::Broken => {
            ui.label(egui::RichText::new(language.replaces_broken()).color(PROBLEM));
        }
    }
    if let Some(homepage) = &manifest.homepage {
        options_field(ui, language.homepage(), |ui| {
            if link_is_openable(homepage) {
                if ui.link(homepage).on_hover_text(homepage).clicked() {
                    choice = Some(ReviewChoice::OpenLink(homepage.clone()));
                }
            } else {
                options_value(ui, homepage);
            }
        });
    }
    if let Some(license) = &manifest.license {
        options_field(ui, language.license(), |ui| options_value(ui, license));
    }
    options_field(ui, language.program(), |ui| {
        options_value(ui, &info.program)
    });
    options_field(ui, language.unpacked_size(), |ui| {
        options_value(
            ui,
            &language.size_in_files(info.unpacked_size, info.file_count),
        );
    });
    if !review.path.as_os_str().is_empty() {
        options_field(ui, language.package_file(), |ui| {
            options_value(ui, &review.path.display().to_string());
        });
    }
    ui.separator();
    options_group_heading(ui, language.asks_heading());
    for line in capability_lines(language, manifest) {
        ui.horizontal_top(|ui| {
            ui.label("•");
            ui.add(egui::Label::new(egui::RichText::new(line).color(OPTIONS_VALUE_TEXT)).wrap());
        });
    }
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(Color32::from_gray(32))
        .stroke(Stroke::new(1.0, OPTIONS_CONTROL_OUTLINE))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(language.trust_warning()).color(OPTIONS_VALUE_TEXT));
        });
    if reviewed.installed == Installed::No {
        options_note(ui, language.starts_note());
    }
    match &review.stage {
        Stage::Ready(_) => {
            ui.horizontal_wrapped(|ui| {
                let install = egui::Button::new(
                    egui::RichText::new(language.install()).color(Color32::WHITE),
                )
                .fill(muted_accent(accent));
                if ui.add(install).clicked() {
                    choice = Some(ReviewChoice::Install);
                }
                if ui.button(language.cancel()).clicked() {
                    choice = Some(ReviewChoice::Close);
                }
            });
        }
        Stage::Installing(..) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(language.installing());
            });
        }
        Stage::Failed(_, reason) => {
            problem_line(ui, &language.install_failed(reason));
            let kept = match &reviewed.installed {
                Installed::No => language.nothing_installed().to_owned(),
                Installed::Version(old) => language.previous_kept(old),
                Installed::Broken => language.previous_broken_kept().to_owned(),
            };
            ui.label(egui::RichText::new(kept).color(OPTIONS_VALUE_TEXT));
            if ui.button(language.close()).clicked() {
                choice = Some(ReviewChoice::Close);
            }
        }
        Stage::Reading(_) | Stage::Refused(_) => {}
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::{AddonNotice, AddonSetting};
    use Language::{English, TraditionalChinese};
    use backtracking_addon::protocol::{Action, ChoiceOption};

    fn manifest(capabilities: &str, open_files: &str) -> Manifest {
        Manifest::parse(&format!(
            r#"{{"contract":1,"id":"com.example.tone","name":"Tone","version":"0.2.0",
                "author":"Example","description":"A tone.","capabilities":[{capabilities}]{open_files},
                "run":{{"windows-x86_64":{{"program":"bin/tone.exe"}},
                        "windows-aarch64":{{"program":"bin/tone.exe"}}}}}}"#
        ))
        .unwrap()
    }

    fn package(manifest: Manifest) -> PackageInfo {
        PackageInfo {
            manifest,
            unpacked_size: 4096,
            entry_count: 3,
            file_count: 2,
            program: "bin/tone.exe".into(),
        }
    }

    fn addon(id: &str, manifest: Option<Manifest>, state: AddonState) -> AddonInfo {
        AddonInfo {
            id: id.into(),
            manifest,
            enabled: true,
            state,
            pid: None,
            message: None,
            sources: Vec::new(),
            settings: Vec::new(),
            actions: Vec::new(),
        }
    }

    fn number(min: f64, max: f64, step: f64) -> Setting {
        Setting {
            key: "frequency".into(),
            label: "Frequency (Hz)".into(),
            help: None,
            kind: SettingKind::Number {
                default: min,
                min,
                max,
                step,
            },
        }
    }

    #[test]
    fn only_add_on_packages_are_taken_from_a_drop() {
        let paths = |names: &[&str]| names.iter().map(PathBuf::from).collect::<Vec<_>>();
        assert_eq!(dropped_package(&[]), None);
        assert_eq!(
            dropped_package(&paths(&[r"C:\clips\take.wav", r"C:\notes.txt"])),
            None
        );
        assert_eq!(
            dropped_package(&paths(&[r"C:\a\tone.btaddon.zip", r"C:\a\btaddon"])),
            None
        );
        assert_eq!(
            dropped_package(&paths(&[
                r"C:\clips\take.wav",
                r"C:\Downloads\Tone.BTADDON"
            ])),
            Some(DroppedPackage {
                path: PathBuf::from(r"C:\Downloads\Tone.BTADDON"),
                others: 0
            })
        );
        assert_eq!(
            dropped_package(&paths(&[
                r"C:\a.btaddon",
                r"C:\b.wav",
                r"C:\c.btaddon",
                r"C:\d.btaddon"
            ])),
            Some(DroppedPackage {
                path: PathBuf::from(r"C:\a.btaddon"),
                others: 2
            })
        );
    }

    #[test]
    fn every_state_reads_as_plain_words_with_its_tone() {
        let cases = [
            (AddonState::Running, true, Tone::Running, "Running"),
            (AddonState::Starting, true, Tone::Busy, "Starting…"),
            (AddonState::Stopping, false, Tone::Busy, "Stopping…"),
            (AddonState::Stopped, true, Tone::Off, "Stopped"),
            (AddonState::Stopped, false, Tone::Off, "Off"),
            (
                AddonState::NotResponding,
                true,
                Tone::Problem,
                "Not responding: it did not connect within 10 seconds, so it was stopped.",
            ),
            (
                AddonState::Failed(
                    "Add-on exited with code 1; stopped after 3 exits within 60 seconds".into(),
                ),
                true,
                Tone::Problem,
                "Stopped because of a problem: Add-on exited with code 1; stopped after 3 exits within 60 seconds",
            ),
            (
                AddonState::BrokenPackage("addon.json is missing".into()),
                false,
                Tone::Problem,
                "Broken package: addon.json is missing",
            ),
        ];
        for (state, enabled, tone, words) in cases {
            assert_eq!(
                state_words(English, &state, enabled),
                (tone, words.to_owned())
            );
            let (chinese_tone, chinese) = state_words(TraditionalChinese, &state, enabled);
            assert_eq!(chinese_tone, tone);
            assert_ne!(chinese, words);
        }
        let (_, broken) = state_words(
            TraditionalChinese,
            &AddonState::BrokenPackage("x".into()),
            true,
        );
        assert_eq!(broken, "套件已損壞：x");
    }

    #[test]
    fn each_capability_gets_one_plain_line() {
        let test_tone = manifest(r#""source","settings","actions""#, "");
        assert_eq!(
            capability_lines(English, &test_tone),
            [
                "Provide audio sources in the source list",
                "Have settings, which Backtracking shows and keeps",
                "Add buttons to its entry on the Add-ons page",
            ]
        );
        let files = manifest(
            r#""open-files","clips""#,
            r#","open_files":{"description":"Audio files","extensions":["wav","mp4"]}"#,
        );
        assert_eq!(
            capability_lines(English, &files),
            [
                "Open files of these types: Audio files (.wav, .mp4)",
                "Be told about every clip you export, with its file path",
            ]
        );
        assert_eq!(
            capability_lines(TraditionalChinese, &files)[0],
            "開啟這些類型的檔案：Audio files（.wav、.mp4）"
        );
        assert_eq!(
            capability_lines(English, &manifest("", "")),
            ["Nothing beyond running its program."]
        );
        for capability in ["source", "open-files", "settings", "actions", "clips"] {
            let extra = if capability == "open-files" {
                r#","open_files":{"description":"Files","extensions":["wav"]}"#
            } else {
                ""
            };
            let manifest = manifest(&format!("\"{capability}\""), extra);
            let english = capability_lines(English, &manifest);
            let chinese = capability_lines(TraditionalChinese, &manifest);
            assert_eq!(english.len(), 1);
            assert_ne!(english, chinese, "{capability}");
        }
    }

    #[test]
    fn settings_show_the_host_value_or_the_default_when_it_does_not_fit() {
        let toggle = Setting {
            key: "mute".into(),
            label: "Mute".into(),
            help: None,
            kind: SettingKind::Toggle { default: false },
        };
        assert_eq!(
            SettingValue::read(&toggle, &Value::Bool(true)),
            SettingValue::Toggle(true)
        );
        assert_eq!(
            SettingValue::read(&toggle, &Value::from("yes")),
            SettingValue::Toggle(false)
        );
        let choice = Setting {
            key: "level".into(),
            label: "Level".into(),
            help: None,
            kind: SettingKind::Choice {
                default: "-12".into(),
                options: vec![
                    ChoiceOption {
                        value: "-12".into(),
                        label: "-12 dB".into(),
                    },
                    ChoiceOption {
                        value: "-6".into(),
                        label: "-6 dB".into(),
                    },
                ],
            },
        };
        assert_eq!(
            SettingValue::read(&choice, &Value::from("-6")),
            SettingValue::Choice("-6".into())
        );
        assert_eq!(
            SettingValue::read(&choice, &Value::from("0")),
            SettingValue::Choice("-12".into())
        );
        let frequency = number(20.0, 20000.0, 1.0);
        assert_eq!(
            SettingValue::read(&frequency, &Value::from(440)),
            SettingValue::Number(440.0)
        );
        assert_eq!(
            SettingValue::read(&frequency, &Value::from(1e9)),
            SettingValue::Number(20000.0)
        );
        assert_eq!(
            SettingValue::read(&frequency, &Value::Null),
            SettingValue::Number(20.0)
        );
        let text = Setting {
            key: "title".into(),
            label: "Title".into(),
            help: None,
            kind: SettingKind::Text {
                default: "Untitled".into(),
            },
        };
        assert_eq!(
            SettingValue::read(&text, &Value::from(3)),
            SettingValue::Text("Untitled".into())
        );
        assert_eq!(
            SettingValue::read(&text, &Value::from("Mix")),
            SettingValue::Text("Mix".into())
        );
        // What is sent back keeps the kind, and text stays inside the contract's limit.
        assert_eq!(
            SettingValue::Toggle(true).to_json(&toggle),
            Value::Bool(true)
        );
        assert_eq!(
            SettingValue::Choice("-6".into()).to_json(&choice),
            Value::from("-6")
        );
        let long = "a".repeat(250);
        assert_eq!(
            SettingValue::Text(long)
                .to_json(&text)
                .as_str()
                .unwrap()
                .chars()
                .count(),
            TEXT_SETTING_LIMIT
        );
    }

    #[test]
    fn numbers_land_on_a_step_inside_the_range() {
        let frequency = number(20.0, 20000.0, 1.0);
        assert_eq!(
            SettingValue::Number(440.4).to_json(&frequency),
            serde_json::json!(440)
        );
        assert!(SettingValue::Number(440.4).to_json(&frequency).is_i64());
        assert_eq!(
            SettingValue::Number(99999.0).to_json(&frequency),
            serde_json::json!(20000)
        );
        assert_eq!(
            SettingValue::Number(-5.0).to_json(&frequency),
            serde_json::json!(20)
        );
        assert_eq!(snap_number(1.26, 0.0, 2.0, 0.5), 1.5);
        assert_eq!(snap_number(0.3, 0.0, 1.0, 0.1), 0.3);
        assert_eq!(snap_number(0.7, 0.0, 1.0, 0.25), 0.75);
        // A top that is not a whole step from the bottom keeps the value on a step.
        assert_eq!(snap_number(10.0, 0.0, 9.5, 2.0), 8.0);
        assert_eq!(snap_number(f64::NAN, 3.0, 9.0, 1.0), 3.0);
        let fine = number(0.0, 1.0, 0.1);
        assert_eq!(
            SettingValue::Number(0.34).to_json(&fine),
            serde_json::json!(0.3)
        );
        assert_eq!(step_decimals(1.0), 0);
        assert_eq!(step_decimals(5.0), 0);
        assert_eq!(step_decimals(0.5), 1);
        assert_eq!(step_decimals(0.25), 2);
        assert_eq!(step_decimals(0.001), 3);
        assert_eq!(step_decimals(1.0 / 3.0), 6);
    }

    #[test]
    fn only_plain_https_links_are_opened() {
        assert!(link_is_openable("https://example.com/test-tone"));
        assert!(!link_is_openable("https://"));
        assert!(!link_is_openable("http://example.com"));
        assert!(!link_is_openable("https://example.com/a b"));
        assert!(!link_is_openable("https://example.com/\"x"));
        assert!(!link_is_openable("file:///C:/Windows"));
    }

    #[test]
    fn what_is_installed_comes_from_the_snapshot() {
        let snapshot = AddonSnapshot {
            revision: 1,
            addons: vec![
                addon(
                    "com.example.tone",
                    Some(manifest("", "")),
                    AddonState::Running,
                ),
                addon(
                    "com.example.broken",
                    None,
                    AddonState::BrokenPackage("bad".into()),
                ),
            ],
            error: None,
        };
        assert_eq!(
            installed(&snapshot, "com.example.tone"),
            Installed::Version("0.2.0".into())
        );
        assert_eq!(
            installed(&snapshot, "com.example.broken"),
            Installed::Broken
        );
        assert_eq!(installed(&snapshot, "com.example.other"), Installed::No);
    }

    fn page_with(stage: Stage) -> AddonsPage {
        AddonsPage {
            review: Some(Review {
                path: PathBuf::from(r"C:\Downloads\tone.btaddon"),
                stage,
                note: None,
            }),
            ..AddonsPage::default()
        }
    }

    #[test]
    fn the_confirmation_follows_the_replies_and_keeps_the_reason() {
        // Reading, then ready: an installed version is named.
        let (send, reply) = mpsc::channel();
        let mut page = page_with(Stage::Reading(reply));
        page.snapshot = Arc::new(AddonSnapshot {
            revision: 1,
            addons: vec![addon(
                "com.example.tone",
                Some(manifest("", "")),
                AddonState::Running,
            )],
            error: None,
        });
        page.poll(None, English);
        assert!(page.waiting());
        send.send(Ok(package(manifest(r#""source""#, "")))).unwrap();
        page.poll(None, English);
        assert!(!page.waiting());
        let Some(Review {
            stage: Stage::Ready(reviewed),
            ..
        }) = &page.review
        else {
            panic!("the package is ready to install");
        };
        assert_eq!(reviewed.installed, Installed::Version("0.2.0".into()));

        // A package that fails inspection is refused with its reason.
        let (send, reply) = mpsc::channel();
        let mut page = page_with(Stage::Reading(reply));
        send.send(Err("Cannot read addon.json".into())).unwrap();
        page.poll(None, English);
        assert!(
            matches!(&page.review, Some(Review { stage: Stage::Refused(reason), .. }) if reason == "Cannot read addon.json")
        );

        // A failed install keeps the reason and what was installed before.
        let (send, reply) = mpsc::channel();
        let reviewed = Box::new(Reviewed {
            info: package(manifest("", "")),
            installed: Installed::Version("0.1.0".into()),
        });
        let mut page = page_with(Stage::Installing(reviewed, reply));
        assert!(page.waiting());
        assert!(
            page.cancel_on_escape(),
            "Escape is taken, not passed to the window"
        );
        assert!(
            page.review.is_some(),
            "an install in progress cannot be cancelled"
        );
        send.send(Err("disk full".into())).unwrap();
        page.poll(None, English);
        assert!(
            matches!(&page.review, Some(Review { stage: Stage::Failed(reviewed, reason), .. })
            if reason == "disk full" && reviewed.installed == Installed::Version("0.1.0".into()))
        );
        assert!(page.cancel_on_escape());
        assert!(page.review.is_none());

        // A finished install closes the screen and says so.
        let (send, reply) = mpsc::channel();
        let reviewed = Box::new(Reviewed {
            info: package(manifest("", "")),
            installed: Installed::No,
        });
        let mut page = page_with(Stage::Installing(reviewed, reply));
        send.send(Ok(package(manifest("", "")))).unwrap();
        page.poll(None, English);
        assert!(page.review.is_none());
        assert_eq!(page.notice.as_deref(), Some("Installed Tone 0.2.0."));

        // A manager that went away is a reason too, not a wait that never ends.
        let (send, reply) = mpsc::channel::<Result<PackageInfo, String>>();
        drop(send);
        let mut page = page_with(Stage::Reading(reply));
        page.poll(None, English);
        assert!(!page.waiting());
    }

    #[test]
    fn a_refused_change_is_shown_and_the_control_returns_to_the_host_value() {
        let mut page = AddonsPage::default();
        let (send, reply) = mpsc::channel();
        let slot = ("com.example.tone".to_owned(), "frequency".to_owned());
        page.drafts
            .insert(slot.clone(), SettingValue::Number(500.0));
        page.requests.push(Request {
            id: slot.0.clone(),
            purpose: Purpose::Change {
                key: slot.1.clone(),
                label: "Frequency (Hz)".into(),
            },
            reply,
        });
        send.send(Err("out of range".into())).unwrap();
        page.poll(None, English);
        assert!(!page.drafts.contains_key(&slot));
        assert_eq!(
            page.problems.get(&slot).map(String::as_str),
            Some("Could not change Frequency (Hz): out of range")
        );
    }

    /// Runs one headless frame of the page, without a manager: what the user asked for stays
    /// in `intents`.
    fn frame(page: &mut AddonsPage, language: Language) -> Option<PageRequest> {
        let mut request = None;
        egui::Context::default()
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    request = page.draw(ui, language, Color32::from_rgb(90, 160, 220));
                },
            )
            .drop_without_applying_deltas();
        request
    }

    #[test]
    fn without_a_manager_the_page_says_so() {
        let mut page = AddonsPage::default();
        egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                assert_eq!(page.show(ui, None, English, Color32::WHITE), None);
            })
            .drop_without_applying_deltas();
        page.review_package(PathBuf::from(r"C:\a.btaddon"), 0, None, English);
        assert!(page.review.is_none());
        assert_eq!(page.notice.as_deref(), Some(English.addons_unavailable()));
    }

    #[test]
    fn every_screen_draws_in_both_languages() {
        let mut page = AddonsPage::default();
        for language in [English, TraditionalChinese] {
            assert_eq!(frame(&mut page, language), None);
        }
        let mut running = addon(
            "com.example.tone",
            Some(manifest(r#""source","settings","actions""#, "")),
            AddonState::Running,
        );
        running.message = Some(AddonNotice {
            level: MessageLevel::Warning,
            text: "Quiet".into(),
        });
        running.settings = vec![AddonSetting {
            declaration: number(20.0, 20000.0, 1.0),
            value: Value::from(440),
        }];
        running.actions = vec![Action {
            id: "restart".into(),
            label: "Restart".into(),
        }];
        page.snapshot = Arc::new(AddonSnapshot {
            revision: 1,
            addons: vec![
                running,
                addon(
                    "com.example.broken",
                    None,
                    AddonState::BrokenPackage("bad".into()),
                ),
            ],
            error: Some("listener failed".into()),
        });
        page.ask_to_remove("com.example.tone").unwrap();
        assert!(page.ask_to_remove("com.example.none").is_err());
        for language in [English, TraditionalChinese] {
            assert_eq!(frame(&mut page, language), None);
        }
        assert!(page.intents.is_empty(), "drawing alone asks for nothing");
        assert!(page.cancel_on_escape());
        assert!(page.removal.is_none());
        for stage in [
            Stage::Refused("not a ZIP file".into()),
            Stage::Ready(Box::new(Reviewed {
                info: package(manifest(r#""source""#, "")),
                installed: Installed::Broken,
            })),
            Stage::Failed(
                Box::new(Reviewed {
                    info: package(manifest("", "")),
                    installed: Installed::No,
                }),
                "disk full".into(),
            ),
        ] {
            let mut page = page_with(stage);
            for language in [English, TraditionalChinese] {
                assert_eq!(frame(&mut page, language), None);
            }
            assert!(page.review.is_some());
            assert!(page.intents.is_empty());
        }
    }
}

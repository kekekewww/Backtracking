mod dialog;
mod http;
mod install;

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use eframe::egui;
use semver::Version;

use crate::update_protocol::{
    ARTIFACT, MANIFEST, MAX_MANIFEST, Manifest, Mode, Policy, Release, SIGNATURE, hex_array,
    stable_version,
};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const POLICY: &str = "policy.json";
const RELEASES: &str = "https://github.com/kekekewww/Backtracking/releases";
const CHECK_TIMEOUT: Duration = Duration::from_secs(6);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(28);
const MAX_NOTES: usize = 8_000;

struct Available {
    version: Version,
    notes: String,
    manifest: Vec<u8>,
    signature: Vec<u8>,
    verified: Manifest,
}

struct Download {
    update: Available,
    bytes: Vec<u8>,
}

fn key() -> Option<VerifyingKey> {
    if !cfg!(target_arch = "x86_64") {
        return None;
    }
    let key = VerifyingKey::from_bytes(&hex_array::<32>(option_env!(
        "BACKTRACKING_UPDATE_PUBLIC_KEY"
    )?)?)
    .ok()?;
    (!key.is_weak()).then_some(key)
}

fn get_asset(
    version: &Version,
    name: &str,
    limit: usize,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let path = format!("/kekekewww/Backtracking/releases/download/v{version}/{name}");
    http::get("github.com", &path, limit, deadline)
}

fn check() -> Result<Option<Available>, String> {
    let key = key().ok_or("This build does not have a trusted update signing key.")?;
    let deadline = Instant::now() + CHECK_TIMEOUT;
    let bytes = http::get(
        "api.github.com",
        "/repos/kekekewww/Backtracking/releases/latest",
        256 * 1024,
        deadline,
    )?;
    let release: Release = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Could not read the GitHub Release: {error}"))?;
    let current =
        stable_version(CURRENT).ok_or("The installed Backtracking version is invalid.")?;
    let Some(version) = release.update_version(&current) else {
        return Ok(None);
    };
    if !release.has_update_assets() {
        return Err(format!(
            "GitHub Release v{version} has no signed Windows update package."
        ));
    }
    let manifest = get_asset(&version, MANIFEST, MAX_MANIFEST, deadline)?;
    let signature = get_asset(&version, SIGNATURE, 64, deadline)?;
    let verified = Manifest::verify(&key, &manifest, &signature, &version)?;
    let notes = release
        .body
        .filter(|body| !body.trim().is_empty())
        .map(|body| body.chars().take(MAX_NOTES).collect())
        .unwrap_or_else(|| "這個版本沒有附上更新內容。".into());
    Ok(Some(Available {
        version,
        notes,
        manifest,
        signature,
        verified,
    }))
}

fn download(update: Available) -> Result<Download, String> {
    let bytes = get_asset(
        &update.version,
        ARTIFACT,
        update.verified.size,
        Instant::now() + DOWNLOAD_TIMEOUT,
    )?;
    update.verified.verify_artifact(&bytes)?;
    Ok(Download { update, bytes })
}

fn timed<T: Send + 'static>(
    limit: Duration,
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (send, receive) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("update-request".into())
        .spawn(move || {
            let _ = send.send(operation());
        })
        .map_err(|error| error.to_string())?;
    receive
        .recv_timeout(limit)
        .map_err(|_| "The update request timed out.")?
}

pub(crate) fn run_helper() -> Result<bool, String> {
    install::helper_entry()
}

pub(crate) fn other_launch_during_update() -> bool {
    install::other_launch_during_update()
}

/// True means the current executable should exit for the staged update helper.
pub(crate) fn install_before_start() -> bool {
    if key().is_none() || install::suppress_check() {
        return false;
    }
    install::clean_old_transactions();
    let mut policy = install::state::<Policy>(POLICY).unwrap_or_default();
    if policy.mode == Mode::Manual {
        return false;
    }
    let Ok(Some(update)) = timed(CHECK_TIMEOUT, check) else {
        return false;
    };
    if policy.mode == Mode::Ask {
        if !policy.allows_prompt(&update.version) {
            return false;
        }
        match dialog::ask(&update) {
            dialog::Consent::Enable => {
                policy.mode = Mode::Automatic;
                policy.skipped = None;
            }
            dialog::Consent::Defer => policy.skipped = Some(update.version.to_string()),
            dialog::Consent::Never => {
                policy.mode = Mode::Manual;
                policy.skipped = None;
            }
        }
        if let Err(error) = install::save_state(POLICY, &policy) {
            dialog::recovered(&format!("無法儲存更新偏好設定。\n\n{error}"));
            return false;
        }
        if policy.mode != Mode::Automatic {
            return false;
        }
    }

    // The time-limited worker only downloads. A late network response cannot schedule an install.
    match timed(DOWNLOAD_TIMEOUT, move || download(update)).and_then(install::schedule) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("Backtracking update not installed: {error}");
            false
        }
    }
}

pub(crate) fn app_ready() {
    install::app_ready();
}

enum Work {
    Checked(Result<Option<Available>, String>),
    Installed(Result<(), String>),
}

pub(crate) struct UpdateOptions {
    policy: Policy,
    last_update: Option<install::LastUpdate>,
    available: Option<Available>,
    work: Option<Receiver<Work>>,
    confirm: bool,
    message: String,
}

impl Default for UpdateOptions {
    fn default() -> Self {
        Self {
            policy: install::state(POLICY).unwrap_or_default(),
            last_update: install::last_update(),
            available: None,
            work: None,
            confirm: false,
            message: String::new(),
        }
    }
}

impl UpdateOptions {
    /// True requests the application's normal full-quit path after scheduling the helper.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(work) = self.work.as_ref() else {
            return false;
        };
        let result = match work.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.work = None;
                self.message = "The update worker did not complete.".into();
                return false;
            }
        };
        self.work = None;
        match result {
            Work::Checked(Ok(update)) => {
                self.message = if update.is_some() {
                    "A signed update is available.".into()
                } else {
                    "There is no newer stable update.".into()
                };
                self.available = update;
                self.confirm = false;
            }
            Work::Checked(Err(error)) | Work::Installed(Err(error)) => self.message = error,
            Work::Installed(Ok(())) => return true,
        }
        false
    }

    fn start_work(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        operation: impl FnOnce() -> Work + Send + 'static,
    ) {
        let (send, receive) = mpsc::sync_channel(1);
        let ctx = ctx.clone();
        match thread::Builder::new()
            .name("update-options".into())
            .spawn(move || {
                let _ = send.send(operation());
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }) {
            Ok(_) => {
                self.work = Some(receive);
                self.message = label.into();
            }
            Err(error) => self.message = format!("Could not start the update worker: {error}"),
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui) -> bool {
        if self.poll() {
            return true;
        }
        ui.label(format!("Installed version  v{CURRENT}"));
        if key().is_none() {
            ui.label("Automatic updates are unavailable for this build. Install a signed release to enable them.");
            ui.hyperlink_to("Open GitHub Releases", RELEASES);
            return false;
        }

        let mut mode = self.policy.mode;
        egui::ComboBox::from_id_salt("startup-update-mode")
            .selected_text(match mode {
                Mode::Ask => "Ask before installing",
                Mode::Automatic => "Automatically update at startup",
                Mode::Manual => "Check manually",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut mode, Mode::Ask, "Ask before installing");
                ui.selectable_value(
                    &mut mode,
                    Mode::Automatic,
                    "Automatically update at startup",
                );
                ui.selectable_value(&mut mode, Mode::Manual, "Check manually");
            });
        if mode != self.policy.mode {
            let policy = Policy {
                mode,
                skipped: None,
            };
            match install::save_state(POLICY, &policy) {
                Ok(()) => self.policy = policy,
                Err(error) => self.message = format!("Could not save update preferences: {error}"),
            }
        }

        if ui
            .add_enabled(self.work.is_none(), egui::Button::new("Check for updates"))
            .clicked()
        {
            self.start_work(ui.ctx(), "Checking GitHub Releases…", || {
                Work::Checked(timed(CHECK_TIMEOUT, check))
            });
        }

        let mut start = false;
        if let Some(update) = &self.available {
            ui.label(format!("Available version  v{}", update.version));
            egui::CollapsingHeader::new("Release notes")
                .default_open(true)
                .show(ui, |ui| {
                    ui.label(&update.notes);
                });
            if self.confirm {
                ui.label("Backtracking will restart and begin a new capture history.");
                ui.horizontal(|ui| {
                    start = ui
                        .add_enabled(
                            self.work.is_none(),
                            egui::Button::new("Install and restart"),
                        )
                        .clicked();
                    if ui.button("Cancel").clicked() {
                        self.confirm = false;
                    }
                });
            } else if ui
                .add_enabled(self.work.is_none(), egui::Button::new("Update now…"))
                .clicked()
            {
                self.confirm = true;
            }
        }
        if start && let Some(update) = self.available.take() {
            self.confirm = false;
            self.start_work(
                ui.ctx(),
                "Downloading and verifying the update…",
                move || {
                    Work::Installed(
                        timed(DOWNLOAD_TIMEOUT, move || download(update))
                            .and_then(install::schedule),
                    )
                },
            );
        }
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
        if let Some(last) = &self.last_update {
            egui::CollapsingHeader::new(format!("Last update  v{}", last.version)).show(ui, |ui| {
                ui.label(&last.notes);
            });
        }
        ui.hyperlink_to("Open GitHub Releases", RELEASES);
        false
    }
}

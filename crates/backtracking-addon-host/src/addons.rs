//! Ported from desktop/windows/src/addons.rs at e9a4efc0d2918bccadeecf2c7e586b84ddd5a4f4.
//! Background ownership of installed add-ons, their processes and their protocol state.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_addon::host::{Connection, Host, HostEvent, PositionFilter};
use backtracking_addon::manifest::{Capability, Manifest, validate_id};
use backtracking_addon::package::{self, PackageInfo};
use backtracking_addon::protocol::{
    Action, AddonMessage, Appearance, ChannelPosition, ControlCommand, ControlError, ControlReply,
    ControlResult, HostMessage, MAX_CONTROL_IN_FLIGHT, MessageLevel, Setting, SettingKind, Source,
    Transport, TransportCommand, control_codes,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

mod bridge;

mod process;
mod startup;
use process::{AddonProcess, random_token};

fn private_dir(path: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(std::io::Error::other("Expected an add-on directory"));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const QUIT_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const STATE_FILE: &str = "addon-host-v1.json";
const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_POSITION_BYTES: usize = 1024 * 1024;

pub(crate) type AddonReply<T> = mpsc::Receiver<Result<T, String>>;
pub(crate) type ChangeCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug)]
pub(crate) struct InspectedPackage {
    pub(crate) info: PackageInfo,
    pub(crate) identity: [u8; 32],
}
type Reply<T> = mpsc::Sender<Result<T, String>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AddonState {
    Stopped,
    Starting,
    Running,
    Stopping,
    NotResponding,
    Failed(String),
    BrokenPackage(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddonNotice {
    pub(crate) level: MessageLevel,
    pub(crate) text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddonSetting {
    pub(crate) declaration: Setting,
    pub(crate) value: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddonInfo {
    pub(crate) id: String,
    pub(crate) manifest: Option<Manifest>,
    pub(crate) enabled: bool,
    pub(crate) state: AddonState,
    pub(crate) pid: Option<u32>,
    pub(crate) message: Option<AddonNotice>,
    pub(crate) sources: Vec<Source>,
    pub(crate) settings: Vec<AddonSetting>,
    pub(crate) actions: Vec<Action>,
    /// The user lets this add-on control Backtracking (capability `control`). Saved per id.
    #[serde(default)]
    pub(crate) control_allowed: bool,
    /// The add-on asked to control Backtracking while that was off, since it last started.
    #[serde(default)]
    pub(crate) control_asked: bool,
}

/// One control request an add-on sent, for the main window to carry out.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ControlCall {
    pub(crate) addon_id: String,
    pub(crate) addon_name: String,
    /// Which run of the add-on asked; a reply to an earlier run is dropped.
    pub(crate) run: u64,
    pub(crate) request_id: String,
    pub(crate) command: ControlCommand,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddonSnapshot {
    pub(crate) revision: u64,
    pub(crate) addons: Vec<AddonInfo>,
    /// A manager-level problem, such as a corrupt state file or a listener failure.
    pub(crate) error: Option<String>,
}

impl AddonSnapshot {
    pub(crate) fn addon(&self, id: &str) -> Option<&AddonInfo> {
        self.addons.iter().find(|addon| addon.id == id)
    }

    pub(crate) fn source(&self, addon_id: &str, source_id: &str) -> Option<&Source> {
        let addon = self.addon(addon_id)?;
        (addon.state == AddonState::Running)
            .then(|| addon.sources.iter().find(|source| source.id == source_id))
            .flatten()
    }
}

/// One accepted full state, stamped on the Sender clock. Readers have separate cursors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddonPositions {
    pub(crate) sequence: u64,
    pub(crate) addon_id: String,
    pub(crate) source_id: String,
    pub(crate) time: u64,
    pub(crate) channels: Vec<ChannelPosition>,
}

impl AddonPositions {
    fn memory_bytes(&self) -> usize {
        size_of::<Self>()
            + self.addon_id.capacity()
            + self.source_id.capacity()
            + self.channels.capacity() * size_of::<ChannelPosition>()
            + size_of::<Arc<Self>>()
            + 2 * size_of::<usize>()
    }
}

#[derive(Default)]
struct PositionJournal {
    sequence: u64,
    bytes: usize,
    messages: VecDeque<Arc<AddonPositions>>,
}

impl PositionJournal {
    fn push(&mut self, mut message: AddonPositions) {
        self.sequence = self.sequence.saturating_add(1);
        message.sequence = self.sequence;
        self.bytes += message.memory_bytes();
        self.messages.push_back(Arc::new(message));
        while self.bytes > MAX_POSITION_BYTES {
            let Some(oldest) = self.messages.pop_front() else {
                break;
            };
            self.bytes -= oldest.memory_bytes();
        }
    }

    fn since(&self, cursor: &mut u64) -> Vec<Arc<AddonPositions>> {
        if *cursor > self.sequence {
            *cursor = 0;
        }
        let result = self
            .messages
            .iter()
            .filter(|message| message.sequence > *cursor)
            .cloned()
            .collect();
        *cursor = self.sequence;
        result
    }
}

/// The current appearance offers, keyed by add-on id.
pub(crate) type AddonAppearances = BTreeMap<String, Appearance>;

#[derive(Default)]
struct Published {
    snapshot: Arc<AddonSnapshot>,
    appearances: Arc<AddonAppearances>,
}

#[derive(Default)]
struct Shared {
    published: Mutex<Published>,
    callbacks: Mutex<Vec<ChangeCallback>>,
    appearance_callbacks: Mutex<Vec<ChangeCallback>>,
    positions: Mutex<PositionJournal>,
    /// Control requests accepted and not yet taken by the main window.
    control_calls: Mutex<VecDeque<ControlCall>>,
    control_callbacks: Mutex<Vec<ChangeCallback>>,
}

impl Shared {
    fn discard_control(&self, id: &str, request: Option<&str>) {
        self.control_calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|call| call.addon_id != id || request.is_some_and(|r| r != call.request_id));
    }

    fn lock_published(&self) -> std::sync::MutexGuard<'_, Published> {
        self.published.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn publish<'a>(
        &self,
        addons: Vec<AddonInfo>,
        error: Option<&str>,
        appearances: impl Iterator<Item = (&'a String, &'a Appearance)> + Clone,
    ) {
        let mut current = self.lock_published();
        let addons_changed =
            current.snapshot.addons != addons || current.snapshot.error.as_deref() != error;
        let mut previous = current.appearances.iter();
        let appearances_changed = !appearances.clone().all(|(id, look)| {
            previous
                .next()
                .is_some_and(|(old_id, old_look)| id == old_id && look == old_look)
        }) || previous.next().is_some();
        if addons_changed {
            current.snapshot = Arc::new(AddonSnapshot {
                revision: current.snapshot.revision.wrapping_add(1),
                addons,
                error: error.map(str::to_owned),
            });
        }
        if appearances_changed {
            current.appearances = Arc::new(
                appearances
                    .map(|(id, look)| (id.clone(), look.clone()))
                    .collect(),
            );
        }
        drop(current);

        if addons_changed {
            notify(&self.callbacks);
        }
        if appearances_changed {
            notify(&self.appearance_callbacks);
        }
    }
}

fn notify(callbacks: &Mutex<Vec<ChangeCallback>>) {
    let callbacks = callbacks.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for callback in callbacks {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback()));
    }
}

/// A handle may be retained by capture and export workers after a UI window closes.
#[derive(Clone)]
pub(crate) struct AddonHandle {
    commands: mpsc::Sender<Command>,
    shared: Arc<Shared>,
}

impl AddonHandle {
    pub(crate) fn published(&self) -> (Arc<AddonSnapshot>, Arc<AddonAppearances>) {
        let p = self.shared.lock_published();
        (p.snapshot.clone(), p.appearances.clone())
    }
    /// Read accepted positions without consuming another lane or window's updates.
    /// A reader which falls behind the bounded journal resumes at its oldest state.
    pub(crate) fn positions_since(&self, cursor: &mut u64) -> Vec<Arc<AddonPositions>> {
        self.shared
            .positions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .since(cursor)
    }

    /// Use on the UI thread; keep the previous snapshot when publication is in progress.
    #[cfg(test)]
    pub(crate) fn try_snapshot(&self) -> Option<Arc<AddonSnapshot>> {
        let published = self.shared.published.try_lock().ok()?;
        Some(published.snapshot.clone())
    }

    /// For workers and tests. The lock only protects in-memory snapshots, never I/O.
    pub(crate) fn snapshot(&self) -> Arc<AddonSnapshot> {
        self.shared.lock_published().snapshot.clone()
    }

    /// Read current appearance offers without waiting for publication. On `None`,
    /// keep the previous snapshot. Its Arc is reused while the offers are unchanged.
    #[cfg(test)]
    pub(crate) fn try_appearances(&self) -> Option<Arc<AddonAppearances>> {
        let published = self.shared.published.try_lock().ok()?;
        Some(published.appearances.clone())
    }

    /// Subscribe UI repaints to appearance changes. Callbacks must return promptly.
    pub(crate) fn subscribe_appearances(&self, callback: ChangeCallback) {
        self.shared
            .appearance_callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(callback);
    }

    /// Subscribe capture wakes and UI repaints. Callbacks must return promptly.
    pub(crate) fn subscribe(&self, callback: ChangeCallback) {
        self.shared
            .callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(callback);
    }

    /// Subscribe a repaint of the main window to new control requests.
    pub(crate) fn subscribe_control(&self, callback: ChangeCallback) {
        self.shared
            .control_callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(callback);
    }

    /// Take the control requests waiting to be carried out, oldest first. Answer each one
    /// with [`AddonHandle::control_reply`]. Empty in a secondary capture window.
    pub(crate) fn take_control_calls(&self) -> Vec<ControlCall> {
        self.shared
            .control_calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }

    /// Answer a control request. A reply after the request timed out is dropped.
    #[cfg(test)]
    pub(crate) fn control_reply(
        &self,
        call: &ControlCall,
        outcome: Result<ControlResult, ControlError>,
    ) {
        self.answer_control(
            call.addon_id.clone(),
            call.run,
            call.request_id.clone(),
            outcome,
        )
    }
    pub(crate) fn answer_control(
        &self,
        addon_id: String,
        run: u64,
        request_id: String,
        outcome: Result<ControlResult, ControlError>,
    ) {
        let reply = match outcome {
            Ok(result) => ControlReply::success(request_id, &result),
            Err(error) => ControlReply::failure(Some(request_id), error),
        };
        let _ = self.commands.send(Command::ControlReply {
            addon_id,
            run,
            reply,
        });
    }

    /// Switch "Let this add-on control Backtracking" for one add-on, and save it.
    pub(crate) fn set_control_allowed(&self, id: &str, allowed: bool) -> AddonReply<()> {
        self.request(|reply| Command::ControlAllowed(id.into(), allowed, reply))
    }

    pub(crate) fn inspect(&self, path: PathBuf) -> AddonReply<InspectedPackage> {
        self.request(|reply| Command::Inspect(path, reply))
    }

    pub(crate) fn install(&self, path: PathBuf, identity: [u8; 32]) -> AddonReply<PackageInfo> {
        self.request(|reply| Command::Install(path, identity, reply))
    }

    pub(crate) fn remove(&self, id: &str, delete_data: bool) -> AddonReply<()> {
        self.request(|reply| Command::Remove(id.into(), delete_data, reply))
    }

    pub(crate) fn set_enabled(&self, id: &str, enabled: bool) -> AddonReply<()> {
        self.request(|reply| Command::Enabled(id.into(), enabled, reply))
    }

    /// Start now, without changing the saved enabled preference.
    pub(crate) fn start(&self, id: &str) -> AddonReply<()> {
        self.request(|reply| Command::Start(id.into(), reply))
    }

    /// Stop now, without changing the saved enabled preference.
    pub(crate) fn stop(&self, id: &str) -> AddonReply<()> {
        self.request(|reply| Command::Stop(id.into(), reply))
    }

    pub(crate) fn set_setting(&self, id: &str, key: &str, value: Value) -> AddonReply<()> {
        self.request(|reply| {
            Command::Send(
                id.into(),
                HostMessage::SetSetting {
                    key: key.into(),
                    value,
                },
                reply,
            )
        })
    }

    pub(crate) fn run_action(&self, id: &str, action: &str) -> AddonReply<()> {
        self.request(|reply| {
            Command::Send(
                id.into(),
                HostMessage::RunAction { id: action.into() },
                reply,
            )
        })
    }

    pub(crate) fn open_file(&self, id: &str, path: PathBuf) -> AddonReply<()> {
        let path = path.to_str().map(str::to_owned);
        self.request(|reply| match path {
            Some(path) => Command::Send(id.into(), HostMessage::OpenFile { path }, reply),
            None => Command::Error("The file path is not valid Unicode".into(), reply),
        })
    }

    pub(crate) fn select_format(&self, id: &str, source: &str, format: &str) -> AddonReply<()> {
        self.request(|reply| {
            Command::Send(
                id.into(),
                HostMessage::SelectFormat {
                    source: source.into(),
                    format: format.into(),
                },
                reply,
            )
        })
    }

    pub(crate) fn transport(
        &self,
        id: &str,
        source: &str,
        command: TransportCommand,
        position: Option<f64>,
    ) -> AddonReply<()> {
        self.request(|reply| {
            Command::Send(
                id.into(),
                HostMessage::Transport {
                    source: source.into(),
                    command,
                    position,
                },
                reply,
            )
        })
    }

    pub(crate) fn clip_exported(
        &self,
        files: Vec<PathBuf>,
        sample_rate: u32,
        frames: u64,
    ) -> AddonReply<()> {
        self.request(|reply| {
            let paths = files
                .iter()
                .map(|path| path.to_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>();
            match paths {
                Some(files) => Command::Clips(
                    HostMessage::ClipExported {
                        files,
                        sample_rate,
                        frames,
                    },
                    reply,
                ),
                None => Command::Error("An exported path is not valid Unicode".into(), reply),
            }
        })
    }

    fn request<T>(&self, make: impl FnOnce(Reply<T>) -> Command) -> AddonReply<T> {
        let (send, receive) = mpsc::channel();
        if self.commands.send(make(send.clone())).is_err() {
            let _ = send.send(Err("The add-on manager has stopped".into()));
        }
        receive
    }
}

/// Drop requests quit for every child and bounds the wait for the worker.
pub(crate) struct AddonManager {
    handle: AddonHandle,
    worker: Option<JoinHandle<()>>,
    bridge: Option<bridge::Bridge>,
}

impl AddonManager {
    pub(crate) fn new(data_root: PathBuf, version: &str) -> Result<Self, String> {
        if !data_root.is_absolute() {
            return Err("The add-on data root must be absolute".into());
        }
        private_dir(&data_root).map_err(|e| e.to_string())?;
        let host = Host::bind(version).map_err(|e| e.to_string())?;
        let (commands, receive) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let worker_state = Worker::new(data_root.clone(), host, shared.clone())?;
        let worker = thread::Builder::new()
            .name("add-on host".into())
            .spawn(move || worker_state.run(receive))
            .map_err(|e| e.to_string())?;
        let mut manager = Self {
            handle: AddonHandle { commands, shared },
            worker: Some(worker),
            bridge: None,
        };
        manager.bridge = Some(bridge::Bridge::start(&data_root, manager.handle())?);
        Ok(manager)
    }

    /// Connect a secondary capture window to the primary host; never start another copy.
    pub(crate) fn connect_existing(data_root: PathBuf) -> Result<Self, String> {
        bridge::connect(data_root)
    }

    pub(crate) fn handle(&self) -> AddonHandle {
        self.handle.clone()
    }
}

impl Drop for AddonManager {
    fn drop(&mut self) {
        self.bridge.take();
        let _ = self.handle.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(POLL_INTERVAL);
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
            // A stuck worker keeps its resources until it returns, but cannot
            // prevent the UI process from exiting.
        }
    }
}

enum Command {
    Inspect(PathBuf, Reply<InspectedPackage>),
    Install(PathBuf, [u8; 32], Reply<PackageInfo>),
    Remove(String, bool, Reply<()>),
    Enabled(String, bool, Reply<()>),
    Start(String, Reply<()>),
    Stop(String, Reply<()>),
    Send(String, HostMessage, Reply<()>),
    Clips(HostMessage, Reply<()>),
    Error(String, Reply<()>),
    ControlAllowed(String, bool, Reply<()>),
    ControlReply {
        addon_id: String,
        run: u64,
        reply: ControlReply,
    },
    Shutdown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct SavedAddon {
    enabled: bool,
    settings: BTreeMap<String, Value>,
    formats: BTreeMap<String, String>,
    /// "Let this add-on control Backtracking". Off unless the user switched it on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    control: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct SavedState {
    addons: BTreeMap<String, SavedAddon>,
}

#[derive(Default)]
struct RestartPolicy {
    exits: VecDeque<Instant>,
}

impl RestartPolicy {
    fn exited(&mut self, now: Instant) -> bool {
        while self
            .exits
            .front()
            .is_some_and(|time| now.saturating_duration_since(*time) > RESTART_WINDOW)
        {
            self.exits.pop_front();
        }
        self.exits.push_back(now);
        self.exits.len() < 3
    }
}

fn hello_expired(started: Instant, now: Instant, connected: bool) -> bool {
    !connected && now.saturating_duration_since(started) >= HELLO_TIMEOUT
}

struct Runtime {
    process: AddonProcess,
    started: Instant,
    connection: Option<Connection>,
    described: bool,
    disconnected_at: Option<Instant>,
    stop: Option<(Instant, AddonState)>,
    restart_after_stop: bool,
    /// Numbers this run, so a reply meant for an earlier run is dropped.
    run: u64,
    /// Control requests waiting for the main window's reply, with when they time out.
    control: BTreeMap<String, Instant>,
}

struct Entry {
    info: AddonInfo,
    runtime: Option<Runtime>,
    restarts: RestartPolicy,
    position_filter: PositionFilter,
    appearance: Option<Appearance>,
}

impl Entry {
    fn from_manifest(id: String, manifest: Result<Manifest, String>, enabled: bool) -> Self {
        let (manifest, state) = match manifest {
            Ok(manifest) => (Some(manifest), AddonState::Stopped),
            Err(reason) => (None, AddonState::BrokenPackage(reason)),
        };
        Self {
            info: AddonInfo {
                id,
                manifest,
                enabled,
                state,
                pid: None,
                message: None,
                sources: Vec::new(),
                settings: Vec::new(),
                actions: Vec::new(),
                control_allowed: false,
                control_asked: false,
            },
            runtime: None,
            restarts: RestartPolicy::default(),
            position_filter: PositionFilter::default(),
            appearance: None,
        }
    }

    fn clear_description(&mut self) {
        self.appearance = None;
        self.position_filter.clear();
        self.info.sources.clear();
        self.info.settings.clear();
        self.info.actions.clear();
    }
}

struct PendingInspection {
    result: AddonReply<PreparedPackage>,
    reply: Reply<PackageInfo>,
}

struct PreparedPackage {
    path: PathBuf,
    info: PackageInfo,
    identity: [u8; 32],
}

impl Drop for PreparedPackage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

enum Operation {
    StopForInstall {
        id: String,
        prepared: PreparedPackage,
        restart: bool,
        reply: Reply<PackageInfo>,
    },
    Install {
        id: String,
        restart: bool,
        result: AddonReply<PackageInfo>,
        reply: Reply<PackageInfo>,
    },
    StopForRemove {
        id: String,
        delete_data: bool,
        reply: Reply<()>,
    },
    Remove {
        id: String,
        delete_data: bool,
        result: AddonReply<()>,
        reply: Reply<()>,
    },
}

impl Operation {
    fn id(&self) -> &str {
        match self {
            Self::StopForInstall { id, .. }
            | Self::Install { id, .. }
            | Self::StopForRemove { id, .. }
            | Self::Remove { id, .. } => id,
        }
    }
}

struct Worker {
    root: PathBuf,
    primary_lease: startup::PrimaryLease,
    host: Host,
    shared: Arc<Shared>,
    entries: BTreeMap<String, Entry>,
    saved: SavedState,
    error: Option<String>,
    inspections: Vec<PendingInspection>,
    operations: Vec<Operation>,
    shutting_down: bool,
    runs: u64,
}

impl Worker {
    fn new(root: PathBuf, host: Host, shared: Arc<Shared>) -> Result<Self, String> {
        let primary_lease = startup::PrimaryLease::acquire(&root)?;
        bridge::ensure_no_primary(&root)?;
        primary_lease.cleanup()?;
        let (saved, error) = match read_saved(&root) {
            Ok(saved) => (saved, None),
            Err(reason) => (SavedState::default(), Some(reason)),
        };
        let mut entries = BTreeMap::new();
        for installed in package::list_installed(root.join("addons")).map_err(|e| e.to_string())? {
            let id = installed
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let enabled = saved.addons.get(&id).is_some_and(|value| value.enabled);
            let mut entry = Entry::from_manifest(
                id.clone(),
                installed.manifest.map_err(|e| e.to_string()),
                enabled,
            );
            entry.info.control_allowed = saved.addons.get(&id).is_some_and(|value| value.control);
            entries.insert(id, entry);
        }
        Ok(Self {
            root,
            primary_lease,
            host,
            shared,
            entries,
            saved,
            error,
            inspections: Vec::new(),
            operations: Vec::new(),
            shutting_down: false,
            runs: 0,
        })
    }

    fn run(mut self, commands: mpsc::Receiver<Command>) {
        let enabled: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.info.enabled)
            .map(|(id, _)| id.clone())
            .collect();
        for id in enabled {
            let _ = self.start(&id);
        }
        self.publish();
        loop {
            match commands.recv_timeout(POLL_INTERVAL) {
                Ok(command) => self.command(command),
                Err(mpsc::RecvTimeoutError::Disconnected) => self.shutdown(),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            for _ in 0..64 {
                let Ok(command) = commands.try_recv() else {
                    break;
                };
                self.command(command);
            }
            self.poll();
            self.advance_operations();
            self.publish();
            if self.shutting_down
                && self.entries.values().all(|entry| entry.runtime.is_none())
                && self.operations.is_empty()
                && self.inspections.is_empty()
            {
                break;
            }
        }
    }

    fn publish(&self) {
        let addons = self
            .entries
            .values()
            .map(|entry| entry.info.clone())
            .collect();
        let appearances = self
            .entries
            .iter()
            .filter_map(|(id, entry)| entry.appearance.as_ref().map(|look| (id, look)));
        self.shared
            .publish(addons, self.error.as_deref(), appearances);
    }

    fn command(&mut self, command: Command) {
        if self.shutting_down && !matches!(command, Command::Shutdown) {
            reject_command(command, "The add-on manager is shutting down");
            return;
        }
        match command {
            Command::Inspect(path, reply) => {
                let root = self.root.clone();
                spawn_task(
                    "inspect add-on",
                    move || {
                        let prepared = prepare_package(&root, &path)?;
                        Ok(InspectedPackage {
                            info: PackageInfo {
                                manifest: prepared.info.manifest.clone(),
                                unpacked_size: prepared.info.unpacked_size,
                                entry_count: prepared.info.entry_count,
                                file_count: prepared.info.file_count,
                                program: prepared.info.program.clone(),
                            },
                            identity: prepared.identity,
                        })
                    },
                    reply,
                    self.primary_lease.clone(),
                );
            }
            Command::Install(path, identity, reply) => {
                let root = self.root.clone();
                let (send, result) = mpsc::channel();
                spawn_task(
                    "inspect add-on",
                    move || {
                        let prepared = prepare_package(&root, &path)?;
                        if prepared.identity != identity {
                            return Err(
                                "The file changed after it was checked. Check it again.".into()
                            );
                        }
                        Ok(prepared)
                    },
                    send,
                    self.primary_lease.clone(),
                );
                self.inspections.push(PendingInspection { result, reply });
            }
            Command::Remove(id, delete_data, reply) => {
                let result = self
                    .available(&id)
                    .and_then(|()| validate_id(&id).map_err(|e| e.to_string()));
                if let Err(reason) = result {
                    let _ = reply.send(Err(reason));
                } else {
                    self.stop(&id, AddonState::Stopped);
                    self.operations.push(Operation::StopForRemove {
                        id,
                        delete_data,
                        reply,
                    });
                }
            }
            Command::Enabled(id, enabled, reply) => {
                let result = self.available(&id).and_then(|()| {
                    if enabled && self.entries[&id].info.manifest.is_none() {
                        return Err("Reinstall this broken package before enabling it".into());
                    }
                    let mut saved = self.saved.clone();
                    saved.addons.entry(id.clone()).or_default().enabled = enabled;
                    self.store(saved)?;
                    self.entries.get_mut(&id).unwrap().info.enabled = enabled;
                    if enabled {
                        self.entries.get_mut(&id).unwrap().restarts = RestartPolicy::default();
                        self.start(&id)
                    } else {
                        self.stop(&id, AddonState::Stopped);
                        Ok(())
                    }
                });
                let _ = reply.send(result);
            }
            Command::Start(id, reply) => {
                let result = self.available(&id).and_then(|()| {
                    self.entries.get_mut(&id).unwrap().restarts = RestartPolicy::default();
                    self.start(&id)
                });
                let _ = reply.send(result);
            }
            Command::Stop(id, reply) => {
                let result = self
                    .available(&id)
                    .map(|()| self.stop(&id, AddonState::Stopped));
                let _ = reply.send(result);
            }
            Command::Send(id, message, reply) => {
                let result = self.send(&id, message);
                let _ = reply.send(result);
            }
            Command::Clips(message, reply) => {
                let result = message
                    .validate()
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        let targets: Vec<_> = self
                            .entries
                            .iter()
                            .filter(|(_, entry)| {
                                entry.info.state == AddonState::Running
                                    && has_capability(&entry.info, Capability::Clips)
                            })
                            .map(|(id, _)| id.clone())
                            .collect();
                        let mut failures = Vec::new();
                        for id in targets {
                            if let Err(reason) = self.send(&id, message.clone()) {
                                failures.push(format!("{id}: {reason}"));
                            }
                        }
                        if failures.is_empty() {
                            Ok(())
                        } else {
                            Err(failures.join("; "))
                        }
                    });
                let _ = reply.send(result);
            }
            Command::Error(reason, reply) => {
                let _ = reply.send(Err(reason));
            }
            Command::ControlAllowed(id, allowed, reply) => {
                let result = self.available(&id).and_then(|()| {
                    let mut saved = self.saved.clone();
                    saved.addons.entry(id.clone()).or_default().control = allowed;
                    self.store(saved)?;
                    self.set_control_access(&id, allowed);
                    self.publish();
                    Ok(())
                });
                let _ = reply.send(result);
            }
            Command::ControlReply {
                addon_id,
                run,
                reply,
            } => {
                let Some(runtime) = self
                    .entries
                    .get_mut(&addon_id)
                    .and_then(|entry| entry.runtime.as_mut())
                    .filter(|runtime| runtime.run == run)
                else {
                    return;
                };
                let waiting = reply
                    .id
                    .as_ref()
                    .is_some_and(|request| runtime.control.remove(request).is_some());
                if waiting {
                    if let Some(connection) = &mut runtime.connection {
                        let _ = connection.send(&HostMessage::ControlReply(reply));
                    }
                }
            }
            Command::Shutdown => self.shutdown(),
        }
    }

    /// Apply the user's switch: tell a running add-on, and on revoking refuse what it asked.
    fn set_control_access(&mut self, id: &str, allowed: bool) {
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        entry.info.control_allowed = allowed;
        if allowed {
            entry.info.control_asked = false;
        }
        if !allowed {
            self.shared.discard_control(id, None)
        }
        let declared = has_capability(&entry.info, Capability::Control);
        let Some(runtime) = entry.runtime.as_mut() else {
            return;
        };
        let mut refused = Vec::new();
        if !allowed {
            refused.extend(std::mem::take(&mut runtime.control).into_keys());
        }
        if let Some(connection) = &mut runtime.connection {
            for request in refused {
                let _ = connection.send(&HostMessage::ControlReply(ControlReply::failure(
                    Some(request),
                    not_allowed(),
                )));
            }
            if declared {
                let _ = connection.send(&HostMessage::ControlAccess { allowed });
            }
        }
    }

    /// Check a control request and queue it for the main window, or answer it at once.
    fn control_request(
        &mut self,
        id: &str,
        request: Option<String>,
        command: Result<ControlCommand, ControlError>,
    ) {
        let entry = self.entries.get_mut(id).unwrap();
        let Some(runtime) = entry.runtime.as_mut() else {
            return;
        };
        let refusal = if !has_capability(&entry.info, Capability::Control) {
            Some(ControlError::new(
                control_codes::NOT_DECLARED,
                "Declare the control capability in addon.json to send control requests",
            ))
        } else if !entry.info.control_allowed {
            entry.info.control_asked = true;
            Some(not_allowed())
        } else {
            match (&request, &command) {
                (_, Err(error)) => Some(error.clone()),
                (None, _) => Some(ControlError::new(
                    control_codes::INVALID_REQUEST,
                    "A control request needs an id",
                )),
                (Some(request), Ok(_)) if runtime.control.contains_key(request) => {
                    Some(ControlError::new(
                        control_codes::DUPLICATE_ID,
                        "A request with this id is still waiting for its reply",
                    ))
                }
                (Some(_), Ok(_)) if runtime.control.len() >= MAX_CONTROL_IN_FLIGHT => {
                    Some(ControlError::new(
                        control_codes::TOO_MANY_REQUESTS,
                        format!(
                            "At most {MAX_CONTROL_IN_FLIGHT} control requests may wait for their \
                             replies at once"
                        ),
                    ))
                }
                _ => None,
            }
        };
        if let Some(error) = refusal {
            if let Some(connection) = &mut runtime.connection {
                let _ = connection.send(&HostMessage::ControlReply(ControlReply::failure(
                    request, error,
                )));
            }
            return;
        }
        let (Some(request), Ok(command)) = (request, command) else {
            return;
        };
        runtime
            .control
            .insert(request.clone(), Instant::now() + command.reply_within());
        let addon_name = entry
            .info
            .manifest
            .as_ref()
            .map_or_else(|| id.to_owned(), |manifest| manifest.name.clone());
        self.shared
            .control_calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(ControlCall {
                addon_id: id.into(),
                addon_name,
                run: runtime.run,
                request_id: request,
                command,
            });
        notify(&self.shared.control_callbacks);
    }

    /// Answer the requests the main window did not answer in time.
    fn expire_control(&mut self, now: Instant) {
        for entry in self.entries.values_mut() {
            let Some(runtime) = entry.runtime.as_mut() else {
                continue;
            };
            let late: Vec<String> = runtime
                .control
                .iter()
                .filter(|(_, deadline)| now >= **deadline)
                .map(|(request, _)| request.clone())
                .collect();
            for request in late {
                self.shared.discard_control(&entry.info.id, Some(&request));
                runtime.control.remove(&request);
                if let Some(connection) = &mut runtime.connection {
                    let _ = connection.send(&HostMessage::ControlReply(ControlReply::failure(
                        Some(request),
                        ControlError::new(
                            control_codes::TIMEOUT,
                            "Backtracking did not finish this in time; it may still happen",
                        ),
                    )));
                }
            }
        }
    }

    fn available(&self, id: &str) -> Result<(), String> {
        if !self.entries.contains_key(id) {
            return Err("This add-on is not installed".into());
        }
        if self.operations.iter().any(|operation| operation.id() == id) {
            return Err("This add-on is being installed or removed".into());
        }
        Ok(())
    }

    fn store(&mut self, saved: SavedState) -> Result<(), String> {
        write_saved(&self.root, &saved)?;
        self.saved = saved;
        Ok(())
    }

    fn start(&mut self, id: &str) -> Result<(), String> {
        let entry = self
            .entries
            .get_mut(id)
            .ok_or("This add-on is not installed")?;
        if let Some(runtime) = &mut entry.runtime {
            if runtime.stop.is_some() {
                runtime.restart_after_stop = true;
            }
            return Ok(());
        }
        entry.clear_description();
        let result = (|| {
            let manifest = entry
                .info
                .manifest
                .as_ref()
                .ok_or("Reinstall this broken package before starting it")?;
            let run = manifest.current_run().map_err(|e| e.to_string())?;
            let token = random_token()?;
            let data = self.root.join("addon-data").join(id);
            private_dir(&self.root.join("addon-data")).map_err(|e| e.to_string())?;
            private_dir(&data).map_err(|e| e.to_string())?;
            let cwd = self.root.join("addons").join(id);
            let settings = if has_capability(&entry.info, Capability::Settings) {
                self.saved
                    .addons
                    .get(id)
                    .map(|value| value.settings.clone())
                    .unwrap_or_default()
            } else {
                BTreeMap::new()
            };
            self.host
                .expect_addon(id, &token, settings)
                .map_err(|e| e.to_string())?;
            let env = vec![
                (
                    OsString::from("BACKTRACKING_ADDON_PORT"),
                    OsString::from(self.host.port().to_string()),
                ),
                (
                    OsString::from("BACKTRACKING_ADDON_TOKEN"),
                    OsString::from(token),
                ),
                (
                    OsString::from("BACKTRACKING_ADDON_DATA"),
                    data.into_os_string(),
                ),
            ];
            let started = Instant::now();
            let process = AddonProcess::launch(&cwd.join(&run.program), &run.args, &cwd, &env)?;
            self.runs += 1;
            Ok::<_, String>(Runtime {
                process,
                started,
                connection: None,
                described: false,
                disconnected_at: None,
                stop: None,
                restart_after_stop: false,
                run: self.runs,
                control: BTreeMap::new(),
            })
        })();
        match result {
            Ok(runtime) => {
                entry.info.pid = Some(runtime.process.id());
                entry.info.state = AddonState::Starting;
                entry.info.message = None;
                entry.info.control_asked = false;
                entry.runtime = Some(runtime);
                Ok(())
            }
            Err(reason) => {
                self.host.forget_addon(id);
                if entry.info.manifest.is_some() {
                    entry.info.state = AddonState::Failed(reason.clone());
                }
                Err(reason)
            }
        }
    }

    fn stop(&mut self, id: &str, final_state: AddonState) {
        self.shared.discard_control(id, None);
        self.host.forget_addon(id);
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        if let Some(runtime) = &mut entry.runtime {
            runtime.restart_after_stop = false;
            if runtime.stop.is_none() {
                if let Some(connection) = &mut runtime.connection {
                    let _ = connection.send(&HostMessage::Quit);
                }
                runtime.stop = Some((Instant::now() + QUIT_TIMEOUT, final_state));
                entry.info.state = AddonState::Stopping;
                entry.clear_description();
            } else if let Some((_, state)) = &mut runtime.stop {
                *state = final_state;
            }
        } else if entry.info.manifest.is_some() {
            entry.info.state = final_state;
            entry.clear_description();
        }
    }

    fn shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        let ids: Vec<_> = self.entries.keys().cloned().collect();
        for id in ids {
            self.stop(&id, AddonState::Stopped);
        }
    }

    fn send(&mut self, id: &str, message: HostMessage) -> Result<(), String> {
        self.available(id)?;
        let info = &self.entries[id].info;
        validate_command(info, &message)?;
        let mut saved = self.saved.clone();
        let changed = match &message {
            HostMessage::SetSetting { key, value } => {
                saved
                    .addons
                    .entry(id.into())
                    .or_default()
                    .settings
                    .insert(key.clone(), value.clone());
                true
            }
            HostMessage::SelectFormat { source, format } => {
                saved
                    .addons
                    .entry(id.into())
                    .or_default()
                    .formats
                    .insert(source.clone(), format.clone());
                true
            }
            _ => false,
        };
        if changed {
            self.store(saved)?;
        }
        let entry = self.entries.get_mut(id).unwrap();
        let connection = entry
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.connection.as_mut())
            .ok_or("This add-on is not connected")?;
        connection.send(&message).map_err(|e| e.to_string())?;
        if let HostMessage::SetSetting { key, value } = message {
            if let Some(setting) = entry
                .info
                .settings
                .iter_mut()
                .find(|setting| setting.declaration.key == key)
            {
                setting.value = value;
            }
        }
        Ok(())
    }

    fn poll(&mut self) {
        match self.host.poll() {
            Ok(events) => {
                if self
                    .error
                    .as_ref()
                    .is_some_and(|error| error.starts_with("Add-on listener failed:"))
                {
                    self.error = None;
                }
                for event in events {
                    if let HostEvent::Connected(mut connection) = event {
                        let id = connection.id().to_owned();
                        if let Some(entry) = self.entries.get_mut(&id) {
                            if let Some(runtime) = &mut entry.runtime {
                                if runtime.stop.is_none()
                                    && !hello_expired(runtime.started, Instant::now(), false)
                                    && runtime.connection.is_none()
                                {
                                    // An add-on that may ask learns at once whether it may.
                                    if has_capability(&entry.info, Capability::Control) {
                                        let _ = connection.send(&HostMessage::ControlAccess {
                                            allowed: entry.info.control_allowed,
                                        });
                                    }
                                    runtime.connection = Some(connection);
                                    entry.info.state = AddonState::Running;
                                    continue;
                                }
                            }
                        }
                        connection.close();
                    }
                    // Rejections come from unauthenticated peers and must not stop a legitimate run.
                }
            }
            Err(reason) => self.error = Some(format!("Add-on listener failed: {reason}")),
        }
        self.expire_control(Instant::now());
        let ids: Vec<_> = self.entries.keys().cloned().collect();
        for id in ids {
            if self.shutting_down
                && self.entries[&id].runtime.as_ref().is_some_and(|runtime| {
                    runtime
                        .stop
                        .as_ref()
                        .is_some_and(|(deadline, _)| Instant::now() >= *deadline)
                })
            {
                let entry = self.entries.get_mut(&id).unwrap();
                let mut runtime = entry.runtime.take().unwrap();
                if let Err(reason) = runtime.process.terminate() {
                    self.error = Some(format!("Cannot stop {id}: {reason}"));
                }
                entry.info.state = runtime.stop.take().unwrap().1;
                entry.info.pid = None;
                entry.clear_description();
                self.host.forget_addon(&id);
                // Closing the job is the final attempt. Do not retry a process
                // that Windows could not terminate during shutdown.
                continue;
            }
            let mut messages = Vec::new();
            let mut failure = None;
            let mut restart = false;
            {
                let entry = self.entries.get_mut(&id).unwrap();
                let Some(runtime) = &mut entry.runtime else {
                    continue;
                };
                let now = Instant::now();
                match runtime.process.try_wait() {
                    Ok(Some(code)) => {
                        let final_state = runtime.stop.take().map(|(_, state)| state);
                        if let Some(state) = final_state {
                            entry.info.state = state;
                            restart = runtime.restart_after_stop && !self.shutting_down;
                        } else {
                            let reason = format!("Add-on exited with code {code}");
                            restart = !self.shutting_down && entry.restarts.exited(now);
                            entry.info.state = if restart {
                                AddonState::Stopped
                            } else {
                                AddonState::Failed(format!(
                                    "{reason}; stopped after 3 exits within 60 seconds"
                                ))
                            };
                            entry.info.message = Some(AddonNotice {
                                level: MessageLevel::Error,
                                text: reason,
                            });
                        }
                        self.shared.discard_control(&id, None);
                        entry.runtime = None;
                        entry.info.pid = None;
                        entry.clear_description();
                        self.host.forget_addon(&id);
                    }
                    Err(reason) => {
                        failure = Some(AddonState::Failed(format!(
                            "Cannot inspect add-on process: {reason}"
                        )));
                    }
                    Ok(None) => {
                        if let Some((deadline, _)) = &runtime.stop {
                            if now >= *deadline {
                                if let Err(reason) = runtime.process.terminate() {
                                    self.error = Some(format!("Cannot stop {id}: {reason}"));
                                }
                            }
                        } else if hello_expired(runtime.started, now, runtime.connection.is_some())
                        {
                            failure = Some(AddonState::NotResponding);
                        }
                        if let Some(connection) = &mut runtime.connection {
                            for _ in 0..64 {
                                match connection.try_recv() {
                                    Ok(Some(message)) => {
                                        if runtime.stop.is_none() {
                                            messages.push(message);
                                        }
                                    }
                                    Ok(None) => break,
                                    Err(reason) => {
                                        failure = Some(AddonState::Failed(format!(
                                            "Invalid add-on message: {reason}"
                                        )));
                                        break;
                                    }
                                }
                            }
                            if connection.is_disconnected() && runtime.stop.is_none() {
                                entry.appearance = None;
                                // EOF can precede the process exit by a scheduling slice. Give a
                                // conforming child time to exit so the restart policy sees it.
                                let disconnected = runtime.disconnected_at.get_or_insert(now);
                                if now.saturating_duration_since(*disconnected) >= QUIT_TIMEOUT {
                                    failure.get_or_insert_with(|| {
                                        AddonState::Failed(
                                            "Add-on disconnected without exiting".into(),
                                        )
                                    });
                                }
                            }
                        }
                    }
                }
            }
            if restart {
                let _ = self.start(&id);
            }
            if let Some(state) = failure {
                self.stop(&id, state);
            }
            for message in messages {
                if let Err(reason) = self.receive(&id, message) {
                    self.stop(
                        &id,
                        AddonState::Failed(format!("Invalid add-on description: {reason}")),
                    );
                    break;
                }
            }
        }
    }

    fn receive(&mut self, id: &str, message: AddonMessage) -> Result<(), String> {
        // A control request is answered, never a reason to stop the add-on.
        match message {
            AddonMessage::Control {
                id: request,
                command,
            } => {
                self.control_request(id, Some(request), Ok(command));
                return Ok(());
            }
            AddonMessage::ControlRefused { id: request, error } => {
                self.control_request(id, request, Err(error));
                return Ok(());
            }
            _ => {}
        }
        message.validate().map_err(|e| e.to_string())?;
        let entry = self.entries.get_mut(id).unwrap();
        match message {
            AddonMessage::Describe {
                mut sources,
                mut settings,
                mut actions,
            } => {
                filter_description(&entry.info, &mut sources, &mut settings, &mut actions);
                validate_description(&sources, &settings, &actions)?;
                let saved = self.saved.addons.get(id).cloned().unwrap_or_default();
                let settings = settings
                    .into_iter()
                    .map(|declaration| {
                        let value = saved
                            .settings
                            .get(&declaration.key)
                            .filter(|value| declaration.validate_value(value).is_ok())
                            .cloned()
                            .unwrap_or_else(|| setting_default(&declaration));
                        AddonSetting { declaration, value }
                    })
                    .collect();
                let runtime = entry.runtime.as_mut().ok_or("The add-on has stopped")?;
                if !runtime.described {
                    if let Some(connection) = &mut runtime.connection {
                        for source in &sources {
                            if let Some(format) = saved.formats.get(&source.id).filter(|format| {
                                **format != source.format
                                    && source.formats.iter().any(|item| item.id == **format)
                            }) {
                                connection
                                    .send(&HostMessage::SelectFormat {
                                        source: source.id.clone(),
                                        format: format.clone(),
                                    })
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                    }
                    runtime.described = true;
                }
                entry.info.sources = sources;
                entry.position_filter.retain_sources(&entry.info.sources);
                entry.info.settings = settings;
                entry.info.actions = actions;
            }
            AddonMessage::Transport {
                source,
                state,
                position,
                duration,
            } => {
                if has_capability(&entry.info, Capability::Source) {
                    let Some(source) = entry.info.sources.iter_mut().find(|item| item.id == source)
                    else {
                        return Ok(());
                    };
                    if source.transport.is_none() {
                        return Ok(());
                    }
                    source.transport = Some(Transport {
                        state,
                        position,
                        duration,
                    });
                }
            }
            AddonMessage::Message { level, text } => {
                let text = text
                    .chars()
                    .map(|character| {
                        if character.is_control() {
                            ' '
                        } else {
                            character
                        }
                    })
                    .collect::<String>();
                entry.info.message = (!text.is_empty()).then_some(AddonNotice { level, text });
            }
            AddonMessage::Hello { .. } => return Err("hello may only be sent once".into()),
            AddonMessage::Positions {
                source,
                time,
                channels,
            } => {
                if !has_capability(&entry.info, Capability::Source) {
                    return Ok(());
                }
                let Some(source_description) =
                    entry.info.sources.iter().find(|item| item.id == source)
                else {
                    return Ok(());
                };
                if let Some(channels) = entry.position_filter.accept(
                    source_description,
                    time,
                    backtracking_core::capture_clock_ns(),
                    Instant::now(),
                    channels,
                ) {
                    self.shared
                        .positions
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(AddonPositions {
                            sequence: 0,
                            addon_id: id.into(),
                            source_id: source,
                            time,
                            channels,
                        });
                }
            }
            AddonMessage::Appearance(appearance) => {
                if entry.info.state == AddonState::Running
                    && has_capability(&entry.info, Capability::Appearance)
                    && !entry
                        .runtime
                        .as_ref()
                        .is_some_and(|run| run.disconnected_at.is_some())
                {
                    entry.appearance = (!appearance.is_empty()).then_some(appearance);
                }
            }
            AddonMessage::Control { .. } | AddonMessage::ControlRefused { .. } => {}
            AddonMessage::Unknown => {}
        }
        Ok(())
    }

    fn advance_operations(&mut self) {
        for inspection in std::mem::take(&mut self.inspections) {
            match inspection.result.try_recv() {
                Ok(Ok(prepared)) if !self.shutting_down => {
                    let id = prepared.info.manifest.id.clone();
                    if self.operations.iter().any(|operation| operation.id() == id) {
                        let _ = inspection.reply.send(Err(
                            "This add-on is already being installed or removed".into(),
                        ));
                        continue;
                    }
                    let restart = self
                        .entries
                        .get(&id)
                        .is_some_and(|entry| entry.runtime.is_some() || entry.info.enabled);
                    self.stop(&id, AddonState::Stopped);
                    self.operations.push(Operation::StopForInstall {
                        id,
                        prepared,
                        restart,
                        reply: inspection.reply,
                    });
                }
                Ok(Ok(_)) => {
                    let _ = inspection
                        .reply
                        .send(Err("The add-on manager is shutting down".into()));
                }
                Ok(Err(reason)) => {
                    let _ = inspection.reply.send(Err(reason));
                }
                Err(mpsc::TryRecvError::Empty) => self.inspections.push(inspection),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = inspection
                        .reply
                        .send(Err("Package inspection worker stopped".into()));
                }
            }
        }
        for operation in std::mem::take(&mut self.operations) {
            match operation {
                Operation::StopForInstall {
                    id,
                    prepared,
                    restart,
                    reply,
                } => {
                    if self
                        .entries
                        .get(&id)
                        .is_some_and(|entry| entry.runtime.is_some())
                    {
                        self.operations.push(Operation::StopForInstall {
                            id,
                            prepared,
                            restart,
                            reply,
                        });
                    } else if self.shutting_down {
                        let _ = reply.send(Err("The add-on manager is shutting down".into()));
                    } else {
                        let addons = self.root.join("addons");
                        let (send, result) = mpsc::channel();
                        spawn_task(
                            "install add-on",
                            move || {
                                package::install(&prepared.path, addons).map_err(|e| e.to_string())
                            },
                            send,
                            self.primary_lease.clone(),
                        );
                        self.operations.push(Operation::Install {
                            id,
                            restart,
                            result,
                            reply,
                        });
                    }
                }
                Operation::Install {
                    id,
                    restart,
                    result,
                    reply,
                } => match result.try_recv() {
                    Ok(result) => {
                        if let Ok(info) = &result {
                            let saved = self.saved.addons.get(&id);
                            let enabled = saved.is_some_and(|saved| saved.enabled);
                            // An update keeps the user's control choice; removal forgets it.
                            let control = saved.is_some_and(|saved| saved.control);
                            let mut entry = Entry::from_manifest(
                                id.clone(),
                                Ok(info.manifest.clone()),
                                enabled,
                            );
                            entry.info.control_allowed = control;
                            self.entries.insert(id.clone(), entry);
                        }
                        if !self.shutting_down && restart {
                            let _ = self.start(&id);
                        }
                        let _ = reply.send(result);
                    }
                    Err(mpsc::TryRecvError::Empty) => self.operations.push(Operation::Install {
                        id,
                        restart,
                        result,
                        reply,
                    }),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if !self.shutting_down && restart {
                            let _ = self.start(&id);
                        }
                        let _ = reply.send(Err("Package installation worker stopped".into()));
                    }
                },
                Operation::StopForRemove {
                    id,
                    delete_data,
                    reply,
                } => {
                    if self
                        .entries
                        .get(&id)
                        .is_some_and(|entry| entry.runtime.is_some())
                    {
                        self.operations.push(Operation::StopForRemove {
                            id,
                            delete_data,
                            reply,
                        });
                    } else {
                        let root = self.root.clone();
                        let remove_id = id.clone();
                        let (send, result) = mpsc::channel();
                        spawn_task(
                            "remove add-on",
                            move || {
                                package::remove(root.join("addons"), &remove_id)
                                    .map_err(|e| e.to_string())?;
                                if delete_data {
                                    remove_data(&root, &remove_id)?;
                                }
                                Ok(())
                            },
                            send,
                            self.primary_lease.clone(),
                        );
                        self.operations.push(Operation::Remove {
                            id,
                            delete_data,
                            result,
                            reply,
                        });
                    }
                }
                Operation::Remove {
                    id,
                    delete_data,
                    result,
                    reply,
                } => match result.try_recv() {
                    Ok(result) => {
                        if !self.root.join("addons").join(&id).exists() {
                            self.entries.remove(&id);
                            let mut saved = self.saved.clone();
                            if delete_data && result.is_ok() {
                                saved.addons.remove(&id);
                            } else if let Some(saved) = saved.addons.get_mut(&id) {
                                saved.enabled = false;
                                // Installing it again asks the user again.
                                saved.control = false;
                            }
                            let stored = self.store(saved);
                            let _ = reply.send(result.and(stored));
                        } else {
                            let _ = reply.send(result);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => self.operations.push(Operation::Remove {
                        id,
                        delete_data,
                        result,
                        reply,
                    }),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let _ = reply.send(Err("Package removal worker stopped".into()));
                    }
                },
            }
        }
    }
}

fn spawn_task<T: Send + 'static>(
    name: &str,
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
    reply: Reply<T>,
    primary_lease: startup::PrimaryLease,
) {
    let failed = reply.clone();
    if let Err(reason) = thread::Builder::new().name(name.into()).spawn(move || {
        let _primary_lease = primary_lease;
        let _ = reply.send(task());
    }) {
        let _ = failed.send(Err(reason.to_string()));
    }
}

fn prepare_package(root: &Path, original: &Path) -> Result<PreparedPackage, String> {
    if !original
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("btaddon"))
    {
        return Err("An add-on package must have the .btaddon extension".into());
    }
    // Hash and inspect the same private bytes, so replacing the download cannot
    // change the package between checking its identity and installing it.
    let path = root.join(format!(".install-{}.btaddon", random_token()?));
    let result = (|| {
        let mut input = File::open(original).map_err(|e| e.to_string())?;
        let mut output = OpenOptions::new()
            .mode(0o600)
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
        #[cfg(target_os = "macos")]
        package::quarantine::Quarantine::read(&input)
            .and_then(|q| q.apply(&output))
            .map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        let mut hasher = Sha256::new();
        let mut copied = File::open(&path).map_err(|e| e.to_string())?;
        let mut bytes = [0_u8; 64 * 1024];
        loop {
            let count = copied.read(&mut bytes).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            hasher.update(&bytes[..count]);
        }
        let info = package::inspect(&path).map_err(|e| e.to_string())?;
        Ok(PreparedPackage {
            path: path.clone(),
            info,
            identity: hasher.finalize().into(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&path);
    }
    result
}

fn reject_command(command: Command, reason: &str) {
    match command {
        Command::Inspect(_, reply) => {
            let _ = reply.send(Err(reason.into()));
        }
        Command::Install(_, _, reply) => {
            let _ = reply.send(Err(reason.into()));
        }
        Command::Remove(_, _, reply)
        | Command::Enabled(_, _, reply)
        | Command::Start(_, reply)
        | Command::Stop(_, reply)
        | Command::Send(_, _, reply)
        | Command::Clips(_, reply)
        | Command::Error(_, reply)
        | Command::ControlAllowed(_, _, reply) => {
            let _ = reply.send(Err(reason.into()));
        }
        Command::ControlReply { .. } | Command::Shutdown => {}
    }
}

fn not_allowed() -> ControlError {
    ControlError::new(
        control_codes::NOT_ALLOWED,
        "The user has not let this add-on control Backtracking. They can switch it on in \
         Options, Add-ons",
    )
}

fn has_capability(info: &AddonInfo, capability: Capability) -> bool {
    info.manifest
        .as_ref()
        .is_some_and(|manifest| manifest.capabilities.contains(&capability))
}

fn filter_description(
    info: &AddonInfo,
    sources: &mut Vec<Source>,
    settings: &mut Vec<Setting>,
    actions: &mut Vec<Action>,
) {
    if !has_capability(info, Capability::Source) {
        sources.clear();
    }
    if !has_capability(info, Capability::Settings) {
        settings.clear();
    }
    if !has_capability(info, Capability::Actions) {
        actions.clear();
    }
}

fn validate_command(info: &AddonInfo, message: &HostMessage) -> Result<(), String> {
    if info.state != AddonState::Running {
        return Err("This add-on is not running".into());
    }
    message.validate().map_err(|e| e.to_string())?;
    let capability = match message {
        HostMessage::SetSetting { .. } => Capability::Settings,
        HostMessage::RunAction { .. } => Capability::Actions,
        HostMessage::OpenFile { .. } => Capability::OpenFiles,
        HostMessage::SelectFormat { .. } | HostMessage::Transport { .. } => Capability::Source,
        HostMessage::ClipExported { .. } => Capability::Clips,
        _ => return Err("This message is reserved for the add-on host".into()),
    };
    if !has_capability(info, capability) {
        return Err("The add-on did not declare this capability".into());
    }
    match message {
        HostMessage::SetSetting { key, value } => info
            .settings
            .iter()
            .find(|setting| setting.declaration.key == *key)
            .ok_or("Unknown add-on setting")?
            .declaration
            .validate_value(value)
            .map_err(|e| e.to_string())?,
        HostMessage::RunAction { id } => {
            if !info.actions.iter().any(|action| action.id == *id) {
                return Err("Unknown add-on action".into());
            }
        }
        HostMessage::OpenFile { path } => {
            let extension = Path::new(path)
                .extension()
                .and_then(|value| value.to_str())
                .ok_or("The file has no extension")?;
            let open = info
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.open_files.as_ref())
                .ok_or("The add-on declared no file types")?;
            if !open
                .extensions
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(extension))
            {
                return Err("This file type is not registered by the add-on".into());
            }
            if !Path::new(path).is_file() {
                return Err("The selected file does not exist".into());
            }
        }
        HostMessage::SelectFormat { source, format } => {
            let source = info
                .sources
                .iter()
                .find(|item| item.id == *source)
                .ok_or("Unknown add-on source")?;
            if !source.formats.iter().any(|item| item.id == *format) {
                return Err("Unknown source format".into());
            }
        }
        HostMessage::Transport {
            source,
            command,
            position,
        } => {
            let transport = info
                .sources
                .iter()
                .find(|item| item.id == *source)
                .and_then(|source| source.transport.as_ref())
                .ok_or("This source does not offer transport controls")?;
            if *command == TransportCommand::Seek
                && position
                    .zip(transport.duration)
                    .is_some_and(|(position, duration)| position > duration)
            {
                return Err("Seek position exceeds the source duration".into());
            }
        }
        HostMessage::ClipExported {
            files,
            sample_rate,
            frames,
        } => {
            if files.is_empty() || *sample_rate == 0 || *frames == 0 {
                return Err(
                    "Clip notification requires files, a sample rate and audio frames".into(),
                );
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_description(
    sources: &[Source],
    settings: &[Setting],
    actions: &[Action],
) -> Result<(), String> {
    unique(sources.iter().map(|source| source.id.as_str()), "source id")?;
    unique(
        settings.iter().map(|setting| setting.key.as_str()),
        "setting key",
    )?;
    unique(actions.iter().map(|action| action.id.as_str()), "action id")?;
    for source in sources {
        unique(
            source.formats.iter().map(|format| format.id.as_str()),
            "format id",
        )?;
        if let Some(groups) = &source.groups {
            unique(groups.iter().map(|group| group.id.as_str()), "group id")?;
        }
    }
    for setting in settings {
        if let SettingKind::Choice { options, .. } = &setting.kind {
            unique(
                options.iter().map(|option| option.value.as_str()),
                "choice value",
            )?;
        }
    }
    Ok(())
}

fn unique<'a>(values: impl Iterator<Item = &'a str>, label: &str) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(format!("Duplicate {label}: {value}"));
        }
    }
    Ok(())
}

fn setting_default(setting: &Setting) -> Value {
    match &setting.kind {
        SettingKind::Toggle { default } => Value::Bool(*default),
        SettingKind::Choice { default, .. } | SettingKind::Text { default } => {
            Value::String(default.clone())
        }
        SettingKind::Number { default, .. } => Value::from(*default),
    }
}

fn read_saved(root: &Path) -> Result<SavedState, String> {
    let file = match File::open(root.join(STATE_FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SavedState::default());
        }
        Err(error) => return Err(format!("Cannot read add-on preferences: {error}")),
    };
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err("Add-on preferences exceed 8 MiB".into());
    }
    let saved: SavedState = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Cannot read add-on preferences: {e}"))?;
    for (id, state) in &saved.addons {
        validate_id(id).map_err(|e| e.to_string())?;
        HostMessage::Welcome {
            contract: 1,
            app: String::new(),
            app_version: String::new(),
            settings: state.settings.clone(),
        }
        .validate()
        .map_err(|e| e.to_string())?;
    }
    Ok(saved)
}

fn write_saved(root: &Path, saved: &SavedState) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(saved).map_err(|e| e.to_string())?;
    if data.len() as u64 > MAX_STATE_BYTES {
        return Err("Add-on preferences exceed 8 MiB".into());
    }
    let temporary = root.join(format!(".{STATE_FILE}-{}.tmp", random_token()?));
    let result = (|| {
        let mut file = OpenOptions::new()
            .mode(0o600)
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, root.join(STATE_FILE))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("Cannot save add-on preferences: {e}"))
}

fn remove_data(root: &Path, id: &str) -> Result<(), String> {
    validate_id(id).map_err(|e| e.to_string())?;
    let parent = root.join("addon-data");
    let target = parent.join(id);
    let metadata = match fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        return Err("The add-on data folder is a reparse point; remove it manually".into());
    }
    let parent = fs::canonicalize(parent).map_err(|e| e.to_string())?;
    let target = fs::canonicalize(target).map_err(|e| e.to_string())?;
    if target.parent() != Some(parent.as_path()) {
        return Err("The add-on data folder escapes its parent".into());
    }
    fs::remove_dir_all(target).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtracking_addon::manifest::{OpenFiles, RunEntry};
    use backtracking_addon::protocol::{ChannelGroup, Format, TransportState};
    use serde_json::json;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "backtracking-addon-host-{}",
                random_token().unwrap()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn info(capabilities: Vec<Capability>) -> AddonInfo {
        let manifest = Manifest {
            contract: 1,
            id: "com.example.test".into(),
            name: "Test".into(),
            version: "1".into(),
            author: "Test".into(),
            description: "Test".into(),
            homepage: None,
            license: None,
            open_files: capabilities
                .contains(&Capability::OpenFiles)
                .then(|| OpenFiles {
                    description: "Audio".into(),
                    extensions: vec!["wav".into()],
                }),
            capabilities,
            run: BTreeMap::from([(
                backtracking_addon::manifest::current_platform()
                    .unwrap()
                    .into(),
                RunEntry {
                    program: "test.exe".into(),
                    args: Vec::new(),
                },
            )]),
        };
        let mut entry = Entry::from_manifest(manifest.id.clone(), Ok(manifest), true);
        entry.info.state = AddonState::Running;
        entry.info.sources = vec![Source {
            id: "main".into(),
            name: "Test".into(),
            sender: "abc-123".into(),
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
            format: "stereo".into(),
            groups: Some(vec![ChannelGroup {
                id: "channels".into(),
                label: "Channels".into(),
                channels: vec!["L".into(), "R".into()],
                positioned: false,
            }]),
            transport: Some(Transport {
                state: TransportState::Playing,
                position: 0.0,
                duration: Some(60.0),
            }),
        }];
        entry.info.settings = vec![AddonSetting {
            declaration: Setting {
                key: "level".into(),
                label: "Level".into(),
                help: None,
                kind: SettingKind::Number {
                    default: -12.0,
                    min: -60.0,
                    max: 0.0,
                    step: 1.0,
                },
            },
            value: json!(-12),
        }];
        entry.info.actions = vec![Action {
            id: "restart".into(),
            label: "Restart".into(),
        }];
        entry.info
    }

    fn worker(root: &Path) -> Worker {
        let shared = Arc::new(Shared::default());
        Worker::new(root.to_owned(), Host::bind("test").unwrap(), shared).unwrap()
    }

    fn appearance_fixture(worker: &mut Worker, capability: Capability) -> (AddonHandle, String) {
        let mut info = info(vec![capability]);
        info.sources.clear();
        info.settings.clear();
        info.actions.clear();
        let id = info.id.clone();
        let mut entry = Entry::from_manifest(id.clone(), Ok(info.manifest.clone().unwrap()), true);
        entry.info = info;
        worker.entries.insert(id.clone(), entry);
        worker.publish();
        let (commands, _) = mpsc::channel();
        let handle = AddonHandle {
            commands,
            shared: worker.shared.clone(),
        };
        (handle, id)
    }

    fn offer_appearance(worker: &mut Worker, id: &str, json: &str) {
        let message = backtracking_addon::protocol::decode_addon(json.as_bytes()).unwrap();
        worker.receive(id, message).unwrap();
        worker.publish();
    }

    #[test]
    fn appearance_requires_the_capability() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let (handle, id) = appearance_fixture(&mut worker, Capability::Source);
        let before = handle.snapshot();
        let initial = handle.try_appearances().unwrap();
        for offer in [
            r##"{"type":"appearance","colors":{"waveform":"#67e7b5"}}"##,
            r##"{"type":"appearance","spectrum":[{"color":"#ff8c3c","position":0},{"color":"#6ec8ff","position":1}]}"##,
        ] {
            offer_appearance(&mut worker, &id, offer);
        }
        assert!(initial.is_empty());
        assert!(Arc::ptr_eq(&initial, &handle.try_appearances().unwrap()));
        assert!(Arc::ptr_eq(&before, &handle.snapshot()));
    }

    #[test]
    fn appearance_publication_replaces_the_offer_without_waking_capture() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let (handle, id) = appearance_fixture(&mut worker, Capability::Appearance);
        let summary = handle.snapshot();
        let before = handle.try_appearances().unwrap();
        let capture_wakes = Arc::new(AtomicUsize::new(0));
        let wakes = capture_wakes.clone();
        handle.subscribe(Arc::new(move || {
            wakes.fetch_add(1, Ordering::Relaxed);
        }));
        let repaints = Arc::new(AtomicUsize::new(0));
        let count = repaints.clone();
        handle.subscribe_appearances(Arc::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
        }));
        let theme = r##"{"type":"appearance","colors":{"waveform":"#67e7b5","border":"#ffffff80"},"rows":{"LFE":"#ff8800"}}"##;
        offer_appearance(&mut worker, &id, theme);
        let look = handle.try_appearances().unwrap();
        assert!(!Arc::ptr_eq(&before, &look));
        assert_eq!(look[&id].colors["waveform"], [103, 231, 181, 255]);
        assert_eq!(look[&id].colors["border"], [255, 255, 255, 128]);
        assert_eq!(look[&id].rows["LFE"], [255, 136, 0, 255]);
        assert_eq!(repaints.load(Ordering::Relaxed), 1);

        offer_appearance(&mut worker, &id, theme);
        offer_appearance(&mut worker, &id, r#"{"type":"appearance","rows":null}"#);
        assert!(Arc::ptr_eq(&look, &handle.try_appearances().unwrap()));
        assert_eq!(repaints.load(Ordering::Relaxed), 1);

        offer_appearance(
            &mut worker,
            &id,
            r#"{"type":"appearance","numbers":{"border_width":9}}"#,
        );
        let replaced = handle.try_appearances().unwrap();
        assert!(!Arc::ptr_eq(&replaced, &look));
        assert!(replaced[&id].colors.is_empty());
        assert!(replaced[&id].rows.is_empty());
        assert_eq!(replaced[&id].numbers["border_width"], 8.0);

        // A gradient alone is an offer; the next offer without one clears it.
        let gradient = r##"{"type":"appearance","spectrum":[{"color":"#ff8c3c","position":0},{"color":"#6ec8ff80","position":1}]}"##;
        offer_appearance(&mut worker, &id, gradient);
        let spectrum = &handle.try_appearances().unwrap()[&id].spectrum;
        assert_eq!(spectrum.len(), 2);
        assert_eq!(spectrum[1].color, [110, 200, 255, 128]);
        assert_eq!(spectrum[1].position, 1.0);
        offer_appearance(
            &mut worker,
            &id,
            r#"{"type":"appearance","numbers":{"border_width":9}}"#,
        );
        assert!(handle.try_appearances().unwrap()[&id].spectrum.is_empty());

        offer_appearance(&mut worker, &id, r#"{"type":"appearance"}"#);
        assert!(handle.try_appearances().unwrap().is_empty());
        assert!(Arc::ptr_eq(&summary, &handle.snapshot()));
        assert_eq!(repaints.load(Ordering::Relaxed), 5);
        assert_eq!(capture_wakes.load(Ordering::Relaxed), 0);
        assert!(!dir.0.join(STATE_FILE).exists());
    }

    #[test]
    fn stopped_disabled_and_failed_addons_clear_their_appearance() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let (handle, id) = appearance_fixture(&mut worker, Capability::Appearance);
        let theme = r##"{"type":"appearance","colors":{"waveform":"#67e7b5"}}"##;

        for state in [AddonState::Stopped, AddonState::Failed("failed".into())] {
            worker.entries.get_mut(&id).unwrap().info.state = AddonState::Running;
            offer_appearance(&mut worker, &id, theme);
            assert!(handle.try_appearances().unwrap().contains_key(&id));
            worker.stop(&id, state.clone());
            worker.publish();
            assert_eq!(handle.snapshot().addons[0].state, state);
            assert!(handle.try_appearances().unwrap().is_empty());
            offer_appearance(&mut worker, &id, theme);
            assert!(handle.try_appearances().unwrap().is_empty());
        }

        worker.entries.get_mut(&id).unwrap().info.state = AddonState::Running;
        offer_appearance(&mut worker, &id, theme);
        let (reply, result) = mpsc::channel();
        worker.command(Command::Enabled(id.clone(), false, reply));
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        worker.publish();
        assert!(!handle.snapshot().addons[0].enabled);
        assert!(handle.try_appearances().unwrap().is_empty());
        let saved = fs::read_to_string(dir.0.join(STATE_FILE)).unwrap();
        assert!(!saved.contains("appearance") && !saved.contains("waveform"));
    }

    #[test]
    fn bridge_delivers_appearance_changes_and_clears_them_when_primary_disappears() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let (handle, id) = appearance_fixture(&mut worker, Capability::Appearance);
        offer_appearance(
            &mut worker,
            &id,
            r##"{"type":"appearance","colors":{"border":"#ffffff80"}}"##,
        );
        let primary = bridge::Bridge::start(&dir.0, handle).unwrap();
        let secondary = AddonManager::connect_existing(dir.0.clone()).unwrap();
        let proxy = secondary.handle();
        let summary = proxy.snapshot();
        let deadline = Instant::now() + Duration::from_secs(5);
        let initial = loop {
            if let Some(offers) = proxy.try_appearances() {
                break offers;
            }
            assert!(
                Instant::now() < deadline,
                "proxy did not publish its initial appearance"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(initial[&id].colors["border"], [255, 255, 255, 128]);

        // Secondary windows take the gradient as well.
        offer_appearance(
            &mut worker,
            &id,
            r##"{"type":"appearance","colors":{"border":"#a1b2c3d4"},"spectrum":[{"color":"#ff8c3c","position":0},{"color":"#f0dc5a","position":0.45},{"color":"#6ec8ff","position":1}]}"##,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let changed = loop {
            if let Some(offers) = proxy.try_appearances() {
                if !Arc::ptr_eq(&offers, &initial) {
                    break offers;
                }
            }
            assert!(
                Instant::now() < deadline,
                "appearance update did not arrive"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(changed[&id].colors["border"], [161, 178, 195, 212]);
        let stops: Vec<_> = changed[&id]
            .spectrum
            .iter()
            .map(|stop| (stop.color, stop.position))
            .collect();
        assert_eq!(
            stops,
            [
                ([255, 140, 60, 255], 0.0),
                ([240, 220, 90, 255], 0.45),
                ([110, 200, 255, 255], 1.0)
            ]
        );
        assert!(Arc::ptr_eq(&summary, &proxy.snapshot()));

        drop(primary);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if proxy.snapshot().error.is_some() {
                if let Some(offers) = proxy.try_appearances() {
                    assert!(offers.is_empty());
                    break;
                }
            }
            assert!(Instant::now() < deadline, "offline snapshot did not arrive");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn position_message(time: u64) -> AddonPositions {
        AddonPositions {
            sequence: 0,
            addon_id: "com.example.test".into(),
            source_id: "main".into(),
            time,
            channels: vec![ChannelPosition::new(0, 0.5, 0.5, 0.0)],
        }
    }

    #[test]
    fn position_journal_is_bounded_and_readers_do_not_consume_each_others_updates() {
        let mut journal = PositionJournal::default();
        for time in 0..20_000 {
            journal.push(position_message(time));
        }
        assert!(journal.bytes <= MAX_POSITION_BYTES);
        assert!(journal.messages.front().unwrap().time > 0);
        assert_eq!(journal.messages.back().unwrap().time, 19_999);
        let (mut a, mut b) = (0, 0);
        let first = journal.since(&mut a);
        let second = journal.since(&mut b);
        assert_eq!((a, b), (20_000, 20_000));
        assert_eq!(first, second);
        assert!(Arc::ptr_eq(&first[0], &second[0]));
        assert!(journal.since(&mut a).is_empty());
        journal.push(position_message(20_000));
        assert_eq!(journal.since(&mut a)[0].time, 20_000);
        assert_eq!(journal.since(&mut b)[0].time, 20_000);
    }

    #[test]
    fn positions_ignore_unknown_sources_and_invalid_entries_without_changing_addon_state() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let mut info = info(vec![Capability::Source]);
        info.sources[0].groups.as_mut().unwrap().push(ChannelGroup {
            id: "objects".into(),
            label: "Objects".into(),
            channels: vec!["Object 1".into(), "Object 2".into()],
            positioned: true,
        });
        let id = info.id.clone();
        let mut entry = Entry::from_manifest(id.clone(), Ok(info.manifest.clone().unwrap()), true);
        entry.info = info;
        worker.entries.insert(id.clone(), entry);
        let time = backtracking_core::capture_clock_ns();
        for source in ["unknown", "main"] {
            let mut invalid = ChannelPosition::new(3, f64::NAN, 0.0, 0.0);
            invalid.gain = Some(f64::INFINITY);
            let mut clamped = ChannelPosition::new(2, -5.0, 4.0, 2.0);
            clamped.size = Some(5.0);
            worker
                .receive(
                    &id,
                    AddonMessage::Positions {
                        source: source.into(),
                        time,
                        channels: vec![
                            ChannelPosition::new(0, 0.0, 0.0, 0.0),
                            invalid,
                            ChannelPosition::new(63, 0.0, 0.0, 0.0),
                            clamped,
                        ],
                    },
                )
                .unwrap();
        }
        let mut cursor = 0;
        let positions = worker.shared.positions.lock().unwrap().since(&mut cursor);
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].channels.len(), 1);
        let channel = &positions[0].channels[0];
        assert_eq!(
            (
                channel.channel,
                channel.x,
                channel.y,
                channel.z,
                channel.size
            ),
            (2, -1.0, 1.0, 1.0, Some(1.0))
        );
        assert_eq!(worker.entries[&id].info.state, AddonState::Running);
        assert_eq!(worker.shared.lock_published().snapshot.revision, 0);
    }

    #[test]
    fn positions_require_capability_and_flood_is_thinned_without_stopping_addon() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let mut info = info(vec![Capability::Source]);
        info.sources[0].groups.as_mut().unwrap()[0].positioned = true;
        let id = info.id.clone();
        let mut entry = Entry::from_manifest(id.clone(), Ok(info.manifest.clone().unwrap()), true);
        entry.info = info;
        worker.entries.insert(id.clone(), entry);
        let time = backtracking_core::capture_clock_ns();
        for _ in 0..60 {
            worker
                .receive(
                    &id,
                    AddonMessage::Positions {
                        source: "main".into(),
                        time,
                        channels: vec![ChannelPosition::new(0, 0.0, 1.0, 0.0)],
                    },
                )
                .unwrap();
        }
        let mut cursor = 0;
        assert_eq!(
            worker
                .shared
                .positions
                .lock()
                .unwrap()
                .since(&mut cursor)
                .len(),
            50
        );
        worker
            .entries
            .get_mut(&id)
            .unwrap()
            .info
            .manifest
            .as_mut()
            .unwrap()
            .capabilities
            .clear();
        worker
            .receive(
                &id,
                AddonMessage::Positions {
                    source: "main".into(),
                    time,
                    channels: vec![ChannelPosition::new(0, 0.0, 1.0, 0.0)],
                },
            )
            .unwrap();
        assert!(
            worker
                .shared
                .positions
                .lock()
                .unwrap()
                .since(&mut cursor)
                .is_empty()
        );
        assert_eq!(worker.entries[&id].info.state, AddonState::Running);
        worker.entries.get_mut(&id).unwrap().clear_description();
        assert!(worker.entries[&id].info.sources.is_empty());
    }

    #[test]
    fn positions_receive_cost_for_sixteen_objects() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let mut info = info(vec![Capability::Source]);
        info.sources[0].groups.as_mut().unwrap()[0].channels =
            (1..=16).map(|object| format!("Object {object}")).collect();
        info.sources[0].groups.as_mut().unwrap()[0].positioned = true;
        let id = info.id.clone();
        let mut entry = Entry::from_manifest(id.clone(), Ok(info.manifest.clone().unwrap()), true);
        entry.info = info;
        worker.entries.insert(id.clone(), entry);
        let mut elapsed = Duration::ZERO;
        for _ in 0..200 {
            // Reset between batches outside the measurement so every measured
            // state goes through the accepted-message path, not flood thinning.
            worker.entries.get_mut(&id).unwrap().position_filter.clear();
            for _ in 0..50 {
                let message = AddonMessage::Positions {
                    source: "main".into(),
                    time: backtracking_core::capture_clock_ns(),
                    channels: (0..16)
                        .map(|channel| ChannelPosition::new(channel, 0.5, 0.5, 0.0))
                        .collect(),
                };
                let started = Instant::now();
                worker.receive(&id, message).unwrap();
                elapsed += started.elapsed();
            }
        }
        println!(
            "positions manager receive: 10000 accepted messages, 16 objects; {:.3} us/message; journal {} accounted bytes",
            elapsed.as_secs_f64() * 1_000_000.0 / 10_000.0,
            worker.shared.positions.lock().unwrap().bytes
        );
        assert_eq!(worker.shared.positions.lock().unwrap().sequence, 10_000);
    }

    #[test]
    fn allowed_messages_sanitize_controls_and_ignore_unoffered_transport() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let info = info(vec![Capability::Source]);
        let id = info.id.clone();
        worker.entries.insert(
            id.clone(),
            Entry {
                info,
                runtime: None,
                restarts: RestartPolicy::default(),
                position_filter: PositionFilter::default(),
                appearance: None,
            },
        );
        worker
            .receive(
                &id,
                AddonMessage::Message {
                    level: MessageLevel::Error,
                    text: "Cannot open\nfile\t\u{7f}".into(),
                },
            )
            .unwrap();
        assert_eq!(
            worker.entries[&id].info.message.as_ref().unwrap().text,
            "Cannot open file  "
        );
        worker.entries.get_mut(&id).unwrap().info.sources[0].transport = None;
        for source in ["main", "unknown"] {
            worker
                .receive(
                    &id,
                    AddonMessage::Transport {
                        source: source.into(),
                        state: TransportState::Playing,
                        position: 1.0,
                        duration: Some(60.0),
                    },
                )
                .unwrap();
        }
        assert!(worker.entries[&id].info.sources[0].transport.is_none());
        assert_eq!(worker.entries[&id].info.state, AddonState::Running);
    }

    #[test]
    fn successful_listener_poll_clears_only_the_listener_error() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        worker.error = Some("Add-on listener failed: injected failure".into());
        worker.poll();
        assert!(worker.error.is_none());
        worker.error = Some("Cannot read add-on preferences".into());
        worker.poll();
        assert_eq!(
            worker.error.as_deref(),
            Some("Cannot read add-on preferences")
        );
    }

    #[test]
    fn transport_for_unknown_and_undeclared_sources_is_ignored() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let mut info = info(vec![Capability::Source]);
        info.sources[0].transport = None;
        let id = info.id.clone();
        worker.entries.insert(
            id.clone(),
            Entry {
                info,
                runtime: None,
                restarts: RestartPolicy::default(),
                position_filter: PositionFilter::default(),
                appearance: None,
            },
        );
        let results: Vec<_> = ["unknown", "main"]
            .into_iter()
            .map(|source| {
                worker.receive(
                    &id,
                    AddonMessage::Transport {
                        source: source.into(),
                        state: TransportState::Playing,
                        position: 1.0,
                        duration: None,
                    },
                )
            })
            .collect();
        assert_eq!(results, vec![Ok(()), Ok(())]);
        assert!(worker.entries[&id].info.sources[0].transport.is_none());
    }

    #[test]
    fn shutdown_releases_process_after_one_failed_termination() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let executable = std::env::current_exe().unwrap();
        let mut process = AddonProcess::launch(
            &executable,
            &[
                "--exact".into(),
                "addons::process::tests::helper_child".into(),
            ],
            executable.parent().unwrap(),
            &[("BACKTRACKING_PROCESS_UNIT_TEST_CHILD".into(), "1".into())],
        )
        .unwrap();
        process.fail_termination = true;
        let info = info(Vec::new());
        let id = info.id.clone();
        worker.entries.insert(
            id.clone(),
            Entry {
                info,
                runtime: Some(Runtime {
                    process,
                    started: Instant::now(),
                    connection: None,
                    described: false,
                    disconnected_at: None,
                    stop: Some((Instant::now(), AddonState::Stopped)),
                    restart_after_stop: false,
                    run: 1,
                    control: BTreeMap::new(),
                }),
                restarts: RestartPolicy::default(),
                position_filter: PositionFilter::default(),
                appearance: None,
            },
        );
        worker.shutting_down = true;
        worker.poll();
        assert!(worker.entries[&id].runtime.is_none());
        assert!(
            worker
                .error
                .as_deref()
                .unwrap()
                .contains("Injected termination failure")
        );
    }

    /// A running entry with a real connection: the add-on side is an SDK client.
    fn connected_control_fixture(
        worker: &mut Worker,
        capabilities: Vec<Capability>,
    ) -> (String, backtracking_addon::client::Client) {
        let executable = std::env::current_exe().unwrap();
        let process = AddonProcess::launch(
            &executable,
            &[
                "--exact".into(),
                "addons::process::tests::helper_child".into(),
            ],
            executable.parent().unwrap(),
            &[("BACKTRACKING_PROCESS_UNIT_TEST_CHILD".into(), "1".into())],
        )
        .unwrap();
        let mut info = info(capabilities);
        info.sources.clear();
        let id = info.id.clone();
        let token = random_token().unwrap();
        worker
            .host
            .expect_addon(&id, &token, BTreeMap::new())
            .unwrap();
        let port = worker.host.port();
        let data = worker.root.clone();
        let client_id = id.clone();
        let client = thread::spawn(move || {
            backtracking_addon::client::Client::connect(
                port,
                &token,
                data,
                &client_id,
                "1",
                Duration::from_secs(5),
            )
            .unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let connection = loop {
            if let Some(HostEvent::Connected(connection)) =
                worker.host.poll().unwrap().into_iter().next()
            {
                break connection;
            }
            assert!(Instant::now() < deadline, "the client did not connect");
            thread::sleep(Duration::from_millis(2));
        };
        worker.runs += 1;
        worker.entries.insert(
            id.clone(),
            Entry {
                info,
                runtime: Some(Runtime {
                    process,
                    started: Instant::now(),
                    connection: Some(connection),
                    described: true,
                    disconnected_at: None,
                    stop: None,
                    restart_after_stop: false,
                    run: worker.runs,
                    control: BTreeMap::new(),
                }),
                restarts: RestartPolicy::default(),
                position_filter: PositionFilter::default(),
                appearance: None,
            },
        );
        (id, client.join().unwrap())
    }

    fn send_control(worker: &mut Worker, id: &str, line: &str) {
        let message = backtracking_addon::protocol::decode_addon(line.as_bytes()).unwrap();
        worker.receive(id, message).unwrap();
        // The add-on is never stopped for a control request, good or bad.
        assert_eq!(worker.entries[id].info.state, AddonState::Running);
    }

    /// The next control message the add-on gets, as its JSON.
    fn next_control(worker: &mut Worker, client: &mut backtracking_addon::client::Client) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            worker.poll();
            if let Some(message) = client.try_recv().unwrap() {
                let line = backtracking_addon::protocol::encode_host(&message).unwrap();
                return serde_json::from_slice(&line).unwrap();
            }
            assert!(Instant::now() < deadline, "no control message arrived");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn control_requests_are_refused_until_allowed_then_queued_and_answered_once() {
        let dir = TestDir::new();
        let mut worker = worker(&dir.0);
        let (commands, _) = mpsc::channel();
        let handle = AddonHandle {
            commands,
            shared: worker.shared.clone(),
        };
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = wakes.clone();
        handle.subscribe_control(Arc::new(move || {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));

        // Without the capability every request gets not_declared.
        let (undeclared, mut client) = connected_control_fixture(&mut worker, Vec::new());
        send_control(
            &mut worker,
            &undeclared,
            r#"{"type":"control","id":"1","command":"get_state"}"#,
        );
        let reply = next_control(&mut worker, &mut client);
        assert_eq!(reply["type"], "control_reply");
        assert_eq!(reply["id"], "1");
        assert_eq!(reply["error"]["code"], "not_declared");
        worker.entries.clear();

        let (id, mut client) = connected_control_fixture(&mut worker, vec![Capability::Control]);
        // Off by default: refused, and the page can say it asked.
        send_control(
            &mut worker,
            &id,
            r#"{"type":"control","id":"a","command":"pause_capture"}"#,
        );
        let reply = next_control(&mut worker, &mut client);
        assert_eq!(reply["error"]["code"], "not_allowed");
        assert!(worker.entries[&id].info.control_asked);
        assert!(handle.take_control_calls().is_empty());

        let (reply, result) = mpsc::channel();
        worker.command(Command::ControlAllowed(id.clone(), true, reply));
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(
            next_control(&mut worker, &mut client),
            json!({"type":"control_access","allowed":true})
        );
        assert!(worker.entries[&id].info.control_allowed);
        assert!(!worker.entries[&id].info.control_asked);
        let saved = fs::read_to_string(dir.0.join(STATE_FILE)).unwrap();
        assert!(saved.contains("\"control\": true"), "{saved}");

        // A bad request is answered from what the SDK read, not queued.
        send_control(
            &mut worker,
            &id,
            r#"{"type":"control","id":"b","command":"open_options"}"#,
        );
        assert_eq!(
            next_control(&mut worker, &mut client)["error"]["code"],
            "unknown_command"
        );

        send_control(
            &mut worker,
            &id,
            r#"{"type":"control","id":"c","command":"select","from_seconds_ago":2}"#,
        );
        send_control(
            &mut worker,
            &id,
            r#"{"type":"control","id":"c","command":"get_state"}"#,
        );
        assert_eq!(
            next_control(&mut worker, &mut client)["error"]["code"],
            "duplicate_id"
        );
        let calls = handle.take_control_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].request_id, "c");
        assert_eq!(calls[0].addon_name, "Test");
        assert_eq!(
            calls[0].command,
            ControlCommand::Select {
                from_seconds_ago: Some(2.0),
                to_seconds_ago: None,
                start_frame: None,
                end_frame: None,
            }
        );
        assert_eq!(wakes.load(std::sync::atomic::Ordering::Relaxed), 1);

        // The main window's reply reaches the add-on once; a second one is dropped.
        let state = ControlResult::default();
        for _ in 0..2 {
            let (addon_id, run) = (calls[0].addon_id.clone(), calls[0].run);
            worker.command(Command::ControlReply {
                addon_id,
                run,
                reply: ControlReply::success("c", &state),
            });
        }
        let reply = next_control(&mut worker, &mut client);
        assert_eq!(reply["id"], "c");
        assert!(reply["result"]["state"].is_object());
        assert!(
            worker.entries[&id]
                .runtime
                .as_ref()
                .unwrap()
                .control
                .is_empty()
        );

        // At most eight wait at once.
        for index in 0..=MAX_CONTROL_IN_FLIGHT {
            send_control(
                &mut worker,
                &id,
                &format!(r#"{{"type":"control","id":"f{index}","command":"get_state"}}"#),
            );
        }
        let reply = next_control(&mut worker, &mut client);
        assert_eq!(reply["id"], format!("f{MAX_CONTROL_IN_FLIGHT}"));
        assert_eq!(reply["error"]["code"], "too_many_requests");

        // Unanswered requests time out with an error.
        for deadline in worker
            .entries
            .get_mut(&id)
            .unwrap()
            .runtime
            .as_mut()
            .unwrap()
            .control
            .values_mut()
            .take(1)
        {
            *deadline = Instant::now();
        }
        let reply = next_control(&mut worker, &mut client);
        assert_eq!(reply["error"]["code"], "timeout");

        // Revoking refuses what is still waiting and empties the queue.
        let (reply, result) = mpsc::channel();
        worker.command(Command::ControlAllowed(id.clone(), false, reply));
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        let mut refused = 0;
        loop {
            let message = next_control(&mut worker, &mut client);
            if message["type"] == "control_access" {
                assert_eq!(message["allowed"], false);
                break;
            }
            assert_eq!(message["error"]["code"], "not_allowed");
            refused += 1;
        }
        assert_eq!(refused, MAX_CONTROL_IN_FLIGHT - 1);
        assert!(handle.take_control_calls().is_empty());
        assert!(!worker.entries[&id].info.control_allowed);
        send_control(
            &mut worker,
            &id,
            r#"{"type":"control","id":"revoked","command":"get_state"}"#,
        );
        assert_eq!(
            next_control(&mut worker, &mut client)["error"]["code"],
            "not_allowed"
        );
        assert!(worker.entries[&id].info.control_asked);
        assert!(handle.take_control_calls().is_empty());
    }

    #[test]
    fn the_control_choice_survives_a_restart_and_is_forgotten_on_removal() {
        let dir = TestDir::new();
        let folder = dir.0.join("package");
        fs::create_dir(&folder).unwrap();
        let mut manifest = info(vec![Capability::Control]).manifest.unwrap();
        manifest.id = "com.example.control".into();
        fs::write(
            folder.join("addon.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(folder.join("test.exe"), b"package fixture").unwrap();
        let path = dir.0.join("control.btaddon");
        package::pack(&folder, &path).unwrap();
        package::install(&path, dir.0.join("addons")).unwrap();
        let id = "com.example.control";
        let allowed = |manager: &AddonManager| {
            manager
                .handle()
                .snapshot()
                .addon(id)
                .map(|addon| addon.control_allowed)
        };
        let wait_for = |manager: &AddonManager, wanted: Option<bool>| {
            let deadline = Instant::now() + Duration::from_secs(3);
            while allowed(manager) != wanted && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(allowed(manager), wanted);
        };

        let manager = AddonManager::new(dir.0.clone(), "test").unwrap();
        wait_for(&manager, Some(false));
        manager
            .handle()
            .set_control_allowed(id, true)
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        wait_for(&manager, Some(true));
        drop(manager);

        let manager = AddonManager::new(dir.0.clone(), "test").unwrap();
        wait_for(&manager, Some(true));
        manager
            .handle()
            .remove(id, false)
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        wait_for(&manager, None);
        drop(manager);

        package::install(&path, dir.0.join("addons")).unwrap();
        let manager = AddonManager::new(dir.0.clone(), "test").unwrap();
        wait_for(&manager, Some(false));
    }

    #[test]
    fn manager_drop_does_not_wait_forever_for_a_stuck_worker() {
        let (release, released) = mpsc::channel();
        let worker = thread::spawn(move || released.recv().unwrap());
        let (commands, _receive) = mpsc::channel();
        let manager = AddonManager {
            handle: AddonHandle {
                commands,
                shared: Arc::new(Shared::default()),
            },
            worker: Some(worker),
            bridge: None,
        };
        let release_later = thread::spawn(move || {
            thread::sleep(Duration::from_secs(6));
            let _ = release.send(());
        });
        let started = Instant::now();
        drop(manager);
        let elapsed = started.elapsed();
        release_later.join().unwrap();
        assert!(
            elapsed < Duration::from_millis(5_750),
            "Drop blocked for {elapsed:?}"
        );
    }

    #[test]
    fn primary_startup_cleans_stale_files_but_a_second_manager_cannot_clean() {
        let dir = TestDir::new();
        fs::create_dir(dir.0.join("addons")).unwrap();
        let lock = dir.0.join("addons/.com.example.test.lock");
        let private = dir.0.join(format!(".install-{}.btaddon", "a".repeat(64)));
        fs::write(&lock, b"stale").unwrap();
        fs::write(&private, b"stale").unwrap();
        let manager = AddonManager::new(dir.0.clone(), "test").unwrap();
        assert!(!lock.exists() && !private.exists());
        let stage = dir.0.join("addons/.com.example.test.install-1-1");
        fs::create_dir(&stage).unwrap();
        assert!(AddonManager::new(dir.0.clone(), "test").is_err());
        assert!(stage.exists());
        drop(manager);
    }

    #[test]
    fn detached_package_task_keeps_primary_ownership_until_it_finishes() {
        let dir = TestDir::new();
        let worker = worker(&dir.0);
        let (release, released) = mpsc::channel();
        let (reply, result) = mpsc::channel();
        spawn_task(
            "ownership test",
            move || {
                released.recv().unwrap();
                Ok(())
            },
            reply,
            worker.primary_lease.clone(),
        );
        drop(worker);
        assert!(startup::PrimaryLease::acquire(&dir.0).is_err());
        release.send(()).unwrap();
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if startup::PrimaryLease::acquire(&dir.0).is_ok() {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(POLL_INTERVAL);
        }
    }

    #[test]
    fn restart_stops_on_third_exit_in_rolling_sixty_seconds() {
        let now = Instant::now();
        let mut policy = RestartPolicy::default();
        assert!(policy.exited(now));
        assert!(policy.exited(now + Duration::from_secs(20)));
        assert!(!policy.exited(now + Duration::from_secs(60)));
        assert!(!policy.exited(now + Duration::from_secs(61)));
        assert!(policy.exited(now + Duration::from_secs(122)));
    }

    #[test]
    fn hello_deadline_uses_process_launch_and_accepts_only_before_ten_seconds() {
        let now = Instant::now();
        assert!(!hello_expired(
            now,
            now + HELLO_TIMEOUT - Duration::from_nanos(1),
            false
        ));
        assert!(hello_expired(now, now + HELLO_TIMEOUT, false));
        assert!(hello_expired(
            now,
            now + HELLO_TIMEOUT + Duration::from_secs(20),
            false
        ));
        assert!(!hello_expired(now, now + HELLO_TIMEOUT, true));
    }

    #[test]
    fn outbound_commands_require_capabilities_and_known_valid_values() {
        let empty = info(Vec::new());
        for command in [
            HostMessage::SetSetting {
                key: "level".into(),
                value: json!(-6),
            },
            HostMessage::RunAction {
                id: "restart".into(),
            },
            HostMessage::OpenFile {
                path: "C:\\test.wav".into(),
            },
            HostMessage::SelectFormat {
                source: "main".into(),
                format: "5.1".into(),
            },
            HostMessage::Transport {
                source: "main".into(),
                command: TransportCommand::Play,
                position: None,
            },
            HostMessage::ClipExported {
                files: vec!["C:\\test.wav".into()],
                sample_rate: 48000,
                frames: 100,
            },
        ] {
            assert!(
                validate_command(&empty, &command)
                    .unwrap_err()
                    .contains("capability")
            );
        }
        let full = info(vec![
            Capability::Source,
            Capability::Settings,
            Capability::Actions,
            Capability::Clips,
        ]);
        assert!(
            validate_command(
                &full,
                &HostMessage::SetSetting {
                    key: "level".into(),
                    value: json!(-6)
                }
            )
            .is_ok()
        );
        assert!(
            validate_command(
                &full,
                &HostMessage::SetSetting {
                    key: "level".into(),
                    value: json!(1)
                }
            )
            .is_err()
        );
        assert!(
            validate_command(
                &full,
                &HostMessage::SetSetting {
                    key: "other".into(),
                    value: json!(-6)
                }
            )
            .is_err()
        );
        assert!(validate_command(&full, &HostMessage::RunAction { id: "other".into() }).is_err());
        assert!(
            validate_command(
                &full,
                &HostMessage::SelectFormat {
                    source: "main".into(),
                    format: "5.1".into()
                }
            )
            .is_ok()
        );
        assert!(
            validate_command(
                &full,
                &HostMessage::SelectFormat {
                    source: "main".into(),
                    format: "missing".into()
                }
            )
            .is_err()
        );
        assert!(
            validate_command(
                &full,
                &HostMessage::Transport {
                    source: "main".into(),
                    command: TransportCommand::Seek,
                    position: Some(61.0)
                }
            )
            .is_err()
        );
        assert!(
            validate_command(
                &full,
                &HostMessage::Transport {
                    source: "main".into(),
                    command: TransportCommand::Seek,
                    position: Some(f64::NAN)
                }
            )
            .is_err()
        );
    }

    #[test]
    fn undeclared_inbound_lists_are_ignored() {
        let empty = info(Vec::new());
        let mut sources = empty.sources.clone();
        let mut settings = empty
            .settings
            .iter()
            .map(|setting| setting.declaration.clone())
            .collect();
        let mut actions = empty.actions.clone();
        filter_description(&empty, &mut sources, &mut settings, &mut actions);
        assert!(sources.is_empty() && settings.is_empty() && actions.is_empty());
        let source_only = info(vec![Capability::Source]);
        let mut sources = source_only.sources.clone();
        filter_description(&source_only, &mut sources, &mut settings, &mut actions);
        assert_eq!(sources.len(), 1);
    }

    #[test]
    fn ambiguous_source_format_group_and_setting_ids_are_rejected() {
        let info = info(vec![Capability::Source]);
        assert!(validate_description(&info.sources, &[], &[]).is_ok());
        assert!(
            validate_description(
                &[info.sources[0].clone(), info.sources[0].clone()],
                &[],
                &[]
            )
            .is_err()
        );
        let mut source = info.sources[0].clone();
        source.formats.push(source.formats[0].clone());
        assert!(validate_description(&[source], &[], &[]).is_err());
        let mut source = info.sources[0].clone();
        let groups = source.groups.as_mut().unwrap();
        groups.push(groups[0].clone());
        assert!(validate_description(&[source], &[], &[]).is_err());
        let setting = info.settings[0].declaration.clone();
        assert!(validate_description(&[], &[setting.clone(), setting], &[]).is_err());
    }

    #[test]
    fn preferences_round_trip_and_leave_an_old_preferences_folder_untouched() {
        let dir = TestDir::new();
        fs::create_dir(dir.0.join("Settings")).unwrap();
        fs::write(dir.0.join("Settings/history"), "600").unwrap();
        assert_eq!(read_saved(&dir.0).unwrap(), SavedState::default());
        let saved = SavedState {
            addons: BTreeMap::from([(
                "com.example.test".into(),
                SavedAddon {
                    enabled: true,
                    settings: BTreeMap::from([
                        ("level".into(), json!(-6)),
                        ("mute".into(), json!(true)),
                    ]),
                    formats: BTreeMap::from([("main".into(), "5.1".into())]),
                    control: true,
                },
            )]),
        };
        write_saved(&dir.0, &saved).unwrap();
        assert_eq!(read_saved(&dir.0).unwrap(), saved);
        write_saved(&dir.0, &SavedState::default()).unwrap();
        assert_eq!(read_saved(&dir.0).unwrap(), SavedState::default());
        assert_eq!(
            fs::read_to_string(dir.0.join("Settings/history")).unwrap(),
            "600"
        );
    }

    fn test_package(root: &Path, id: &str, name: &str) -> PathBuf {
        let folder = root.join(name);
        fs::create_dir(&folder).unwrap();
        let mut manifest = info(Vec::new()).manifest.unwrap();
        manifest.id = id.into();
        fs::write(
            folder.join("addon.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(folder.join("test.exe"), b"package fixture").unwrap();
        let path = root.join(format!("{name}.btaddon"));
        package::pack(folder, &path).unwrap();
        path
    }

    #[test]
    fn install_refuses_replacement_after_review_even_with_the_same_manifest() {
        for use_proxy in [false, true] {
            let dir = TestDir::new();
            let original = test_package(&dir.0, "com.example.original", "original");
            let replacement = test_package(&dir.0, "com.example.original", "replacement");
            fs::write(dir.0.join("replacement/test.exe"), b"changed program").unwrap();
            fs::remove_file(&replacement).unwrap();
            package::pack(dir.0.join("replacement"), &replacement).unwrap();
            let manager = AddonManager::new(dir.0.clone(), "test").unwrap();
            let proxy = use_proxy.then(|| AddonManager::connect_existing(dir.0.clone()).unwrap());
            let handle = proxy.as_ref().unwrap_or(&manager).handle();
            let reviewed = handle
                .inspect(original.clone())
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap();
            fs::rename(replacement, &original).unwrap();
            let result = handle
                .install(original, reviewed.identity)
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            assert!(result.unwrap_err().contains("changed after it was checked"));
            assert!(!dir.0.join("addons/com.example.original").exists());
            assert!(!fs::read_dir(&dir.0).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".install-")
            }));
        }
    }

    #[test]
    fn reviewed_package_installs_and_cancelled_reviews_leave_no_private_copies() {
        for use_proxy in [false, true] {
            let dir = TestDir::new();
            let original = test_package(&dir.0, "com.example.original", "original");
            let manager = AddonManager::new(dir.0.clone(), "test").unwrap();
            let proxy = use_proxy.then(|| AddonManager::connect_existing(dir.0.clone()).unwrap());
            let handle = proxy.as_ref().unwrap_or(&manager).handle();
            for _ in 0..3 {
                let reviewed = handle
                    .inspect(original.clone())
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .unwrap();
                drop(reviewed);
                assert!(!fs::read_dir(&dir.0).unwrap().any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".install-")
                }));
            }
            let reviewed = handle
                .inspect(original.clone())
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap();
            let installed = handle
                .install(original, reviewed.identity)
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap();
            assert_eq!(installed.manifest.id, reviewed.info.manifest.id);
            assert_eq!(
                fs::read(dir.0.join("addons/com.example.original/test.exe")).unwrap(),
                b"package fixture"
            );
        }
    }

    #[test]
    fn install_uses_inspected_copy_when_the_selected_download_is_replaced() {
        let dir = TestDir::new();
        let original = test_package(&dir.0, "com.example.original", "original");
        let replacement = test_package(&dir.0, "com.example.replacement", "replacement");
        let prepared = prepare_package(&dir.0, &original).unwrap();
        let private_path = prepared.path.clone();
        fs::rename(replacement, &original).unwrap();
        let installed = package::install(&prepared.path, dir.0.join("addons")).unwrap();
        assert_eq!(installed.manifest.id, prepared.info.manifest.id);
        assert_eq!(installed.manifest.id, "com.example.original");
        assert!(!dir.0.join("addons/com.example.replacement").exists());
        drop(prepared);
        assert!(!private_path.exists());
    }

    #[test]
    fn failed_data_removal_does_not_leave_a_removed_program_listed_as_installed() {
        let dir = TestDir::new();
        let path = test_package(&dir.0, "com.example.test", "package");
        package::install(path, dir.0.join("addons")).unwrap();
        fs::create_dir(dir.0.join("addon-data")).unwrap();
        // A malformed data target produces a real removal failure after the
        // program folder has been removed successfully.
        fs::write(dir.0.join("addon-data/com.example.test"), b"not a folder").unwrap();
        let manager = AddonManager::new(dir.0.clone(), "test").unwrap();
        let handle = manager.handle();
        assert!(
            handle
                .remove("com.example.test", true)
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .is_err()
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        while handle.snapshot().addon("com.example.test").is_some() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.snapshot().addon("com.example.test").is_none());
        assert!(!dir.0.join("addons/com.example.test").exists());
        assert!(dir.0.join("addon-data/com.example.test").exists());
    }

    #[test]
    fn snapshot_reads_never_wait_on_publisher_and_sources_require_running() {
        let shared = Arc::new(Shared::default());
        let (commands, _receive) = mpsc::channel();
        let handle = AddonHandle {
            commands,
            shared: shared.clone(),
        };
        let guard = shared.lock_published();
        assert!(handle.try_snapshot().is_none());
        assert!(handle.try_appearances().is_none());
        drop(guard);
        assert!(handle.try_snapshot().is_some());
        assert!(handle.try_appearances().is_some());
        let mut snapshot = AddonSnapshot {
            addons: vec![info(vec![Capability::Source])],
            ..Default::default()
        };
        assert!(snapshot.source("com.example.test", "main").is_some());
        snapshot.addons[0].state = AddonState::Stopped;
        assert!(snapshot.source("com.example.test", "main").is_none());
    }

    #[test]
    fn broken_installation_is_visible_and_cannot_be_enabled() {
        let dir = TestDir::new();
        fs::create_dir_all(dir.0.join("addons/com.example.broken")).unwrap();
        let manager = AddonManager::new(dir.0.clone(), "test").unwrap();
        let handle = manager.handle();
        let reply = handle
            .set_enabled("com.example.broken", true)
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(reply.unwrap_err().contains("broken package"));
        let snapshot = handle.snapshot();
        assert!(matches!(
            snapshot.addon("com.example.broken").unwrap().state,
            AddonState::BrokenPackage(_)
        ));
        assert!(!dir.0.join("addon-data").exists());
    }
}

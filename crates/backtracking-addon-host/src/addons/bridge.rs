//! Ported from desktop/windows/src/addons/bridge.rs at e9a4efc0d2918bccadeecf2c7e586b84ddd5a4f4.
//! Shares the primary process's add-on host with additional capture windows.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;

const ENDPOINT_FILE: &str = "addon-host-endpoint-v1.json";
const MAX_FRAME: usize = 8 * 1024 * 1024;
const MAX_CLIENTS: usize = 32;
const MAX_PROXY_COMMANDS: usize = 8;
const NETWORK_TIMEOUT: Duration = Duration::from_secs(2);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Endpoint {
    version: u32,
    port: u16,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    command: WireCommand,
}

#[derive(Serialize, Deserialize)]
enum WireCommand {
    Snapshot,
    Positions { after: u64 },
    Inspect(PathBuf),
    Install(PathBuf, [u8; 32]),
    Remove { id: String, delete_data: bool },
    Enabled { id: String, enabled: bool },
    Start(String),
    Stop(String),
    Send { id: String, message: HostMessage },
    Clips(HostMessage),
    ControlAllowed { id: String, allowed: bool },
}

#[derive(Serialize, Deserialize)]
enum ResponseData {
    Snapshot(WireSnapshot),
    Positions {
        cursor: u64,
        messages: Vec<AddonPositions>,
    },
    Package(WirePackage),
    Inspected(WirePackage, [u8; 32]),
    Done,
}

type Response = Result<ResponseData, String>;

#[derive(Serialize, Deserialize)]
struct WireSnapshot {
    #[serde(flatten)]
    snapshot: AddonSnapshot,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    appearances: AddonAppearances,
}

#[derive(Serialize, Deserialize)]
struct WirePackage {
    manifest: Manifest,
    unpacked_size: u64,
    entry_count: usize,
    file_count: usize,
    program: String,
}

impl From<PackageInfo> for WirePackage {
    fn from(info: PackageInfo) -> Self {
        Self {
            manifest: info.manifest,
            unpacked_size: info.unpacked_size,
            entry_count: info.entry_count,
            file_count: info.file_count,
            program: info.program,
        }
    }
}

impl From<WirePackage> for PackageInfo {
    fn from(info: WirePackage) -> Self {
        Self {
            manifest: info.manifest,
            unpacked_size: info.unpacked_size,
            entry_count: info.entry_count,
            file_count: info.file_count,
            program: info.program,
        }
    }
}

pub(super) struct Bridge {
    root: PathBuf,
    endpoint: Endpoint,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Bridge {
    pub(super) fn start(root: &Path, handle: AddonHandle) -> Result<Self, String> {
        ensure_no_primary(root)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let endpoint = Endpoint {
            version: 1,
            port: listener.local_addr().map_err(|e| e.to_string())?.port(),
            token: random_token()?,
        };
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = stopping.clone();
        let worker_token = endpoint.token.clone();
        let worker = thread::Builder::new()
            .name("add-on host bridge".into())
            .spawn(move || serve(listener, handle, worker_token, worker_stopping))
            .map_err(|e| e.to_string())?;
        let bridge = Self {
            root: root.to_owned(),
            endpoint,
            stopping,
            worker: Some(worker),
        };
        write_endpoint(root, &bridge.endpoint)?;
        Ok(bridge)
    }
}

pub(super) fn ensure_no_primary(root: &Path) -> Result<(), String> {
    if read_endpoint(root)
        .and_then(|endpoint| exchange(&endpoint, WireCommand::Snapshot, None))
        .is_ok()
    {
        Err("An add-on host is already running for this data folder".into())
    } else {
        Ok(())
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if read_endpoint(&self.root).as_ref() == Ok(&self.endpoint) {
            let _ = fs::remove_file(self.root.join(ENDPOINT_FILE));
        }
    }
}

/// A proxy owns no add-on process and never sends host shutdown over the bridge.
pub(super) fn connect(root: PathBuf) -> Result<AddonManager, String> {
    if !root.is_absolute() {
        return Err("The add-on data root must be absolute".into());
    }
    let WireSnapshot {
        snapshot,
        appearances,
    } = remote_snapshot(&root, None)?;
    let (commands, receiver) = mpsc::channel();
    let shared = Arc::new(Shared {
        published: Mutex::new(Published {
            snapshot: Arc::new(snapshot),
            appearances: Arc::new(appearances),
        }),
        ..Shared::default()
    });
    let worker_shared = shared.clone();
    let worker = thread::Builder::new()
        .name("add-on host proxy".into())
        .spawn(move || proxy(root, receiver, worker_shared))
        .map_err(|e| e.to_string())?;
    Ok(AddonManager {
        handle: AddonHandle { commands, shared },
        worker: Some(worker),
        bridge: None,
    })
}

fn serve(listener: TcpListener, handle: AddonHandle, token: String, stopping: Arc<AtomicBool>) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut clients: Vec<JoinHandle<()>> = Vec::new();
    while !stopping.load(Ordering::Acquire) {
        reap(&mut clients);
        match listener.accept() {
            Ok((stream, address)) => {
                if !address.ip().is_loopback() {
                    continue;
                }
                // Windows accepts inherit the listener's nonblocking mode.
                // Client workers use blocking reads with short bounded timeouts.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                if active.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
                    active.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                let active_slot = Active(active.clone());
                let handle = handle.clone();
                let token = token.clone();
                let stopping = stopping.clone();
                if let Ok(worker) = thread::Builder::new()
                    .name("add-on bridge request".into())
                    .spawn(move || {
                        let _slot = active_slot;
                        serve_client(stream, &handle, &token, &stopping);
                    })
                {
                    clients.push(worker);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
    for client in clients {
        let _ = client.join();
    }
}

struct Active(Arc<AtomicUsize>);

impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve_client(mut stream: TcpStream, handle: &AddonHandle, token: &str, stopping: &AtomicBool) {
    let response = (|| {
        let request: Request = read_json(&mut stream, NETWORK_TIMEOUT, Some(stopping))?;
        if !same_token(&request.token, token) {
            return Err("The add-on host bridge rejected authentication".into());
        }
        dispatch(handle, request.command, stopping)
    })();
    let _ = write_json(&mut stream, &response, NETWORK_TIMEOUT, Some(stopping));
}

fn dispatch(handle: &AddonHandle, command: WireCommand, stopping: &AtomicBool) -> Response {
    match command {
        WireCommand::Snapshot => {
            let (snapshot, appearances) = {
                let current = handle.shared.lock_published();
                (
                    Arc::clone(&current.snapshot),
                    Arc::clone(&current.appearances),
                )
            };
            Ok(ResponseData::Snapshot(WireSnapshot {
                snapshot: snapshot.as_ref().clone(),
                appearances: appearances.as_ref().clone(),
            }))
        }
        WireCommand::Positions { after } => {
            let mut cursor = after;
            let messages = handle
                .positions_since(&mut cursor)
                .into_iter()
                .map(|message| (*message).clone())
                .collect();
            Ok(ResponseData::Positions { cursor, messages })
        }
        WireCommand::Inspect(path) => await_reply(handle.inspect(path), stopping)
            .map(|reviewed| ResponseData::Inspected(reviewed.info.into(), reviewed.identity)),
        WireCommand::Install(path, identity) => {
            await_reply(handle.install(path, identity), stopping)
                .map(|info| ResponseData::Package(info.into()))
        }
        WireCommand::Remove { id, delete_data } => {
            await_reply(handle.remove(&id, delete_data), stopping).map(|()| ResponseData::Done)
        }
        WireCommand::Enabled { id, enabled } => {
            await_reply(handle.set_enabled(&id, enabled), stopping).map(|()| ResponseData::Done)
        }
        WireCommand::Start(id) => {
            await_reply(handle.start(&id), stopping).map(|()| ResponseData::Done)
        }
        WireCommand::Stop(id) => {
            await_reply(handle.stop(&id), stopping).map(|()| ResponseData::Done)
        }
        WireCommand::Send { id, message } => {
            if !matches!(
                message,
                HostMessage::SetSetting { .. }
                    | HostMessage::RunAction { .. }
                    | HostMessage::OpenFile { .. }
                    | HostMessage::SelectFormat { .. }
                    | HostMessage::Transport { .. }
            ) {
                return Err("This command is not available through the add-on bridge".into());
            }
            message.validate().map_err(|e| e.to_string())?;
            await_reply(
                handle.request(|reply| Command::Send(id, message, reply)),
                stopping,
            )
            .map(|()| ResponseData::Done)
        }
        WireCommand::Clips(message) => {
            if !matches!(message, HostMessage::ClipExported { .. }) {
                return Err("Expected a clip export notification".into());
            }
            message.validate().map_err(|e| e.to_string())?;
            await_reply(
                handle.request(|reply| Command::Clips(message, reply)),
                stopping,
            )
            .map(|()| ResponseData::Done)
        }
        WireCommand::ControlAllowed { id, allowed } => {
            await_reply(handle.set_control_allowed(&id, allowed), stopping)
                .map(|()| ResponseData::Done)
        }
    }
}

fn await_reply<T>(reply: AddonReply<T>, stopping: &AtomicBool) -> Result<T, String> {
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        if stopping.load(Ordering::Acquire) {
            return Err("The primary add-on host is closing".into());
        }
        if Instant::now() >= deadline {
            return Err(
                "The primary add-on host timed out; the operation may still complete".into(),
            );
        }
        match reply.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("The primary add-on host has stopped".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn proxy(root: PathBuf, receiver: mpsc::Receiver<Command>, shared: Arc<Shared>) {
    let stopping = Arc::new(AtomicBool::new(false));
    let mut pending: Vec<JoinHandle<()>> = Vec::new();
    let mut queued = VecDeque::new();
    let mut refresh = Instant::now();
    let mut position_cursor = 0;
    let mut position_primary = None;
    loop {
        reap(&mut pending);
        let mut wait = refresh.saturating_duration_since(Instant::now());
        if !pending.is_empty() || !queued.is_empty() {
            wait = wait.min(Duration::from_millis(10));
        }
        match receiver.recv_timeout(wait) {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(command) => {
                if pending.len() + queued.len() >= MAX_PROXY_COMMANDS {
                    reject(command, "Too many add-on commands are pending".into());
                } else {
                    queued.push_back(command);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // Commands keep the handle's FIFO order. Only their socket work runs
        // separately, so an install never blocks snapshot polling or repaints.
        if pending.is_empty() {
            if let Some(command) = queued.pop_front() {
                let root = root.clone();
                let stopping = stopping.clone();
                let command = Arc::new(Mutex::new(Some(command)));
                let worker_command = command.clone();
                match thread::Builder::new()
                    .name("add-on proxy request".into())
                    .spawn(move || {
                        if let Some(command) = worker_command.lock().unwrap().take() {
                            forward(&root, command, &stopping);
                        }
                    }) {
                    Ok(worker) => pending.push(worker),
                    Err(error) => {
                        if let Some(command) = command.lock().unwrap().take() {
                            reject(command, error.to_string());
                        }
                    }
                }
            }
        }
        if Instant::now() >= refresh {
            match remote_snapshot(&root, Some(&stopping)) {
                Ok(WireSnapshot {
                    snapshot,
                    appearances,
                }) => {
                    // The remote revision can reset when the primary restarts.
                    shared.publish(
                        snapshot.addons,
                        snapshot.error.as_deref(),
                        appearances.iter(),
                    );
                    if let Ok((cursor, messages)) = remote_positions(
                        &root,
                        position_cursor,
                        &mut position_primary,
                        Some(&stopping),
                    ) {
                        position_cursor = cursor;
                        let mut journal =
                            shared.positions.lock().unwrap_or_else(|e| e.into_inner());
                        for message in messages {
                            journal.push(message);
                        }
                    }
                }
                Err(error) => {
                    let mut addons = shared.lock_published().snapshot.addons.clone();
                    let reason = format!("Cannot reach the primary add-on host: {error}");
                    for addon in &mut addons {
                        addon.state = AddonState::Stopped;
                        addon.pid = None;
                        addon.sources.clear();
                        addon.settings.clear();
                        addon.actions.clear();
                    }
                    shared.publish(addons, Some(&reason), std::iter::empty());
                }
            }
            refresh = Instant::now() + SNAPSHOT_INTERVAL;
        }
    }
    stopping.store(true, Ordering::Release);
    for command in queued {
        reject(command, "The add-on host proxy is closing".into());
    }
    for worker in pending {
        let _ = worker.join();
    }
}

fn forward(root: &Path, command: Command, stopping: &AtomicBool) {
    match command {
        Command::Inspect(path, reply) => {
            let result = read_endpoint(root).and_then(|endpoint| {
                exchange(&endpoint, WireCommand::Inspect(path), Some(stopping))
            });
            let _ = reply.send(result.and_then(|response| match response {
                ResponseData::Inspected(info, identity) => Ok(InspectedPackage {
                    info: info.into(),
                    identity,
                }),
                _ => Err("The primary add-on host returned an unexpected reply".into()),
            }));
        }
        Command::Install(path, identity, reply) => {
            let _ = reply.send(request_package(
                root,
                WireCommand::Install(path, identity),
                stopping,
            ));
        }
        Command::Remove(id, delete_data, reply) => {
            let _ = reply.send(request_done(
                root,
                WireCommand::Remove { id, delete_data },
                stopping,
            ));
        }
        Command::Enabled(id, enabled, reply) => {
            let _ = reply.send(request_done(
                root,
                WireCommand::Enabled { id, enabled },
                stopping,
            ));
        }
        Command::Start(id, reply) => {
            let _ = reply.send(request_done(root, WireCommand::Start(id), stopping));
        }
        Command::Stop(id, reply) => {
            let _ = reply.send(request_done(root, WireCommand::Stop(id), stopping));
        }
        Command::Send(id, message, reply) => {
            let _ = reply.send(request_done(
                root,
                WireCommand::Send { id, message },
                stopping,
            ));
        }
        Command::Clips(message, reply) => {
            let _ = reply.send(request_done(root, WireCommand::Clips(message), stopping));
        }
        Command::Error(error, reply) => {
            let _ = reply.send(Err(error));
        }
        Command::ControlAllowed(id, allowed, reply) => {
            let _ = reply.send(request_done(
                root,
                WireCommand::ControlAllowed { id, allowed },
                stopping,
            ));
        }
        // Control requests are carried out by the primary window, which answers them itself.
        Command::ControlReply { .. } | Command::Shutdown => {}
    }
}

fn reject(command: Command, error: String) {
    match command {
        Command::Inspect(_, reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Install(_, _, reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Remove(_, _, reply)
        | Command::Enabled(_, _, reply)
        | Command::Start(_, reply)
        | Command::Stop(_, reply)
        | Command::Send(_, _, reply)
        | Command::Clips(_, reply)
        | Command::Error(_, reply)
        | Command::ControlAllowed(_, _, reply) => {
            let _ = reply.send(Err(error));
        }
        Command::ControlReply { .. } | Command::Shutdown => {}
    }
}

fn request_package(
    root: &Path,
    command: WireCommand,
    stopping: &AtomicBool,
) -> Result<PackageInfo, String> {
    match exchange(&read_endpoint(root)?, command, Some(stopping))? {
        ResponseData::Package(info) => Ok(info.into()),
        _ => Err("The primary add-on host returned an unexpected reply".into()),
    }
}

fn request_done(root: &Path, command: WireCommand, stopping: &AtomicBool) -> Result<(), String> {
    match exchange(&read_endpoint(root)?, command, Some(stopping))? {
        ResponseData::Done => Ok(()),
        _ => Err("The primary add-on host returned an unexpected reply".into()),
    }
}

fn remote_snapshot(root: &Path, stopping: Option<&AtomicBool>) -> Result<WireSnapshot, String> {
    match exchange(&read_endpoint(root)?, WireCommand::Snapshot, stopping)? {
        ResponseData::Snapshot(snapshot) => Ok(snapshot),
        _ => Err("The primary add-on host returned an unexpected snapshot".into()),
    }
}

fn remote_positions(
    root: &Path,
    mut after: u64,
    primary: &mut Option<String>,
    stopping: Option<&AtomicBool>,
) -> Result<(u64, Vec<AddonPositions>), String> {
    let endpoint = read_endpoint(root)?;
    if primary.as_deref() != Some(endpoint.token.as_str()) {
        after = 0;
    }
    match exchange(&endpoint, WireCommand::Positions { after }, stopping)? {
        ResponseData::Positions { cursor, messages } => {
            *primary = Some(endpoint.token);
            Ok((cursor, messages))
        }
        _ => Err("The primary add-on host returned unexpected positions".into()),
    }
}

fn exchange(endpoint: &Endpoint, command: WireCommand, stopping: Option<&AtomicBool>) -> Response {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .map_err(|e| e.to_string())?;
    let timeout = if matches!(
        command,
        WireCommand::Snapshot | WireCommand::Positions { .. }
    ) {
        NETWORK_TIMEOUT
    } else {
        COMMAND_TIMEOUT
    };
    write_json(
        &mut stream,
        &Request {
            token: endpoint.token.clone(),
            command,
        },
        NETWORK_TIMEOUT,
        stopping,
    )?;
    read_json::<Response>(&mut stream, timeout, stopping)?
}

fn reap(workers: &mut Vec<JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let _ = workers.swap_remove(index).join();
        } else {
            index += 1;
        }
    }
}

fn same_token(given: &str, expected: &str) -> bool {
    if given.len() != expected.len() {
        return false;
    }
    given
        .bytes()
        .zip(expected.bytes())
        .fold(0_u8, |different, (a, b)| different | (a ^ b))
        == 0
}

fn read_endpoint(root: &Path) -> Result<Endpoint, String> {
    let file = File::open(root.join(ENDPOINT_FILE))
        .map_err(|e| format!("The primary add-on host is not available: {e}"))?;
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("The add-on host endpoint file is too large".into());
    }
    let endpoint: Endpoint = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if endpoint.version != 1
        || endpoint.port == 0
        || endpoint.token.len() != 64
        || !endpoint
            .token
            .bytes()
            .all(|value| value.is_ascii_hexdigit())
    {
        return Err("The add-on host endpoint file is invalid".into());
    }
    Ok(endpoint)
}

fn write_endpoint(root: &Path, endpoint: &Endpoint) -> Result<(), String> {
    let temporary = root.join(format!(".{ENDPOINT_FILE}-{}.tmp", random_token()?));
    let bytes = serde_json::to_vec(endpoint).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .mode(0o600)
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, root.join(ENDPOINT_FILE))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("Cannot publish the add-on host endpoint: {e}"))
}

fn read_json<T: serde::de::DeserializeOwned>(
    stream: &mut TcpStream,
    timeout: Duration,
    stopping: Option<&AtomicBool>,
) -> Result<T, String> {
    let deadline = Instant::now() + timeout;
    let mut header = [0_u8; 4];
    read_bytes(stream, &mut header, deadline, stopping)?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err("The add-on bridge message exceeds its size limit".into());
    }
    let mut bytes = vec![0; length];
    read_bytes(stream, &mut bytes, deadline, stopping)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid add-on bridge message: {e}"))
}

fn write_json<T: Serialize>(
    stream: &mut TcpStream,
    value: &T,
    timeout: Duration,
    stopping: Option<&AtomicBool>,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME {
        return Err("The add-on bridge message exceeds its size limit".into());
    }
    let deadline = Instant::now() + timeout;
    write_bytes(
        stream,
        &(bytes.len() as u32).to_le_bytes(),
        deadline,
        stopping,
    )?;
    write_bytes(stream, &bytes, deadline, stopping)
}

fn remaining(deadline: Instant, stopping: Option<&AtomicBool>) -> Result<Duration, String> {
    if stopping.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err("The add-on bridge is closing".into());
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("The add-on bridge request timed out; the operation may still complete".into());
    }
    Ok(remaining.min(Duration::from_millis(100)))
}

fn read_bytes(
    stream: &mut TcpStream,
    mut bytes: &mut [u8],
    deadline: Instant,
    stopping: Option<&AtomicBool>,
) -> Result<(), String> {
    while !bytes.is_empty() {
        stream
            .set_read_timeout(Some(remaining(deadline, stopping)?))
            .map_err(|e| e.to_string())?;
        match stream.read(bytes) {
            Ok(0) => return Err("The primary add-on host closed the connection".into()),
            Ok(count) => bytes = &mut bytes[count..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn write_bytes(
    stream: &mut TcpStream,
    mut bytes: &[u8],
    deadline: Instant,
    stopping: Option<&AtomicBool>,
) -> Result<(), String> {
    while !bytes.is_empty() {
        stream
            .set_write_timeout(Some(remaining(deadline, stopping)?))
            .map_err(|e| e.to_string())?;
        match stream.write(bytes) {
            Ok(0) => return Err("The add-on bridge could not send its message".into()),
            Ok(count) => bytes = &bytes[count..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("backtracking-bridge-{}", random_token().unwrap()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for(handle: &AddonHandle, predicate: impl Fn(&AddonSnapshot) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if predicate(&handle.snapshot()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for proxy state"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn fake_host() -> (AddonHandle, mpsc::Receiver<Command>) {
        let (commands, receiver) = mpsc::channel();
        let source = Source {
            id: "main".into(),
            name: "Test source".into(),
            sender: "abc-123".into(),
            formats: vec![backtracking_addon::protocol::Format {
                id: "stereo".into(),
                label: "Stereo".into(),
            }],
            format: "stereo".into(),
            groups: None,
            transport: None,
        };
        let snapshot = AddonSnapshot {
            revision: 8,
            error: None,
            addons: vec![AddonInfo {
                id: "com.example.test".into(),
                manifest: None,
                enabled: true,
                state: AddonState::Running,
                pid: Some(123),
                message: None,
                sources: vec![source],
                settings: Vec::new(),
                actions: Vec::new(),
                control_allowed: false,
                control_asked: false,
            }],
        };
        let shared = Arc::new(Shared {
            published: Mutex::new(Published {
                snapshot: Arc::new(snapshot),
                ..Published::default()
            }),
            ..Shared::default()
        });
        (AddonHandle { commands, shared }, receiver)
    }

    #[test]
    fn proxy_delivers_positions_to_independent_readers_without_changing_snapshot() {
        let root = Directory::new();
        let (primary_handle, _commands) = fake_host();
        let primary = Bridge::start(&root.0, primary_handle.clone()).unwrap();
        let secondary = connect(root.0.clone()).unwrap();
        let revision = secondary.handle().snapshot().revision;
        primary_handle
            .shared
            .positions
            .lock()
            .unwrap()
            .push(AddonPositions {
                sequence: 0,
                addon_id: "com.example.test".into(),
                source_id: "main".into(),
                time: 123_000_000,
                channels: vec![ChannelPosition::new(2, -0.5, 0.8, 0.0)],
            });
        let handle = secondary.handle();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut a = 0;
        let first = loop {
            let messages = handle.positions_since(&mut a);
            if !messages.is_empty() {
                break messages;
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for bridged positions"
            );
            thread::sleep(Duration::from_millis(10));
        };
        let mut b = 0;
        let second = handle.positions_since(&mut b);
        assert_eq!(first, second);
        assert_eq!(first[0].time, 123_000_000);
        assert_eq!(first[0].channels[0].channel, 2);
        assert_eq!(handle.snapshot().revision, revision);
        assert!(handle.positions_since(&mut a).is_empty());
        let mut primary_cursor = 0;
        assert_eq!(primary_handle.positions_since(&mut primary_cursor).len(), 1);
        drop(primary);
        let (restarted_handle, _restarted_commands) = fake_host();
        restarted_handle
            .shared
            .positions
            .lock()
            .unwrap()
            .push(AddonPositions {
                sequence: 0,
                addon_id: "com.example.test".into(),
                source_id: "main".into(),
                time: 456_000_000,
                channels: vec![ChannelPosition::new(2, 0.5, 0.8, 0.0)],
            });
        let _restarted = Bridge::start(&root.0, restarted_handle).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let messages = handle.positions_since(&mut a);
            if let Some(message) = messages.last() {
                assert_eq!(message.time, 456_000_000);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for restarted host's positions"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn proxy_reads_snapshots_forwards_commands_and_does_not_stop_primary() {
        let root = Directory::new();
        let broken = root.0.join("addons/com.example.broken");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join("addon.json"), b"not json").unwrap();
        let primary = AddonManager::new(root.0.clone(), "test").unwrap();
        let secondary = connect(root.0.clone()).unwrap();
        let handle = secondary.handle();
        wait_for(&handle, |snapshot| {
            snapshot.addon("com.example.broken").is_some()
        });
        assert!(
            handle
                .install(root.0.join("missing.btaddon"), [0; 32])
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .is_err()
        );
        handle
            .remove("com.example.broken", false)
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        wait_for(&handle, |snapshot| snapshot.addons.is_empty());
        assert!(!broken.exists());
        drop(secondary);
        assert!(remote_snapshot(&root.0, None).is_ok());
        drop(primary);
        assert!(!root.0.join(ENDPOINT_FILE).exists());
    }

    #[test]
    fn proxy_reports_primary_disappearance_and_clears_running_state() {
        let root = Directory::new();
        let (primary_handle, _commands) = fake_host();
        let primary = Bridge::start(&root.0, primary_handle.clone()).unwrap();
        let secondary = connect(root.0.clone()).unwrap();
        assert_eq!(
            secondary.handle().snapshot().addons[0].state,
            AddonState::Running
        );
        let running_revision = secondary.handle().snapshot().revision;
        drop(primary);
        wait_for(&secondary.handle(), |snapshot| snapshot.error.is_some());
        let offline_revision = secondary.handle().snapshot().revision;
        assert!(offline_revision > running_revision);
        assert!(
            secondary
                .handle()
                .snapshot()
                .addons
                .iter()
                .all(|addon| addon.state == AddonState::Stopped
                    && addon.pid.is_none()
                    && addon.sources.is_empty())
        );
        let _restarted = Bridge::start(&root.0, primary_handle).unwrap();
        wait_for(&secondary.handle(), |snapshot| {
            snapshot.error.is_none() && snapshot.addons[0].state == AddonState::Running
        });
        assert!(secondary.handle().snapshot().revision > offline_revision);
        assert_eq!(secondary.handle().snapshot().addons[0].sources.len(), 1);
    }

    #[test]
    fn proxy_keeps_command_order_while_snapshots_and_callbacks_continue() {
        let root = Directory::new();
        let (primary_handle, commands) = fake_host();
        let _primary = Bridge::start(&root.0, primary_handle.clone()).unwrap();
        let secondary = connect(root.0.clone()).unwrap();
        let handle = secondary.handle();
        let callbacks = Arc::new(AtomicUsize::new(0));
        let count = callbacks.clone();
        handle.subscribe(Arc::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
        }));
        let first = handle.set_enabled("com.example.test", false);
        let second = handle.set_enabled("com.example.test", true);
        let Command::Enabled(_, false, first_reply) =
            commands.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("Expected disable first");
        };
        assert!(commands.recv_timeout(Duration::from_millis(150)).is_err());
        let mut changed = primary_handle.snapshot().as_ref().clone();
        changed.addons[0].sources[0].name = "Updated source".into();
        primary_handle.shared.lock_published().snapshot = Arc::new(changed);
        wait_for(&handle, |snapshot| {
            snapshot.addons[0].sources[0].name == "Updated source"
        });
        // Callbacks run after the new snapshot is stored and its lock let go, so the
        // snapshot can be seen a moment before any callback has run.
        wait_for(&handle, |_| callbacks.load(Ordering::Relaxed) > 0);
        first_reply.send(Ok(())).unwrap();
        first.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        let Command::Enabled(_, true, second_reply) =
            commands.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("Expected enable second");
        };
        second_reply.send(Ok(())).unwrap();
        second
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
    }

    #[test]
    fn bridge_rejects_wrong_token_and_internal_protocol_messages() {
        let root = Directory::new();
        let _primary = AddonManager::new(root.0.clone(), "test").unwrap();
        let endpoint = read_endpoint(&root.0).unwrap();
        let mut wrong = endpoint.clone();
        wrong.token = "0".repeat(64);
        assert!(exchange(&wrong, WireCommand::Snapshot, None).is_err());
        assert!(
            exchange(
                &endpoint,
                WireCommand::Send {
                    id: "com.example.none".into(),
                    message: HostMessage::Quit
                },
                None
            )
            .is_err()
        );
        assert!(exchange(&endpoint, WireCommand::Clips(HostMessage::Quit), None).is_err());
    }

    #[test]
    fn bridge_rejects_oversized_frames_before_reading_payloads() {
        let root = Directory::new();
        let _primary = AddonManager::new(root.0.clone(), "test").unwrap();
        let endpoint = read_endpoint(&root.0).unwrap();
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, endpoint.port)).unwrap();
        socket
            .write_all(&((MAX_FRAME + 1) as u32).to_le_bytes())
            .unwrap();
        assert!(
            read_json::<Response>(&mut socket, NETWORK_TIMEOUT, None)
                .unwrap()
                .is_err()
        );
    }
}

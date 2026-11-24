//! Add-on mode: what Backtracking starts. One thread owns the connection to Backtracking
//! (the SDK's `Client` is synchronous); Tokio's threads serve MCP over HTTP and hand each
//! tool call to that thread, which sends it as a `control` request and routes the reply back.

use std::collections::{BTreeMap, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use backtracking_addon::client::Client;
use backtracking_addon::protocol::{
    control_codes, Action, AddonMessage, ControlError, HostMessage, MessageLevel, Setting,
    SettingKind, MAX_CONTROL_IN_FLIGHT,
};
use tokio::sync::oneshot;

use crate::bridge::{Bridge, Job, Outcome, STOPPING};
use crate::endpoint::{self, Endpoint, ENDPOINT_FILE};
use crate::http::Server;
use crate::platform;

/// How much longer than the host's own bound a request waits, as `Client::control` does.
pub const REPLY_MARGIN: Duration = Duration::from_secs(5);
/// The port the server asks for unless the user picks another one: fixed, so that a client
/// set up with the URL keeps working across restarts. Outside Windows' dynamic range.
pub const DEFAULT_PORT: u16 = 47615;
/// How long connection details with the token stay on the Add-ons page.
const DETAILS_FOR: Duration = Duration::from_secs(120);
/// How long a note about a button that was pressed stays.
const NOTE_FOR: Duration = Duration::from_secs(30);
/// The add-on's message is at most this long (contract section 4.2).
const MESSAGE_CHARS: usize = 300;

const ACTION_DETAILS: &str = "details";
const ACTION_COPY_CODE: &str = "copy-claude-code";
const ACTION_COPY_DESKTOP: &str = "copy-claude-desktop";
const ACTION_COPY_HTTP: &str = "copy-http";
const ACTION_NEW_TOKEN: &str = "new-token";
const PORT_SETTING: &str = "port";

pub fn run() -> Result<(), String> {
    let mut client =
        Client::connect_from_env(crate::ID, crate::VERSION).map_err(|error| error.to_string())?;
    let data = client.data_dir().to_path_buf();
    let log = Log::open(&data);
    // Backtracking starts add-ons as `...\bin/name.exe`; rebuilding the path from its
    // components gives the copied commands one kind of separator.
    let program: PathBuf = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .components()
        .collect();
    let token = Arc::new(RwLock::new(
        endpoint::load_or_make_token(&data).map_err(|error| format!("Keep the token: {error}"))?,
    ));
    client
        .send(&describe())
        .map_err(|error| error.to_string())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("backtracking-agent-http")
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let (bridge, jobs) = Bridge::new();
    let wanted = port_setting(client.settings().get(PORT_SETTING));
    let mut agent = Agent {
        client,
        endpoint_path: data.join(ENDPOINT_FILE),
        data,
        program,
        token,
        bridge,
        runtime,
        server: None,
        wanted_port: wanted,
        port_note: None,
        allowed: None,
        pending: BTreeMap::new(),
        queue: VecDeque::new(),
        next_id: 1,
        details_until: None,
        note: None,
        shown: None,
        log,
    };
    agent.log.line(&format!(
        "started, version {}, program {}",
        crate::VERSION,
        agent.program.display()
    ));
    agent.listen()?;
    let result = agent.serve(&jobs);
    agent.stop();
    result
}

struct Pending {
    reply: oneshot::Sender<Outcome>,
    deadline: Instant,
    name: &'static str,
}

struct Agent {
    client: Client,
    data: PathBuf,
    endpoint_path: PathBuf,
    program: PathBuf,
    token: Arc<RwLock<String>>,
    bridge: Bridge,
    runtime: tokio::runtime::Runtime,
    server: Option<Server>,
    /// The port in the user's setting; 0 is any free port.
    wanted_port: u16,
    /// Why the server is not on the wanted port.
    port_note: Option<String>,
    allowed: Option<bool>,
    pending: BTreeMap<String, Pending>,
    queue: VecDeque<Job>,
    next_id: u64,
    details_until: Option<Instant>,
    note: Option<(Instant, MessageLevel, String)>,
    /// The message last sent, so that it is sent only when it changes.
    shown: Option<(MessageLevel, String)>,
    log: Log,
}

impl Agent {
    /// Start (or move) the HTTP server to the wanted port, falling back to any free port when
    /// that one is taken, and publish the connection file.
    fn listen(&mut self) -> Result<(), String> {
        self.server = None;
        let (runtime, token, bridge) = (&self.runtime, &self.token, &self.bridge);
        let start = |port| Server::start(runtime, port, token.clone(), bridge.clone());
        let server = match start(self.wanted_port) {
            Ok(server) => {
                self.port_note = None;
                server
            }
            Err(error) if self.wanted_port != 0 => {
                let server = start(0).map_err(|error| format!("Serve on 127.0.0.1: {error}"))?;
                self.port_note = Some(format!(
                    "Port {} could not be used ({error}), so the server took port {} instead.",
                    self.wanted_port, server.port
                ));
                server
            }
            Err(error) => return Err(format!("Serve on 127.0.0.1: {error}")),
        };
        self.log.line(&format!("serving {}", server.url()));
        self.server = Some(server);
        self.publish();
        self.show();
        Ok(())
    }

    fn url(&self) -> String {
        self.server.as_ref().map(Server::url).unwrap_or_default()
    }

    fn publish(&mut self) {
        let endpoint = Endpoint {
            url: self.url(),
            token: self
                .token
                .read()
                .map(|token| token.clone())
                .unwrap_or_default(),
            pid: std::process::id(),
            program: self.program.clone(),
            version: crate::VERSION.into(),
        };
        if let Err(error) = endpoint.write(&self.endpoint_path) {
            self.log
                .line(&format!("could not write the connection file: {error}"));
            self.note(
                MessageLevel::Error,
                format!("Could not write the connection file: {error}"),
            );
        }
    }

    fn serve(&mut self, jobs: &Receiver<Job>) -> Result<(), String> {
        loop {
            loop {
                match self.client.try_recv() {
                    Ok(Some(HostMessage::Quit)) => {
                        self.log.line("asked to quit");
                        return Ok(());
                    }
                    Ok(Some(message)) => self.on_host(message),
                    Ok(None) => break,
                    Err(error) => return Err(error.to_string()),
                }
            }
            if self.client.is_disconnected() {
                self.log.line("Backtracking closed the connection");
                return Ok(());
            }
            self.expire();
            self.show();
            let wait = if self.pending.is_empty() {
                Duration::from_millis(10)
            } else {
                Duration::from_millis(2)
            };
            match jobs.recv_timeout(wait) {
                Ok(job) => {
                    self.queue.push_back(job);
                    self.queue.extend(jobs.try_iter());
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
            self.dispatch();
        }
    }

    fn on_host(&mut self, message: HostMessage) {
        match message {
            HostMessage::ControlAccess { allowed } => {
                self.log.line(&format!("control_access allowed={allowed}"));
                self.allowed = Some(allowed);
            }
            HostMessage::ControlReply(reply) => {
                let Some(id) = reply.id.clone() else {
                    return;
                };
                // A reply to a request that already timed out here is dropped.
                let Some(pending) = self.pending.remove(&id) else {
                    return;
                };
                let outcome = match reply.outcome() {
                    Ok(result) => Outcome::Done(result.clone()),
                    Err(error) => Outcome::Refused(error.clone()),
                };
                match &outcome {
                    Outcome::Done(_) => self.log.line(&format!("< {id} {} ok", pending.name)),
                    Outcome::Refused(error) => self.log.line(&format!(
                        "< {id} {} refused {}: {}",
                        pending.name, error.code, error.message
                    )),
                    Outcome::Lost(_) => {}
                }
                let _ = pending.reply.send(outcome);
            }
            HostMessage::SetSetting { key, value } if key == PORT_SETTING => {
                let wanted = port_setting(Some(&value));
                if wanted != self.wanted_port || self.port_note.is_some() {
                    self.wanted_port = wanted;
                    if let Err(error) = self.listen() {
                        self.note(MessageLevel::Error, error);
                    }
                }
            }
            HostMessage::RunAction { id } => self.action(&id),
            _ => {}
        }
    }

    fn action(&mut self, id: &str) {
        let program = self.program.display().to_string();
        match id {
            ACTION_DETAILS => {
                self.details_until = Some(Instant::now() + DETAILS_FOR);
                self.note = None;
            }
            ACTION_COPY_CODE => {
                let line = claude_code_command(&program);
                self.copy(&line, format!("Copied for a terminal: {line}"));
            }
            ACTION_COPY_DESKTOP => {
                let entry = claude_desktop_entry(&program);
                self.copy(
                    &entry,
                    "Copied the mcpServers entry for claude_desktop_config.json (Claude Desktop: \
                     Settings > Developer > Edit Config)."
                        .into(),
                );
            }
            ACTION_COPY_HTTP => {
                let token = self
                    .token
                    .read()
                    .map(|token| token.clone())
                    .unwrap_or_default();
                let line = format!(
                    "claude mcp add --transport http backtracking {} --header \"Authorization: \
                     Bearer {token}\"",
                    self.url()
                );
                self.copy(
                    &line,
                    "Copied a Claude Code command for the HTTP address, with the token. Keep it \
                     private."
                        .into(),
                );
            }
            ACTION_NEW_TOKEN => match endpoint::renew_token(&self.data) {
                Ok(token) => {
                    if let Ok(mut current) = self.token.write() {
                        *current = token;
                    }
                    self.publish();
                    self.details_until = None;
                    self.log.line("made a new token");
                    self.note(
                        MessageLevel::Info,
                        "Made a new token. HTTP clients set up with the old one need the new one; \
                         clients started with --stdio pick it up by themselves."
                            .into(),
                    );
                }
                Err(error) => self.note(MessageLevel::Error, format!("No new token: {error}")),
            },
            _ => {}
        }
    }

    fn copy(&mut self, text: &str, done: String) {
        match platform::copy_to_clipboard(text) {
            Ok(()) => self.note(MessageLevel::Info, done),
            Err(error) => self.note(MessageLevel::Error, format!("Could not copy: {error}")),
        }
    }

    fn note(&mut self, level: MessageLevel, text: String) {
        self.note = Some((Instant::now() + NOTE_FOR, level, text));
        self.show();
    }

    /// Send the Add-ons page message when it changed.
    fn show(&mut self) {
        let now = Instant::now();
        if self.details_until.is_some_and(|until| until <= now) {
            self.details_until = None;
        }
        if self
            .note
            .as_ref()
            .is_some_and(|(until, _, _)| *until <= now)
        {
            self.note = None;
        }
        let token = self
            .token
            .read()
            .map(|token| token.clone())
            .unwrap_or_default();
        let wanted = status(&Status {
            url: &self.url(),
            program: &self.program,
            allowed: self.allowed,
            port_note: self.port_note.as_deref(),
            details_token: self.details_until.map(|_| token.as_str()),
            note: self
                .note
                .as_ref()
                .map(|(_, level, text)| (*level, text.as_str())),
        });
        if self.shown.as_ref() != Some(&wanted) {
            let (level, text) = wanted.clone();
            if self
                .client
                .send(&AddonMessage::Message { level, text })
                .is_ok()
            {
                self.shown = Some(wanted);
            }
        }
    }

    fn dispatch(&mut self) {
        while self.pending.len() < MAX_CONTROL_IN_FLIGHT {
            let Some(job) = self.queue.pop_front() else {
                return;
            };
            let id = format!("m{}", self.next_id);
            self.next_id += 1;
            let name = job.command.name();
            let wire = serde_json::to_string(&job.command).unwrap_or_default();
            // Sent as a plain message with ids of this add-on's own, so that every reply
            // comes back through try_recv and nothing here blocks on one.
            match self.client.send(&AddonMessage::Control {
                id: id.clone(),
                command: job.command.clone(),
            }) {
                Ok(()) => {
                    self.log.line(&format!("> {id} {wire}"));
                    self.pending.insert(
                        id,
                        Pending {
                            reply: job.reply,
                            deadline: Instant::now() + job.command.reply_within() + REPLY_MARGIN,
                            name,
                        },
                    );
                }
                Err(error) => {
                    let _ = job.reply.send(Outcome::Refused(ControlError::new(
                        control_codes::INVALID_REQUEST,
                        error.to_string(),
                    )));
                }
            }
        }
    }

    fn expire(&mut self) {
        let now = Instant::now();
        let late: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in late {
            if let Some(pending) = self.pending.remove(&id) {
                self.log
                    .line(&format!("< {id} {} no reply in time", pending.name));
                let _ = pending.reply.send(Outcome::Lost(
                    "Backtracking did not answer in time. Call get_state to see whether it \
                     happened."
                        .into(),
                ));
            }
        }
    }

    fn stop(&mut self) {
        for (_, pending) in std::mem::take(&mut self.pending) {
            let _ = pending.reply.send(Outcome::Lost(STOPPING.into()));
        }
        for job in self.queue.drain(..) {
            let _ = job.reply.send(Outcome::Lost(STOPPING.into()));
        }
        let _ = std::fs::remove_file(&self.endpoint_path);
        self.server = None;
        self.log.line("stopped");
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        // Covers an early error return as well.
        let _ = std::fs::remove_file(&self.endpoint_path);
    }
}

/// What the Add-ons page message is made from.
struct Status<'a> {
    url: &'a str,
    program: &'a Path,
    allowed: Option<bool>,
    port_note: Option<&'a str>,
    /// The token, while the user asked to see the connection details.
    details_token: Option<&'a str>,
    note: Option<(MessageLevel, &'a str)>,
}

/// The one line on the Add-ons page. The token appears only while the user asked for it.
fn status(status: &Status<'_>) -> (MessageLevel, String) {
    let (level, text) = if let Some(token) = status.details_token {
        (
            MessageLevel::Info,
            format!(
                "Address {} with the header Authorization: Bearer {token} . Clients started with \
                 --stdio need neither. Shown for 2 minutes.",
                status.url
            ),
        )
    } else if let Some((level, text)) = status.note {
        (level, text.to_owned())
    } else if let Some(note) = status.port_note {
        (
            MessageLevel::Warning,
            format!("{note} MCP server at {}.", status.url),
        )
    } else if status.allowed != Some(true) {
        (
            MessageLevel::Warning,
            format!(
                "MCP server running at {}. Clients can connect but cannot act until you switch \
                 on \"Let this add-on control Backtracking\" below.",
                status.url
            ),
        )
    } else {
        let full = format!(
            "MCP server running at {}. To connect a client, press Copy Claude Code command, or \
             have it start: \"{}\" --stdio",
            status.url,
            status.program.display()
        );
        // A long program path would be cut off; the buttons carry it whole.
        let text = if full.chars().count() <= MESSAGE_CHARS {
            full
        } else {
            format!(
                "MCP server running at {}. To connect a client, press Copy Claude Code command \
                 or Copy Claude Desktop entry.",
                status.url
            )
        };
        (MessageLevel::Info, text)
    };
    (level, text.chars().take(MESSAGE_CHARS).collect())
}

/// The command that registers this add-on with Claude Code.
pub fn claude_code_command(program: &str) -> String {
    format!("claude mcp add backtracking -- \"{program}\" --stdio")
}

/// The `mcpServers` entry for Claude Desktop's `claude_desktop_config.json`.
pub fn claude_desktop_entry(program: &str) -> String {
    let entry = serde_json::json!({
        "mcpServers": {
            "backtracking": { "command": program, "args": ["--stdio"] }
        }
    });
    serde_json::to_string_pretty(&entry).unwrap_or_default()
}

/// The port setting's value; anything that is not a whole port number means the default.
fn port_setting(value: Option<&serde_json::Value>) -> u16 {
    value
        .and_then(serde_json::Value::as_f64)
        .filter(|port| port.fract() == 0.0 && (0.0..=65535.0).contains(port))
        .map_or(DEFAULT_PORT, |port| port as u16)
}

fn describe() -> AddonMessage {
    AddonMessage::Describe {
        sources: Vec::new(),
        settings: vec![Setting {
            key: PORT_SETTING.into(),
            label: "Port on 127.0.0.1".into(),
            help: Some(
                "0 picks a free port at each start. Clients started with --stdio find the port \
                 by themselves; only clients given the http:// address need it to stay the same."
                    .into(),
            ),
            kind: SettingKind::Number {
                default: f64::from(DEFAULT_PORT),
                min: 0.0,
                max: 65535.0,
                step: 1.0,
            },
        }],
        actions: [
            (ACTION_COPY_CODE, "Copy Claude Code command"),
            (ACTION_COPY_DESKTOP, "Copy Claude Desktop entry"),
            (ACTION_DETAILS, "Show connection details"),
            (ACTION_COPY_HTTP, "Copy HTTP command with token"),
            (ACTION_NEW_TOKEN, "New token"),
        ]
        .into_iter()
        .map(|(id, label)| Action {
            id: id.into(),
            label: label.into(),
        })
        .collect(),
    }
}

/// A short log of what the add-on did, in its data folder. It never holds the token.
struct Log {
    file: Option<std::fs::File>,
}

impl Log {
    const LIMIT: u64 = 1 << 20;

    fn open(data: &Path) -> Self {
        let path = data.join("agent.log");
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > Self::LIMIT) {
            let _ = std::fs::remove_file(&path);
        }
        Self {
            file: OpenOptions::new().create(true).append(true).open(path).ok(),
        }
    }

    fn line(&mut self, text: &str) {
        if let Some(file) = &mut self.file {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            let _ = writeln!(file, "{}.{:03} {text}", now.as_secs(), now.subsec_millis());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base<'a>() -> Status<'a> {
        Status {
            url: "http://127.0.0.1:47615/mcp",
            program: Path::new(
                "C:\\Users\\someone\\AppData\\Local\\Backtracking\\addons\\com.kamelkyp.backtracking.agent\\bin\\backtracking-agent.exe",
            ),
            allowed: Some(true),
            port_note: None,
            details_token: None,
            note: None,
        }
    }

    #[test]
    fn the_page_shows_how_to_connect_and_the_token_only_when_asked() {
        let (level, text) = status(&base());
        assert_eq!(level, MessageLevel::Info);
        assert!(text.contains("http://127.0.0.1:47615/mcp"));
        assert!(text.ends_with("backtracking-agent.exe\" --stdio"), "{text}");
        assert!(text.chars().count() <= MESSAGE_CHARS);
        // A program path too long for the line is left to the buttons, not cut in half.
        let deep = format!("C:\\{}\\backtracking-agent.exe", "folder\\".repeat(40));
        let long = Status {
            program: Path::new(&deep),
            ..base()
        };
        let (_, text) = status(&long);
        assert!(!text.contains("folder"), "{text}");
        assert!(text.contains("Copy Claude Code command"));

        let off = Status {
            allowed: Some(false),
            ..base()
        };
        let (level, text) = status(&off);
        assert_eq!(level, MessageLevel::Warning);
        assert!(text.contains("Let this add-on control Backtracking"));

        let token = "ab".repeat(32);
        let details = Status {
            details_token: Some(&token),
            ..base()
        };
        assert!(status(&details).1.contains(&token));
        for other in [base(), off] {
            assert!(!status(&other).1.contains(&token));
        }
    }

    #[test]
    fn the_copied_commands_quote_the_program() {
        let program = "C:\\Users\\A B\\backtracking-agent.exe";
        assert_eq!(
            claude_code_command(program),
            "claude mcp add backtracking -- \"C:\\Users\\A B\\backtracking-agent.exe\" --stdio"
        );
        let entry: serde_json::Value =
            serde_json::from_str(&claude_desktop_entry(program)).unwrap();
        assert_eq!(entry["mcpServers"]["backtracking"]["command"], program);
        assert_eq!(entry["mcpServers"]["backtracking"]["args"][0], "--stdio");
    }

    #[test]
    fn the_port_setting_falls_back_to_the_default() {
        use serde_json::json;
        assert_eq!(port_setting(None), DEFAULT_PORT);
        assert_eq!(port_setting(Some(&json!(0))), 0);
        assert_eq!(port_setting(Some(&json!(41000.0))), 41000);
        assert_eq!(port_setting(Some(&json!(70000))), DEFAULT_PORT);
        assert_eq!(port_setting(Some(&json!(1.5))), DEFAULT_PORT);
        assert_eq!(port_setting(Some(&json!("80"))), DEFAULT_PORT);
    }
}

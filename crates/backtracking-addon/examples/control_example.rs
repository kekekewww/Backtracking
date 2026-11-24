//! An add-on that controls Backtracking once the user allows it: it reads the state, pauses,
//! selects the last two seconds, saves them as files, clears the selection and starts
//! capturing again, writing what it did to `transcript.log` in its data folder and to stdout.
//!
//! Build with `cargo build --release --example control_example`. The files go to the folder
//! in the add-on's `folder` setting, else to `BACKTRACKING_CONTROL_EXAMPLE_FOLDER`, else to
//! `saved` in the add-on's data folder. `BACKTRACKING_CONTROL_EXAMPLE_HOLD_MS` keeps the
//! selection that long before saving, so that it can be seen.

use backtracking_addon::client::Client;
use backtracking_addon::protocol::{
    Action, AddonMessage, ControlCommand, ControlResult, HostMessage, MessageLevel, Setting,
    SettingKind,
};
use backtracking_addon::{Error, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const ID: &str = "com.example.control";
const VERSION: &str = "0.1.0";

fn main() {
    if let Err(error) = run() {
        eprintln!("Control example: {error}");
        std::process::exit(1);
    }
}

struct Example {
    client: Client,
    folder: String,
    transcript: PathBuf,
}

fn run() -> Result<()> {
    let client = Client::connect_from_env(ID, VERSION)?;
    let folder = client
        .settings()
        .get("folder")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned();
    let transcript = client.data_dir().join("transcript.log");
    let mut example = Example {
        client,
        folder,
        transcript,
    };
    example.client.send(&describe())?;
    example.note("connected")?;

    let started = Instant::now();
    let mut tried_refused = false;
    // Whether the sequence has run since it was last allowed.
    let mut done = false;
    loop {
        match example.client.recv_timeout(Duration::from_millis(50))? {
            Some(HostMessage::Quit) => {
                example.note("asked to quit")?;
                return Ok(());
            }
            Some(HostMessage::SetSetting { key, value }) if key == "folder" => {
                example.folder = value.as_str().unwrap_or_default().to_owned();
                example.note(&format!("folder setting is now {:?}", example.folder))?;
            }
            Some(HostMessage::RunAction { id }) if id == "run" => {
                done = true;
                example.sequence()?;
            }
            Some(HostMessage::ControlAccess { allowed }) => {
                example.note(&format!("control_access allowed={allowed}"))?;
                if !allowed {
                    done = false;
                }
            }
            _ => {}
        }
        if example.client.is_disconnected() {
            return Ok(());
        }
        match example.client.control_allowed() {
            // Once, as soon as the user allows it; the Run again action repeats it.
            Some(true) if !done => {
                done = true;
                example.sequence()?;
            }
            // Show what a refusal looks like once, then wait for the user's choice.
            Some(false) if !tried_refused => {
                tried_refused = true;
                example.step(&ControlCommand::GetState)?;
                example.note("waiting until control is allowed on Options > Add-ons")?;
            }
            None if !tried_refused && started.elapsed() > Duration::from_secs(2) => {
                tried_refused = true;
                example.note("the host has not said whether control is allowed")?;
            }
            _ => {}
        }
    }
}

impl Example {
    /// Run the whole sequence; true when it got to the end.
    fn sequence(&mut self) -> Result<bool> {
        let folder = self.save_folder()?;
        let Some(state) = self.step(&ControlCommand::GetState)? else {
            return Ok(false);
        };
        self.note(&format!(
            "{} lane(s) at {} Hz, {:.1} s of history, capturing={}",
            state.state.lanes.len(),
            state.state.sample_rate,
            state.state.history.seconds,
            state.state.capturing
        ))?;
        if self.step(&ControlCommand::PauseCapture)?.is_none() {
            return Ok(false);
        }
        if self
            .step(&ControlCommand::select_seconds_ago(2.0, 0.0))?
            .is_none()
        {
            return Ok(false);
        }
        if let Some(hold) = std::env::var("BACKTRACKING_CONTROL_EXAMPLE_HOLD_MS")
            .ok()
            .and_then(|text| text.parse::<u64>().ok())
        {
            self.note(&format!("holding the selection for {hold} ms"))?;
            let until = Instant::now() + Duration::from_millis(hold);
            while Instant::now() < until {
                if matches!(
                    self.client.recv_timeout(Duration::from_millis(50))?,
                    Some(HostMessage::Quit)
                ) {
                    return Ok(false);
                }
            }
        }
        let Some(saved) = self.step(&ControlCommand::save(Some(folder.clone())))? else {
            return Ok(false);
        };
        let files = saved.files.unwrap_or_default();
        for file in &files {
            self.note(&format!(
                "saved {} lane={} channels={} rate={} frames={}",
                file.path,
                file.lane.as_deref().unwrap_or("?"),
                file.channels.map_or("?".into(), |v| v.to_string()),
                file.sample_rate.map_or("?".into(), |v| v.to_string()),
                file.frames.map_or("?".into(), |v| v.to_string()),
            ))?;
        }
        self.step(&ControlCommand::ClearSelection)?;
        self.step(&ControlCommand::StartCapture)?;
        self.step(&ControlCommand::GetState)?;
        let summary = format!("Saved {} file(s) to {folder}", files.len());
        self.note(&summary)?;
        self.client.send(&AddonMessage::Message {
            level: MessageLevel::Info,
            text: summary,
        })?;
        Ok(true)
    }

    /// Send one command and write the request and reply to the transcript. `None` when the
    /// host refused it.
    fn step(&mut self, command: &ControlCommand) -> Result<Option<ControlResult>> {
        let request = serde_json::to_string(command).map_err(|e| Error::new(e.to_string()))?;
        self.note(&format!("> {request}"))?;
        match self.client.control(command)? {
            Ok(result) => {
                let reply =
                    serde_json::to_string(&result).map_err(|e| Error::new(e.to_string()))?;
                self.note(&format!("< ok {reply}"))?;
                Ok(Some(result))
            }
            Err(error) => {
                self.note(&format!("< error {}: {}", error.code, error.message))?;
                Ok(None)
            }
        }
    }

    fn save_folder(&self) -> Result<String> {
        let folder = if !self.folder.trim().is_empty() {
            PathBuf::from(self.folder.trim())
        } else if let Some(folder) = std::env::var_os("BACKTRACKING_CONTROL_EXAMPLE_FOLDER") {
            PathBuf::from(folder)
        } else {
            self.client.data_dir().join("saved")
        };
        fs::create_dir_all(&folder)?;
        Ok(folder.display().to_string())
    }

    fn note(&self, line: &str) -> Result<()> {
        println!("{line}");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.transcript)?;
        writeln!(file, "{line}")?;
        Ok(())
    }
}

fn describe() -> AddonMessage {
    AddonMessage::Describe {
        sources: Vec::new(),
        settings: vec![Setting {
            key: "folder".into(),
            label: "Save to folder".into(),
            help: Some("Leave empty to save in the add-on's own data folder.".into()),
            kind: SettingKind::Text {
                default: String::new(),
            },
        }],
        actions: vec![Action {
            id: "run".into(),
            label: "Run again".into(),
        }],
    }
}

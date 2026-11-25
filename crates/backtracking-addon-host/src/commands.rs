use crate::addons::{AddonHandle, AddonReply};
use backtracking_addon::{
    package::PackageInfo,
    protocol::{ControlError, ControlResult, TransportCommand},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Request {
    Inspect {
        path: PathBuf,
    },
    Install {
        path: PathBuf,
        review_id: String,
    },
    Remove {
        id: String,
        delete_data: bool,
    },
    SetEnabled {
        id: String,
        enabled: bool,
    },
    Start {
        id: String,
    },
    Stop {
        id: String,
    },
    SetSetting {
        id: String,
        key: String,
        value: Value,
    },
    RunAction {
        id: String,
        action: String,
    },
    OpenFile {
        id: String,
        path: PathBuf,
    },
    SelectFormat {
        id: String,
        source: String,
        format: String,
    },
    Transport {
        id: String,
        source: String,
        command: TransportCommand,
        position: Option<f64>,
    },
    ClipExported {
        files: Vec<PathBuf>,
        sample_rate: u32,
        frames: u64,
    },
    SetControlAllowed {
        id: String,
        allowed: bool,
    },
    TakeControl,
    ControlReply {
        addon_id: String,
        run: u64,
        request_id: String,
        result: Option<ControlResult>,
        error: Option<ControlError>,
    },
    PositionsSince {
        cursor: u64,
    },
    TakeResult {
        ticket: u64,
    },
}
pub(crate) type Pending = Box<dyn FnMut() -> Option<Result<Value, String>>>;
fn reply<T: 'static>(rx: AddonReply<T>, convert: impl Fn(T) -> Value + 'static) -> Pending {
    Box::new(move || match rx.try_recv() {
        Ok(v) => Some(v.map(&convert)),
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
        Err(_) => Some(Err("The add-on manager has stopped".into())),
    })
}
fn summary(info: PackageInfo) -> Value {
    json!({"manifest":info.manifest,"program":info.program,"entry_count":info.entry_count,"file_count":info.file_count,"unpacked_size":info.unpacked_size})
}
pub(crate) fn dispatch(h: &AddonHandle, request: Request) -> Result<Pending, String> {
    use Request::*;
    let result = match request {
        Inspect { path } => {
            return Ok(reply(h.inspect(path), |inspection| {
                let mut v = summary(inspection.info);
                v["review_id"] = json!(
                    inspection
                        .identity
                        .iter()
                        .map(|n| format!("{n:02x}"))
                        .collect::<String>()
                );
                v
            }));
        }
        Install { path, review_id } => {
            if review_id.len() != 64
                || !review_id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err("Invalid package review id".into());
            }
            let mut hash = [0u8; 32];
            for (i, byte) in hash.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&review_id[2 * i..2 * i + 2], 16).unwrap()
            }
            return Ok(reply(h.install(path, hash), summary));
        }
        Start { id } => h.start(&id),
        Stop { id } => h.stop(&id),
        Remove { id, delete_data } => h.remove(&id, delete_data),
        SetEnabled { id, enabled } => h.set_enabled(&id, enabled),
        SetControlAllowed { id, allowed } => h.set_control_allowed(&id, allowed),
        SetSetting { id, key, value } => h.set_setting(&id, &key, value),
        RunAction { id, action } => h.run_action(&id, &action),
        SelectFormat { id, source, format } => h.select_format(&id, &source, &format),
        OpenFile { id, path } => h.open_file(&id, path),
        Transport {
            id,
            source,
            command,
            position,
        } => h.transport(&id, &source, command, position),
        ClipExported {
            files,
            sample_rate,
            frames,
        } => h.clip_exported(files, sample_rate, frames),
        TakeControl | PositionsSince { .. } | ControlReply { .. } | TakeResult { .. } => {
            return Err("Invalid queued command".into());
        }
    };
    Ok(reply(result, |()| Value::Null))
}

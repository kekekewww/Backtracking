//! Requests an add-on with the `control` capability sends to drive Backtracking, and the
//! replies the host sends back.
//!
//! Every request carries an `id` the add-on chooses. The host answers each one exactly once
//! with a `control_reply` carrying the same `id` and either a `result` or an `error`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{RawFields, MAX_LINE_BYTES};
use crate::{Error, Result};

/// The longest control request the host reads, in bytes including the newline. A longer line
/// up to [`MAX_LINE_BYTES`] gets the `request_too_large` error; a line longer than that closes
/// the connection, as any other message would.
pub const MAX_CONTROL_REQUEST_BYTES: usize = 16_384;

/// How many requests of one add-on may wait for their replies at once. One more gets the
/// `too_many_requests` error.
pub const MAX_CONTROL_IN_FLIGHT: usize = 8;

/// The longest request id, in characters.
pub const MAX_CONTROL_ID_CHARS: usize = 64;

/// The host answers every request within this time, or with the `timeout` error at that time.
pub const CONTROL_REPLY_WITHIN: Duration = Duration::from_secs(10);

/// `save` writes audio and gets longer: the host answers within this time.
pub const CONTROL_SAVE_REPLY_WITHIN: Duration = Duration::from_secs(120);

/// The command names this version knows, in the order of the contract's table.
pub const CONTROL_COMMANDS: [&str; 14] = [
    "get_state",
    "list_sources",
    "start_capture",
    "pause_capture",
    "clear_history",
    "select",
    "clear_selection",
    "choose_rows",
    "set_view",
    "save",
    "play_preview",
    "stop_preview",
    "add_source",
    "remove_source",
];

/// The stable error codes of a `control_reply`. A later host may add codes; treat one you
/// do not know like `failed`.
pub mod codes {
    /// The add-on's manifest does not declare `control`.
    pub const NOT_DECLARED: &str = "not_declared";
    /// The user has not switched on "Let this add-on control Backtracking" for this add-on.
    pub const NOT_ALLOWED: &str = "not_allowed";
    /// The request is not a valid control request: a missing or bad `id`, a missing
    /// `command`, or a field of the wrong type or out of range.
    pub const INVALID_REQUEST: &str = "invalid_request";
    /// The request line is longer than `MAX_CONTROL_REQUEST_BYTES`.
    pub const REQUEST_TOO_LARGE: &str = "request_too_large";
    /// This version of Backtracking does not know the command.
    pub const UNKNOWN_COMMAND: &str = "unknown_command";
    /// `MAX_CONTROL_IN_FLIGHT` requests of this add-on are already waiting for replies.
    pub const TOO_MANY_REQUESTS: &str = "too_many_requests";
    /// A request with this `id` is still waiting for its reply.
    pub const DUPLICATE_ID: &str = "duplicate_id";
    /// The user is in the middle of something (a drag, or a drag-out or save being written).
    /// Try again shortly.
    pub const BUSY: &str = "busy";
    /// The lane, row or source named does not exist.
    pub const NOT_FOUND: &str = "not_found";
    /// There is no audio to act on yet.
    pub const NO_AUDIO: &str = "no_audio";
    /// The command needs a selection and there is none.
    pub const NO_SELECTION: &str = "no_selection";
    /// Backtracking tried and could not do it; `message` says why.
    pub const FAILED: &str = "failed";
    /// No result came within the time the command is given.
    pub const TIMEOUT: &str = "timeout";
}

/// What an add-on asks Backtracking to do. Times are seconds, frames count from the start of
/// the capture at the capture's sample rate, and lanes are named by their letters, `A` to
/// `BL`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ControlCommand {
    /// Reply with the state only.
    GetState,
    /// The sources that can be added as a lane.
    ListSources,
    /// Start (or resume) capture, as the toolbar's start button does.
    StartCapture,
    /// Pause capture, as the toolbar's pause button does. The audio held stays.
    PauseCapture,
    /// Throw away the audio held, as Clear does.
    ClearHistory,
    /// Select a stretch of the audio held, as a drag across the waveform does. Give either
    /// `from_seconds_ago` (with `to_seconds_ago`, 0 when left out), or `start_frame` and
    /// `end_frame`.
    Select {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_seconds_ago: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_seconds_ago: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_frame: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_frame: Option<u64>,
    },
    /// Drop the selection, as Escape does.
    ClearSelection,
    /// Which rows the selection covers, as "Edit separately" and a drag across rows do:
    /// every row of `lane`, the rows from `first` to `last`, or with neither, every row
    /// (Edit separately off).
    ChooseRows {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lane: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first: Option<RowRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last: Option<RowRef>,
    },
    /// Show the newest `seconds`, or `start_frame` to `end_frame`, or with neither everything
    /// held, as zooming does.
    SetView {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seconds: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_frame: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_frame: Option<u64>,
    },
    /// Write the selection, or everything held without one, as WAV files into `folder` (an
    /// absolute path, made when missing), or into the folder Save uses when it is left out.
    /// This is an add-on's way to drag a clip out of the window. The reply comes once the
    /// files are written.
    Save {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
    },
    /// Play the selection, as Space does.
    PlayPreview,
    /// Stop playing the selection.
    StopPreview,
    /// Add `source`, an `id` from `list_sources`, as the next lane. This starts a new, empty
    /// history, as adding a source on the Sources page does.
    AddSource { source: String },
    /// Remove a lane; the lanes after it move up a letter. This starts a new, empty history,
    /// as removing a source on the Sources page does.
    RemoveSource { lane: String },
}

/// One row of a lane, counted from 1 at the top, as the row tooltips count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowRef {
    pub lane: String,
    pub row: u32,
}

impl ControlCommand {
    /// The wire name of the command.
    pub fn name(&self) -> &'static str {
        match self {
            Self::GetState => "get_state",
            Self::ListSources => "list_sources",
            Self::StartCapture => "start_capture",
            Self::PauseCapture => "pause_capture",
            Self::ClearHistory => "clear_history",
            Self::Select { .. } => "select",
            Self::ClearSelection => "clear_selection",
            Self::ChooseRows { .. } => "choose_rows",
            Self::SetView { .. } => "set_view",
            Self::Save { .. } => "save",
            Self::PlayPreview => "play_preview",
            Self::StopPreview => "stop_preview",
            Self::AddSource { .. } => "add_source",
            Self::RemoveSource { .. } => "remove_source",
        }
    }

    /// Selects from `from` seconds before the newest audio to `to` seconds before it.
    pub fn select_seconds_ago(from: f64, to: f64) -> Self {
        Self::Select {
            from_seconds_ago: Some(from),
            to_seconds_ago: Some(to),
            start_frame: None,
            end_frame: None,
        }
    }

    /// Selects absolute frames, as `get_state` reports them.
    pub fn select_frames(start: u64, end: u64) -> Self {
        Self::Select {
            from_seconds_ago: None,
            to_seconds_ago: None,
            start_frame: Some(start),
            end_frame: Some(end),
        }
    }

    /// Writes into `folder`, or into the folder Save uses with `None`.
    pub fn save(folder: Option<String>) -> Self {
        Self::Save { folder }
    }

    /// How long the host may take to answer this command.
    pub fn reply_within(&self) -> Duration {
        match self {
            Self::Save { .. } => CONTROL_SAVE_REPLY_WITHIN,
            _ => CONTROL_REPLY_WITHIN,
        }
    }

    /// Checks the fields against the rules above.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Select {
                from_seconds_ago,
                to_seconds_ago,
                start_frame,
                end_frame,
            } => {
                let seconds = from_seconds_ago.is_some() || to_seconds_ago.is_some();
                let frames = start_frame.is_some() || end_frame.is_some();
                match (seconds, frames) {
                    (true, true) => {
                        return invalid(
                            "select takes from_seconds_ago and to_seconds_ago, or start_frame \
                             and end_frame, not both",
                        )
                    }
                    (false, false) => {
                        return invalid(
                            "select needs from_seconds_ago, or start_frame and end_frame",
                        )
                    }
                    (true, false) => {
                        let from = from_seconds_ago
                            .ok_or_else(|| Error::new("select needs from_seconds_ago"))?;
                        let to = to_seconds_ago.unwrap_or(0.0);
                        seconds_field("select.from_seconds_ago", from)?;
                        seconds_field("select.to_seconds_ago", to)?;
                        if from <= to {
                            return invalid(
                                "select.from_seconds_ago must be further back than to_seconds_ago",
                            );
                        }
                    }
                    (false, true) => frames_field("select", *start_frame, *end_frame)?,
                }
            }
            Self::ChooseRows { lane, first, last } => {
                match (lane, first, last) {
                    (Some(_), None, None) | (None, None, None) => {}
                    (None, Some(_), Some(_)) => {}
                    _ => {
                        return invalid(
                            "choose_rows takes lane, or first and last, or nothing for every row",
                        )
                    }
                }
                if let Some(lane) = lane {
                    lane_field("choose_rows.lane", lane)?;
                }
                for row in [first, last].into_iter().flatten() {
                    lane_field("choose_rows row lane", &row.lane)?;
                    if row.row == 0 {
                        return invalid("choose_rows rows count from 1");
                    }
                }
            }
            Self::SetView {
                seconds,
                start_frame,
                end_frame,
            } => {
                let frames = start_frame.is_some() || end_frame.is_some();
                if seconds.is_some() && frames {
                    return invalid("set_view takes seconds, or start_frame and end_frame");
                }
                if let Some(seconds) = seconds {
                    seconds_field("set_view.seconds", *seconds)?;
                    if *seconds <= 0.0 {
                        return invalid("set_view.seconds must be more than 0");
                    }
                }
                if frames {
                    frames_field("set_view", *start_frame, *end_frame)?;
                }
            }
            Self::Save { folder: Some(folder) } => {
                if folder.chars().count() > 1024 {
                    return invalid("save.folder must have at most 1024 characters");
                }
                super::absolute_path("save.folder", folder)?;
            }
            Self::AddSource { source } => {
                if source.is_empty() || source.chars().count() > 1024 {
                    return invalid("add_source.source must have 1 to 1024 characters");
                }
            }
            Self::RemoveSource { lane } => lane_field("remove_source.lane", lane)?,
            _ => {}
        }
        Ok(())
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::new(message))
}

fn seconds_field(field: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value < 0.0 || value > 1.0e7 {
        return invalid(format!(
            "{field} must be a number of seconds from 0 to 10000000"
        ));
    }
    Ok(())
}

fn frames_field(command: &str, start: Option<u64>, end: Option<u64>) -> Result<()> {
    match (start, end) {
        (Some(start), Some(end)) if start < end => Ok(()),
        (Some(_), Some(_)) => invalid(format!("{command}.start_frame must be below end_frame")),
        _ => invalid(format!("{command} needs both start_frame and end_frame")),
    }
}

/// A lane letter: one or two letters, `A` to `BL`, in either case. The host checks that the
/// lane exists.
fn lane_field(field: &str, lane: &str) -> Result<()> {
    if (1..=2).contains(&lane.len()) && lane.bytes().all(|b| b.is_ascii_alphabetic()) {
        Ok(())
    } else {
        invalid(format!("{field} must be a lane letter, A to BL"))
    }
}

/// Why a request was not carried out. `code` is one of [`codes`]; `message` is for people.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlError {
    pub code: String,
    pub message: String,
}

impl ControlError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        let message: String = message.into();
        Self {
            code: code.into(),
            // A parser's message can quote much of a line.
            message: message.chars().take(300).collect(),
        }
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ControlError {}

/// The host's answer to one request. Exactly one of `result` and `error` is present. `id` is
/// `null` only when the request had no usable id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlReply {
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ControlError>,
}

impl ControlReply {
    pub fn success(id: impl Into<String>, result: &ControlResult) -> Self {
        Self {
            id: Some(id.into()),
            result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
            error: None,
        }
    }

    pub fn failure(id: Option<String>, error: ControlError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }

    /// The result, or the error the host gave.
    pub fn outcome(&self) -> std::result::Result<&Value, &ControlError> {
        match (&self.result, &self.error) {
            (_, Some(error)) => Err(error),
            (Some(result), None) => Ok(result),
            (None, None) => Ok(&Value::Null),
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.result.is_some() == self.error.is_some() {
            return invalid("control_reply carries exactly one of result and error");
        }
        if let Some(id) = &self.id {
            id_field(id)?;
        }
        Ok(())
    }
}

/// What every successful reply holds: the state after the command, and what the command
/// itself returns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlResult {
    pub state: ControlState,
    /// `list_sources`: the sources that can be added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<SourceInfo>>,
    /// `save`: the folder written to and every file written, the positions files included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<SavedFile>>,
    /// `add_source` and `remove_source`: the audio held was cleared and capture started anew.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub history_restarted: bool,
}

/// What Backtracking is doing, as `get_state` and every other reply report it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlState {
    /// Capture is on (not paused).
    pub capturing: bool,
    /// Capture is on and audio is arriving.
    pub recording: bool,
    /// What stops capture, when something does.
    #[serde(default)]
    pub problem: Option<String>,
    /// The capture's sample rate. 0 before any audio has arrived.
    pub sample_rate: u32,
    pub lanes: Vec<LaneState>,
    /// The audio held, newest at `end_frame`.
    pub history: FrameSpan,
    #[serde(default)]
    pub selection: Option<SelectionState>,
    /// What the window shows.
    pub view: FrameSpan,
    /// "Edit separately" is on: the selection covers `chosen_rows` only.
    pub separate_rows: bool,
    /// The rows the selection covers while rows are edited separately; empty covers every row.
    #[serde(default)]
    pub chosen_rows: Vec<RowState>,
    /// The selection is playing.
    pub preview_playing: bool,
    /// Where Save writes when `save` names no folder.
    pub save_folder: String,
    /// Where a drag-out's files are written before they are dropped.
    pub drag_folder: String,
    /// Why commands that change things would get `busy` now, when they would.
    #[serde(default)]
    pub busy: Option<String>,
}

/// A stretch of frames, and how long it is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameSpan {
    pub start_frame: u64,
    pub end_frame: u64,
    pub seconds: f64,
}

/// The selection, in frames and in seconds back from the newest audio held.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SelectionState {
    pub start_frame: u64,
    pub end_frame: u64,
    pub seconds: f64,
    pub from_seconds_ago: f64,
    pub to_seconds_ago: f64,
}

/// One lane.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LaneState {
    /// Its letter, `A` to `BL`.
    pub lane: String,
    pub source: SourceInfo,
    /// The channels it captures, once known.
    #[serde(default)]
    pub channels: Option<u32>,
    /// `merged`, `left`, `right`, `center`, `mid`, `side` or `all`.
    pub route: String,
    /// What is wrong with its source, when something is.
    #[serde(default)]
    pub problem: Option<String>,
}

/// A source that is or can be a lane.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// Pass this to `add_source`.
    pub id: String,
    /// `system`, `default-microphone`, `microphone`, `application`, `sender` or `addon`.
    pub kind: String,
    pub name: String,
}

/// One chosen row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowState {
    pub lane: String,
    /// Counted from 1 at the top of the lane.
    pub row: u32,
    pub label: String,
}

/// One file `save` wrote.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedFile {
    /// Absolute.
    pub path: String,
    /// `wav`, or `positions` for the positions file beside a lane's object audio.
    pub kind: String,
    /// The lane whose audio it holds.
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(default)]
    pub channels: Option<u32>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub frames: Option<u64>,
}

fn id_field(id: &str) -> Result<()> {
    if id.is_empty()
        || id.chars().count() > MAX_CONTROL_ID_CHARS
        || id.chars().any(char::is_control)
    {
        return invalid(format!(
            "control id must be a string of 1 to {MAX_CONTROL_ID_CHARS} characters without \
             control characters"
        ));
    }
    Ok(())
}

/// A request as the host reads it: a command to carry out, or the error to reply with.
pub(super) enum Decoded {
    Request { id: String, command: ControlCommand },
    Refused { id: Option<String>, error: ControlError },
}

/// Reads a `control` line leniently: a bad request is answered, not a reason to close.
pub(super) fn decode_control(line: &[u8], fields: &RawFields<'_>) -> Decoded {
    let id = fields
        .get("id")
        .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
        .filter(|id| id_field(id).is_ok());
    let refuse = |code: &str, message: String| Decoded::Refused {
        id: id.clone(),
        error: ControlError::new(code, message),
    };
    let length = line.len() + usize::from(line.last() != Some(&b'\n'));
    if length > MAX_CONTROL_REQUEST_BYTES {
        return refuse(
            codes::REQUEST_TOO_LARGE,
            format!("A control request may have at most {MAX_CONTROL_REQUEST_BYTES} bytes"),
        );
    }
    let Some(id) = id.clone() else {
        return refuse(
            codes::INVALID_REQUEST,
            "A control request needs an id: a string of 1 to 64 characters".into(),
        );
    };
    let Some(command) = fields
        .get("command")
        .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
    else {
        return refuse(
            codes::INVALID_REQUEST,
            "A control request needs a command".into(),
        );
    };
    if !CONTROL_COMMANDS.contains(&command.as_str()) {
        let shown: String = command.chars().take(64).collect();
        return refuse(
            codes::UNKNOWN_COMMAND,
            format!("This Backtracking does not know the command {shown:?}"),
        );
    }
    let payload = line.strip_suffix(b"\n").unwrap_or(line);
    match serde_json::from_slice::<ControlCommand>(payload) {
        Ok(parsed) => match parsed.validate() {
            Ok(()) => Decoded::Request {
                id,
                command: parsed,
            },
            Err(error) => refuse(codes::INVALID_REQUEST, error.to_string()),
        },
        Err(error) => refuse(codes::INVALID_REQUEST, format!("{command}: {error}")),
    }
}

/// Encodes a request, after checking it, for an add-on to send.
pub(super) fn request_line(id: &str, command: &ControlCommand) -> Result<Vec<u8>> {
    id_field(id)?;
    command.validate()?;
    let mut value = serde_json::to_value(command)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| Error::new("a control command is an object"))?;
    object.insert("type".into(), "control".into());
    object.insert("id".into(), id.into());
    let mut line = serde_json::to_vec(&value)?;
    line.push(b'\n');
    if line.len() > MAX_CONTROL_REQUEST_BYTES.min(MAX_LINE_BYTES) {
        return invalid(format!(
            "A control request may have at most {MAX_CONTROL_REQUEST_BYTES} bytes"
        ));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::super::{decode_addon, decode_host, encode_addon, encode_host, AddonMessage};
    use super::super::HostMessage;
    use super::*;
    use serde_json::json;

    fn refused(line: &str) -> (Option<String>, ControlError) {
        match decode_addon(line.as_bytes()).unwrap() {
            AddonMessage::ControlRefused { id, error } => (id, error),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn request(line: &str) -> (String, ControlCommand) {
        match decode_addon(line.as_bytes()).unwrap() {
            AddonMessage::Control { id, command } => (id, command),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[test]
    fn every_command_round_trips_with_its_id() {
        let commands = [
            ControlCommand::GetState,
            ControlCommand::ListSources,
            ControlCommand::StartCapture,
            ControlCommand::PauseCapture,
            ControlCommand::ClearHistory,
            ControlCommand::select_seconds_ago(2.0, 0.0),
            ControlCommand::select_frames(10, 20),
            ControlCommand::ClearSelection,
            ControlCommand::ChooseRows {
                lane: Some("B".into()),
                first: None,
                last: None,
            },
            ControlCommand::ChooseRows {
                lane: None,
                first: Some(RowRef {
                    lane: "A".into(),
                    row: 1,
                }),
                last: Some(RowRef {
                    lane: "B".into(),
                    row: 2,
                }),
            },
            ControlCommand::ChooseRows {
                lane: None,
                first: None,
                last: None,
            },
            ControlCommand::SetView {
                seconds: Some(4.0),
                start_frame: None,
                end_frame: None,
            },
            ControlCommand::save(Some("C:\\clips".into())),
            ControlCommand::save(None),
            ControlCommand::PlayPreview,
            ControlCommand::StopPreview,
            ControlCommand::AddSource {
                source: "system".into(),
            },
            ControlCommand::RemoveSource { lane: "BL".into() },
        ];
        let mut names = Vec::new();
        for (index, command) in commands.into_iter().enumerate() {
            let id = format!("r{index}");
            let line = encode_addon(&AddonMessage::Control {
                id: id.clone(),
                command: command.clone(),
            })
            .unwrap();
            let wire: Value = serde_json::from_slice(&line).unwrap();
            assert_eq!(wire["type"], "control");
            assert_eq!(wire["id"], id.as_str());
            assert_eq!(wire["command"], command.name());
            assert_eq!(
                request(std::str::from_utf8(&line).unwrap()),
                (id, command.clone())
            );
            if !names.contains(&command.name()) {
                names.push(command.name());
            }
        }
        assert_eq!(names, CONTROL_COMMANDS);
    }

    #[test]
    fn the_wire_shape_is_flat() {
        let line = encode_addon(&AddonMessage::Control {
            id: "7".into(),
            command: ControlCommand::select_seconds_ago(2.0, 0.5),
        })
        .unwrap();
        let wire: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(
            wire,
            json!({"type":"control","id":"7","command":"select",
                   "from_seconds_ago":2.0,"to_seconds_ago":0.5})
        );
        // Unknown fields are ignored, as everywhere in contract 1.
        let (_, command) =
            request(r#"{"type":"control","id":"x","command":"get_state","later":true}"#);
        assert_eq!(command, ControlCommand::GetState);
        // to_seconds_ago defaults to the newest audio.
        let (_, command) =
            request(r#"{"type":"control","id":"x","command":"select","from_seconds_ago":3}"#);
        assert_eq!(
            command,
            ControlCommand::Select {
                from_seconds_ago: Some(3.0),
                to_seconds_ago: None,
                start_frame: None,
                end_frame: None
            }
        );
    }

    #[test]
    fn bad_requests_are_refused_with_stable_codes_and_keep_the_connection() {
        let cases = [
            (r#"{"type":"control","command":"get_state"}"#, None, codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"","command":"get_state"}"#, None, codes::INVALID_REQUEST),
            (r#"{"type":"control","id":5,"command":"get_state"}"#, None, codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a"}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":7}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"quit"}"#, Some("a"), codes::UNKNOWN_COMMAND),
            (r#"{"type":"control","id":"a","command":"set_allowed"}"#, Some("a"), codes::UNKNOWN_COMMAND),
            (r#"{"type":"control","id":"a","command":"select"}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"select","from_seconds_ago":"2"}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"select","from_seconds_ago":1,"to_seconds_ago":2}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"select","from_seconds_ago":-1}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"select","from_seconds_ago":2,"start_frame":0,"end_frame":5}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"select","start_frame":9,"end_frame":5}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"choose_rows","lane":"A","first":{"lane":"A","row":1}}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"choose_rows","first":{"lane":"A","row":0},"last":{"lane":"A","row":1}}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"remove_source","lane":"A1"}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"set_view","seconds":0}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"save","folder":"relative\\clips"}"#, Some("a"), codes::INVALID_REQUEST),
            (r#"{"type":"control","id":"a","command":"add_source"}"#, Some("a"), codes::INVALID_REQUEST),
        ];
        for (line, id, code) in cases {
            let (got_id, error) = refused(line);
            assert_eq!(got_id.as_deref(), id, "{line}");
            assert_eq!(error.code, code, "{line}: {}", error.message);
            assert!(!error.message.is_empty());
        }
        let long_id = format!(r#"{{"type":"control","id":"{}","command":"get_state"}}"#, "x".repeat(65));
        assert_eq!(refused(&long_id).1.code, codes::INVALID_REQUEST);
    }

    #[test]
    fn a_request_over_the_control_limit_is_refused_and_a_line_over_the_line_limit_closes() {
        let padding = "p".repeat(MAX_CONTROL_REQUEST_BYTES);
        let large = format!(r#"{{"type":"control","id":"big","command":"get_state","pad":"{padding}"}}"#);
        let (id, error) = refused(&large);
        assert_eq!(id.as_deref(), Some("big"));
        assert_eq!(error.code, codes::REQUEST_TOO_LARGE);
        // Exactly at the limit, newline included, is still read.
        let base = r#"{"type":"control","id":"edge","command":"get_state","pad":""}"#;
        let fill = MAX_CONTROL_REQUEST_BYTES - 1 - base.len();
        let edge = format!(
            r#"{{"type":"control","id":"edge","command":"get_state","pad":"{}"}}"#,
            "p".repeat(fill)
        );
        assert_eq!(edge.len() + 1, MAX_CONTROL_REQUEST_BYTES);
        assert_eq!(request(&format!("{edge}\n")).1, ControlCommand::GetState);
        let huge = format!(
            r#"{{"type":"control","id":"big","command":"get_state","pad":"{}"}}"#,
            "p".repeat(MAX_LINE_BYTES)
        );
        assert!(decode_addon(huge.as_bytes()).is_err());
        // Not JSON at all is an invalid line, as for every other message.
        assert!(decode_addon(br#"{"type":"control","id":"#).is_err());
    }

    #[test]
    fn an_add_on_cannot_send_what_it_could_not_read() {
        let bad = AddonMessage::Control {
            id: "a".into(),
            command: ControlCommand::select_seconds_ago(1.0, 2.0),
        };
        assert!(encode_addon(&bad).is_err());
        let bad_id = AddonMessage::Control {
            id: "".into(),
            command: ControlCommand::GetState,
        };
        assert!(encode_addon(&bad_id).is_err());
        let refused = AddonMessage::ControlRefused {
            id: None,
            error: ControlError::new(codes::FAILED, "x"),
        };
        assert!(encode_addon(&refused).is_err());
    }

    #[test]
    fn replies_round_trip_and_carry_exactly_one_outcome() {
        let result = ControlResult {
            state: ControlState {
                capturing: true,
                sample_rate: 48_000,
                lanes: vec![LaneState {
                    lane: "A".into(),
                    source: SourceInfo {
                        id: "system".into(),
                        kind: "system".into(),
                        name: "System audio".into(),
                    },
                    channels: Some(2),
                    route: "merged".into(),
                    problem: None,
                }],
                ..ControlState::default()
            },
            files: Some(vec![SavedFile {
                path: "C:\\clips\\a.wav".into(),
                kind: "wav".into(),
                lane: Some("A".into()),
                channels: Some(2),
                sample_rate: Some(48_000),
                frames: Some(96_000),
            }]),
            folder: Some("C:\\clips".into()),
            ..ControlResult::default()
        };
        let reply = HostMessage::ControlReply(ControlReply::success("9", &result));
        let line = encode_host(&reply).unwrap();
        let wire: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(wire["type"], "control_reply");
        assert_eq!(wire["id"], "9");
        assert!(wire.get("error").is_none());
        assert!(wire["result"].get("history_restarted").is_none());
        let HostMessage::ControlReply(back) = decode_host(&line).unwrap() else {
            panic!("not a reply")
        };
        let parsed: ControlResult =
            serde_json::from_value(back.outcome().unwrap().clone()).unwrap();
        assert_eq!(parsed, result);

        let failure = HostMessage::ControlReply(ControlReply::failure(
            None,
            ControlError::new(codes::NOT_ALLOWED, "m".repeat(400)),
        ));
        let line = encode_host(&failure).unwrap();
        let wire: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(wire["id"], Value::Null);
        assert_eq!(wire["error"]["code"], "not_allowed");
        assert_eq!(wire["error"]["message"].as_str().unwrap().len(), 300);

        let both = HostMessage::ControlReply(ControlReply {
            id: Some("1".into()),
            result: Some(json!({})),
            error: Some(ControlError::new(codes::FAILED, "x")),
        });
        assert!(encode_host(&both).is_err());
        assert!(decode_host(br#"{"type":"control_reply","id":"1"}"#).is_err());

        let access = encode_host(&HostMessage::ControlAccess { allowed: false }).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&access).unwrap(),
            json!({"type":"control_access","allowed":false})
        );
    }

    #[test]
    fn save_gets_longer_to_reply() {
        assert_eq!(ControlCommand::save(None).reply_within(), CONTROL_SAVE_REPLY_WITHIN);
        assert_eq!(ControlCommand::GetState.reply_within(), CONTROL_REPLY_WITHIN);
    }
}

//! Contract 1 messages and validation for newline-delimited JSON.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};

use crate::{Error, Result};

/// Maximum encoded line length, including its terminating newline.
pub const MAX_LINE_BYTES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AddonMessage {
    Hello {
        contract: u32,
        token: String,
        id: String,
        version: String,
    },
    Describe {
        #[serde(default)]
        sources: Vec<Source>,
        #[serde(default)]
        settings: Vec<Setting>,
        #[serde(default)]
        actions: Vec<Action>,
    },
    Transport {
        source: String,
        state: TransportState,
        position: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration: Option<f64>,
    },
    Message {
        level: MessageLevel,
        text: String,
    },
    /// Where the channels of positioned groups are at one moment of the audio.
    Positions {
        source: String,
        /// Nanoseconds on the clock of the Sender packets.
        time: u64,
        #[serde(default)]
        channels: Vec<ChannelPosition>,
    },
    /// A later contract 1 message that this implementation ignores.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    Welcome {
        contract: u32,
        app: String,
        app_version: String,
        settings: BTreeMap<String, Value>,
    },
    Rejected {
        reason: String,
    },
    SetSetting {
        key: String,
        value: Value,
    },
    RunAction {
        id: String,
    },
    OpenFile {
        path: String,
    },
    SelectFormat {
        source: String,
        format: String,
    },
    Transport {
        source: String,
        command: TransportCommand,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<f64>,
    },
    ClipExported {
        files: Vec<String>,
        sample_rate: u32,
        frames: u64,
    },
    Quit,
    /// A later contract 1 message that this implementation ignores.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub sender: String,
    pub formats: Vec<Format>,
    pub format: String,
    /// Absence means one group with numbered channels, determined by the Sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<ChannelGroup>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Format {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelGroup {
    pub id: String,
    pub label: String,
    pub channels: Vec<String>,
    /// The add-on sends `positions` for the channels of this group.
    #[serde(default, skip_serializing_if = "is_false")]
    pub positioned: bool,
}

/// One channel's place in the room. The listener is at 0, 0, 0. `x` runs from -1 (left wall)
/// to 1 (right wall), `y` from -1 (back wall) to 1 (front wall), `z` from -1 (floor) to 1
/// (ceiling). The host clamps values outside that range and drops an entry whose numbers are
/// not finite; neither stops the add-on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelPosition {
    /// Zero-based index into the Sender stream, counted across groups.
    pub channel: u32,
    #[serde(deserialize_with = "number_or_nan")]
    pub x: f64,
    #[serde(deserialize_with = "number_or_nan")]
    pub y: f64,
    #[serde(deserialize_with = "number_or_nan")]
    pub z: f64,
    /// 0 is a point, 1 fills the room. Stored by the host, not rendered yet.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_number_or_nan"
    )]
    pub size: Option<f64>,
    /// A linear level the add-on has not already applied to the audio. Absent means 1.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_number_or_nan"
    )]
    pub gain: Option<f64>,
}

impl ChannelPosition {
    /// A point at `x`, `y`, `z` with no size and a gain of 1.
    pub fn new(channel: u32, x: f64, y: f64, z: f64) -> Self {
        Self {
            channel,
            x,
            y,
            z,
            size: None,
            gain: None,
        }
    }

    /// Whether every number is finite. JSON cannot carry the others: they arrive as `null`.
    pub fn is_finite(&self) -> bool {
        [
            Some(self.x),
            Some(self.y),
            Some(self.z),
            self.size,
            self.gain,
        ]
        .into_iter()
        .flatten()
        .all(f64::is_finite)
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// JSON has no NaN or infinity; serde_json writes them as `null`. Reading `null` back as NaN
/// lets the host drop that one entry instead of failing the whole line.
fn number_or_nan<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(deserializer)?.unwrap_or(f64::NAN))
}

fn optional_number_or_nan<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<f64>, D::Error> {
    Ok(Some(
        Option::<f64>::deserialize(deserializer)?.unwrap_or(f64::NAN),
    ))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transport {
    pub state: TransportState,
    pub position: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportCommand {
    Play,
    Pause,
    Stop,
    Seek,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(flatten)]
    pub kind: SettingKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingKind {
    Toggle {
        default: bool,
    },
    Choice {
        default: String,
        options: Vec<ChoiceOption>,
    },
    Number {
        default: f64,
        min: f64,
        max: f64,
        step: f64,
    },
    Text {
        default: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub label: String,
}

impl AddonMessage {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Hello { id, version, .. } => {
                addon_id(id)?;
                text_limit("hello.version", version, 32)?;
                if version.chars().any(char::is_control) {
                    return invalid("hello.version must not contain control characters");
                }
            }
            Self::Describe {
                sources,
                settings,
                actions,
            } => {
                for source in sources {
                    source.validate()?;
                }
                count_limit("describe.settings", settings.len(), 32)?;
                for setting in settings {
                    setting.validate()?;
                }
                count_limit("describe.actions", actions.len(), 16)?;
                for action in actions {
                    text_limit("action.label", &action.label, 40)?;
                }
            }
            Self::Transport {
                source,
                position,
                duration,
                ..
            } => {
                source_id("transport.source", source)?;
                seconds("transport.position", *position)?;
                if let Some(duration) = duration {
                    seconds("transport.duration", *duration)?;
                }
            }
            Self::Message { text, .. } => text_limit("message.text", text, 300)?,
            Self::Positions {
                source, channels, ..
            } => {
                source_id("positions.source", source)?;
                count_limit("positions.channels", channels.len(), 64)?;
            }
            Self::Unknown => {}
        }
        Ok(())
    }
}

impl HostMessage {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Welcome { settings, .. } => {
                for key in settings.keys() {
                    setting_key(key)?;
                }
            }
            Self::SetSetting { key, .. } => setting_key(key)?,
            Self::OpenFile { path } => absolute_path("open_file.path", path)?,
            Self::SelectFormat { source, .. } => source_id("select_format.source", source)?,
            Self::Transport {
                source,
                command,
                position,
            } => {
                source_id("transport.source", source)?;
                if *command == TransportCommand::Seek && position.is_none() {
                    return invalid("transport seek requires position in seconds");
                }
                if let Some(position) = position {
                    seconds("transport.position", *position)?;
                }
            }
            Self::ClipExported { files, .. } => {
                for path in files {
                    absolute_path("clip_exported.files", path)?;
                }
            }
            Self::Rejected { .. } | Self::RunAction { .. } | Self::Quit | Self::Unknown => {}
        }
        Ok(())
    }
}

impl Source {
    pub fn validate(&self) -> Result<()> {
        source_id("source.id", &self.id)?;
        text_limit("source.name", &self.name, 120)?;
        if self.sender.is_empty()
            || self.sender.len() > 72
            || !self
                .sender
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'-')
        {
            return invalid("source.sender must contain 1 to 72 hex digits or hyphens");
        }
        if self.formats.is_empty() {
            return invalid("source.formats must contain at least one format");
        }
        count_limit("source.formats", self.formats.len(), 32)?;
        for format in &self.formats {
            text_limit("format.label", &format.label, 40)?;
        }
        if !self.formats.iter().any(|format| format.id == self.format) {
            return invalid("source.format must name an entry in source.formats");
        }
        if let Some(groups) = &self.groups {
            if groups.is_empty() {
                return invalid("source.groups must contain at least one group, or be omitted");
            }
            let mut channels = 0_usize;
            for group in groups {
                text_limit("group.label", &group.label, 24)?;
                if group.channels.is_empty() {
                    return invalid("each source group must contain at least one channel");
                }
                channels = channels.saturating_add(group.channels.len());
                for label in &group.channels {
                    text_limit("channel label", label, 24)?;
                }
            }
            count_limit("source channel count", channels, 64)?;
        }
        if let Some(transport) = &self.transport {
            seconds("source.transport.position", transport.position)?;
            if let Some(duration) = transport.duration {
                seconds("source.transport.duration", duration)?;
            }
        }
        Ok(())
    }
}

impl Setting {
    pub fn validate(&self) -> Result<()> {
        setting_key(&self.key)?;
        if let Some(help) = &self.help {
            text_limit("setting.help", help, 200)?;
        }
        match &self.kind {
            SettingKind::Toggle { .. } => {}
            SettingKind::Choice { default, options } => {
                if !options.iter().any(|option| option.value == *default) {
                    return invalid("choice setting default must be one of its options");
                }
            }
            SettingKind::Number {
                default,
                min,
                max,
                step,
            } => {
                if ![default, min, max, step]
                    .iter()
                    .all(|value| value.is_finite())
                {
                    return invalid("number setting default, min, max and step must be finite");
                }
                if min > max {
                    return invalid("number setting min must not exceed max");
                }
                if default < min || default > max {
                    return invalid("number setting default must be between min and max");
                }
                if *step <= 0.0 {
                    return invalid("number setting step must be greater than zero");
                }
            }
            SettingKind::Text { default } => text_limit("text setting default", default, 200)?,
        }
        Ok(())
    }

    /// Check a value against this declaration before applying a setting change.
    pub fn validate_value(&self, value: &Value) -> Result<()> {
        self.validate()?;
        match &self.kind {
            SettingKind::Toggle { .. } if value.is_boolean() => Ok(()),
            SettingKind::Choice { options, .. }
                if value
                    .as_str()
                    .is_some_and(|value| options.iter().any(|option| option.value == value)) =>
            {
                Ok(())
            }
            SettingKind::Number { min, max, .. }
                if value
                    .as_f64()
                    .is_some_and(|value| value.is_finite() && value >= *min && value <= *max) =>
            {
                Ok(())
            }
            SettingKind::Text { .. } if value.is_string() => text_limit(
                "text setting value",
                value.as_str().unwrap_or_default(),
                200,
            ),
            _ => invalid(format!(
                "setting '{}' value does not match its kind or allowed range",
                self.key
            )),
        }
    }
}

pub fn encode_addon(message: &AddonMessage) -> Result<Vec<u8>> {
    message.validate()?;
    encode(message)
}

pub fn decode_addon(line: &[u8]) -> Result<AddonMessage> {
    // Read raw fields so even a number outside f64's range can be discarded on its own.
    if let Ok(fields) = decode::<RawFields>(line) {
        if fields
            .get("type")
            .and_then(|kind| serde_json::from_str::<String>(kind.get()).ok())
            .as_deref()
            == Some("positions")
        {
            return Ok(decode_positions(&fields).unwrap_or(AddonMessage::Unknown));
        }
    }
    let message: AddonMessage = decode(line)?;
    message.validate()?;
    Ok(message)
}

type RawFields<'a> = BTreeMap<String, &'a RawValue>;

fn decode_positions(fields: &RawFields<'_>) -> Option<AddonMessage> {
    let source = serde_json::from_str::<String>(fields.get("source")?.get()).ok()?;
    source_id("positions.source", &source).ok()?;
    let time = whole_number(fields.get("time")?)?;
    let entries: Vec<&RawValue> = match fields.get("channels") {
        Some(array) => serde_json::from_str(array.get()).ok()?,
        None => Vec::new(),
    };
    if entries.len() > 64 {
        return None;
    }
    let channels = entries.into_iter().filter_map(decode_position).collect();
    Some(AddonMessage::Positions {
        source,
        time,
        channels,
    })
}

fn decode_position(entry: &RawValue) -> Option<ChannelPosition> {
    let fields: RawFields = serde_json::from_str(entry.get()).ok()?;
    let finite = |raw: &&RawValue| raw.get().parse::<f64>().ok().filter(|n| n.is_finite());
    let optional = |name: &str| match fields.get(name) {
        Some(raw) => finite(raw).map(Some),
        None => Some(None),
    };
    Some(ChannelPosition {
        channel: u32::try_from(whole_number(fields.get("channel")?)?).ok()?,
        x: finite(fields.get("x")?)?,
        y: finite(fields.get("y")?)?,
        z: finite(fields.get("z")?)?,
        size: optional("size")?,
        gain: optional("gain")?,
    })
}

// Check the decimal digits directly. An f64 would round fractional frames and
// timestamps near u64::MAX before we could check that they are whole and in range.
fn whole_number(raw: &RawValue) -> Option<u64> {
    let text = raw.get();
    if let Ok(integer) = text.parse::<u64>() {
        return Some(integer);
    }
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{integer}{fraction}");
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some(0);
    }
    if text.starts_with('-') {
        return None;
    }
    let coefficient = significant.trim_end_matches('0');
    let zeros = significant.len() - coefficient.len();
    let shift = exponent
        .parse::<i64>()
        .ok()?
        .checked_add(zeros as i64)?
        .checked_sub(fraction.len() as i64)?;
    coefficient
        .parse::<u64>()
        .ok()?
        .checked_mul(10u64.checked_pow(shift.try_into().ok()?)?)
}

pub fn encode_host(message: &HostMessage) -> Result<Vec<u8>> {
    message.validate()?;
    encode(message)
}

pub fn decode_host(line: &[u8]) -> Result<HostMessage> {
    let message: HostMessage = decode(line)?;
    message.validate()?;
    Ok(message)
}

fn encode(message: &impl Serialize) -> Result<Vec<u8>> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    if line.len() > MAX_LINE_BYTES {
        return invalid("message exceeds the 65,536-byte line limit, including newline");
    }
    Ok(line)
}

fn decode<'de, T: Deserialize<'de>>(line: &'de [u8]) -> Result<T> {
    let terminated = line.last() == Some(&b'\n');
    if line.len().saturating_add(usize::from(!terminated)) > MAX_LINE_BYTES {
        return invalid("message exceeds the 65,536-byte line limit, including newline");
    }
    let payload = if terminated {
        &line[..line.len() - 1]
    } else {
        line
    };
    if payload.contains(&b'\n') || payload.contains(&b'\r') {
        return invalid("message must be a single JSON line");
    }
    Ok(serde_json::from_slice(payload)?)
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::new(message))
}

fn text_limit(field: &str, text: &str, max: usize) -> Result<()> {
    if text.chars().count() > max {
        return invalid(format!("{field} must contain at most {max} characters"));
    }
    Ok(())
}

fn count_limit(field: &str, count: usize, max: usize) -> Result<()> {
    if count > max {
        return invalid(format!("{field} must contain at most {max} entries"));
    }
    Ok(())
}

fn addon_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-'))
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
    {
        return invalid("hello.id must be a package id: 1 to 64 lower-case letters, digits, dots or hyphens, starting and ending with a letter or digit");
    }
    Ok(())
}

fn source_id(field: &str, id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return invalid(format!(
            "{field} must contain 1 to 64 letters, digits, dots, hyphens or underscores"
        ));
    }
    Ok(())
}

fn setting_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 40
        || !key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return invalid(
            "setting key must contain 1 to 40 lower-case letters, digits or underscores",
        );
    }
    Ok(())
}

fn seconds(field: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        return invalid(format!(
            "{field} must be a finite, nonnegative number of seconds"
        ));
    }
    Ok(())
}

fn absolute_path(field: &str, path: &str) -> Result<()> {
    // Accept Unix and Windows absolute paths independently of the build platform.
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    if !(path.starts_with('/') || path.starts_with("\\\\") || drive) || path.contains('\0') {
        return invalid(format!(
            "{field} must be an absolute path without NUL characters"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_addon_message_round_trips() {
        let messages = [
            json!({"type":"hello","contract":1,"id":"com.example.tone","version":"1","token":"token"}),
            json!({"type":"describe","sources":[{"id":"main","name":"Tone","sender":"ab-1","formats":[{"id":"stereo","label":"Stereo"}],"format":"stereo","groups":[{"id":"channels","label":"Channels","channels":["L","R"]}],"transport":{"state":"playing","position":0}}],"settings":[{"key":"mute","label":"Mute","kind":"toggle","default":false},{"key":"level","label":"Level","kind":"choice","default":"-12","options":[{"value":"-12","label":"-12 dB"}]},{"key":"frequency","label":"Frequency","kind":"number","default":440,"min":20,"max":20000,"step":1},{"key":"title","label":"Title","kind":"text","default":""}],"actions":[{"id":"restart","label":"Restart"}]}),
            json!({"type":"transport","source":"main","state":"paused","position":3.0,"duration":60.0}),
            json!({"type":"message","level":"error","text":"No audio"}),
            json!({"type":"positions","source":"main","time":8_123_456_789_012_u64,"channels":[{"channel":12,"x":-0.5,"y":0.8,"z":0.0,"size":0.0,"gain":1.0},{"channel":13,"x":1.0,"y":-1.0,"z":0.5}]}),
        ];
        for value in messages {
            let message = decode_addon(&serde_json::to_vec(&value).unwrap()).unwrap();
            let encoded = encode_addon(&message).unwrap();
            assert_eq!(encoded.last(), Some(&b'\n'));
            assert_eq!(decode_addon(&encoded).unwrap(), message);
        }
    }

    #[test]
    fn a_group_is_positioned_only_when_it_says_so() {
        let plain: ChannelGroup =
            serde_json::from_value(json!({"id":"channels","label":"Channels","channels":["L"]}))
                .unwrap();
        assert!(!plain.positioned);
        // An older host reads the same bytes it always did.
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            json!({"id":"channels","label":"Channels","channels":["L"]})
        );
        let positioned: ChannelGroup = serde_json::from_value(
            json!({"id":"objects","label":"Objects","channels":["Object 1"],"positioned":true}),
        )
        .unwrap();
        assert!(positioned.positioned);
        assert_eq!(
            serde_json::to_value(&positioned).unwrap()["positioned"],
            json!(true)
        );
    }

    #[test]
    fn malformed_positions_drop_only_bad_entries() {
        let good = r#"{"channel":2.0,"x":1,"y":0,"z":0}"#;
        for bad in [
            "null",
            "1",
            "[]",
            "{}",
            r#"{"channel":"2","x":0,"y":0,"z":0}"#,
            r#"{"channel":-1,"x":0,"y":0,"z":0}"#,
            r#"{"channel":4294967296,"x":0,"y":0,"z":0}"#,
            r#"{"channel":2.00000000000000001,"x":0,"y":0,"z":0}"#,
            r#"{"channel":2,"x":1e400,"y":0,"z":0}"#,
            r#"{"channel":2,"x":null,"y":0,"z":0}"#,
            r#"{"channel":2,"x":"0.5","y":0,"z":0}"#,
            r#"{"channel":2,"x":0,"y":0}"#,
            r#"{"channel":2,"x":0,"y":0,"z":0,"gain":null}"#,
            r#"{"channel":2,"x":0,"y":0,"z":0,"size":false}"#,
        ] {
            let line = format!(
                r#"{{"type":"positions","source":"main","time":8123456789012.0,"channels":[{bad},{good}]}}"#
            );
            assert_eq!(
                decode_addon(line.as_bytes()).unwrap(),
                AddonMessage::Positions {
                    source: "main".into(),
                    time: 8123456789012,
                    channels: vec![ChannelPosition::new(2, 1., 0., 0.)],
                },
                "{bad}"
            );
        }
        for token in ["NaN", "Infinity"] {
            let line = format!(r#"{{"type":"positions","x":{token}}}"#);
            assert!(decode_addon(line.as_bytes()).is_err());
        }
    }

    #[test]
    fn malformed_positions_ignore_invalid_envelopes() {
        for fields in [
            r#""time":0"#,
            r#""source":"bad id","time":0"#,
            r#""source":"main""#,
            r#""source":"main","time":-1"#,
            r#""source":"main","time":1.5"#,
            r#""source":"main","time":18446744073709551616"#,
            r#""source":"main","time":0,"channels":null"#,
            &format!(
                r#""source":"main","time":0,"channels":[{}]"#,
                ["null"; 65].join(",")
            ),
        ] {
            let line = format!(r#"{{"type":"positions",{fields}}}"#);
            assert_eq!(
                decode_addon(line.as_bytes()).unwrap(),
                AddonMessage::Unknown
            );
        }
        let line = br#"{"type":"positions","source":"main","time":184467440737095516150e-1}"#;
        assert_eq!(
            decode_addon(line).unwrap(),
            AddonMessage::Positions {
                source: "main".into(),
                time: u64::MAX,
                channels: vec![],
            }
        );
    }

    #[test]
    fn positions_carry_optional_size_and_gain_and_survive_numbers_json_cannot_hold() {
        let line = br#"{"type":"positions","source":"main","time":5,"channels":[{"channel":2,"x":null,"y":0.5,"z":0,"gain":null},{"channel":3,"x":0.25,"y":0.5,"z":0}]}"#;
        let AddonMessage::Positions {
            source,
            time,
            channels,
        } = decode_addon(line).unwrap()
        else {
            panic!("expected positions");
        };
        assert_eq!((source.as_str(), time), ("main", 5));
        assert_eq!(channels, [ChannelPosition::new(3, 0.25, 0.5, 0.0)]);

        let many = AddonMessage::Positions {
            source: "main".into(),
            time: 0,
            channels: vec![ChannelPosition::new(0, 0.0, 0.0, 0.0); 65],
        };
        assert!(encode_addon(&many).is_err());
        let bad_source = AddonMessage::Positions {
            source: "not a source id".into(),
            time: 0,
            channels: Vec::new(),
        };
        assert!(encode_addon(&bad_source).is_err());
    }

    #[test]
    fn every_host_message_round_trips() {
        let messages = [
            json!({"type":"welcome","contract":1,"app":"Backtracking","app_version":"1","settings":{"mute":true}}),
            json!({"type":"rejected","reason":"Wrong token"}),
            json!({"type":"set_setting","key":"level","value":"-6"}),
            json!({"type":"run_action","id":"restart"}),
            json!({"type":"open_file","path":"C:/audio/tone.wav"}),
            json!({"type":"select_format","source":"main","format":"5.1"}),
            json!({"type":"transport","source":"main","command":"seek","position":4.0}),
            json!({"type":"clip_exported","files":["/audio/clip.wav"],"sample_rate":48000,"frames":480}),
            json!({"type":"quit"}),
        ];
        for value in messages {
            let message = decode_host(&serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(
                decode_host(&encode_host(&message).unwrap()).unwrap(),
                message
            );
        }
    }

    #[test]
    fn unknown_types_and_nested_fields_are_ignored() {
        assert_eq!(
            decode_addon(br#"{"type":"future","content":[null,{"deep":true}]}"#).unwrap(),
            AddonMessage::Unknown
        );
        assert_eq!(
            decode_host(br#"{"type":"future","content":{"deep":true}}"#).unwrap(),
            HostMessage::Unknown
        );
        let message = decode_addon(br#"{"type":"describe","future":true,"sources":[{"id":"main","name":"Tone","sender":"ab","formats":[{"id":"stereo","label":"Stereo","future":true}],"format":"stereo","future":42}]}"#).unwrap();
        let AddonMessage::Describe {
            settings, actions, ..
        } = message
        else {
            panic!("expected describe")
        };
        assert!(settings.is_empty());
        assert!(actions.is_empty());
        assert!(decode_host(br#"{"type":"quit","future":true}"#).is_ok());
        assert!(decode_host(br#"{"future":true}"#).is_err());
        assert!(decode_host(br#"{"type":42}"#).is_err());
    }

    #[test]
    fn line_size_and_single_line_framing_are_enforced() {
        let prefix = b"{\"type\":\"future\",\"padding\":\"";
        let suffix = b"\"}\n";
        let mut line = prefix.to_vec();
        line.resize(MAX_LINE_BYTES - suffix.len(), b'a');
        line.extend_from_slice(suffix);
        assert_eq!(line.len(), 65536);
        assert_eq!(decode_addon(&line).unwrap(), AddonMessage::Unknown);
        line.insert(prefix.len(), b'a');
        assert!(decode_addon(&line).is_err());
        assert!(decode_host(b"{\n\"type\":\"quit\"}\n").is_err());
        assert!(decode_addon(&[0xff]).is_err());
        let huge = HostMessage::Rejected {
            reason: "a".repeat(65536),
        };
        assert!(encode_host(&huge).is_err());
        assert!(
            decode_addon(br#"{"type":"message","level":"info","text":"escaped\nline"}"#).is_ok()
        );
    }

    #[test]
    fn invalid_utf8_in_ignored_fields_is_rejected() {
        assert!(decode_addon(b"{\"type\":\"future\",\"ignored\":\"\xff\"}").is_err());
        assert!(decode_host(b"{\"type\":\"quit\",\"ignored\":\"\xff\"}").is_err());
    }

    fn describe_with_source() -> serde_json::Value {
        json!({"type":"describe","sources":[{"id":"main","name":"Tone","sender":"ab-1","formats":[{"id":"stereo","label":"Stereo"}],"format":"stereo","groups":[{"id":"channels","label":"Channels","channels":["L","R"]}]}]})
    }

    #[test]
    fn source_limits_are_checked() {
        let cases = [
            ("id", json!("a".repeat(65))),
            ("id", json!("bad/id")),
            ("name", json!("a".repeat(121))),
            ("sender", json!("a".repeat(73))),
            ("sender", json!("not-hex")),
            ("formats", json!([])),
            (
                "formats",
                json!(vec![json!({"id":"stereo","label":"Stereo"}); 33]),
            ),
            ("formats", json!([{"id":"stereo","label":"a".repeat(41)}])),
            ("format", json!("missing")),
            ("groups", json!([])),
            (
                "groups",
                json!([{"id":"channels","label":"a".repeat(25),"channels":["L"]}]),
            ),
            (
                "groups",
                json!([{"id":"channels","label":"Channels","channels":["a".repeat(25)]}]),
            ),
            (
                "groups",
                json!([{"id":"channels","label":"Channels","channels":[]}]),
            ),
            (
                "groups",
                json!([{"id":"channels","label":"Channels","channels":vec!["L";65]}]),
            ),
        ];
        for (field, value) in cases {
            let mut message = describe_with_source();
            message["sources"][0][field] = value;
            assert!(
                decode_addon(&serde_json::to_vec(&message).unwrap()).is_err(),
                "accepted invalid source {field}: {message}"
            );
        }
        let mut message = describe_with_source();
        message["sources"][0]["name"] = json!("音".repeat(120));
        assert!(decode_addon(&serde_json::to_vec(&message).unwrap()).is_ok());
    }

    #[test]
    fn setting_limits_are_checked() {
        let base = json!({"key":"title","label":"Title","kind":"text","default":""});
        for (field, value) in [
            ("key", json!("a".repeat(41))),
            ("key", json!("Upper")),
            ("help", json!("a".repeat(201))),
            ("default", json!("a".repeat(201))),
        ] {
            let mut setting = base.clone();
            setting[field] = value;
            let message = json!({"type":"describe","settings":[setting]});
            assert!(
                decode_addon(&serde_json::to_vec(&message).unwrap()).is_err(),
                "accepted invalid {field}"
            );
        }
        for setting in [
            json!({"key":"level","label":"Level","kind":"choice","default":"missing","options":[{"value":"-12","label":"-12 dB"}]}),
            json!({"key":"n","label":"Number","kind":"number","default":5,"min":10,"max":20,"step":1}),
            json!({"key":"n","label":"Number","kind":"number","default":5,"min":0,"max":20,"step":0}),
            json!({"key":"n","label":"Number","kind":"number","default":5,"min":10,"max":0,"step":1}),
        ] {
            assert!(decode_addon(
                &serde_json::to_vec(&json!({"type":"describe","settings":[setting]})).unwrap()
            )
            .is_err());
        }
        assert!(decode_addon(
            &serde_json::to_vec(&json!({"type":"describe","settings":vec![base;33]})).unwrap()
        )
        .is_err());
        let action = json!({"id":"restart","label":"Restart"});
        assert!(decode_addon(
            &serde_json::to_vec(&json!({"type":"describe","actions":vec![action;17]})).unwrap()
        )
        .is_err());
        assert!(decode_addon(
            &serde_json::to_vec(
                &json!({"type":"describe","actions":[{"id":"restart","label":"a".repeat(41)}]})
            )
            .unwrap()
        )
        .is_err());
    }

    #[test]
    fn command_and_message_limits_are_checked() {
        assert!(decode_addon(
            &serde_json::to_vec(&json!({"type":"message","level":"info","text":"a".repeat(301)}))
                .unwrap()
        )
        .is_err());
        assert!(decode_addon(br#"{"type":"message","level":"info","text":""}"#).is_ok());
        assert!(decode_addon(
            br#"{"type":"transport","source":"main","state":"playing","position":-1}"#
        )
        .is_err());
        assert!(decode_host(br#"{"type":"transport","source":"main","command":"seek"}"#).is_err());
        assert!(decode_host(br#"{"type":"open_file","path":"relative.wav"}"#).is_err());
        assert!(decode_host(
            br#"{"type":"clip_exported","files":["relative.wav"],"sample_rate":48000,"frames":1}"#
        )
        .is_err());
        assert!(decode_host(br#"{"type":"set_setting","key":"Bad","value":false}"#).is_err());
    }

    #[test]
    fn declared_setting_values_are_checked() {
        let declarations = [
            (
                json!({"key":"mute","label":"Mute","kind":"toggle","default":false}),
                json!(true),
                json!("true"),
            ),
            (
                json!({"key":"level","label":"Level","kind":"choice","default":"-12","options":[{"value":"-12","label":"Quiet"}]}),
                json!("-12"),
                json!("missing"),
            ),
            (
                json!({"key":"frequency","label":"Frequency","kind":"number","default":440,"min":20,"max":20000,"step":1}),
                json!(20),
                json!(20001),
            ),
            (
                json!({"key":"title","label":"Title","kind":"text","default":""}),
                json!("音".repeat(200)),
                json!("音".repeat(201)),
            ),
        ];
        for (declaration, valid, invalid) in declarations {
            let setting: Setting = serde_json::from_value(declaration).unwrap();
            assert!(setting.validate_value(&valid).is_ok());
            assert!(setting.validate_value(&invalid).is_err());
            assert!(setting.validate_value(&Value::Null).is_err());
        }
    }

    #[test]
    fn documented_boundaries_are_accepted() {
        let mut message = describe_with_source();
        let source = &mut message["sources"][0];
        source["id"] = json!("a".repeat(64));
        source["name"] = json!("音".repeat(120));
        source["sender"] = json!("a".repeat(72));
        source["formats"] = json!(vec![json!({"id":"stereo","label":"音".repeat(40)}); 32]);
        source["groups"] =
            json!([{"id":"channels","label":"音".repeat(24),"channels":vec!["音".repeat(24);64]}]);
        let setting = json!({"key":"a".repeat(40),"label":"Title","kind":"text","default":"音".repeat(200),"help":"音".repeat(200),"future":true});
        message["settings"] = json!(vec![setting; 32]);
        message["actions"] = json!(vec![json!({"id":"restart","label":"音".repeat(40)}); 16]);
        assert!(decode_addon(&serde_json::to_vec(&message).unwrap()).is_ok());
        assert!(decode_addon(
            &serde_json::to_vec(&json!({"type":"message","level":"info","text":"音".repeat(300)}))
                .unwrap()
        )
        .is_ok());
    }
}

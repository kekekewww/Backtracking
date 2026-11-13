use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use backtracking_core::{
    AutoResumeDelay, ChannelRoute, DurationChoice, WavExportProfile, WavSampleFormat,
};

use crate::controller::{CaptureSource, SourceSettings};

const MAX_VALUE_BYTES: u64 = 128 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
pub(crate) const DEFAULT_ACCENT: [u8; 3] = [103, 231, 181];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeterMode {
    Dbfs,
    Vu,
    Lufs,
}

impl MeterMode {
    pub(crate) fn unit(self) -> &'static str {
        match self {
            Self::Dbfs => "dBFS",
            Self::Vu => "VU",
            Self::Lufs => "LUFS",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Key {
    History,
    IdleResume,
    Transparency,
    Accent,
    Meter,
    MeterB,
    SplitWaveform,
    CaptureSources,
    WavSampleRate,
    WavSampleFormat,
    WavForceMono,
    Output,
    SavedSnapshots,
    AlwaysOnTop,
    WaveformOnly,
    WaveformBorder,
    WaveformBorderColor,
    WaveformBorderFollowsBackground,
    BarLines,
    SecondsLines,
    SecondsInterval,
    SnapshotTarget,
    WindowPosition,
}

impl Key {
    fn filename(self) -> &'static str {
        match self {
            Self::History => "history-seconds",
            // The old preference used 0 for Off.
            Self::IdleResume => "idle-resume-delay-seconds",
            Self::Transparency => "transparency-percent",
            Self::Accent => "theme-color",
            Self::Meter => "meter-unit",
            Self::MeterB => "meter-b-unit",
            Self::SplitWaveform => "split-waveform",
            Self::CaptureSources => "capture-sources",
            Self::WavSampleRate => "wav-sample-rate",
            Self::WavSampleFormat => "wav-sample-format",
            Self::WavForceMono => "wav-force-mono",
            Self::Output => "output-folder",
            Self::SavedSnapshots => "saved-snapshots-folder",
            Self::AlwaysOnTop => "always-on-top",
            Self::WaveformOnly => "waveform-only",
            Self::WaveformBorder => "waveform-border",
            Self::WaveformBorderColor => "waveform-border-color",
            Self::WaveformBorderFollowsBackground => "waveform-border-follows-background",
            Self::BarLines => "show-bar-lines",
            Self::SecondsLines => "show-second-lines",
            Self::SecondsInterval => "second-line-interval",
            Self::SnapshotTarget => "snapshot-target",
            Self::WindowPosition => "window-position",
        }
    }
}

pub(crate) struct Settings {
    pub(crate) history: DurationChoice,
    pub(crate) idle_resume: AutoResumeDelay,
    pub(crate) transparency: u8,
    pub(crate) accent: [u8; 3],
    pub(crate) meter_mode: MeterMode,
    pub(crate) meter_b_mode: MeterMode,
    pub(crate) split_waveform: bool,
    pub(crate) sources: [Option<SourceSettings>; 2],
    pub(crate) wav_profile: WavExportProfile,
    pub(crate) wav_force_mono: bool,
    pub(crate) output: PathBuf,
    pub(crate) saved_output: PathBuf,
    pub(crate) pinned: bool,
    pub(crate) waveform_only: bool,
    pub(crate) waveform_border: bool,
    pub(crate) waveform_border_color: Option<[u8; 3]>,
    pub(crate) waveform_border_follows_background: bool,
    pub(crate) bar_lines: bool,
    pub(crate) second_lines: bool,
    pub(crate) second_interval: u8,
    pub(crate) snapshot_target: Option<String>,
    pub(crate) window_position: Option<[i32; 2]>,
}

impl Default for Settings {
    fn default() -> Self {
        let directory = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Backtracking");
        Self {
            history: DurationChoice::Seconds20,
            idle_resume: AutoResumeDelay::default(),
            transparency: 0,
            accent: DEFAULT_ACCENT,
            meter_mode: MeterMode::Dbfs,
            meter_b_mode: MeterMode::Dbfs,
            split_waveform: false,
            sources: [
                Some(SourceSettings {
                    source: CaptureSource::System,
                    route: ChannelRoute::Merged,
                }),
                None,
            ],
            wav_profile: WavExportProfile::default(),
            wav_force_mono: false,
            output: directory.join("output"),
            saved_output: directory.join("saved"),
            pinned: true,
            waveform_only: false,
            waveform_border: false,
            waveform_border_color: None,
            waveform_border_follows_background: false,
            bar_lines: false,
            second_lines: false,
            second_interval: 5,
            snapshot_target: None,
            window_position: None,
        }
    }
}

pub(crate) struct Preferences {
    directory: Option<PathBuf>,
}

impl Preferences {
    pub(crate) fn for_current_user() -> Self {
        let directory = ["LOCALAPPDATA", "APPDATA"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .find(|path| path.is_absolute())
            .map(|path| path.join("Backtracking").join("Settings"));
        Self { directory }
    }

    pub(crate) fn load(&self) -> Settings {
        let mut settings = Settings::default();

        if let Some(history) = self
            .read_number(Key::History)
            .and_then(DurationChoice::from_seconds)
        {
            settings.history = history;
        }
        if let Some(delay) = self
            .read_number(Key::IdleResume)
            .and_then(AutoResumeDelay::from_seconds)
        {
            settings.idle_resume = delay;
        }
        if let Some(percent @ (0 | 25 | 50 | 75 | 100)) = self.read_number(Key::Transparency) {
            settings.transparency = percent;
        }
        if let Some(accent) = self.read(Key::Accent).as_deref().and_then(parse_accent) {
            settings.accent = accent;
        }
        settings.meter_mode = self.meter_mode(Key::Meter);
        settings.meter_b_mode = self.meter_mode(Key::MeterB);
        settings.split_waveform = self.read_number::<u8>(Key::SplitWaveform) == Some(1);
        if let Some(sources) = self
            .read(Key::CaptureSources)
            .as_deref()
            .and_then(parse_sources)
        {
            settings.sources = sources;
        }
        if let Some(rate @ (44_100 | 48_000 | 96_000)) = self.read_number(Key::WavSampleRate) {
            settings.wav_profile.sample_rate = rate;
        }
        settings.wav_profile.sample_format =
            match self.read(Key::WavSampleFormat).as_deref().map(str::trim) {
                Some("pcm16") => WavSampleFormat::Pcm16,
                Some("pcm24") => WavSampleFormat::Pcm24,
                _ => WavSampleFormat::Float32,
            };
        settings.wav_force_mono = self.read_number::<u8>(Key::WavForceMono) == Some(1);
        if let Some(path) = self.read_absolute_path(Key::Output) {
            settings.output = path;
        }
        if let Some(path) = self.read_absolute_path(Key::SavedSnapshots) {
            settings.saved_output = path;
        }
        match self.read_number(Key::AlwaysOnTop) {
            Some(0) => settings.pinned = false,
            Some(1) => settings.pinned = true,
            _ => {}
        }
        settings.waveform_only = self.read_number::<u8>(Key::WaveformOnly) == Some(1);
        settings.waveform_border = self.read_number::<u8>(Key::WaveformBorder) == Some(1);
        settings.waveform_border_color = self
            .read(Key::WaveformBorderColor)
            .as_deref()
            .and_then(parse_accent);
        settings.waveform_border_follows_background =
            self.read_number::<u8>(Key::WaveformBorderFollowsBackground) == Some(1);
        settings.bar_lines = self.read_number::<u8>(Key::BarLines) == Some(1);
        settings.second_lines = self.read_number::<u8>(Key::SecondsLines) == Some(1);
        if let Some(interval @ (1 | 5 | 10)) = self.read_number(Key::SecondsInterval) {
            settings.second_interval = interval;
        }
        settings.snapshot_target = self.read(Key::SnapshotTarget).filter(|id| {
            !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)
        });
        settings.window_position = self.read(Key::WindowPosition).and_then(|value| {
            let mut parts = value.split_whitespace();
            let x = parts.next()?.parse().ok()?;
            let y = parts.next()?.parse().ok()?;
            parts.next().is_none().then_some([x, y])
        });
        settings
    }

    fn read_number<T: std::str::FromStr>(&self, key: Key) -> Option<T> {
        self.read(key)?.trim().parse().ok()
    }

    fn meter_mode(&self, key: Key) -> MeterMode {
        match self.read(key).as_deref().map(str::trim) {
            Some("VU") => MeterMode::Vu,
            Some("LUFS") => MeterMode::Lufs,
            _ => MeterMode::Dbfs,
        }
    }

    fn read_absolute_path(&self, key: Key) -> Option<PathBuf> {
        let path = self.read(key)?;
        if path.bytes().any(|byte| byte < 32) {
            return None;
        }
        let path = PathBuf::from(path);
        path.is_absolute().then_some(path)
    }

    fn read(&self, key: Key) -> Option<String> {
        let path = self.directory.as_ref()?.join(key.filename());
        let mut data = Vec::new();
        File::open(path)
            .ok()?
            .take(MAX_VALUE_BYTES + 1)
            .read_to_end(&mut data)
            .ok()?;
        if data.len() as u64 > MAX_VALUE_BYTES {
            return None;
        }
        String::from_utf8(data).ok()
    }

    pub(crate) fn save(&self, key: Key, value: &str) -> io::Result<()> {
        let directory = self.directory.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Windows app data directory unavailable",
            )
        })?;
        fs::create_dir_all(directory)?;
        let target = directory.join(key.filename());
        // Only replace this preference, leaving concurrent changes to the others intact.
        let (temporary, mut file) = loop {
            let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let temporary = directory.join(format!(
                ".{}-{}-{sequence}.tmp",
                key.filename(),
                std::process::id()
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        };
        let written = file
            .write_all(value.as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);
        let result = written.and_then(|()| fs::rename(&temporary, target));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

pub(crate) fn wav_sample_format_preference(format: WavSampleFormat) -> &'static str {
    match format {
        WavSampleFormat::Pcm16 => "pcm16",
        WavSampleFormat::Pcm24 => "pcm24",
        WavSampleFormat::Float32 => "float32",
    }
}

pub(crate) fn serialize_sources(sources: &[Option<SourceSettings>; 2]) -> String {
    fn encode_slot(settings: Option<&SourceSettings>) -> String {
        let Some(settings) = settings else {
            return "off".into();
        };
        let route = settings.route as u32;
        match &settings.source {
            CaptureSource::System => format!("{route}\tsystem"),
            CaptureSource::DefaultMicrophone => format!("{route}\tdefault-mic"),
            CaptureSource::Microphone { id, name } => {
                format!("{route}\tmic\t{}\t{}", encode_text(id), encode_text(name))
            }
            CaptureSource::Application { path, name } => {
                format!("{route}\tapp\t{}\t{}", encode_text(path), encode_text(name))
            }
            CaptureSource::Sender { id, name } => {
                format!("{route}\tsender\t{}\t{}", encode_text(id), encode_text(name))
            }
        }
    }
    format!(
        "{}\n{}",
        encode_slot(sources[0].as_ref()),
        encode_slot(sources[1].as_ref()),
    )
}

fn parse_sources(value: &str) -> Option<[Option<SourceSettings>; 2]> {
    fn parse_slot(line: &str) -> Option<SourceSettings> {
        let mut fields = line.split('\t');
        let route = ChannelRoute::from_code(fields.next()?.parse().ok()?)?;
        let source = match fields.next()? {
            "system" => CaptureSource::System,
            "default-mic" => CaptureSource::DefaultMicrophone,
            "mic" => CaptureSource::Microphone {
                id: decode_text(fields.next()?)?,
                name: decode_text(fields.next()?)?,
            },
            "app" => CaptureSource::Application {
                path: decode_text(fields.next()?)?,
                name: decode_text(fields.next()?)?,
            },
            "sender" => CaptureSource::Sender {
                id: decode_text(fields.next()?)?,
                name: decode_text(fields.next()?)?,
            },
            _ => return None,
        };
        fields
            .next()
            .is_none()
            .then_some(SourceSettings { source, route })
    }
    let (a, b) = value.split_once('\n')?;
    Some([
        Some(parse_slot(a)?),
        if b == "off" {
            None
        } else {
            Some(parse_slot(b)?)
        },
    ])
}

fn encode_text(value: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn decode_text(value: &str) -> Option<String> {
    let (bytes, remainder) = value.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let bytes = bytes
        .iter()
        .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    if bytes.is_empty() || bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn parse_accent(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let [_, r, g, b] = u32::from_str_radix(hex, 16).ok()?.to_be_bytes();
    Some([r, g, b])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_wav_export_defaults_off_and_persists_changes() {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "Backtracking-mono-preference-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let preferences = Preferences {
            directory: Some(directory.clone()),
        };

        assert!(!preferences.load().wav_force_mono);
        preferences.save(Key::WavForceMono, "1").unwrap();
        assert!(preferences.load().wav_force_mono);
        preferences.save(Key::WavForceMono, "0").unwrap();
        assert!(!preferences.load().wav_force_mono);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn persisted_lanes_keep_unicode_source_identity_and_independent_routes() {
        let source = CaptureSource::Application {
            path: r"C:\瀏覽器\chrome.exe".into(),
            name: "Chrome\t瀏覽器".into(),
        };
        let sources = [
            Some(SourceSettings {
                source: source.clone(),
                route: ChannelRoute::Left,
            }),
            Some(SourceSettings {
                source,
                route: ChannelRoute::Right,
            }),
        ];
        assert_eq!(parse_sources(&serialize_sources(&sources)), Some(sources));
        let one_source = Settings::default().sources;
        assert_eq!(
            parse_sources(&serialize_sources(&one_source)),
            Some(one_source)
        );
        assert_eq!(parse_sources("off\noff"), None);
    }

    #[test]
    fn same_name_sender_choices_keep_separate_ids_after_reload() {
        let sources = ["sender-one", "sender-two"].map(|id| {
            Some(SourceSettings {
                source: CaptureSource::Sender {
                    id: id.into(),
                    name: "Shared name".into(),
                },
                route: ChannelRoute::Merged,
            })
        });
        assert_eq!(parse_sources(&serialize_sources(&sources)), Some(sources));
    }
}

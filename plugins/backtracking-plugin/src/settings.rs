use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, Ordering};

use backtracking_core::{AutoResumeDelay, DurationChoice, WavExportProfile, WavSampleFormat};
use nice_plug::params::persist::{PersistentField, deserialize_field, serialize_field};

pub(crate) const TRANSPARENCY_LEVELS: [u8; 5] = [0, 25, 50, 75, 100];
pub(crate) const DEFAULT_ACCENT: [u8; 3] = [103, 231, 181];
const AUTO_RESUME_FIELD: &str = "auto_resume_delay_seconds";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeterMode {
    Dbfs,
    Vu,
    Lufs,
}

impl MeterMode {
    pub(crate) const ALL: [Self; 3] = [Self::Dbfs, Self::Vu, Self::Lufs];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Dbfs => "dBFS",
            Self::Vu => "VU",
            Self::Lufs => "LUFS",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WaveformLayout {
    #[default]
    Merged,
    Stereo,
}

#[cfg(feature = "editor")]
impl WaveformLayout {
    pub(crate) const ALL: [Self; 2] = [Self::Merged, Self::Stereo];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Merged => "Merged",
            Self::Stereo => "Left / Right",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CaptureInput {
    #[default]
    Main,
    Sidechain,
    Both,
}

impl CaptureInput {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Main => "Main",
            Self::Sidechain => "Sidechain",
            Self::Both => "Main + Sidechain",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionSettings {
    pub(crate) retention: DurationChoice,
    pub(crate) retain_silence: bool,
    pub(crate) idle_resume: AutoResumeDelay,
    pub(crate) panel_transparency: u8,
    pub(crate) accent: [u8; 3],
    pub(crate) meter_mode: MeterMode,
    pub(crate) wav_profile: WavExportProfile,
    pub(crate) force_mono: bool,
    pub(crate) show_waveform_only: bool,
    pub(crate) waveform_layout: WaveformLayout,
    pub(crate) show_bar_lines: bool,
    pub(crate) show_seconds_lines: bool,
    pub(crate) seconds_interval: u32,
    pub(crate) border_enabled: bool,
    pub(crate) border_color: Option<[u8; 3]>,
    pub(crate) border_follows_background: bool,
    pub(crate) capture_input: CaptureInput,
    pub(crate) sender_enabled: bool,
    pub(crate) sender_name: String,
    pub(crate) sender_id: Option<String>,
    pub(crate) output_dir: PathBuf,
    pub(crate) saved_dir: PathBuf,
}

impl SessionSettings {
    pub(crate) fn requires_continuous_processing(&self) -> bool {
        self.retain_silence || self.sender_enabled
    }
}

impl Default for SessionSettings {
    fn default() -> Self {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir);
        let backtracking = home.join("Backtracking");
        Self {
            retention: DurationChoice::Seconds20,
            retain_silence: false,
            idle_resume: AutoResumeDelay::default(),
            panel_transparency: 0,
            accent: DEFAULT_ACCENT,
            meter_mode: MeterMode::Dbfs,
            wav_profile: WavExportProfile::default(),
            force_mono: false,
            show_waveform_only: false,
            waveform_layout: WaveformLayout::default(),
            show_bar_lines: false,
            show_seconds_lines: false,
            seconds_interval: 5,
            border_enabled: false,
            border_color: None,
            border_follows_background: true,
            capture_input: CaptureInput::default(),
            sender_enabled: false,
            sender_name: String::new(),
            sender_id: None,
            output_dir: backtracking.join("output"),
            saved_dir: backtracking.join("saved"),
        }
    }
}

/// The DAW and editor share settings without locking or serializing capture data.
#[derive(Clone, Default)]
pub(crate) struct SettingsStore(Arc<Mutex<SessionSettings>>, Arc<AtomicBool>);

type PersistedFields = BTreeMap<String, String>;

impl SettingsStore {
    pub(crate) fn lock(&self) -> MutexGuard<'_, SessionSettings> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn continuous_processing_signal(&self) -> Arc<AtomicBool> {
        self.1.clone()
    }
}

impl PersistentField<'_, PersistedFields> for SettingsStore {
    fn set(&self, fields: PersistedFields) {
        let defaults = SessionSettings::default();
        let mut current = self.lock();
        let read_dir = |key| {
            fields
                .get(key)
                .and_then(|value| deserialize_field::<OsString>(value).ok())
                .map(PathBuf::from)
                .filter(|path| {
                    path.is_absolute() && !path.as_os_str().as_encoded_bytes().contains(&0)
                })
        };
        let restored = SessionSettings {
            retention: fields
                .get("history_seconds")
                .and_then(|value| value.parse().ok())
                .and_then(DurationChoice::from_seconds)
                .unwrap_or(defaults.retention),
            retain_silence: fields.get("retain_silence").is_some_and(|value| value == "true"),
            // Older projects used idle_resume_seconds=0 for Off.
            idle_resume: fields
                .get(AUTO_RESUME_FIELD)
                .and_then(|value| value.parse().ok())
                .and_then(AutoResumeDelay::from_seconds)
                .unwrap_or(defaults.idle_resume),
            panel_transparency: fields
                .get("panel_transparency")
                .and_then(|value| value.parse().ok())
                .filter(|value| TRANSPARENCY_LEVELS.contains(value))
                .unwrap_or(defaults.panel_transparency),
            accent: fields
                .get("accent_rgb")
                .and_then(|value| deserialize_field(value).ok())
                .unwrap_or(defaults.accent),
            meter_mode: fields
                .get("meter_unit")
                .and_then(|value| {
                    MeterMode::ALL
                        .into_iter()
                        .find(|mode| mode.label() == value)
                })
                .unwrap_or(defaults.meter_mode),
            wav_profile: WavExportProfile {
                sample_rate: fields
                    .get("wav_sample_rate")
                    .and_then(|value| value.parse().ok())
                    .filter(|value| WavExportProfile::SAMPLE_RATES.contains(value))
                    .unwrap_or(defaults.wav_profile.sample_rate),
                sample_format: fields
                    .get("wav_bits_per_sample")
                    .and_then(|value| value.parse().ok())
                    .and_then(WavSampleFormat::from_bits_per_sample)
                    .unwrap_or(defaults.wav_profile.sample_format),
            },
            force_mono: fields
                .get("wav_force_mono")
                .is_some_and(|value| value == "true"),
            show_waveform_only: fields
                .get("show_waveform_only")
                .is_some_and(|value| value == "true"),
            waveform_layout: if fields
                .get("waveform_layout")
                .is_some_and(|value| value == "stereo")
            {
                WaveformLayout::Stereo
            } else {
                defaults.waveform_layout
            },
            show_bar_lines: fields.get("show_bar_lines").is_some_and(|value| value == "true"),
            show_seconds_lines: fields.get("show_seconds_lines").is_some_and(|value| value == "true"),
            seconds_interval: fields
                .get("seconds_interval")
                .and_then(|value| value.parse().ok())
                .filter(|value| [1, 5, 10].contains(value))
                .unwrap_or(defaults.seconds_interval),
            border_enabled: fields.get("border_enabled").is_some_and(|value| value == "true"),
            border_color: fields.get("border_color").and_then(|value| deserialize_field(value).ok()),
            border_follows_background: fields
                .get("border_follows_background")
                .map_or(defaults.border_follows_background, |value| value == "true"),
            capture_input: match fields.get("capture_input").map(String::as_str) {
                Some("sidechain") => CaptureInput::Sidechain,
                Some("both") => CaptureInput::Both,
                _ => defaults.capture_input,
            },
            sender_enabled: fields.get("sender_enabled").is_some_and(|value| value == "true"),
            sender_name: fields
                .get("sender_name")
                .map_or_else(String::new, |value| value.chars().take(80).collect()),
            sender_id: fields.get("sender_instance_id")
                .filter(|id| {
                    !id.is_empty() && id.len() <= 72
                        && id.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
                })
                .cloned()
                .or_else(|| current.sender_id.clone()),
            output_dir: read_dir("output_dir").unwrap_or(defaults.output_dir),
            saved_dir: read_dir("saved_dir").unwrap_or(defaults.saved_dir),
        };
        *current = restored;
        self.1.store(current.requires_continuous_processing(), Ordering::Release);
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&PersistedFields) -> R,
    {
        let settings = self.lock().clone();
        let mut fields = BTreeMap::from([
            (
                "history_seconds".into(),
                settings.retention.seconds().to_string(),
            ),
            ("retain_silence".into(), settings.retain_silence.to_string()),
            (
                AUTO_RESUME_FIELD.into(),
                settings.idle_resume.seconds().to_string(),
            ),
            (
                "panel_transparency".into(),
                settings.panel_transparency.to_string(),
            ),
            ("meter_unit".into(), settings.meter_mode.label().into()),
            (
                "show_waveform_only".into(),
                settings.show_waveform_only.to_string(),
            ),
            (
                "waveform_layout".into(),
                match settings.waveform_layout {
                    WaveformLayout::Merged => "merged",
                    WaveformLayout::Stereo => "stereo",
                }
                .into(),
            ),
            (
                "wav_sample_rate".into(),
                settings.wav_profile.sample_rate.to_string(),
            ),
            (
                "wav_bits_per_sample".into(),
                settings
                    .wav_profile
                    .sample_format
                    .bits_per_sample()
                    .to_string(),
            ),
            ("wav_force_mono".into(), settings.force_mono.to_string()),
            ("show_bar_lines".into(), settings.show_bar_lines.to_string()),
            ("show_seconds_lines".into(), settings.show_seconds_lines.to_string()),
            ("seconds_interval".into(), settings.seconds_interval.to_string()),
            ("border_enabled".into(), settings.border_enabled.to_string()),
            ("border_follows_background".into(), settings.border_follows_background.to_string()),
            ("capture_input".into(), match settings.capture_input {
                CaptureInput::Main => "main",
                CaptureInput::Sidechain => "sidechain",
                CaptureInput::Both => "both",
            }.into()),
            ("sender_enabled".into(), settings.sender_enabled.to_string()),
            ("sender_name".into(), settings.sender_name),
        ]);
        // The framework's serde codec preserves native paths, including non-Unicode names.
        if let Ok(path) = serialize_field(settings.output_dir.as_os_str()) {
            fields.insert("output_dir".into(), path);
        }
        if let Ok(path) = serialize_field(settings.saved_dir.as_os_str()) {
            fields.insert("saved_dir".into(), path);
        }
        if let Ok(accent) = serialize_field(&settings.accent) {
            fields.insert("accent_rgb".into(), accent);
        }
        if let Some(color) = settings.border_color
            && let Ok(color) = serialize_field(&color)
        {
            fields.insert("border_color".into(), color);
        }
        if let Some(id) = settings.sender_id {
            fields.insert("sender_instance_id".into(), id);
        }
        f(&fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_processing_signal_follows_restore_without_worker_and_stays_instance_local() {
        let settings = SettingsStore::default();
        let signal = settings.continuous_processing_signal();
        let shared = settings.clone();
        let independent = SettingsStore::default();
        assert!(Arc::ptr_eq(&signal, &shared.continuous_processing_signal()));
        assert!(!Arc::ptr_eq(&signal, &independent.continuous_processing_signal()));
        assert!(!signal.load(Ordering::Acquire));

        settings.set(BTreeMap::from([("retain_silence".into(), "true".into())]));
        assert!(signal.load(Ordering::Acquire));
        assert!(!independent.continuous_processing_signal().load(Ordering::Acquire));
        settings.set(BTreeMap::new());
        assert!(!signal.load(Ordering::Acquire));
        shared.set(BTreeMap::from([("retain_silence".into(), "true".into())]));
        assert!(signal.load(Ordering::Acquire));
        shared.set(BTreeMap::from([("retain_silence".into(), "invalid".into())]));
        assert!(!signal.load(Ordering::Acquire));
    }

    #[test]
    fn continuous_processing_signal_combines_sender_and_quiet_settings_atomically() {
        let settings = SettingsStore::default();
        let signal = settings.continuous_processing_signal();
        for (quiet, sender, expected) in [(false, false, false), (false, true, true), (true, true, true),
            (true, false, true), (false, false, false)]
        {
            settings.set(BTreeMap::from([
                ("retain_silence".into(), quiet.to_string()),
                ("sender_enabled".into(), sender.to_string()),
            ]));
            assert_eq!(signal.load(Ordering::Acquire), expected);
            assert_eq!(settings.lock().retain_silence, quiet);
        }
        settings.set(BTreeMap::from([("sender_enabled".into(), "true".into())]));
        assert!(!settings.lock().retain_silence);
        assert!(signal.load(Ordering::Acquire));
    }

    #[test]
    fn quiet_sections_default_off_and_legacy_or_invalid_state_restores_off() {
        let settings = SettingsStore::default();
        assert_eq!(settings.map(|fields| fields.get("retain_silence").cloned()), Some("false".into()));

        settings.set(BTreeMap::from([("retain_silence".into(), "true".into())]));
        assert_eq!(settings.map(|fields| fields.get("retain_silence").cloned()), Some("true".into()));

        settings.set(BTreeMap::from([("history_seconds".into(), "30".into())]));
        assert_eq!(settings.lock().retention, DurationChoice::Seconds30);
        assert_eq!(settings.map(|fields| fields.get("retain_silence").cloned()), Some("false".into()));

        settings.set(BTreeMap::from([("retain_silence".into(), "invalid".into())]));
        assert_eq!(settings.map(|fields| fields.get("retain_silence").cloned()), Some("false".into()));
    }

    #[test]
    fn quiet_sections_round_trips_when_enabled() {
        let original = SettingsStore::default();
        original.set(BTreeMap::from([("retain_silence".into(), "true".into())]));
        let fields = original.map(Clone::clone);
        let restored = SettingsStore::default();
        restored.set(fields);
        assert_eq!(restored.map(|fields| fields.get("retain_silence").cloned()), Some("true".into()));
    }

    #[test]
    fn source_and_waveform_preferences_round_trip() {
        let source = SettingsStore::default();
        {
            let mut settings = source.lock();
            settings.retain_silence = true;
            settings.capture_input = CaptureInput::Both;
            settings.sender_enabled = true;
            settings.sender_name = "Rhythm".into();
            settings.sender_id = Some("abcd-1234".into());
            settings.show_bar_lines = true;
            settings.show_seconds_lines = true;
            settings.seconds_interval = 10;
            settings.border_enabled = true;
            settings.border_color = Some([210, 180, 50]);
            settings.border_follows_background = false;
        }
        let restored = SettingsStore::default();
        restored.set(source.map(Clone::clone));
        assert_eq!(*restored.lock(), *source.lock());
    }
}

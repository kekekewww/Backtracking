use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use backtracking_core::{DurationChoice, WavExportProfile, WavSampleFormat};
use nice_plug::params::persist::{PersistentField, deserialize_field, serialize_field};

pub(crate) const TRANSPARENCY_LEVELS: [u8; 5] = [0, 25, 50, 75, 100];

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionSettings {
    pub(crate) retention: DurationChoice,
    pub(crate) idle_resume: Option<DurationChoice>,
    pub(crate) panel_transparency: u8,
    pub(crate) meter_mode: MeterMode,
    pub(crate) wav_profile: WavExportProfile,
    pub(crate) output_dir: PathBuf,
}

impl Default for SessionSettings {
    fn default() -> Self {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir);
        Self {
            retention: DurationChoice::Seconds20,
            idle_resume: Some(DurationChoice::Seconds10),
            panel_transparency: 0,
            meter_mode: MeterMode::Dbfs,
            wav_profile: WavExportProfile::default(),
            output_dir: home.join("Backtracking").join("output"),
        }
    }
}

/// The DAW and editor share settings without locking or serializing capture data.
#[derive(Clone, Default)]
pub(crate) struct SettingsStore(Arc<Mutex<SessionSettings>>);

type PersistedFields = BTreeMap<String, String>;

impl SettingsStore {
    pub(crate) fn lock(&self) -> MutexGuard<'_, SessionSettings> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl PersistentField<'_, PersistedFields> for SettingsStore {
    fn set(&self, fields: PersistedFields) {
        let defaults = SessionSettings::default();
        let read_duration = |key: &str| match fields.get(key).map(String::as_str) {
            Some("10") => Some(DurationChoice::Seconds10),
            Some("20") => Some(DurationChoice::Seconds20),
            Some("30") => Some(DurationChoice::Seconds30),
            _ => None,
        };
        let output_dir = fields
            .get("output_dir")
            .and_then(|value| deserialize_field::<OsString>(value).ok())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && !path.as_os_str().as_encoded_bytes().contains(&0));
        let restored = SessionSettings {
            retention: read_duration("history_seconds").unwrap_or(defaults.retention),
            idle_resume: if fields
                .get("idle_resume_seconds")
                .is_some_and(|value| value == "0")
            {
                None
            } else {
                read_duration("idle_resume_seconds").or(defaults.idle_resume)
            },
            panel_transparency: fields
                .get("panel_transparency")
                .and_then(|value| value.parse().ok())
                .filter(|value| TRANSPARENCY_LEVELS.contains(value))
                .unwrap_or(defaults.panel_transparency),
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
            output_dir: output_dir.unwrap_or(defaults.output_dir),
        };
        *self.lock() = restored;
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
            (
                "idle_resume_seconds".into(),
                settings
                    .idle_resume
                    .map_or(0, DurationChoice::seconds)
                    .to_string(),
            ),
            (
                "panel_transparency".into(),
                settings.panel_transparency.to_string(),
            ),
            ("meter_unit".into(), settings.meter_mode.label().into()),
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
        ]);
        // The framework's serde codec preserves native paths, including non-Unicode names.
        if let Ok(path) = serialize_field(settings.output_dir.as_os_str()) {
            fields.insert("output_dir".into(), path);
        }
        f(&fields)
    }
}

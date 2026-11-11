use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionSettings {
    pub(crate) retention: DurationChoice,
    pub(crate) idle_resume: AutoResumeDelay,
    pub(crate) panel_transparency: u8,
    pub(crate) accent: [u8; 3],
    pub(crate) meter_mode: MeterMode,
    pub(crate) wav_profile: WavExportProfile,
    pub(crate) force_mono: bool,
    pub(crate) show_waveform_only: bool,
    pub(crate) waveform_layout: WaveformLayout,
    pub(crate) output_dir: PathBuf,
    pub(crate) saved_dir: PathBuf,
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
            idle_resume: AutoResumeDelay::default(),
            panel_transparency: 0,
            accent: DEFAULT_ACCENT,
            meter_mode: MeterMode::Dbfs,
            wav_profile: WavExportProfile::default(),
            force_mono: false,
            show_waveform_only: false,
            waveform_layout: WaveformLayout::default(),
            output_dir: backtracking.join("output"),
            saved_dir: backtracking.join("saved"),
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
            output_dir: read_dir("output_dir").unwrap_or(defaults.output_dir),
            saved_dir: read_dir("saved_dir").unwrap_or(defaults.saved_dir),
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
        f(&fields)
    }
}

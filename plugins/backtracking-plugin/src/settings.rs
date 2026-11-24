use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, Ordering};

use backtracking_core::{AutoResumeDelay, DurationChoice, WavExportProfile, WavSampleFormat, cleanup};
use backtracking_core::wide_export::WideExportMode;
use nice_plug::params::persist::{PersistentField, deserialize_field, serialize_field};

pub(crate) const TRANSPARENCY_LEVELS: [u8; 5] = [0, 25, 50, 75, 100];
pub(crate) const DEFAULT_ACCENT: [u8; 3] = [103, 231, 181];
const AUTO_RESUME_FIELD: &str = "auto_resume_delay_seconds";
/// The widths the waveform border can have, in points.
pub(crate) const BORDER_WIDTHS: [u8; 4] = [1, 2, 3, 4];

/// When the capture status light in the toolbar is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum StatusMark {
    #[default]
    Always,
    /// Only while capture is paused.
    Paused,
    Never,
}

impl StatusMark {
    pub(crate) const ALL: [Self; 3] = [Self::Always, Self::Paused, Self::Never];

    #[cfg(feature = "editor")]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Always => "Always",
            Self::Paused => "Only when paused",
            Self::Never => "Never",
        }
    }

    /// The value kept in the project.
    fn stored(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Paused => "paused",
            Self::Never => "never",
        }
    }

    /// Whether the light is drawn while capture is `paused` or not.
    #[cfg(feature = "editor")]
    pub(crate) fn shown(self, paused: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Paused => paused,
            Self::Never => false,
        }
    }
}

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

/// How the waveform is colored: in the theme color, as it always was, or by what each moment
/// of it holds from low to high frequencies, as on the desktop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WaveformColoring {
    #[default]
    Single,
    Frequency,
}

impl WaveformColoring {
    pub(crate) const ALL: [Self; 2] = [Self::Single, Self::Frequency];

    #[cfg(feature = "editor")]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Single => "Single color",
            Self::Frequency => "By frequency",
        }
    }

    /// The value kept in the project, the same as the desktop's preference.
    fn stored(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Frequency => "frequency",
        }
    }
}

/// One color of a [`WaveformGradient`], at `position` from the lowest band (0) to the highest
/// (1).
#[derive(Clone, Copy, Debug)]
pub(crate) struct GradientStop {
    pub(crate) position: f32,
    pub(crate) rgb: [u8; 3],
}

impl PartialEq for GradientStop {
    // Positions are finite and never -0, so their bits compare as the numbers do.
    fn eq(&self, other: &Self) -> bool {
        self.position.to_bits() == other.position.to_bits() && self.rgb == other.rgb
    }
}

impl Eq for GradientStop {}

/// The user's own gradient for coloring by frequency: 2 to 8 stops, sorted by position, as
/// `backtracking_ui::Spectrum` takes them. Kept here, without the editor's types, so that a
/// build without the editor reads and writes the same project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WaveformGradient {
    stops: Vec<GradientStop>,
}

impl WaveformGradient {
    pub(crate) const MIN_STOPS: usize = 2;
    pub(crate) const MAX_STOPS: usize = 8;

    /// Sorts the stops by position, keeping the given order of stops at one position. `None`
    /// unless there are 2 to 8 and each lies in 0 to 1.
    pub(crate) fn new(stops: impl IntoIterator<Item = GradientStop>) -> Option<Self> {
        let mut stops: Vec<_> = stops.into_iter().take(Self::MAX_STOPS + 1).collect();
        if !(Self::MIN_STOPS..=Self::MAX_STOPS).contains(&stops.len())
            || !stops.iter().all(|stop| (0.0..=1.0).contains(&stop.position))
        {
            return None;
        }
        for stop in &mut stops {
            // -0 is 0.
            stop.position += 0.0;
        }
        stops.sort_by(|a, b| a.position.total_cmp(&b.position));
        Some(Self { stops })
    }

    pub(crate) fn stops(&self) -> &[GradientStop] {
        &self.stops
    }

    /// The value kept in the project, in the desktop's `waveform-gradient-v1` form: each stop
    /// as `#rrggbb@position`, lowest first, separated by commas, `#e8302c@0,#2c68ec@1` say.
    fn stored(&self) -> String {
        self.stops
            .iter()
            .map(|stop| {
                let [r, g, b] = stop.rgb;
                format!("#{r:02x}{g:02x}{b:02x}@{}", stop.position)
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Reads [`Self::stored`]'s text, as the desktop does. `None` for anything else.
    fn parse(text: &str) -> Option<Self> {
        let stops = text
            .split(',')
            .map(|stop| {
                let (color, position) = stop.trim().split_once('@')?;
                let hex = color.trim().strip_prefix('#')?;
                if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return None;
                }
                let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
                Some(GradientStop {
                    position: position.trim().parse().ok()?,
                    rgb: [channel(0)?, channel(2)?, channel(4)?],
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Self::new(stops)
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

/// How the L and R rows of a capture wider than stereo sound in preview and on export. The
/// waveform is the same either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PairRender {
    #[default]
    Stereo,
    Binaural,
}

impl PairRender {
    pub(crate) const ALL: [Self; 2] = [Self::Stereo, Self::Binaural];

    #[cfg(feature = "editor")]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stereo => "Stereo",
            Self::Binaural => "Binaural",
        }
    }

    /// The value kept in the project, the same as the desktop's.
    fn stored(self) -> &'static str {
        match self {
            Self::Stereo => "stereo",
            Self::Binaural => "binaural",
        }
    }

    pub(crate) fn binaural(self) -> bool {
        self == Self::Binaural
    }
}

/// The choices of Multichannel sources on the Export page, in the desktop's order.
pub(crate) const WIDE_EXPORT_MODES: [WideExportMode; 4] = [
    WideExportMode::BinauralFiles,
    WideExportMode::MonoFiles,
    WideExportMode::Multichannel,
    WideExportMode::BinauralMix,
];

#[cfg(feature = "editor")]
pub(crate) fn wide_export_label(mode: WideExportMode) -> &'static str {
    match mode {
        WideExportMode::BinauralFiles => "Binaural, one file per channel",
        WideExportMode::MonoFiles => "Mono, one file per channel",
        WideExportMode::Multichannel => "One multichannel file",
        WideExportMode::BinauralMix => "One binaural mix",
    }
}

/// The value kept in the project, the same as the desktop's.
fn wide_export_stored(mode: WideExportMode) -> &'static str {
    match mode {
        WideExportMode::BinauralFiles => "binaural-files",
        WideExportMode::BinauralMix => "binaural-mix",
        WideExportMode::MonoFiles => "mono-files",
        WideExportMode::Multichannel => "multichannel",
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
    /// How the L and R of a capture wider than stereo are played and written.
    pub(crate) pair_render: PairRender,
    pub(crate) preview_as_lr: bool,
    /// The files a capture wider than stereo is saved and dragged out as.
    pub(crate) wide_export: WideExportMode,
    pub(crate) show_waveform_only: bool,
    pub(crate) waveform_layout: WaveformLayout,
    pub(crate) waveform_coloring: WaveformColoring,
    /// The gradient the user made for coloring by frequency, or `None` for the default.
    pub(crate) waveform_gradient: Option<WaveformGradient>,
    pub(crate) show_bar_lines: bool,
    pub(crate) show_seconds_lines: bool,
    pub(crate) seconds_interval: u32,
    pub(crate) border_enabled: bool,
    pub(crate) border_width: u8,
    pub(crate) border_color: Option<[u8; 3]>,
    pub(crate) border_follows_background: bool,
    pub(crate) show_title: bool,
    pub(crate) show_meter: bool,
    pub(crate) status_mark: StatusMark,
    pub(crate) capture_input: CaptureInput,
    pub(crate) sender_enabled: bool,
    pub(crate) sender_name: String,
    pub(crate) sender_id: Option<String>,
    pub(crate) output_dir: PathBuf,
    pub(crate) saved_dir: PathBuf,
    /// The age in days at which the editor deletes its own old files from `output_dir`.
    /// `None` keeps them all.
    pub(crate) output_cleanup: Option<u16>,
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
            pair_render: PairRender::default(),
            preview_as_lr: false,
            wide_export: WideExportMode::default(),
            show_waveform_only: false,
            waveform_layout: WaveformLayout::default(),
            waveform_coloring: WaveformColoring::default(),
            waveform_gradient: None,
            show_bar_lines: false,
            show_seconds_lines: false,
            seconds_interval: 5,
            border_enabled: false,
            border_width: 1,
            border_color: None,
            border_follows_background: true,
            show_title: true,
            show_meter: true,
            status_mark: StatusMark::default(),
            capture_input: CaptureInput::default(),
            sender_enabled: false,
            sender_name: String::new(),
            sender_id: None,
            output_dir: backtracking.join("output"),
            saved_dir: backtracking.join("saved"),
            output_cleanup: None,
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
            // Projects saved before these choices existed get the desktop's defaults.
            preview_as_lr: fields
                .get("wide_preview")
                .is_some_and(|value| value == "lr"),
            pair_render: fields
                .get("pair_render")
                .and_then(|value| {
                    PairRender::ALL
                        .into_iter()
                        .find(|pair| pair.stored() == value)
                })
                .unwrap_or(defaults.pair_render),
            wide_export: fields
                .get("wide_export")
                .and_then(|value| {
                    WIDE_EXPORT_MODES
                        .into_iter()
                        .find(|mode| wide_export_stored(*mode) == value)
                })
                .unwrap_or(defaults.wide_export),
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
            // Projects saved before these choices existed, and any value this version does
            // not know, draw in a single color with the default gradient kept for later.
            waveform_coloring: fields
                .get("waveform_color")
                .and_then(|value| {
                    WaveformColoring::ALL
                        .into_iter()
                        .find(|coloring| coloring.stored() == value)
                })
                .unwrap_or(defaults.waveform_coloring),
            waveform_gradient: fields
                .get("waveform_gradient")
                .and_then(|value| WaveformGradient::parse(value)),
            show_bar_lines: fields.get("show_bar_lines").is_some_and(|value| value == "true"),
            show_seconds_lines: fields.get("show_seconds_lines").is_some_and(|value| value == "true"),
            seconds_interval: fields
                .get("seconds_interval")
                .and_then(|value| value.parse().ok())
                .filter(|value| [1, 5, 10].contains(value))
                .unwrap_or(defaults.seconds_interval),
            border_enabled: fields.get("border_enabled").is_some_and(|value| value == "true"),
            border_width: fields
                .get("border_width")
                .and_then(|value| value.parse().ok())
                .filter(|value| BORDER_WIDTHS.contains(value))
                .unwrap_or(defaults.border_width),
            border_color: fields.get("border_color").and_then(|value| deserialize_field(value).ok()),
            border_follows_background: fields
                .get("border_follows_background")
                .map_or(defaults.border_follows_background, |value| value == "true"),
            // Projects saved before these choices existed show everything.
            show_title: fields.get("show_title").is_none_or(|value| value != "false"),
            show_meter: fields.get("show_meter").is_none_or(|value| value != "false"),
            status_mark: fields
                .get("status_mark")
                .and_then(|value| StatusMark::ALL.into_iter().find(|mark| mark.stored() == value))
                .unwrap_or_default(),
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
            // Anything but an offered age is Off: deleting files is never a default.
            output_cleanup: fields
                .get("output_cleanup_days")
                .and_then(|value| value.parse().ok())
                .and_then(cleanup::age_from_days),
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
                "waveform_color".into(),
                settings.waveform_coloring.stored().into(),
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
            (
                "wide_preview".into(),
                if settings.preview_as_lr {
                    "lr"
                } else {
                    "own-channels"
                }
                .into(),
            ),
            ("pair_render".into(), settings.pair_render.stored().into()),
            (
                "wide_export".into(),
                wide_export_stored(settings.wide_export).into(),
            ),
            ("show_bar_lines".into(), settings.show_bar_lines.to_string()),
            ("show_seconds_lines".into(), settings.show_seconds_lines.to_string()),
            ("seconds_interval".into(), settings.seconds_interval.to_string()),
            ("border_enabled".into(), settings.border_enabled.to_string()),
            ("border_width".into(), settings.border_width.to_string()),
            ("show_title".into(), settings.show_title.to_string()),
            ("show_meter".into(), settings.show_meter.to_string()),
            ("status_mark".into(), settings.status_mark.stored().into()),
            ("output_cleanup_days".into(), settings.output_cleanup.unwrap_or(0).to_string()),
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
        // Absent is the default gradient, as an empty preference is on the desktop.
        if let Some(gradient) = &settings.waveform_gradient {
            fields.insert("waveform_gradient".into(), gradient.stored());
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
    fn display_and_cleanup_choices_keep_their_defaults_until_a_valid_value_is_stored() {
        let settings = SettingsStore::default();
        // A project saved by an older version has none of these fields.
        settings.set(BTreeMap::new());
        let restored = settings.lock().clone();
        assert!(restored.show_title && restored.show_meter);
        assert_eq!(restored.status_mark, StatusMark::Always);
        assert_eq!(restored.border_width, 1);
        assert_eq!(restored.output_cleanup, None);

        for (days, expected) in [("0", None), ("7", None), ("-10", None), ("soon", None),
            ("10", Some(10)), ("90", Some(90)), ("91", None)]
        {
            settings.set(BTreeMap::from([("output_cleanup_days".into(), days.into())]));
            assert_eq!(settings.lock().output_cleanup, expected, "{days}");
        }
        for (width, expected) in [("0", 1), ("4", 4), ("5", 1), ("wide", 1)] {
            settings.set(BTreeMap::from([("border_width".into(), width.into())]));
            assert_eq!(settings.lock().border_width, expected, "{width}");
        }
        settings.set(BTreeMap::from([
            ("show_title".into(), "false".into()),
            ("show_meter".into(), "nonsense".into()),
            ("status_mark".into(), "sometimes".into()),
        ]));
        let restored = settings.lock().clone();
        assert!(!restored.show_title && restored.show_meter);
        assert_eq!(restored.status_mark, StatusMark::Always);
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
            settings.border_width = 3;
            settings.show_title = false;
            settings.show_meter = false;
            settings.status_mark = StatusMark::Paused;
            settings.output_cleanup = Some(30);
        }
        let restored = SettingsStore::default();
        restored.set(source.map(Clone::clone));
        assert_eq!(*restored.lock(), *source.lock());
    }

    #[test]
    fn wide_preview_round_trips_and_defaults_for_unknown_and_missing_values() {
        let store = SettingsStore::default();
        for value in [Some("lr"), Some("own-channels"), Some("future"), None] {
            store.set(
                value
                    .map(|v| ("wide_preview".into(), v.to_owned()))
                    .into_iter()
                    .collect(),
            );
            assert_eq!(store.lock().preview_as_lr, value == Some("lr"));
            let fields = store.map(Clone::clone);
            assert_eq!(
                fields["wide_preview"],
                if value == Some("lr") {
                    "lr"
                } else {
                    "own-channels"
                }
            );
            let restored = SettingsStore::default();
            restored.set(fields);
            assert_eq!(restored.lock().preview_as_lr, store.lock().preview_as_lr);
        }
    }

    #[test]
    fn multichannel_choices_are_stored_restored_and_default_for_old_projects() {
        let settings = SettingsStore::default();
        // A project saved before these choices existed: the desktop's defaults.
        settings.set(BTreeMap::from([("history_seconds".into(), "30".into())]));
        assert_eq!(settings.lock().pair_render, PairRender::Stereo);
        assert_eq!(settings.lock().wide_export, WideExportMode::BinauralFiles);
        let stored = settings.map(Clone::clone);
        assert_eq!(stored["pair_render"], "stereo");
        assert_eq!(stored["wide_export"], "binaural-files");

        for (pair, mode, stored_pair, stored_mode) in [
            (
                PairRender::Binaural,
                WideExportMode::MonoFiles,
                "binaural",
                "mono-files",
            ),
            (
                PairRender::Stereo,
                WideExportMode::Multichannel,
                "stereo",
                "multichannel",
            ),
            (
                PairRender::Binaural,
                WideExportMode::BinauralMix,
                "binaural",
                "binaural-mix",
            ),
            (
                PairRender::Stereo,
                WideExportMode::BinauralFiles,
                "stereo",
                "binaural-files",
            ),
        ] {
            let source = SettingsStore::default();
            {
                let mut settings = source.lock();
                settings.pair_render = pair;
                settings.wide_export = mode;
            }
            let fields = source.map(Clone::clone);
            assert_eq!(fields["pair_render"], stored_pair);
            assert_eq!(fields["wide_export"], stored_mode);
            let restored = SettingsStore::default();
            restored.set(fields);
            assert_eq!(restored.lock().pair_render, pair);
            assert_eq!(restored.lock().wide_export, mode);
            assert_eq!(*restored.lock(), *source.lock());
        }

        // Anything else, such as a value from a later version, keeps the default.
        settings.set(BTreeMap::from([
            ("pair_render".into(), "surround".into()),
            ("wide_export".into(), "Binaural".into()),
        ]));
        assert_eq!(settings.lock().pair_render, PairRender::Stereo);
        assert_eq!(settings.lock().wide_export, WideExportMode::BinauralFiles);
    }

    fn stop(position: f32, rgb: [u8; 3]) -> GradientStop {
        GradientStop { position, rgb }
    }

    /// The fields an editor with every waveform color choice made writes.
    fn colored_fields() -> (PersistedFields, WaveformGradient) {
        let gradient = WaveformGradient::new([
            stop(0.0, [232, 48, 44]),
            stop(1.0 / 3.0, [250, 200, 40]),
            stop(0.62, [40, 200, 90]),
            stop(1.0, [44, 104, 236]),
        ])
        .unwrap();
        let source = SettingsStore::default();
        {
            let mut settings = source.lock();
            settings.waveform_coloring = WaveformColoring::Frequency;
            settings.waveform_gradient = Some(gradient.clone());
            settings.waveform_layout = WaveformLayout::Stereo;
            settings.accent = [224, 90, 155];
            settings.border_color = Some([210, 180, 50]);
        }
        (source.map(Clone::clone), gradient)
    }

    #[test]
    fn a_project_saved_before_waveform_colors_restores_a_single_color_and_the_rest() {
        let settings = SettingsStore::default();
        // What an older version writes: every field it had, none of the color fields.
        let mut older = SettingsStore::default().map(Clone::clone);
        older.remove("waveform_color");
        older.remove("waveform_gradient");
        older.insert("waveform_layout".into(), "stereo".into());
        older.insert("accent_rgb".into(), serialize_field(&[224u8, 90, 155]).unwrap());
        settings.set(older);
        let restored = settings.lock().clone();
        assert_eq!(restored.waveform_coloring, WaveformColoring::Single);
        assert_eq!(restored.waveform_gradient, None);
        assert_eq!(restored.waveform_layout, WaveformLayout::Stereo);
        assert_eq!(restored.accent, [224, 90, 155]);
        // Saving it again writes a single color and no gradient.
        let fields = settings.map(Clone::clone);
        assert_eq!(fields["waveform_color"], "single");
        assert!(!fields.contains_key("waveform_gradient"));
        settings.set(BTreeMap::new());
        assert_eq!(*settings.lock(), SessionSettings::default());
    }

    #[test]
    fn waveform_colors_round_trip_in_the_desktops_form() {
        let (fields, gradient) = colored_fields();
        assert_eq!(fields["waveform_color"], "frequency");
        // The desktop's waveform-gradient-v1 text for the same gradient.
        assert_eq!(
            fields["waveform_gradient"],
            "#e8302c@0,#fac828@0.33333334,#28c85a@0.62,#2c68ec@1"
        );
        let restored = SettingsStore::default();
        restored.set(fields.clone());
        assert_eq!(restored.lock().waveform_coloring, WaveformColoring::Frequency);
        assert_eq!(restored.lock().waveform_gradient.as_ref(), Some(&gradient));
        assert_eq!(restored.map(Clone::clone), fields);
        // Single color keeps the gradient the user made, for when they turn it back on.
        restored.lock().waveform_coloring = WaveformColoring::Single;
        let single = restored.map(Clone::clone);
        assert_eq!(single["waveform_color"], "single");
        let again = SettingsStore::default();
        again.set(single);
        assert_eq!(again.lock().waveform_coloring, WaveformColoring::Single);
        assert_eq!(again.lock().waveform_gradient.as_ref(), Some(&gradient));
    }

    #[test]
    fn the_color_fields_leave_every_other_setting_as_an_older_version_reads_it() {
        // An older version looks up only the fields it knows, so the fields below are all it
        // sees of a project saved with colors: they must restore exactly as without them.
        let (fields, _) = colored_fields();
        let mut without = fields.clone();
        without.remove("waveform_color");
        without.remove("waveform_gradient");
        let with_colors = SettingsStore::default();
        with_colors.set(fields);
        let older_view = SettingsStore::default();
        older_view.set(without);
        let expected = SessionSettings {
            waveform_coloring: WaveformColoring::Frequency,
            waveform_gradient: with_colors.lock().waveform_gradient.clone(),
            ..older_view.lock().clone()
        };
        assert_eq!(*with_colors.lock(), expected);
        assert_eq!(older_view.lock().waveform_layout, WaveformLayout::Stereo);
        assert_eq!(older_view.lock().border_color, Some([210, 180, 50]));
    }

    #[test]
    fn unknown_or_damaged_color_fields_read_as_the_defaults() {
        let settings = SettingsStore::default();
        for bad in ["Frequency", "spectral", "", "1", " frequency"] {
            settings.set(BTreeMap::from([("waveform_color".into(), bad.into())]));
            assert_eq!(settings.lock().waveform_coloring, WaveformColoring::Single, "{bad:?}");
        }
        for bad in [
            "#e8302c@0",
            "#e8302c@0,#2c68ec@1.5",
            "#e8302c@0,#2c68ec@-0.1",
            "#e8302c@0,#2c68ec@NaN",
            "#e8302c@0,#2c68ec@inf",
            "#e8302c@0,#2c68ec",
            "#e8302c@0,2c68ec@1",
            "#e8302c@0,#2c68zz@1",
            "#e8302c@0,#2c68e@1",
            "#e8302c@0,#+c68ec1@1",
            "#e8302c@0,,#2c68ec@1",
            "",
            &["#e8302c@0.5"; 9].join(","),
            "not a gradient",
        ] {
            settings.set(BTreeMap::from([("waveform_gradient".into(), bad.to_owned())]));
            assert_eq!(settings.lock().waveform_gradient, None, "{bad:?}");
        }
        // Stops may come in any order and with spaces, as the desktop reads them.
        settings.set(BTreeMap::from([(
            "waveform_gradient".into(),
            " #2C68EC@1 , #E8302C@-0 ".into(),
        )]));
        let gradient = settings.lock().waveform_gradient.clone().unwrap();
        assert_eq!(gradient.stops(), [stop(0.0, [232, 48, 44]), stop(1.0, [44, 104, 236])]);
        assert_eq!(gradient.stored(), "#e8302c@0,#2c68ec@1");
        // Eight stops are the most; stops at one position keep their order.
        let eight = WaveformGradient::new((0..8).map(|index| stop(0.5, [index, 0, 0]))).unwrap();
        assert_eq!(eight.stops().iter().map(|stop| stop.rgb[0]).collect::<Vec<_>>(), [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(WaveformGradient::parse(&eight.stored()), Some(eight));
    }
}

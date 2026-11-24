use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    AutoResumeDelay, ChannelRoute, DurationChoice, WavExportProfile, WavSampleFormat,
};

use crate::controller::{CaptureSource, SourceSettings};
use crate::data_paths::DataPaths;
use crate::hotkeys::Hotkeys;

const MAX_VALUE_BYTES: u64 = 128 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
pub(crate) const DEFAULT_ACCENT: [u8; 3] = [103, 231, 181];

/// Export choice for lanes wider than stereo. Existing mono/stereo choices are separate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WideExportMode {
    #[default]
    BinauralFiles,
    BinauralMix,
    MonoFiles,
    Multichannel,
}

impl From<WideExportMode> for backtracking_core::wide_export::WideExportMode {
    fn from(mode: WideExportMode) -> Self {
        match mode {
            WideExportMode::BinauralFiles => Self::BinauralFiles,
            WideExportMode::BinauralMix => Self::BinauralMix,
            WideExportMode::MonoFiles => Self::MonoFiles,
            WideExportMode::Multichannel => Self::Multichannel,
        }
    }
}

impl WideExportMode {
    pub(crate) const ALL: [Self; 4] = [
        Self::BinauralFiles,
        Self::MonoFiles,
        Self::Multichannel,
        Self::BinauralMix,
    ];

    pub(crate) fn preference(self) -> &'static str {
        match self {
            Self::BinauralFiles => "binaural-files",
            Self::BinauralMix => "binaural-mix",
            Self::MonoFiles => "mono-files",
            Self::Multichannel => "multichannel",
        }
    }

    fn from_preference(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.preference() == text)
    }
}

/// How the L and R rows on top of a lane wider than stereo sound in preview and on export. The
/// window draws them as the stereo fold-down either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PairRender {
    /// Each channel goes to the left, the right or both by its label.
    #[default]
    Stereo,
    /// Each channel is placed around the head by its direction.
    Binaural,
}

impl PairRender {
    pub(crate) const ALL: [Self; 2] = [Self::Stereo, Self::Binaural];

    pub(crate) fn preference(self) -> &'static str {
        match self {
            Self::Stereo => "stereo",
            Self::Binaural => "binaural",
        }
    }

    pub(crate) fn from_preference(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.preference() == text)
    }

    /// What the export calls `pair_binaural`.
    pub(crate) fn binaural(self) -> bool {
        self == Self::Binaural
    }
}

/// Source identity to the channel group last selected for that source.
pub(crate) type SourceGroupChoices = BTreeMap<String, String>;

/// When the capture status mark is shown: the light in the toolbar and the mark on a bare
/// waveform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum StatusMark {
    #[default]
    Always,
    /// Only when capture is paused or a source has a problem.
    Attention,
    Never,
}

impl StatusMark {
    pub(crate) const ALL: [Self; 3] = [Self::Always, Self::Attention, Self::Never];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Always => "Always",
            Self::Attention => "Only when paused or failing",
            Self::Never => "Never",
        }
    }

    /// The value written to preferences.
    pub(crate) fn preference(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Attention => "attention",
            Self::Never => "never",
        }
    }

    fn from_preference(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mark| mark.preference() == text)
    }
}

/// How the waveform is coloured: in one colour, as it always was, or by what each moment of it
/// holds from low to high frequencies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WaveformColoring {
    #[default]
    Single,
    Frequency,
}

impl WaveformColoring {
    pub(crate) const ALL: [Self; 2] = [Self::Single, Self::Frequency];

    /// The value written to preferences.
    pub(crate) fn preference(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Frequency => "frequency",
        }
    }

    pub(crate) fn from_preference(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|coloring| coloring.preference() == text)
    }
}

/// The user's own gradient for "By frequency" as it is stored: each stop as `#rrggbb@position`,
/// lowest first, separated by commas, for example `#e8302c@0,#2c68ec@1`. Positions run from
/// 0 (the lowest band) to 1 (the highest).
pub(crate) fn serialize_gradient(spectrum: &backtracking_ui::Spectrum) -> String {
    spectrum
        .stops()
        .iter()
        .map(|stop| {
            let [r, g, b, _] = stop.color.to_array();
            format!("#{r:02x}{g:02x}{b:02x}@{}", stop.position)
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Reads [`serialize_gradient`]'s text. `None` for anything else: too few or too many stops,
/// a colour that is not `#rrggbb`, or a position outside 0 to 1.
pub(crate) fn parse_gradient(text: &str) -> Option<backtracking_ui::Spectrum> {
    let stops = text
        .split(',')
        .map(|stop| {
            let (color, position) = stop.trim().split_once('@')?;
            let [r, g, b] = parse_accent(color)?;
            let position: f32 = position.trim().parse().ok()?;
            Some(backtracking_ui::SpectrumStop {
                position,
                color: eframe::egui::Color32::from_rgb(r, g, b),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    backtracking_ui::Spectrum::new(stops).ok()
}

/// The widths the waveform border can have, in points.
pub(crate) const BORDER_WIDTHS: [u8; 4] = [1, 2, 3, 4];

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    History,
    IdleResume,
    RetainSilence,
    HideSilentChannels,
    Transparency,
    Accent,
    Meter,
    MeterB,
    MeterUnitsAfterB,
    SplitWaveform,
    CaptureSources,
    CaptureLanes,
    SourceNames,
    WavSampleRate,
    WavSampleFormat,
    WavForceMono,
    WideExport,
    PairRender,
    ShownGroups,
    Hotkeys,
    Output,
    OutputCleanup,
    SavedSnapshots,
    AlwaysOnTop,
    WaveformOnly,
    WaveformBorder,
    WaveformBorderWidth,
    WaveformBorderColor,
    WaveformBorderFollowsBackground,
    StatusMark,
    ShowTitle,
    ShowMeters,
    BarLines,
    SecondsLines,
    SecondsInterval,
    SnapshotTarget,
    WindowPosition,
    AddonLook,
    WaveformColor,
    WaveformGradient,
}

impl Key {
    fn filename(self) -> &'static str {
        match self {
            Self::History => "history-seconds",
            // The old preference used 0 for Off.
            Self::IdleResume => "idle-resume-delay-seconds",
            Self::HideSilentChannels => "hide-silent-channels",
            Self::RetainSilence => "retain-silence",
            Self::Transparency => "transparency-percent",
            Self::Accent => "theme-color",
            Self::Meter => "meter-unit",
            Self::MeterB => "meter-b-unit",
            // The units of lanes C, D, … one per line; A and B keep the two files above.
            Self::MeterUnitsAfterB => "meter-units-after-b-v1",
            Self::SplitWaveform => "split-waveform",
            // Lanes A and B as two lines (`off` for no lane B), as version 0.2.1 reads them.
            Self::CaptureSources => "capture-sources",
            // Every lane, one line each (`serialize_lanes`); used while its first two lanes
            // match `capture-sources`, so a change made by an older version wins.
            Self::CaptureLanes => "capture-lanes-v1",
            Self::SourceNames => "source-file-names",
            Self::WavSampleRate => "wav-sample-rate",
            Self::WavSampleFormat => "wav-sample-format",
            Self::WavForceMono => "wav-force-mono",
            Self::WideExport => "wide-lane-export-v1",
            Self::PairRender => "multichannel-pair-render",
            Self::ShownGroups => "shown-channel-groups-v1",
            Self::Hotkeys => "hotkeys-v1",
            Self::Output => "output-folder",
            // Absent or 0 keeps every file.
            Self::OutputCleanup => "output-folder-cleanup-days",
            Self::SavedSnapshots => "saved-snapshots-folder",
            Self::AlwaysOnTop => "always-on-top",
            Self::WaveformOnly => "waveform-only",
            Self::WaveformBorder => "waveform-border",
            Self::WaveformBorderWidth => "waveform-border-width",
            Self::WaveformBorderColor => "waveform-border-color",
            Self::StatusMark => "capture-status-mark",
            // Both are shown unless the file says 0.
            Self::ShowTitle => "show-app-name",
            Self::ShowMeters => "show-level-meters",
            Self::WaveformBorderFollowsBackground => "waveform-border-follows-background",
            Self::BarLines => "show-bar-lines",
            Self::SecondsLines => "show-second-lines",
            Self::SecondsInterval => "second-line-interval",
            Self::SnapshotTarget => "snapshot-target",
            Self::WindowPosition => "window-position",
            // The id of the add-on whose look is on; absent or empty is the app's own look.
            Self::AddonLook => "addon-look",
            // `single` or `frequency`; anything else is a single colour.
            Self::WaveformColor => "waveform-color",
            // The user's own gradient (`serialize_gradient`); absent or empty is the default.
            Self::WaveformGradient => "waveform-gradient-v1",
        }
    }
}

pub(crate) struct Settings {
    pub(crate) history: DurationChoice,
    pub(crate) idle_resume: AutoResumeDelay,
    pub(crate) hide_silent_channels: bool,
    pub(crate) retain_silence: bool,
    pub(crate) transparency: u8,
    pub(crate) accent: [u8; 3],
    /// The meter unit of lanes A, B, C, …: at least two, and one for each lane C on that has
    /// one stored.
    pub(crate) meter_modes: Vec<MeterMode>,
    pub(crate) split_waveform: bool,
    /// The lanes as stored: lanes A and B, `None` for a lane that is off, from
    /// `capture-sources`, or every lane from `capture-lanes-v1`. The controller takes the
    /// lanes that are on ([`crate::controller::compact_sources`]).
    pub(crate) sources: Vec<Option<SourceSettings>>,
    pub(crate) source_names: SourceNames,
    pub(crate) wav_profile: WavExportProfile,
    pub(crate) wav_force_mono: bool,
    pub(crate) wide_export: WideExportMode,
    pub(crate) pair_render: PairRender,
    pub(crate) shown_groups: SourceGroupChoices,
    pub(crate) hotkeys: Hotkeys,
    pub(crate) output: PathBuf,
    /// Days after which old exports in `output` are deleted; `None` keeps them.
    pub(crate) output_cleanup: Option<u16>,
    pub(crate) saved_output: PathBuf,
    pub(crate) pinned: bool,
    pub(crate) waveform_only: bool,
    pub(crate) waveform_border: bool,
    pub(crate) waveform_border_width: u8,
    pub(crate) waveform_border_color: Option<[u8; 3]>,
    pub(crate) status_mark: StatusMark,
    pub(crate) show_title: bool,
    pub(crate) show_meters: bool,
    pub(crate) waveform_border_follows_background: bool,
    pub(crate) bar_lines: bool,
    pub(crate) second_lines: bool,
    pub(crate) second_interval: u8,
    pub(crate) snapshot_target: Option<String>,
    pub(crate) window_position: Option<[i32; 2]>,
    /// The add-on whose look the window takes while it offers one.
    pub(crate) addon_look: Option<String>,
    pub(crate) waveform_coloring: WaveformColoring,
    /// The gradient the user made for "By frequency", or `None` when they made none.
    pub(crate) waveform_gradient: Option<backtracking_ui::Spectrum>,
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
            hide_silent_channels: true,
            retain_silence: false,
            transparency: 0,
            accent: DEFAULT_ACCENT,
            meter_modes: vec![MeterMode::Dbfs; 2],
            split_waveform: false,
            sources: vec![
                Some(SourceSettings {
                    source: CaptureSource::System,
                    route: ChannelRoute::Merged,
                }),
                None,
            ],
            source_names: SourceNames::default(),
            wav_profile: WavExportProfile::default(),
            wav_force_mono: false,
            wide_export: WideExportMode::default(),
            pair_render: PairRender::default(),
            shown_groups: SourceGroupChoices::new(),
            hotkeys: Hotkeys::default(),
            output: directory.join("output"),
            output_cleanup: None,
            saved_output: directory.join("saved"),
            pinned: true,
            waveform_only: false,
            waveform_border: false,
            waveform_border_width: 1,
            waveform_border_color: None,
            status_mark: StatusMark::Always,
            show_title: true,
            show_meters: true,
            waveform_border_follows_background: false,
            bar_lines: false,
            second_lines: false,
            second_interval: 5,
            snapshot_target: None,
            window_position: None,
            addon_look: None,
            waveform_coloring: WaveformColoring::Single,
            waveform_gradient: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Preferences {
    directory: Option<PathBuf>,
}

impl Preferences {
    pub(crate) fn for_current_user() -> Self {
        let directory = DataPaths::for_current_user()
            .ok()
            .map(|paths| paths.settings());
        Self { directory }
    }

    /// Use the same injected app data root as the add-on manager and capture sources.
    pub(crate) fn in_data_root(root: impl AsRef<Path>) -> Self {
        Self {
            directory: DataPaths::new(root.as_ref().to_path_buf())
                .ok()
                .map(|paths| paths.settings()),
        }
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
        settings.hide_silent_channels = self.read_number::<u8>(Key::HideSilentChannels) != Some(0);
        settings.retain_silence = self.read_number::<u8>(Key::RetainSilence) == Some(1);
        if let Some(percent @ (0 | 25 | 50 | 75 | 100)) = self.read_number(Key::Transparency) {
            settings.transparency = percent;
        }
        if let Some(accent) = self.read(Key::Accent).as_deref().and_then(parse_accent) {
            settings.accent = accent;
        }
        settings.meter_modes = vec![self.meter_mode(Key::Meter), self.meter_mode(Key::MeterB)];
        if let Some(units) = self.read(Key::MeterUnitsAfterB) {
            settings.meter_modes.extend(
                units
                    .lines()
                    .take(crate::controller::MAX_LANES - 2)
                    .map(parse_meter_mode),
            );
        }
        settings.split_waveform = self.read_number::<u8>(Key::SplitWaveform) == Some(1);
        let pair = self
            .read(Key::CaptureSources)
            .as_deref()
            .and_then(parse_sources);
        if let Some(pair) = &pair {
            settings.sources = pair.to_vec();
        }
        if let Some(lanes) = self
            .read(Key::CaptureLanes)
            .as_deref()
            .and_then(parse_lanes)
            && pair
                .as_ref()
                .is_some_and(|pair| lanes_match_pair(&lanes, pair))
        {
            settings.sources = lanes.into_iter().map(Some).collect();
        }
        if let Some(names) = self.read(Key::SourceNames) {
            settings.source_names = SourceNames::parse(&names);
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
        settings.wide_export = self
            .read(Key::WideExport)
            .as_deref()
            .and_then(|value| WideExportMode::from_preference(value.trim()))
            .unwrap_or_default();
        settings.pair_render = self
            .read(Key::PairRender)
            .as_deref()
            .and_then(|value| PairRender::from_preference(value.trim()))
            .unwrap_or_default();
        settings.shown_groups = self.load_shown_groups();
        if let Some(hotkeys) = self
            .read(Key::Hotkeys)
            .as_deref()
            .and_then(Hotkeys::from_json)
        {
            settings.hotkeys = hotkeys;
        }
        if let Some(path) = self.read_absolute_path(Key::Output) {
            settings.output = path;
        }
        settings.output_cleanup = self
            .read_number(Key::OutputCleanup)
            .and_then(backtracking_core::cleanup::age_from_days);
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
        if let Some(width) = self
            .read_number::<u8>(Key::WaveformBorderWidth)
            .filter(|width| BORDER_WIDTHS.contains(width))
        {
            settings.waveform_border_width = width;
        }
        if let Some(mark) = self
            .read(Key::StatusMark)
            .and_then(|text| StatusMark::from_preference(text.trim()))
        {
            settings.status_mark = mark;
        }
        settings.show_title = self.read_number::<u8>(Key::ShowTitle) != Some(0);
        settings.show_meters = self.read_number::<u8>(Key::ShowMeters) != Some(0);
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
        settings.snapshot_target = self
            .read(Key::SnapshotTarget)
            .filter(|id| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control));
        settings.window_position = self.read(Key::WindowPosition).and_then(|value| {
            let mut parts = value.split_whitespace();
            let x = parts.next()?.parse().ok()?;
            let y = parts.next()?.parse().ok()?;
            parts.next().is_none().then_some([x, y])
        });
        settings.addon_look = self
            .read(Key::AddonLook)
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control));
        settings.waveform_coloring = self
            .read(Key::WaveformColor)
            .and_then(|text| WaveformColoring::from_preference(text.trim()))
            .unwrap_or_default();
        settings.waveform_gradient = self
            .read(Key::WaveformGradient)
            .as_deref()
            .and_then(parse_gradient);
        settings
    }

    pub(crate) fn load_shown_groups(&self) -> SourceGroupChoices {
        self.read(Key::ShownGroups)
            .as_deref()
            .and_then(parse_shown_groups)
            .unwrap_or_default()
    }

    fn read_number<T: std::str::FromStr>(&self, key: Key) -> Option<T> {
        self.read(key)?.trim().parse().ok()
    }

    fn meter_mode(&self, key: Key) -> MeterMode {
        self.read(key)
            .as_deref()
            .map_or(MeterMode::Dbfs, parse_meter_mode)
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
        self.validate_save(value)?;
        let directory = self
            .directory
            .as_ref()
            .expect("validated preferences directory");
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

    fn validate_save(&self, value: &str) -> io::Result<()> {
        let directory = self.directory.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Windows app data directory unavailable",
            )
        })?;
        if !directory.is_absolute() || value.len() as u64 > MAX_VALUE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "preference path must be absolute and value must fit the read limit",
            ));
        }
        Ok(())
    }
}

const PREFERENCE_COALESCE_WINDOW: Duration = Duration::from_millis(25);
const MAX_QUEUED_PREFERENCE_DRAIN: usize = 1024;

enum PreferenceCommand {
    Save(Key, String),
    Flush(mpsc::SyncSender<io::Result<()>>),
    Shutdown,
}

struct FailedPreference {
    key: Key,
    value: String,
    kind: io::ErrorKind,
    message: String,
}

impl FailedPreference {
    fn error(&self) -> io::Error {
        io::Error::new(self.kind, self.message.clone())
    }
}

/// Durable preference I/O belongs to this worker, never to a painting callback.
/// A successful save means validated and queued; flush confirms disk completion.
pub(crate) struct PreferenceWriter {
    preferences: Preferences,
    commands: mpsc::Sender<PreferenceCommand>,
    errors: mpsc::Receiver<io::Error>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

impl PreferenceWriter {
    pub(crate) fn new(preferences: Preferences) -> io::Result<Self> {
        let sink = preferences.clone();
        Self::with_sink(preferences, move |key, value| sink.save(key, value))
    }

    fn with_sink(
        preferences: Preferences,
        sink: impl FnMut(Key, &str) -> io::Result<()> + Send + 'static,
    ) -> io::Result<Self> {
        preferences.validate_save("")?;
        let (commands, requests) = mpsc::channel();
        let (errors, failures) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("backtracking-settings".into())
            .spawn(move || Self::run(requests, errors, sink))?;
        Ok(Self {
            preferences,
            commands,
            errors: failures,
            worker: Some(worker),
        })
    }

    pub(crate) fn save(&self, key: Key, value: &str) -> io::Result<()> {
        self.preferences.validate_save(value)?;
        self.commands
            .send(PreferenceCommand::Save(key, value.to_owned()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "settings worker is closed"))
    }

    pub(crate) fn poll_error(&self) -> Option<io::Error> {
        self.errors.try_recv().ok()
    }

    /// A barrier for all saves queued before this call. Failed latest values
    /// remain available for a later flush retry; newer values replace them.
    pub(crate) fn flush(&self) -> io::Result<()> {
        let (reply, result) = mpsc::sync_channel(1);
        self.commands
            .send(PreferenceCommand::Flush(reply))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "settings worker is closed"))?;
        result.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "settings worker stopped before flushing",
            )
        })?
    }

    fn run(
        requests: mpsc::Receiver<PreferenceCommand>,
        errors: mpsc::Sender<io::Error>,
        mut sink: impl FnMut(Key, &str) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut failed: Vec<FailedPreference> = Vec::new();
        loop {
            let Ok(first) = requests.recv() else {
                return failed
                    .first()
                    .map_or(Ok(()), |failure| Err(failure.error()));
            };
            let mut pending = Vec::new();
            let mut flush = None;
            let mut shutdown = false;
            absorb_preference(first, &mut pending, &mut flush, &mut shutdown);
            let deadline = Instant::now() + PREFERENCE_COALESCE_WINDOW;
            while flush.is_none() && !shutdown {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match requests.recv_timeout(remaining) {
                    Ok(command) => {
                        absorb_preference(command, &mut pending, &mut flush, &mut shutdown)
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => shutdown = true,
                }
            }
            // Values already queued while the previous disk write was slow
            // belong to this batch even if a scheduling delay used the window.
            for _ in 0..MAX_QUEUED_PREFERENCE_DRAIN {
                if flush.is_some() || shutdown {
                    break;
                }
                match requests.try_recv() {
                    Ok(command) => {
                        absorb_preference(command, &mut pending, &mut flush, &mut shutdown)
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => shutdown = true,
                }
            }

            // A flush/shutdown retries previously failed latest values once.
            // Pending newer values supersede those retries before touching disk.
            let mut writes = Vec::new();
            if flush.is_some() || shutdown {
                for failure in &failed {
                    coalesce_preference(&mut writes, failure.key, failure.value.clone());
                }
            }
            for (key, value) in pending {
                coalesce_preference(&mut writes, key, value);
            }
            for (key, value) in writes {
                failed.retain(|failure| failure.key != key);
                if let Err(error) = sink(key, &value) {
                    let failure = FailedPreference {
                        key,
                        value,
                        kind: error.kind(),
                        message: error.to_string(),
                    };
                    let _ = errors.send(failure.error());
                    failed.push(failure);
                }
            }
            if let Some(reply) = flush {
                let result = failed
                    .first()
                    .map_or(Ok(()), |failure| Err(failure.error()));
                let _ = reply.send(result);
            }
            if shutdown {
                return failed
                    .first()
                    .map_or(Ok(()), |failure| Err(failure.error()));
            }
        }
    }
}

fn coalesce_preference(pending: &mut Vec<(Key, String)>, key: Key, value: String) {
    pending.retain(|(previous, _)| *previous != key);
    pending.push((key, value));
}

fn absorb_preference(
    command: PreferenceCommand,
    pending: &mut Vec<(Key, String)>,
    flush: &mut Option<mpsc::SyncSender<io::Result<()>>>,
    shutdown: &mut bool,
) {
    match command {
        PreferenceCommand::Save(key, value) => coalesce_preference(pending, key, value),
        PreferenceCommand::Flush(reply) => *flush = Some(reply),
        PreferenceCommand::Shutdown => *shutdown = true,
    }
}

impl Drop for PreferenceWriter {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.commands.send(PreferenceCommand::Shutdown);
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("Could not flush settings at shutdown: {error}"),
                Err(_) => eprintln!("Settings worker panicked at shutdown"),
            }
        }
    }
}

pub(crate) fn wav_sample_format_preference(format: WavSampleFormat) -> &'static str {
    match format {
        WavSampleFormat::Pcm16 => "pcm16",
        WavSampleFormat::Pcm24 => "pcm24",
        WavSampleFormat::Float32 => "float32",
    }
}

/// The names the user gave sources for their files, by [`CaptureSource::identity`], the one
/// named most recently last. A name belongs to the device, so it follows it to either lane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SourceNames(Vec<(String, String)>);

impl SourceNames {
    /// The most names kept. Naming one more forgets the one named longest ago, which keeps
    /// the file far inside what a preference may hold.
    const LIMIT: usize = 64;

    /// The name the user gave `source`, as they typed it.
    pub(crate) fn get(&self, source: &CaptureSource) -> Option<&str> {
        let identity = source.identity();
        self.0.iter().find(|(key, _)| *key == identity).map(|(_, name)| name.as_str())
    }

    /// Gives `source` the name `name`, or forgets its name when `name` is blank. True when
    /// that changed anything.
    pub(crate) fn set(&mut self, source: &CaptureSource, name: &str) -> bool {
        let identity = source.identity();
        let name: String = name.chars()
            .map(|character| if character.is_control() { ' ' } else { character })
            .collect();
        let index = self.0.iter().position(|(key, _)| *key == identity);
        if name.trim().is_empty() {
            return index.map(|index| self.0.remove(index)).is_some();
        }
        if index.is_some_and(|index| self.0[index].1 == name) {
            return false;
        }
        if let Some(index) = index {
            self.0.remove(index);
        }
        self.0.push((identity, name));
        if self.0.len() > Self::LIMIT {
            self.0.remove(0);
        }
        true
    }

    pub(crate) fn serialize(&self) -> String {
        let lines: Vec<_> = self.0.iter()
            .map(|(identity, name)| format!("{}\t{}", encode_text(identity), encode_text(name)))
            .collect();
        lines.join("\n")
    }

    /// A damaged line loses only its own name.
    fn parse(value: &str) -> Self {
        let mut names = Self::default();
        for line in value.lines() {
            let Some((identity, name)) = line.split_once('\t') else { continue };
            let (Some(identity), Some(name)) = (decode_text(identity), decode_text(name)) else {
                continue;
            };
            if name.trim().is_empty() || name.chars().any(char::is_control) {
                continue;
            }
            names.0.retain(|(key, _)| *key != identity);
            names.0.push((identity, name));
        }
        let excess = names.0.len().saturating_sub(Self::LIMIT);
        names.0.drain(..excess);
        names
    }
}

/// The `meter-unit` text of a unit; anything unknown is dBFS.
fn parse_meter_mode(text: &str) -> MeterMode {
    match text.trim() {
        "VU" => MeterMode::Vu,
        "LUFS" => MeterMode::Lufs,
        _ => MeterMode::Dbfs,
    }
}

/// `meter-units-after-b-v1`: the units of lanes C, D, … (`modes[2..]`), one per line. Empty
/// with two lanes or fewer.
pub(crate) fn serialize_meter_units_after_b(modes: &[MeterMode]) -> String {
    modes
        .iter()
        .skip(2)
        .map(|mode| mode.unit())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `capture-sources`, the file version 0.2.1 reads: lanes A and B as two lines, `off` when
/// there is no lane B. Lanes that are off are skipped, as in `capture-lanes-v1`, and lanes
/// after B are left out; `capture-lanes-v1` holds them.
pub(crate) fn serialize_sources(sources: &[Option<SourceSettings>]) -> String {
    let mut lanes = sources.iter().flatten();
    format!(
        "{}\n{}",
        encode_slot(lanes.next()),
        encode_slot(lanes.next())
    )
}

/// `capture-lanes-v1`: one line per lane that is on, in order, each as a line of
/// `capture-sources` is written.
pub(crate) fn serialize_lanes(sources: &[Option<SourceSettings>]) -> String {
    sources
        .iter()
        .flatten()
        .map(|settings| encode_slot(Some(settings)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads [`serialize_lanes`]: 1 to `MAX_LANES` lanes, or `None` when any line is not a lane.
fn parse_lanes(value: &str) -> Option<Vec<SourceSettings>> {
    let lanes = value.lines().map(parse_slot).collect::<Option<Vec<_>>>()?;
    (1..=crate::controller::MAX_LANES)
        .contains(&lanes.len())
        .then_some(lanes)
}

/// Whether `lanes` begins with the lanes A and B of `capture-sources`. When an older version
/// changed those two since `capture-lanes-v1` was written, that change is kept instead.
fn lanes_match_pair(lanes: &[SourceSettings], pair: &[Option<SourceSettings>; 2]) -> bool {
    pair.iter()
        .enumerate()
        .all(|(index, stored)| match (lanes.get(index), stored) {
            (None, None) => true,
            (Some(lane), Some(stored)) => {
                lane.route == stored.route && lane.source.same_source(&stored.source)
            }
            _ => false,
        })
}

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
            format!(
                "{route}\tsender\t{}\t{}",
                encode_text(id),
                encode_text(name)
            )
        }
        CaptureSource::Addon {
            addon_id,
            source_id,
            name,
        } => {
            format!(
                "{route}\taddon\t{}\t{}\t{}",
                encode_text(addon_id),
                encode_text(source_id),
                encode_text(name)
            )
        }
    }
}

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
        "addon" => {
            let addon_id = decode_text(fields.next()?)?;
            let source_id = decode_text(fields.next()?)?;
            backtracking_addon::manifest::validate_id(&addon_id).ok()?;
            if source_id.len() > 64
                || !source_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
            {
                return None;
            }
            let encoded_name = fields.next()?;
            let name = if encoded_name.is_empty() {
                String::new()
            } else {
                decode_text(encoded_name)?
            };
            if name.chars().count() > 120 || name.chars().any(char::is_control) {
                return None;
            }
            CaptureSource::Addon {
                addon_id,
                source_id,
                name,
            }
        }
        _ => return None,
    };
    fields
        .next()
        .is_none()
        .then_some(SourceSettings { source, route })
}

/// Reads `capture-sources`: lanes A and B, lane B `None` when it is `off`.
fn parse_sources(value: &str) -> Option<[Option<SourceSettings>; 2]> {
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

/// Names may change, so only stable source identifiers appear in this key.
pub(crate) fn source_preference_key(source: &CaptureSource) -> String {
    match source {
        CaptureSource::System => "system".into(),
        CaptureSource::DefaultMicrophone => "default-mic".into(),
        CaptureSource::Microphone { id, .. } => format!("mic:{}", encode_text(id)),
        CaptureSource::Application { path, .. } => {
            format!("app:{}", encode_text(&path.to_lowercase()))
        }
        CaptureSource::Sender { id, .. } => format!("sender:{}", encode_text(id)),
        CaptureSource::Addon {
            addon_id,
            source_id,
            ..
        } => {
            format!("addon:{}:{}", encode_text(addon_id), encode_text(source_id))
        }
    }
}

pub(crate) fn serialize_shown_groups(choices: &SourceGroupChoices) -> String {
    serde_json::to_string(choices).expect("string map can be encoded as JSON")
}

fn parse_shown_groups(value: &str) -> Option<SourceGroupChoices> {
    let choices: SourceGroupChoices = serde_json::from_str(value).ok()?;
    choices
        .keys()
        .all(|source| !source.is_empty() && !source.chars().any(char::is_control))
        .then_some(choices)
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
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    fn workspace_preferences(label: &str) -> Preferences {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        Preferences {
            directory: Some(
                workspace
                    .join("target/preference-writer-tests")
                    .join(format!("{label}-{}-{sequence}", std::process::id())),
            ),
        }
    }

    #[test]
    fn slow_disk_never_blocks_enqueue_and_pending_keys_keep_their_latest_order() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&writes);
        let (entered, entering) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let mut first = true;
        let writer =
            PreferenceWriter::with_sink(workspace_preferences("slow"), move |key, value| {
                if first {
                    first = false;
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                }
                observed.lock().unwrap().push((key, value.to_owned()));
                Ok(())
            })
            .unwrap();
        writer.save(Key::Accent, "initial").unwrap();
        entering.recv().unwrap();
        let (queued, queue) = mpsc::sync_channel(1);
        let ui = thread::spawn(move || {
            for _ in 0..100 {
                writer.save(Key::History, "10").unwrap();
            }
            writer.save(Key::Transparency, "25").unwrap();
            writer.save(Key::History, "600").unwrap();
            queued.send(writer).unwrap();
        });
        // save does no I/O. A save waiting for the sink cannot finish until we release it below.
        // The timeout gives the enqueue thread time to run on a busy machine.
        let result = queue.recv_timeout(Duration::from_secs(10));
        let returned_while_disk_busy = result.is_ok();
        release.send(()).unwrap();
        ui.join().unwrap();
        assert!(returned_while_disk_busy, "enqueue waited for the disk sink");
        let writer = result.unwrap();
        writer.flush().unwrap();
        assert_eq!(
            *writes.lock().unwrap(),
            [
                (Key::Accent, "initial".into()),
                (Key::Transparency, "25".into()),
                (Key::History, "600".into()),
            ]
        );
    }

    #[test]
    fn failed_disk_writes_are_reported_and_flush_preserves_the_latest_retry() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&writes);
        let writer =
            PreferenceWriter::with_sink(workspace_preferences("errors"), move |key, value| {
                observed.lock().unwrap().push((key, value.to_owned()));
                if value == "10" {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "simulated disk denial",
                    ))
                } else {
                    Ok(())
                }
            })
            .unwrap();
        writer.save(Key::History, "10").unwrap();
        assert_eq!(
            writer.flush().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let mut errors = 0;
        while let Some(error) = writer.poll_error() {
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            errors += 1;
        }
        assert!(errors > 0);
        writer.save(Key::History, "600").unwrap();
        writer.flush().unwrap();
        assert!(writer.poll_error().is_none());
        let writes = writes.lock().unwrap();
        assert_eq!(writes.last(), Some(&(Key::History, "600".into())));
        assert!(
            writes[..writes.len() - 1]
                .iter()
                .all(|write| write == &(Key::History, "10".into()))
        );
    }

    #[test]
    fn flush_barrier_does_not_wait_for_saves_queued_after_it() {
        let (late, late_started) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let writer =
            PreferenceWriter::with_sink(workspace_preferences("barrier"), move |_, value| {
                if value == "600" {
                    late.send(()).unwrap();
                    released.recv().unwrap();
                }
                Ok(())
            })
            .unwrap();
        writer.save(Key::History, "10").unwrap();
        let (barrier, result) = mpsc::sync_channel(1);
        writer
            .commands
            .send(PreferenceCommand::Flush(barrier))
            .unwrap();
        writer.save(Key::History, "600").unwrap();
        late_started.recv().unwrap();
        let completed_before_late_write = result.recv_timeout(Duration::from_millis(100));
        release.send(()).unwrap();
        assert!(
            completed_before_late_write.is_ok(),
            "flush crossed its queue barrier"
        );
        completed_before_late_write.unwrap().unwrap();
        writer.flush().unwrap();
    }

    #[test]
    fn flush_retries_an_unresolved_latest_value_and_never_reports_false_success() {
        let attempts = Arc::new(Mutex::new(0));
        let observed = Arc::clone(&attempts);
        let writable = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sink_writable = Arc::clone(&writable);
        let writer = PreferenceWriter::with_sink(workspace_preferences("retry"), move |_, _| {
            *observed.lock().unwrap() += 1;
            if sink_writable.load(Ordering::Relaxed) {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "simulated disk denial",
                ))
            }
        })
        .unwrap();
        writer.save(Key::History, "600").unwrap();
        for _ in 0..2 {
            assert_eq!(
                writer.flush().unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
            let mut errors = 0;
            while let Some(error) = writer.poll_error() {
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                errors += 1;
            }
            assert!(errors > 0);
        }
        writable.store(true, Ordering::Relaxed);
        writer.flush().unwrap();
        assert!(writer.poll_error().is_none());
        assert!(*attempts.lock().unwrap() >= 3);
    }

    #[test]
    fn writer_drop_drains_latest_values_before_a_new_instance_loads_them() {
        let preferences = workspace_preferences("restart");
        {
            let writer = PreferenceWriter::new(preferences.clone()).unwrap();
            writer.save(Key::History, "600").unwrap();
            writer.save(Key::IdleResume, "5").unwrap();
            writer.save(Key::History, "30").unwrap();
        }
        let reloaded = Preferences {
            directory: preferences.directory.clone(),
        }
        .load();
        assert_eq!(reloaded.history, DurationChoice::Seconds30);
        assert_eq!(
            reloaded.idle_resume,
            AutoResumeDelay::from_seconds(5).unwrap()
        );
    }

    #[test]
    fn invalid_preferences_fail_synchronously_before_any_disk_work() {
        assert!(PreferenceWriter::new(Preferences { directory: None }).is_err());
        let writes = Arc::new(Mutex::new(0));
        let observed = Arc::clone(&writes);
        let writer =
            PreferenceWriter::with_sink(workspace_preferences("validation"), move |_, _| {
                *observed.lock().unwrap() += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            writer
                .save(Key::History, &"x".repeat(MAX_VALUE_BYTES as usize + 1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        writer.flush().unwrap();
        assert_eq!(*writes.lock().unwrap(), 0);
    }

    #[test]
    #[ignore = "diagnostic durable writes in the workspace; not a reboot or GUI latency test"]
    fn preference_save_latency_probe() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let output = workspace.join("target/preference-perf-probe");
        fs::create_dir_all(&output).unwrap();
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let run = output.join(format!("run-{}-{sequence}", std::process::id()));
        let mut fresh = Vec::new();
        for index in 0..8 {
            let preferences = Preferences {
                directory: Some(run.join(format!("fresh-{index}"))),
            };
            let started = std::time::Instant::now();
            preferences.save(Key::History, "600").unwrap();
            fresh.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let preferences = Preferences {
            directory: Some(run.join("existing")),
        };
        preferences.save(Key::History, "600").unwrap();
        let mut warm = Vec::new();
        for index in 0..64 {
            let started = std::time::Instant::now();
            preferences
                .save(Key::History, if index % 2 == 0 { "10" } else { "600" })
                .unwrap();
            warm.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        assert_eq!(preferences.load().history, DurationChoice::Minutes10);
        let result = serde_json::json!({
            "schema_version": 1,
            "scope": "workspace-only real Preferences::save; durable sync_all and atomic rename retained",
            "limitations": "new test process; fresh directories are not cold boot, cache eviction, HDD or antivirus emulation; timings are not GUI latency",
            "value_bytes": 3,
            "fresh_directory_first_save_ms": fresh,
            "existing_directory_history_replace_ms": warm,
        });
        fs::write(
            run.join("synchronous-save-results.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
        println!("{result}");
    }

    #[test]
    fn silent_channel_preference_and_all_channels_route_round_trip() {
        let store = workspace_preferences("hide-silent-channels");
        assert!(store.load().hide_silent_channels);
        store.save(Key::HideSilentChannels, "0").unwrap();
        assert!(!store.load().hide_silent_channels);
        store.save(Key::HideSilentChannels, "1").unwrap();
        assert!(store.load().hide_silent_channels);
        let lanes = [
            Some(SourceSettings {
                source: CaptureSource::System,
                route: ChannelRoute::AllChannels,
            }),
            None,
        ];
        store
            .save(Key::CaptureSources, &serialize_sources(&lanes))
            .unwrap();
        assert_eq!(store.load().sources, lanes);
    }

    /// A different source and route for every lane up to 64.
    fn varied_lane(index: usize) -> SourceSettings {
        let route = [
            ChannelRoute::Merged,
            ChannelRoute::Left,
            ChannelRoute::Right,
            ChannelRoute::Side,
            ChannelRoute::AllChannels,
        ][index % 5];
        let source = match index % 6 {
            0 => CaptureSource::System,
            1 => CaptureSource::DefaultMicrophone,
            2 => CaptureSource::Sender {
                id: format!("c1c1-{index:04x}"),
                name: format!("Sender {index}"),
            },
            3 => CaptureSource::Addon {
                addon_id: "com.example.test-tone".into(),
                source_id: format!("stream-{index}"),
                name: format!("Stream {index}"),
            },
            4 => CaptureSource::Microphone {
                id: format!("{{0.0.1.00000000}}.{index}"),
                name: format!("Mic {index} – 麥克風"),
            },
            _ => CaptureSource::Application {
                path: format!(r"C:\Apps\player{index}.exe"),
                name: format!("Player {index}"),
            },
        };
        SourceSettings { source, route }
    }

    /// Saves the lanes as the Options page does.
    fn save_lanes(store: &Preferences, lanes: &[Option<SourceSettings>]) {
        store
            .save(Key::CaptureSources, &serialize_sources(lanes))
            .unwrap();
        store
            .save(Key::CaptureLanes, &serialize_lanes(lanes))
            .unwrap();
    }

    #[test]
    fn one_two_three_and_sixty_four_lanes_round_trip() {
        for count in [1, 2, 3, 64] {
            let store = workspace_preferences(&format!("lanes-{count}"));
            let lanes: Vec<_> = (0..count).map(|index| Some(varied_lane(index))).collect();
            save_lanes(&store, &lanes);
            let loaded = store.load().sources;
            assert_eq!(
                crate::controller::compact_sources(loaded.clone()),
                lanes,
                "{count} lanes"
            );
            assert_eq!(loaded, lanes, "{count} lanes");
        }
        // Lanes that are off are not written.
        let store = workspace_preferences("lanes-off");
        let a = Some(varied_lane(0));
        let c = Some(varied_lane(2));
        save_lanes(&store, &[a.clone(), None, c.clone()]);
        assert_eq!(
            serialize_lanes(&[a.clone(), None, c.clone()])
                .lines()
                .count(),
            2
        );
        assert_eq!(
            crate::controller::compact_sources(store.load().sources),
            [a, c]
        );
    }

    #[test]
    fn a_folder_from_version_0_2_1_loads_unchanged() {
        let store = workspace_preferences("lanes-0.2.1");
        // What version 0.2.1 writes, byte for byte: lane A, then lane B or `off`.
        store.save(Key::CaptureSources, "7\tsystem\noff").unwrap();
        store.save(Key::Meter, "VU").unwrap();
        store.save(Key::MeterB, "LUFS").unwrap();
        let settings = store.load();
        assert_eq!(
            settings.sources,
            [
                Some(SourceSettings {
                    source: CaptureSource::System,
                    route: ChannelRoute::AllChannels,
                }),
                None
            ]
        );
        assert_eq!(settings.meter_modes, [MeterMode::Vu, MeterMode::Lufs]);
        let b = varied_lane(2);
        store
            .save(
                Key::CaptureSources,
                &format!("5\tdefault-mic\n{}", encode_slot(Some(&b))),
            )
            .unwrap();
        assert_eq!(
            store.load().sources,
            [
                Some(SourceSettings {
                    source: CaptureSource::DefaultMicrophone,
                    route: ChannelRoute::Merged,
                }),
                Some(b)
            ]
        );
        // And an empty folder starts as before: system audio, Merged, and no lane B.
        let empty = workspace_preferences("lanes-empty").load();
        assert_eq!(empty.sources, Settings::default().sources);
        assert_eq!(empty.meter_modes, [MeterMode::Dbfs; 2]);
    }

    #[test]
    fn version_0_2_1_reads_lanes_a_and_b_of_more_lanes_and_its_changes_win() {
        let lanes: Vec<_> = (0..64).map(|index| Some(varied_lane(index))).collect();
        // `parse_sources` is the reader of version 0.2.1, line for line.
        for count in [1, 2, 3, 64] {
            let written = serialize_sources(&lanes[..count]);
            assert_eq!(written.lines().count(), 2, "{count} lanes");
            let b = if count == 1 { None } else { lanes[1].clone() };
            assert_eq!(parse_sources(&written), Some([lanes[0].clone(), b]));
        }
        assert!(serialize_sources(&lanes[..1]).ends_with("\noff"));
        // Version 0.2.1 sets lane B to another source: back on this version, its two lanes
        // are used and the lanes after B, which it never saw, are dropped.
        let store = workspace_preferences("lanes-downgrade");
        save_lanes(&store, &lanes[..3]);
        let other = Some(varied_lane(4));
        store
            .save(
                Key::CaptureSources,
                &serialize_sources(&[lanes[0].clone(), other.clone()]),
            )
            .unwrap();
        assert_eq!(store.load().sources, [lanes[0].clone(), other]);
        // It turns lane B off: one lane.
        save_lanes(&store, &lanes[..3]);
        store
            .save(Key::CaptureSources, &serialize_sources(&lanes[..1]))
            .unwrap();
        assert_eq!(store.load().sources, [lanes[0].clone(), None]);
        // It changes only lane B's route: that counts as a change too.
        save_lanes(&store, &lanes[..3]);
        let mut rerouted = lanes[1].clone().unwrap();
        rerouted.route = ChannelRoute::Merged;
        store
            .save(
                Key::CaptureSources,
                &serialize_sources(&[lanes[0].clone(), Some(rerouted.clone())]),
            )
            .unwrap();
        assert_eq!(store.load().sources, [lanes[0].clone(), Some(rerouted)]);
    }

    #[test]
    fn unreadable_lane_files_leave_lanes_a_and_b() {
        let lanes: Vec<_> = (0..3).map(|index| Some(varied_lane(index))).collect();
        let pair = vec![lanes[0].clone(), lanes[1].clone()];
        let [a, b] = [0, 1].map(|lane| encode_slot(lanes[lane].as_ref()));
        let too_many = vec![a.clone(); 65].join("\n");
        for (label, garbage) in [
            ("empty", String::new()),
            ("text", "not a lane".to_owned()),
            ("off", format!("{a}\noff\n{b}")),
            ("bad-route", "9\tsystem".to_owned()),
            ("too-many", too_many),
            ("binary", "\u{0}\u{1}\u{2}".to_owned()),
        ] {
            let store = workspace_preferences(&format!("lanes-garbage-{label}"));
            save_lanes(&store, &lanes);
            store.save(Key::CaptureLanes, &garbage).unwrap();
            assert_eq!(store.load().sources, pair, "{label}");
        }
        // Without a readable `capture-sources` the lanes file is not used either.
        let store = workspace_preferences("lanes-garbage-pair");
        save_lanes(&store, &lanes);
        store.save(Key::CaptureSources, "garbage").unwrap();
        assert_eq!(store.load().sources, Settings::default().sources);
    }

    #[test]
    fn meter_units_after_lane_b_round_trip_beside_the_old_two() {
        let store = workspace_preferences("meter-units-after-b");
        let mut modes = vec![
            MeterMode::Vu,
            MeterMode::Dbfs,
            MeterMode::Lufs,
            MeterMode::Vu,
        ];
        store.save(Key::Meter, modes[0].unit()).unwrap();
        store.save(Key::MeterB, modes[1].unit()).unwrap();
        store
            .save(
                Key::MeterUnitsAfterB,
                &serialize_meter_units_after_b(&modes),
            )
            .unwrap();
        assert_eq!(store.load().meter_modes, modes);
        assert_eq!(serialize_meter_units_after_b(&modes[..2]), "");
        // An unknown line is decibels, and no more than 64 lanes are read.
        store.save(Key::MeterUnitsAfterB, "LUFS\nloud\nVU").unwrap();
        modes.insert(3, MeterMode::Dbfs);
        assert_eq!(store.load().meter_modes, modes);
        store
            .save(Key::MeterUnitsAfterB, &vec!["VU"; 100].join("\n"))
            .unwrap();
        assert_eq!(store.load().meter_modes.len(), crate::controller::MAX_LANES);
    }

    #[test]
    fn the_chosen_addon_look_round_trips_and_none_clears_it() {
        let preferences = workspace_preferences("addon-look");
        assert_eq!(Settings::default().addon_look, None);
        assert_eq!(preferences.load().addon_look, None);

        let writer = PreferenceWriter::new(preferences.clone()).unwrap();
        writer.save(Key::AddonLook, "com.example.theme").unwrap();
        writer.flush().unwrap();
        assert_eq!(
            preferences.load().addon_look.as_deref(),
            Some("com.example.theme")
        );
        assert!(
            preferences
                .directory
                .as_ref()
                .unwrap()
                .join("addon-look")
                .is_file()
        );
        // Choosing None writes an empty file.
        writer.save(Key::AddonLook, "").unwrap();
        writer.flush().unwrap();
        assert_eq!(preferences.load().addon_look, None);
        for bad in ["  ", "a\nb", &"x".repeat(129)] {
            preferences.save(Key::AddonLook, bad).unwrap();
            assert_eq!(preferences.load().addon_look, None, "{bad:?}");
        }
    }

    #[test]
    fn the_waveform_colouring_and_gradient_round_trip_and_garbage_reads_as_the_default() {
        use backtracking_ui::{Spectrum, SpectrumStop};
        use eframe::egui::Color32;

        let preferences = workspace_preferences("waveform-color");
        let settings = preferences.load();
        assert_eq!(settings.waveform_coloring, WaveformColoring::Single);
        assert_eq!(settings.waveform_gradient, None);

        let gradient = Spectrum::new([
            SpectrumStop {
                position: 0.0,
                color: Color32::from_rgb(232, 48, 44),
            },
            SpectrumStop {
                position: 1.0 / 3.0,
                color: Color32::from_rgb(250, 200, 40),
            },
            SpectrumStop {
                position: 0.62,
                color: Color32::from_rgb(40, 200, 90),
            },
            SpectrumStop {
                position: 1.0,
                color: Color32::from_rgb(44, 104, 236),
            },
        ])
        .unwrap();
        let writer = PreferenceWriter::new(preferences.clone()).unwrap();
        writer.save(Key::WaveformColor, "frequency").unwrap();
        writer
            .save(Key::WaveformGradient, &serialize_gradient(&gradient))
            .unwrap();
        writer.flush().unwrap();
        let settings = preferences.load();
        assert_eq!(settings.waveform_coloring, WaveformColoring::Frequency);
        assert_eq!(settings.waveform_gradient.as_ref(), Some(&gradient));
        for (key, file) in [
            (Key::WaveformColor, "waveform-color"),
            (Key::WaveformGradient, "waveform-gradient-v1"),
        ] {
            assert_eq!(key.filename(), file);
            assert!(preferences.directory.as_ref().unwrap().join(file).is_file());
        }
        assert_eq!(
            preferences.read(Key::WaveformGradient).as_deref(),
            Some("#e8302c@0,#fac828@0.33333334,#28c85a@0.62,#2c68ec@1")
        );
        // Reset writes an empty file, which is the default gradient again.
        writer.save(Key::WaveformGradient, "").unwrap();
        writer.save(Key::WaveformColor, "single").unwrap();
        writer.flush().unwrap();
        let settings = preferences.load();
        assert_eq!(settings.waveform_coloring, WaveformColoring::Single);
        assert_eq!(settings.waveform_gradient, None);

        for bad in ["Frequency", "spectral", "", "1"] {
            preferences.save(Key::WaveformColor, bad).unwrap();
            assert_eq!(
                preferences.load().waveform_coloring,
                WaveformColoring::Single,
                "{bad:?}"
            );
        }
        preferences
            .save(Key::WaveformColor, " frequency\n")
            .unwrap();
        assert_eq!(
            preferences.load().waveform_coloring,
            WaveformColoring::Frequency
        );
        for bad in [
            "#e8302c@0",
            "#e8302c@0,#2c68ec@1.5",
            "#e8302c@0,#2c68ec@-0.1",
            "#e8302c@0,#2c68ec@NaN",
            "#e8302c@0,#2c68ec",
            "#e8302c@0,2c68ec@1",
            "#e8302c@0,#2c68zz@1",
            "#e8302c@0,,#2c68ec@1",
            &["#e8302c@0.5"; 9].join(","),
            "not a gradient",
        ] {
            preferences.save(Key::WaveformGradient, bad).unwrap();
            assert_eq!(preferences.load().waveform_gradient, None, "{bad:?}");
        }
        // Stops may be written in any order and with spaces; they read back sorted.
        preferences
            .save(Key::WaveformGradient, " #2C68EC@1 , #E8302C@0 ")
            .unwrap();
        assert_eq!(
            preferences.load().waveform_gradient,
            Some(Spectrum::default())
        );
    }

    #[test]
    fn quiet_sections_default_off_and_persist_through_the_background_writer() {
        let preferences = workspace_preferences("retain-silence");
        assert!(!Settings::default().retain_silence);
        assert!(!preferences.load().retain_silence);

        let writer = PreferenceWriter::new(preferences.clone()).unwrap();
        writer.save(Key::RetainSilence, "1").unwrap();
        writer.flush().unwrap();
        assert!(preferences.load().retain_silence);
        assert_eq!(preferences.read(Key::RetainSilence).as_deref(), Some("1"));
        assert!(
            preferences
                .directory
                .as_ref()
                .unwrap()
                .join("retain-silence")
                .is_file()
        );

        writer.save(Key::RetainSilence, "0").unwrap();
        writer.flush().unwrap();
        assert!(!preferences.load().retain_silence);
        preferences.save(Key::RetainSilence, "invalid").unwrap();
        assert!(!preferences.load().retain_silence);
    }

    #[test]
    fn auto_resume_off_is_stored_as_zero_seconds_and_survives_a_restart() {
        let preferences = workspace_preferences("auto-resume-off");
        assert_eq!(preferences.load().idle_resume, AutoResumeDelay::Seconds10);

        let writer = PreferenceWriter::new(preferences.clone()).unwrap();
        writer
            .save(Key::IdleResume, &AutoResumeDelay::Off.seconds().to_string())
            .unwrap();
        writer.flush().unwrap();
        // The file keeps the value older versions wrote for this choice.
        assert_eq!(preferences.read(Key::IdleResume).as_deref(), Some("0"));
        assert_eq!(preferences.load().idle_resume, AutoResumeDelay::Off);

        for choice in AutoResumeDelay::ALL {
            preferences
                .save(Key::IdleResume, &choice.seconds().to_string())
                .unwrap();
            assert_eq!(preferences.load().idle_resume, choice);
        }
        preferences.save(Key::IdleResume, "3").unwrap();
        assert_eq!(preferences.load().idle_resume, AutoResumeDelay::Seconds10);
    }

    #[test]
    fn display_choices_keep_their_defaults_until_a_valid_value_is_stored() {
        let preferences = workspace_preferences("display-choices");
        let settings = preferences.load();
        assert_eq!(settings.status_mark, StatusMark::Always);
        assert!(settings.show_title && settings.show_meters);
        assert_eq!(settings.waveform_border_width, 1);

        for mark in StatusMark::ALL {
            preferences
                .save(Key::StatusMark, mark.preference())
                .unwrap();
            assert_eq!(preferences.load().status_mark, mark);
        }
        preferences.save(Key::StatusMark, "sometimes").unwrap();
        assert_eq!(preferences.load().status_mark, StatusMark::Always);

        for (key, shown) in [(Key::ShowTitle, false), (Key::ShowMeters, false)] {
            preferences.save(key, "0").unwrap();
            let settings = preferences.load();
            assert_eq!(
                if key == Key::ShowTitle {
                    settings.show_title
                } else {
                    settings.show_meters
                },
                shown
            );
            preferences.save(key, "1").unwrap();
        }
        let settings = preferences.load();
        assert!(settings.show_title && settings.show_meters);

        for width in BORDER_WIDTHS {
            preferences
                .save(Key::WaveformBorderWidth, &width.to_string())
                .unwrap();
            assert_eq!(preferences.load().waveform_border_width, width);
        }
        for stored in ["0", "5", "wide"] {
            preferences.save(Key::WaveformBorderWidth, stored).unwrap();
            assert_eq!(preferences.load().waveform_border_width, 1, "{stored:?}");
        }
    }

    #[test]
    fn drag_folder_cleanup_is_off_until_an_offered_age_is_stored() {
        let preferences = workspace_preferences("output-cleanup");
        assert_eq!(preferences.load().output_cleanup, None);

        for days in backtracking_core::cleanup::AGE_CHOICES {
            preferences
                .save(Key::OutputCleanup, &days.to_string())
                .unwrap();
            assert_eq!(preferences.load().output_cleanup, Some(days));
        }
        // Off is stored as 0. Anything else unexpected must never start deleting files.
        for stored in ["0", "7", "-30", "30 days", ""] {
            preferences.save(Key::OutputCleanup, stored).unwrap();
            assert_eq!(preferences.load().output_cleanup, None, "{stored:?}");
        }
    }

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
            parse_sources(&serialize_sources(&one_source)).map(Vec::from),
            Some(one_source)
        );
        assert_eq!(parse_sources("off\noff"), None);
    }

    #[test]
    fn source_names_belong_to_the_device_and_survive_a_reload() {
        let chrome = CaptureSource::Application {
            path: r"C:\Apps\Chrome.exe".into(),
            name: "Chrome.exe".into(),
        };
        // The same program, listed again with its path in other letters.
        let chrome_again = CaptureSource::Application {
            path: r"c:\apps\chrome.exe".into(),
            name: "chrome.exe".into(),
        };
        let microphone = |id: &str| CaptureSource::Microphone { id: id.into(), name: "USB mic".into() };
        let mut names = SourceNames::default();
        assert!(names.set(&CaptureSource::System, "Desk\tspeakers 系統聲音"));
        assert!(names.set(&chrome, "Browser "));
        assert!(names.set(&microphone("one"), "Vocal"));
        assert!(!names.set(&chrome_again, "Browser "), "the same name changes nothing");
        assert_eq!(names.get(&CaptureSource::System), Some("Desk speakers 系統聲音"));
        assert_eq!(names.get(&chrome_again), Some("Browser "));
        assert_eq!(names.get(&microphone("two")), None, "another device of the same name");
        assert_eq!(names.get(&CaptureSource::DefaultMicrophone), None);

        let preferences = workspace_preferences("source-names");
        preferences.save(Key::SourceNames, &names.serialize()).unwrap();
        assert_eq!(preferences.load().source_names, names);

        // A blank name goes back to the default.
        assert!(names.set(&chrome, "   "));
        assert!(!names.set(&chrome, ""));
        assert_eq!(names.get(&chrome), None);
        preferences.save(Key::SourceNames, &names.serialize()).unwrap();
        assert_eq!(preferences.load().source_names, names);
        fs::remove_dir_all(preferences.directory.unwrap()).unwrap();
    }

    #[test]
    fn source_names_keep_the_latest_names_and_skip_damaged_lines() {
        let microphone = |index: usize| CaptureSource::Microphone {
            id: format!("mic-{index}"),
            name: "Mic".into(),
        };
        let mut names = SourceNames::default();
        for index in 0..=SourceNames::LIMIT {
            names.set(&microphone(index), &format!("Take {index}"));
        }
        assert_eq!(names.get(&microphone(0)), None, "the name set longest ago is forgotten");
        assert_eq!(names.get(&microphone(1)).unwrap(), "Take 1");
        assert_eq!(names.get(&microphone(SourceNames::LIMIT)).unwrap(),
            format!("Take {}", SourceNames::LIMIT));

        let text = format!("{}\nnot hex\tzz\n{}\t\n", names.serialize(), encode_text("system"));
        assert_eq!(SourceNames::parse(&text), names);
        assert_eq!(SourceNames::parse(""), SourceNames::default());
    }

    #[test]
    fn each_addon_source_keeps_its_own_file_name_after_reload() {
        let addon = |addon_id: &str, source_id: &str, name: &str| CaptureSource::Addon {
            addon_id: addon_id.into(),
            source_id: source_id.into(),
            name: name.into(),
        };
        let mut names = SourceNames::default();
        assert!(names.set(&addon("atmos", "bed", "Atmos bed"), "Dolby bed"));
        assert!(names.set(&addon("tone", "bed", "Test tone"), "Tone"));
        // The add-on can rename its source; the name the user gave stays with the source.
        assert_eq!(
            names.get(&addon("atmos", "bed", "Renamed")),
            Some("Dolby bed")
        );
        assert_eq!(names.get(&addon("atmos", "objects", "Atmos bed")), None);
        assert_eq!(names.get(&addon("tone", "bed", "Test tone")), Some("Tone"));
        assert_eq!(names.get(&CaptureSource::System), None);

        let preferences = workspace_preferences("addon-source-names");
        preferences
            .save(Key::SourceNames, &names.serialize())
            .unwrap();
        assert_eq!(preferences.load().source_names, names);
        fs::remove_dir_all(preferences.directory.unwrap()).unwrap();
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

    #[test]
    fn wide_export_and_group_choices_round_trip_without_changing_legacy_preferences() {
        let preferences = workspace_preferences("wide-export-old-folder");
        preferences.save(Key::History, "600").unwrap();
        preferences
            .save(Key::CaptureSources, "0\tsystem\noff")
            .unwrap();
        preferences.save(Key::WavForceMono, "1").unwrap();
        preferences.save(Key::WavSampleFormat, "pcm24").unwrap();
        let folder = preferences.directory.as_ref().unwrap();
        fs::write(
            folder.join("older-version-unknown-option"),
            "leave this alone",
        )
        .unwrap();

        let old = preferences.load();
        assert_eq!(old.history, DurationChoice::Minutes10);
        assert!(old.wav_force_mono);
        assert_eq!(old.wav_profile.sample_format, WavSampleFormat::Pcm24);
        assert_eq!(old.wide_export, WideExportMode::BinauralFiles);
        assert!(old.shown_groups.is_empty());

        let groups = SourceGroupChoices::from([
            (source_preference_key(&CaptureSource::System), String::new()),
            (
                source_preference_key(&CaptureSource::Addon {
                    addon_id: "com.example.decoder".into(),
                    source_id: "main".into(),
                    name: "Decoder".into(),
                }),
                "objects".into(),
            ),
        ]);
        let writer = PreferenceWriter::new(preferences.clone()).unwrap();
        writer
            .save(Key::WideExport, WideExportMode::Multichannel.preference())
            .unwrap();
        writer
            .save(Key::ShownGroups, &serialize_shown_groups(&groups))
            .unwrap();
        writer.flush().unwrap();
        let loaded = preferences.load();
        assert_eq!(loaded.wide_export, WideExportMode::Multichannel);
        assert_eq!(loaded.shown_groups, groups);
        assert_eq!(loaded.sources, old.sources);
        assert_eq!(loaded.history, old.history);
        assert_eq!(loaded.wav_profile, old.wav_profile);
        assert_eq!(loaded.wav_force_mono, old.wav_force_mono);
        assert_eq!(
            fs::read_to_string(folder.join("capture-sources")).unwrap(),
            "0\tsystem\noff"
        );
        assert_eq!(
            fs::read_to_string(folder.join("older-version-unknown-option")).unwrap(),
            "leave this alone"
        );
        assert!(folder.join("wide-lane-export-v1").is_file());
        assert!(folder.join("shown-channel-groups-v1").is_file());
    }

    #[test]
    fn invalid_new_preferences_use_safe_defaults_and_leave_saved_values_intact() {
        let preferences = workspace_preferences("wide-export-invalid");
        for mode in WideExportMode::ALL {
            preferences
                .save(Key::WideExport, mode.preference())
                .unwrap();
            assert_eq!(preferences.load().wide_export, mode);
        }
        preferences.save(Key::WideExport, "future-mode").unwrap();
        preferences.save(Key::ShownGroups, "{invalid json").unwrap();
        let loaded = preferences.load();
        assert_eq!(loaded.wide_export, WideExportMode::BinauralFiles);
        assert!(loaded.shown_groups.is_empty());
        assert_eq!(
            preferences.read(Key::WideExport).as_deref(),
            Some("future-mode")
        );
        assert_eq!(
            preferences.read(Key::ShownGroups).as_deref(),
            Some("{invalid json")
        );
    }

    #[test]
    fn loading_hotkeys_leaves_missing_corrupt_and_future_files_untouched() {
        let preferences = workspace_preferences("hotkeys-safe-load");
        let file = preferences.directory.as_ref().unwrap().join("hotkeys-v1");
        assert_eq!(Settings::default().hotkeys, Hotkeys::defaults());
        assert_eq!(preferences.load().hotkeys, Hotkeys::defaults());
        assert!(!file.exists());

        for text in [
            "{damaged json",
            r#"{"version":2,"bindings":{"clear":[]}}"#,
            r#"{"version":1,"bindings":[]}"#,
        ] {
            preferences.save(Key::Hotkeys, text).unwrap();
            assert_eq!(preferences.load().hotkeys, Hotkeys::defaults());
            assert_eq!(fs::read_to_string(&file).unwrap(), text);
        }
    }

    #[test]
    fn hotkey_settings_keep_valid_overrides_and_skip_unknown_keys_and_actions() {
        use crate::hotkeys::{Action, Binding, Key as HotkeyKey, Modifiers};

        let preferences = workspace_preferences("hotkeys-partial");
        let text = r#"{
            "version": 1,
            "bindings": {
                "future-action": {"a-future-shape": true},
                "toggle-capture": [{"type":"chord","key":"future-key"}],
                "clear": [],
                "cancel-selection": [
                    {"type":"chord","key":"f6"},
                    {"type":"chord","key":"another-future-key"}
                ],
                "separate-lanes": [{"type":"chord","key":"tab"}],
                "keep-on-top": [{"type":"chord","key":"x","modifiers":{"ctrl":"yes"}}],
                "save-snapshot-global": [{"type":"right-ctrl-tap"}]
            }
        }"#;
        preferences.save(Key::Hotkeys, text).unwrap();
        let defaults = Hotkeys::defaults();
        let hotkeys = preferences.load().hotkeys;
        for action in [
            Action::ToggleCapture,
            Action::SeparateLanes,
            Action::KeepOnTop,
            Action::SaveSnapshotGlobal,
            Action::OpenOptions,
        ] {
            assert_eq!(hotkeys.bindings(action), defaults.bindings(action), "{action:?}");
        }
        assert_eq!(
            hotkeys.bindings(Action::CancelSelection),
            &[Binding::Chord {
                modifiers: Modifiers::NONE,
                key: HotkeyKey::F6,
            }]
        );
        assert!(hotkeys.bindings(Action::Clear).is_empty());
        assert_eq!(preferences.read(Key::Hotkeys).as_deref(), Some(text));
    }

    #[test]
    fn preference_writer_saves_edited_hotkeys_as_one_document() {
        use crate::hotkeys::Action;

        let preferences = workspace_preferences("hotkeys-save");
        let mut hotkeys = preferences.load().hotkeys;
        hotkeys.clear(Action::SaveSnapshotGlobal);
        let writer = PreferenceWriter::new(preferences.clone()).unwrap();
        writer.save(Key::Hotkeys, &hotkeys.to_json()).unwrap();
        writer.flush().unwrap();
        assert_eq!(preferences.load().hotkeys, hotkeys);
        assert!(
            preferences
                .directory
                .as_ref()
                .unwrap()
                .join("hotkeys-v1")
                .is_file()
        );
    }

    #[test]
    fn the_pair_render_defaults_to_stereo_and_keeps_what_it_cannot_read() {
        let preferences = workspace_preferences("pair-render");
        assert_eq!(preferences.load().pair_render, PairRender::Stereo);
        assert!(!preferences.load().pair_render.binaural());
        for mode in PairRender::ALL {
            preferences
                .save(Key::PairRender, mode.preference())
                .unwrap();
            assert_eq!(preferences.load().pair_render, mode);
        }
        assert!(preferences.load().pair_render.binaural());
        preferences.save(Key::PairRender, "surround").unwrap();
        assert_eq!(preferences.load().pair_render, PairRender::Stereo);
        assert_eq!(
            preferences.read(Key::PairRender).as_deref(),
            Some("surround")
        );
    }

    #[test]
    fn addon_sources_round_trip_by_pair_and_do_not_store_sender_instances() {
        let sources = ["main", "objects"].map(|source_id| {
            Some(SourceSettings {
                source: CaptureSource::Addon {
                    addon_id: "com.example.decoder".into(),
                    source_id: source_id.into(),
                    name: "音訊來源".into(),
                },
                route: ChannelRoute::Merged,
            })
        });
        let preferences = workspace_preferences("addon-sources");
        let stored = serialize_sources(&sources);
        assert!(stored.contains("\taddon\t"));
        assert!(!stored.contains("\tsender\t"));
        preferences.save(Key::CaptureSources, &stored).unwrap();
        assert_eq!(preferences.load().sources, sources);

        let unnamed = [
            Some(SourceSettings {
                source: CaptureSource::Addon {
                    addon_id: "com.example.decoder".into(),
                    source_id: "main".into(),
                    name: String::new(),
                },
                route: ChannelRoute::Merged,
            }),
            None,
        ];
        assert_eq!(parse_sources(&serialize_sources(&unnamed)), Some(unnamed));
    }

    #[test]
    fn source_group_identity_ignores_names_and_keeps_source_pairs_separate() {
        let source = |addon_id: &str, source_id: &str, name: &str| CaptureSource::Addon {
            addon_id: addon_id.into(),
            source_id: source_id.into(),
            name: name.into(),
        };
        let key = source_preference_key(&source("com.example.a", "main", "First name"));
        assert_eq!(
            key,
            source_preference_key(&source("com.example.a", "main", "New name"))
        );
        assert_ne!(
            key,
            source_preference_key(&source("com.example.b", "main", "First name"))
        );
        assert_ne!(
            key,
            source_preference_key(&source("com.example.a", "objects", "First name"))
        );
        let groups = SourceGroupChoices::from([(key, "物件".into())]);
        assert_eq!(
            parse_shown_groups(&serialize_shown_groups(&groups)),
            Some(groups)
        );
    }

    #[test]
    fn injected_preference_root_uses_settings_subfolder() {
        let parent = workspace_preferences("injected-root");
        let root = parent.directory.unwrap();
        let preferences = Preferences::in_data_root(&root);
        preferences.save(Key::WideExport, "multichannel").unwrap();
        assert!(root.join("Settings/wide-lane-export-v1").is_file());
        assert_eq!(preferences.load().wide_export, WideExportMode::Multichannel);
        assert!(
            Preferences::in_data_root("relative")
                .save(Key::WideExport, "mono-files")
                .is_err()
        );
    }
}

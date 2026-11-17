use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    AutoResumeDelay, ChannelRoute, DurationChoice, WavExportProfile, WavSampleFormat,
};

use crate::controller::{CaptureSource, SourceSettings};

const MAX_VALUE_BYTES: u64 = 128 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
pub(crate) const DEFAULT_ACCENT: [u8; 3] = [103, 231, 181];

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
    Transparency,
    Accent,
    Meter,
    MeterB,
    SplitWaveform,
    CaptureSources,
    SourceNames,
    WavSampleRate,
    WavSampleFormat,
    WavForceMono,
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
}

impl Key {
    fn filename(self) -> &'static str {
        match self {
            Self::History => "history-seconds",
            // The old preference used 0 for Off.
            Self::IdleResume => "idle-resume-delay-seconds",
            Self::RetainSilence => "retain-silence",
            Self::Transparency => "transparency-percent",
            Self::Accent => "theme-color",
            Self::Meter => "meter-unit",
            Self::MeterB => "meter-b-unit",
            Self::SplitWaveform => "split-waveform",
            Self::CaptureSources => "capture-sources",
            Self::SourceNames => "source-file-names",
            Self::WavSampleRate => "wav-sample-rate",
            Self::WavSampleFormat => "wav-sample-format",
            Self::WavForceMono => "wav-force-mono",
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
        }
    }
}

pub(crate) struct Settings {
    pub(crate) history: DurationChoice,
    pub(crate) idle_resume: AutoResumeDelay,
    pub(crate) retain_silence: bool,
    pub(crate) transparency: u8,
    pub(crate) accent: [u8; 3],
    pub(crate) meter_mode: MeterMode,
    pub(crate) meter_b_mode: MeterMode,
    pub(crate) split_waveform: bool,
    pub(crate) sources: [Option<SourceSettings>; 2],
    pub(crate) source_names: SourceNames,
    pub(crate) wav_profile: WavExportProfile,
    pub(crate) wav_force_mono: bool,
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
            retain_silence: false,
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
            source_names: SourceNames::default(),
            wav_profile: WavExportProfile::default(),
            wav_force_mono: false,
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
        }
    }
}

#[derive(Clone)]
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
        settings.retain_silence = self.read_number::<u8>(Key::RetainSilence) == Some(1);
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
        self.validate_save(value)?;
        let directory = self.directory.as_ref().expect("validated preferences directory");
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
        self.commands.send(PreferenceCommand::Flush(reply)).map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "settings worker is closed")
        })?;
        result.recv().map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "settings worker stopped before flushing")
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
                return failed.first().map_or(Ok(()), |failure| Err(failure.error()));
            };
            let mut pending = Vec::new();
            let mut flush = None;
            let mut shutdown = false;
            absorb_preference(first, &mut pending, &mut flush, &mut shutdown);
            let deadline = Instant::now() + PREFERENCE_COALESCE_WINDOW;
            while flush.is_none() && !shutdown {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() { break; }
                match requests.recv_timeout(remaining) {
                    Ok(command) => absorb_preference(command, &mut pending, &mut flush, &mut shutdown),
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => shutdown = true,
                }
            }
            // Values already queued while the previous disk write was slow
            // belong to this batch even if a scheduling delay used the window.
            for _ in 0..MAX_QUEUED_PREFERENCE_DRAIN {
                if flush.is_some() || shutdown { break; }
                match requests.try_recv() {
                    Ok(command) => absorb_preference(command, &mut pending, &mut flush, &mut shutdown),
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
                let result = failed.first().map_or(Ok(()), |failure| Err(failure.error()));
                let _ = reply.send(result);
            }
            if shutdown {
                return failed.first().map_or(Ok(()), |failure| Err(failure.error()));
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
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    fn workspace_preferences(label: &str) -> Preferences {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap().to_path_buf();
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        Preferences {
            directory: Some(workspace.join("target/preference-writer-tests").join(format!(
                "{label}-{}-{sequence}", std::process::id()
            ))),
        }
    }

    #[test]
    fn slow_disk_never_blocks_enqueue_and_pending_keys_keep_their_latest_order() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&writes);
        let (entered, entering) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let mut first = true;
        let writer = PreferenceWriter::with_sink(workspace_preferences("slow"), move |key, value| {
            if first {
                first = false;
                entered.send(()).unwrap();
                released.recv().unwrap();
            }
            observed.lock().unwrap().push((key, value.to_owned()));
            Ok(())
        }).unwrap();
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
        let result = queue.recv_timeout(Duration::from_millis(100));
        let returned_while_disk_busy = result.is_ok();
        release.send(()).unwrap();
        ui.join().unwrap();
        assert!(returned_while_disk_busy, "enqueue waited for the disk sink");
        let writer = result.unwrap();
        writer.flush().unwrap();
        assert_eq!(*writes.lock().unwrap(), [
            (Key::Accent, "initial".into()),
            (Key::Transparency, "25".into()),
            (Key::History, "600".into()),
        ]);
    }

    #[test]
    fn failed_disk_writes_are_reported_and_flush_preserves_the_latest_retry() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&writes);
        let writer = PreferenceWriter::with_sink(workspace_preferences("errors"), move |key, value| {
            observed.lock().unwrap().push((key, value.to_owned()));
            if value == "10" {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "simulated disk denial"))
            } else {
                Ok(())
            }
        }).unwrap();
        writer.save(Key::History, "10").unwrap();
        assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::PermissionDenied);
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
        assert!(writes[..writes.len() - 1].iter().all(|write| write == &(Key::History, "10".into())));
    }

    #[test]
    fn flush_barrier_does_not_wait_for_saves_queued_after_it() {
        let (late, late_started) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let writer = PreferenceWriter::with_sink(workspace_preferences("barrier"), move |_, value| {
            if value == "600" {
                late.send(()).unwrap();
                released.recv().unwrap();
            }
            Ok(())
        }).unwrap();
        writer.save(Key::History, "10").unwrap();
        let (barrier, result) = mpsc::sync_channel(1);
        writer.commands.send(PreferenceCommand::Flush(barrier)).unwrap();
        writer.save(Key::History, "600").unwrap();
        late_started.recv().unwrap();
        let completed_before_late_write = result.recv_timeout(Duration::from_millis(100));
        release.send(()).unwrap();
        assert!(completed_before_late_write.is_ok(), "flush crossed its queue barrier");
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
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "simulated disk denial"))
            }
        }).unwrap();
        writer.save(Key::History, "600").unwrap();
        for _ in 0..2 {
            assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::PermissionDenied);
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
        let reloaded = Preferences { directory: preferences.directory.clone() }.load();
        assert_eq!(reloaded.history, DurationChoice::Seconds30);
        assert_eq!(reloaded.idle_resume, AutoResumeDelay::from_seconds(5).unwrap());
    }

    #[test]
    fn invalid_preferences_fail_synchronously_before_any_disk_work() {
        assert!(PreferenceWriter::new(Preferences { directory: None }).is_err());
        let writes = Arc::new(Mutex::new(0));
        let observed = Arc::clone(&writes);
        let writer = PreferenceWriter::with_sink(workspace_preferences("validation"), move |_, _| {
            *observed.lock().unwrap() += 1;
            Ok(())
        }).unwrap();
        assert_eq!(writer.save(Key::History, &"x".repeat(MAX_VALUE_BYTES as usize + 1)).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        writer.flush().unwrap();
        assert_eq!(*writes.lock().unwrap(), 0);
    }

    #[test]
    #[ignore = "diagnostic durable writes in the workspace; not a reboot or GUI latency test"]
    fn preference_save_latency_probe() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap().to_path_buf();
        let output = workspace.join("target/preference-perf-probe");
        fs::create_dir_all(&output).unwrap();
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let run = output.join(format!("run-{}-{sequence}", std::process::id()));
        let mut fresh = Vec::new();
        for index in 0..8 {
            let preferences = Preferences { directory: Some(run.join(format!("fresh-{index}"))) };
            let started = std::time::Instant::now();
            preferences.save(Key::History, "600").unwrap();
            fresh.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let preferences = Preferences { directory: Some(run.join("existing")) };
        preferences.save(Key::History, "600").unwrap();
        let mut warm = Vec::new();
        for index in 0..64 {
            let started = std::time::Instant::now();
            preferences.save(Key::History, if index % 2 == 0 { "10" } else { "600" }).unwrap();
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
        fs::write(run.join("synchronous-save-results.json"), serde_json::to_vec_pretty(&result).unwrap()).unwrap();
        println!("{result}");
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
        assert!(preferences.directory.as_ref().unwrap().join("retain-silence").is_file());

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
        writer.save(Key::IdleResume, &AutoResumeDelay::Off.seconds().to_string()).unwrap();
        writer.flush().unwrap();
        // The file keeps the value older versions wrote for this choice.
        assert_eq!(preferences.read(Key::IdleResume).as_deref(), Some("0"));
        assert_eq!(preferences.load().idle_resume, AutoResumeDelay::Off);

        for choice in AutoResumeDelay::ALL {
            preferences.save(Key::IdleResume, &choice.seconds().to_string()).unwrap();
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
            preferences.save(Key::StatusMark, mark.preference()).unwrap();
            assert_eq!(preferences.load().status_mark, mark);
        }
        preferences.save(Key::StatusMark, "sometimes").unwrap();
        assert_eq!(preferences.load().status_mark, StatusMark::Always);

        for (key, shown) in [(Key::ShowTitle, false), (Key::ShowMeters, false)] {
            preferences.save(key, "0").unwrap();
            let settings = preferences.load();
            assert_eq!(
                if key == Key::ShowTitle { settings.show_title } else { settings.show_meters },
                shown
            );
            preferences.save(key, "1").unwrap();
        }
        let settings = preferences.load();
        assert!(settings.show_title && settings.show_meters);

        for width in BORDER_WIDTHS {
            preferences.save(Key::WaveformBorderWidth, &width.to_string()).unwrap();
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
            preferences.save(Key::OutputCleanup, &days.to_string()).unwrap();
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
            parse_sources(&serialize_sources(&one_source)),
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

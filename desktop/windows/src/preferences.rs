use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use backtracking_core::DurationChoice;

const MAX_VALUE_BYTES: u64 = 128 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

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
    Meter,
    Output,
    AlwaysOnTop,
}

impl Key {
    fn filename(self) -> &'static str {
        match self {
            Self::History => "history-seconds",
            Self::IdleResume => "idle-resume-seconds",
            Self::Transparency => "transparency-percent",
            Self::Meter => "meter-unit",
            Self::Output => "output-folder",
            Self::AlwaysOnTop => "always-on-top",
        }
    }
}

pub(crate) struct Settings {
    pub(crate) history: DurationChoice,
    pub(crate) idle_resume: Option<DurationChoice>,
    pub(crate) transparency: u8,
    pub(crate) meter_mode: MeterMode,
    pub(crate) output: PathBuf,
    pub(crate) pinned: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            history: DurationChoice::Seconds20,
            idle_resume: Some(DurationChoice::Seconds10),
            transparency: 0,
            meter_mode: MeterMode::Dbfs,
            output: home.join("Backtracking").join("output"),
            pinned: true,
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

        if let Some(history) = self.read_number(Key::History).and_then(duration) {
            settings.history = history;
        }
        match self.read_number(Key::IdleResume) {
            Some(0) => settings.idle_resume = None,
            Some(value) if duration(value).is_some() => settings.idle_resume = duration(value),
            _ => {}
        }
        if let Some(percent @ (0 | 25 | 50 | 75 | 100)) = self.read_number(Key::Transparency) {
            settings.transparency = percent;
        }
        settings.meter_mode = match self.read(Key::Meter).as_deref().map(str::trim) {
            Some("VU") => MeterMode::Vu,
            Some("LUFS") => MeterMode::Lufs,
            _ => MeterMode::Dbfs,
        };
        if let Some(path) = self.read(Key::Output)
            && !path.bytes().any(|byte| byte < 32)
        {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                settings.output = path;
            }
        }
        match self.read_number(Key::AlwaysOnTop) {
            Some(0) => settings.pinned = false,
            Some(1) => settings.pinned = true,
            _ => {}
        }
        settings
    }

    fn read_number(&self, key: Key) -> Option<u8> {
        self.read(key)?.trim().parse().ok()
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

fn duration(value: u8) -> Option<DurationChoice> {
    match value {
        10 => Some(DurationChoice::Seconds10),
        20 => Some(DurationChoice::Seconds20),
        30 => Some(DurationChoice::Seconds30),
        _ => None,
    }
}

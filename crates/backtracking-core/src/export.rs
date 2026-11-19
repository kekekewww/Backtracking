use std::f64::consts::PI;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{ChannelRoute, Fades, FrozenTrack, Snapshot};

const PHASES: usize = 256;
/// How many output frames one piece of a clip holds, whatever the length of the clip.
/// The writer holds one piece at a time, and each resampling thread at most two more.
const CHUNK_FRAMES: usize = 4096;
/// The most threads one export resamples on, however many the processor offers.
const MAX_RESAMPLING_THREADS: usize = 8;

/// Threads an export may resample on: half of the processor's, so capture, the UI and
/// a plug-in host's audio engine keep the rest, and never more than eight.
fn resampling_threads() -> usize {
    thread::available_parallelism()
        .map_or(1, |threads| threads.get() / 2)
        .clamp(1, MAX_RESAMPLING_THREADS)
}

/// One lane of audio the WAV exporter reads a piece at a time, so a lane held in
/// shared pages is written without first being copied into one buffer. Resampling
/// reads one lane from several threads at once.
pub trait TrackAudio: Sync {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> usize;
    /// The frames this lane holds, on the retained-audio timeline.
    fn frame_range(&self) -> Range<u64>;
    /// Fills `output` with whole interleaved frames, `first` counted from the lane's
    /// first frame. Returns false unless the lane holds all of them.
    fn read_frames(&self, first: usize, output: &mut [f32]) -> bool;
}

impl TrackAudio for Snapshot {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn frame_range(&self) -> Range<u64> {
        Snapshot::frame_range(self)
    }

    fn read_frames(&self, first: usize, output: &mut [f32]) -> bool {
        let source = first
            .checked_mul(self.channels)
            .and_then(|start| self.samples.get(start..)?.get(..output.len()));
        source
            .map(|source| output.copy_from_slice(source))
            .is_some()
    }
}

impl TrackAudio for FrozenTrack {
    fn sample_rate(&self) -> u32 {
        FrozenTrack::sample_rate(self)
    }

    fn channels(&self) -> usize {
        FrozenTrack::channels(self)
    }

    fn frame_range(&self) -> Range<u64> {
        FrozenTrack::frame_range(self)
    }

    fn read_frames(&self, first: usize, output: &mut [f32]) -> bool {
        self.first_frame()
            .checked_add(first as u64)
            .is_some_and(|first| FrozenTrack::read_frames(self, first, output))
    }
}

/// The frames one WAV is written from. The writer pulls them in bounded pieces.
struct Clip<'a> {
    sample_rate: u32,
    channels: usize,
    frames: usize,
    /// False when the source ends inside a frame.
    whole_frames: bool,
    /// Fills its buffer with whole interleaved frames, starting at the given clip frame.
    read: &'a (dyn Fn(usize, &mut [f32]) -> io::Result<()> + Sync),
}

impl Clip<'_> {
    fn read(&self, first: usize, output: &mut [f32]) -> io::Result<()> {
        (self.read)(first, output)
    }
}

/// Runs `write` on a clip that reads `interleaved` in place.
fn with_slice_clip<R>(
    sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
    write: impl FnOnce(&Clip) -> R,
) -> R {
    let read = |first: usize, output: &mut [f32]| {
        output.copy_from_slice(&interleaved[first * channels..][..output.len()]);
        Ok(())
    };
    write(&Clip {
        sample_rate,
        channels,
        frames: interleaved.len() / channels.max(1),
        whole_frames: channels != 0 && interleaved.len() % channels == 0,
        read: &read,
    })
}

/// Runs `write` on the clip of `frames`, counted from the first frame `track` holds, with
/// `fades` applied to its ends.
fn with_track_clip<T: TrackAudio + ?Sized, R>(
    track: &T,
    frames: &Range<usize>,
    fades: Fades,
    write: impl FnOnce(&Clip) -> R,
) -> R {
    let length = (frames.end - frames.start) as u64;
    let channels = track.channels();
    let read = |first: usize, output: &mut [f32]| {
        if track.read_frames(frames.start + first, output) {
            // The ramps are laid on the source frames, before any resampling.
            fades.apply(output, channels, first as u64, length);
            Ok(())
        } else {
            Err(invalid_tracks())
        }
    };
    write(&Clip {
        sample_rate: track.sample_rate(),
        channels: track.channels(),
        frames: frames.end - frames.start,
        whole_frames: true,
        read: &read,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WavSampleFormat {
    Pcm16,
    Pcm24,
    Float32,
}

impl WavSampleFormat {
    pub const fn bits_per_sample(self) -> u16 {
        match self {
            Self::Pcm16 => 16,
            Self::Pcm24 => 24,
            Self::Float32 => 32,
        }
    }

    /// The only 32-bit encoding supported by Backtracking is IEEE float.
    pub const fn from_bits_per_sample(bits: u32) -> Option<Self> {
        match bits {
            16 => Some(Self::Pcm16),
            24 => Some(Self::Pcm24),
            32 => Some(Self::Float32),
            _ => None,
        }
    }

    fn format_chunk_size(self, channels: u16) -> u32 {
        if channels > 2 {
            return 40; // WAVEFORMATEXTENSIBLE, with the native channel order.
        }
        match self {
            Self::Pcm16 => 16,
            Self::Pcm24 => 40,   // WAVEFORMATEXTENSIBLE
            Self::Float32 => 18, // WAVEFORMATEX
        }
    }

    fn riff_overhead(self, channels: u16) -> u32 {
        let fact_bytes = if self == Self::Float32 { 12 } else { 0 };
        4 + 8 + self.format_chunk_size(channels) + fact_bytes + 8
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WavExportProfile {
    pub sample_rate: u32,
    pub sample_format: WavSampleFormat,
}

impl WavExportProfile {
    pub const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];
}

impl Default for WavExportProfile {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            sample_format: WavSampleFormat::Float32,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WavChannelMap {
    Unchanged,
    Left,
    Right,
    Side,
    Downmix,
}

impl WavChannelMap {
    fn from_route(route: ChannelRoute, channels: usize, force_mono: bool) -> io::Result<Self> {
        // All channels keeps every captured channel, as Merged does. A device that offered more
        // than two channels when the route was chosen may give one or two now; its audio is
        // then captured merged, and must be written the same way.
        let route = match route {
            ChannelRoute::AllChannels => ChannelRoute::Merged,
            route => route,
        };
        match (route, channels, force_mono) {
            (ChannelRoute::Merged, 2, true) => Ok(Self::Downmix),
            (ChannelRoute::Merged, 1..=64, false) => Ok(Self::Unchanged),
            (ChannelRoute::Left, 1, false) => Ok(Self::Left),
            (ChannelRoute::Right, 1, false) => Ok(Self::Right),
            (ChannelRoute::Side, 1, false) => Ok(Self::Side),
            (_, 1, _) => Ok(Self::Unchanged),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "channel route does not match the captured audio",
            )),
        }
    }

    fn output_channels(self, source_channels: usize) -> usize {
        match self {
            Self::Unchanged => source_channels,
            Self::Downmix => 1,
            Self::Left | Self::Right | Self::Side => 2,
        }
    }

    fn map_frame(self, frame: &[f32]) -> [f32; 2] {
        match self {
            Self::Left => [frame[0], 0.0],
            Self::Right => [0.0, frame[0]],
            Self::Side => [frame[0], -frame[0]],
            Self::Downmix => [frame[0] * 0.5 + frame[1] * 0.5, 0.0],
            Self::Unchanged => unreachable!(),
        }
    }
}

/// Writes a new 1- to 64-channel 48 kHz, IEEE float32 WAV. The caller can expose
/// the path for dragging after this returns successfully.
pub fn write_wav(
    path: &Path,
    source_sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
) -> io::Result<()> {
    write_wav_with_profile(
        path,
        source_sample_rate,
        channels,
        interleaved,
        WavExportProfile::default(),
    )
}

/// Writes a new 1- to 64-channel WAV using the requested export profile.
/// More than two channels use WAVE_FORMAT_EXTENSIBLE with an unspecified speaker mask,
/// preserving their order without guessing speaker positions from channel labels.
pub fn write_wav_with_profile(
    path: &Path,
    source_sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
    profile: WavExportProfile,
) -> io::Result<()> {
    with_slice_clip(source_sample_rate, channels, interleaved, |clip| {
        write_wav_mapped(path, clip, profile, WavChannelMap::Unchanged)
    })
}

fn write_wav_mapped(
    path: &Path,
    clip: &Clip,
    profile: WavExportProfile,
    channel_map: WavChannelMap,
) -> io::Result<()> {
    WavFile::create(path, clip, profile, channel_map)?.write(clip, resampling_threads())
}

/// A new WAV file sized for one clip, with no audio in it yet. Creating it claims the
/// name, so both files of a pair are claimed before either is written.
struct WavFile<'p> {
    path: &'p Path,
    file: File,
    profile: WavExportProfile,
    channel_map: WavChannelMap,
    channels: u16,
    frames: u32,
    data_bytes: u32,
}

impl<'p> WavFile<'p> {
    fn create(
        path: &'p Path,
        clip: &Clip,
        profile: WavExportProfile,
        channel_map: WavChannelMap,
    ) -> io::Result<Self> {
        let source_sample_rate = clip.sample_rate;
        let channels = clip.channels;
        if source_sample_rate == 0 || !(1..=64).contains(&channels) || !clip.whole_frames {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a nonzero sample rate and whole frames of 1 to 64 channels",
            ));
        }
        if !WavExportProfile::SAMPLE_RATES.contains(&profile.sample_rate) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected an export rate of 44,100, 48,000 or 96,000 Hz",
            ));
        }

        let source_frames = clip.frames;
        let rate = u128::from(source_sample_rate);
        let rounded = (source_frames as u128 * u128::from(profile.sample_rate) + rate / 2) / rate;
        let frames = rounded.max((source_frames != 0) as u128);
        let output_channels = channel_map.output_channels(channels) as u16;
        let frame_bytes = output_channels * (profile.sample_format.bits_per_sample() / 8);
        let data_bytes = frames * u128::from(frame_bytes);
        let riff_size = data_bytes
            + data_bytes % 2
            + u128::from(profile.sample_format.riff_overhead(output_channels));
        if riff_size > u128::from(u32::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "selection exceeds the RIFF WAV size limit",
            ));
        }

        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        Ok(Self {
            path,
            file,
            profile,
            channel_map,
            channels: output_channels,
            frames: frames as u32,
            data_bytes: data_bytes as u32,
        })
    }

    /// Writes `clip`, the clip the file was created for, resampling on up to `threads`
    /// threads. A file that cannot be completed is removed.
    fn write(self, clip: &Clip, threads: usize) -> io::Result<()> {
        let result = {
            let mut writer = BufWriter::new(self.file);
            write_wave_from_clip(
                &mut writer,
                self.profile,
                self.channels,
                self.frames,
                self.data_bytes,
                clip,
                self.channel_map,
                threads,
            )
            .and_then(|()| writer.flush())
        };
        if result.is_err() {
            let _ = fs::remove_file(self.path);
        }
        result
    }

    /// Closes and removes a file whose partner could not be created.
    fn discard(self) -> io::Result<()> {
        drop(self.file);
        fs::remove_file(self.path)
    }
}

impl WavExportProfile {
    /// Write a create-new WAV from consecutive capture-rate blocks. A single resampling
    /// worker reads overlapping windows, so retain their overlap without rewinding the FIR.
    #[allow(clippy::too_many_arguments)]
    pub fn write_rendered(
        self,
        path: &Path,
        sample_rate: u32,
        channels: usize,
        frames: usize,
        fades: Fades,
        render: impl FnMut(usize, &mut [f32]) -> io::Result<()> + Send,
    ) -> io::Result<()> {
        let state = std::sync::Mutex::new((0usize, Vec::<f32>::new(), render));
        let read = |first: usize, output: &mut [f32]| {
            let mut state = state
                .lock()
                .map_err(|_| io::Error::other("render worker stopped"))?;
            let (start, window, render) = &mut *state;
            let end = *start + window.len() / channels;
            if first < *start || first > end {
                return Err(invalid_tracks());
            }
            window.drain(..(first - *start) * channels);
            *start = first;
            let cached = window.len();
            if output.len() > cached {
                window.resize(output.len(), 0.);
                render(end, &mut window[cached..])?;
                fades.apply(&mut window[cached..], channels, end as u64, frames as u64);
            }
            output.copy_from_slice(&window[..output.len()]);
            Ok(())
        };
        let clip = Clip {
            sample_rate,
            channels,
            frames,
            whole_frames: true,
            read: &read,
        };
        WavFile::create(path, &clip, self, WavChannelMap::Unchanged)?.write(&clip, 1)
    }

    /// Concurrent file renders may use this many single-threaded writers.
    pub fn worker_limit(self) -> usize {
        resampling_threads()
    }
}

/// Creates the output directory if needed and saves a uniquely named clip.
pub fn export_wav(
    output_dir: &Path,
    source_sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
) -> io::Result<PathBuf> {
    export_wav_with_profile(
        output_dir,
        source_sample_rate,
        channels,
        interleaved,
        WavExportProfile::default(),
    )
}

/// Saves a uniquely named clip using the requested WAV profile.
pub fn export_wav_with_profile(
    output_dir: &Path,
    source_sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
    profile: WavExportProfile,
) -> io::Result<PathBuf> {
    export_routed_wav_with_profile(
        output_dir,
        source_sample_rate,
        channels,
        interleaved,
        profile,
        ChannelRoute::Merged,
        false,
    )
}

/// Exports already-routed capture samples with their intended speaker placement.
/// Forcing mono keeps a routed lane's samples intact and averages a stereo merged lane.
/// A merged lane can hold up to 64 native channels; more than two requires `force_mono = false`.
pub fn export_routed_wav_with_profile(
    output_dir: &Path,
    source_sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
    profile: WavExportProfile,
    route: ChannelRoute,
    force_mono: bool,
) -> io::Result<PathBuf> {
    export_routed_wav_named(
        output_dir,
        source_sample_rate,
        channels,
        interleaved,
        profile,
        route,
        force_mono,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn export_routed_wav_named(
    output_dir: &Path,
    source_sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
    profile: WavExportProfile,
    route: ChannelRoute,
    force_mono: bool,
    tempo_bpm: Option<f64>,
) -> io::Result<PathBuf> {
    with_slice_clip(source_sample_rate, channels, interleaved, |clip| {
        export_clip_named(
            output_dir, clip, profile, route, force_mono, tempo_bpm, &TakeLabel::default(),
        )
    })
}

fn export_clip_named(
    output_dir: &Path,
    clip: &Clip,
    profile: WavExportProfile,
    route: ChannelRoute,
    force_mono: bool,
    tempo_bpm: Option<f64>,
    label: &TakeLabel,
) -> io::Result<PathBuf> {
    let channel_map = WavChannelMap::from_route(route, clip.channels, force_mono)?;
    fs::create_dir_all(output_dir)?;
    let take = dated_take(clip_seconds(clip.frames, clip.sample_rate), tempo_bpm);
    let mut attempt = 1;
    loop {
        let path = output_dir.join(format!("{}.wav", label.file_stem(&take, attempt)));
        match write_wav_mapped(&path, clip, profile, channel_map) {
            Ok(()) => return Ok(path),
            // Another export in the same second, or a file that was already there.
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => attempt += 1,
            Err(err) => return Err(err),
        }
    }
}

/// Calendar fields of the moment a take was exported, in the user's time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TakeTime {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl TakeTime {
    /// Local wall-clock time, or UTC where the platform cannot report it.
    fn now() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        local_time(seconds).unwrap_or_else(|| Self::from_utc_seconds(seconds))
    }

    fn from_utc_seconds(seconds: u64) -> Self {
        let days = (seconds / 86_400) as i64;
        let time = (seconds % 86_400) as u32;
        // Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's civil_from_days).
        let shifted = days + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        };
        Self {
            year: (year_of_era + era * 400 + i64::from(month <= 2)) as i32,
            month: month as u32,
            day: (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32,
            hour: time / 3_600,
            minute: time / 60 % 60,
            second: time % 60,
        }
    }
}

// The core has no dependencies, so local time comes straight from the platform.
#[cfg(windows)]
fn local_time(_seconds: u64) -> Option<TakeTime> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTimeFields {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(time: *mut SystemTimeFields);
    }
    let mut fields = SystemTimeFields::default();
    // SAFETY: GetLocalTime fills the SYSTEMTIME it is given and cannot fail.
    unsafe { GetLocalTime(&mut fields) };
    (fields.month != 0).then(|| TakeTime {
        year: i32::from(fields.year),
        month: u32::from(fields.month),
        day: u32::from(fields.day),
        hour: u32::from(fields.hour),
        minute: u32::from(fields.minute),
        second: u32::from(fields.second),
    })
}

// Every 64-bit Unix has a 64-bit `time_t`. Other Unix targets fall back to UTC.
#[cfg(all(unix, target_pointer_width = "64"))]
fn local_time(seconds: u64) -> Option<TakeTime> {
    use std::ffi::{c_char, c_int, c_long};

    // `struct tm` as laid out by glibc, musl and the BSD family (macOS included).
    #[repr(C)]
    struct Tm {
        second: c_int,
        minute: c_int,
        hour: c_int,
        day: c_int,
        month: c_int,
        year: c_int,
        weekday: c_int,
        day_of_year: c_int,
        daylight_saving: c_int,
        utc_offset: c_long,
        zone: *const c_char,
        // Slack for a C library whose `struct tm` carries more than the fields above.
        reserve: [u64; 8],
    }
    extern "C" {
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }
    let time = i64::try_from(seconds).ok()?;
    let mut fields = std::mem::MaybeUninit::<Tm>::zeroed();
    // SAFETY: both pointers are valid for the call, and `localtime_r` writes only a `struct tm`,
    // which is no larger than `Tm`. A zeroed `Tm` is a valid value should a field stay unwritten.
    let fields = unsafe {
        if localtime_r(&time, fields.as_mut_ptr()).is_null() {
            return None;
        }
        fields.assume_init()
    };
    Some(TakeTime {
        year: fields.year.checked_add(1_900)?,
        month: u32::try_from(fields.month.checked_add(1)?).ok()?,
        day: u32::try_from(fields.day).ok()?,
        hour: u32::try_from(fields.hour).ok()?,
        minute: u32::try_from(fields.minute).ok()?,
        second: u32::try_from(fields.second).ok()?,
    })
}

#[cfg(not(any(windows, all(unix, target_pointer_width = "64"))))]
fn local_time(_seconds: u64) -> Option<TakeTime> {
    None
}

fn clip_seconds(frames: usize, sample_rate: u32) -> f64 {
    frames as f64 / f64::from(sample_rate.max(1))
}

/// A lane of a two-source recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lane {
    A,
    B,
}

impl Lane {
    pub const BOTH: [Self; 2] = [Self::A, Self::B];

    /// The lane at `index` of a recording's tracks: 0 is A and 1 is B.
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::A),
            1 => Some(Self::B),
            _ => None,
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }

    pub const fn letter(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// The longest source name a file name keeps, in characters.
pub const SOURCE_NAME_LIMIT: usize = 48;

/// What one exported file's name says about its audio, ahead of when it was taken:
/// `Backtracking_<source>_<lane>_<date>_<time>_<length>s.wav`, leaving out what is empty.
/// A lane whose source has no name keeps the form the plug-in names its pairs with, the
/// letter after the take: `Backtracking_<date>_<time>_<length>s-A.wav`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TakeLabel {
    /// The name of the source the audio came from. Characters a file name cannot hold are
    /// left out when the name is written, and an empty name leaves the field out.
    pub source: String,
    /// The lane, given whenever two sources were recording, even if only this one is exported.
    pub lane: Option<Lane>,
}

impl TakeLabel {
    pub fn new(source: impl Into<String>, lane: Option<Lane>) -> Self {
        Self { source: source.into(), lane }
    }

    /// The label of an unnamed lane of a pair, or of a single file when `lane` is `None`.
    fn unnamed(lane: Option<Lane>) -> Self {
        Self { source: String::new(), lane }
    }

    /// How the names of this label's files begin: everything before the date, such as
    /// `Backtracking_System audio_A_`. Without a source name it is `Backtracking_`, since the
    /// lane then follows the take.
    pub fn file_prefix(&self) -> String {
        match (file_name_source(&self.source), self.lane) {
            (Some(source), Some(lane)) => format!("Backtracking_{source}_{}_", lane.letter()),
            (Some(source), None) => format!("Backtracking_{source}_"),
            (None, _) => String::from("Backtracking_"),
        }
    }

    /// Stems for successive attempts to claim the files of one take.
    pub fn stems(&self, frames: usize, sample_rate: u32) -> impl Iterator<Item = String> + '_ {
        let take = dated_take(clip_seconds(frames, sample_rate), None);
        (1..=u32::MAX).map(move |attempt| self.file_stem(&take, attempt))
    }

    pub fn channel_suffix(channel: usize, label: &str) -> String {
        format!(
            "-Ch{:02}_{}",
            channel + 1,
            channel_file_label(label, channel)
        )
    }

    /// The file name without `.wav` for attempt `attempt` at writing `take`, a [`dated_take`].
    fn file_stem(&self, take: &str, attempt: u32) -> String {
        let stem = self.file_prefix() + &numbered_take(take, attempt);
        match (self.lane, file_name_source(&self.source)) {
            (Some(lane), None) => format!("{stem}-{}", lane.letter()),
            _ => stem,
        }
    }
}

/// `name` as it can stand in a file name on Windows and macOS, or `None` when nothing is left.
/// Characters either system refuses count as spaces, runs of spaces close up to one, the ends
/// are trimmed, and the name stops at [`SOURCE_NAME_LIMIT`] characters.
pub fn file_name_source(name: &str) -> Option<String> {
    let mut cleaned = String::new();
    let mut length = 0;
    let mut gap = false;
    for character in name.chars() {
        if character.is_whitespace()
            || character.is_control()
            || matches!(character, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        {
            gap = length > 0;
            continue;
        }
        if gap {
            if length + 1 == SOURCE_NAME_LIMIT {
                break;
            }
            cleaned.push(' ');
            length += 1;
            gap = false;
        }
        cleaned.push(character);
        length += 1;
        if length == SOURCE_NAME_LIMIT {
            break;
        }
    }
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The part of a file name a DAW reads the take by: when it was taken, how long it is, and the
/// tempo if known. It follows the source and lane, which [`TakeLabel`] adds.
fn dated_take(seconds: f64, tempo_bpm: Option<f64>) -> String {
    dated_take_at(TakeTime::now(), seconds, tempo_bpm)
}

fn dated_take_at(time: TakeTime, seconds: f64, tempo_bpm: Option<f64>) -> String {
    let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
    let mut name = format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}_{seconds:.1}s",
        time.year, time.month, time.day, time.hour, time.minute, time.second,
    );
    if let Some(tempo) =
        tempo_bpm.filter(|tempo| tempo.is_finite() && (1.0..1_000.0).contains(tempo))
    {
        let tenths = (tempo * 10.0).round() as u32;
        name.push_str(&if tenths % 10 == 0 {
            format!("_{}bpm", tenths / 10)
        } else {
            format!("_{}.{}bpm", tenths / 10, tenths % 10)
        });
    }
    name
}

/// Whether `name` is a file name this module's WAV exports produce: the form with a source
/// and lane, the form without a source whose pairs end in `-A` and `-B`, or the first
/// `Backtracking-<milliseconds>-<process>-<serial>` one, each optionally followed by the
/// `-ChNN_<label>` of one channel of a wide lane. Housekeeping asks before it removes
/// anything, so a file the user put in an export folder is never touched.
pub fn is_export_file_name(name: &str) -> bool {
    if let Some(stem) = name.strip_suffix("-Positions.json") {
        return is_take_stem(stem);
    }
    let Some(stem) = name.strip_suffix(".wav") else {
        return false;
    };
    // A source's name may hold "-Ch" itself, so the channel can start at any of them.
    is_take_stem(stem)
        || ["-Stereo", "-Binaural", "-Mix-Binaural"]
            .iter()
            .any(|suffix| stem.strip_suffix(suffix).is_some_and(is_take_stem))
        || stem.strip_suffix("_binaural").is_some_and(|stem| {
            stem.match_indices("-Ch")
                .any(|(i, _)| is_channel_suffix(&stem[i + 3..]) && is_take_stem(&stem[..i]))
        })
        || stem
            .match_indices("-Ch")
            .any(|(index, _)| is_channel_suffix(&stem[index + 3..]) && is_take_stem(&stem[..index]))
}

/// Whether `suffix`, the part after `-Ch`, is a channel number and label as
/// [`export_channel_range_with_profile`] writes them.
fn is_channel_suffix(suffix: &str) -> bool {
    let Some((number, label)) = suffix.split_once('_') else {
        return false;
    };
    let Ok(channel) = number.parse::<usize>() else {
        return false;
    };
    number.len() == 2
        && (1..=64).contains(&channel)
        && !label.is_empty()
        && channel_file_label(label, channel - 1) == label
}

/// Whether `stem` is a file name without `.wav` and without a channel suffix.
fn is_take_stem(stem: &str) -> bool {
    // A lane follows the take only when no source is named.
    let stem = stem.strip_suffix("-A").or_else(|| stem.strip_suffix("-B")).unwrap_or(stem);
    if let Some(take) = stem.strip_prefix("Backtracking_") {
        // The source's name may hold underscores itself, so the date can start at any field.
        is_dated_take(take)
            || take
                .match_indices('_')
                .any(|(index, _)| index > 0 && is_dated_take(&take[index + 1..]))
    } else if let Some(take) = stem.strip_prefix("Backtracking-") {
        let mut numbers = take.split('-');
        (0..3).all(|_| numbers.next().is_some_and(is_digits)) && numbers.next().is_none()
    } else {
        false
    }
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_decimal(text: &str) -> bool {
    let mut parts = text.split('.');
    parts.next().is_some_and(is_digits)
        && parts.next().is_none_or(is_digits)
        && parts.next().is_none()
}

/// The part of a file name after its source and lane: date, time, length, then optionally a
/// tempo and a collision number, as `dated_take_at` and `numbered_take` write them.
fn is_dated_take(take: &str) -> bool {
    let dashed = |text: Option<&str>, widths: [usize; 3]| {
        text.is_some_and(|text| {
            let mut parts = text.split('-');
            widths.iter().all(|&width| {
                parts
                    .next()
                    .is_some_and(|part| part.len() == width && is_digits(part))
            }) && parts.next().is_none()
        })
    };
    let mut fields = take.split('_');
    if !dashed(fields.next(), [4, 2, 2]) || !dashed(fields.next(), [2, 2, 2]) {
        return false;
    }
    if !fields
        .next()
        .and_then(|length| length.strip_suffix('s'))
        .is_some_and(is_decimal)
    {
        return false;
    }
    let mut next = fields.next();
    if next
        .and_then(|tempo| tempo.strip_suffix("bpm"))
        .is_some_and(is_decimal)
    {
        next = fields.next();
    }
    if next.is_some_and(is_digits) {
        next = fields.next();
    }
    next.is_none()
}

/// The first export keeps the plain take name. Later ones that would collide get `_2`, `_3`, …
fn numbered_take(take: &str, attempt: u32) -> String {
    if attempt <= 1 {
        take.to_owned()
    } else {
        format!("{take}_{attempt}")
    }
}

/// Exports one or two lanes from the same retained time range.
/// Both files in a pair share a take name, after their lane letters.
/// The lanes can be owned [`Snapshot`]s or [`FrozenTrack`]s that share a capture's pages.
pub fn export_tracks_with_profile<T: TrackAudio>(
    output_dir: &Path,
    snapshots: &[T],
    frames: Range<usize>,
    profile: WavExportProfile,
) -> io::Result<Vec<PathBuf>> {
    export_routed_tracks_with_profile(
        output_dir,
        snapshots,
        frames,
        profile,
        &[ChannelRoute::Merged; 2][..snapshots.len().min(2)],
        false,
    )
}

/// How a selected range of native channels is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelExportMode {
    /// One mono WAV for each channel, named with its index and caller-supplied label.
    MonoFiles,
    /// One WAV with the selected channels in their original order.
    /// More than two channels use WAVE_FORMAT_EXTENSIBLE.
    Multichannel,
}

/// Exports a contiguous range of channels from a nonempty range of retained frames.
/// `frames` is relative to the first frame held by `track`; `channels` is zero-based.
/// `labels` is either empty (numbered labels) or contains one label per native channel.
/// File names retain spaces in labels and replace characters unsafe on Windows.
/// All files share a take name and are removed if any file cannot be completed.
/// Each file uses the profile's rate, bit depth and bounded resampling threads.
/// Names begin as `label` makes them, and a mono file adds `-ChNN_<label>` after the take, so
/// the channels of one take sort together in channel order. `fades` ramp every channel alike.
#[allow(clippy::too_many_arguments)]
pub fn export_channel_range_with_profile<T: TrackAudio + ?Sized>(
    output_dir: &Path,
    track: &T,
    frames: Range<usize>,
    channels: Range<usize>,
    labels: &[String],
    profile: WavExportProfile,
    mode: ChannelExportMode,
    fades: Fades,
    label: &TakeLabel,
) -> io::Result<Vec<PathBuf>> {
    check_track_clip(track, &frames)?;
    if channels.start >= channels.end
        || channels.end > track.channels()
        || (!labels.is_empty() && labels.len() != track.channels())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a channel range and labels matching the lane",
        ));
    }
    let selections: Vec<_> = match mode {
        ChannelExportMode::MonoFiles => channels
            .clone()
            .map(|channel| channel..channel + 1)
            .collect(),
        ChannelExportMode::Multichannel => vec![channels],
    };
    let tracks: Vec<_> = selections
        .iter()
        .map(|channels| ChannelSelection {
            track,
            channels: channels.clone(),
        })
        .collect();
    fs::create_dir_all(output_dir)?;
    let take = dated_take(
        clip_seconds(frames.end - frames.start, track.sample_rate()),
        None,
    );
    let mut attempt = 0;
    loop {
        attempt += 1;
        let prefix = label.file_stem(&take, attempt);
        let paths: Vec<_> = selections
            .iter()
            .map(|channels| {
                let suffix = if mode == ChannelExportMode::MonoFiles {
                    let channel = channels.start;
                    let label = channel_file_label(
                        labels.get(channel).map(String::as_str).unwrap_or(""),
                        channel,
                    );
                    format!("-Ch{:02}_{label}", channel + 1)
                } else {
                    String::new()
                };
                output_dir.join(format!("{prefix}{suffix}.wav"))
            })
            .collect();
        let mut files = Vec::with_capacity(paths.len());
        let mut failure = None;
        for (path, selected) in paths.iter().zip(&tracks) {
            match with_track_clip(selected, &frames, fades, |clip| {
                WavFile::create(path, clip, profile, WavChannelMap::Unchanged)
            }) {
                Ok(file) => files.push(file),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = failure {
            // Try every removal even when an earlier file cannot be removed.
            files
                .into_iter()
                .map(WavFile::discard)
                .fold(Ok(()), |first, next| first.and(next))?;
            if error.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(error);
        }
        // Every name is claimed first, and only one channel file is rendered at a time.
        // Its resampler uses the same bounded worker pool as any other WAV export.
        let mut files = files.into_iter();
        for (index, selected) in tracks.iter().enumerate() {
            let file = files.next().unwrap();
            if let Err(error) = with_track_clip(selected, &frames, fades, |clip| {
                file.write(clip, resampling_threads())
            }) {
                files
                    .map(WavFile::discard)
                    .chain(paths[..index].iter().map(fs::remove_file))
                    .fold(Ok(()), |first, next| first.and(next))?;
                return Err(error);
            }
        }
        return Ok(paths);
    }
}

fn channel_file_label(label: &str, channel: usize) -> String {
    let label: String = label
        .chars()
        .take(48)
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let label = label.trim_matches([' ', '.']);
    if label.is_empty() {
        format!("Channel {}", channel + 1)
    } else {
        label.to_owned()
    }
}

/// A channel selection reads one bounded block of its source lane at a time.
struct ChannelSelection<'a, T: TrackAudio + ?Sized> {
    track: &'a T,
    channels: Range<usize>,
}

impl<T: TrackAudio + ?Sized> TrackAudio for ChannelSelection<'_, T> {
    fn sample_rate(&self) -> u32 {
        self.track.sample_rate()
    }
    fn channels(&self) -> usize {
        self.channels.len()
    }
    fn frame_range(&self) -> Range<u64> {
        self.track.frame_range()
    }
    fn read_frames(&self, first: usize, output: &mut [f32]) -> bool {
        let width = self.channels();
        if output.len() % width != 0 {
            return false;
        }
        if self.channels == (0..self.track.channels()) {
            return self.track.read_frames(first, output);
        }
        let mut source = vec![0.0; output.len() / width * self.track.channels()];
        if !self.track.read_frames(first, &mut source) {
            return false;
        }
        for (frame, selected) in source
            .chunks_exact(self.track.channels())
            .zip(output.chunks_exact_mut(width))
        {
            selected.copy_from_slice(&frame[self.channels.clone()]);
        }
        true
    }
}

/// Exports each selected lane to its own WAV using its capture route and output setting.
pub fn export_routed_tracks_with_profile<T: TrackAudio>(
    output_dir: &Path,
    snapshots: &[T],
    frames: Range<usize>,
    profile: WavExportProfile,
    routes: &[ChannelRoute],
    force_mono: bool,
) -> io::Result<Vec<PathBuf>> {
    export_routed_tracks_with_tempo(
        output_dir, snapshots, frames, profile, routes, force_mono, None,
    )
}

/// Like [`export_routed_tracks_with_profile`], for a caller that knows the tempo the audio was
/// played at. The tempo becomes part of the file name, for example `…_12.4s_120bpm.wav`.
pub fn export_routed_tracks_with_tempo<T: TrackAudio>(
    output_dir: &Path,
    snapshots: &[T],
    frames: Range<usize>,
    profile: WavExportProfile,
    routes: &[ChannelRoute],
    force_mono: bool,
    tempo_bpm: Option<f64>,
) -> io::Result<Vec<PathBuf>> {
    export_routed_tracks_with_fades(
        output_dir, snapshots, frames, profile, routes, force_mono, tempo_bpm, Fades::NONE,
    )
}

/// Like [`export_routed_tracks_with_tempo`], with `fades` at the ends of the exported frames.
/// The ramps are counted in source frames, so both lanes of a pair fade at the same moments.
#[allow(clippy::too_many_arguments)]
pub fn export_routed_tracks_with_fades<T: TrackAudio>(
    output_dir: &Path,
    snapshots: &[T],
    frames: Range<usize>,
    profile: WavExportProfile,
    routes: &[ChannelRoute],
    force_mono: bool,
    tempo_bpm: Option<f64>,
    fades: Fades,
) -> io::Result<Vec<PathBuf>> {
    let labels = TakeLabel::unnamed_lanes(snapshots.len());
    export_labelled_tracks(
        output_dir, snapshots, frames, profile, routes, force_mono, tempo_bpm, fades, &labels,
    )
}

/// Like [`export_routed_tracks_with_fades`], naming each file after the label of its lane:
/// `labels` has one entry per track. The two files of a pair need different labels.
#[allow(clippy::too_many_arguments)]
pub fn export_labelled_tracks<T: TrackAudio>(
    output_dir: &Path,
    snapshots: &[T],
    frames: Range<usize>,
    profile: WavExportProfile,
    routes: &[ChannelRoute],
    force_mono: bool,
    tempo_bpm: Option<f64>,
    fades: Fades,
    labels: &[TakeLabel],
) -> io::Result<Vec<PathBuf>> {
    match snapshots {
        [one] => export_routed_track_refs(
            output_dir, &[one], frames, profile, routes, force_mono, tempo_bpm, fades, labels,
        ),
        [a, b] => export_routed_track_refs(
            output_dir, &[a, b], frames, profile, routes, force_mono, tempo_bpm, fades, labels,
        ),
        _ => Err(invalid_tracks()),
    }
}

impl TakeLabel {
    /// Labels for `count` tracks that carry no source names: a single file has no lane, the
    /// two files of a pair are lanes A and B.
    pub(crate) fn unnamed_lanes(count: usize) -> Vec<Self> {
        match count {
            2 => Lane::BOTH.map(|lane| Self::unnamed(Some(lane))).to_vec(),
            count => vec![Self::unnamed(None); count],
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn export_routed_track_refs<T: TrackAudio>(
    output_dir: &Path,
    snapshots: &[&T],
    frames: Range<usize>,
    profile: WavExportProfile,
    routes: &[ChannelRoute],
    force_mono: bool,
    tempo_bpm: Option<f64>,
    fades: Fades,
    labels: &[TakeLabel],
) -> io::Result<Vec<PathBuf>> {
    match (snapshots, routes, labels) {
        ([snapshot], [route], [label]) => {
            check_track_clip(*snapshot, &frames)?;
            with_track_clip(*snapshot, &frames, fades, |clip| {
                export_clip_named(output_dir, clip, profile, *route, force_mono, tempo_bpm, label)
            })
            .map(|path| vec![path])
        }
        ([a, b], [route_a, route_b], [label_a, label_b])
            if a.sample_rate() == b.sample_rate()
                && a.frame_range() == b.frame_range()
                // Two files of one take under one name would keep colliding with each other.
                && label_a.file_stem("", 1) != label_b.file_stem("", 1) =>
        {
            check_track_clip(*a, &frames)?;
            check_track_clip(*b, &frames)?;
            let a_map = WavChannelMap::from_route(*route_a, a.channels(), force_mono)?;
            let b_map = WavChannelMap::from_route(*route_b, b.channels(), force_mono)?;
            fs::create_dir_all(output_dir)?;
            let take = dated_take(
                clip_seconds(frames.end - frames.start, a.sample_rate()),
                tempo_bpm,
            );
            // The lanes are written at the same time, so each resamples on half the threads.
            let threads = (resampling_threads() / 2).max(1);
            let mut attempt = 0;
            loop {
                // Both lanes of a take share one name, so a clash on either file moves the pair on.
                attempt += 1;
                let a_path = output_dir.join(format!("{}.wav", label_a.file_stem(&take, attempt)));
                let b_path = output_dir.join(format!("{}.wav", label_b.file_stem(&take, attempt)));
                // Both names are claimed before any audio is written.
                let a_file = match with_track_clip(*a, &frames, fades, |clip| {
                    WavFile::create(&a_path, clip, profile, a_map)
                }) {
                    Ok(file) => file,
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(err) => return Err(err),
                };
                let b_file = match with_track_clip(*b, &frames, fades, |clip| {
                    WavFile::create(&b_path, clip, profile, b_map)
                }) {
                    Ok(file) => file,
                    Err(err) => {
                        a_file.discard()?;
                        if err.kind() == io::ErrorKind::AlreadyExists {
                            continue;
                        }
                        return Err(err);
                    }
                };
                let (a_written, b_written) = thread::scope(|scope| {
                    let lane_frames = &frames;
                    let b_writer = scope.spawn(move || {
                        with_track_clip(*b, lane_frames, fades, |clip| b_file.write(clip, threads))
                    });
                    let a_written =
                        with_track_clip(*a, lane_frames, fades, |clip| a_file.write(clip, threads));
                    (a_written, b_writer.join())
                });
                let b_written = b_written.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                // A lane that could not be written removed its own file. Its partner goes
                // too, so half of a pair is never left behind.
                return match (a_written, b_written) {
                    (Ok(()), Ok(())) => Ok(vec![a_path, b_path]),
                    (Err(err), b_written) => {
                        if b_written.is_ok() {
                            fs::remove_file(&b_path)?;
                        }
                        Err(err)
                    }
                    (Ok(()), Err(err)) => {
                        fs::remove_file(&a_path)?;
                        Err(err)
                    }
                };
            }
        }
        _ => Err(invalid_tracks()),
    }
}

/// A clip must be a nonempty run of frames the lane holds.
fn check_track_clip<T: TrackAudio + ?Sized>(track: &T, frames: &Range<usize>) -> io::Result<()> {
    let available = track.frame_range();
    let held = usize::try_from(available.end.saturating_sub(available.start)).unwrap_or(usize::MAX);
    if !(1..=64).contains(&track.channels()) || frames.start >= frames.end || frames.end > held {
        return Err(invalid_tracks());
    }
    Ok(())
}

fn invalid_tracks() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "expected aligned nonempty audio lanes",
    )
}

/// Writes everything up to the first sample: the RIFF, format and data chunk headers.
fn write_header(
    output: &mut impl Write,
    profile: WavExportProfile,
    channels: u16,
    frames: u32,
    data_bytes: u32,
) -> io::Result<()> {
    let format = profile.sample_format;
    let bits = format.bits_per_sample();
    let sample_bytes = bits / 8;
    let frame_bytes = channels * sample_bytes;
    let padding = data_bytes % 2;
    let extensible = channels > 2 || format == WavSampleFormat::Pcm24;
    let format_tag = if extensible {
        0xfffe
    } else {
        match format {
            WavSampleFormat::Pcm16 => 1u16,   // WAVE_FORMAT_PCM
            WavSampleFormat::Pcm24 => 0xfffe, // WAVE_FORMAT_EXTENSIBLE, PCM subformat
            WavSampleFormat::Float32 => 3,    // WAVE_FORMAT_IEEE_FLOAT
        }
    };
    output.write_all(b"RIFF")?;
    output.write_all(&(data_bytes + padding + format.riff_overhead(channels)).to_le_bytes())?;
    output.write_all(b"WAVEfmt ")?;
    output.write_all(&format.format_chunk_size(channels).to_le_bytes())?;
    output.write_all(&format_tag.to_le_bytes())?;
    output.write_all(&channels.to_le_bytes())?;
    output.write_all(&profile.sample_rate.to_le_bytes())?;
    output.write_all(&(profile.sample_rate * u32::from(frame_bytes)).to_le_bytes())?;
    output.write_all(&frame_bytes.to_le_bytes())?;
    output.write_all(&bits.to_le_bytes())?;
    if extensible {
        let mut subformat: [u8; 16] = [
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38,
            0x9b, 0x71,
        ];
        if format == WavSampleFormat::Float32 {
            subformat[0] = 3;
        }
        // Native channels can include objects, so their labels do not imply a speaker map.
        let channel_mask = match channels {
            1 => 0x4u32,
            2 => 0x3u32,
            _ => 0,
        };
        output.write_all(&22u16.to_le_bytes())?; // Extension size
        output.write_all(&bits.to_le_bytes())?; // Valid bits
        output.write_all(&channel_mask.to_le_bytes())?;
        output.write_all(&subformat)?;
    }
    if format == WavSampleFormat::Float32 {
        if !extensible {
            output.write_all(&0u16.to_le_bytes())?; // Extension size
        }
        output.write_all(b"fact")?;
        output.write_all(&4u32.to_le_bytes())?;
        output.write_all(&frames.to_le_bytes())?;
    }
    output.write_all(b"data")?;
    output.write_all(&data_bytes.to_le_bytes())
}

/// Writes a complete WAV, reading the clip and, when the rates differ, resampling it
/// a bounded piece at a time on up to `threads` threads. The bytes are those of
/// [`write_wave`] over the whole clip, whatever the number of threads.
#[allow(clippy::too_many_arguments)]
fn write_wave_from_clip(
    output: &mut impl Write,
    profile: WavExportProfile,
    channels: u16,
    frames: u32,
    data_bytes: u32,
    clip: &Clip,
    channel_map: WavChannelMap,
    threads: usize,
) -> io::Result<()> {
    write_header(output, profile, channels, frames, data_bytes)?;
    let mut encoder = SampleEncoder::new(
        output,
        profile.sample_format,
        channel_map == WavChannelMap::Side,
    );
    let mut write = |samples: &[f32]| {
        if channel_map == WavChannelMap::Unchanged {
            samples.iter().try_for_each(|&sample| encoder.push(sample))
        } else {
            let source_channels = if channel_map == WavChannelMap::Downmix {
                2
            } else {
                1
            };
            samples.chunks_exact(source_channels).try_for_each(|frame| {
                channel_map
                    .map_frame(frame)
                    .into_iter()
                    .take(usize::from(channels))
                    .try_for_each(|sample| encoder.push(sample))
            })
        }
    };
    if clip.sample_rate == profile.sample_rate || clip.frames == 0 {
        let mut source = Vec::new();
        let mut first = 0;
        while first < clip.frames {
            let count = (clip.frames - first).min(CHUNK_FRAMES);
            source.resize(count * clip.channels, 0.0);
            clip.read(first, &mut source)?;
            write(&source)?;
            first += count;
        }
    } else {
        resample_clip(
            clip,
            profile.sample_rate,
            frames as usize,
            threads,
            &mut write,
        )?;
    }
    encoder.finish()?;
    if data_bytes % 2 != 0 {
        output.write_all(&[0])?;
    }
    Ok(())
}

/// Resamples `clip` to `frames` frames at `target_rate` and hands them to `write` in
/// order, a piece of [`CHUNK_FRAMES`] at a time. With more than one thread, worker
/// threads render the pieces while this one encodes and writes them.
fn resample_clip(
    clip: &Clip,
    target_rate: u32,
    frames: usize,
    threads: usize,
    write: &mut dyn FnMut(&[f32]) -> io::Result<()>,
) -> io::Result<()> {
    let width = clip.channels;
    let step = f64::from(clip.sample_rate) / f64::from(target_rate);
    let kernel = ResamplingKernel::new(step);
    let last_frame = (clip.frames - 1) as isize;
    let window_frame = |frame: usize, reach: isize| {
        (((frame as f64 * step).floor() as isize) + reach).clamp(0, last_frame) as usize
    };
    // Each piece of output needs only the source frames its filter taps reach, so it
    // is resampled from a short window instead of the whole clip.
    let render = |piece: usize, window: &mut Vec<f32>, output: &mut Vec<f32>| -> io::Result<()> {
        let first = piece * CHUNK_FRAMES;
        let end = (first + CHUNK_FRAMES).min(frames);
        let window_first = window_frame(first, -kernel.radius);
        let window_last = window_frame(end - 1, kernel.radius);
        window.resize((window_last - window_first + 1) * width, 0.0);
        clip.read(window_first, window)?;
        output.clear();
        for frame in first..end {
            let frame = kernel.at_window(
                window,
                width,
                frame as f64 * step,
                window_first as isize,
                last_frame,
            );
            output.extend_from_slice(&frame[..width]);
        }
        Ok(())
    };
    let pieces = frames.div_ceil(CHUNK_FRAMES);
    let workers = threads.min(pieces).max(1);
    if workers == 1 {
        let mut window = Vec::new();
        let mut output = Vec::with_capacity(CHUNK_FRAMES.min(frames) * width);
        for piece in 0..pieces {
            render(piece, &mut window, &mut output)?;
            write(&output)?;
        }
        return Ok(());
    }
    thread::scope(|scope| -> io::Result<()> {
        let render = &render;
        // Worker `w` renders pieces w, w + workers, w + 2 × workers and so on, each
        // handed over through the worker's own channel. The pieces arrive in order, and
        // no worker gets more than two pieces ahead of the writer.
        let finished: Vec<_> = (0..workers)
            .map(|worker| {
                let (sender, receiver) = mpsc::sync_channel(1);
                scope.spawn(move || {
                    let mut window = Vec::new();
                    for piece in (worker..pieces).step_by(workers) {
                        let mut output = Vec::with_capacity(CHUNK_FRAMES * width);
                        let rendered = render(piece, &mut window, &mut output).map(|()| output);
                        let failed = rendered.is_err();
                        // The writer stops listening when it fails, and the worker stops with it.
                        if sender.send(rendered).is_err() || failed {
                            break;
                        }
                    }
                });
                receiver
            })
            .collect();
        for piece in 0..pieces {
            let samples = finished[piece % workers]
                .recv()
                .map_err(|_| io::Error::other("a resampling thread stopped"))??;
            write(&samples)?;
        }
        Ok(())
    })
}

/// Turns samples into WAV bytes, handing them on a block at a time.
struct SampleEncoder<'a, W: Write> {
    output: &'a mut W,
    format: WavSampleFormat,
    symmetric_pcm: bool,
    bytes: [u8; 4096],
    used: usize,
}

impl<'a, W: Write> SampleEncoder<'a, W> {
    fn new(output: &'a mut W, format: WavSampleFormat, symmetric_pcm: bool) -> Self {
        Self {
            output,
            format,
            symmetric_pcm,
            bytes: [0; 4096],
            used: 0,
        }
    }

    fn push(&mut self, sample: f32) -> io::Result<()> {
        let sample_bytes = usize::from(self.format.bits_per_sample() / 8);
        let encoded = match self.format {
            WavSampleFormat::Pcm16 => quantize(sample, 1 << 15, self.symmetric_pcm).to_le_bytes(),
            WavSampleFormat::Pcm24 => quantize(sample, 1 << 23, self.symmetric_pcm).to_le_bytes(),
            WavSampleFormat::Float32 => sample.to_le_bytes(),
        };
        self.bytes[self.used..self.used + sample_bytes].copy_from_slice(&encoded[..sample_bytes]);
        self.used += sample_bytes;
        if self.used + sample_bytes > self.bytes.len() {
            self.output.write_all(&self.bytes[..self.used])?;
            self.used = 0;
        }
        Ok(())
    }

    fn finish(self) -> io::Result<()> {
        self.output.write_all(&self.bytes[..self.used])
    }
}

/// The writer this crate used before it streamed: every sample of the clip, already
/// resampled, in one slice. Tests hold the streaming writer to its exact bytes.
#[cfg(test)]
fn write_wave(
    output: &mut impl Write,
    profile: WavExportProfile,
    channels: u16,
    frames: u32,
    data_bytes: u32,
    samples: &[f32],
    channel_map: WavChannelMap,
) -> io::Result<()> {
    let format = profile.sample_format;
    let padding = data_bytes % 2;
    write_header(output, profile, channels, frames, data_bytes)?;
    if channel_map == WavChannelMap::Unchanged {
        write_samples(output, format, samples.iter().copied(), false)?;
    } else {
        let source_channels = if channel_map == WavChannelMap::Downmix {
            2
        } else {
            1
        };
        let routed = samples.chunks_exact(source_channels).flat_map(|frame| {
            channel_map
                .map_frame(frame)
                .into_iter()
                .take(usize::from(channels))
        });
        write_samples(output, format, routed, channel_map == WavChannelMap::Side)?;
    }
    if padding != 0 {
        output.write_all(&[0])?;
    }
    Ok(())
}

#[cfg(test)]
fn write_samples(
    output: &mut impl Write,
    format: WavSampleFormat,
    samples: impl Iterator<Item = f32>,
    symmetric_pcm: bool,
) -> io::Result<()> {
    let sample_bytes = usize::from(format.bits_per_sample() / 8);
    let mut bytes = [0; 4096];
    let capacity = bytes.len() / sample_bytes * sample_bytes;
    let mut used = 0;
    for sample in samples {
        let encoded = match format {
            WavSampleFormat::Pcm16 => quantize(sample, 1 << 15, symmetric_pcm).to_le_bytes(),
            WavSampleFormat::Pcm24 => quantize(sample, 1 << 23, symmetric_pcm).to_le_bytes(),
            WavSampleFormat::Float32 => sample.to_le_bytes(),
        };
        bytes[used..used + sample_bytes].copy_from_slice(&encoded[..sample_bytes]);
        used += sample_bytes;
        if used == capacity {
            output.write_all(&bytes[..used])?;
            used = 0;
        }
    }
    output.write_all(&bytes[..used])
}

fn quantize(sample: f32, full_scale: i32, symmetric: bool) -> i32 {
    // Rust's float-to-int cast maps NaN to 0 and saturates infinities.
    let scaled = (f64::from(sample) * f64::from(full_scale)).round() as i32;
    // A clipped Side frame still needs an exactly inverted right sample.
    scaled.clamp(-full_scale + i32::from(symmetric), full_scale - 1)
}

/// Resamples a whole clip held in one slice. The exporter resamples in pieces now;
/// tests hold those pieces to this function's exact samples.
#[cfg(test)]
fn resample(
    samples: &[f32],
    channels: usize,
    source_rate: u32,
    target_rate: u32,
    frames: usize,
) -> Vec<f32> {
    let step = f64::from(source_rate) / f64::from(target_rate);
    let kernel = ResamplingKernel::new(step);
    let mut output = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        let frame = kernel.at(samples, channels, frame as f64 * step);
        output.extend_from_slice(&frame[..channels]);
    }
    output
}

pub(crate) struct ResamplingKernel {
    radius: isize,
    phases: Vec<Vec<f64>>,
}

impl ResamplingKernel {
    pub(crate) fn new(step: f64) -> Self {
        let cutoff = if step > 1.0 { 0.95 / step } else { 1.0 };
        let radius = (24.0 / cutoff).ceil().min(192.0) as isize;
        Self {
            radius,
            phases: phase_kernels(cutoff, radius),
        }
    }

    pub(crate) fn radius(&self) -> usize {
        self.radius as usize
    }

    pub(crate) fn at(&self, samples: &[f32], channels: usize, position: f64) -> [f32; 64] {
        let last_frame = (samples.len() / channels - 1) as isize;
        self.at_window(samples, channels, position, 0, last_frame)
    }

    /// Like [`Self::at`] for a clip of which `window` holds only the frames from
    /// `window_first` on. `last_frame` is the last frame of the whole clip, and
    /// `window` must hold every frame the taps around `position` reach.
    fn at_window(
        &self,
        window: &[f32],
        channels: usize,
        position: f64,
        window_first: isize,
        last_frame: isize,
    ) -> [f32; 64] {
        let first = position.floor() as isize - self.radius;
        let weights = &self.phases[(position.fract() * PHASES as f64).round() as usize];
        let last = first + weights.len() as isize - 1;
        if first < 0 || last > last_frame {
            return self.at_edge(window, channels, position, window_first, last_frame);
        }
        // Every tap lies inside the clip, so the taps are one run of the window and none
        // needs clamping. Each sum adds the same products in the same order as
        // `at_edge`, so the result is identical to the bit.
        let start = (first - window_first) as usize * channels;
        let taps = &window[start..start + weights.len() * channels];
        let mut output = [0.0; 64];
        if channels == 1 {
            let mut sum = 0.0f64;
            for (&sample, &weight) in taps.iter().zip(weights) {
                sum += f64::from(sample) * weight;
            }
            output[0] = sum as f32;
        } else if channels == 2 {
            let (mut left, mut right) = (0.0f64, 0.0f64);
            for (frame, &weight) in taps.chunks_exact(2).zip(weights) {
                left += f64::from(frame[0]) * weight;
                right += f64::from(frame[1]) * weight;
            }
            output[0] = left as f32;
            output[1] = right as f32;
        } else {
            let mut value = [0.0f64; 64];
            for (frame, &weight) in taps.chunks_exact(channels).zip(weights) {
                for (sum, &sample) in value[..channels].iter_mut().zip(frame) {
                    *sum += f64::from(sample) * weight;
                }
            }
            for (output, sum) in output.iter_mut().zip(value) {
                *output = sum as f32;
            }
        }
        output
    }

    /// [`Self::at_window`] for any position: taps that reach past either end of the
    /// clip repeat its first or last frame.
    fn at_edge(
        &self,
        window: &[f32],
        channels: usize,
        position: f64,
        window_first: isize,
        last_frame: isize,
    ) -> [f32; 64] {
        let first = position.floor() as isize - self.radius;
        let phase = (position.fract() * PHASES as f64).round() as usize;
        let mut value = [0.0f64; 64];
        for (tap, &weight) in self.phases[phase].iter().enumerate() {
            let source_frame =
                ((first + tap as isize).clamp(0, last_frame) - window_first) as usize;
            let source = &window[source_frame * channels..][..channels];
            for (sum, &sample) in value[..channels].iter_mut().zip(source) {
                *sum += f64::from(sample) * weight;
            }
        }
        value.map(|sample| sample as f32)
    }
}

// Precompute Blackman-windowed sinc weights by fractional phase so processing
// a clip only requires multiplies and sums. Normalize for unity gain at DC.
fn phase_kernels(cutoff: f64, radius: isize) -> Vec<Vec<f64>> {
    (0..=PHASES)
        .map(|phase| {
            let phase = phase as f64 / PHASES as f64;
            let mut weights: Vec<_> = (-radius..=radius)
                .map(|offset| {
                    let offset = offset as f64 - phase;
                    let window_phase = PI * offset / radius as f64;
                    if window_phase.abs() >= PI {
                        return 0.0;
                    }
                    let window =
                        0.42 + 0.5 * window_phase.cos() + 0.08 * (2.0 * window_phase).cos();
                    let phase = PI * cutoff * offset;
                    let sinc = if phase == 0.0 {
                        1.0
                    } else {
                        phase.sin() / phase
                    };
                    sinc * window
                })
                .collect();
            let sum: f64 = weights.iter().sum();
            for weight in &mut weights {
                *weight /= sum;
            }
            weights
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_EXPORT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn narrow_export_reference() {
        let dir = test_dir("narrow-reference");
        // Hashes of the complete files written by the pre-spatial export path.
        for (width, expected) in [(1, 0x48b50d87718600bcu64), (2, 0x3215bea1a0ff641bu64)] {
            let samples = noise(951, 811 * width);
            let track = FrozenTrack::from_interleaved(48000, width, 10, &samples).unwrap();
            let paths = export_labelled_tracks(
                &dir,
                &[track],
                7..799,
                WavExportProfile::default(),
                &[ChannelRoute::Merged],
                false,
                None,
                Fades::new(3, 5),
                &[TakeLabel::default()],
            )
            .unwrap();
            let wav = fs::read(&paths[0]).unwrap();
            let hash = wav.iter().fold(0xcbf29ce484222325u64, |hash, &byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
            });
            assert_eq!(hash, expected);
            if let Some(reference) = std::env::var_os("W3B_REFERENCE") {
                assert_eq!(
                    wav,
                    fs::read(PathBuf::from(reference).join(format!("reference-{width}.bin")))
                        .unwrap()
                );
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    fn test_dir(label: &str) -> PathBuf {
        let serial = NEXT_EXPORT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "backtracking-{label}-{}-{serial}",
            std::process::id()
        ))
    }

    fn file_name(path: &Path) -> &str {
        path.file_name().unwrap().to_str().unwrap()
    }

    /// `Backtracking_YYYY-MM-DD_HH-MM-SS_<seconds>s`, the part every export of a take shares.
    fn assert_take_shape(name: &str, seconds: &str) {
        let fields: Vec<_> = name.split('_').collect();
        assert!(fields.len() >= 4, "{name}");
        assert_eq!(fields[0], "Backtracking", "{name}");
        for (field, lengths) in [(fields[1], [4, 2, 2]), (fields[2], [2, 2, 2])] {
            let parts: Vec<_> = field.split('-').collect();
            assert_eq!(parts.len(), 3, "{name}");
            for (part, length) in parts.iter().zip(lengths) {
                assert_eq!(part.len(), length, "{name}");
                assert!(part.bytes().all(|byte| byte.is_ascii_digit()), "{name}");
            }
        }
        assert_eq!(fields[3], seconds, "{name}");
        // Nothing Windows rejects in a file name, and no spaces for scripts to trip over.
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')),
            "{name}"
        );
    }

    #[test]
    fn export_file_names_are_recognised_and_other_files_are_not() {
        let time = TakeTime {
            year: 2026,
            month: 11,
            day: 14,
            hour: 15,
            minute: 32,
            second: 7,
        };
        // Every name the exporter can write: with and without a source and a lane, tempo and a
        // collision number. A source's name can hold anything a file name can, and the
        // nameless source stands for the plug-in's files, whose pairs end in their lane.
        let sources = ["", "Spotify", "chrome", "Mic_2", "Headset (USB) 2", "系統聲音", "A", "2026-11-14"];
        for tempo in [None, Some(120.0), Some(98.46)] {
            let take = dated_take_at(time, 12.44, tempo);
            for attempt in [1, 2, 37] {
                for source in sources {
                    for lane in [None, Some(Lane::A), Some(Lane::B)] {
                        let name = format!("{}.wav", TakeLabel::new(source, lane).file_stem(&take, attempt));
                        assert!(is_export_file_name(&name), "{name}");
                    }
                }
            }
        }
        // The names used before takes were dated.
        assert!(is_export_file_name("Backtracking-1763100000000-4242-7.wav"));
        assert!(is_export_file_name(
            "Backtracking-1763100000000-4242-7-B.wav"
        ));

        for other in [
            "Backtracking_Spotify.wav",
            "Backtracking_Spotify_A.wav",
            "Backtracking__2026-11-14_15-32-07_12.4s.wav",
            "Backtracking_Spotify_A_2026-11-14_15-32-07_12.4s_final.wav",
            "Backtracking_Spotify_2026-11-14_15-32-07_12.4s.mp3",
            "Spotify_A_2026-11-14_15-32-07_12.4s.wav",
            "Backtracking.wav",
            "Backtracking_notes.wav",
            "Backtracking_2026-11-14_15-32-07_12.4s.mp3",
            "Backtracking_2026-11-14_15-32-07_12.4s",
            "Backtracking_2026-11-14_15-32-07_12.4s-C.wav",
            "Backtracking_2026-11-14_15-32-07_12.4s_final.wav",
            "Backtracking_2026-11-14_15-32-07_12.4s_120bpm_2_3.wav",
            "Backtracking_2026-11-14_15-32_12.4s.wav",
            "Backtracking_2026-11-14_15-32-07_long.wav",
            "Backtracking-1763100000000-4242.wav",
            "Backtracking-1763100000000-4242-7-8.wav",
            "Backtracking-take-1-2.wav",
            "My Backtracking_2026-11-14_15-32-07_12.4s.wav",
            "Kick_2026-11-14_15-32-07_12.4s.wav",
            "song.wav",
            "",
        ] {
            assert!(!is_export_file_name(other), "{other}");
        }
    }

    #[test]
    fn take_names_show_local_date_time_length_and_tempo() {
        let time = TakeTime {
            year: 2026,
            month: 11,
            day: 14,
            hour: 15,
            minute: 32,
            second: 7,
        };
        let unnamed = TakeLabel::default();
        assert_eq!(
            unnamed.file_stem(&dated_take_at(time, 12.44, None), 1),
            "Backtracking_2026-11-14_15-32-07_12.4s"
        );
        assert_eq!(
            unnamed.file_stem(&dated_take_at(time, 12.44, Some(120.0)), 1),
            "Backtracking_2026-11-14_15-32-07_12.4s_120bpm"
        );
        assert_eq!(
            dated_take_at(time, 0.5, Some(98.46)),
            "2026-11-14_15-32-07_0.5s_98.5bpm"
        );
        // A host that reports no usable tempo leaves the name as it is without one.
        for tempo in [0.0, -120.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                dated_take_at(time, f64::NAN, Some(tempo)),
                "2026-11-14_15-32-07_0.0s"
            );
        }
        let take = dated_take_at(time, 12.44, Some(120.5));
        let name = unnamed.file_stem(&take, 1);
        assert_take_shape(&name, "12.4s");
        assert_eq!(unnamed.file_stem(&take, 2), format!("{name}_2"));
        // Names sort in the order they were taken, whatever the clip length.
        let later = TakeTime {
            minute: 33,
            second: 0,
            ..time
        };
        assert!(dated_take_at(time, 99.0, None) < dated_take_at(later, 1.0, None));
    }

    #[test]
    fn take_names_lead_with_the_source_and_its_lane() {
        let time = TakeTime { year: 2026, month: 11, day: 17, hour: 9, minute: 5, second: 0 };
        let take = dated_take_at(time, 12.44, None);
        let stem = |source: &str, lane| TakeLabel::new(source, lane).file_stem(&take, 1);
        assert_eq!(stem("Spotify", Some(Lane::A)), "Backtracking_Spotify_A_2026-11-17_09-05-00_12.4s");
        assert_eq!(stem("Vocal mic", Some(Lane::B)), "Backtracking_Vocal mic_B_2026-11-17_09-05-00_12.4s");
        // One source shows no lane.
        assert_eq!(stem("chrome", None), "Backtracking_chrome_2026-11-17_09-05-00_12.4s");
        assert_eq!(stem("  \t ", None), "Backtracking_2026-11-17_09-05-00_12.4s");
        // A lane without a source name, as the plug-in writes its pairs, ends in its letter.
        assert_eq!(stem("", Some(Lane::B)), "Backtracking_2026-11-17_09-05-00_12.4s-B");
        assert_eq!(stem("/:", Some(Lane::A)), "Backtracking_2026-11-17_09-05-00_12.4s-A");
        // The collision number stays after the length, and ahead of a trailing lane.
        assert_eq!(
            TakeLabel::new("Spotify", Some(Lane::A)).file_stem(&take, 3),
            "Backtracking_Spotify_A_2026-11-17_09-05-00_12.4s_3"
        );
        assert_eq!(
            TakeLabel::new("", Some(Lane::A)).file_stem(&take, 2),
            "Backtracking_2026-11-17_09-05-00_12.4s_2-A"
        );
        // Settings show the part before the date, cleaned the way the file will be.
        assert_eq!(TakeLabel::new("AC/DC ", Some(Lane::B)).file_prefix(), "Backtracking_AC DC_B_");
        assert_eq!(TakeLabel::new("", None).file_prefix(), "Backtracking_");
        assert_eq!(TakeLabel::new("", Some(Lane::B)).file_prefix(), "Backtracking_");
    }

    #[test]
    fn source_names_keep_what_a_file_name_can_hold() {
        assert_eq!(file_name_source("Spotify").as_deref(), Some("Spotify"));
        assert_eq!(file_name_source("系統聲音").as_deref(), Some("系統聲音"));
        assert_eq!(
            file_name_source("Microphone (Realtek(R) Audio)").as_deref(),
            Some("Microphone (Realtek(R) Audio)")
        );
        // What Windows or macOS refuses reads as a space, and spaces close up.
        assert_eq!(file_name_source("AC/DC: Live?").as_deref(), Some("AC DC Live"));
        assert_eq!(file_name_source(" a\\b*c\"d<e>f|g ").as_deref(), Some("a b c d e f g"));
        assert_eq!(file_name_source("tab\there\nnew").as_deref(), Some("tab here new"));
        assert_eq!(file_name_source("under_score.v2").as_deref(), Some("under_score.v2"));
        for nothing in ["", "   ", "/:*?", "\u{7}"] {
            assert_eq!(file_name_source(nothing), None, "{nothing:?}");
        }
        // Long names stop at the limit, counted in characters, and never end in a space.
        let long = "名".repeat(SOURCE_NAME_LIMIT + 10);
        assert_eq!(file_name_source(&long).unwrap().chars().count(), SOURCE_NAME_LIMIT);
        let spaced = format!("{} tail", "x".repeat(SOURCE_NAME_LIMIT - 1));
        let cut = file_name_source(&spaced).unwrap();
        assert_eq!(cut, "x".repeat(SOURCE_NAME_LIMIT - 1));
    }

    #[test]
    fn utc_fallback_converts_epoch_seconds_to_calendar_fields() {
        let fields = |seconds| {
            let time = TakeTime::from_utc_seconds(seconds);
            (
                time.year,
                time.month,
                time.day,
                time.hour,
                time.minute,
                time.second,
            )
        };
        assert_eq!(fields(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(fields(951_782_399), (2000, 2, 28, 23, 59, 59));
        assert_eq!(fields(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(fields(1_700_000_000), (2023, 11, 14, 22, 13, 20));
        assert_eq!(fields(4_102_444_799), (2099, 12, 31, 23, 59, 59));
    }

    #[test]
    fn take_time_is_the_local_wall_clock() {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let local = local_time(seconds).expect("this platform reports local time");
        let utc = TakeTime::from_utc_seconds(seconds);
        assert!((1..=12).contains(&local.month) && (1..=31).contains(&local.day));
        assert!(local.hour < 24 && local.minute < 60 && local.second < 61);
        // Time zones sit within a day of UTC, on quarter-hour offsets.
        let date = |time: TakeTime| (time.year, time.month, time.day);
        let minutes = |time: TakeTime| i64::from(time.hour) * 60 + i64::from(time.minute);
        let day_shift = match date(local).cmp(&date(utc)) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        };
        let offset = minutes(local) + day_shift * 1_440 - minutes(utc);
        assert!(
            (-12 * 60..=14 * 60).contains(&offset),
            "offset {offset} min"
        );
        // The two clocks are read a moment apart, so the minute may have ticked over.
        assert!(
            matches!(offset.rem_euclid(15), 0 | 1 | 14),
            "offset {offset} min"
        );
    }

    #[test]
    fn exports_in_the_same_second_get_numbered_names_and_keep_existing_files() {
        let dir = test_dir("names");
        let samples = [0.25; 4_800];
        let take_name = |seconds, tempo| format!("Backtracking_{}", dated_take(seconds, tempo));
        loop {
            // The name an export started right now will ask for first.
            let take = take_name(0.1, None);
            assert_take_shape(&take, "0.1s");
            if dir.join(format!("{take}.wav")).exists() {
                continue;
            }
            // Something the user already has under that name must survive.
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("{take}_2.wav")), b"keep").unwrap();
            let paths: Vec<_> = (0..3)
                .map(|_| export_wav(&dir, 48_000, 1, &samples).unwrap())
                .collect();
            if take_name(0.1, None) != take {
                // The clock moved on to the next second part-way through; try again.
                continue;
            }
            let names: Vec<_> = paths.iter().map(|path| file_name(path)).collect();
            assert_eq!(
                names,
                [
                    format!("{take}.wav"),
                    format!("{take}_3.wav"),
                    format!("{take}_4.wav")
                ]
            );
            assert_eq!(
                fs::read(dir.join(format!("{take}_2.wav"))).unwrap(),
                b"keep"
            );
            break;
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn paired_exports_share_one_free_name_and_carry_the_tempo() {
        let profile = WavExportProfile::default();
        let a = Snapshot {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.5; 24_000],
            first_frame: 0,
        };
        let export = |dir: &Path, lanes: usize, tempo| {
            export_routed_tracks_with_tempo(
                dir,
                &vec![a.clone(); lanes],
                0..24_000,
                profile,
                &[ChannelRoute::Merged; 2][..lanes],
                false,
                Some(tempo),
            )
            .unwrap()
        };
        // Unnamed lanes, as the plug-in exports them, keep the letter at the end of the name.
        let take_name = |seconds, tempo| format!("Backtracking_{}", dated_take(seconds, tempo));
        // Exercise a discarded attempt even when both exports finish in the same second.
        let mut force_retry = true;
        loop {
            let dir = test_dir("pair-names");
            let take = take_name(0.5, Some(174.0));
            assert!(take.ends_with("_0.5s_174bpm"), "{take}");
            assert_take_shape(&take, "0.5s");
            // Only lane B of the first name is taken: the pair must not be split across names.
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("{take}-B.wav")), b"keep").unwrap();
            let pair = export(&dir, 2, 174.0);
            let single = export(&dir, 1, 92.5);
            if std::mem::take(&mut force_retry) || take_name(0.5, Some(174.0)) != take {
                fs::remove_dir_all(&dir).unwrap();
                continue;
            }
            assert_eq!(file_name(&pair[0]), format!("{take}_2-A.wav"));
            assert_eq!(file_name(&pair[1]), format!("{take}_2-B.wav"));
            assert!(!dir.join(format!("{take}-A.wav")).exists());
            assert_eq!(
                fs::read(dir.join(format!("{take}-B.wav"))).unwrap(),
                b"keep"
            );
            let single = file_name(&single[0]);
            assert!(single.ends_with("_0.5s_92.5bpm.wav"), "{single}");
            fs::remove_dir_all(dir).unwrap();
            break;
        }
    }

    #[test]
    fn labelled_exports_name_each_file_after_its_source_and_lane() {
        let dir = test_dir("labelled");
        let profile = WavExportProfile::default();
        let lane = Snapshot {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.25; 4_800],
            first_frame: 0,
        };
        let export = |tracks: &[Snapshot], labels: &[TakeLabel]| {
            export_labelled_tracks(
                &dir,
                tracks,
                0..4_800,
                profile,
                &[ChannelRoute::Merged; 2][..tracks.len()],
                false,
                None,
                Fades::NONE,
                labels,
            )
        };
        let pair = export(
            &[lane.clone(), lane.clone()],
            &[TakeLabel::new("Spotify", Some(Lane::A)), TakeLabel::new("Vocal/mic", Some(Lane::B))],
        )
        .unwrap();
        let [a, b] = pair.as_slice() else {
            panic!("expected A/B files")
        };
        let (a, b) = (file_name(a), file_name(b));
        assert!(a.starts_with("Backtracking_Spotify_A_") && a.ends_with("_0.1s.wav"), "{a}");
        assert!(b.starts_with("Backtracking_Vocal mic_B_") && b.ends_with("_0.1s.wav"), "{b}");
        assert_eq!(
            a.strip_prefix("Backtracking_Spotify_A_"),
            b.strip_prefix("Backtracking_Vocal mic_B_")
        );
        // One lane of a two-source recording keeps its letter.
        let one = export(&[lane.clone()], &[TakeLabel::new("Vocal mic", Some(Lane::B))]).unwrap();
        let one = file_name(&one[0]);
        assert!(one.starts_with("Backtracking_Vocal mic_B_"), "{one}");
        for name in [&a, &b, &one] {
            assert!(is_export_file_name(name), "{name}");
        }
        // The two files of a pair must not ask for one name, and each track needs a label.
        let same = [TakeLabel::new("System", Some(Lane::A)), TakeLabel::new("System_A", None)];
        assert!(export(&[lane.clone(), lane.clone()], &same).is_err());
        assert!(export(&[lane.clone(), lane.clone()], &[TakeLabel::default()]).is_err());
        assert!(export(&[lane], &[]).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn grouped_exports_keep_exact_l_r_samples_and_remove_incomplete_pair() {
        let dir = test_dir("grouped");
        let profile = WavExportProfile::default();
        let a = Snapshot {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.125, 0.25, 0.5, 0.75],
            first_frame: 42,
        };
        let b = Snapshot {
            samples: vec![-0.125, -0.25, -0.5, -0.75],
            ..a.clone()
        };
        let paths = export_tracks_with_profile(&dir, &[a.clone(), b], 1..3, profile).unwrap();
        let name_a = paths[0].file_name().unwrap().to_str().unwrap();
        let name_b = paths[1].file_name().unwrap().to_str().unwrap();
        assert_eq!(name_a.strip_suffix("-A.wav"), name_b.strip_suffix("-B.wav"));
        for (path, expected) in paths.iter().zip([[0.25, 0.5], [-0.25, -0.5]]) {
            let bytes = fs::read(path).unwrap();
            assert_eq!(&bytes[46..50], &2u32.to_le_bytes());
            let pcm: Vec<_> = bytes[58..]
                .chunks_exact(4)
                .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
                .collect();
            assert_eq!(pcm, expected);
        }

        let invalid = Snapshot {
            channels: 65,
            samples: vec![0.5; 260],
            ..a.clone()
        };
        assert!(export_tracks_with_profile(&dir, &[a, invalid], 1..3, profile).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn routed_wavs_keep_speaker_placement_and_can_export_each_file_as_mono() {
        let dir = test_dir("routed");
        let profile = WavExportProfile::default();
        let mono = [0.25, -0.5];
        let stereo = [0.25, 0.75, -0.5, 0.25];
        for (route, input_channels, input, output_channels, expected) in [
            (
                ChannelRoute::Left,
                1,
                &mono[..],
                2,
                &[0.25, 0.0, -0.5, 0.0][..],
            ),
            (
                ChannelRoute::Right,
                1,
                &mono[..],
                2,
                &[0.0, 0.25, 0.0, -0.5][..],
            ),
            (ChannelRoute::Mid, 1, &mono[..], 1, &mono[..]),
            (ChannelRoute::Center, 1, &mono[..], 1, &mono[..]),
            (
                ChannelRoute::Side,
                1,
                &mono[..],
                2,
                &[0.25, -0.25, -0.5, 0.5][..],
            ),
            (ChannelRoute::Merged, 1, &mono[..], 1, &mono[..]),
            (ChannelRoute::Merged, 2, &stereo[..], 2, &stereo[..]),
            // All channels chosen on a wider device that now gives one or two channels.
            (ChannelRoute::AllChannels, 1, &mono[..], 1, &mono[..]),
            (ChannelRoute::AllChannels, 2, &stereo[..], 2, &stereo[..]),
        ] {
            let path = export_routed_wav_with_profile(
                &dir,
                48_000,
                input_channels,
                input,
                profile,
                route,
                false,
            )
            .unwrap();
            let bytes = fs::read(path).unwrap();
            assert_eq!(
                u16::from_le_bytes(bytes[22..24].try_into().unwrap()),
                output_channels
            );
            assert_eq!(u32::from_le_bytes(bytes[46..50].try_into().unwrap()), 2);
            assert_eq!(float_samples(&bytes), expected);

            let mono_path = export_routed_wav_with_profile(
                &dir,
                48_000,
                input_channels,
                input,
                profile,
                route,
                true,
            )
            .unwrap();
            let bytes = fs::read(mono_path).unwrap();
            assert_eq!(&bytes[22..24], &1u16.to_le_bytes());
            assert_eq!(
                float_samples(&bytes),
                if input_channels == 1 {
                    mono
                } else {
                    [0.5, -0.125]
                }
            );
        }

        let a = Snapshot {
            sample_rate: 44_100,
            channels: 1,
            samples: vec![0.0; 442],
            first_frame: 42,
        };
        let mut b = a.clone();
        b.samples[1..].fill(-0.25);
        let mut a = a;
        a.samples[1..].fill(0.5);
        let routes = [ChannelRoute::Side, ChannelRoute::Mid];
        for force_mono in [false, true] {
            let paths = export_routed_tracks_with_profile(
                &dir,
                &[a.clone(), b.clone()],
                1..442,
                profile,
                &routes,
                force_mono,
            )
            .unwrap();
            let [path_a, path_b] = paths.as_slice() else {
                panic!("expected A/B files")
            };
            let a_name = path_a.file_name().unwrap().to_str().unwrap();
            let b_name = path_b.file_name().unwrap().to_str().unwrap();
            assert_eq!(a_name.strip_suffix("-A.wav"), b_name.strip_suffix("-B.wav"));
            let a_wave = fs::read(path_a).unwrap();
            let b_wave = fs::read(path_b).unwrap();
            for bytes in [&a_wave, &b_wave] {
                assert_eq!(&bytes[46..50], &480u32.to_le_bytes());
            }
            assert_eq!(
                float_samples(&a_wave),
                if force_mono {
                    vec![0.5; 480]
                } else {
                    [0.5, -0.5].repeat(480)
                }
            );
            assert_eq!(float_samples(&b_wave), vec![-0.25; 480]);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    fn float_samples(wav: &[u8]) -> Vec<f32> {
        wav[58..]
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn fades_ramp_both_files_of_a_pair_at_the_same_frames_and_survive_resampling() {
        let dir = test_dir("fades");
        let lane = |level: f32| FrozenTrack::from_interleaved(48_000, 1, 500, &[level; 400]).unwrap();
        let tracks = [lane(1.0), lane(-0.5)];
        let fades = Fades::new(20, 30);
        let same_rate = WavExportProfile { sample_rate: 48_000, sample_format: WavSampleFormat::Float32 };
        let paths = export_routed_tracks_with_fades(
            &dir, &tracks, 10..110, same_rate, &[ChannelRoute::Merged; 2], false, None, fades,
        )
        .unwrap();
        let [a, b] = [&paths[0], &paths[1]].map(|path| float_samples(&fs::read(path).unwrap()));
        let gains: Vec<f32> = (0..100).map(|index| fades.gain(index, 100)).collect();
        assert_eq!(a, gains);
        assert_eq!(b, gains.iter().map(|gain| gain * -0.5).collect::<Vec<_>>());
        assert_eq!((a[0], a[19], a[20], a[69], a[70], a[99]), (0.0, 0.95, 1.0, 1.0, 29.0 / 30.0, 0.0));

        // Through the resampler the ramps keep their length in time.
        let doubled = WavExportProfile { sample_rate: 96_000, sample_format: WavSampleFormat::Float32 };
        let path = export_routed_tracks_with_fades(
            &dir, &tracks[..1], 10..110, doubled, &[ChannelRoute::Merged], false, None, fades,
        )
        .unwrap();
        let samples = float_samples(&fs::read(&path[0]).unwrap());
        assert_eq!(samples.len(), 200);
        assert!(samples[0].abs() < 0.05, "{}", samples[0]);
        assert!(samples[..36].windows(2).all(|pair| pair[1] > pair[0]));
        assert!((samples[100] - 1.0).abs() < 0.01, "{}", samples[100]);
        assert!(samples[199].abs() < 0.05, "{}", samples[199]);

        // Without fades the export is unchanged.
        let plain = export_routed_tracks_with_fades(
            &dir, &tracks[..1], 10..110, same_rate, &[ChannelRoute::Merged], false, None,
            Fades::NONE,
        )
        .unwrap();
        assert_eq!(float_samples(&fs::read(&plain[0]).unwrap()), vec![1.0; 100]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clipped_pcm_side_channels_remain_exact_inverses() {
        let dir = test_dir("side");
        for (format, bytes_per_sample, data_offset, peak) in [
            (WavSampleFormat::Pcm16, 2, 44, 32_767),
            (WavSampleFormat::Pcm24, 3, 68, 8_388_607),
        ] {
            let path = export_routed_wav_with_profile(
                &dir,
                48_000,
                1,
                &[1.5, -1.5, 0.5],
                WavExportProfile {
                    sample_rate: 48_000,
                    sample_format: format,
                },
                ChannelRoute::Side,
                false,
            )
            .unwrap();
            let bytes = fs::read(path).unwrap();
            assert_eq!(&bytes[22..24], &2u16.to_le_bytes());
            let samples: Vec<_> = bytes[data_offset..]
                .chunks_exact(bytes_per_sample)
                .map(|bytes| {
                    let mut sample = [if bytes[bytes.len() - 1] & 0x80 == 0 {
                        0
                    } else {
                        0xff
                    }; 4];
                    sample[..bytes.len()].copy_from_slice(bytes);
                    i32::from_le_bytes(sample)
                })
                .collect();
            assert_eq!(&samples[..4], &[peak, -peak, -peak, peak]);
            assert!(samples.chunks_exact(2).all(|frame| frame[0] == -frame[1]));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wave_header_and_samples_are_ieee_float_stereo() {
        let samples = [1.25, -0.125, 0.5, 0.75];
        let mut wave = Vec::new();
        write_wave(
            &mut wave,
            WavExportProfile::default(),
            2,
            2,
            16,
            &samples,
            WavChannelMap::Unchanged,
        )
        .unwrap();

        assert_eq!(&wave[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(wave[4..8].try_into().unwrap()) as usize,
            wave.len() - 8
        );
        assert_eq!(&wave[8..16], b"WAVEfmt ");
        assert_eq!(&wave[20..24], &[3, 0, 2, 0]);
        assert_eq!(&wave[24..28], &48_000u32.to_le_bytes());
        assert_eq!(&wave[38..42], b"fact");
        assert_eq!(&wave[46..50], &2u32.to_le_bytes());
        assert_eq!(&wave[50..54], b"data");
        assert_eq!(&wave[54..58], &16u32.to_le_bytes());
        let decoded: Vec<_> = wave[58..]
            .chunks_exact(4)
            .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
            .collect();
        assert_eq!(decoded, samples);
    }

    #[test]
    fn pcm16_saturates_and_uses_a_standard_header() {
        let samples = [-1.5, -0.5, 0.0, 0.5, 1.5];
        let profile = WavExportProfile {
            sample_rate: 44_100,
            sample_format: WavSampleFormat::Pcm16,
        };
        let mut wave = Vec::new();
        write_wave(
            &mut wave,
            profile,
            1,
            5,
            10,
            &samples,
            WavChannelMap::Unchanged,
        )
        .unwrap();
        assert_eq!(&wave[16..20], &16u32.to_le_bytes());
        assert_eq!(&wave[20..24], &[1, 0, 1, 0]);
        assert_eq!(&wave[24..28], &44_100u32.to_le_bytes());
        assert_eq!(&wave[28..32], &88_200u32.to_le_bytes());
        assert_eq!(&wave[32..36], &[2, 0, 16, 0]);
        assert_eq!(&wave[36..40], b"data");
        let payload: Vec<_> = wave[44..]
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes(bytes.try_into().unwrap()))
            .collect();
        assert_eq!(payload, [i16::MIN, -16_384, 0, 16_384, i16::MAX]);
    }

    #[test]
    fn pcm24_is_packed_and_pads_an_odd_sized_mono_payload() {
        let samples = [-1.5, -0.5, 0.0, 0.5, 1.5];
        let profile = WavExportProfile {
            sample_rate: 96_000,
            sample_format: WavSampleFormat::Pcm24,
        };
        let mut wave = Vec::new();
        write_wave(
            &mut wave,
            profile,
            1,
            5,
            15,
            &samples,
            WavChannelMap::Unchanged,
        )
        .unwrap();
        assert_eq!(&wave[4..8], &76u32.to_le_bytes());
        assert_eq!(&wave[16..20], &40u32.to_le_bytes());
        assert_eq!(&wave[20..24], &[0xfe, 0xff, 1, 0]);
        assert_eq!(&wave[24..28], &96_000u32.to_le_bytes());
        assert_eq!(&wave[32..40], &[3, 0, 24, 0, 22, 0, 24, 0]);
        assert_eq!(&wave[40..48], &[4, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&wave[60..68], b"data\x0f\0\0\0");
        assert_eq!(
            &wave[68..],
            &[0, 0, 0x80, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0x40, 0xff, 0xff, 0x7f, 0]
        );
        assert_eq!(wave.len(), 84);
    }

    #[test]
    fn resampling_keeps_channel_order_and_level() {
        let input: Vec<_> = [0.5, -0.25].repeat(441);
        for (rate, frames) in [(48_000, 480), (96_000, 960)] {
            let output = resample(&input, 2, 44_100, rate, frames);
            assert_eq!(output.len(), frames * 2);
            for frame in output.chunks_exact(2) {
                assert!((frame[0] - 0.5).abs() < 1e-6);
                assert!((frame[1] + 0.25).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn downsampling_rejects_content_above_the_output_nyquist_frequency() {
        let input: Vec<_> = (0..960)
            .map(|frame| (2.0 * PI * 34_000.0 * f64::from(frame) / 96_000.0).sin() as f32)
            .collect();
        for (rate, frames) in [(44_100, 441), (48_000, 480)] {
            let output = resample(&input, 1, 96_000, rate, frames);
            assert!(output[48..frames - 48]
                .iter()
                .all(|sample| sample.abs() < 0.001));
        }
    }

    #[test]
    fn exports_use_unique_paths_and_do_not_overwrite_files() {
        let dir = test_dir("wav");
        let first = export_wav(&dir, 44_100, 1, &[0.25; 441]).unwrap();
        let second = export_wav(&dir, 48_000, 1, &[1.25]).unwrap();
        assert_ne!(first, second);
        let bytes = fs::read(&first).unwrap();
        assert_eq!(&bytes[46..50], &480u32.to_le_bytes());
        assert_eq!(bytes.len(), 58 + 480 * 4);
        let err = write_wav(&first, 48_000, 1, &[0.0]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(first).unwrap(), bytes);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn profiles_write_the_selected_rate_bit_depth_and_duration() {
        let dir = test_dir("profiles");
        let samples = [1.5, -1.5].repeat(480);
        for rate in WavExportProfile::SAMPLE_RATES {
            for (format, header_bytes) in [
                (WavSampleFormat::Pcm16, 44),
                (WavSampleFormat::Pcm24, 68),
                (WavSampleFormat::Float32, 58),
            ] {
                let path = export_wav_with_profile(
                    &dir,
                    48_000,
                    2,
                    &samples,
                    WavExportProfile {
                        sample_rate: rate,
                        sample_format: format,
                    },
                )
                .unwrap();
                let bytes = fs::read(path).unwrap();
                let bits = format.bits_per_sample();
                assert_eq!(&bytes[24..28], &rate.to_le_bytes());
                assert_eq!(&bytes[34..36], &bits.to_le_bytes());
                assert_eq!(
                    bytes.len(),
                    header_bytes + rate as usize / 100 * usize::from(bits / 8) * 2
                );
                if format == WavSampleFormat::Float32 {
                    assert_eq!(
                        &bytes[header_bytes..header_bytes + 8],
                        &[0, 0, 0xc0, 0x3f, 0, 0, 0xc0, 0xbf]
                    );
                }
            }
        }
        let invalid = dir.join("invalid.wav");
        assert_eq!(
            write_wav_with_profile(
                &invalid,
                48_000,
                2,
                &samples,
                WavExportProfile {
                    sample_rate: 0,
                    sample_format: WavSampleFormat::Pcm24,
                }
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!invalid.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    /// The bytes the exporter wrote before it streamed: the whole clip resampled into
    /// one buffer, then encoded by the old writer.
    fn reference_wav(
        source_rate: u32,
        channels: usize,
        interleaved: &[f32],
        profile: WavExportProfile,
        channel_map: WavChannelMap,
    ) -> Vec<u8> {
        let source_frames = interleaved.len() / channels;
        let rate = u128::from(source_rate);
        let rounded = (source_frames as u128 * u128::from(profile.sample_rate) + rate / 2) / rate;
        let frames = rounded.max((source_frames != 0) as u128) as usize;
        let samples = if source_rate == profile.sample_rate {
            interleaved.to_vec()
        } else {
            resample(
                interleaved,
                channels,
                source_rate,
                profile.sample_rate,
                frames,
            )
        };
        let output_channels = channel_map.output_channels(channels) as u16;
        let frame_bytes = output_channels * (profile.sample_format.bits_per_sample() / 8);
        let mut bytes = Vec::new();
        write_wave(
            &mut bytes,
            profile,
            output_channels,
            frames as u32,
            (frames * usize::from(frame_bytes)) as u32,
            &samples,
            channel_map,
        )
        .unwrap();
        bytes
    }

    /// Deterministic samples: mostly in range, some clipping, a few that are not numbers.
    fn noise(seed: u64, samples: usize) -> Vec<f32> {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        (0..samples)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                match state % 997 {
                    0 => f32::NAN,
                    1 => f32::INFINITY,
                    2 => -0.0,
                    _ => ((state >> 40) as f32 / (1u64 << 23) as f32 - 1.0) * 1.25,
                }
            })
            .collect()
    }

    const FORMATS: [WavSampleFormat; 3] = [
        WavSampleFormat::Pcm16,
        WavSampleFormat::Pcm24,
        WavSampleFormat::Float32,
    ];

    /// Every channel map with the channel count it reads.
    const MAPS: [(WavChannelMap, usize); 6] = [
        (WavChannelMap::Unchanged, 1),
        (WavChannelMap::Unchanged, 2),
        (WavChannelMap::Left, 1),
        (WavChannelMap::Right, 1),
        (WavChannelMap::Side, 1),
        (WavChannelMap::Downmix, 2),
    ];

    #[test]
    fn streamed_wavs_have_the_bytes_of_the_whole_clip_writer() {
        let dir = test_dir("streamed");
        fs::create_dir_all(&dir).unwrap();
        let mut written = 0;
        let mut check = |source_rate: u32,
                         frames: usize,
                         profile: WavExportProfile,
                         (channel_map, channels): (WavChannelMap, usize)| {
            let samples = noise(written + 1, frames * channels);
            let path = dir.join(format!("{written}.wav"));
            written += 1;
            let expected = reference_wav(source_rate, channels, &samples, profile, channel_map);
            // One thread, and thread counts that share a clip's pieces out unevenly.
            for threads in [1, 2, 3, 8] {
                with_slice_clip(source_rate, channels, &samples, |clip| {
                    WavFile::create(&path, clip, profile, channel_map)?.write(clip, threads)
                })
                .unwrap();
                assert!(
                    fs::read(&path).unwrap() == expected,
                    "{source_rate} Hz, {frames} frames of {channels} channels to {profile:?} \
                     on {threads} threads"
                );
                fs::remove_file(&path).unwrap();
            }
        };
        // Clip lengths on both sides of the piece the writer handles at a time, at the
        // same rate and resampled up and down, with the widest filter the exporter uses.
        let rates = [
            (48_000, 48_000),
            (44_100, 44_100),
            (44_100, 48_000),
            (48_000, 44_100),
            (96_000, 44_100),
            (44_100, 96_000),
            (8_000, 48_000),
            (192_000, 44_100),
            (400_000, 44_100),
        ];
        let lengths = [
            1,
            2,
            3,
            CHUNK_FRAMES - 1,
            CHUNK_FRAMES,
            CHUNK_FRAMES + 1,
            2 * CHUNK_FRAMES + 809,
        ];
        let mut case = 0;
        for (source_rate, sample_rate) in rates {
            for frames in lengths {
                let profile = WavExportProfile {
                    sample_rate,
                    sample_format: FORMATS[case % 3],
                };
                check(source_rate, frames, profile, MAPS[case % 6]);
                check(source_rate, frames, profile, MAPS[(case / 3 + 1) % 6]);
                case += 1;
            }
        }
        // Every sample format with every channel map, at the same rate and resampled.
        for source_rate in [48_000, 44_100] {
            for sample_format in FORMATS {
                for map in MAPS {
                    let profile = WavExportProfile {
                        sample_rate: 48_000,
                        sample_format,
                    };
                    check(source_rate, 5_003, profile, map);
                }
            }
        }
        // Output lengths that end exactly on, and one frame past, a piece of the writer.
        for frames in [CHUNK_FRAMES / 2, CHUNK_FRAMES / 2 + 1, CHUNK_FRAMES] {
            let profile = WavExportProfile {
                sample_rate: 96_000,
                sample_format: WavSampleFormat::Pcm24,
            };
            check(48_000, frames, profile, MAPS[1]);
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unclamped_interior_taps_sum_to_the_same_bits_as_clamped_ones() {
        // Up and down, by small and large ratios, up to the widest filter the exporter uses.
        for (source_rate, target_rate) in [
            (48_000, 96_000),
            (44_100, 96_000),
            (44_100, 48_000),
            (48_000, 44_100),
            (96_000, 44_100),
            (400_000, 44_100),
        ] {
            let step = f64::from(source_rate) / f64::from(target_rate);
            let kernel = ResamplingKernel::new(step);
            for channels in [1, 2] {
                let frames = 3 * kernel.radius() + 700;
                let samples = noise(channels as u64, frames * channels);
                let last_frame = frames as isize - 1;
                let mut interior = 0;
                for frame in 0..(frames as f64 / step) as usize {
                    let position = frame as f64 * step;
                    let first = position.floor() as isize - kernel.radius;
                    interior += usize::from(first >= 0 && first + 2 * kernel.radius <= last_frame);
                    assert_eq!(
                        kernel
                            .at_window(&samples, channels, position, 0, last_frame)
                            .map(f32::to_bits),
                        kernel
                            .at_edge(&samples, channels, position, 0, last_frame)
                            .map(f32::to_bits),
                        "{source_rate} Hz to {target_rate} Hz, {channels} channels, frame {frame}"
                    );
                }
                assert!(interior > 0, "{source_rate} Hz to {target_rate} Hz");
            }
        }
    }

    /// A lane that claims frames it cannot read, like a source that fails during an export.
    struct Unreadable {
        lane: Snapshot,
        readable: usize,
    }

    impl TrackAudio for Unreadable {
        fn sample_rate(&self) -> u32 {
            self.lane.sample_rate
        }

        fn channels(&self) -> usize {
            self.lane.channels
        }

        fn frame_range(&self) -> Range<u64> {
            self.lane.frame_range()
        }

        fn read_frames(&self, first: usize, output: &mut [f32]) -> bool {
            first + output.len() / self.lane.channels <= self.readable
                && TrackAudio::read_frames(&self.lane, first, output)
        }
    }

    #[test]
    fn an_export_that_fails_part_way_leaves_no_file_behind() {
        let dir = test_dir("unreadable");
        fs::create_dir_all(&dir).unwrap();
        let lane = |readable| Unreadable {
            lane: Snapshot {
                sample_rate: 44_100,
                channels: 2,
                samples: noise(3, 30_000 * 2),
                first_frame: 0,
            },
            readable,
        };
        let profile = |sample_rate| WavExportProfile {
            sample_rate,
            sample_format: WavSampleFormat::Float32,
        };
        // Resampled pieces are rendered on worker threads, so there the failure starts on one.
        for (rate, threads) in [
            (44_100, 1),
            (48_000, 1),
            (48_000, 2),
            (48_000, 3),
            (96_000, 8),
        ] {
            let path = dir.join("lane.wav");
            let error = with_track_clip(&lane(20_000), &(0..30_000), Fades::NONE, |clip| {
                WavFile::create(&path, clip, profile(rate), WavChannelMap::Unchanged)?
                    .write(clip, threads)
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(!path.exists(), "{rate} Hz on {threads} threads");
        }
        // Whichever lane of a pair fails, its partner is removed with it.
        for readable in [[30_000, 20_000], [20_000, 30_000]] {
            for rate in [44_100, 96_000] {
                let lanes = readable.map(lane);
                let routes = [ChannelRoute::Merged; 2];
                assert!(export_routed_tracks_with_profile(
                    &dir,
                    &lanes,
                    0..30_000,
                    profile(rate),
                    &routes,
                    false
                )
                .is_err());
                assert_eq!(
                    fs::read_dir(&dir).unwrap().count(),
                    0,
                    "{readable:?} at {rate} Hz"
                );
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// Takes `room` bytes, then fails like a full disk.
    struct FullDisk {
        room: usize,
    }

    impl Write for FullDisk {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.room == 0 {
                return Err(io::Error::other("disk full"));
            }
            let taken = bytes.len().min(self.room);
            self.room -= taken;
            Ok(taken)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_write_stops_every_resampling_thread() {
        let samples = noise(5, 100_000 * 2);
        let profile = WavExportProfile::default();
        let frames = ((100_000u64 * 48_000 + 22_050) / 44_100) as u32;
        for threads in [1, 2, 3, 8] {
            // Returning at all shows that no worker was left waiting to hand over a piece.
            let error = with_slice_clip(44_100, 2, &samples, |clip| {
                write_wave_from_clip(
                    &mut FullDisk { room: 50_000 },
                    profile,
                    2,
                    frames,
                    frames * 8,
                    clip,
                    WavChannelMap::Unchanged,
                    threads,
                )
            })
            .unwrap_err();
            assert_eq!(error.to_string(), "disk full", "{threads} threads");
        }
    }

    fn wav_chunk<'a>(wav: &'a [u8], name: &[u8; 4]) -> &'a [u8] {
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize + 8,
            wav.len()
        );
        assert_eq!(&wav[8..12], b"WAVE");
        let mut offset = 12;
        while offset < wav.len() {
            let size = u32::from_le_bytes(wav[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if &wav[offset..offset + 4] == name {
                return &wav[offset + 8..offset + 8 + size];
            }
            offset += 8 + size + size % 2;
        }
        panic!("missing WAV chunk {name:?}");
    }

    fn decoded_wav_samples(wav: &[u8]) -> Vec<f32> {
        let format = wav_chunk(wav, b"fmt ");
        let tag = u16::from_le_bytes(format[..2].try_into().unwrap());
        let tag = if tag == 0xfffe {
            u16::from_le_bytes(format[24..26].try_into().unwrap())
        } else {
            tag
        };
        let bits = u16::from_le_bytes(format[14..16].try_into().unwrap());
        wav_chunk(wav, b"data")
            .chunks_exact(usize::from(bits / 8))
            .map(|sample| match (tag, bits) {
                (3, 32) => f32::from_le_bytes(sample.try_into().unwrap()),
                (1, 16) => f32::from(i16::from_le_bytes(sample.try_into().unwrap())) / 32768.0,
                (1, 24) => {
                    ((i32::from_le_bytes([sample[0], sample[1], sample[2], 0]) << 8) >> 8) as f32
                        / 8_388_608.0
                }
                _ => panic!("unexpected WAV format {tag}/{bits}"),
            })
            .collect()
    }

    #[test]
    fn twelve_channel_capture_exports_native_channels_and_labelled_mono_groups() {
        use crate::{Capture, DurationChoice};
        let dir = test_dir("channel-groups");
        let mut capture = Capture::new_tracks(48_000, 12, None, DurationChoice::Seconds10).unwrap();
        let samples: Vec<f32> = (0..5_003)
            .flat_map(|frame| {
                (0..12)
                    .map(move |channel| (channel + 1) as f32 / 32.0 + (frame % 17) as f32 / 1024.0)
            })
            .collect();
        capture.push_interleaved(&samples).unwrap();
        let frozen = capture.freeze().track(0).unwrap();
        assert_eq!(frozen.snapshot().samples, samples);
        let mut labels: Vec<String> = (1..=12)
            .map(|channel| format!("Object {channel}"))
            .collect();
        labels[0] = "Left Front".into();
        labels[1] = "Right/Front:?*".into();
        labels[2] = "...".into();
        labels[3] = "CON".into();
        labels[4] = "Object 1".into();
        let frames = 7..5_001;
        for channels in [0..12, 4..9] {
            let expected: Vec<f32> = samples[frames.start * 12..frames.end * 12]
                .chunks_exact(12)
                .flat_map(|frame| frame[channels.clone()].iter().copied())
                .collect();
            let multi = export_channel_range_with_profile(
                &dir,
                &frozen,
                frames.clone(),
                channels.clone(),
                &labels,
                WavExportProfile::default(),
                ChannelExportMode::Multichannel,
                Fades::NONE,
                &TakeLabel::default(),
            )
            .unwrap();
            assert_eq!(multi.len(), 1);
            let wav = fs::read(&multi[0]).unwrap();
            let format = wav_chunk(&wav, b"fmt ");
            assert_eq!(format.len(), 40);
            assert_eq!(&format[..2], &0xfffeu16.to_le_bytes());
            assert_eq!(&format[2..4], &(channels.len() as u16).to_le_bytes());
            assert_eq!(&format[20..24], &[0; 4]);
            assert_eq!(decoded_wav_samples(&wav), expected);
            let mono = export_channel_range_with_profile(
                &dir,
                &frozen,
                frames.clone(),
                channels.clone(),
                &labels,
                WavExportProfile::default(),
                ChannelExportMode::MonoFiles,
                Fades::NONE,
                &TakeLabel::default(),
            )
            .unwrap();
            assert_eq!(mono.len(), channels.len());
            for (path, channel) in mono.iter().zip(channels.clone()) {
                assert!(is_export_file_name(file_name(path)), "{}", file_name(path));
                assert!(file_name(path).contains(&format!("-Ch{:02}_", channel + 1)));
                let wav = fs::read(path).unwrap();
                assert_eq!(&wav_chunk(&wav, b"fmt ")[2..4], &1u16.to_le_bytes());
                assert_eq!(
                    decoded_wav_samples(&wav),
                    samples[frames.start * 12..frames.end * 12]
                        .chunks_exact(12)
                        .map(|frame| frame[channel])
                        .collect::<Vec<_>>()
                );
            }
            if channels.start == 0 {
                assert!(file_name(&mono[0]).ends_with("-Ch01_Left Front.wav"));
                assert!(file_name(&mono[1]).ends_with("-Ch02_Right_Front___.wav"));
                assert!(file_name(&mono[2]).ends_with("-Ch03_Channel 3.wav"));
            }
            let again = export_channel_range_with_profile(
                &dir,
                &frozen,
                frames.clone(),
                channels,
                &labels,
                WavExportProfile::default(),
                ChannelExportMode::MonoFiles,
                Fades::NONE,
                &TakeLabel::default(),
            )
            .unwrap();
            assert!(again.iter().all(|path| !mono.contains(path)));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_wavs_resample_all_channels_with_each_sample_format() {
        let dir = test_dir("native-profiles");
        fs::create_dir_all(&dir).unwrap();
        for channels in [12, 64] {
            let frames = 2_501;
            let samples: Vec<f32> = (0..frames)
                .flat_map(|frame| {
                    (0..channels).map(move |channel| {
                        (frame as f32 * (channel + 1) as f32 / 100.0).sin() * (channel + 1) as f32
                            / 128.0
                    })
                })
                .collect();
            for profile in [
                WavExportProfile {
                    sample_rate: 44_100,
                    sample_format: WavSampleFormat::Pcm16,
                },
                WavExportProfile {
                    sample_rate: 48_000,
                    sample_format: WavSampleFormat::Pcm24,
                },
                WavExportProfile {
                    sample_rate: 96_000,
                    sample_format: WavSampleFormat::Float32,
                },
            ] {
                let path = dir.join(format!("{channels}-{}.wav", profile.sample_rate));
                write_wav_with_profile(&path, 44_100, channels, &samples, profile).unwrap();
                let wav = fs::read(path).unwrap();
                let format = wav_chunk(&wav, b"fmt ");
                assert_eq!(&format[..2], &0xfffeu16.to_le_bytes());
                assert_eq!(format.len(), 40);
                assert_eq!(
                    &format[18..20],
                    &profile.sample_format.bits_per_sample().to_le_bytes()
                );
                assert_eq!(&format[20..24], &[0; 4]);
                let decoded = decoded_wav_samples(&wav);
                for channel in 0..channels {
                    let mono: Vec<f32> = samples
                        .chunks_exact(channels)
                        .map(|frame| frame[channel])
                        .collect();
                    let reference =
                        reference_wav(44_100, 1, &mono, profile, WavChannelMap::Unchanged);
                    assert_eq!(
                        decoded
                            .chunks_exact(channels)
                            .map(|frame| frame[channel])
                            .collect::<Vec<_>>(),
                        decoded_wav_samples(&reference),
                        "{channels} channels, channel {channel}, {profile:?}"
                    );
                }
                // The bounded worker path must produce the same bytes with one or several workers.
                for threads in [1, 3] {
                    let mut streamed = Vec::new();
                    let output_frames = decoded.len() / channels;
                    with_slice_clip(44_100, channels, &samples, |clip| {
                        write_wave_from_clip(
                            &mut streamed,
                            profile,
                            channels as u16,
                            output_frames as u32,
                            wav_chunk(&wav, b"data").len() as u32,
                            clip,
                            WavChannelMap::Unchanged,
                            threads,
                        )
                    })
                    .unwrap();
                    assert_eq!(streamed, wav);
                }
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn channel_export_rejects_bad_selections_and_removes_incomplete_files() {
        let dir = test_dir("channel-errors");
        let track = Unreadable {
            lane: Snapshot {
                sample_rate: 48_000,
                channels: 12,
                samples: vec![0.25; 10_000 * 12],
                first_frame: 100,
            },
            readable: 5_000,
        };
        let profile = WavExportProfile::default();
        for channels in [0..0, 11..13, 12..11] {
            assert!(export_channel_range_with_profile(
                &dir,
                &track,
                0..10_000,
                channels,
                &[],
                profile,
                ChannelExportMode::MonoFiles,
                Fades::NONE,
                &TakeLabel::default(),
            )
            .is_err());
        }
        assert!(!dir.exists());
        assert!(export_channel_range_with_profile(
            &dir,
            &track,
            0..10_000,
            0..12,
            &["L".into()],
            profile,
            ChannelExportMode::MonoFiles,
            Fades::NONE,
            &TakeLabel::default(),
        )
        .is_err());
        assert!(!dir.exists());
        for mode in [
            ChannelExportMode::MonoFiles,
            ChannelExportMode::Multichannel,
        ] {
            assert!(export_channel_range_with_profile(
                &dir,
                &track,
                0..10_000,
                0..12,
                &[],
                profile,
                mode,
                Fades::NONE,
                &TakeLabel::default(),
            )
            .is_err());
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn frozen_tracks_export_the_same_files_as_flat_copies() {
        use crate::{Capture, DurationChoice};

        let dir = test_dir("frozen");
        // 2.27 one-second pages of a stereo lane A and a mono lane B.
        let mut capture =
            Capture::new_tracks(44_100, 2, Some(1), DurationChoice::Seconds10).unwrap();
        for (block, samples) in noise(7, 100_000 * 3).chunks(441 * 3).enumerate() {
            // Finite, so the capture keeps every block.
            let samples: Vec<f32> = samples
                .iter()
                .map(|sample| if sample.is_finite() { *sample } else { 0.5 })
                .collect();
            assert!(!samples.is_empty(), "block {block}");
            capture.push_interleaved(&samples).unwrap();
        }
        let flat: Vec<Snapshot> = (0..2)
            .map(|lane| capture.track_snapshot(lane).unwrap())
            .collect();
        let frozen = capture.freeze().tracks();
        assert_eq!(flat[0].frame_count(), 100_000);
        let routes = [ChannelRoute::Merged, ChannelRoute::Side];
        let maps = [WavChannelMap::Unchanged, WavChannelMap::Side];
        let mut exports = 0;
        // Ranges that start and end inside pages: across two page seams at the capture's
        // rate, and across one seam through the resampler.
        for (frames, sample_rate, sample_format) in [
            (30_001..95_003, 44_100, WavSampleFormat::Pcm24),
            (0..100_000, 44_100, WavSampleFormat::Float32),
            (43_000..47_501, 48_000, WavSampleFormat::Pcm16),
            (88_190..88_215, 96_000, WavSampleFormat::Float32),
        ] {
            let profile = WavExportProfile {
                sample_rate,
                sample_format,
            };
            let mut export = |tracks: &dyn Fn(&Path) -> io::Result<Vec<PathBuf>>| {
                exports += 1;
                let paths = tracks(&dir.join(exports.to_string())).unwrap();
                paths
                    .iter()
                    .map(|path| fs::read(path).unwrap())
                    .collect::<Vec<_>>()
            };
            let expected: Vec<Vec<u8>> = flat
                .iter()
                .zip(maps)
                .map(|(lane, map)| {
                    reference_wav(
                        lane.sample_rate,
                        lane.channels,
                        lane.frames(frames.clone()).unwrap(),
                        profile,
                        map,
                    )
                })
                .collect();
            let context = format!("frames {frames:?} to {profile:?}");
            let from_flat = export(&|dir| {
                export_routed_tracks_with_profile(
                    dir,
                    &flat,
                    frames.clone(),
                    profile,
                    &routes,
                    false,
                )
            });
            assert!(from_flat == expected, "flat copies, {context}");
            let from_frozen = export(&|dir| {
                export_routed_tracks_with_profile(
                    dir,
                    &frozen,
                    frames.clone(),
                    profile,
                    &routes,
                    false,
                )
            });
            assert!(from_frozen == expected, "frozen tracks, {context}");
            // A view of only the clip counts its frames from the clip's start.
            let first = frozen[0].first_frame();
            let part = capture
                .freeze_range(first + frames.start as u64..first + frames.end as u64)
                .unwrap()
                .tracks();
            let from_part = export(&|dir| {
                export_routed_tracks_with_profile(
                    dir,
                    &part,
                    0..frames.len(),
                    profile,
                    &routes,
                    false,
                )
            });
            assert!(from_part == expected, "a frozen range, {context}");
            // One lane on its own takes the single-file path.
            let single = export(&|dir| {
                export_routed_tracks_with_profile(
                    dir,
                    &frozen[1..],
                    frames.clone(),
                    profile,
                    &routes[1..],
                    false,
                )
            });
            assert!(single == expected[1..], "lane B alone, {context}");
        }
        // A range the lanes do not hold is refused before any file exists.
        let outside = dir.join("outside");
        assert!(export_tracks_with_profile(
            &outside,
            &frozen,
            99_999..100_001,
            WavExportProfile::default()
        )
        .is_err());
        assert!(!outside.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}

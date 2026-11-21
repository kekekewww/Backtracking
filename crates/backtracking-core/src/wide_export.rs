//! WAV and position exports for tracks with more than two channels.

use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    spatial::{self, BinauralRenderer, BinauralSource, HeadModel, PositionKey},
    Fades, FrozenPositionTrack, TakeLabel, TrackAudio, WavExportProfile,
};

/// How the selected native channels of a wide track are written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WideExportMode {
    /// One stereo binaural render per channel.
    #[default]
    BinauralFiles,
    /// One stereo binaural render of the selected channels together.
    BinauralMix,
    /// One mono WAV per channel in stream order.
    MonoFiles,
    /// One interleaved WAV in stream order.
    Multichannel,
}

/// A contiguous group in the track's native channel order.
#[derive(Clone, Debug)]
pub struct WideExportGroup<'a> {
    /// Zero-based channel indices in the track.
    pub channels: Range<usize>,
    /// One name for each channel in the range, in stream order.
    pub labels: &'a [String],
    /// Whether the group carries objects with position tracks.
    pub positioned: bool,
}

/// Which rows of a wide track to write.
#[derive(Default)]
pub struct WideRowExport<'a> {
    /// Zero-based native channel indices. None selects the entire shown group, or the whole track
    /// when no group is given. An empty slice selects only the pair and requires pair to be true.
    pub channels: Option<&'a [usize]>,
    /// Whether to write the L/R pair of the entire speaker bed.
    pub pair: bool,
    /// Whether the pair uses binaural rendering. Otherwise it uses the stereo fold-down.
    pub pair_binaural: bool,
    /// The speaker bed for the pair, needed when the shown group carries positioned objects.
    pub bed: Option<&'a WideExportGroup<'a>>,
    /// Positions frozen on the same absolute frame timeline as the audio.
    pub positions: Option<&'a FrozenPositionTrack>,
}

/// Files and status from one wide track export.
pub struct WideExportResult {
    /// Written WAVs in export order, followed by the positions JSON if it was written.
    pub paths: Vec<PathBuf>,
    /// At least one positioned-group channel had no keys and was written in mono.
    pub objects_in_mono: bool,
}

fn invalid_selection() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "The selected rows do not match this capture",
    )
}

enum Audio {
    Native,
    Stereo(Vec<[f32; 2]>),
    Binaural,
}

struct Output {
    suffix: String,
    indices: Vec<usize>,
    audio: Audio,
}

impl Output {
    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        path: &Path,
        track: &impl TrackAudio,
        frames: Range<usize>,
        fades: Fades,
        profile: WavExportProfile,
        sources: &[BinauralSource],
        model: Option<&Arc<HeadModel>>,
    ) -> io::Result<()> {
        let width = self.indices.len();
        let channels = if matches!(self.audio, Audio::Native) {
            width
        } else {
            2
        };
        let first = track.frame_range().start + frames.start as u64;
        let mut renderer = if matches!(self.audio, Audio::Binaural) {
            let selected = self
                .indices
                .iter()
                .map(|&i| sources[i].clone())
                .collect::<Vec<_>>();
            Some(
                BinauralRenderer::new(model.unwrap().clone(), &selected, None, first)
                    .map_err(io::Error::other)?,
            )
        } else {
            None
        };
        let mut native = Vec::new();
        let mut group = Vec::new();
        profile.write_rendered(
            path,
            track.sample_rate(),
            channels,
            frames.len(),
            fades,
            |offset, output| {
                let count = output.len() / channels;
                native.resize(count * track.channels(), 0.);
                if !track.read_frames(frames.start + offset, &mut native) {
                    return Err(invalid_selection());
                }
                let selected = if matches!(self.audio, Audio::Native) {
                    &mut *output
                } else {
                    group.resize(count * width, 0.);
                    &mut group[..]
                };
                for (src, dst) in native
                    .chunks_exact(track.channels())
                    .zip(selected.chunks_exact_mut(width))
                {
                    for (value, &i) in dst.iter_mut().zip(&self.indices) {
                        *value = src[i];
                    }
                }
                match &self.audio {
                    Audio::Native => Ok(()),
                    Audio::Stereo(weights) => {
                        spatial::fold_down(&group, weights, output).map_err(io::Error::other)
                    }
                    Audio::Binaural => renderer
                        .as_mut()
                        .unwrap()
                        .render(&group, output)
                        .map_err(io::Error::other),
                }
            },
        )
    }
}

/// Writes a clip from a 3- to 64-channel track. Frames are relative to the start of retained audio.
/// A shown group supplies labels and directions. With no groups, channels get numbered labels and
/// the L/R pair uses the full track. Explicit native indices are written in stream order.
///
/// Fades and the WAV profile apply to every output. A binaural pair and a full speaker-bed mix
/// share one file. The result contains paths rooted at output. If writing fails, files created
/// for the take are removed.
#[allow(clippy::too_many_arguments)]
pub fn export_wide_track(
    output: &Path,
    track: &impl TrackAudio,
    frames: Range<usize>,
    fades: Fades,
    profile: WavExportProfile,
    label: &TakeLabel,
    shown: Option<&WideExportGroup<'_>>,
    mode: WideExportMode,
    rows: &WideRowExport<'_>,
) -> io::Result<WideExportResult> {
    let width = track.channels();
    let retained = track.frame_range();
    if frames.is_empty()
        || frames.end as u64 > retained.end.saturating_sub(retained.start)
        || !(3..=64).contains(&width)
    {
        return Err(invalid_selection());
    }
    let absolute = retained.start + frames.start as u64..retained.start + frames.end as u64;
    let bed = rows.bed.or(shown.filter(|group| !group.positioned));
    if rows.pair && bed.is_none() && shown.is_some() {
        return Err(invalid_selection());
    }
    let mut labels = (1..=width)
        .map(|i| format!("Channel {i}"))
        .collect::<Vec<_>>();
    let mut sources = vec![BinauralSource::Fixed(None); width];
    for group in [bed, shown].into_iter().flatten() {
        let range = group.channels.clone();
        if range.is_empty() || range.end > width || range.len() != group.labels.len() {
            return Err(invalid_selection());
        }
        labels[range.clone()].clone_from_slice(group.labels);
        let names = group.labels.iter().map(String::as_str).collect::<Vec<_>>();
        for (i, direction) in range.zip(spatial::speaker_directions(&names)) {
            sources[i] = BinauralSource::Fixed(direction);
        }
    }
    if let Some(track) = rows.positions {
        for (i, source) in sources.iter_mut().enumerate() {
            let keys = track.render_keys(i, absolute.clone());
            if !keys.is_empty() {
                *source = BinauralSource::Moving(keys);
            }
        }
    }
    let mut selected = rows
        .channels
        .map(<[usize]>::to_vec)
        .unwrap_or_else(|| shown.map_or(0..width, |g| g.channels.clone()).collect());
    selected.sort_unstable();
    if (selected.is_empty() && !rows.pair)
        || selected.last().is_some_and(|&i| i >= width)
        || selected.windows(2).any(|w| w[0] == w[1])
    {
        return Err(invalid_selection());
    }
    let binaural = matches!(
        mode,
        WideExportMode::BinauralFiles | WideExportMode::BinauralMix
    );
    let mut jobs = Vec::new();
    let mut mixed = Vec::new();
    let mut objects_in_mono = false;
    for i in selected {
        let fallback = binaural
            && shown.is_some_and(|g| g.positioned && g.channels.contains(&i))
            && !matches!(sources[i], BinauralSource::Moving(_));
        objects_in_mono |= fallback;
        if !fallback
            && matches!(
                mode,
                WideExportMode::Multichannel | WideExportMode::BinauralMix
            )
        {
            mixed.push(i);
        } else {
            let mut suffix = TakeLabel::channel_suffix(i, &labels[i]);
            let audio = if binaural && !fallback {
                suffix.push_str("_binaural");
                Audio::Binaural
            } else {
                Audio::Native
            };
            jobs.push(Output {
                suffix,
                indices: vec![i],
                audio,
            });
        }
    }
    let range = bed.map_or(0..width, |g| g.channels.clone());
    let whole_bed_mix =
        binaural && (bed.is_some() || shown.is_none()) && mixed.iter().copied().eq(range.clone());
    if !mixed.is_empty() {
        let suffix = if !binaural {
            ""
        } else if whole_bed_mix {
            "-Binaural"
        } else {
            "-Mix-Binaural"
        };
        jobs.push(Output {
            suffix: suffix.into(),
            indices: mixed,
            audio: if binaural {
                Audio::Binaural
            } else {
                Audio::Native
            },
        });
    }
    if rows.pair && !(rows.pair_binaural && whole_bed_mix) {
        let suffix = if rows.pair_binaural {
            "-Binaural"
        } else {
            "-Stereo"
        };
        let names = labels[range.clone()]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        jobs.push(Output {
            suffix: suffix.into(),
            indices: range.collect(),
            audio: if rows.pair_binaural {
                Audio::Binaural
            } else {
                Audio::Stereo(spatial::stereo_weights(&names))
            },
        });
    }
    let model = jobs
        .iter()
        .any(|job| matches!(job.audio, Audio::Binaural))
        .then(|| HeadModel::new(track.sample_rate()).map(Arc::new))
        .transpose()
        .map_err(io::Error::other)?;
    std::fs::create_dir_all(output)?;
    for stem in label.stems(frames.len(), track.sample_rate()) {
        let paths = jobs
            .iter()
            .map(|job| output.join(format!("{stem}{}.wav", job.suffix)))
            .collect::<Vec<_>>();
        let sidecar = output.join(format!("{stem}-Positions.json"));
        if paths.iter().any(|path| path.exists()) || sidecar.exists() {
            continue;
        }
        let mut created = Vec::new();
        let write: io::Result<()> = (|| {
            let workers = profile.worker_limit();
            for (batch, files) in jobs.chunks(workers).zip(paths.chunks(workers)) {
                std::thread::scope(|scope| {
                    let mut running = Vec::new();
                    let mut result = Ok(());
                    for (job, path) in batch.iter().zip(files) {
                        match std::thread::Builder::new().spawn_scoped(scope, || {
                            job.write(
                                path,
                                track,
                                frames.clone(),
                                fades,
                                profile,
                                &sources,
                                model.as_ref(),
                            )
                        }) {
                            Ok(worker) => running.push((worker, path)),
                            Err(error) => {
                                result = Err(error);
                                break;
                            }
                        }
                    }
                    for (worker, path) in running {
                        match worker.join().unwrap_or_else(|_| {
                            Err(io::Error::other("audio export worker stopped"))
                        }) {
                            Ok(()) => created.push(path.clone()),
                            Err(error) => result = Err(error),
                        }
                    }
                    result
                })?;
            }
            let mut channels = Vec::new();
            for (i, source) in sources.iter().enumerate() {
                let BinauralSource::Moving(keys) = source else {
                    continue;
                };
                if let Some(index) = jobs.iter().position(|job| job.indices.contains(&i)) {
                    channels.push(PositionChannel {
                        channel: i + 1,
                        label: &labels[i],
                        file: paths[index].file_name().unwrap().to_str().unwrap(),
                        keys: clip_keys(keys, absolute.clone()),
                    });
                }
            }
            if !channels.is_empty() {
                let file = std::fs::File::create_new(&sidecar)?;
                created.push(sidecar.clone());
                let mut writer = io::BufWriter::new(file);
                write_positions(&mut writer, track.sample_rate(), frames.len(), &channels)?;
                writer.flush()?;
            }
            Ok(())
        })();
        match write {
            Ok(()) => {
                return Ok(WideExportResult {
                    paths: created,
                    objects_in_mono,
                });
            }
            Err(error) => {
                let mut cleanup = Ok(());
                for path in created {
                    cleanup = cleanup.and(std::fs::remove_file(path));
                }
                cleanup?;
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
    }
    Err(io::Error::other("No free take name"))
}

struct PositionChannel<'a> {
    channel: usize,
    label: &'a str,
    file: &'a str,
    keys: Vec<ClipKey>,
}
struct ClipKey {
    frame: u64,
    x: f32,
    y: f32,
    z: f32,
    size: f32,
    gain: f32,
}

// The sidecar has a fixed schema. Keep its compact form without adding a dependency to core.
fn write_positions(
    writer: &mut impl Write,
    sample_rate: u32,
    frames: usize,
    channels: &[PositionChannel<'_>],
) -> io::Result<()> {
    write!(
        writer,
        "{{\"version\":1,\"sample_rate\":{sample_rate},\"frames\":{frames},\
         \"coordinates\":\"listener at 0,0,0; x -1 left to 1 right; y -1 back to 1 front; z -1 floor to 1 ceiling\",\
         \"channels\":["
    )?;
    for (index, channel) in channels.iter().enumerate() {
        if index != 0 {
            writer.write_all(b",")?;
        }
        write!(writer, "{{\"channel\":{},\"label\":", channel.channel)?;
        write_json_string(writer, channel.label)?;
        writer.write_all(b",\"file\":")?;
        write_json_string(writer, channel.file)?;
        writer.write_all(b",\"keys\":[")?;
        for (index, key) in channel.keys.iter().enumerate() {
            if index != 0 {
                writer.write_all(b",")?;
            }
            write!(writer, "{{\"frame\":{}", key.frame)?;
            for (name, value) in [
                ("x", key.x),
                ("y", key.y),
                ("z", key.z),
                ("size", key.size),
                ("gain", key.gain),
            ] {
                write!(writer, ",\"{name}\":")?;
                write_json_float(writer, value)?;
            }
            writer.write_all(b"}")?;
        }
        writer.write_all(b"]}")?;
    }
    writer.write_all(b"]}")
}

fn write_json_string(writer: &mut impl Write, text: &str) -> io::Result<()> {
    writer.write_all(b"\"")?;
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        let escape = match byte {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            b'\x08' => b"\\b",
            b'\x0c' => b"\\f",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            b'\x00'..=b'\x1f' => {
                writer.write_all(&text.as_bytes()[start..index])?;
                write!(writer, "\\u{byte:04x}")?;
                start = index + 1;
                continue;
            }
            _ => continue,
        };
        writer.write_all(&text.as_bytes()[start..index])?;
        writer.write_all(escape)?;
        start = index + 1;
    }
    writer.write_all(&text.as_bytes()[start..])?;
    writer.write_all(b"\"")
}

// Match the old sidecar's f32 notation, including its fixed-point range and signed exponents.
fn write_json_float(writer: &mut impl Write, value: f32) -> io::Result<()> {
    if !value.is_finite() {
        return writer.write_all(b"null");
    }
    // Find the shortest f32 precision, then round the exact value to it with ties to even.
    let absolute = value.abs();
    if absolute >= 1e13 || (absolute != 0.0 && absolute < 1e-6) {
        let scientific = format!("{value:e}");
        let (mantissa, _) = scientific.split_once('e').expect("scientific notation");
        let digits = mantissa
            .split_once('.')
            .map_or(0, |(_, digits)| digits.len());
        let scientific = format!("{:.*e}", digits, f64::from(value));
        if absolute >= 1e13 {
            let (mantissa, exponent) = scientific.split_once('e').expect("scientific notation");
            write!(writer, "{mantissa}e+{exponent}")
        } else {
            writer.write_all(scientific.as_bytes())
        }
    } else {
        let shortest = value.to_string();
        match shortest.split_once('.') {
            Some((_, digits)) => write!(writer, "{:.*}", digits.len(), f64::from(value)),
            None => write!(writer, "{shortest}.0"),
        }
    }
}

fn clip_keys(keys: &[PositionKey], frames: Range<u64>) -> Vec<ClipKey> {
    let state = |frame| {
        let next = keys.partition_point(|key| key.frame <= frame);
        if next == 0 {
            return ClipKey {
                frame: frame - frames.start,
                x: 0.,
                y: 0.,
                z: 0.,
                size: 0.,
                gain: 1.,
            };
        }
        let a = keys[next - 1];
        let b = keys.get(next).unwrap_or(&a);
        let t = (frame - a.frame) as f64 / (b.frame - a.frame).max(1) as f64;
        let lerp = |x: f32, y: f32| (f64::from(x) + (f64::from(y) - f64::from(x)) * t) as f32;
        ClipKey {
            frame: frame - frames.start,
            x: lerp(a.x, b.x),
            y: lerp(a.y, b.y),
            z: lerp(a.z, b.z),
            size: lerp(a.size, b.size),
            gain: lerp(a.gain, b.gain),
        }
    };
    let mut result = vec![state(frames.start)];
    if keys[0].frame.saturating_sub(1) > frames.start && keys[0].frame < frames.end {
        result.push(state(keys[0].frame - 1));
    }
    for key in keys
        .iter()
        .filter(|k| k.frame > frames.start && k.frame < frames.end)
    {
        result.push(state(key.frame));
    }
    let last = frames.end - 1;
    if keys.last().is_some_and(|k| k.frame > last)
        && result.last().unwrap().frame < last - frames.start
    {
        result.push(state(last));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrozenTrack;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let name = format!("backtracking-wide-export-{}-{nonce}", std::process::id());
            Self(std::env::temp_dir().join(name))
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn wav_window(path: &Path, offset: usize, count: usize) -> (usize, Vec<f32>) {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path).unwrap();
        file.seek(SeekFrom::Start(22)).unwrap();
        let mut width = [0; 2];
        file.read_exact(&mut width).unwrap();
        let width = u16::from_le_bytes(width) as usize;
        file.seek(SeekFrom::Start(12)).unwrap();
        loop {
            let mut header = [0; 8];
            file.read_exact(&mut header).unwrap();
            let bytes = u32::from_le_bytes(header[4..].try_into().unwrap());
            if &header[..4] == b"data" {
                assert!(offset + count <= bytes as usize / (4 * width));
                break;
            }
            file.seek(SeekFrom::Current(i64::from(bytes + bytes % 2)))
                .unwrap();
        }
        file.seek(SeekFrom::Current((offset * width * 4) as i64))
            .unwrap();
        let mut data = vec![0; count * width * 4];
        file.read_exact(&mut data).unwrap();
        (
            width,
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|&bytes| f32::from_le_bytes(bytes))
                .collect(),
        )
    }

    fn file_names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn resampled_binaural_pair_matches_profile_after_render_and_fades() {
        use crate::{write_wav_with_profile, WavSampleFormat};
        let folder = Folder::new();
        let raw: Vec<_> = (0..16_789 * 3)
            .map(|i| ((i * 13 % 53) as f32 - 26.) * 0.015)
            .collect();
        let track = FrozenTrack::from_interleaved(48000, 3, 763, &raw).unwrap();
        let group = WideExportGroup {
            positioned: false,
            channels: 0..3,
            labels: &["FL", "FR", "FC"].map(str::to_owned),
        };
        let profile = WavExportProfile {
            sample_rate: 44100,
            sample_format: WavSampleFormat::Pcm24,
        };
        let frames = 113..15987;
        let fades = Fades::new(1234, 567);
        let result = export_wide_track(
            &folder.0,
            &track,
            frames.clone(),
            fades,
            profile,
            &TakeLabel::default(),
            Some(&group),
            WideExportMode::BinauralMix,
            &WideRowExport {
                pair: true,
                pair_binaural: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.paths.len(), 1);
        let sources = spatial::speaker_directions(&["FL", "FR", "FC"])
            .into_iter()
            .map(BinauralSource::Fixed)
            .collect::<Vec<_>>();
        let mut pcm = spatial::render_binaural(
            Arc::new(HeadModel::new(48000).unwrap()),
            &sources,
            None,
            763 + frames.start as u64,
            &raw[frames.start * 3..frames.end * 3],
        )
        .unwrap();
        fades.apply(&mut pcm, 2, 0, frames.len() as u64);
        let expected = folder.0.join("reference.wav");
        write_wav_with_profile(&expected, 48000, 2, &pcm, profile).unwrap();
        assert_eq!(
            std::fs::read(expected).unwrap(),
            std::fs::read(&result.paths[0]).unwrap()
        );
    }

    #[test]
    fn unplaced_objects_are_plain_mono_in_both_binaural_modes() {
        let folder = Folder::new();
        let track =
            FrozenTrack::from_interleaved(48000, 4, 0, &[0.25, 0.5, -0.25, -0.5].repeat(512))
                .unwrap();
        let group = WideExportGroup {
            positioned: true,
            channels: 2..4,
            labels: &["First".into(), "Second".into()],
        };
        for mode in [WideExportMode::BinauralMix, WideExportMode::BinauralFiles] {
            let files = export_wide_track(
                &folder.0,
                &track,
                0..512,
                Fades::NONE,
                WavExportProfile::default(),
                &TakeLabel::default(),
                Some(&group),
                mode,
                &WideRowExport::default(),
            )
            .unwrap();
            assert!(files.objects_in_mono);
            assert_eq!(files.paths.len(), 2);
            for (i, path) in files.paths.iter().enumerate() {
                assert!(path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .ends_with(["-Ch03_First.wav", "-Ch04_Second.wav"][i]));
                assert_eq!(wav_window(path, 0, 512), (1, vec![[-0.25, -0.5][i]; 512]));
            }
        }
    }

    #[test]
    fn subset_mix_and_full_binaural_pair_have_different_names_and_audio() {
        let folder = Folder::new();
        let track =
            FrozenTrack::from_interleaved(48000, 3, 0, &[0., 0.25, -0.125].repeat(1024)).unwrap();
        let bed = WideExportGroup {
            positioned: false,
            channels: 0..3,
            labels: &["FL", "FR", "FC"].map(str::to_owned),
        };
        let export = |indices| {
            export_wide_track(
                &folder.0,
                &track,
                0..1024,
                Fades::NONE,
                WavExportProfile::default(),
                &TakeLabel::default(),
                Some(&bed),
                WideExportMode::BinauralMix,
                &WideRowExport {
                    channels: indices,
                    pair: true,
                    pair_binaural: true,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let both = export(Some(&[0][..]));
        let names = file_names(&both.paths);
        assert_eq!(names.len(), 2);
        assert!(names[0].ends_with("-Mix-Binaural.wav") && names[1].ends_with("s-Binaural.wav"));
        assert!(names.iter().all(|name| crate::is_export_file_name(name)));
        assert!(wav_window(&both.paths[0], 0, 1024)
            .1
            .iter()
            .all(|&x| x == 0.));
        assert!(wav_window(&both.paths[1], 0, 1024)
            .1
            .iter()
            .any(|&x| x != 0.));
        let whole = export(None);
        assert_eq!(whole.paths.len(), 1);
        assert_eq!(
            std::fs::read(&whole.paths[0]).unwrap(),
            std::fs::read(&both.paths[1]).unwrap()
        );
    }

    #[test]
    fn read_failure_rolls_back_every_parallel_output() {
        struct Broken;
        impl TrackAudio for Broken {
            fn channels(&self) -> usize {
                12
            }
            fn sample_rate(&self) -> u32 {
                48000
            }
            fn frame_range(&self) -> Range<u64> {
                0..10_000
            }
            fn read_frames(&self, first: usize, dst: &mut [f32]) -> bool {
                dst.fill(0.125);
                first < 4096
            }
        }
        let folder = Folder::new();
        assert!(export_wide_track(
            &folder.0,
            &Broken,
            0..10_000,
            Fades::NONE,
            WavExportProfile::default(),
            &TakeLabel::default(),
            None,
            WideExportMode::BinauralFiles,
            &WideRowExport {
                pair: true,
                ..Default::default()
            }
        )
        .is_err());
        assert_eq!(std::fs::read_dir(folder.0.as_path()).unwrap().count(), 0);
    }

    #[test]
    fn positions_json_keeps_the_compact_number_and_string_format() {
        let channel = PositionChannel {
            channel: 3,
            label: "物件 \"3\"\n\u{1f}",
            file: "Backtracking\\take.wav",
            keys: vec![ClipKey {
                frame: 0,
                x: 1e-6,
                y: -1e-7,
                z: -0.0,
                size: 219_650.0 + 0.125,
                gain: 1e13,
            }],
        };
        let mut json = Vec::new();
        write_positions(&mut json, 48_000, 24, &[channel]).unwrap();
        assert_eq!(
            String::from_utf8(json).unwrap(),
            concat!(
                r#"{"version":1,"sample_rate":48000,"frames":24,"coordinates":"#,
                r#""listener at 0,0,0; x -1 left to 1 right; y -1 back to 1 front; z -1 floor to 1 ceiling","#,
                r#""channels":[{"channel":3,"label":"物件 \"3\"\n\u001f","file":"Backtracking\\take.wav","#,
                r#""keys":[{"frame":0,"x":0.000001,"y":-1e-7,"z":-0.0,"size":219650.12,"gain":1e+13}]}]}"#,
            ),
        );
    }
}

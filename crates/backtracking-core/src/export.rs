use std::borrow::Cow;
use std::f64::consts::PI;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Snapshot;

const PHASES: usize = 256;
static NEXT_EXPORT: AtomicU64 = AtomicU64::new(0);

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

    fn format_chunk_size(self) -> u32 {
        match self {
            Self::Pcm16 => 16,
            Self::Pcm24 => 40,   // WAVEFORMATEXTENSIBLE
            Self::Float32 => 18, // WAVEFORMATEX
        }
    }

    fn riff_overhead(self) -> u32 {
        let fact_bytes = if self == Self::Float32 { 12 } else { 0 };
        4 + 8 + self.format_chunk_size() + fact_bytes + 8
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

/// Writes a new mono or stereo 48 kHz, IEEE float32 WAV. The caller can expose
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

/// Writes a new mono or stereo WAV using the requested export profile.
pub fn write_wav_with_profile(
    path: &Path,
    source_sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
    profile: WavExportProfile,
) -> io::Result<()> {
    if source_sample_rate == 0 || !matches!(channels, 1 | 2) || interleaved.len() % channels != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a nonzero sample rate and whole mono or stereo frames",
        ));
    }
    if !WavExportProfile::SAMPLE_RATES.contains(&profile.sample_rate) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected an export rate of 44,100, 48,000 or 96,000 Hz",
        ));
    }

    let source_frames = interleaved.len() / channels;
    let rate = u128::from(source_sample_rate);
    let rounded = (source_frames as u128 * u128::from(profile.sample_rate) + rate / 2) / rate;
    let frames = rounded.max((source_frames != 0) as u128);
    let frame_bytes = channels as u16 * (profile.sample_format.bits_per_sample() / 8);
    let data_bytes = frames * u128::from(frame_bytes);
    let riff_size = data_bytes + data_bytes % 2 + u128::from(profile.sample_format.riff_overhead());
    if riff_size > u128::from(u32::MAX) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "selection exceeds the RIFF WAV size limit",
        ));
    }

    let samples = if source_sample_rate == profile.sample_rate || source_frames == 0 {
        Cow::Borrowed(interleaved)
    } else {
        Cow::Owned(resample(
            interleaved,
            channels,
            source_sample_rate,
            profile.sample_rate,
            frames as usize,
        ))
    };

    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = {
        let mut writer = BufWriter::new(file);
        write_wave(
            &mut writer,
            profile,
            channels as u16,
            frames as u32,
            data_bytes as u32,
            &samples,
        )
        .and_then(|()| writer.flush())
    };
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
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
    fs::create_dir_all(output_dir)?;
    loop {
        let prefix = take_name();
        let path = output_dir.join(format!("{prefix}.wav"));
        match write_wav_with_profile(&path, source_sample_rate, channels, interleaved, profile) {
            Ok(()) => return Ok(path),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

fn take_name() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let process = std::process::id();
    let serial = NEXT_EXPORT.fetch_add(1, Ordering::Relaxed);
    format!("Backtracking-{timestamp}-{process}-{serial}")
}

/// Exports one or two lanes from the same retained time range.
/// Both files in a pair share a take name, with A/B suffixes.
pub fn export_tracks_with_profile(
    output_dir: &Path,
    snapshots: &[Snapshot],
    frames: Range<usize>,
    profile: WavExportProfile,
) -> io::Result<Vec<PathBuf>> {
    match snapshots {
        [one] => export_track_refs(output_dir, &[one], frames, profile),
        [a, b] => export_track_refs(output_dir, &[a, b], frames, profile),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected one or two audio lanes",
        )),
    }
}

pub(crate) fn export_track_refs(
    output_dir: &Path,
    snapshots: &[&Snapshot],
    frames: Range<usize>,
    profile: WavExportProfile,
) -> io::Result<Vec<PathBuf>> {
    match snapshots {
        [snapshot] => export_wav_with_profile(
            output_dir,
            snapshot.sample_rate,
            snapshot.channels,
            track_clip(snapshot, frames)?,
            profile,
        )
        .map(|path| vec![path]),
        [a, b] if a.sample_rate == b.sample_rate && a.frame_range() == b.frame_range() => {
            let a_samples = track_clip(a, frames.clone())?;
            let b_samples = track_clip(b, frames)?;
            fs::create_dir_all(output_dir)?;
            loop {
                let prefix = take_name();
                let a_path = output_dir.join(format!("{prefix}-A.wav"));
                let b_path = output_dir.join(format!("{prefix}-B.wav"));
                match write_wav_with_profile(&a_path, a.sample_rate, a.channels, a_samples, profile)
                {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(err) => return Err(err),
                }
                match write_wav_with_profile(&b_path, b.sample_rate, b.channels, b_samples, profile)
                {
                    Ok(()) => return Ok(vec![a_path, b_path]),
                    Err(err) => {
                        fs::remove_file(a_path)?;
                        if err.kind() != io::ErrorKind::AlreadyExists {
                            return Err(err);
                        }
                    }
                }
            }
        }
        _ => Err(invalid_tracks()),
    }
}

fn track_clip(snapshot: &Snapshot, frames: Range<usize>) -> io::Result<&[f32]> {
    snapshot
        .frames(frames)
        .filter(|samples| !samples.is_empty())
        .ok_or_else(invalid_tracks)
}

fn invalid_tracks() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "expected aligned nonempty audio lanes",
    )
}

fn write_wave(
    output: &mut impl Write,
    profile: WavExportProfile,
    channels: u16,
    frames: u32,
    data_bytes: u32,
    samples: &[f32],
) -> io::Result<()> {
    let format = profile.sample_format;
    let bits = format.bits_per_sample();
    let sample_bytes = bits / 8;
    let frame_bytes = channels * sample_bytes;
    let padding = data_bytes % 2;
    let format_tag = match format {
        WavSampleFormat::Pcm16 => 1u16,   // WAVE_FORMAT_PCM
        WavSampleFormat::Pcm24 => 0xfffe, // WAVE_FORMAT_EXTENSIBLE, PCM subformat
        WavSampleFormat::Float32 => 3,    // WAVE_FORMAT_IEEE_FLOAT
    };
    output.write_all(b"RIFF")?;
    output.write_all(&(data_bytes + padding + format.riff_overhead()).to_le_bytes())?;
    output.write_all(b"WAVEfmt ")?;
    output.write_all(&format.format_chunk_size().to_le_bytes())?;
    output.write_all(&format_tag.to_le_bytes())?;
    output.write_all(&channels.to_le_bytes())?;
    output.write_all(&profile.sample_rate.to_le_bytes())?;
    output.write_all(&(profile.sample_rate * u32::from(frame_bytes)).to_le_bytes())?;
    output.write_all(&frame_bytes.to_le_bytes())?;
    output.write_all(&bits.to_le_bytes())?;
    match format {
        WavSampleFormat::Pcm16 => {}
        WavSampleFormat::Pcm24 => {
            const PCM_SUBFORMAT: [u8; 16] = [
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38,
                0x9b, 0x71,
            ];
            let channel_mask = if channels == 1 { 0x4u32 } else { 0x3u32 };
            output.write_all(&22u16.to_le_bytes())?; // Extension size
            output.write_all(&bits.to_le_bytes())?; // Valid bits
            output.write_all(&channel_mask.to_le_bytes())?;
            output.write_all(&PCM_SUBFORMAT)?;
        }
        WavSampleFormat::Float32 => {
            output.write_all(&0u16.to_le_bytes())?; // Extension size
            output.write_all(b"fact")?;
            output.write_all(&4u32.to_le_bytes())?;
            output.write_all(&frames.to_le_bytes())?;
        }
    }
    output.write_all(b"data")?;
    output.write_all(&data_bytes.to_le_bytes())?;
    let sample_bytes = usize::from(sample_bytes);
    let mut bytes = [0; 4096];
    for chunk in samples.chunks(bytes.len() / sample_bytes) {
        for (slot, &sample) in bytes.chunks_exact_mut(sample_bytes).zip(chunk) {
            let sample = match format {
                WavSampleFormat::Pcm16 => quantize(sample, 1 << 15).to_le_bytes(),
                WavSampleFormat::Pcm24 => quantize(sample, 1 << 23).to_le_bytes(),
                WavSampleFormat::Float32 => sample.to_le_bytes(),
            };
            slot.copy_from_slice(&sample[..sample_bytes]);
        }
        output.write_all(&bytes[..chunk.len() * sample_bytes])?;
    }
    if padding != 0 {
        output.write_all(&[0])?;
    }
    Ok(())
}

fn quantize(sample: f32, full_scale: i32) -> i32 {
    // Rust's float-to-int cast maps NaN to 0 and saturates infinities.
    let scaled = (f64::from(sample) * f64::from(full_scale)).round() as i32;
    scaled.clamp(-full_scale, full_scale - 1)
}

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

    pub(crate) fn at(&self, samples: &[f32], channels: usize, position: f64) -> [f32; 2] {
        let last_frame = (samples.len() / channels - 1) as isize;
        let first = position.floor() as isize - self.radius;
        let phase = (position.fract() * PHASES as f64).round() as usize;
        let mut value = [0.0f64; 2];
        for (tap, &weight) in self.phases[phase].iter().enumerate() {
            let source_frame = (first + tap as isize).clamp(0, last_frame) as usize;
            let source = &samples[source_frame * channels..][..channels];
            for (sum, &sample) in value.iter_mut().zip(source) {
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
    use super::*;

    #[test]
    fn grouped_exports_keep_exact_l_r_samples_and_remove_incomplete_pair() {
        let dir = std::env::temp_dir().join(take_name());
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
            channels: 3,
            samples: vec![0.5; 12],
            ..a.clone()
        };
        assert!(export_tracks_with_profile(&dir, &[a, invalid], 1..3, profile).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wave_header_and_samples_are_ieee_float_stereo() {
        let samples = [1.25, -0.125, 0.5, 0.75];
        let mut wave = Vec::new();
        write_wave(&mut wave, WavExportProfile::default(), 2, 2, 16, &samples).unwrap();

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
        write_wave(&mut wave, profile, 1, 5, 10, &samples).unwrap();
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
        write_wave(&mut wave, profile, 1, 5, 15, &samples).unwrap();
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
        let serial = NEXT_EXPORT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("backtracking-wav-{}-{serial}", std::process::id()));
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
        let serial = NEXT_EXPORT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "backtracking-profiles-{}-{serial}",
            std::process::id()
        ));
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
}

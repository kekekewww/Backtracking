use std::borrow::Cow;
use std::f64::consts::PI;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const WAV_SAMPLE_RATE: u32 = 48_000;

const WAV_HEADER_BYTES: u32 = 58;
const PHASES: usize = 256;
static NEXT_EXPORT: AtomicU64 = AtomicU64::new(0);

/// Writes a new mono or stereo 48 kHz, IEEE float32 WAV. The caller can expose
/// the path for dragging after this returns successfully.
pub fn write_wav(
    path: &Path,
    source_sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
) -> io::Result<()> {
    if source_sample_rate == 0 || !matches!(channels, 1 | 2) || interleaved.len() % channels != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a nonzero sample rate and whole mono or stereo frames",
        ));
    }

    let source_frames = interleaved.len() / channels;
    let rate = u128::from(source_sample_rate);
    let rounded = (source_frames as u128 * u128::from(WAV_SAMPLE_RATE) + rate / 2) / rate;
    let frames = rounded.max((source_frames != 0) as u128);
    let frame_bytes = channels as u16 * 4;
    let data_bytes = frames * u128::from(frame_bytes);
    if data_bytes + u128::from(WAV_HEADER_BYTES - 8) > u128::from(u32::MAX) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "selection exceeds the RIFF WAV size limit",
        ));
    }

    let samples = if source_sample_rate == WAV_SAMPLE_RATE || source_frames == 0 {
        Cow::Borrowed(interleaved)
    } else {
        Cow::Owned(resample(
            interleaved,
            channels,
            source_sample_rate,
            frames as usize,
        ))
    };

    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = {
        let mut writer = BufWriter::new(file);
        write_wave(
            &mut writer,
            frame_bytes,
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
    fs::create_dir_all(output_dir)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let process = std::process::id();
    loop {
        let serial = NEXT_EXPORT.fetch_add(1, Ordering::Relaxed);
        let path = output_dir.join(format!("Backtracking-{timestamp}-{process}-{serial}.wav"));
        match write_wav(&path, source_sample_rate, channels, interleaved) {
            Ok(()) => return Ok(path),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

fn write_wave(
    output: &mut impl Write,
    frame_bytes: u16,
    frames: u32,
    data_bytes: u32,
    samples: &[f32],
) -> io::Result<()> {
    output.write_all(b"RIFF")?;
    output.write_all(&(data_bytes + (WAV_HEADER_BYTES - 8)).to_le_bytes())?;
    output.write_all(b"WAVEfmt ")?;
    output.write_all(&18u32.to_le_bytes())?;
    output.write_all(&3u16.to_le_bytes())?; // WAVE_FORMAT_IEEE_FLOAT
    output.write_all(&(frame_bytes / 4).to_le_bytes())?;
    output.write_all(&WAV_SAMPLE_RATE.to_le_bytes())?;
    output.write_all(&(WAV_SAMPLE_RATE * u32::from(frame_bytes)).to_le_bytes())?;
    output.write_all(&frame_bytes.to_le_bytes())?;
    output.write_all(&32u16.to_le_bytes())?;
    output.write_all(&0u16.to_le_bytes())?; // WAVEFORMATEX extension size
    output.write_all(b"fact")?;
    output.write_all(&4u32.to_le_bytes())?;
    output.write_all(&frames.to_le_bytes())?;
    output.write_all(b"data")?;
    output.write_all(&data_bytes.to_le_bytes())?;
    let mut bytes = [0; 4096];
    for chunk in samples.chunks(bytes.len() / 4) {
        for (slot, sample) in bytes.chunks_exact_mut(4).zip(chunk) {
            slot.copy_from_slice(&sample.to_le_bytes());
        }
        output.write_all(&bytes[..chunk.len() * 4])?;
    }
    Ok(())
}

fn resample(samples: &[f32], channels: usize, source_rate: u32, frames: usize) -> Vec<f32> {
    let step = f64::from(source_rate) / f64::from(WAV_SAMPLE_RATE);
    let cutoff = if step > 1.0 { 0.95 / step } else { 1.0 };
    let radius = (24.0 / cutoff).ceil().min(192.0) as isize;
    let kernels = phase_kernels(cutoff, radius);
    let last_frame = (samples.len() / channels - 1) as isize;
    let mut output = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        let position = frame as f64 * step;
        let first = position.floor() as isize - radius;
        let phase = (position.fract() * PHASES as f64).round() as usize;
        let mut value = [0.0f64; 2];
        for (tap, &weight) in kernels[phase].iter().enumerate() {
            let source_frame = (first + tap as isize).clamp(0, last_frame) as usize;
            let source = &samples[source_frame * channels..][..channels];
            for (sum, &sample) in value.iter_mut().zip(source) {
                *sum += f64::from(sample) * weight;
            }
        }
        output.extend(value[..channels].iter().map(|sample| *sample as f32));
    }
    output
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
    fn wave_header_and_samples_are_ieee_float_stereo() {
        let samples = [1.25, -0.125, 0.5, 0.75];
        let mut wave = Vec::new();
        write_wave(&mut wave, 8, 2, 16, &samples).unwrap();

        assert_eq!(&wave[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(wave[4..8].try_into().unwrap()) as usize,
            wave.len() - 8
        );
        assert_eq!(&wave[8..16], b"WAVEfmt ");
        assert_eq!(&wave[20..24], &[3, 0, 2, 0]);
        assert_eq!(&wave[24..28], &WAV_SAMPLE_RATE.to_le_bytes());
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
    fn resampling_keeps_channel_order_and_level() {
        let input: Vec<_> = [0.5, -0.25].repeat(441);
        let output = resample(&input, 2, 44_100, 480);
        assert_eq!(output.len(), 960);
        for frame in output.chunks_exact(2) {
            assert!((frame[0] - 0.5).abs() < 1e-6);
            assert!((frame[1] + 0.25).abs() < 1e-6);
        }
    }

    #[test]
    fn downsampling_rejects_content_above_the_output_nyquist_frequency() {
        let input: Vec<_> = (0..960)
            .map(|frame| (2.0 * PI * 34_000.0 * f64::from(frame) / 96_000.0).sin() as f32)
            .collect();
        let output = resample(&input, 1, 96_000, 480);
        assert!(output[48..432].iter().all(|sample| sample.abs() < 0.001));
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
}

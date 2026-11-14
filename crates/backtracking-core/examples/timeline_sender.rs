//! Quiet, real-time Sender fixture for desktop bar/seconds timeline checks.
//!
//! `--describe` and `--check` never create or register a Sender endpoint.
//! Normal execution publishes only to the Sender API, not a system audio device.
//! `--start-frame` controls signal/bar phase; the wire format has no source frame
//! field, so desktop retained frame zero still depends on when capture connects.

use backtracking_core::{capture_clock_ns, Capture, DurationChoice, SenderEndpoint, SenderInfo};
use std::error::Error;
use std::f64::consts::{FRAC_PI_2, TAU};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const BLOCK_FRAMES: usize = 480;
const BAR_FRAMES: u64 = 96_000;
const BEAT_FRAMES: u64 = 24_000;
const NAME: &str = "Backtracking Timeline Test";

struct Options {
    duration_seconds: u64,
    start_frame: u64,
    stop_marker: Option<PathBuf>,
    describe: bool,
    check: bool,
}

impl Options {
    fn parse() -> Result<Option<Self>, Box<dyn Error>> {
        let mut options = Self {
            duration_seconds: 300,
            start_frame: 0,
            stop_marker: None,
            describe: false,
            check: false,
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--duration-seconds" | "--durationSeconds" => {
                    options.duration_seconds = arguments.next().ok_or("missing duration")?.parse()?;
                }
                "--start-frame" => options.start_frame = arguments.next().ok_or("missing start frame")?.parse()?,
                "--stop-marker" => options.stop_marker = Some(arguments.next().ok_or("missing stop marker")?.into()),
                "--describe" => options.describe = true,
                "--check" => options.check = true,
                "--help" | "-h" => {
                    println!("timeline_sender [--duration-seconds N] [--start-frame N] [--stop-marker PATH] [--describe | --check]\n\
                        Defaults: 300 seconds, source frame 0. --describe/--check do not register or send.\n\
                        An existing stop marker prevents startup; a newly created marker stops within one block.\n\
                        Source frame controls waveform/bar phase, not desktop retained timeline origin.");
                    return Ok(None);
                }
                _ => return Err(format!("unknown argument: {argument}").into()),
            }
        }
        if options.duration_seconds == 0 {
            return Err("duration must be at least one second".into());
        }
        let frames = options.duration_seconds.checked_mul(u64::from(RATE)).ok_or("duration overflow")?;
        options.start_frame.checked_add(frames).ok_or("source frame overflow")?;
        frame_duration(frames)?;
        if options.describe && options.check {
            return Err("choose only one of --describe and --check".into());
        }
        Ok(Some(options))
    }

    fn stopped(&self) -> bool {
        self.stop_marker.as_ref().is_some_and(|path| path.exists())
    }
}

fn fill_block(first: u64, frames: usize, audio: &mut Vec<f32>, bars: &mut Vec<u32>) {
    audio.clear();
    bars.clear();
    for offset in 0..frames {
        let source_frame = first + offset as u64;
        let bar_frame = source_frame % BAR_FRAMES;
        if bar_frame == 0 {
            bars.push(offset as u32);
        }
        // Integer-period phases keep precision stable even at large start frames.
        let time = bar_frame as f64 / f64::from(RATE);
        let beat_time = (source_frame % BEAT_FRAMES) as f64 / f64::from(RATE);
        let pulse = if beat_time < 0.080 {
            let attack_decay = if beat_time < 0.003 {
                (FRAC_PI_2 * beat_time / 0.003).sin().powi(2)
            } else {
                (-(beat_time - 0.003) / 0.018).exp()
            };
            attack_decay * ((0.080 - beat_time) / 0.005).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let accent = bar_frame < BEAT_FRAMES;
        let left = 0.012 * (TAU * 220.0 * time).sin()
            + 0.004 * (TAU * 440.0 * time).sin()
            + if accent { 0.064 } else { 0.034 } * pulse * (TAU * 1320.0 * time).sin();
        let right = 0.013 * (TAU * 330.0 * time).sin()
            + 0.005 * (TAU * 660.0 * time).sin()
            + if accent { 0.062 } else { 0.032 } * pulse * (TAU * 1760.0 * time).sin();
        audio.extend_from_slice(&[left as f32, right as f32]);
    }
}

fn frame_duration(frames: u64) -> Result<Duration, Box<dyn Error>> {
    let nanos = u128::from(frames) * 1_000_000_000 / u128::from(RATE);
    Ok(Duration::from_nanos(u64::try_from(nanos)?))
}

fn describe(options: &Options) {
    let info = SenderInfo { id: String::new(), name: NAME.into(), sample_rate: RATE, channels: CHANNELS };
    let first_bar = options.start_frame.checked_add((BAR_FRAMES - options.start_frame % BAR_FRAMES) % BAR_FRAMES);
    let first_bar = first_bar.map_or_else(|| "null".into(), |frame| frame.to_string());
    println!("{{\"event\":\"layout\",\"name\":\"{}\",\"sample_rate_hz\":{},\"channels\":{},\"encoding\":\"interleaved f32 little-endian on wire\",\"block_frames\":{},\"block_samples\":{},\"block_ms\":10,\"bar_interval_frames\":{},\"bar_interval_seconds\":2,\"beat_interval_frames\":{},\"peak_upper_bound\":0.08,\"start_frame\":{},\"first_bar_source_frame\":{},\"duration_seconds\":{}}}",
        info.name, info.sample_rate, info.channels, BLOCK_FRAMES, BLOCK_FRAMES * CHANNELS,
        BAR_FRAMES, BEAT_FRAMES, options.start_frame, first_bar, options.duration_seconds);
}

fn offline_check(options: &Options) -> Result<(), Box<dyn Error>> {
    let frames = (options.duration_seconds * u64::from(RATE)).min(8 * u64::from(RATE));
    let mut capture = Capture::new(RATE, CHANNELS, DurationChoice::Seconds20)?;
    let mut audio = Vec::with_capacity(BLOCK_FRAMES * CHANNELS);
    let mut bars = Vec::with_capacity(1);
    let mut peak = 0.0_f32;
    let mut sum_squares = 0.0;
    let mut minimum_block_rms = f64::INFINITY;
    let mut expected_markers = Vec::new();
    for sent in (0..frames).step_by(BLOCK_FRAMES) {
        let count = (frames - sent).min(BLOCK_FRAMES as u64) as usize;
        fill_block(options.start_frame + sent, count, &mut audio, &mut bars);
        let squares = audio.iter().map(|&sample| f64::from(sample).powi(2)).sum::<f64>();
        minimum_block_rms = minimum_block_rms.min((squares / audio.len() as f64).sqrt());
        sum_squares += squares;
        peak = audio.iter().fold(peak, |current, sample| current.max(sample.abs()));
        if audio.iter().any(|sample| !sample.is_finite()) {
            return Err("fixture contains non-finite samples".into());
        }
        let offsets: Vec<_> = bars.iter().map(|&offset| offset as usize).collect();
        capture.push_interleaved_with_markers(&audio, &offsets)?;
    }
    for retained_frame in 0..frames {
        if (options.start_frame + retained_frame) % BAR_FRAMES == 0 {
            expected_markers.push(retained_frame);
        }
    }
    let markers = capture.bar_markers(capture.frame_range());
    if capture.frame_range() != (0..frames) || markers != expected_markers
        || peak > 0.080_001 || minimum_block_rms <= 0.005 {
        return Err("offline capture, marker, or amplitude check failed".into());
    }
    let rms = (sum_squares / (frames * CHANNELS as u64) as f64).sqrt();
    println!("{{\"event\":\"offline_check\",\"passed\":true,\"endpoint_created\":false,\"frames\":{frames},\"peak_linear\":{peak},\"rms_linear\":{rms},\"minimum_10ms_rms\":{minimum_block_rms},\"retained_bar_frames\":{markers:?}}}");
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let Some(options) = Options::parse()? else { return Ok(()) };
    describe(&options);
    if options.describe {
        return Ok(());
    }
    if options.check {
        return offline_check(&options);
    }
    if options.stopped() {
        println!("{{\"event\":\"stopped_before_registration\"}}");
        return Ok(());
    }

    // No endpoint is created until after every non-streaming early return.
    let mut endpoint = SenderEndpoint::new()?;
    endpoint.update(NAME, true, RATE, CHANNELS);
    let clock_anchor = capture_clock_ns();
    let wall_anchor = Instant::now();
    let total_frames = options.duration_seconds * u64::from(RATE);
    let mut sent = 0_u64;
    let mut maximum_lateness = Duration::ZERO;
    let mut late_blocks = 0_u64;
    let mut audio = Vec::with_capacity(BLOCK_FRAMES * CHANNELS);
    let mut bars = Vec::with_capacity(1);
    println!("{{\"event\":\"stream_started\",\"sender_id\":\"{}\",\"clock_anchor_ns\":{clock_anchor}}}", endpoint.id());
    while sent < total_frames && !options.stopped() {
        // Absolute deadlines avoid accumulating synthesis/serialization/sleep cost.
        // Late wakeups catch up without skipping source frames or bar markers.
        let relative = frame_duration(sent)?;
        let deadline = wall_anchor.checked_add(relative).ok_or("wall-clock deadline overflow")?;
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        if options.stopped() { break; }
        let lateness = Instant::now().saturating_duration_since(deadline);
        maximum_lateness = maximum_lateness.max(lateness);
        late_blocks += u64::from(lateness >= Duration::from_millis(10));
        let frames = (total_frames - sent).min(BLOCK_FRAMES as u64) as usize;
        fill_block(options.start_frame + sent, frames, &mut audio, &mut bars);
        let timestamp = clock_anchor.checked_add(u64::try_from(relative.as_nanos())?).ok_or("timestamp overflow")?;
        endpoint.publish(NAME, RATE, CHANNELS, timestamp, &audio, &bars)?;
        sent += frames as u64;
    }
    if sent == total_frames {
        let end = wall_anchor.checked_add(frame_duration(sent)?).ok_or("wall-clock deadline overflow")?;
        if let Some(wait) = end.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    println!("{{\"event\":\"stream_stopped\",\"frames_sent\":{sent},\"source_end_frame_exclusive\":{},\"wall_seconds\":{},\"late_blocks_at_least_10ms\":{late_blocks},\"maximum_lateness_ms\":{},\"stopped_by_marker\":{}}}",
        options.start_frame + sent, wall_anchor.elapsed().as_secs_f64(), maximum_lateness.as_secs_f64() * 1000.0, options.stopped());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtracking_core::waveform::{
        snap_to_visible_markers, timeline_guides, DisplayPolicy,
    };

    #[test]
    fn non_aligned_source_start_places_bar_inside_its_packet() {
        let mut audio = Vec::new();
        let mut bars = Vec::new();
        fill_block(95_990, BLOCK_FRAMES, &mut audio, &mut bars);
        assert_eq!(audio.len(), 960);
        assert_eq!(bars, [10]);
    }

    #[test]
    fn fixture_survives_capture_and_offers_bar_and_seconds_snapping() {
        let mut capture = Capture::new(RATE, CHANNELS, DurationChoice::Seconds20).unwrap();
        let mut audio = Vec::new();
        let mut bars = Vec::new();
        let mut minimum_rms = f64::INFINITY;
        let mut peak = 0.0_f32;
        for offset in (0..384_000).step_by(BLOCK_FRAMES) {
            fill_block(24_000 + offset as u64, BLOCK_FRAMES, &mut audio, &mut bars);
            let rms = (audio.iter().map(|&sample| f64::from(sample).powi(2)).sum::<f64>()
                / audio.len() as f64).sqrt();
            minimum_rms = minimum_rms.min(rms);
            peak = audio.iter().fold(peak, |value, sample| value.max(sample.abs()));
            assert!(audio.iter().all(|sample| sample.is_finite()));
            let offsets: Vec<_> = bars.iter().map(|&offset| offset as usize).collect();
            capture.push_interleaved_with_markers(&audio, &offsets).unwrap();
        }
        assert_eq!(capture.frame_range(), 0..384_000);
        assert_eq!(capture.snapshot().frame_count(), 384_000);
        assert!(minimum_rms > 0.005, "minimum 10 ms RMS: {minimum_rms}");
        assert!((0.07..=0.080_001).contains(&peak), "peak: {peak}");
        let bars = capture.bar_markers(capture.frame_range());
        assert_eq!(bars, [72_000, 168_000, 264_000, 360_000]);
        let visible = capture.frame_range();
        let policy = DisplayPolicy::for_view(960.0, 100.0, 384_000, RATE, true);
        let guides = timeline_guides(visible.clone(), RATE, 1, &bars, true, true, true, &policy);
        assert_eq!(guides.bars, bars);
        assert_eq!(guides.seconds, [0, 48_000, 96_000, 144_000, 192_000, 240_000, 288_000, 336_000, 384_000]);
        assert_eq!(snap_to_visible_markers(73_000.0 / 384_000.0, visible.clone(), &[], &guides, 0.01), 72_000.0 / 384_000.0);
        assert_eq!(snap_to_visible_markers(49_000.0 / 384_000.0, visible.clone(), &[], &guides, 0.01), 48_000.0 / 384_000.0);
        assert_eq!(snap_to_visible_markers(73_000.0 / 384_000.0, visible, &[72_500], &guides, 0.01), 72_500.0 / 384_000.0);
    }
}

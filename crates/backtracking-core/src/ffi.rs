use std::ffi::{c_char, CStr, CString};
use std::path::Path;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    export_wav_with_profile, AudioQueue, Capture, CaptureState, DurationChoice, MeterReadings,
    Snapshot, WavExportProfile, WavSampleFormat, WaveformView,
};

struct Reader {
    capture: Capture,
    scratch: Vec<f32>,
}

struct BtCore {
    queue: Arc<AudioQueue>,
    reader: Mutex<Reader>,
    enabled: AtomicBool,
    channels: usize,
}

impl BtCore {
    fn drain(&self, reader: &mut Reader, ingest: bool, limit: usize) {
        for _ in 0..limit {
            let frames = self.queue.pop_interleaved(&mut reader.scratch);
            if frames == 0 {
                break;
            }
            if ingest {
                let _ = reader
                    .capture
                    .push_interleaved(&reader.scratch[..frames * self.channels]);
            }
        }
    }
}

#[no_mangle]
extern "C" fn bt_new(sample_rate: f64, channels: u32, history_secs: u32) -> *mut BtCore {
    let Some(history) = DurationChoice::from_seconds(history_secs) else {
        return ptr::null_mut();
    };
    if !(1.0..=384_000.0).contains(&sample_rate) || !(1..=2).contains(&channels) {
        return ptr::null_mut();
    }
    let sample_rate = sample_rate.round() as u32;
    let channels = channels as usize;
    let Ok(capture) = Capture::new(sample_rate, channels, history) else {
        return ptr::null_mut();
    };

    Box::into_raw(Box::new(BtCore {
        queue: AudioQueue::new(sample_rate as usize / 4 + 1, channels),
        reader: Mutex::new(Reader {
            capture,
            scratch: vec![0.0; 2048 * channels],
        }),
        enabled: AtomicBool::new(true),
        channels,
    }))
}

#[no_mangle]
unsafe extern "C" fn bt_free(core: *mut BtCore) {
    if !core.is_null() {
        drop(Box::from_raw(core));
    }
}

#[no_mangle]
unsafe extern "C" fn bt_push_interleaved(
    core: *const BtCore,
    data: *const f32,
    frames: u32,
) -> u32 {
    let Some(core) = core.as_ref() else { return 0 };
    if data.is_null() || !core.enabled.load(Ordering::Acquire) {
        return 0;
    }
    let samples = slice::from_raw_parts(data, frames as usize * core.channels);
    core.queue.push_interleaved(samples) as u32
}

#[no_mangle]
unsafe extern "C" fn bt_push_planar_stereo(
    core: *const BtCore,
    left: *const f32,
    right: *const f32,
    frames: u32,
) -> u32 {
    let Some(core) = core.as_ref() else { return 0 };
    if left.is_null()
        || right.is_null()
        || core.channels != 2
        || !core.enabled.load(Ordering::Acquire)
    {
        return 0;
    }
    let left = slice::from_raw_parts(left, frames as usize);
    let right = slice::from_raw_parts(right, frames as usize);
    core.queue.push_planar(left, Some(right)) as u32
}

#[no_mangle]
unsafe extern "C" fn bt_poll(core: *const BtCore) {
    if let Some(core) = core.as_ref() {
        if let Ok(mut reader) = core.reader.lock() {
            core.drain(&mut reader, core.enabled.load(Ordering::Acquire), 64);
        }
    }
}

#[no_mangle]
unsafe extern "C" fn bt_pause(core: *const BtCore) {
    if let Some(core) = core.as_ref() {
        if let Ok(mut reader) = core.reader.lock() {
            if core.enabled.swap(false, Ordering::AcqRel) {
                core.drain(&mut reader, true, 64);
                reader.capture.pause();
            }
        }
    }
}

#[no_mangle]
unsafe extern "C" fn bt_resume(core: *const BtCore) {
    if let Some(core) = core.as_ref() {
        if let Ok(mut reader) = core.reader.lock() {
            if !core.enabled.load(Ordering::Acquire) {
                core.drain(&mut reader, false, 64);
                reader.capture.resume();
                core.enabled.store(true, Ordering::Release);
            }
        }
    }
}

#[no_mangle]
unsafe extern "C" fn bt_clear(core: *const BtCore) {
    if let Some(core) = core.as_ref() {
        if let Ok(mut reader) = core.reader.lock() {
            core.queue.discard_pending();
            reader.capture.clear();
        }
    }
}

#[no_mangle]
unsafe extern "C" fn bt_meter_readings(core: *const BtCore) -> MeterReadings {
    let Some(core) = core.as_ref() else {
        return MeterReadings::default();
    };
    core.reader.lock().map_or_else(
        |_| MeterReadings::default(),
        |reader| reader.capture.meter_readings(),
    )
}

#[no_mangle]
unsafe extern "C" fn bt_set_history(core: *const BtCore, seconds: u32) -> i32 {
    let (Some(core), Some(retention)) = (core.as_ref(), DurationChoice::from_seconds(seconds))
    else {
        return 0;
    };
    let Ok(mut reader) = core.reader.lock() else {
        return 0;
    };
    reader.capture.set_retention(retention);
    1
}

#[no_mangle]
unsafe extern "C" fn bt_state(core: *const BtCore) -> u32 {
    let Some(core) = core.as_ref() else { return 2 };
    let Ok(reader) = core.reader.lock() else {
        return 2;
    };
    match reader.capture.state() {
        CaptureState::Waiting => 0,
        CaptureState::Recording => 1,
        CaptureState::Paused => 2,
    }
}

#[no_mangle]
unsafe extern "C" fn bt_revision(core: *const BtCore) -> u64 {
    let Some(core) = core.as_ref() else { return 0 };
    core.reader
        .lock()
        .map_or(0, |reader| reader.capture.revision())
}

#[no_mangle]
unsafe extern "C" fn bt_waveform(
    core: *const BtCore,
    minimum: *mut f32,
    maximum: *mut f32,
    bins: u32,
) -> u64 {
    let Some(core) = core.as_ref() else { return 0 };
    if minimum.is_null() || maximum.is_null() {
        return 0;
    }
    let Ok(reader) = core.reader.lock() else {
        return 0;
    };
    let minimum = slice::from_raw_parts_mut(minimum, bins as usize);
    let maximum = slice::from_raw_parts_mut(maximum, bins as usize);
    reader.capture.waveform(minimum, maximum) as u64
}

#[no_mangle]
unsafe extern "C" fn bt_waveform_view(
    core: *const BtCore,
    view: u32,
    minimum: *mut f32,
    maximum: *mut f32,
    bins: u32,
) -> u64 {
    let (Some(core), Some(view)) = (core.as_ref(), WaveformView::from_code(view)) else {
        return 0;
    };
    if minimum.is_null() || maximum.is_null() {
        return 0;
    }
    let Ok(reader) = core.reader.lock() else {
        return 0;
    };
    let minimum = slice::from_raw_parts_mut(minimum, bins as usize);
    let maximum = slice::from_raw_parts_mut(maximum, bins as usize);
    reader.capture.track_waveform(0, view, minimum, maximum) as u64
}

#[no_mangle]
unsafe extern "C" fn bt_first_frame(core: *const BtCore) -> u64 {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .map_or(0, |reader| reader.capture.frame_range().start)
}

#[no_mangle]
unsafe extern "C" fn bt_end_frame(core: *const BtCore) -> u64 {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .map_or(0, |reader| reader.capture.frame_range().end)
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot(core: *const BtCore) -> *mut Snapshot {
    let Some(core) = core.as_ref() else {
        return ptr::null_mut();
    };
    let Ok(reader) = core.reader.lock() else {
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(reader.capture.snapshot()))
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot_free(snapshot: *mut Snapshot) {
    if !snapshot.is_null() {
        drop(Box::from_raw(snapshot));
    }
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot_export(
    snapshot: *const Snapshot,
    start: f64,
    end: f64,
    directory: *const c_char,
) -> *mut c_char {
    export_snapshot(snapshot, start, end, directory, WavExportProfile::default())
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot_export_with_profile(
    snapshot: *const Snapshot,
    start: f64,
    end: f64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
) -> *mut c_char {
    let Some(sample_format) = WavSampleFormat::from_bits_per_sample(bits_per_sample) else {
        return ptr::null_mut();
    };
    export_snapshot(
        snapshot,
        start,
        end,
        directory,
        WavExportProfile {
            sample_rate,
            sample_format,
        },
    )
}

unsafe fn export_snapshot(
    snapshot: *const Snapshot,
    start: f64,
    end: f64,
    directory: *const c_char,
    profile: WavExportProfile,
) -> *mut c_char {
    let Some(snapshot) = snapshot.as_ref() else {
        return ptr::null_mut();
    };
    if directory.is_null()
        || !(0.0..=1.0).contains(&start)
        || !(0.0..=1.0).contains(&end)
        || start >= end
    {
        return ptr::null_mut();
    }
    let Ok(directory) = CStr::from_ptr(directory).to_str() else {
        return ptr::null_mut();
    };
    if directory.is_empty() {
        return ptr::null_mut();
    }
    let frames = snapshot.frame_count();
    let first = (start * frames as f64).floor() as usize;
    let last = (end * frames as f64).ceil() as usize;
    let Some(samples) = snapshot
        .frames(first..last)
        .filter(|samples| !samples.is_empty())
    else {
        return ptr::null_mut();
    };
    export_wav_with_profile(
        Path::new(directory),
        snapshot.sample_rate,
        snapshot.channels,
        samples,
        profile,
    )
    .ok()
    .and_then(|path| path.to_str().and_then(|path| CString::new(path).ok()))
    .map_or(ptr::null_mut(), CString::into_raw)
}

#[no_mangle]
unsafe extern "C" fn bt_string_free(string: *mut c_char) {
    if !string.is_null() {
        drop(CString::from_raw(string));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn sixty_second_history_retains_the_latest_frames() {
        let core = bt_new(1.0, 1, 60);
        assert!(!core.is_null());
        unsafe {
            for frame in 1..=63 {
                let sample = frame as f32 / 64.0;
                assert_eq!(bt_push_interleaved(core, &sample, 1), 1);
                bt_poll(core);
            }

            let snapshot = bt_snapshot(core);
            let expected: Vec<_> = (4..=63).map(|frame| frame as f32 / 64.0).collect();
            assert_eq!((*snapshot).samples, expected);
            bt_snapshot_free(snapshot);

            assert_eq!(bt_set_history(core, 30), 1);
            assert_eq!(bt_set_history(core, 60), 1);
            assert_eq!(bt_push_interleaved(core, &1.0, 1), 1);
            bt_poll(core);
            let snapshot = bt_snapshot(core);
            assert_eq!((*snapshot).frame_count(), 31);
            bt_snapshot_free(snapshot);
            bt_free(core);
        }
    }

    #[test]
    fn clear_discards_already_queued_audio_and_preserves_mode() {
        let core = bt_new(48_000.0, 1, 10);
        assert!(!core.is_null());
        unsafe {
            assert_eq!(bt_push_interleaved(core, [0.75].as_ptr(), 1), 1);
            bt_clear(core);
            bt_poll(core);
            assert_eq!(bt_state(core), 0);
            assert_eq!(bt_meter_readings(core), MeterReadings::default());
            assert_eq!(bt_push_interleaved(core, [0.5].as_ptr(), 1), 1);
            bt_poll(core);
            assert_eq!(bt_state(core), 1);
            assert!((bt_meter_readings(core).peak_dbfs + 6.0206).abs() < 0.0001);

            bt_clear(core);
            assert_eq!(bt_state(core), 0);
            assert_eq!(bt_meter_readings(core), MeterReadings::default());
            let empty = bt_snapshot(core);
            assert_eq!((*empty).frame_count(), 0);
            bt_snapshot_free(empty);
            bt_pause(core);
            bt_clear(core);
            assert_eq!(bt_state(core), 2);
            assert_eq!(bt_meter_readings(core), MeterReadings::default());
            bt_free(core);
        }
    }

    #[test]
    fn pause_freezes_the_snapshot_and_export_uses_the_selected_frames() {
        let core = bt_new(48_000.0, 1, 10);
        assert!(!core.is_null());
        let output = std::env::temp_dir().join(format!(
            "backtracking-ffi-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let directory = CString::new(output.to_str().unwrap()).unwrap();
        unsafe {
            let input = [0.1, 0.2, 0.3, 0.4];
            assert_eq!(bt_push_interleaved(core, input.as_ptr(), 4), 4);
            bt_pause(core);
            assert_eq!(bt_state(core), 2);
            assert_eq!(bt_push_interleaved(core, [0.8].as_ptr(), 1), 0);

            let frozen = bt_snapshot(core);
            assert_eq!((*frozen).samples, input);
            bt_resume(core);
            assert_eq!(bt_push_interleaved(core, [0.5].as_ptr(), 1), 1);
            bt_poll(core);

            let resumed = bt_snapshot(core);
            assert_eq!((*resumed).samples, [0.1, 0.2, 0.3, 0.4, 0.5]);
            let result = bt_snapshot_export(frozen, 0.25, 0.75, directory.as_ptr());
            assert!(!result.is_null());
            let bytes = fs::read(CStr::from_ptr(result).to_str().unwrap()).unwrap();
            assert_eq!(&bytes[0..4], b"RIFF");
            assert_eq!(&bytes[24..28], &48_000u32.to_le_bytes());
            assert_eq!(&bytes[58..62], &0.2f32.to_le_bytes());
            assert_eq!(&bytes[62..66], &0.3f32.to_le_bytes());
            assert_eq!(bytes.len(), 66);

            let pcm =
                bt_snapshot_export_with_profile(frozen, 0.25, 0.75, directory.as_ptr(), 96_000, 24);
            assert!(!pcm.is_null());
            let bytes = fs::read(CStr::from_ptr(pcm).to_str().unwrap()).unwrap();
            assert_eq!(&bytes[24..28], &96_000u32.to_le_bytes());
            assert_eq!(&bytes[34..36], &24u16.to_le_bytes());
            assert_eq!(&bytes[60..68], b"data\x0c\0\0\0");
            assert_eq!(bytes.len(), 80);
            assert!(bt_snapshot_export_with_profile(
                frozen,
                0.25,
                0.75,
                directory.as_ptr(),
                96_000,
                20
            )
            .is_null());

            bt_string_free(pcm);
            bt_string_free(result);
            bt_snapshot_free(resumed);
            bt_snapshot_free(frozen);
            bt_free(core);
        }
        fs::remove_dir_all(output).unwrap();
    }
}

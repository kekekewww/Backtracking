use std::ffi::{c_char, CStr, CString};
use std::path::Path;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::{export_wav, AudioQueue, Capture, CaptureState, DurationChoice, Snapshot};

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

fn duration(seconds: u32) -> Option<DurationChoice> {
    match seconds {
        10 => Some(DurationChoice::Seconds10),
        20 => Some(DurationChoice::Seconds20),
        30 => Some(DurationChoice::Seconds30),
        _ => None,
    }
}

#[no_mangle]
extern "C" fn bt_new(sample_rate: f64, channels: u32, history_secs: u32) -> *mut BtCore {
    let Some(history) = duration(history_secs) else {
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
unsafe extern "C" fn bt_set_history(core: *const BtCore, seconds: u32) -> i32 {
    let (Some(core), Some(retention)) = (core.as_ref(), duration(seconds)) else {
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
    export_wav(
        Path::new(directory),
        snapshot.sample_rate,
        snapshot.channels,
        samples,
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

            bt_string_free(result);
            bt_snapshot_free(resumed);
            bt_snapshot_free(frozen);
            bt_free(core);
        }
        fs::remove_dir_all(output).unwrap();
    }
}

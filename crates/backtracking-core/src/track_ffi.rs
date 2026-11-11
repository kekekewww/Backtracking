use std::ffi::{c_char, CStr, CString};
use std::ops::Range;
use std::path::Path;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::export::export_routed_track_refs;
use crate::{
    ChannelRoute, DurationChoice, MeterReadings, Snapshot, SynchronizedCapture, TimedAudioQueue,
    WavExportProfile, WavSampleFormat, WaveformView,
};

struct TrackReader {
    synchronized: SynchronizedCapture,
    scratch: Vec<f32>,
}

struct BtTracks {
    queues: [Option<Arc<TimedAudioQueue>>; 2],
    reader: Mutex<TrackReader>,
    enabled: AtomicBool,
}

impl BtTracks {
    fn queue(&self, lane: u32) -> Option<&TimedAudioQueue> {
        self.queues.get(lane as usize)?.as_deref()
    }

    fn drain(&self, reader: &mut TrackReader, ingest: bool, flush: bool) {
        for (lane, queue) in self.queues.iter().enumerate() {
            if let Some(queue) = queue {
                for _ in 0..queue.pending_packets() {
                    let Some(packet) = queue.pop(&mut reader.scratch) else {
                        break;
                    };
                    if ingest {
                        let _ = reader.synchronized.push(
                            lane,
                            packet.timestamp_ns,
                            packet.sample_rate,
                            &reader.scratch,
                        );
                    }
                }
            }
        }
        if ingest {
            reader.synchronized.drain(flush);
        }
    }
}

fn sample_rate(rate: f64) -> Option<u32> {
    (1.0..=384_000.0)
        .contains(&rate)
        .then(|| rate.round() as u32)
}

#[no_mangle]
extern "C" fn bt_tracks_new(
    rate: f64,
    a_channels: u32,
    b_channels: u32,
    history_secs: u32,
) -> *mut BtTracks {
    let (Some(rate), Some(history)) = (
        sample_rate(rate),
        DurationChoice::from_seconds(history_secs),
    ) else {
        return ptr::null_mut();
    };
    let b_channels = (b_channels != 0).then_some(b_channels as usize);
    let Ok(synchronized) = SynchronizedCapture::new(rate, a_channels as usize, b_channels, history)
    else {
        return ptr::null_mut();
    };
    // Keep enough capacity to tolerate a delayed UI poll at high source rates.
    let capacity = 96_000.max(rate as usize / 4);
    let queue = |channels| TimedAudioQueue::new(capacity, 512, channels);
    Box::into_raw(Box::new(BtTracks {
        queues: [Some(queue(a_channels as usize)), b_channels.map(queue)],
        reader: Mutex::new(TrackReader {
            synchronized,
            scratch: Vec::new(),
        }),
        enabled: AtomicBool::new(true),
    }))
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_free(core: *mut BtTracks) {
    if !core.is_null() {
        drop(Box::from_raw(core));
    }
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_push_interleaved(
    core: *const BtTracks,
    lane: u32,
    data: *const f32,
    frames: u32,
    timestamp_ns: u64,
    source_rate: f64,
) -> u32 {
    let Some(core) = core.as_ref() else { return 0 };
    let (Some(queue), Some(rate)) = (core.queue(lane), sample_rate(source_rate)) else {
        return 0;
    };
    if data.is_null() || !core.enabled.load(Ordering::Acquire) {
        return 0;
    }
    let Some(samples) = (frames as usize).checked_mul(queue.channels()) else {
        return 0;
    };
    queue.push_interleaved(timestamp_ns, rate, slice::from_raw_parts(data, samples)) as u32
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_push_planar_stereo(
    core: *const BtTracks,
    lane: u32,
    left: *const f32,
    right: *const f32,
    frames: u32,
    timestamp_ns: u64,
    source_rate: f64,
) -> u32 {
    let Some(core) = core.as_ref() else { return 0 };
    let (Some(queue), Some(rate)) = (core.queue(lane), sample_rate(source_rate)) else {
        return 0;
    };
    if left.is_null() || right.is_null() || !core.enabled.load(Ordering::Acquire) {
        return 0;
    }
    let left = slice::from_raw_parts(left, frames as usize);
    let right = slice::from_raw_parts(right, frames as usize);
    queue.push_planar(timestamp_ns, rate, left, Some(right)) as u32
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_poll(core: *const BtTracks) {
    if let Some(core) = core.as_ref() {
        if let Ok(mut reader) = core.reader.lock() {
            core.drain(&mut reader, core.enabled.load(Ordering::Acquire), false);
        }
    }
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_pause(core: *const BtTracks) {
    if let Some(core) = core.as_ref() {
        if let Ok(mut reader) = core.reader.lock() {
            if core.enabled.swap(false, Ordering::AcqRel) {
                core.drain(&mut reader, true, true);
                reader.synchronized.capture_mut().pause();
            }
        }
    }
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_resume(core: *const BtTracks) {
    if let Some(core) = core.as_ref() {
        if let Ok(mut reader) = core.reader.lock() {
            if !core.enabled.load(Ordering::Acquire) {
                core.drain(&mut reader, false, false);
                reader.synchronized.reset_input();
                reader.synchronized.capture_mut().resume();
                core.enabled.store(true, Ordering::Release);
            }
        }
    }
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_clear(core: *const BtTracks) {
    if let Some(core) = core.as_ref() {
        if let Ok(mut reader) = core.reader.lock() {
            core.drain(&mut reader, false, false);
            reader.synchronized.reset_input();
            reader.synchronized.capture_mut().clear();
        }
    }
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_state(core: *const BtTracks) -> u32 {
    use crate::CaptureState;
    let Some(core) = core.as_ref() else { return 2 };
    let Ok(reader) = core.reader.lock() else {
        return 2;
    };
    match reader.synchronized.capture().state() {
        CaptureState::Waiting => 0,
        CaptureState::Recording => 1,
        CaptureState::Paused => 2,
    }
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_revision(core: *const BtTracks) -> u64 {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .map_or(0, |reader| reader.synchronized.capture().revision())
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_set_history(core: *const BtTracks, seconds: u32) -> i32 {
    let (Some(core), Some(history)) = (core.as_ref(), DurationChoice::from_seconds(seconds)) else {
        return 0;
    };
    let Ok(mut reader) = core.reader.lock() else {
        return 0;
    };
    reader.synchronized.capture_mut().set_retention(history);
    1
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_meter_readings(core: *const BtTracks, lane: u32) -> MeterReadings {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .and_then(|reader| {
            reader
                .synchronized
                .capture()
                .track_meter_readings(lane as usize)
        })
        .unwrap_or_default()
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_waveform(
    core: *const BtTracks,
    lane: u32,
    view: u32,
    minimum: *mut f32,
    maximum: *mut f32,
    bins: u32,
) -> u64 {
    let Some(core) = core.as_ref() else { return 0 };
    let Some(view) = WaveformView::from_code(view) else {
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
    reader
        .synchronized
        .capture()
        .track_waveform(lane as usize, view, minimum, maximum) as u64
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_first_frame(core: *const BtTracks) -> u64 {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .map_or(0, |reader| {
            reader.synchronized.capture().frame_range().start
        })
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_end_frame(core: *const BtTracks) -> u64 {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .map_or(0, |reader| reader.synchronized.capture().frame_range().end)
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_channels(core: *const BtTracks, lane: u32) -> u32 {
    core.as_ref()
        .and_then(|core| core.queue(lane))
        .map_or(0, |queue| queue.channels() as u32)
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_snapshot(core: *const BtTracks, lane: u32) -> *mut Snapshot {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .and_then(|reader| reader.synchronized.capture().track_snapshot(lane as usize))
        .map_or(ptr::null_mut(), |snapshot| {
            Box::into_raw(Box::new(snapshot))
        })
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot_first_frame(snapshot: *const Snapshot) -> u64 {
    snapshot.as_ref().map_or(0, |snapshot| snapshot.first_frame)
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot_frame_count(snapshot: *const Snapshot) -> u64 {
    snapshot
        .as_ref()
        .map_or(0, |snapshot| snapshot.frame_count() as u64)
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot_channels(snapshot: *const Snapshot) -> u32 {
    snapshot
        .as_ref()
        .map_or(0, |snapshot| snapshot.channels as u32)
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot_waveform(
    snapshot: *const Snapshot,
    view: u32,
    minimum: *mut f32,
    maximum: *mut f32,
    bins: u32,
) -> u64 {
    let Some(snapshot) = snapshot.as_ref() else {
        return 0;
    };
    let Some(view) = WaveformView::from_code(view) else {
        return 0;
    };
    if minimum.is_null() || maximum.is_null() {
        return 0;
    }
    let minimum = slice::from_raw_parts_mut(minimum, bins as usize);
    let maximum = slice::from_raw_parts_mut(maximum, bins as usize);
    crate::waveform::view_peaks(&snapshot.samples, snapshot.channels, view, minimum, maximum);
    snapshot.frame_count() as u64
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot_export_frames_with_profile(
    snapshot: *const Snapshot,
    first_frame: u64,
    end_frame: u64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
) -> *mut c_char {
    bt_snapshot_export_routed_frames_with_profile(
        snapshot,
        first_frame,
        end_frame,
        directory,
        sample_rate,
        bits_per_sample,
        ChannelRoute::Merged as u32,
        0,
    )
}

#[no_mangle]
unsafe extern "C" fn bt_snapshot_export_routed_frames_with_profile(
    snapshot: *const Snapshot,
    first_frame: u64,
    end_frame: u64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    route: u32,
    force_mono: u32,
) -> *mut c_char {
    let Some(snapshot) = snapshot.as_ref() else {
        return ptr::null_mut();
    };
    let Some(frames) = relative_frames(snapshot, first_frame, end_frame) else {
        return ptr::null_mut();
    };
    let Some(route) = ChannelRoute::from_code(route) else {
        return ptr::null_mut();
    };
    let Some(mut paths) = export_snapshots(
        &[snapshot],
        frames,
        directory,
        sample_rate,
        bits_per_sample,
        &[route],
        force_mono != 0,
    ) else {
        return ptr::null_mut();
    };
    paths.remove(0).into_raw()
}

#[no_mangle]
unsafe extern "C" fn bt_snapshots_export_pair_frames_with_profile(
    snapshot_a: *const Snapshot,
    snapshot_b: *const Snapshot,
    first_frame: u64,
    end_frame: u64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    path_a: *mut *mut c_char,
    path_b: *mut *mut c_char,
) -> i32 {
    bt_snapshots_export_pair_routed_frames_with_profile(
        snapshot_a,
        snapshot_b,
        first_frame,
        end_frame,
        directory,
        sample_rate,
        bits_per_sample,
        ChannelRoute::Merged as u32,
        ChannelRoute::Merged as u32,
        0,
        path_a,
        path_b,
    )
}

#[no_mangle]
unsafe extern "C" fn bt_snapshots_export_pair_routed_frames_with_profile(
    snapshot_a: *const Snapshot,
    snapshot_b: *const Snapshot,
    first_frame: u64,
    end_frame: u64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    route_a: u32,
    route_b: u32,
    force_mono: u32,
    path_a: *mut *mut c_char,
    path_b: *mut *mut c_char,
) -> i32 {
    let Some(frames) = snapshot_a
        .as_ref()
        .and_then(|snapshot| relative_frames(snapshot, first_frame, end_frame))
    else {
        return 0;
    };
    let (Some(route_a), Some(route_b)) = (
        ChannelRoute::from_code(route_a),
        ChannelRoute::from_code(route_b),
    ) else {
        return 0;
    };
    export_pair(
        [snapshot_a, snapshot_b],
        frames,
        directory,
        [sample_rate, bits_per_sample],
        [route_a, route_b],
        force_mono != 0,
        [path_a, path_b],
    )
}

#[no_mangle]
unsafe extern "C" fn bt_snapshots_export_pair_with_profile(
    snapshot_a: *const Snapshot,
    snapshot_b: *const Snapshot,
    start: f64,
    end: f64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    path_a: *mut *mut c_char,
    path_b: *mut *mut c_char,
) -> i32 {
    let Some(snapshot) = snapshot_a.as_ref() else {
        return 0;
    };
    if !(0.0..=1.0).contains(&start) || !(0.0..=1.0).contains(&end) || start >= end {
        return 0;
    }
    let count = snapshot.frame_count() as f64;
    let frames = (start * count).floor() as usize..(end * count).ceil() as usize;
    export_pair(
        [snapshot_a, snapshot_b],
        frames,
        directory,
        [sample_rate, bits_per_sample],
        [ChannelRoute::Merged; 2],
        false,
        [path_a, path_b],
    )
}

fn relative_frames(snapshot: &Snapshot, first: u64, end: u64) -> Option<Range<usize>> {
    if first >= end || first < snapshot.first_frame || end > snapshot.frame_range().end {
        return None;
    }
    Some((first - snapshot.first_frame) as usize..(end - snapshot.first_frame) as usize)
}

unsafe fn export_pair(
    [snapshot_a, snapshot_b]: [*const Snapshot; 2],
    frames: Range<usize>,
    directory: *const c_char,
    [sample_rate, bits_per_sample]: [u32; 2],
    routes: [ChannelRoute; 2],
    force_mono: bool,
    [path_a, path_b]: [*mut *mut c_char; 2],
) -> i32 {
    if path_a.is_null() || path_b.is_null() || path_a == path_b {
        return 0;
    }
    *path_a = ptr::null_mut();
    *path_b = ptr::null_mut();
    let (Some(a), Some(b)) = (snapshot_a.as_ref(), snapshot_b.as_ref()) else {
        return 0;
    };
    let Some(mut paths) = export_snapshots(
        &[a, b],
        frames,
        directory,
        sample_rate,
        bits_per_sample,
        &routes,
        force_mono,
    ) else {
        return 0;
    };
    *path_a = paths.remove(0).into_raw();
    *path_b = paths.remove(0).into_raw();
    1
}

unsafe fn export_snapshots(
    snapshots: &[&Snapshot],
    frames: Range<usize>,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    routes: &[ChannelRoute],
    force_mono: bool,
) -> Option<Vec<CString>> {
    if directory.is_null() {
        return None;
    }
    let directory = CStr::from_ptr(directory).to_str().ok()?;
    if directory.is_empty() {
        return None;
    }
    let sample_format = WavSampleFormat::from_bits_per_sample(bits_per_sample)?;
    let profile = WavExportProfile {
        sample_rate,
        sample_format,
    };
    let paths = export_routed_track_refs(
        Path::new(directory),
        snapshots,
        frames,
        profile,
        routes,
        force_mono,
    )
    .ok()?;
    let result: Option<Vec<CString>> = paths
        .iter()
        .map(|path| CString::new(path.to_str()?).ok())
        .collect();
    if result.is_none() {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    }
    result
}

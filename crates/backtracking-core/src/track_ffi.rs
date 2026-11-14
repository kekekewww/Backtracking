use std::ffi::{c_char, CStr, CString};
use std::ops::Range;
use std::path::Path;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::export::export_routed_track_refs;
use crate::{
    discover_senders, discover_snapshot_targets, request_sender_snapshot, ChannelRoute,
    DurationChoice, MeterReadings, SenderInfo, SenderReceiver, Snapshot, SynchronizedCapture,
    TimedAudioQueue, WavExportProfile, WavSampleFormat, WaveformView,
};

struct TrackSender {
    id: String,
    receiver: Option<SenderReceiver>,
    route: ChannelRoute,
    retry_at: Instant,
}

impl TrackSender {
    fn receiver(&mut self, channels: usize) -> Option<&mut SenderReceiver> {
        if self.receiver.is_none() && Instant::now() >= self.retry_at {
            self.retry_at = Instant::now() + Duration::from_secs(2);
            self.receiver = SenderReceiver::connect(&self.id).ok().filter(|receiver| {
                self.route.output_channels(receiver.info.channels, None) == Some(channels)
            });
        }
        self.receiver.as_mut()
    }
}

struct TrackReader {
    rate: u32,
    senders: [Option<TrackSender>; 2],
    sender_audio: Vec<f32>,
    sender_bars: Vec<usize>,
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
        // Alternate lanes so bursts cannot advance one clock ahead of queued input from the other.
        let TrackReader { synchronized, senders, sender_audio, sender_bars, scratch, .. } = reader;
        for _ in 0..512 {
            let mut progressed = false;
            for (lane, queue) in self.queues.iter().enumerate() {
                if let Some(sender) = &mut senders[lane] {
                    let route = sender.route;
                    let Some(receiver) = queue.as_ref().and_then(|queue| sender.receiver(queue.channels())) else {
                        continue;
                    };
                    match receiver.packet(sender_audio, sender_bars) {
                        Ok(Some(packet)) => {
                            progressed = true;
                            if ingest && route.route_interleaved(
                                sender_audio, receiver.info.channels, None, scratch,
                            ).is_ok() {
                                let _ = synchronized.push_with_markers(
                                    lane, packet.timestamp_ns, packet.sample_rate, scratch, false, sender_bars,
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {
                            sender.receiver = None;
                            sender.retry_at = Instant::now() + Duration::from_secs(2);
                        }
                    }
                } else if let Some(packet) = queue.as_ref().and_then(|queue| queue.pop(scratch)) {
                    progressed = true;
                    if ingest {
                        let _ = synchronized.push_with_discontinuity(
                            lane, packet.timestamp_ns, packet.sample_rate, scratch, packet.discontinuity,
                        );
                    }
                }
            }
            if !progressed { break; }
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
            rate,
            synchronized,
            senders: [None, None],
            sender_audio: Vec::new(),
            sender_bars: Vec::new(),
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
unsafe extern "C" fn bt_tracks_set_retain_silence(core: *const BtTracks, enabled: u32) {
    let Some(core) = core.as_ref() else { return };
    let Ok(mut reader) = core.reader.lock() else { return };
    if reader.synchronized.retain_silence() == (enabled != 0) { return; }
    core.drain(&mut reader, core.enabled.load(Ordering::Acquire), true);
    reader.synchronized.set_retain_silence(enabled != 0);
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
unsafe extern "C" fn bt_tracks_sample_rate(core: *const BtTracks) -> u32 {
    core.as_ref().and_then(|core| core.reader.lock().ok()).map_or(0, |reader| reader.rate)
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_waveform_range(
    core: *const BtTracks, lane: u32, view: u32, first: u64, end: u64,
    minimum: *mut f32, maximum: *mut f32, bins: u32,
) -> u64 {
    let (Some(core), Some(view)) = (core.as_ref(), WaveformView::from_code(view)) else { return 0 };
    if minimum.is_null() || maximum.is_null() { return 0; }
    let Ok(reader) = core.reader.lock() else { return 0 };
    let minimum = slice::from_raw_parts_mut(minimum, bins as usize);
    let maximum = slice::from_raw_parts_mut(maximum, bins as usize);
    reader.synchronized.capture().track_waveform_range(lane as usize, view, first..end, minimum, maximum) as u64
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_copy_bar_markers(
    core: *const BtTracks, first: u64, end: u64, markers: *mut u64, capacity: usize,
) -> usize {
    let Some(core) = core.as_ref() else { return 0 };
    let Ok(reader) = core.reader.lock() else { return 0 };
    let bars = reader.synchronized.capture().bar_markers(first..end);
    if !markers.is_null() {
        ptr::copy_nonoverlapping(bars.as_ptr(), markers, bars.len().min(capacity));
    }
    bars.len()
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_attach_sender(
    core: *const BtTracks, lane: u32, id: *const c_char, route: u32,
) -> i32 {
    let (Some(core), Some(route)) = (core.as_ref(), ChannelRoute::from_code(route)) else { return 0 };
    if id.is_null() { return 0; }
    let Ok(id) = CStr::from_ptr(id).to_str() else { return 0 };
    let Some(queue) = core.queue(lane) else { return 0 };
    let Ok(receiver) = SenderReceiver::connect(id) else { return 0 };
    if route.output_channels(receiver.info.channels, None) != Some(queue.channels()) { return 0; }
    let Ok(mut reader) = core.reader.lock() else { return 0 };
    reader.senders[lane as usize] = Some(TrackSender {
        id: id.to_owned(),
        receiver: Some(receiver),
        route,
        retry_at: Instant::now(),
    });
    1
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_detach_sender(core: *const BtTracks, lane: u32) {
    if let Some(mut reader) = core.as_ref().and_then(|core| core.reader.lock().ok()) {
        if let Some(slot) = reader.senders.get_mut(lane as usize) { *slot = None; }
    }
}

#[no_mangle]
extern "C" fn bt_senders_new(audio_only: i32) -> *mut Vec<SenderInfo> {
    let senders = if audio_only != 0 { discover_senders() } else { discover_snapshot_targets() };
    Box::into_raw(Box::new(senders))
}

#[no_mangle]
unsafe extern "C" fn bt_senders_free(senders: *mut Vec<SenderInfo>) {
    if !senders.is_null() { drop(Box::from_raw(senders)); }
}

#[no_mangle]
unsafe extern "C" fn bt_senders_count(senders: *const Vec<SenderInfo>) -> usize {
    senders.as_ref().map_or(0, Vec::len)
}

#[no_mangle]
unsafe extern "C" fn bt_senders_string(senders: *const Vec<SenderInfo>, index: usize, name: i32) -> *mut c_char {
    let Some(sender) = senders.as_ref().and_then(|entries| entries.get(index)) else { return ptr::null_mut() };
    let value = if name != 0 { &sender.name } else { &sender.id };
    CString::new(value.as_bytes()).map_or(ptr::null_mut(), CString::into_raw)
}

#[no_mangle]
unsafe extern "C" fn bt_senders_format(senders: *const Vec<SenderInfo>, index: usize, channels: *mut u32) -> u32 {
    let Some(sender) = senders.as_ref().and_then(|entries| entries.get(index)) else { return 0 };
    if let Some(channels) = channels.as_mut() { *channels = sender.channels as u32; }
    sender.sample_rate
}

#[no_mangle]
unsafe extern "C" fn bt_sender_request_snapshot(id: *const c_char) -> i32 {
    if id.is_null() { return 0; }
    i32::from(CStr::from_ptr(id).to_str().is_ok_and(|id| request_sender_snapshot(id).is_ok()))
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
unsafe extern "C" fn bt_tracks_snapshot_range(
    core: *const BtTracks, lane: u32, first: u64, end: u64,
) -> *mut Snapshot {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .and_then(|reader| reader.synchronized.capture().track_snapshot_range(lane as usize, first..end))
        .map_or(ptr::null_mut(), |snapshot| Box::into_raw(Box::new(snapshot)))
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
unsafe extern "C" fn bt_snapshot_waveform_range(
    snapshot: *const Snapshot, view: u32, first: u64, end: u64,
    minimum: *mut f32, maximum: *mut f32, bins: u32,
) -> u64 {
    let (Some(snapshot), Some(view)) = (snapshot.as_ref(), WaveformView::from_code(view)) else { return 0 };
    if minimum.is_null() || maximum.is_null() { return 0; }
    let Some(frames) = relative_frames(snapshot, first, end) else { return 0 };
    let Some(samples) = snapshot.frames(frames) else { return 0 };
    crate::waveform::view_peaks(
        samples, snapshot.channels, view,
        slice::from_raw_parts_mut(minimum, bins as usize), slice::from_raw_parts_mut(maximum, bins as usize),
    );
    end - first
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

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn tracks_silence_mode_ffi_flushes_old_policy_and_keeps_aligned_quiet_samples() {
        unsafe {
            bt_tracks_set_retain_silence(ptr::null(), 1);
            let core = bt_tracks_new(1_000., 1, 1, 10);
            assert!(!core.is_null());
            let a = [0.000_1; 100];
            let b = [-0.000_2; 100];
            assert_eq!(bt_tracks_push_interleaved(core, 0, a.as_ptr(), 100, 0, 1_000.), 100);
            assert_eq!(bt_tracks_push_interleaved(core, 1, b.as_ptr(), 100, 0, 1_000.), 100);
            bt_tracks_set_retain_silence(core, 1);
            assert_eq!(bt_tracks_end_frame(core), 0);
            assert_eq!(bt_tracks_push_interleaved(core, 0, a.as_ptr(), 100, 10_000_000_000, 1_000.), 100);
            assert_eq!(bt_tracks_push_interleaved(core, 1, b.as_ptr(), 100, 10_000_000_000, 1_000.), 100);
            bt_tracks_set_retain_silence(core, 0);
            let first = bt_tracks_snapshot(core, 0);
            let second = bt_tracks_snapshot(core, 1);
            assert_eq!((*first).samples, a);
            assert_eq!((*second).samples, b);
            assert_eq!((*first).frame_range(), (*second).frame_range());
            drop(Box::from_raw(first));
            drop(Box::from_raw(second));
            assert_eq!(bt_tracks_state(core), 0);
            assert_eq!(bt_tracks_push_interleaved(core, 0, a.as_ptr(), 100, 10_100_000_000, 1_000.), 100);
            assert_eq!(bt_tracks_push_interleaved(core, 1, b.as_ptr(), 100, 10_100_000_000, 1_000.), 100);
            bt_tracks_set_retain_silence(core, 1);
            assert_eq!(bt_tracks_end_frame(core), 100);
            bt_tracks_pause(core);
            bt_tracks_set_retain_silence(core, 0);
            assert_eq!(bt_tracks_state(core), 2);
            assert_eq!(bt_tracks_end_frame(core), 100);
            bt_tracks_free(core);
        }
    }

    #[test]
    fn setting_the_same_silence_mode_does_not_force_flush_a_late_lane() {
        unsafe {
            let core = bt_tracks_new(1_000., 1, 1, 10);
            bt_tracks_set_retain_silence(core, 1);
            let a = [0.000_1; 10];
            let b = [-0.000_2; 10];
            assert_eq!(bt_tracks_push_interleaved(core, 0, a.as_ptr(), 10, 0, 1_000.), 10);
            bt_tracks_poll(core);
            assert_eq!(bt_tracks_end_frame(core), 0);
            bt_tracks_set_retain_silence(core, 1);
            assert_eq!(bt_tracks_end_frame(core), 0);
            assert_eq!(bt_tracks_push_interleaved(core, 1, b.as_ptr(), 10, 0, 1_000.), 10);
            bt_tracks_pause(core);
            let first = bt_tracks_snapshot(core, 0);
            let second = bt_tracks_snapshot(core, 1);
            assert_eq!((*first).samples, a);
            assert_eq!((*second).samples, b);
            drop(Box::from_raw(first));
            drop(Box::from_raw(second));
            bt_tracks_free(core);
        }
    }

    #[test]
    fn frozen_both_lanes_remain_renderable_while_capture_wraps_and_shrinks() {
        let mut capture = crate::Capture::new_tracks(100, 1, Some(1), DurationChoice::Minutes10).unwrap();
        capture.push_interleaved(&[0.2, -0.3].repeat(2_000)).unwrap();
        let material = [capture.track_snapshot_range(0, 1_100..1_500).unwrap(),
            capture.track_snapshot_range(1, 1_100..1_500).unwrap()];
        capture.set_retention(DurationChoice::Seconds10);
        capture.push_interleaved(&[0.75, 0.65].repeat(3_000)).unwrap();
        assert!(capture.frame_range().start > material[0].frame_range().end);
        assert_eq!(material[0].frame_range(), material[1].frame_range());
        for (snapshot, expected) in material.iter().zip([0.2, -0.3]) {
            let mut low = [0.0; 4];
            let mut high = [0.0; 4];
            unsafe {
                assert_eq!(bt_snapshot_waveform_range(snapshot, 0, 1_200, 1_204,
                    low.as_mut_ptr(), high.as_mut_ptr(), 4), 4);
            }
            assert_eq!(low, [expected; 4]);
            assert_eq!(high, [expected; 4]);
        }
    }

    #[test]
    fn frozen_waveform_range_keeps_absolute_frames_and_channel_layout() {
        let snapshot = Snapshot {
            sample_rate: 100,
            channels: 2,
            first_frame: 100,
            samples: vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4],
        };
        let mut low = [0.0; 2];
        let mut high = [0.0; 2];
        unsafe {
            assert_eq!(bt_snapshot_waveform_range(
                &snapshot, 1, 102, 104, low.as_mut_ptr(), high.as_mut_ptr(), 2,
            ), 2);
            assert_eq!(low, [0.3, 0.4]);
            assert_eq!(high, [0.3, 0.4]);
            assert_eq!(bt_snapshot_waveform_range(
                &snapshot, 2, 102, 104, low.as_mut_ptr(), high.as_mut_ptr(), 2,
            ), 2);
            assert_eq!(low, [-0.3, -0.4]);
            assert_eq!(high, [-0.3, -0.4]);
            assert_eq!(bt_snapshot_waveform_range(
                &snapshot, 0, 99, 104, low.as_mut_ptr(), high.as_mut_ptr(), 2,
            ), 0);
        }
    }
}

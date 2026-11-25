use std::ffi::{c_char, CStr, CString};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::export::{export_routed_track_refs, TrackAudio};
use crate::preview::PreviewLane;
use crate::waveform::{PairGroup, PairSide, SpectrumRow, SPECTRUM_BANDS};
use crate::wide_export::{export_wide_track, WideExportGroup, WideExportMode, WideRowExport};
use crate::{
    cleanup, discover_senders, discover_snapshot_targets, request_sender_snapshot, ChannelRoute,
    DurationChoice, Fades, FrozenCapture, Lane, MeterReadings, Preview, PreviewGate, PreviewSlot,
    SenderInfo, SenderReceiver, Snapshot,
    SynchronizedCapture, TakeLabel, TimedAudioQueue, WavExportProfile, WavSampleFormat,
    WaveformView, DEFAULT_MAX_BUFFER_SAMPLES, MAX_LANES,
};

struct TrackSender {
    id: String,
    receiver: Option<SenderReceiver>,
    route: ChannelRoute,
    weights: Vec<[f32; 2]>,
    retry_at: Instant,
}

impl TrackSender {
    fn connect(&mut self, channels: usize) {
        if self.receiver.is_some() || Instant::now() < self.retry_at {
            return;
        }
        self.retry_at = Instant::now() + Duration::from_secs(2);
        if let Ok(receiver) = SenderReceiver::connect(&self.id) {
            self.bind(receiver, channels);
        }
    }

    fn bind(&mut self, receiver: SenderReceiver, channels: usize) -> bool {
        let width = receiver.info.channels;
        if self.route.output_channels(width, None) != Some(channels) {
            return false;
        }
        self.weights = if self.route == ChannelRoute::Merged && width > 2 {
            let labels = receiver.labels().map_or_else(|| vec![""; width], |labels| {
                labels.iter().map(String::as_str).collect()
            });
            crate::spatial::stereo_weights(&labels)
        } else {
            Vec::new()
        };
        self.receiver = Some(receiver);
        true
    }

}

struct TrackReader {
    rate: u32,
    senders: Vec<Option<TrackSender>>,
    sender_audio: Vec<f32>,
    sender_bars: Vec<usize>,
    synchronized: SynchronizedCapture,
    scratch: Vec<f32>,
}

struct BtTracks {
    queues: Vec<Arc<TimedAudioQueue>>,
    reader: Mutex<TrackReader>,
    enabled: AtomicBool,
    /// Lanes that record System playback, where audio captured while a Backtracking preview may
    /// have been heard is silenced, so the preview never enters the history.
    preview_silence: Vec<AtomicBool>,
    gate: &'static PreviewGate,
}

impl BtTracks {
    fn queue(&self, lane: u32) -> Option<&TimedAudioQueue> {
        self.queues.get(lane as usize).map(Arc::as_ref)
    }

    fn drain(&self, reader: &mut TrackReader, ingest: bool, flush: bool) {
        // Alternate lanes so bursts cannot advance one clock ahead of queued input from the other.
        let TrackReader { synchronized, senders, sender_audio, sender_bars, scratch, .. } = reader;
        for _ in 0..512 {
            let mut progressed = false;
            for (lane, queue) in self.queues.iter().enumerate() {
                if let Some(sender) = &mut senders[lane] {
                    sender.connect(queue.channels());
                    let Some(receiver) = &mut sender.receiver else { continue };
                    match receiver.packet(sender_audio, sender_bars) {
                        Ok(Some(packet)) => {
                            progressed = true;
                            if ingest {
                                let routed = if sender.weights.is_empty() {
                                    sender.route.route_interleaved(sender_audio, receiver.info.channels, None, scratch).is_ok()
                                } else {
                                    scratch.resize(packet.frames * 2, 0.0);
                                    crate::spatial::fold_down(sender_audio, &sender.weights, scratch).is_ok()
                                };
                                if routed {
                                    let _ = synchronized.push_with_markers(
                                        lane, packet.timestamp_ns, packet.sample_rate, scratch, false, sender_bars,
                                    );
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {
                            sender.receiver = None;
                            sender.retry_at = Instant::now() + Duration::from_secs(2);
                        }
                    }
                } else if let Some(packet) = queue.pop(scratch) {
                    progressed = true;
                    if ingest {
                        if self.preview_silence[lane].load(Ordering::Relaxed) {
                            self.gate.silence(
                                packet.timestamp_ns, scratch, queue.channels(), packet.sample_rate,
                            );
                        }
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
    let widths = [a_channels, b_channels];
    new_tracks(rate, &widths[..if b_channels == 0 { 1 } else { 2 }], history_secs,
        DEFAULT_MAX_BUFFER_SAMPLES)
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_new_lanes(
    rate: f64, widths: *const u32, count: usize, history_secs: u32, physical_memory: u64,
) -> *mut BtTracks {
    if widths.is_null() || !(1..=MAX_LANES).contains(&count) {
        return ptr::null_mut();
    }
    let budget = (physical_memory / 8 / size_of::<f32>() as u64)
        .clamp(DEFAULT_MAX_BUFFER_SAMPLES as u64, 512 * 1024 * 1024) as usize;
    new_tracks(rate, slice::from_raw_parts(widths, count), history_secs, budget)
}

fn new_tracks(rate: f64, widths: &[u32], history_secs: u32, budget: usize) -> *mut BtTracks {
    let (Some(rate), Some(history)) = (
        sample_rate(rate),
        DurationChoice::from_seconds(history_secs),
    ) else {
        return ptr::null_mut();
    };
    let widths: Vec<_> = widths.iter().map(|&width| width as usize).collect();
    let Ok(synchronized) = SynchronizedCapture::new_lanes_with_budget(rate, &widths, history, budget)
    else {
        return ptr::null_mut();
    };
    // Keep enough capacity to tolerate a delayed UI poll at high source rates.
    let capacity = 96_000.max(rate as usize / 4);
    Box::into_raw(Box::new(BtTracks {
        queues: widths.iter().map(|&width| TimedAudioQueue::new(capacity, 512, width)).collect(),
        reader: Mutex::new(TrackReader {
            rate,
            synchronized,
            senders: widths.iter().map(|_| None).collect(),
            sender_audio: Vec::new(),
            sender_bars: Vec::new(),
            scratch: Vec::new(),
        }),
        enabled: AtomicBool::new(true),
        preview_silence: widths.iter().map(|_| AtomicBool::new(false)).collect(),
        gate: PreviewGate::shared(),
    }))
}

/// Marks `lane` as recording System playback (nonzero `enabled`), so that what it captures
/// while a Backtracking preview may be heard is kept as silence.
#[no_mangle]
unsafe extern "C" fn bt_tracks_set_preview_silence(core: *const BtTracks, lane: u32, enabled: u32) {
    if let Some(flag) = core.as_ref().and_then(|core| core.preview_silence.get(lane as usize)) {
        flag.store(enabled != 0, Ordering::Relaxed);
    }
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
unsafe extern "C" fn bt_tracks_retention_frames(core: *const BtTracks) -> u64 {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .map_or(0, |reader| reader.synchronized.capture().effective_retention_frames() as u64)
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
    let Some(core) = core.as_ref() else { return 0; };
    if minimum.is_null() || maximum.is_null() || first >= end { return 0; }
    let Ok(reader) = core.reader.lock() else { return 0; };
    let minimum = slice::from_raw_parts_mut(minimum, bins as usize);
    let maximum = slice::from_raw_parts_mut(maximum, bins as usize);
    let capture = reader.synchronized.capture();
    (match WaveformView::from_code(view) {
        Some(view) => capture.track_waveform_range(lane as usize, view, first..end, minimum, maximum),
        None => capture.track_channel_waveform_range(
            lane as usize, (view - 3) as usize, first..end, minimum, maximum,
        ),
    }) as u64
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
    let mut sender = TrackSender {
        id: id.to_owned(),
        receiver: None,
        route,
        weights: Vec::new(),
        retry_at: Instant::now(),
    };
    if !sender.bind(receiver, queue.channels()) { return 0; }
    let Ok(mut reader) = core.reader.lock() else { return 0 };
    reader.senders[lane as usize] = Some(sender);
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
unsafe extern "C" fn bt_sender_channel_labels(id: *const c_char) -> *mut c_char {
    if id.is_null() { return ptr::null_mut(); }
    let Ok(id) = CStr::from_ptr(id).to_str() else { return ptr::null_mut(); };
    crate::sender::sender_channel_labels(id)
        .and_then(|labels| CString::new(labels.join("\t")).ok())
        .map_or(ptr::null_mut(), CString::into_raw)
}

/// The housekeeping schedule for the drag-and-drop folder. `days` is 0 for Off; a number the
/// Options form does not offer also means Off.
#[no_mangle]
extern "C" fn bt_cleanup_new(days: u32) -> *mut cleanup::Schedule {
    Box::into_raw(Box::new(cleanup::Schedule::new(cleanup_age(days), Instant::now())))
}

fn cleanup_age(days: u32) -> Option<u16> {
    u16::try_from(days).ok().and_then(cleanup::age_from_days)
}

/// Waits for a pass that is still running.
#[no_mangle]
unsafe extern "C" fn bt_cleanup_free(schedule: *mut cleanup::Schedule) {
    if !schedule.is_null() { drop(Box::from_raw(schedule)); }
}

#[no_mangle]
unsafe extern "C" fn bt_cleanup_days(schedule: *const cleanup::Schedule) -> u32 {
    schedule.as_ref().and_then(cleanup::Schedule::age).map_or(0, u32::from)
}

#[no_mangle]
unsafe extern "C" fn bt_cleanup_set_days(schedule: *mut cleanup::Schedule, days: u32) {
    if let Some(schedule) = schedule.as_mut() { schedule.set_age(cleanup_age(days), Instant::now()); }
}

/// Puts the next pass off. Call it when either folder setting changes.
#[no_mangle]
unsafe extern "C" fn bt_cleanup_restart(schedule: *mut cleanup::Schedule) {
    if let Some(schedule) = schedule.as_mut() { schedule.restart(Instant::now()); }
}

#[no_mangle]
unsafe extern "C" fn bt_cleanup_due(schedule: *const cleanup::Schedule) -> i32 {
    i32::from(schedule.as_ref().is_some_and(|schedule| schedule.due(Instant::now())))
}

/// Starts a pass over `folder` when one is due. A null `folder` means it must not be cleaned
/// now. `keep` lists files that stay whatever their age. Returns the number of files a
/// finished pass deleted, -1 when that pass failed and -2 when no pass has finished.
#[no_mangle]
unsafe extern "C" fn bt_cleanup_poll(
    schedule: *mut cleanup::Schedule, folder: *const c_char, keep: *const *const c_char, keep_count: usize,
) -> i64 {
    let Some(schedule) = schedule.as_mut() else { return -2 };
    let path = |text: *const c_char| -> Option<PathBuf> {
        if text.is_null() { return None; }
        CStr::from_ptr(text).to_str().ok().map(PathBuf::from)
    };
    let keep: Vec<PathBuf> = if keep.is_null() { Vec::new() } else {
        slice::from_raw_parts(keep, keep_count).iter().filter_map(|&file| path(file)).collect()
    };
    match schedule.poll(path(folder).as_deref(), &keep, Instant::now()) {
        None => -2,
        Some(Ok(removed)) => i64::try_from(removed).unwrap_or(i64::MAX),
        Some(Err(_)) => -1,
    }
}

/// Whether two folder settings name the same folder on disk.
#[no_mangle]
unsafe extern "C" fn bt_same_folder(a: *const c_char, b: *const c_char) -> i32 {
    if a.is_null() || b.is_null() { return 0; }
    let (Ok(a), Ok(b)) = (CStr::from_ptr(a).to_str(), CStr::from_ptr(b).to_str()) else { return 0 };
    i32::from(cleanup::same_folder(Path::new(a), Path::new(b)))
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
    bt_snapshot_export_routed_faded_frames(
        snapshot, first_frame, end_frame, directory, sample_rate, bits_per_sample, route,
        force_mono, 0, 0,
    )
}

/// Exports `first_frame..end_frame` with a fade in and a fade out of the given lengths, in
/// the snapshot's own frames. Ramps longer than the clip are shortened to fit it.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_snapshot_export_routed_faded_frames(
    snapshot: *const Snapshot,
    first_frame: u64,
    end_frame: u64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    route: u32,
    force_mono: u32,
    fade_in: u64,
    fade_out: u64,
) -> *mut c_char {
    bt_snapshot_export_labelled_frames(
        snapshot, first_frame, end_frame, directory, sample_rate, bits_per_sample, route,
        force_mono, fade_in, fade_out, ptr::null(), 0,
    )
}

/// Like [`bt_snapshot_export_routed_faded_frames`], naming the file after its source and lane:
/// `Backtracking_<source>_<lane>_<date>…`. `source_name` is UTF-8 and may be null or empty,
/// which puts the lane after the take as the unnamed exports do; `lane` is 0 for none, 1 for
/// A and 2 for B.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_snapshot_export_labelled_frames(
    snapshot: *const Snapshot,
    first_frame: u64,
    end_frame: u64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    route: u32,
    force_mono: u32,
    fade_in: u64,
    fade_out: u64,
    source_name: *const c_char,
    lane: u32,
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
    let Some(lane) = lane_arg(lane) else {
        return ptr::null_mut();
    };
    let Some(source) = source_name_arg(source_name) else {
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
        Fades::new(fade_in, fade_out),
        &[TakeLabel::new(source, lane)],
    ) else {
        return ptr::null_mut();
    };
    paths.remove(0).into_raw()
}

/// A source name passed in from C: null is no name, and text that is not UTF-8 is refused.
unsafe fn source_name_arg(name: *const c_char) -> Option<String> {
    if name.is_null() {
        return Some(String::new());
    }
    CStr::from_ptr(name).to_str().ok().map(str::to_owned)
}

/// A lane passed in from C: 0 is none, 1 is A, through 64 for BL.
fn lane_arg(lane: u32) -> Option<Option<Lane>> {
    if lane == 0 {
        Some(None)
    } else {
        Lane::from_index(lane as usize - 1).map(Some)
    }
}

/// How the files of a source and lane are named, up to the date, for showing in settings.
/// Takes the same `source_name` and `lane` as [`bt_snapshot_export_labelled_frames`]; free
/// the result with `bt_string_free`. Null for a lane it does not know or text that is not
/// UTF-8.
#[no_mangle]
unsafe extern "C" fn bt_take_file_prefix(source_name: *const c_char, lane: u32) -> *mut c_char {
    let (Some(source), Some(lane)) = (source_name_arg(source_name), lane_arg(lane)) else {
        return ptr::null_mut();
    };
    CString::new(TakeLabel::new(source, lane).file_prefix()).map_or(ptr::null_mut(), CString::into_raw)
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
    bt_snapshots_export_pair_routed_faded_frames(
        snapshot_a, snapshot_b, first_frame, end_frame, directory, sample_rate,
        bits_per_sample, route_a, route_b, force_mono, 0, 0, path_a, path_b,
    )
}

/// The A/B form of [`bt_snapshot_export_routed_faded_frames`]: both files fade at the same frames.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_snapshots_export_pair_routed_faded_frames(
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
    fade_in: u64,
    fade_out: u64,
    path_a: *mut *mut c_char,
    path_b: *mut *mut c_char,
) -> i32 {
    bt_snapshots_export_pair_labelled_frames(
        snapshot_a, snapshot_b, first_frame, end_frame, directory, sample_rate,
        bits_per_sample, route_a, route_b, force_mono, fade_in, fade_out, ptr::null(),
        ptr::null(), path_a, path_b,
    )
}

/// The A/B form of [`bt_snapshot_export_labelled_frames`]: each file is named after its own
/// source, as lane A and lane B.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_snapshots_export_pair_labelled_frames(
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
    fade_in: u64,
    fade_out: u64,
    source_name_a: *const c_char,
    source_name_b: *const c_char,
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
    let (Some(source_a), Some(source_b)) =
        (source_name_arg(source_name_a), source_name_arg(source_name_b))
    else {
        return 0;
    };
    export_pair(
        [snapshot_a, snapshot_b],
        frames,
        directory,
        [sample_rate, bits_per_sample],
        [route_a, route_b],
        force_mono != 0,
        Fades::new(fade_in, fade_out),
        [TakeLabel::new(source_a, Some(Lane::A)), TakeLabel::new(source_b, Some(Lane::B))],
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
        Fades::NONE,
        Lane::BOTH.map(|lane| TakeLabel::new("", Some(lane))),
        [path_a, path_b],
    )
}

fn relative_frames(snapshot: &Snapshot, first: u64, end: u64) -> Option<Range<usize>> {
    if first >= end || first < snapshot.first_frame || end > snapshot.frame_range().end {
        return None;
    }
    Some((first - snapshot.first_frame) as usize..(end - snapshot.first_frame) as usize)
}

#[allow(clippy::too_many_arguments)]
unsafe fn export_pair(
    [snapshot_a, snapshot_b]: [*const Snapshot; 2],
    frames: Range<usize>,
    directory: *const c_char,
    [sample_rate, bits_per_sample]: [u32; 2],
    routes: [ChannelRoute; 2],
    force_mono: bool,
    fades: Fades,
    labels: [TakeLabel; 2],
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
        fades,
        &labels,
    ) else {
        return 0;
    };
    *path_a = paths.remove(0).into_raw();
    *path_b = paths.remove(0).into_raw();
    1
}

#[allow(clippy::too_many_arguments)]
unsafe fn export_snapshots<T: TrackAudio>(
    snapshots: &[&T],
    frames: Range<usize>,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    routes: &[ChannelRoute],
    force_mono: bool,
    fades: Fades,
    labels: &[TakeLabel],
) -> Option<Vec<CString>> {
    let directory = export_directory(directory)?;
    let sample_format = WavSampleFormat::from_bits_per_sample(bits_per_sample)?;
    let profile = WavExportProfile {
        sample_rate,
        sample_format,
    };
    let paths = export_routed_track_refs(
        directory,
        snapshots,
        frames,
        profile,
        routes,
        force_mono,
        // The desktop apps capture system audio and have no tempo to report.
        None,
        fades,
        labels,
    )
    .ok()?;
    encode_export_paths(paths)
}

unsafe fn export_directory<'a>(directory: *const c_char) -> Option<&'a Path> {
    if directory.is_null() {
        return None;
    }
    let directory = CStr::from_ptr(directory).to_str().ok()?;
    (!directory.is_empty()).then(|| Path::new(directory))
}

fn remove_export_paths(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

fn encode_export_paths(paths: Vec<PathBuf>) -> Option<Vec<CString>> {
    let result: Option<Vec<CString>> = paths
        .iter()
        .map(|path| CString::new(path.to_str()?).ok())
        .collect();
    if result.is_none() {
        remove_export_paths(&paths);
    }
    result
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_freeze(
    core: *const BtTracks, first: u64, end: u64,
) -> *mut FrozenCapture {
    let frozen = core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .and_then(|reader| reader.synchronized.capture().freeze_range(first..end));
    frozen.map_or(ptr::null_mut(), |value| Box::into_raw(Box::new(value)))
}

#[no_mangle]
unsafe extern "C" fn bt_frozen_free(frozen: *mut FrozenCapture) {
    if !frozen.is_null() {
        drop(Box::from_raw(frozen));
    }
}

const PAIR_LEFT: u32 = 67;
const PAIR_RIGHT: u32 = 68;

fn pair_side(view: u32) -> Option<PairSide> {
    match view {
        PAIR_LEFT => Some(PairSide::Left),
        PAIR_RIGHT => Some(PairSide::Right),
        _ => None,
    }
}

#[no_mangle]
unsafe extern "C" fn bt_frozen_waveform(
    frozen: *const FrozenCapture, lane: u32, view: u32, first: u64, end: u64,
    minimum: *mut f32, maximum: *mut f32, bins: u32, labels: *const c_char,
) {
    let Some(track) = frozen.as_ref().and_then(|frozen| frozen.track(lane as usize)) else {
        return;
    };
    if minimum.is_null() || maximum.is_null() || first >= end { return; }
    let minimum = slice::from_raw_parts_mut(minimum, bins as usize);
    let maximum = slice::from_raw_parts_mut(maximum, bins as usize);
    if let Some(view) = WaveformView::from_code(view) {
        track.waveform(view, first..end, minimum, maximum);
    } else if let Some(side) = pair_side(view) {
        if let Some(group) = PairGroup::new(0, &channel_names(labels, track.channels())) {
            track.pair_waveform(&group, side, first..end, minimum, maximum);
        }
    } else {
        track.channel_waveform((view - 3) as usize, first..end, minimum, maximum);
    }
}

#[no_mangle]
unsafe extern "C" fn bt_frozen_spectrum(
    frozen: *const FrozenCapture, lane: u32, view: u32, first: u64, end: u64,
    energies: *mut [f32; SPECTRUM_BANDS], bins: u32, labels: *const c_char,
) {
    let Some(track) = frozen.as_ref().and_then(|frozen| frozen.track(lane as usize)) else {
        return;
    };
    if energies.is_null() || first >= end { return; }
    let group;
    let row = if let Some(view) = WaveformView::from_code(view) {
        SpectrumRow::View(view)
    } else if let Some(side) = pair_side(view) {
        let Some(pair) = PairGroup::new(0, &channel_names(labels, track.channels())) else {
            return;
        };
        group = pair;
        SpectrumRow::Pair(&group, side)
    } else {
        SpectrumRow::Channel((view - 3) as usize)
    };
    let mut results = vec![None; bins as usize];
    track.spectrum(row, first..end, &mut results);
    for (out, bin) in slice::from_raw_parts_mut(energies, bins as usize).iter_mut().zip(results) {
        *out = bin.unwrap_or_default();
    }
}

#[repr(C)]
struct BtFrozenLane {
    index: u32,
    route: u32,
    source_name: *const c_char,
    channel_labels: *const c_char,
    view: u32,
}

unsafe fn frozen_lanes<'a>(
    frozen: *const FrozenCapture, lanes: *const BtFrozenLane, count: usize,
    first: u64, end: u64,
) -> Option<(&'a FrozenCapture, &'a [BtFrozenLane])> {
    let frozen = frozen.as_ref()?;
    let held = frozen.frame_range();
    if lanes.is_null() || !(1..=frozen.track_count()).contains(&count)
        || first < held.start || end > held.end || first >= end
    {
        return None;
    }
    Some((frozen, slice::from_raw_parts(lanes, count)))
}

#[no_mangle]
unsafe extern "C" fn bt_frozen_play(
    frozen: *const FrozenCapture, slot: *const PreviewSlot, lanes: *const BtFrozenLane,
    count: usize, first: u64, end: u64, force_mono: u32, pair_binaural: u32, fades: Fades,
) -> i32 {
    let (Some(slot), Some((frozen, lanes))) = (slot.as_ref(), frozen_lanes(frozen, lanes, count, first, end))
    else {
        return 0;
    };
    let Some(tracks) = lanes.iter().map(|lane| frozen.track(lane.index as usize)).collect::<Option<Vec<_>>>()
    else { return 0 };
    let Some(routes) = lanes.iter().map(|lane| ChannelRoute::from_code(lane.route)).collect::<Option<Vec<_>>>()
    else { return 0 };
    let groups: Vec<_> = tracks.iter().zip(lanes).map(|(track, lane)| {
        (track.channels() > 2)
            .then(|| PairGroup::new(0, &channel_names(lane.channel_labels, track.channels())))
            .flatten()
    }).collect();
    if tracks.iter().zip(lanes).any(|(track, lane)| !valid_row(lane.view, track.channels())) {
        return 0;
    }
    let selected: Vec<_> = tracks.iter().zip(lanes).map(|(track, lane)| {
        selection_channel(lane.view, track.channels()).map(|channel| [channel])
    }).collect();
    let playback: Vec<_> = tracks.iter().zip(routes).zip(groups.iter().zip(&selected))
        .map(|((track, route), (group, selected))| PreviewLane {
            track, route, group: group.as_ref(), selected: selected.as_ref().map(<[_; 1]>::as_slice),
        }).collect();
    let model = if pair_binaural != 0 && groups.iter().any(Option::is_some) {
        let Ok(model) = crate::spatial::HeadModel::new(frozen.sample_rate()) else { return 0; };
        Some(Arc::new(model))
    } else {
        None
    };
    let Some(preview) = Preview::with_channels(&playback, first..end, model, force_mono != 0, fades)
    else { return 0 };
    slot.play(preview);
    1
}

fn valid_row(view: u32, width: usize) -> bool {
    view == 0 || (width > 2 && pair_side(view).is_some())
        || selection_channel(view, width).is_some_and(|channel| channel < width)
}

fn selection_channel(view: u32, width: usize) -> Option<usize> {
    match view {
        1..=2 if width == 2 => Some((view - 1) as usize),
        3..=66 if width > 2 => Some((view - 3) as usize),
        _ => None,
    }
}

unsafe fn channel_names(labels: *const c_char, channels: usize) -> Vec<String> {
    let labels = source_name_arg(labels).unwrap_or_default();
    let labels: Vec<_> = labels.split('\t').map(str::to_owned).collect();
    if labels.len() == channels {
        labels
    } else {
        (1..=channels).map(|channel| format!("Channel {channel}")).collect()
    }
}

#[no_mangle]
unsafe extern "C" fn bt_frozen_export(
    frozen: *const FrozenCapture, lanes: *const BtFrozenLane, count: usize,
    first: u64, end: u64, directory: *const c_char, sample_rate: u32, bits_per_sample: u32,
    force_mono: u32, wide_mode: u32, pair_binaural: u32, fades: Fades,
) -> *mut Vec<CString> {
    let Some((frozen, lanes)) = frozen_lanes(frozen, lanes, count, first, end) else {
        return ptr::null_mut();
    };
    let (Some(directory), Some(sample_format)) = (
        export_directory(directory), WavSampleFormat::from_bits_per_sample(bits_per_sample),
    ) else {
        return ptr::null_mut();
    };
    let mode = match wide_mode {
        0 => WideExportMode::BinauralFiles,
        1 => WideExportMode::MonoFiles,
        2 => WideExportMode::Multichannel,
        3 => WideExportMode::BinauralMix,
        _ => return ptr::null_mut(),
    };
    let mut tracks = Vec::with_capacity(count);
    let mut routes = Vec::with_capacity(count);
    let mut labels = Vec::with_capacity(count);
    for lane in lanes {
        let (Some(track), Some(route), Some(name)) = (
            frozen.track(lane.index as usize),
            ChannelRoute::from_code(lane.route),
            source_name_arg(lane.source_name),
        ) else {
            return ptr::null_mut();
        };
        if !valid_row(lane.view, track.channels()) {
            return ptr::null_mut();
        }
        tracks.push(track);
        routes.push(route);
        let letter = if frozen.track_count() > 1 { Lane::from_index(lane.index as usize) } else { None };
        labels.push(TakeLabel::new(name, letter));
    }
    let start = frozen.frame_range().start;
    let frames = (first - start) as usize..(end - start) as usize;
    let profile = WavExportProfile { sample_rate, sample_format };
    let mut paths = Vec::new();
    let result: std::io::Result<()> = (|| {
        if tracks.iter().zip(lanes).all(|(track, lane)| track.channels() <= 2 && lane.view == 0) {
            let references: Vec<_> = tracks.iter().collect();
            paths = export_routed_track_refs(
                directory, &references, frames, profile, &routes, force_mono != 0, None, fades, &labels,
            )?;
        } else {
            for (index, track) in tracks.iter().enumerate() {
                let view = lanes[index].view;
                let channel = selection_channel(view, track.channels());
                let selected = channel.map(|channel| [channel]);
                let written = if track.channels() > 2 {
                    let names = channel_names(lanes[index].channel_labels, track.channels());
                    let group = WideExportGroup { channels: 0..track.channels(), labels: &names, positioned: false };
                    let pair_only = pair_side(view).is_some();
                    let channels = if pair_only { Some(&[][..]) } else { selected.as_ref().map(<[_; 1]>::as_slice) };
                    export_wide_track(
                        directory, track, frames.clone(), fades, profile, &labels[index], Some(&group), mode,
                        &WideRowExport { channels, pair: channel.is_none(), pair_binaural: pair_binaural != 0,
                            ..WideRowExport::default() },
                    )?.paths
                } else if let Some(channel) = channel {
                    crate::export_channel_range_with_profile(
                        directory, track, frames.clone(), channel..channel + 1, &[], profile,
                        crate::ChannelExportMode::Multichannel, fades, &labels[index],
                    )?
                } else {
                    export_routed_track_refs(
                        directory, &[track], frames.clone(), profile, &routes[index..=index], force_mono != 0,
                        None, fades, &labels[index..=index],
                    )?
                };
                paths.extend(written);
            }
        }
        Ok(())
    })();
    if result.is_err() {
        remove_export_paths(&paths);
        return ptr::null_mut();
    }
    encode_export_paths(paths).map_or(ptr::null_mut(), |paths| Box::into_raw(Box::new(paths)))
}

#[no_mangle]
unsafe extern "C" fn bt_export_paths_count(paths: *const Vec<CString>) -> usize {
    paths.as_ref().map_or(0, Vec::len)
}

#[no_mangle]
unsafe extern "C" fn bt_export_path(paths: *const Vec<CString>, index: usize) -> *const c_char {
    paths.as_ref().and_then(|paths| paths.get(index)).map_or(ptr::null(), |path| path.as_ptr())
}

#[no_mangle]
unsafe extern "C" fn bt_export_paths_free(paths: *mut Vec<CString>) {
    if !paths.is_null() {
        drop(Box::from_raw(paths));
    }
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_active_channels(core: *const BtTracks, lane: u32) -> u64 {
    core.as_ref()
        .and_then(|core| core.reader.lock().ok())
        .map_or(0, |reader| reader.synchronized.capture().active_channels(lane as usize))
}

#[no_mangle]
unsafe extern "C" fn bt_frozen_active_channels(frozen: *const FrozenCapture, lane: u32) -> u64 {
    frozen
        .as_ref()
        .and_then(|frozen| frozen.track(lane as usize))
        .map_or(0, |track| track.active_channels())
}

#[repr(C)]
struct BtFrozenSelectionLane {
    index: u32,
    route: u32,
    source_name: *const c_char,
    channel_labels: *const c_char,
    channel_mask: u64,
    channel_mask_valid: u32,
    include_pair: u32,
}

unsafe fn set_ffi_error(error_out: *mut *mut c_char, message: impl AsRef<str>) {
    if !error_out.is_null() {
        let sanitized = message.as_ref().replace('\0', " ");
        *error_out = CString::new(sanitized).map_or(ptr::null_mut(), CString::into_raw);
    }
}

fn mask_channels(mask: u64, width: usize) -> Option<Vec<usize>> {
    if width == 0 || width > 64 {
        return None;
    }
    let valid_bits = if width == 64 { u64::MAX } else { (1u64 << width) - 1 };
    if mask & !valid_bits != 0 {
        return None;
    }
    Some((0..width).filter(|&i| (mask & (1u64 << i)) != 0).collect())
}

#[no_mangle]
unsafe extern "C" fn bt_frozen_play_selection(
    frozen: *const FrozenCapture,
    slot: *const PreviewSlot,
    lanes: *const BtFrozenSelectionLane,
    count: usize,
    first: u64,
    end: u64,
    force_mono: u32,
    pair_binaural: u32,
    fades: Fades,
    error_out: *mut *mut c_char,
) -> i32 {
    if !error_out.is_null() {
        *error_out = ptr::null_mut();
    }
    let (Some(slot), Some(frozen)) = (slot.as_ref(), frozen.as_ref()) else {
        set_ffi_error(error_out, "Invalid capture lanes");
        return 0;
    };
    let held = frozen.frame_range();
    if lanes.is_null() || !(1..=frozen.track_count()).contains(&count) {
        set_ffi_error(error_out, "Invalid capture lanes");
        return 0;
    }
    if first < held.start || end > held.end || first >= end {
        set_ffi_error(error_out, "The selected audio is unavailable.");
        return 0;
    }
    let lanes = slice::from_raw_parts(lanes, count);
    let mut tracks = Vec::with_capacity(count);
    let mut routes = Vec::with_capacity(count);
    let mut groups = Vec::with_capacity(count);
    let mut selected: Vec<Option<Vec<usize>>> = Vec::with_capacity(count);
    for lane in lanes {
        let Some(track) = frozen.track(lane.index as usize) else {
            set_ffi_error(error_out, "Invalid capture lanes");
            return 0;
        };
        let Some(route) = ChannelRoute::from_code(lane.route) else {
            set_ffi_error(error_out, "channel route does not match the captured audio");
            return 0;
        };
        let width = track.channels();
        let group = (width > 2)
            .then(|| PairGroup::new(0, &channel_names(lane.channel_labels, width)))
            .flatten();
        let sel = if lane.channel_mask_valid == 0 {
            None
        } else {
            let Some(indices) = mask_channels(lane.channel_mask, width) else {
                set_ffi_error(error_out, "The selected rows do not match this capture");
                return 0;
            };
            if width > 2 {
                if lane.include_pair != 0 {
                    // Pair already mixes the full speaker bed.
                    None
                } else if indices.is_empty() {
                    set_ffi_error(error_out, "The selected rows do not match this capture");
                    return 0;
                } else {
                    Some(indices)
                }
            } else if width == 2 {
                match indices.as_slice() {
                    [c] => Some(vec![*c]),
                    [0, 1] => None,
                    _ => {
                        set_ffi_error(error_out, "The selected rows do not match this capture");
                        return 0;
                    }
                }
            } else {
                if indices.is_empty() {
                    set_ffi_error(error_out, "The selected rows do not match this capture");
                    return 0;
                }
                None
            }
        };
        tracks.push(track);
        routes.push(route);
        groups.push(group);
        selected.push(sel);
    }
    let playback: Vec<_> = tracks
        .iter()
        .zip(routes)
        .zip(groups.iter().zip(&selected))
        .map(|((track, route), (group, sel))| PreviewLane {
            track,
            route,
            group: group.as_ref(),
            selected: sel.as_deref(),
        })
        .collect();
    let model = if pair_binaural != 0 && groups.iter().any(Option::is_some) {
        let Ok(model) = crate::spatial::HeadModel::new(frozen.sample_rate()) else {
            set_ffi_error(error_out, "Could not prepare binaural head model");
            return 0;
        };
        Some(Arc::new(model))
    } else {
        None
    };
    let Some(preview) = Preview::with_channels(&playback, first..end, model, force_mono != 0, fades) else {
        set_ffi_error(error_out, "The selected audio is unavailable.");
        return 0;
    };
    slot.play(preview);
    1
}

#[no_mangle]
unsafe extern "C" fn bt_frozen_export_selection(
    frozen: *const FrozenCapture,
    lanes: *const BtFrozenSelectionLane,
    count: usize,
    first: u64,
    end: u64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    force_mono: u32,
    wide_mode: u32,
    pair_binaural: u32,
    fades: Fades,
    error_out: *mut *mut c_char,
) -> *mut Vec<CString> {
    if !error_out.is_null() {
        *error_out = ptr::null_mut();
    }
    let Some(frozen) = frozen.as_ref() else {
        set_ffi_error(error_out, "Invalid capture lanes");
        return ptr::null_mut();
    };
    let held = frozen.frame_range();
    if lanes.is_null() || !(1..=frozen.track_count()).contains(&count) {
        set_ffi_error(error_out, "Invalid capture lanes");
        return ptr::null_mut();
    }
    if first < held.start || end > held.end || first >= end {
        set_ffi_error(error_out, "expected aligned nonempty audio lanes");
        return ptr::null_mut();
    }
    let Some(directory) = export_directory(directory) else {
        set_ffi_error(error_out, "Invalid export folder");
        return ptr::null_mut();
    };
    let Some(sample_format) = WavSampleFormat::from_bits_per_sample(bits_per_sample) else {
        set_ffi_error(error_out, "Invalid WAV encoding");
        return ptr::null_mut();
    };
    let mode = match wide_mode {
        0 => WideExportMode::BinauralFiles,
        1 => WideExportMode::MonoFiles,
        2 => WideExportMode::Multichannel,
        3 => WideExportMode::BinauralMix,
        _ => {
            set_ffi_error(error_out, "Invalid multichannel export mode");
            return ptr::null_mut();
        }
    };
    let lanes = slice::from_raw_parts(lanes, count);
    let mut tracks = Vec::with_capacity(count);
    let mut routes = Vec::with_capacity(count);
    let mut labels = Vec::with_capacity(count);
    let mut subsets: Vec<Option<Vec<usize>>> = Vec::with_capacity(count);
    let mut narrowed_stereo: Vec<Option<usize>> = Vec::with_capacity(count);
    for lane in lanes {
        let Some(track) = frozen.track(lane.index as usize) else {
            set_ffi_error(error_out, "Invalid capture lanes");
            return ptr::null_mut();
        };
        let Some(route) = ChannelRoute::from_code(lane.route) else {
            set_ffi_error(error_out, "channel route does not match the captured audio");
            return ptr::null_mut();
        };
        let Some(name) = source_name_arg(lane.source_name) else {
            set_ffi_error(error_out, "Invalid source name");
            return ptr::null_mut();
        };
        let width = track.channels();
        let mut narrow = None;
        let subset = if lane.channel_mask_valid == 0 {
            None
        } else {
            let Some(indices) = mask_channels(lane.channel_mask, width) else {
                set_ffi_error(error_out, "The selected rows do not match this capture");
                return ptr::null_mut();
            };
            if indices.is_empty() && (width <= 2 || lane.include_pair == 0) {
                set_ffi_error(error_out, "The selected rows do not match this capture");
                return ptr::null_mut();
            }
            if width == 2 && indices.len() == 1 {
                narrow = Some(indices[0]);
            }
            Some(indices)
        };
        tracks.push(track);
        routes.push(route);
        let letter = if frozen.track_count() > 1 {
            Lane::from_index(lane.index as usize)
        } else {
            None
        };
        labels.push(TakeLabel::new(name, letter));
        subsets.push(subset);
        narrowed_stereo.push(narrow);
    }
    let start = frozen.frame_range().start;
    let frames = (first - start) as usize..(end - start) as usize;
    let profile = WavExportProfile { sample_rate, sample_format };
    let mut paths = Vec::new();
    let result: std::io::Result<()> = (|| {
        if tracks.iter().all(|track| track.channels() <= 2) && narrowed_stereo.iter().all(Option::is_none) {
            let references: Vec<_> = tracks.iter().collect();
            paths = export_routed_track_refs(
                directory, &references, frames, profile, &routes, force_mono != 0, None, fades, &labels,
            )?;
        } else {
            for (index, track) in tracks.iter().enumerate() {
                let written = if track.channels() > 2 {
                    let names = channel_names(lanes[index].channel_labels, track.channels());
                    let group = WideExportGroup {
                        channels: 0..track.channels(),
                        labels: &names,
                        positioned: false,
                    };
                    export_wide_track(
                        directory,
                        track,
                        frames.clone(),
                        fades,
                        profile,
                        &labels[index],
                        Some(&group),
                        mode,
                        &WideRowExport {
                            channels: subsets[index].as_deref(),
                            pair: lanes[index].include_pair != 0,
                            pair_binaural: pair_binaural != 0,
                            ..WideRowExport::default()
                        },
                    )?.paths
                } else if let Some(channel) = narrowed_stereo[index] {
                    crate::export_channel_range_with_profile(
                        directory,
                        track,
                        frames.clone(),
                        channel..channel + 1,
                        &[],
                        profile,
                        crate::ChannelExportMode::Multichannel,
                        fades,
                        &labels[index],
                    )?
                } else {
                    export_routed_track_refs(
                        directory,
                        &[track],
                        frames.clone(),
                        profile,
                        &routes[index..=index],
                        force_mono != 0,
                        None,
                        fades,
                        &labels[index..=index],
                    )?
                };
                paths.extend(written);
            }
        }
        Ok(())
    })();
    if let Err(err) = result {
        remove_export_paths(&paths);
        set_ffi_error(error_out, err.to_string());
        return ptr::null_mut();
    }
    match encode_export_paths(paths) {
        Some(encoded) => Box::into_raw(Box::new(encoded)),
        None => {
            set_ffi_error(error_out, "Export path is not valid UTF-8");
            ptr::null_mut()
        }
    }
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_enable_frame_clock(core: *const BtTracks, lane: u32) -> i32 {
    let Some(core) = core.as_ref() else { return 0 };
    let Ok(mut reader) = core.reader.lock() else { return 0 };
    i32::from(reader.synchronized.enable_frame_clock(lane as usize).is_ok())
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_take_frame_clock_eviction(
    core: *const BtTracks,
    lane: u32,
    first_out: *mut u64,
) -> i32 {
    let Some(core) = core.as_ref() else { return 0 };
    let Ok(mut reader) = core.reader.lock() else { return 0 };
    if let Some(first) = reader.synchronized.take_frame_clock_eviction(lane as usize) {
        if let Some(out) = first_out.as_mut() {
            *out = first;
        }
        1
    } else {
        0
    }
}

#[no_mangle]
unsafe extern "C" fn bt_tracks_retained_frame_at(
    core: *const BtTracks,
    lane: u32,
    time_ns: u64,
    frame_out: *mut u64,
) -> i32 {
    let Some(core) = core.as_ref() else { return 0 };
    let Ok(reader) = core.reader.lock() else { return 0 };
    match reader.synchronized.retained_frame_at(lane as usize, time_ns) {
        std::task::Poll::Ready(Some(frame)) => {
            if let Some(out) = frame_out.as_mut() {
                *out = frame;
            }
            1
        }
        std::task::Poll::Ready(None) => 0,
        std::task::Poll::Pending => -1,
    }
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_frozen_waveform_bed(
    frozen: *const FrozenCapture,
    lane: u32,
    view: u32,
    bed_start: u32,
    bed_count: u32,
    first: u64,
    end: u64,
    minimum: *mut f32,
    maximum: *mut f32,
    bins: u32,
    bed_labels: *const c_char,
) {
    let Some(track) = frozen.as_ref().and_then(|frozen| frozen.track(lane as usize)) else {
        return;
    };
    if minimum.is_null() || maximum.is_null() || first >= end {
        return;
    }
    let minimum = slice::from_raw_parts_mut(minimum, bins as usize);
    let maximum = slice::from_raw_parts_mut(maximum, bins as usize);
    if let Some(view) = WaveformView::from_code(view) {
        track.waveform(view, first..end, minimum, maximum);
    } else if let Some(side) = pair_side(view) {
        let width = track.channels();
        let (start, count) = if bed_count > 0 && (bed_start as usize) + (bed_count as usize) <= width {
            (bed_start as usize, bed_count as usize)
        } else {
            (0, width)
        };
        if let Some(group) = PairGroup::new(start, &channel_names(bed_labels, count)) {
            track.pair_waveform(&group, side, first..end, minimum, maximum);
        }
    } else {
        track.channel_waveform((view - 3) as usize, first..end, minimum, maximum);
    }
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_frozen_spectrum_bed(
    frozen: *const FrozenCapture,
    lane: u32,
    view: u32,
    bed_start: u32,
    bed_count: u32,
    first: u64,
    end: u64,
    energies: *mut [f32; SPECTRUM_BANDS],
    bins: u32,
    bed_labels: *const c_char,
) {
    let Some(track) = frozen.as_ref().and_then(|frozen| frozen.track(lane as usize)) else {
        return;
    };
    if energies.is_null() || first >= end {
        return;
    }
    let group;
    let row = if let Some(view) = WaveformView::from_code(view) {
        SpectrumRow::View(view)
    } else if let Some(side) = pair_side(view) {
        let width = track.channels();
        let (start, count) = if bed_count > 0 && (bed_start as usize) + (bed_count as usize) <= width {
            (bed_start as usize, bed_count as usize)
        } else {
            (0, width)
        };
        let Some(pair) = PairGroup::new(start, &channel_names(bed_labels, count)) else {
            return;
        };
        group = pair;
        SpectrumRow::Pair(&group, side)
    } else {
        SpectrumRow::Channel((view - 3) as usize)
    };
    let mut results = vec![None; bins as usize];
    track.spectrum(row, first..end, &mut results);
    for (out, bin) in slice::from_raw_parts_mut(energies, bins as usize).iter_mut().zip(results) {
        *out = bin.unwrap_or_default();
    }
}

#[repr(C)]
struct BtFrozenGroupSelectionLane {
    index: u32,
    route: u32,
    source_name: *const c_char,
    channel_labels: *const c_char,
    channel_mask: u64,
    channel_mask_valid: u32,
    include_pair: u32,
    shown_start: u32,
    shown_count: u32,
    shown_positioned: u32,
    bed_start: u32,
    bed_count: u32,
    positions: *const crate::FrozenPositionTrack,
}

fn valid_group_range(start: u32, count: u32, width: usize) -> Option<Range<usize>> {
    if count == 0 {
        return None;
    }
    let s = start as usize;
    let e = s.checked_add(count as usize)?;
    (e <= width).then_some(s..e)
}

/// The head model for binaural preview at `rate`, made the first time it is needed and kept
/// for later previews at that rate (Windows `Recorder::head_model`).
fn cached_head_model(rate: u32) -> Option<Arc<crate::spatial::HeadModel>> {
    static MODELS: Mutex<Vec<Arc<crate::spatial::HeadModel>>> = Mutex::new(Vec::new());
    let mut models = MODELS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(model) = models.iter().find(|model| model.sample_rate() == rate) {
        return Some(Arc::clone(model));
    }
    let model = Arc::new(crate::spatial::HeadModel::new(rate).ok()?);
    models.push(Arc::clone(&model));
    Some(model)
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_frozen_play_group_selection(
    frozen: *const FrozenCapture,
    slot: *const PreviewSlot,
    lanes: *const BtFrozenGroupSelectionLane,
    count: usize,
    first: u64,
    end: u64,
    force_mono: u32,
    pair_binaural: u32,
    fades: Fades,
    error_out: *mut *mut c_char,
) -> i32 {
    if !error_out.is_null() {
        *error_out = ptr::null_mut();
    }
    let (Some(slot), Some(frozen)) = (slot.as_ref(), frozen.as_ref()) else {
        set_ffi_error(error_out, "Invalid capture lanes");
        return 0;
    };
    let held = frozen.frame_range();
    if lanes.is_null() || !(1..=frozen.track_count()).contains(&count) {
        set_ffi_error(error_out, "Invalid capture lanes");
        return 0;
    }
    if first < held.start || end > held.end || first >= end {
        set_ffi_error(error_out, "The selected audio is unavailable.");
        return 0;
    }
    let lanes = slice::from_raw_parts(lanes, count);
    struct PreparedPreviewEntry<'a> {
        track: crate::FrozenTrack,
        route: ChannelRoute,
        group: Option<PairGroup>,
        /// The lane-native channels of `group`; empty for a mono or stereo lane.
        channels: Range<usize>,
        selected: Option<Vec<usize>>,
        positions: Option<&'a crate::FrozenPositionTrack>,
    }
    let mut entries: Vec<PreparedPreviewEntry<'_>> = Vec::with_capacity(count * 2);
    for lane in lanes {
        let Some(track) = frozen.track(lane.index as usize) else {
            set_ffi_error(error_out, "Invalid capture lanes");
            return 0;
        };
        let Some(route) = ChannelRoute::from_code(lane.route) else {
            set_ffi_error(error_out, "channel route does not match the captured audio");
            return 0;
        };
        let width = track.channels();
        if width <= 2 {
            let sel = if lane.channel_mask_valid == 0 {
                None
            } else {
                let Some(indices) = mask_channels(lane.channel_mask, width) else {
                    set_ffi_error(error_out, "The selected rows do not match this capture");
                    return 0;
                };
                if width == 2 {
                    match indices.as_slice() {
                        [c] => Some(vec![*c]),
                        [0, 1] => None,
                        _ => {
                            set_ffi_error(error_out, "The selected rows do not match this capture");
                            return 0;
                        }
                    }
                } else if indices.is_empty() {
                    set_ffi_error(error_out, "The selected rows do not match this capture");
                    return 0;
                } else {
                    None
                }
            };
            entries.push(PreparedPreviewEntry {
                track,
                route,
                group: None,
                channels: 0..0,
                selected: sel,
                positions: None,
            });
            continue;
        }
        let all_names = channel_names(lane.channel_labels, width);
        let shown_range = if lane.shown_count == 0 {
            0..width
        } else if let Some(range) = valid_group_range(lane.shown_start, lane.shown_count, width) {
            range
        } else {
            set_ffi_error(error_out, "The selected rows do not match this capture");
            return 0;
        };
        let bed_range = if lane.bed_count > 0 {
            let Some(range) = valid_group_range(lane.bed_start, lane.bed_count, width) else {
                set_ffi_error(error_out, "The selected rows do not match this capture");
                return 0;
            };
            Some(range)
        } else if lane.shown_positioned == 0 {
            Some(shown_range.clone())
        } else {
            None
        };
        let explicit_indices = if lane.channel_mask_valid == 0 {
            None
        } else {
            let Some(indices) = mask_channels(lane.channel_mask, width) else {
                set_ffi_error(error_out, "The selected rows do not match this capture");
                return 0;
            };
            Some(indices)
        };
        if lane.channel_mask_valid != 0 {
            let indices = explicit_indices.as_ref().unwrap();
            if indices.is_empty() && lane.include_pair == 0 {
                set_ffi_error(error_out, "The selected rows do not match this capture");
                return 0;
            }
        }
        let shown_is_bed = bed_range.as_ref().is_some_and(|b| *b == shown_range);
        if lane.include_pair != 0 && shown_is_bed {
            // Pair already mixes the entire speaker bed.
            let group = PairGroup::new(shown_range.start, &all_names[shown_range.clone()]);
            entries.push(PreparedPreviewEntry {
                track,
                route,
                group,
                channels: shown_range.clone(),
                selected: None,
                positions: None,
            });
        } else {
            if lane.include_pair != 0 {
                if let Some(bed) = &bed_range {
                    let group = PairGroup::new(bed.start, &all_names[bed.clone()]);
                    entries.push(PreparedPreviewEntry {
                        track: track.clone(),
                        route,
                        group,
                        channels: bed.clone(),
                        selected: None,
                        positions: None,
                    });
                }
            }
            let shown_sel = match explicit_indices {
                None => Some(None),
                Some(indices) => {
                    let in_shown: Vec<usize> =
                        indices.into_iter().filter(|c| shown_range.contains(c)).collect();
                    if in_shown.is_empty() {
                        None
                    } else {
                        Some(Some(in_shown))
                    }
                }
            };
            if let Some(sel) = shown_sel {
                let group = PairGroup::new(shown_range.start, &all_names[shown_range.clone()]);
                let positions = if lane.shown_positioned != 0 {
                    lane.positions.as_ref()
                } else {
                    None
                };
                entries.push(PreparedPreviewEntry {
                    track,
                    route,
                    group,
                    channels: shown_range.clone(),
                    selected: sel,
                    positions,
                });
            }
        }
    }
    if entries.is_empty() || entries.len() > MAX_LANES {
        set_ffi_error(error_out, "The selected rows do not match this capture");
        return 0;
    }
    let has_wide = entries.iter().any(|e| e.group.is_some());
    // A binaural pair, or objects that move in the selection, need the head model of the
    // capture's rate (Windows `Recorder::preview_selection`).
    let moving = entries.iter().any(|entry| {
        entry.positions.is_some_and(|positions| {
            let heard: Vec<usize> = match &entry.selected {
                Some(selected) => selected.clone(),
                None => entry.channels.clone().collect(),
            };
            heard
                .into_iter()
                .any(|channel| !positions.render_keys(channel, first..end).is_empty())
        })
    });
    let model = if has_wide && (pair_binaural != 0 || moving) {
        let Some(model) = cached_head_model(frozen.sample_rate()) else {
            set_ffi_error(error_out, "Could not prepare binaural head model");
            return 0;
        };
        Some(model)
    } else {
        None
    };
    let playback: Vec<PreviewLane<'_>> = entries
        .iter()
        .map(|entry| PreviewLane {
            track: &entry.track,
            route: entry.route,
            group: entry.group.as_ref(),
            selected: entry.selected.as_deref(),
        })
        .collect();
    let lane_positions: Vec<Option<&crate::FrozenPositionTrack>> =
        entries.iter().map(|entry| entry.positions).collect();
    let Some(preview) = Preview::with_lane_positions(
        &playback,
        first..end,
        model,
        force_mono != 0,
        fades,
        &lane_positions,
    ) else {
        set_ffi_error(error_out, "The selected audio is unavailable.");
        return 0;
    };
    slot.play(preview);
    1
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_frozen_export_group_selection(
    frozen: *const FrozenCapture,
    lanes: *const BtFrozenGroupSelectionLane,
    count: usize,
    first: u64,
    end: u64,
    directory: *const c_char,
    sample_rate: u32,
    bits_per_sample: u32,
    force_mono: u32,
    wide_mode: u32,
    pair_binaural: u32,
    fades: Fades,
    objects_in_mono_out: *mut u32,
    error_out: *mut *mut c_char,
) -> *mut Vec<CString> {
    if !error_out.is_null() {
        *error_out = ptr::null_mut();
    }
    if !objects_in_mono_out.is_null() {
        *objects_in_mono_out = 0;
    }
    let Some(frozen) = frozen.as_ref() else {
        set_ffi_error(error_out, "Invalid capture lanes");
        return ptr::null_mut();
    };
    let held = frozen.frame_range();
    if lanes.is_null() || !(1..=frozen.track_count()).contains(&count) {
        set_ffi_error(error_out, "Invalid capture lanes");
        return ptr::null_mut();
    }
    if first < held.start || end > held.end || first >= end {
        set_ffi_error(error_out, "expected aligned nonempty audio lanes");
        return ptr::null_mut();
    }
    let Some(directory) = export_directory(directory) else {
        set_ffi_error(error_out, "Invalid export folder");
        return ptr::null_mut();
    };
    let Some(sample_format) = WavSampleFormat::from_bits_per_sample(bits_per_sample) else {
        set_ffi_error(error_out, "Invalid WAV encoding");
        return ptr::null_mut();
    };
    let mode = match wide_mode {
        0 => WideExportMode::BinauralFiles,
        1 => WideExportMode::MonoFiles,
        2 => WideExportMode::Multichannel,
        3 => WideExportMode::BinauralMix,
        _ => {
            set_ffi_error(error_out, "Invalid multichannel export mode");
            return ptr::null_mut();
        }
    };
    let lanes = slice::from_raw_parts(lanes, count);
    let mut tracks = Vec::with_capacity(count);
    let mut routes = Vec::with_capacity(count);
    let mut labels = Vec::with_capacity(count);
    let mut subsets: Vec<Option<Vec<usize>>> = Vec::with_capacity(count);
    let mut narrowed_stereo: Vec<Option<usize>> = Vec::with_capacity(count);
    for lane in lanes {
        let Some(track) = frozen.track(lane.index as usize) else {
            set_ffi_error(error_out, "Invalid capture lanes");
            return ptr::null_mut();
        };
        let Some(route) = ChannelRoute::from_code(lane.route) else {
            set_ffi_error(error_out, "channel route does not match the captured audio");
            return ptr::null_mut();
        };
        let Some(name) = source_name_arg(lane.source_name) else {
            set_ffi_error(error_out, "Invalid source name");
            return ptr::null_mut();
        };
        let width = track.channels();
        let mut narrow = None;
        let subset = if lane.channel_mask_valid == 0 {
            None
        } else {
            let Some(indices) = mask_channels(lane.channel_mask, width) else {
                set_ffi_error(error_out, "The selected rows do not match this capture");
                return ptr::null_mut();
            };
            if indices.is_empty() && (width <= 2 || lane.include_pair == 0) {
                set_ffi_error(error_out, "The selected rows do not match this capture");
                return ptr::null_mut();
            }
            if width == 2 && indices.len() == 1 {
                narrow = Some(indices[0]);
            }
            Some(indices)
        };
        tracks.push(track);
        routes.push(route);
        let letter = if frozen.track_count() > 1 {
            Lane::from_index(lane.index as usize)
        } else {
            None
        };
        labels.push(TakeLabel::new(name, letter));
        subsets.push(subset);
        narrowed_stereo.push(narrow);
    }
    let start = frozen.frame_range().start;
    let frames = (first - start) as usize..(end - start) as usize;
    let profile = WavExportProfile { sample_rate, sample_format };
    let mut paths = Vec::new();
    let mut any_objects_in_mono = false;
    let result: std::io::Result<()> = (|| {
        if tracks.iter().all(|track| track.channels() <= 2) && narrowed_stereo.iter().all(Option::is_none) {
            let references: Vec<_> = tracks.iter().collect();
            paths = export_routed_track_refs(
                directory, &references, frames, profile, &routes, force_mono != 0, None, fades, &labels,
            )?;
        } else {
            for (index, track) in tracks.iter().enumerate() {
                let lane = &lanes[index];
                let written = if track.channels() > 2 {
                    let width = track.channels();
                    let all_names = channel_names(lane.channel_labels, width);
                    let shown_range = if lane.shown_count == 0 {
                        0..width
                    } else if let Some(range) = valid_group_range(lane.shown_start, lane.shown_count, width) {
                        range
                    } else {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "The selected rows do not match this capture",
                        ));
                    };
                    let shown_labels = all_names[shown_range.clone()].to_vec();
                    let shown_group = WideExportGroup {
                        channels: shown_range,
                        labels: &shown_labels,
                        positioned: lane.shown_positioned != 0,
                    };
                    let bed_range = if lane.bed_count > 0 {
                        let Some(range) = valid_group_range(lane.bed_start, lane.bed_count, width) else {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "The selected rows do not match this capture",
                            ));
                        };
                        Some(range)
                    } else {
                        None
                    };
                    let bed_labels = bed_range
                        .as_ref()
                        .map(|r| all_names[r.clone()].to_vec())
                        .unwrap_or_default();
                    let bed_group = bed_range.map(|r| WideExportGroup {
                        channels: r,
                        labels: &bed_labels,
                        positioned: false,
                    });
                    let exported = export_wide_track(
                        directory,
                        track,
                        frames.clone(),
                        fades,
                        profile,
                        &labels[index],
                        Some(&shown_group),
                        mode,
                        &WideRowExport {
                            channels: subsets[index].as_deref(),
                            pair: lane.include_pair != 0,
                            pair_binaural: pair_binaural != 0,
                            bed: bed_group.as_ref(),
                            positions: lane.positions.as_ref(),
                        },
                    )?;
                    any_objects_in_mono |= exported.objects_in_mono;
                    exported.paths
                } else if let Some(channel) = narrowed_stereo[index] {
                    crate::export_channel_range_with_profile(
                        directory,
                        track,
                        frames.clone(),
                        channel..channel + 1,
                        &[],
                        profile,
                        crate::ChannelExportMode::Multichannel,
                        fades,
                        &labels[index],
                    )?
                } else {
                    export_routed_track_refs(
                        directory,
                        &[track],
                        frames.clone(),
                        profile,
                        &routes[index..=index],
                        force_mono != 0,
                        None,
                        fades,
                        &labels[index..=index],
                    )?
                };
                paths.extend(written);
            }
        }
        Ok(())
    })();
    if let Err(err) = result {
        remove_export_paths(&paths);
        set_ffi_error(error_out, err.to_string());
        return ptr::null_mut();
    }
    if !objects_in_mono_out.is_null() {
        *objects_in_mono_out = u32::from(any_objects_in_mono);
    }
    match encode_export_paths(paths) {
        Some(encoded) => Box::into_raw(Box::new(encoded)),
        None => {
            set_ffi_error(error_out, "Export path is not valid UTF-8");
            ptr::null_mut()
        }
    }
}

#[no_mangle]
extern "C" fn bt_lane_letter(index: u32) -> *mut c_char {
    Lane::from_index(index as usize)
        .and_then(|lane| CString::new(lane.letter()).ok())
        .map_or(ptr::null_mut(), CString::into_raw)
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn labelled_ffi_exports_name_files_after_their_sources_and_lanes() {
        let directory = std::env::temp_dir()
            .join(format!("bt-labelled-ffi-{}", std::process::id()));
        let folder = CString::new(directory.to_str().unwrap()).unwrap();
        let lane = Snapshot { sample_rate: 48_000, channels: 1, samples: vec![0.25; 4_800], first_frame: 100 };
        let name = |path: *mut c_char| unsafe {
            assert!(!path.is_null());
            let path = CString::from_raw(path);
            Path::new(path.to_str().unwrap()).file_name().unwrap().to_str().unwrap().to_owned()
        };
        let spotify = CString::new("Spotify").unwrap();
        let mic = CString::new("系統 mic").unwrap();
        unsafe {
            let (mut a, mut b) = (ptr::null_mut(), ptr::null_mut());
            assert_eq!(
                bt_snapshots_export_pair_labelled_frames(
                    &lane, &lane, 100, 4_900, folder.as_ptr(), 48_000, 32, 5, 5, 0, 0, 0,
                    spotify.as_ptr(), mic.as_ptr(), &mut a, &mut b,
                ),
                1
            );
            let (a, b) = (name(a), name(b));
            assert!(a.starts_with("Backtracking_Spotify_A_"), "{a}");
            assert!(b.starts_with("Backtracking_系統 mic_B_"), "{b}");

            // One lane on its own keeps its letter; a single source shows none.
            let single = bt_snapshot_export_labelled_frames(
                &lane, 100, 4_900, folder.as_ptr(), 48_000, 32, 5, 0, 0, 0, mic.as_ptr(), 2,
            );
            assert!(name(single).starts_with("Backtracking_系統 mic_B_"));
            let single = bt_snapshot_export_labelled_frames(
                &lane, 100, 4_900, folder.as_ptr(), 48_000, 32, 5, 0, 0, 0, spotify.as_ptr(), 0,
            );
            let single = name(single);
            assert!(single.starts_with("Backtracking_Spotify_2"), "{single}");

            // An unknown lane and a name that is not UTF-8 write nothing.
            let bad = [0xffu8, 0x00];
            for (source, lane_code) in [(spotify.as_ptr(), 65), (bad.as_ptr().cast(), 1)] {
                assert!(bt_snapshot_export_labelled_frames(
                    &lane, 100, 4_900, folder.as_ptr(), 48_000, 32, 5, 0, 0, 0, source, lane_code,
                )
                .is_null());
            }
        }
        let written = std::fs::read_dir(&directory).unwrap().count();
        assert_eq!(written, 4);
        std::fs::remove_dir_all(directory).unwrap();

        // Settings show how those names begin.
        let prefix = |name: *const c_char, lane| unsafe {
            let text = bt_take_file_prefix(name, lane);
            (!text.is_null()).then(|| CString::from_raw(text).into_string().unwrap())
        };
        assert_eq!(prefix(mic.as_ptr(), 2).as_deref(), Some("Backtracking_系統 mic_B_"));
        assert_eq!(prefix(spotify.as_ptr(), 0).as_deref(), Some("Backtracking_Spotify_"));
        // Without a source name the lane follows the take, so the beginning is the same.
        assert_eq!(prefix(ptr::null(), 1).as_deref(), Some("Backtracking_"));
        assert_eq!(prefix(spotify.as_ptr(), 64).as_deref(), Some("Backtracking_Spotify_BL_"));
        assert_eq!(prefix(spotify.as_ptr(), 65), None);
    }

    #[test]
    fn cleanup_ffi_is_off_unless_an_offered_age_is_chosen_and_never_cleans_a_withheld_folder() {
        unsafe {
            let schedule = bt_cleanup_new(15);
            assert_eq!(bt_cleanup_days(schedule), 0);
            assert_eq!(bt_cleanup_due(schedule), 0);
            assert_eq!(bt_cleanup_poll(schedule, ptr::null(), ptr::null(), 0), -2);

            bt_cleanup_set_days(schedule, 30);
            assert_eq!(bt_cleanup_days(schedule), 30);
            // A new choice waits before its first pass, so nothing is due or running yet.
            assert_eq!(bt_cleanup_due(schedule), 0);
            let folder = CString::new(std::env::temp_dir().to_str().unwrap()).unwrap();
            assert_eq!(bt_cleanup_poll(schedule, folder.as_ptr(), ptr::null(), 0), -2);
            bt_cleanup_set_days(schedule, 0);
            assert_eq!(bt_cleanup_days(schedule), 0);
            bt_cleanup_free(schedule);

            assert_eq!(bt_same_folder(folder.as_ptr(), folder.as_ptr()), 1);
            let other = CString::new(std::env::temp_dir().join("bt-other").to_str().unwrap()).unwrap();
            assert_eq!(bt_same_folder(folder.as_ptr(), other.as_ptr()), 0);
            assert_eq!(bt_same_folder(ptr::null(), folder.as_ptr()), 0);
        }
    }

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
    fn system_lanes_record_silence_where_a_preview_may_have_been_heard() {
        unsafe {
            let core = bt_tracks_new(1_000., 1, 1, 10);
            // Never the gate of a Backtracking that is recording on this computer.
            let gate: &'static PreviewGate = Box::leak(Box::new(PreviewGate::local()));
            (*core).gate = gate;
            bt_tracks_set_retain_silence(core, 1);
            bt_tracks_set_preview_silence(core, 0, 1);
            bt_tracks_set_preview_silence(core, 7, 1);
            // A preview was heard from 50 ms to 80 ms into packets captured at one second.
            gate.hold(1_050_000_000, 1_080_000_000);
            let a = [0.5; 100];
            let b = [0.25; 100];
            assert_eq!(bt_tracks_push_interleaved(core, 0, a.as_ptr(), 100, 1_000_000_000, 1_000.), 100);
            assert_eq!(bt_tracks_push_interleaved(core, 1, b.as_ptr(), 100, 1_000_000_000, 1_000.), 100);
            bt_tracks_pause(core);
            let first = bt_tracks_snapshot(core, 0);
            let second = bt_tracks_snapshot(core, 1);
            let expected: Vec<f32> =
                (0..100).map(|frame| if (50..80).contains(&frame) { 0.0 } else { 0.5 }).collect();
            assert_eq!((*first).samples, expected);
            // Lane B records something else, so it keeps all of it.
            assert_eq!((*second).samples, b);
            drop(Box::from_raw(first));
            drop(Box::from_raw(second));
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

    #[test]
    fn active_channels_and_selection_ffi_cover_subsets_pair_only_and_errors() {
        let directory = std::env::temp_dir()
            .join(format!("bt-selection-ffi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let folder = CString::new(directory.to_str().unwrap()).unwrap();
        let labels = CString::new("FL\tFR\tFC\tLFE\tLs\tRs").unwrap();
        let name = CString::new("Synth").unwrap();
        let widths = [6u32, 2u32];
        unsafe {
            let core = bt_tracks_new_lanes(48_000.0, widths.as_ptr(), widths.len(), 10, 1 << 30);
            assert!(!core.is_null());
            bt_tracks_set_retain_silence(core, 1);
            // Push 480 frames where lane 0 has audio on FL (ch0), FR (ch1), and Ls (ch4) only.
            let mut wide = vec![0.0f32; 480 * 6];
            for f in 0..480 {
                wide[f * 6 + 0] = 0.25;
                wide[f * 6 + 1] = -0.25;
                wide[f * 6 + 4] = 0.15;
            }
            let stereo = vec![0.2f32; 480 * 2];
            assert_eq!(bt_tracks_push_interleaved(core, 0, wide.as_ptr(), 480, 1_000_000_000, 48_000.0), 480);
            assert_eq!(bt_tracks_push_interleaved(core, 1, stereo.as_ptr(), 480, 1_000_000_000, 48_000.0), 480);
            bt_tracks_pause(core);
            assert_eq!(bt_tracks_active_channels(core, 0), (1 << 0) | (1 << 1) | (1 << 4));
            assert_eq!(bt_tracks_active_channels(core, 1), 0);

            let first = bt_tracks_first_frame(core);
            let end = bt_tracks_end_frame(core);
            let frozen = bt_tracks_freeze(core, first, end);
            assert!(!frozen.is_null());
            assert_eq!(bt_frozen_active_channels(frozen, 0), (1 << 0) | (1 << 1) | (1 << 4));
            assert_eq!(bt_frozen_active_channels(frozen, 1), 0);

            // Export cross-row subset: lane 0 channels 0 & 4 (no pair) + lane 1 right channel only.
            let sel = [
                BtFrozenSelectionLane {
                    index: 0,
                    route: 7,
                    source_name: name.as_ptr(),
                    channel_labels: labels.as_ptr(),
                    channel_mask: (1 << 0) | (1 << 4),
                    channel_mask_valid: 1,
                    include_pair: 0,
                },
                BtFrozenSelectionLane {
                    index: 1,
                    route: 5,
                    source_name: name.as_ptr(),
                    channel_labels: ptr::null(),
                    channel_mask: 1 << 1,
                    channel_mask_valid: 1,
                    include_pair: 0,
                },
            ];
            let mut err: *mut c_char = ptr::null_mut();
            let paths = bt_frozen_export_selection(
                frozen, sel.as_ptr(), sel.len(), first, end, folder.as_ptr(),
                48_000, 32, 0, 1, 0, Fades::NONE, &mut err,
            );
            assert!(err.is_null());
            assert!(!paths.is_null());
            assert_eq!(bt_export_paths_count(paths), 3);
            bt_export_paths_free(paths);

            // Export pair-only for hide-silent when only FL/FR are active.
            let pair_only = [BtFrozenSelectionLane {
                index: 0,
                route: 7,
                source_name: name.as_ptr(),
                channel_labels: labels.as_ptr(),
                channel_mask: 0,
                channel_mask_valid: 1,
                include_pair: 1,
            }];
            let paths = bt_frozen_export_selection(
                frozen, pair_only.as_ptr(), 1, first, end, folder.as_ptr(),
                48_000, 32, 0, 1, 0, Fades::NONE, &mut err,
            );
            assert!(err.is_null());
            assert!(!paths.is_null());
            assert_eq!(bt_export_paths_count(paths), 1);
            let only_name = CStr::from_ptr(bt_export_path(paths, 0)).to_str().unwrap().to_owned();
            assert!(only_name.ends_with("-Stereo.wav"), "{only_name}");
            bt_export_paths_free(paths);

            // Play selection subset succeeds.
            let slot = PreviewSlot::new();
            assert_eq!(
                bt_frozen_play_selection(
                    frozen, &slot, sel.as_ptr(), sel.len(), first, end, 0, 0, Fades::NONE, &mut err,
                ),
                1
            );
            assert!(err.is_null());
            slot.clear();

            // Invalid mask returns error_out.
            let bad = [BtFrozenSelectionLane {
                index: 0,
                route: 7,
                source_name: name.as_ptr(),
                channel_labels: labels.as_ptr(),
                channel_mask: 1 << 9,
                channel_mask_valid: 1,
                include_pair: 0,
            }];
            let null_paths = bt_frozen_export_selection(
                frozen, bad.as_ptr(), 1, first, end, folder.as_ptr(),
                48_000, 32, 0, 1, 0, Fades::NONE, &mut err,
            );
            assert!(null_paths.is_null());
            assert!(!err.is_null());
            let err_str = CString::from_raw(err).into_string().unwrap();
            assert_eq!(err_str, "The selected rows do not match this capture");

            bt_frozen_free(frozen);
            bt_tracks_free(core);
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn group_preview_head_models_are_made_once_per_sample_rate() {
        let first = cached_head_model(48_000).unwrap();
        let again = cached_head_model(48_000).unwrap();
        let other = cached_head_model(44_100).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert!(!Arc::ptr_eq(&first, &other));
        assert_eq!((first.sample_rate(), other.sample_rate()), (48_000, 44_100));
    }

    /// Objects whose positions move in the selection make the whole preview binaural even when
    /// the pair is set to Stereo, as on Windows; without positions the setting still decides.
    #[test]
    fn group_preview_with_moving_objects_is_binaural_in_stereo_mode() {
        let all_labels = CString::new("FL\tFR\tFC\tObject 1\tObject 2").unwrap();
        let name = CString::new("Atmos").unwrap();
        let widths = [5u32];
        let frames = 4_800usize;
        unsafe {
            let core = bt_tracks_new_lanes(48_000.0, widths.as_ptr(), widths.len(), 10, 1 << 30);
            assert!(!core.is_null());
            bt_tracks_set_retain_silence(core, 1);
            let mut wide = vec![0.0f32; frames * 5];
            for f in 0..frames {
                let t = f as f32 / 48_000.0;
                wide[f * 5] = 0.3 * (t * 440.0 * std::f32::consts::TAU).sin();
                wide[f * 5 + 1] = 0.2 * (t * 660.0 * std::f32::consts::TAU).sin();
                wide[f * 5 + 2] = 0.1 * (t * 880.0 * std::f32::consts::TAU).sin();
                wide[f * 5 + 3] = 0.4 * (t * 330.0 * std::f32::consts::TAU).sin();
            }
            assert_eq!(
                bt_tracks_push_interleaved(core, 0, wide.as_ptr(), frames as u32, 1_000_000_000, 48_000.0),
                frames as u32
            );
            bt_tracks_pause(core);
            let first = bt_tracks_first_frame(core);
            let end = bt_tracks_end_frame(core);
            let frozen = bt_tracks_freeze(core, first, end);
            assert!(!frozen.is_null());
            let mut track = crate::PositionTrack::with_memory_limit(1024 * 1024);
            for (frame, x) in [(first, -0.8), (first + 2_400, 0.8)] {
                track.insert(3, crate::PositionKey {
                    frame,
                    position: crate::Position { x, y: 0.5, z: 0.0, size: 0.0, gain: 1.0 },
                });
            }
            let moving = track.freeze();
            let still = crate::PositionTrack::with_memory_limit(1024 * 1024).freeze();
            let render = |positions: &crate::FrozenPositionTrack, pair_binaural: u32| {
                let lane = [BtFrozenGroupSelectionLane {
                    index: 0,
                    route: 7,
                    source_name: name.as_ptr(),
                    channel_labels: all_labels.as_ptr(),
                    channel_mask: 1 << 3,
                    channel_mask_valid: 1,
                    include_pair: 1,
                    shown_start: 3,
                    shown_count: 2,
                    shown_positioned: 1,
                    bed_start: 0,
                    bed_count: 3,
                    positions,
                }];
                let slot = PreviewSlot::new();
                let mut err: *mut c_char = ptr::null_mut();
                assert_eq!(
                    bt_frozen_play_group_selection(
                        frozen, &slot, lane.as_ptr(), 1, first, end, 0, pair_binaural, Fades::NONE, &mut err,
                    ),
                    1
                );
                assert!(err.is_null());
                let mut out = vec![[0.0f32; 2]; frames];
                let rendered = slot.render_with(frames, 48_000, |i, frame| out[i] = frame);
                assert!(rendered.frames > 1_000, "rendered {}", rendered.frames);
                slot.clear();
                out
            };
            let differs = |a: &[[f32; 2]], b: &[[f32; 2]]| {
                a.iter().zip(b).any(|(x, y)| (x[0] - y[0]).abs() > 1e-4 || (x[1] - y[1]).abs() > 1e-4)
            };
            // Without positions the pair setting decides between a stereo and a binaural render.
            assert!(differs(&render(&still, 0), &render(&still, 1)));
            // With moving objects the Stereo setting renders exactly as Binaural does.
            assert!(!differs(&render(&moving, 0), &render(&moving, 1)),
                "moving objects in Stereo mode were not rendered binaurally");
            bt_frozen_free(frozen);
            bt_tracks_free(core);
        }
    }

    #[test]
    fn frame_clock_bed_waveform_and_group_positions_ffi_work_end_to_end() {
        let directory = std::env::temp_dir()
            .join(format!("bt-group-pos-ffi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let folder = CString::new(directory.to_str().unwrap()).unwrap();
        let all_labels = CString::new("FL\tFR\tFC\tObject 1\tObject 2").unwrap();
        let bed_labels = CString::new("FL\tFR\tFC").unwrap();
        let name = CString::new("Atmos").unwrap();
        let widths = [5u32];
        unsafe {
            let core = bt_tracks_new_lanes(48_000.0, widths.as_ptr(), widths.len(), 10, 1 << 30);
            assert!(!core.is_null());
            bt_tracks_set_retain_silence(core, 1);
            assert_eq!(bt_tracks_enable_frame_clock(core, 0), 1);
            assert_eq!(bt_tracks_enable_frame_clock(core, 9), 0);

            let mut wide = vec![0.0f32; 960 * 5];
            for f in 0..960 {
                wide[f * 5 + 0] = 0.2;
                wide[f * 5 + 1] = -0.2;
                wide[f * 5 + 3] = 0.4;
                wide[f * 5 + 4] = -0.3;
            }
            assert_eq!(bt_tracks_push_interleaved(core, 0, wide.as_ptr(), 960, 1_000_000_000, 48_000.0), 960);
            bt_tracks_pause(core);

            let first = bt_tracks_first_frame(core);
            let end = bt_tracks_end_frame(core);
            assert_eq!(end - first, 960);

            let mut mapped_frame = 0u64;
            assert_eq!(bt_tracks_retained_frame_at(core, 0, 1_010_000_000, &mut mapped_frame), 1);
            assert!(mapped_frame >= first && mapped_frame < end, "mapped={mapped_frame} range={first}..{end}");
            // Future timestamp is Pending (-1).
            assert_eq!(bt_tracks_retained_frame_at(core, 0, 2_000_000_000, &mut mapped_frame), -1);
            let mut evicted = 0u64;
            assert_eq!(bt_tracks_take_frame_clock_eviction(core, 0, &mut evicted), 0);

            let frozen = bt_tracks_freeze(core, first, end);
            assert!(!frozen.is_null());

            // Bed-only waveform reads FL/FR/FC (0..3) and ignores objects (3..5).
            let mut min_bins = [0.0f32; 4];
            let mut max_bins = [0.0f32; 4];
            bt_frozen_waveform_bed(
                frozen, 0, 67, 0, 3, first, end,
                min_bins.as_mut_ptr(), max_bins.as_mut_ptr(), 4, bed_labels.as_ptr(),
            );
            assert!(max_bins.iter().all(|&v| v > 0.05));

            // Create positions only for Object 1 (channel 3), leaving Object 2 (channel 4) unpositioned.
            let mut pos_track = crate::PositionTrack::with_memory_limit(1024 * 1024);
            pos_track.insert(3, crate::PositionKey {
                frame: first,
                position: crate::Position { x: 0.5, y: 0.5, z: 0.0, size: 0.0, gain: 1.0 },
            });
            let frozen_pos = pos_track.freeze();

            // Export shown group Objects (3..5, positioned=1) with bed (0..3), BinauralFiles (0), pair included.
            let group_lane = [BtFrozenGroupSelectionLane {
                index: 0,
                route: 7,
                source_name: name.as_ptr(),
                channel_labels: all_labels.as_ptr(),
                channel_mask: (1 << 3) | (1 << 4),
                channel_mask_valid: 1,
                include_pair: 1,
                shown_start: 3,
                shown_count: 2,
                shown_positioned: 1,
                bed_start: 0,
                bed_count: 3,
                positions: &frozen_pos,
            }];
            let mut objects_in_mono = 0u32;
            let mut err: *mut c_char = ptr::null_mut();
            let paths = bt_frozen_export_group_selection(
                frozen, group_lane.as_ptr(), 1, first, end, folder.as_ptr(),
                48_000, 32, 0, 0, 0, Fades::NONE, &mut objects_in_mono, &mut err,
            );
            assert!(err.is_null());
            assert!(!paths.is_null());
            // Object 2 had no position keys, so objects_in_mono must be 1.
            assert_eq!(objects_in_mono, 1);
            let mut out_names = Vec::new();
            for i in 0..bt_export_paths_count(paths) {
                out_names.push(CStr::from_ptr(bt_export_path(paths, i)).to_str().unwrap().to_owned());
            }
            assert!(out_names.iter().any(|p| p.ends_with("-Positions.json")), "{out_names:?}");
            assert!(out_names.iter().any(|p| p.ends_with("-Stereo.wav")), "{out_names:?}");
            assert!(out_names.iter().any(|p| p.contains("Object 1_binaural.wav")), "{out_names:?}");
            assert!(out_names.iter().any(|p| p.contains("Object 2.wav")), "{out_names:?}");
            bt_export_paths_free(paths);

            // Preview positioned group + bed pair succeeds.
            let slot = PreviewSlot::new();
            assert_eq!(
                bt_frozen_play_group_selection(
                    frozen, &slot, group_lane.as_ptr(), 1, first, end, 0, 0, Fades::NONE, &mut err,
                ),
                1
            );
            assert!(err.is_null());
            slot.clear();

            bt_frozen_free(frozen);
            bt_tracks_free(core);
        }
        let _ = std::fs::remove_dir_all(&directory);
    }
}

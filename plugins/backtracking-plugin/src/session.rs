use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    AutoResumeDelay, Capture, CaptureState, ChannelRoute, DurationChoice, MeterReadings,
    SenderEndpoint, Snapshot, TimedAudioQueue, WavExportProfile, WaveformView, capture_clock_ns,
    export_routed_tracks_with_profile,
};

use crate::settings::{CaptureInput, MeterMode, SessionSettings, SettingsStore, WaveformLayout};

pub(crate) const WAVEFORM_BINS: usize = 512;

#[derive(Clone)]
pub(crate) struct WaveformPeaks {
    pub(crate) minimum: Vec<f32>,
    pub(crate) maximum: Vec<f32>,
}

impl Default for WaveformPeaks {
    fn default() -> Self {
        Self {
            minimum: vec![0.0; WAVEFORM_BINS],
            maximum: vec![0.0; WAVEFORM_BINS],
        }
    }
}

pub(crate) fn waveform_views(layout: WaveformLayout, channels: usize) -> &'static [WaveformView] {
    if layout == WaveformLayout::Stereo && channels == 2 {
        &[WaveformView::Left, WaveformView::Right]
    } else {
        &[WaveformView::Merged]
    }
}

#[derive(Clone, Default)]
pub(crate) struct WaveformTimeline {
    pub(crate) epoch: u64,
    pub(crate) frames: Range<u64>,
    pub(crate) sample_rate: u32,
    pub(crate) channels: usize,
    pub(crate) b_channels: Option<usize>,
    pub(crate) bar_markers: Vec<u64>,
}

#[derive(Clone)]
pub(crate) struct EditingSnapshot {
    pub(crate) token: u64,
    pub(crate) timeline: Arc<WaveformTimeline>,
    pub(crate) snapshots: Arc<[Snapshot]>,
}

#[derive(Default)]
struct PublishedState {
    epoch: u64,
    paused: bool,
    recording: bool,
    input_expires_at: Option<Instant>,
    meter: MeterReadings,
    sidechain: bool,
    host_track_name: Option<String>,
    editing: Option<EditingSnapshot>,
    editing_pending: bool,
}

enum EditorAction {
    Begin,
    Selection(Option<Range<u64>>),
    End(bool),
    Cancel,
    Gesture(bool),
}

struct EditorCommand {
    epoch: u64,
    token: u64,
    action: EditorAction,
}

impl WaveformTimeline {
    pub(crate) fn duration(&self) -> f32 {
        (self.frames.end - self.frames.start) as f32 / self.sample_rate.max(1) as f32
    }
}

struct Consumer {
    queue: Arc<TimedAudioQueue>,
    capture: Capture,
    scratch: Vec<f32>,
    routed: Vec<f32>,
    bars: Vec<usize>,
    sample_rate: u32,
    main_channels: usize,
    sidechain_channels: usize,
    input: CaptureInput,
    waveform: WaveformCache,
    last_input: Option<Instant>,
    input_timeout: Duration,
}

#[derive(Default)]
struct WaveformCache {
    revision: Option<u64>,
    layout: WaveformLayout,
    range: Range<u64>,
    peaks: [WaveformPeaks; 2],
}

impl Consumer {
    fn has_fresh_input(&self) -> bool {
        self.last_input
            .is_some_and(|last| last.elapsed() < self.input_timeout)
    }

    fn discard(&mut self) {
        self.queue.discard_pending(&mut self.scratch);
    }

    fn capture_for(&self, input: CaptureInput, retention: DurationChoice) -> Option<Capture> {
        let main = self.main_channels;
        let sidechain = self.sidechain_channels;
        let (a, b) = match input {
            CaptureInput::Main => (main, None),
            CaptureInput::Sidechain => (sidechain, None),
            CaptureInput::Both => (main, Some(sidechain)),
        };
        Capture::new_tracks(self.sample_rate, a, b, retention).ok()
    }

    fn drain(&mut self, max_blocks: usize) {
        let mut received_input = false;
        for _ in 0..max_blocks.min(self.queue.pending_packets()) {
            let Some((packet, tags)) = self.queue.pop_tagged(&mut self.scratch) else {
                break;
            };
            received_input = true;
            fill_bar_offsets(&mut self.bars, packet.frames, tags);
            let samples = route_block(
                &self.scratch,
                self.main_channels,
                self.sidechain_channels,
                self.input,
                &mut self.routed,
            );
            let _ = self
                .capture
                .push_interleaved_with_markers(samples, &self.bars);
        }
        if received_input {
            self.last_input = Some(Instant::now());
        }
    }

    fn waveform(
        &mut self,
        layout: WaveformLayout,
        range: Range<u64>,
        bins: usize,
        peaks: &mut [WaveformPeaks; 2],
    ) {
        let cache = &mut self.waveform;
        let revision = self.capture.revision();
        if cache.revision != Some(revision) || cache.layout != layout || cache.range != range
            || cache.peaks[0].minimum.len() != bins
        {
            let a = self.capture.track_channels(0).unwrap_or(1);
            let dual = self.capture.track_channels(1).is_some();
            let views = waveform_views(layout, a);
            for (index, trace) in cache.peaks.iter_mut().enumerate() {
                trace.minimum.resize(bins, 0.0);
                trace.maximum.resize(bins, 0.0);
                let route = if dual {
                    Some((index, WaveformView::Merged))
                } else {
                    views.get(index).map(|&view| (0, view))
                };
                if let Some((lane, view)) = route {
                    self.capture.track_waveform_range(
                        lane, view, range.clone(), &mut trace.minimum, &mut trace.maximum,
                    );
                } else {
                    trace.minimum.fill(0.0);
                    trace.maximum.fill(0.0);
                }
            }
            cache.revision = Some(revision);
            cache.layout = layout;
            cache.range = range;
        }
        for (peaks, cached) in peaks.iter_mut().zip(&cache.peaks) {
            peaks.minimum.clone_from(&cached.minimum);
            peaks.maximum.clone_from(&cached.maximum);
        }
    }
}

fn route_block<'a>(
    samples: &'a [f32],
    main: usize,
    sidechain: usize,
    input: CaptureInput,
    routed: &'a mut Vec<f32>,
) -> &'a [f32] {
    if sidechain == 0 || input == CaptureInput::Both {
        return samples;
    }
    let route = if input == CaptureInput::Main { 0..main } else { main..main + sidechain };
    routed.clear();
    for frame in samples.chunks_exact(main + sidechain) {
        routed.extend_from_slice(&frame[route.clone()]);
    }
    routed
}

fn fill_bar_offsets<T>(offsets: &mut Vec<T>, frames: usize, tags: [u64; 2])
where
    T: TryFrom<usize>,
{
    offsets.clear();
    let mut frame = f64::from_bits(tags[0]);
    let interval = f64::from_bits(tags[1]);
    if !interval.is_finite() || interval < 1. {
        return;
    }
    while frame.round() < frames as f64 {
        if let Ok(offset) = T::try_from(frame.round() as usize) {
            offsets.push(offset);
        }
        frame += interval;
    }
}

struct State {
    retention: DurationChoice,
    idle_resume: AutoResumeDelay,
    consumer: Option<Consumer>,
    timeline_epoch: u64,
    paused: bool,
    gesture_active: bool,
    selection: Option<Range<u64>>,
    editing: Option<EditingSnapshot>,
    editing_pending: bool,
    edit_serial: u64,
    host_track_name: Option<String>,
    last_interaction: Instant,
}

pub(crate) struct Session {
    settings: SettingsStore,
    state: Mutex<State>,
    published: Mutex<PublishedState>,
    editor_commands: mpsc::Sender<EditorCommand>,
    editor_requests: Mutex<mpsc::Receiver<EditorCommand>>,
    edit_requested: AtomicBool,
    edit_token: AtomicU64,
    snapshot_commands: mpsc::Sender<u64>,
    snapshot_requests: Mutex<mpsc::Receiver<u64>>,
    accepting: AtomicBool,
    writer_active: AtomicBool,
    dropped_frames: AtomicUsize,
    snapshot_result: Arc<Mutex<Option<SnapshotResult>>>,
}

type SnapshotResult = (u64, Range<u64>, Result<Vec<PathBuf>, String>);

impl Session {
    pub(crate) fn new(settings: SettingsStore) -> Self {
        let (retention, idle_resume) = {
            let settings = settings.lock();
            (settings.retention, settings.idle_resume)
        };
        let (editor_commands, editor_requests) = mpsc::channel();
        let (snapshot_commands, snapshot_requests) = mpsc::channel();
        Self {
            settings,
            state: Mutex::new(State {
                retention,
                idle_resume,
                consumer: None,
                timeline_epoch: 0,
                paused: false,
                gesture_active: false,
                selection: None,
                editing: None,
                editing_pending: false,
                edit_serial: 0,
                host_track_name: None,
                last_interaction: Instant::now(),
            }),
            published: Mutex::new(PublishedState::default()),
            editor_commands,
            editor_requests: Mutex::new(editor_requests),
            edit_requested: AtomicBool::new(false),
            edit_token: AtomicU64::new(0),
            snapshot_commands,
            snapshot_requests: Mutex::new(snapshot_requests),
            accepting: AtomicBool::new(false),
            writer_active: AtomicBool::new(false),
            dropped_frames: AtomicUsize::new(0),
            snapshot_result: Arc::new(Mutex::new(None)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn published(&self) -> MutexGuard<'_, PublishedState> {
        self.published.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn publish(&self, state: &State) {
        let consumer = state.consumer.as_ref();
        let fresh = consumer.is_some_and(Consumer::has_fresh_input);
        let mut published = self.published();
        published.epoch = state.timeline_epoch;
        published.paused = state.paused;
        published.recording = !state.paused && fresh
            && consumer.is_some_and(|consumer| consumer.capture.state() == CaptureState::Recording);
        published.input_expires_at = consumer.and_then(|consumer|
            consumer.last_input.map(|last| last + consumer.input_timeout));
        published.meter = consumer.filter(|_| !state.paused && fresh)
            .map_or_else(MeterReadings::default, |consumer| consumer.capture.meter_readings());
        published.sidechain = consumer.is_some_and(|consumer| consumer.sidechain_channels != 0);
        published.host_track_name.clone_from(&state.host_track_name);
        let previous_editing = std::mem::replace(&mut published.editing, state.editing.clone());
        published.editing_pending = state.editing_pending;
        drop(published);
        // Releasing the last large snapshot must never hold the metadata lock.
        drop(previous_editing);
    }

    pub(crate) fn editing_snapshot(&self) -> Option<EditingSnapshot> {
        if !self.edit_requested.load(Ordering::Acquire) { return None; }
        let token = self.edit_token.load(Ordering::Acquire);
        self.published().editing.as_ref().filter(|edit| edit.token == token).cloned()
    }

    pub(crate) fn editing_pending(&self) -> bool {
        self.edit_requested.load(Ordering::Acquire) && self.editing_snapshot().is_none()
    }

    pub(crate) fn settings(&self) -> SessionSettings {
        self.settings.lock().clone()
    }

    pub(crate) fn set_output_dir(&self, output_dir: PathBuf) {
        self.settings.lock().output_dir = output_dir;
    }

    pub(crate) fn set_saved_dir(&self, saved_dir: PathBuf) {
        self.settings.lock().saved_dir = saved_dir;
    }

    pub(crate) fn set_panel_transparency(&self, panel_transparency: u8) {
        self.settings.lock().panel_transparency = panel_transparency;
    }

    pub(crate) fn set_accent(&self, accent: [u8; 3]) {
        self.settings.lock().accent = accent;
    }

    pub(crate) fn set_meter_mode(&self, meter_mode: MeterMode) {
        self.settings.lock().meter_mode = meter_mode;
    }

    pub(crate) fn set_wav_export(&self, profile: WavExportProfile, force_mono: bool) {
        let mut settings = self.settings.lock();
        settings.wav_profile = profile;
        settings.force_mono = force_mono;
    }

    pub(crate) fn set_show_waveform_only(&self, show_waveform_only: bool) {
        self.settings.lock().show_waveform_only = show_waveform_only;
    }

    pub(crate) fn set_waveform_layout(&self, layout: WaveformLayout) {
        self.settings.lock().waveform_layout = layout;
    }

    pub(crate) fn update_editor_settings(&self, mut settings: SessionSettings) {
        let mut current = self.settings.lock();
        settings.sender_id = current.sender_id.clone();
        *current = settings;
    }

    pub(crate) fn sidechain_available(&self) -> bool {
        self.published().sidechain
    }

    pub(crate) fn host_track_name(&self) -> Option<String> {
        self.published().host_track_name.clone()
    }

    pub(crate) fn set_host_track_name(&self, name: Option<&str>) {
        let mut state = self.lock();
        state.host_track_name = name.filter(|value| !value.trim().is_empty()).map(str::to_owned);
        self.publish(&state);
    }

    pub(crate) fn set_retention(&self, retention: DurationChoice) {
        self.settings.lock().retention = retention;
    }

    pub(crate) fn set_idle_resume(&self, idle_resume: AutoResumeDelay) {
        self.settings.lock().idle_resume = idle_resume;
        self.apply_capture_settings(&mut self.lock());
    }

    fn apply_capture_settings(&self, state: &mut State) {
        let (retention, idle_resume, capture_input) = {
            let settings = self.settings.lock();
            (settings.retention, settings.idle_resume, settings.capture_input)
        };
        if let Some(consumer) = &mut state.consumer {
            let input = if consumer.sidechain_channels == 0 {
                CaptureInput::Main
            } else {
                capture_input
            };
            if input != consumer.input
                && let Some(mut capture) = consumer.capture_for(input, retention)
            {
                consumer.discard();
                if state.paused {
                    capture.pause();
                }
                consumer.capture = capture;
                consumer.input = input;
                consumer.waveform = WaveformCache::default();
                state.timeline_epoch = state.timeline_epoch.wrapping_add(1);
                state.gesture_active = false;
                state.selection = None;
                state.editing = None;
                state.editing_pending = false;
                self.edit_requested.store(false, Ordering::Release);
            }
        }
        if retention != state.retention {
            state.retention = retention;
            if let Some(consumer) = &mut state.consumer {
                consumer.capture.set_retention(retention);
            }
        }
        if idle_resume != state.idle_resume {
            state.idle_resume = idle_resume;
            state.last_interaction = Instant::now();
        }
    }

    pub(crate) fn pause(&self) {
        let mut state = self.lock();
        self.pause_locked(&mut state);
        self.publish(&state);
    }

    fn pause_locked(&self, state: &mut State) {
        state.last_interaction = Instant::now();
        if state.paused {
            return;
        }
        let was_accepting = self.accepting.swap(false, Ordering::SeqCst);
        if let Some(consumer) = &mut state.consumer {
            if was_accepting {
                // Bring the snapshot up to date with audio already published by the producer.
                consumer.drain(usize::MAX);
            } else {
                consumer.discard();
            }
            consumer.capture.pause();
        }
        state.paused = true;
    }

    pub(crate) fn resume(&self) {
        let mut state = self.lock();
        Self::resume_locked(&mut state);
        self.publish(&state);
    }

    pub(crate) fn clear(&self) {
        self.edit_requested.store(false, Ordering::Release);
        let mut state = self.lock();
        state.last_interaction = Instant::now();
        state.timeline_epoch = state.timeline_epoch.wrapping_add(1);
        state.gesture_active = false;
        self.accepting.store(false, Ordering::SeqCst);
        if let Some(consumer) = &mut state.consumer {
            // Discard audio published before the clear. The worker re-arms only after any
            // in-flight producer has also finished and its remaining frames are discarded.
            consumer.discard();
            consumer.capture.clear();
            consumer.last_input = None;
        }
        state.selection = None;
        state.editing = None;
        state.editing_pending = false;
        self.publish(&state);
    }

    fn resume_locked(state: &mut State) {
        if let Some(consumer) = &mut state.consumer {
            consumer.capture.resume();
        }
        state.paused = false;
        state.last_interaction = Instant::now();
    }

    pub(crate) fn mark_interaction(&self) {
        if let Ok(mut state) = self.state.try_lock() {
            state.last_interaction = Instant::now();
        }
    }

    pub(crate) fn set_gesture_active(&self, active: bool) {
        self.queue_editor_action(EditorAction::Gesture(active));
    }

    pub(crate) fn begin_waveform_gesture(&self) {
        if !self.edit_requested.swap(true, Ordering::AcqRel) {
            self.edit_token.fetch_add(1, Ordering::AcqRel);
        }
        self.queue_editor_action(EditorAction::Begin);
    }

    pub(crate) fn set_snapshot_selection(&self, selection: Option<Range<u64>>) {
        self.queue_editor_action(EditorAction::Selection(selection.filter(|range| !range.is_empty())));
    }

    pub(crate) fn end_waveform_gesture(&self, keep_selection: bool) {
        if !keep_selection {
            self.edit_requested.store(false, Ordering::Release);
        }
        self.queue_editor_action(EditorAction::End(keep_selection));
    }

    pub(crate) fn cancel_waveform_selection(&self) {
        self.edit_requested.store(false, Ordering::Release);
        self.queue_editor_action(EditorAction::Cancel);
    }

    fn queue_editor_action(&self, action: EditorAction) {
        let _ = self.editor_commands.send(EditorCommand {
            epoch: self.published().epoch,
            token: self.edit_token.load(Ordering::Acquire),
            action,
        });
    }

    fn apply_editor_commands(&self, state: &mut State) {
        let requests = self.editor_requests.lock().unwrap_or_else(|error| error.into_inner());
        for command in requests.try_iter() {
            if command.epoch != state.timeline_epoch { continue; }
            if !matches!(command.action, EditorAction::Begin | EditorAction::Gesture(_))
                && command.token != state.edit_serial { continue; }
            match command.action {
                EditorAction::Begin => {
                    state.gesture_active = true;
                    if state.edit_serial != command.token {
                        state.edit_serial = command.token;
                        state.editing = None;
                        state.editing_pending = true;
                        state.selection = None;
                    }
                }
                EditorAction::Selection(selection) => state.selection = selection,
                EditorAction::End(keep) => {
                    state.gesture_active = false;
                    if !keep {
                        state.selection = None;
                        state.editing = None;
                        state.editing_pending = false;
                    }
                }
                EditorAction::Cancel => {
                    state.gesture_active = false;
                    state.selection = None;
                    state.editing = None;
                    state.editing_pending = false;
                }
                EditorAction::Gesture(active) => state.gesture_active = active,
            }
            state.last_interaction = Instant::now();
        }
    }

    pub(crate) fn timeline_epoch(&self) -> u64 {
        self.published().epoch
    }

    pub(crate) fn positioned_snapshots(&self, live: bool) -> Option<(u64, Arc<[Snapshot]>)> {
        if !live && self.edit_requested.load(Ordering::Acquire) {
            let editing = self.editing_snapshot()?;
            return Some((editing.timeline.epoch, editing.snapshots));
        }
        let mut state = self.lock();
        let recording = live && !state.paused && self.accepting.load(Ordering::SeqCst);
        let epoch = state.timeline_epoch;
        let consumer = state.consumer.as_mut()?;
        if recording {
            consumer.drain(usize::MAX);
        }
        Some((epoch, track_snapshots(&consumer.capture).into()))
    }

    #[cfg(test)]
    fn positioned_snapshot(&self, live: bool) -> Option<(u64, Snapshot)> {
        let (epoch, snapshots) = self.positioned_snapshots(live)?;
        snapshots.first().cloned().map(|snapshot| (epoch, snapshot))
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Option<Snapshot> {
        self.positioned_snapshot(false)
            .map(|(_, snapshot)| snapshot)
    }

    #[cfg(test)]
    pub(crate) fn snapshot_live(&self) -> Option<Snapshot> {
        self.positioned_snapshot(true).map(|(_, snapshot)| snapshot)
    }

    #[cfg(test)]
    pub(crate) fn waveform(
        &self,
        layout: WaveformLayout,
        peaks: &mut [WaveformPeaks; 2],
    ) -> WaveformTimeline {
        self.waveform_in_range(layout, None, peaks)
    }

    #[cfg(test)]
    pub(crate) fn waveform_in_range(
        &self,
        layout: WaveformLayout,
        visible: Option<Range<u64>>,
        peaks: &mut [WaveformPeaks; 2],
    ) -> WaveformTimeline {
        self.waveform_in_range_with_bins(layout, visible, WAVEFORM_BINS, peaks)
    }

    #[cfg(test)]
    pub(crate) fn waveform_in_range_with_bins(
        &self,
        layout: WaveformLayout,
        visible: Option<Range<u64>>,
        bins: usize,
        peaks: &mut [WaveformPeaks; 2],
    ) -> WaveformTimeline {
        Self::render_waveform(&mut self.lock(), layout, visible, bins, peaks)
    }

    pub(crate) fn try_waveform_in_range_with_bins(
        &self,
        layout: WaveformLayout,
        visible: Option<Range<u64>>,
        bins: usize,
        peaks: &mut [WaveformPeaks; 2],
    ) -> Option<WaveformTimeline> {
        let mut state = self.state.try_lock().ok()?;
        Some(Self::render_waveform(&mut state, layout, visible, bins, peaks))
    }

    fn render_waveform(
        state: &mut State,
        layout: WaveformLayout,
        visible: Option<Range<u64>>,
        bins: usize,
        peaks: &mut [WaveformPeaks; 2],
    ) -> WaveformTimeline {
        let bins = bins.clamp(1, backtracking_core::waveform::MAX_WAVEFORM_BINS);
        let epoch = state.timeline_epoch;
        let Some(consumer) = &mut state.consumer else {
            peaks.fill(WaveformPeaks::default());
            return WaveformTimeline {
                epoch,
                ..WaveformTimeline::default()
            };
        };
        let frames = consumer.capture.frame_range();
        let range = visible
            .map(|range| range.start.max(frames.start)..range.end.min(frames.end))
            .filter(|range| !range.is_empty())
            .unwrap_or_else(|| frames.clone());
        consumer.waveform(layout, range, bins, peaks);
        WaveformTimeline {
            epoch,
            bar_markers: consumer.capture.bar_markers(frames.clone()),
            frames,
            sample_rate: consumer.sample_rate,
            channels: consumer.capture.track_channels(0).unwrap_or(1),
            b_channels: consumer.capture.track_channels(1),
        }
    }


    pub(crate) fn is_paused(&self) -> bool {
        self.published().paused
    }

    pub(crate) fn is_recording(&self) -> bool {
        if let Ok(state) = self.state.try_lock() { self.publish(&state); }
        let published = self.published();
        published.recording && published.input_expires_at.is_some_and(|deadline| Instant::now() < deadline)
    }

    pub(crate) fn meter_readings(&self) -> MeterReadings {
        let published = self.published();
        if published.input_expires_at.is_some_and(|deadline| Instant::now() < deadline) {
            published.meter
        } else { MeterReadings::default() }
    }

    pub(crate) fn dropped_frames(&self) -> usize {
        self.dropped_frames.load(Ordering::Relaxed)
    }

    pub(crate) fn take_snapshot_result(&self) -> Option<SnapshotResult> {
        self.snapshot_result.lock().unwrap_or_else(|error| error.into_inner()).take()
    }

    pub(crate) fn request_snapshot(&self) {
        let _ = self.snapshot_commands.send(self.timeline_epoch());
    }

    fn try_snapshot_for_shortcut(&self, expected_epoch: Option<u64>) -> SnapshotAttempt {
        self.try_snapshot(expected_epoch, false)
    }

    fn try_snapshot(&self, expected_epoch: Option<u64>, full_history: bool) -> SnapshotAttempt {
        let mut state = self.lock();
        if expected_epoch.is_some_and(|epoch| epoch != state.timeline_epoch) {
            return SnapshotAttempt::Empty;
        }
        // A shortcut can arrive after consume() and after the editor queued its
        // latest selection. Observe those commands before choosing the material.
        self.apply_editor_commands(&mut state);
        if full_history {
            self.pin_editing_material(&mut state);
        } else if state.editing_pending || state.gesture_active {
            return SnapshotAttempt::Pending;
        }
        self.snapshot_for_shortcut_locked(&mut state, full_history)
            .map_or(SnapshotAttempt::Empty, SnapshotAttempt::Ready)
    }

    #[cfg(test)]
    fn snapshot_for_shortcut(&self) -> Option<SnapshotJob> {
        match self.try_snapshot_for_shortcut(None) {
            SnapshotAttempt::Ready(job) => Some(job),
            SnapshotAttempt::Pending | SnapshotAttempt::Empty => None,
        }
    }

    fn snapshot_for_shortcut_locked(&self, state: &mut State, full_history: bool) -> Option<SnapshotJob> {
        let settings = self.settings();
        let selected = if full_history { None } else { state.selection.clone() };
        let epoch = state.timeline_epoch;
        let snapshots: Arc<[Snapshot]> = if selected.is_some() {
            if state.editing_pending { return None; }
            state.editing.as_ref()?.snapshots.clone()
        } else {
            let recording = !state.paused && self.accepting.load(Ordering::SeqCst);
            let consumer = state.consumer.as_mut()?;
            if recording { consumer.drain(usize::MAX); }
            track_snapshots(&consumer.capture).into()
        };
        let available = snapshots.first()?.frame_range();
        let range = selected
            .clone()
            .map_or(available.clone(), |range| {
                range.start.max(available.start)..range.end.min(available.end)
            });
        if range.is_empty() {
            return None;
        }
        let first = (range.start - available.start) as usize;
        let end = (range.end - available.start) as usize;
        if selected.is_some() {
            if self.edit_token.load(Ordering::Acquire) == state.edit_serial {
                self.edit_requested.store(false, Ordering::Release);
            }
            state.selection = None;
            state.editing = None;
            state.editing_pending = false;
            state.gesture_active = false;
        }
        self.publish(&state);
        Some(SnapshotJob {
            settings,
            snapshots,
            epoch,
            range,
            relative: first..end,
        })
    }

    fn consume(&self) {
        let mut state = self.lock();
        self.apply_capture_settings(&mut state);
        self.apply_editor_commands(&mut state);
        self.pin_editing_material(&mut state);
        self.publish(&state);
        if state.paused
            && !state.gesture_active
            && state.selection.is_none()
            && state.idle_resume != AutoResumeDelay::Immediate
            && state.last_interaction.elapsed()
                >= Duration::from_secs(u64::from(state.idle_resume.seconds()))
        {
            Self::resume_locked(&mut state);
        }

        let paused = state.paused;
        let Some(consumer) = &mut state.consumer else {
            return;
        };
        let accepting = self.accepting.load(Ordering::SeqCst);
        if accepting && !paused {
            // Yield the session lock regularly even when a DAW renders ahead of realtime.
            consumer.drain(256);
        } else {
            consumer.discard();
        }
        if !accepting && !paused {
            self.accept_input_when_safe(consumer);
        }
        self.publish(&state);
    }

    fn pin_editing_material(&self, state: &mut State) {
        // Pin the already displayed material before queued audio advances the
        // rolling boundary. The audio callback continues accepting packets.
        if state.editing_pending && let Some(consumer) = &state.consumer {
            let frames = consumer.capture.frame_range();
            let editing = EditingSnapshot {
                token: state.edit_serial,
                timeline: Arc::new(WaveformTimeline {
                    epoch: state.timeline_epoch,
                    bar_markers: consumer.capture.bar_markers(frames.clone()),
                    frames,
                    sample_rate: consumer.sample_rate,
                    channels: consumer.capture.track_channels(0).unwrap_or(1),
                    b_channels: consumer.capture.track_channels(1),
                }),
                snapshots: track_snapshots(&consumer.capture).into(),
            };
            state.editing = Some(editing);
            state.editing_pending = false;
        }
    }

    fn accept_input_when_safe(&self, consumer: &mut Consumer) {
        if !self.writer_active.load(Ordering::SeqCst) {
            // A callback may have started just before Pause. Clear anything it published after
            // the frozen snapshot before accepting input again.
            consumer.discard();
            self.accepting.store(true, Ordering::SeqCst);
        }
    }
}

fn track_snapshots(capture: &Capture) -> Vec<Snapshot> {
    (0..2).filter_map(|lane| capture.track_snapshot(lane)).collect()
}

struct SnapshotJob {
    settings: SessionSettings,
    snapshots: Arc<[Snapshot]>,
    epoch: u64,
    range: Range<u64>,
    relative: Range<usize>,
}

enum SnapshotAttempt {
    Pending,
    Empty,
    Ready(SnapshotJob),
}

impl SnapshotJob {
    fn write(self) -> SnapshotResult {
        let result = export_routed_tracks_with_profile(
            &self.settings.saved_dir,
            &self.snapshots,
            self.relative,
            self.settings.wav_profile,
            &[ChannelRoute::Merged; 2][..self.snapshots.len()],
            self.settings.force_mono,
        ).map_err(|error| error.to_string());
        (self.epoch, self.range, result)
    }
}

pub(crate) struct Worker {
    session: Arc<Session>,
    queue: Arc<TimedAudioQueue>,
    sender_queue: Arc<TimedAudioQueue>,
    sender_active: Arc<AtomicBool>,
    sender_clock: AtomicU64,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    rate: u32,
    channels: usize,
    sidechain: usize,
    capacity_frames: usize,
}

impl Worker {
    #[cfg(test)]
    pub(crate) fn new(
        session: Arc<Session>,
        rate: u32,
        channels: usize,
        max_buffer: u32,
    ) -> Option<Self> {
        Self::with_sidechain(session, rate, channels, 0, max_buffer)
    }

    pub(crate) fn with_sidechain(
        session: Arc<Session>,
        rate: u32,
        channels: usize,
        sidechain: usize,
        max_buffer: u32,
    ) -> Option<Self> {
        if rate == 0 || !(1..=2).contains(&channels) || sidechain > 2 {
            return None;
        }
        let (queue, capacity_frames, metadata_slots) = {
            let mut state = session.lock();
            session.apply_capture_settings(&mut state);
            let input = if sidechain == 0 {
                CaptureInput::Main
            } else {
                session.settings.lock().capture_input
            };
            let (a, b) = match input {
                CaptureInput::Main => (channels, None),
                CaptureInput::Sidechain => (sidechain, None),
                CaptureInput::Both => (channels, Some(sidechain)),
            };
            let mut capture = Capture::new_tracks(rate, a, b, state.retention).ok()?;
            if state.paused {
                capture.pause();
            }
            let block_capacity = (max_buffer as usize).saturating_mul(2).min(rate as usize);
            // A bounded two-second backlog absorbs cold allocation/snapshot work
            // while the realtime callback keeps publishing without taking locks.
            let capacity_frames = (rate as usize).saturating_mul(2)
                .max(block_capacity)
                .max(4096)
                .next_power_of_two();
            let metadata_slots = capacity_frames.div_ceil(16).clamp(4096, 65_536);
            let queue = TimedAudioQueue::new(capacity_frames, metadata_slots, channels + sidechain);
            state.timeline_epoch = state.timeline_epoch.wrapping_add(1);
            state.gesture_active = false;
            state.selection = None;
            state.editing = None;
            state.editing_pending = false;
            session.edit_requested.store(false, Ordering::Release);
            state.consumer = Some(Consumer {
                queue: queue.clone(),
                capture,
                scratch: Vec::new(),
                routed: Vec::new(),
                bars: Vec::new(),
                sample_rate: rate,
                main_channels: channels,
                sidechain_channels: sidechain,
                input,
                waveform: WaveformCache::default(),
                last_input: None,
                // A stopped host may stop sending blocks before the gate receives silence.
                input_timeout: Duration::from_secs_f64(
                    (2.0 * f64::from(max_buffer) / f64::from(rate)).max(0.25),
                ),
            });
            session.accepting.store(!state.paused, Ordering::SeqCst);
            session.publish(&state);
            (queue, capacity_frames, metadata_slots)
        };
        session.dropped_frames.store(0, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_session = session.clone();
        let sender_queue = TimedAudioQueue::new(capacity_frames, metadata_slots, channels + sidechain);
        let worker_sender_queue = sender_queue.clone();
        let sender_active = Arc::new(AtomicBool::new(false));
        let worker_sender_active = sender_active.clone();
        let thread = match thread::Builder::new()
            .name("backtracking-capture".to_owned())
            .spawn(move || {
                let mut sender = SenderWorker::new(
                    worker_sender_queue,
                    worker_sender_active,
                    rate,
                    channels,
                    sidechain,
                );
                while !worker_stop.load(Ordering::Relaxed) {
                    worker_session.consume();
                    sender.update(&worker_session);
                    thread::park_timeout(Duration::from_millis(4));
                }
            }) {
            Ok(thread) => thread,
            Err(_) => {
                session.accepting.store(false, Ordering::SeqCst);
                session.lock().consumer = None;
                return None;
            }
        };
        Some(Self {
            session,
            queue,
            sender_queue,
            sender_active,
            sender_clock: AtomicU64::new(0),
            stop,
            thread: Some(thread),
            rate,
            channels,
            sidechain,
            capacity_frames,
        })
    }

    pub(crate) fn matches(&self, rate: u32, channels: usize, sidechain: usize, max_buffer: u32) -> bool {
        self.rate == rate
            && self.channels == channels
            && self.sidechain == sidechain
            && self.capacity_frames >= max_buffer.min(rate) as usize
    }

    #[cfg(test)]
    pub(crate) fn push(&self, audio: &[&mut [f32]]) {
        self.push_block(audio, &[], None);
    }

    pub(crate) fn push_block(
        &self,
        audio: &[&mut [f32]],
        sidechain: &[&mut [f32]],
        bars: Option<(f64, f64)>,
    ) {
        let Some(first) = audio.first().filter(|_| audio.len() == self.channels) else {
            return;
        };
        let frames = first.len();
        let tags = bars.map_or([0; 2], |(first, interval)| [first.to_bits(), interval.to_bits()]);
        let sample = |frame: usize, channel: usize| {
            if channel < self.channels {
                audio[channel][frame]
            } else {
                sidechain
                    .get(channel - self.channels)
                    .and_then(|samples| samples.get(frame))
                    .copied()
                    .unwrap_or(0.)
            }
        };
        self.session.writer_active.store(true, Ordering::SeqCst);
        if self.session.accepting.load(Ordering::SeqCst) {
            let written = self.queue.push_tagged(0, self.rate, frames, tags, sample);
            if written < frames {
                self.session
                    .dropped_frames
                    .fetch_add(frames - written, Ordering::Relaxed);
            }
        }
        self.session.writer_active.store(false, Ordering::SeqCst);
        if self.sender_active.load(Ordering::Relaxed) {
            let now = capture_clock_ns();
            let expected = self.sender_clock.load(Ordering::Relaxed);
            let duration = (frames as u64).saturating_mul(1_000_000_000) / u64::from(self.rate);
            let start = if expected == 0 || expected.abs_diff(now) > 80_000_000 {
                now
            } else {
                expected
            };
            self.sender_clock.store(start.saturating_add(duration), Ordering::Relaxed);
            self.sender_queue.push_tagged(start, self.rate, frames, tags, sample);
        } else {
            self.sender_clock.store(0, Ordering::Relaxed);
        }
    }
}

struct SenderWorker {
    endpoint: Option<SenderEndpoint>,
    retry_at: Instant,
    queue: Arc<TimedAudioQueue>,
    active: Arc<AtomicBool>,
    writer: Option<SyncSender<SnapshotJob>>,
    pending_snapshots: usize,
    pending_snapshot_epoch: Option<u64>,
    rate: u32,
    main: usize,
    sidechain: usize,
    scratch: Vec<f32>,
    routed: Vec<f32>,
    bars: Vec<u32>,
}

impl SenderWorker {
    fn new(
        queue: Arc<TimedAudioQueue>,
        active: Arc<AtomicBool>,
        rate: u32,
        main: usize,
        sidechain: usize,
    ) -> Self {
        Self {
            endpoint: None,
            retry_at: Instant::now(),
            queue,
            active,
            writer: None,
            pending_snapshots: 0,
            pending_snapshot_epoch: None,
            rate,
            main,
            sidechain,
            scratch: Vec::new(),
            routed: Vec::new(),
            bars: Vec::new(),
        }
    }

    fn update(&mut self, session: &Session) {
        // Local saves work independently of the Sender connection. Both paths
        // collect audio and hand it to the same writer on this worker thread.
        self.process_local_snapshots(session);
        let (mut name, enabled, input, preferred_id) = {
            let settings = session.settings.lock();
            (
                settings.sender_name.clone(),
                settings.sender_enabled,
                settings.capture_input,
                settings.sender_id.clone(),
            )
        };
        let needs_endpoint = self.endpoint.as_ref().is_none_or(|endpoint| {
            preferred_id.as_deref().is_some_and(|id| endpoint.id() != id)
        });
        if needs_endpoint && Instant::now() >= self.retry_at {
            self.retry_at = Instant::now() + Duration::from_secs(2);
            if let Ok(endpoint) = SenderEndpoint::with_id(preferred_id.as_deref().unwrap_or("")) {
                let mut settings = session.settings.lock();
                if settings.sender_id == preferred_id {
                    settings.sender_id = Some(endpoint.id().to_owned());
                    self.endpoint = Some(endpoint);
                    self.retry_at = Instant::now();
                }
            }
        }
        let Some(endpoint) = &mut self.endpoint else { return };
        if let Some(host_name) = session.host_track_name() {
            name = host_name;
        }
        let (route, channels) = if input == CaptureInput::Sidechain && self.sidechain != 0 {
            (CaptureInput::Sidechain, self.sidechain)
        } else {
            (CaptureInput::Main, self.main)
        };
        endpoint.update(&name, enabled, self.rate, channels);
        self.active.store(enabled, Ordering::Relaxed);
        if enabled {
            for _ in 0..self.queue.pending_packets().min(64) {
                let Some((packet, tags)) = self.queue.pop_tagged(&mut self.scratch) else { break };
                fill_bar_offsets(&mut self.bars, packet.frames, tags);
                let audio = route_block(
                    &self.scratch, self.main, self.sidechain, route, &mut self.routed,
                );
                let _ = endpoint.publish(
                    &name, self.rate, channels, packet.timestamp_ns, audio, &self.bars,
                );
            }
        } else {
            self.queue.discard_pending(&mut self.scratch);
        }
        if endpoint.take_snapshot_request() {
            self.add_snapshot_request(session.timeline_epoch());
        }
        if let Some(job) = self.take_ready_snapshot(session) {
            self.queue_snapshot(session, job);
        }
    }

    fn add_snapshot_request(&mut self, epoch: u64) {
        if self.pending_snapshot_epoch != Some(epoch) {
            self.pending_snapshots = 0;
            self.pending_snapshot_epoch = Some(epoch);
        }
        self.pending_snapshots = self.pending_snapshots.saturating_add(1);
    }

    fn process_local_snapshots(&mut self, session: &Session) {
        let requests = session.snapshot_requests.lock().unwrap_or_else(|error| error.into_inner());
        for epoch in requests.try_iter() {
            match session.try_snapshot(Some(epoch), true) {
                SnapshotAttempt::Ready(job) => self.queue_snapshot(session, job),
                SnapshotAttempt::Empty => {
                    *session.snapshot_result.lock().unwrap_or_else(|error| error.into_inner()) =
                        Some((epoch, 0..0, Err("No captured audio to save yet.".into())));
                }
                SnapshotAttempt::Pending => unreachable!("full history snapshots never await a gesture"),
            }
        }
    }

    fn take_ready_snapshot(&mut self, session: &Session) -> Option<SnapshotJob> {
        if self.pending_snapshots == 0 { return None; }
        match session.try_snapshot_for_shortcut(self.pending_snapshot_epoch) {
            SnapshotAttempt::Pending => None,
            SnapshotAttempt::Empty => {
                self.pending_snapshots -= 1;
                None
            }
            SnapshotAttempt::Ready(job) => {
                self.pending_snapshots -= 1;
                Some(job)
            }
        }
    }

    fn queue_snapshot(&mut self, session: &Session, job: SnapshotJob) {
        let result = &session.snapshot_result;
        let save_result = result.clone();
        let write = || {
            let (tx, rx) = mpsc::sync_channel::<SnapshotJob>(2);
            thread::Builder::new()
                .name("backtracking-shortcut-save".to_owned())
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        *save_result.lock().unwrap_or_else(|error| error.into_inner()) =
                            Some(job.write());
                    }
                })?;
            Ok::<_, std::io::Error>(tx)
        };
        let epoch = job.epoch;
        let range = job.range.clone();
        let queued = match &self.writer {
            Some(writer) => writer.try_send(job).map_err(|error| error.to_string()),
            None => write().map_err(|error| error.to_string()).and_then(|writer| {
                self.writer.insert(writer).try_send(job).map_err(|error| error.to_string())
            }),
        };
        if let Err(error) = queued {
            *result.lock().unwrap_or_else(|error| error.into_inner()) =
                Some((epoch, range, Err(error)));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
        self.session.accepting.store(false, Ordering::SeqCst);
        let mut state = self.session.lock();
        state.consumer = None;
        state.selection = None;
        state.timeline_epoch = state.timeline_epoch.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use nice_plug::params::persist::PersistentField;

    use super::*;

    #[test]
    fn split_and_merged_waveforms_preserve_the_same_stereo_capture() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let worker = Worker::new(session.clone(), 48_000, 2, 256).unwrap();
        worker.push(&[&mut [0.5; 64], &mut [-0.5; 64]]);
        session.pause();
        let original = session.snapshot().unwrap();
        let mut peaks = std::array::from_fn(|_| WaveformPeaks::default());
        let merged = session.waveform(WaveformLayout::Merged, &mut peaks);
        assert_eq!(merged.frames, original.frame_range());
        assert!(peaks[0].minimum.iter().all(|&sample| sample == 0.0));
        assert!(peaks[0].maximum.iter().all(|&sample| sample == 0.0));
        let split = session.waveform(WaveformLayout::Stereo, &mut peaks);
        assert_eq!(split.frames, merged.frames);
        assert_eq!(split.epoch, merged.epoch);
        assert!(peaks[0].maximum.iter().all(|&sample| sample == 0.5));
        assert!(peaks[1].minimum.iter().all(|&sample| sample == -0.5));
        assert_eq!(session.snapshot().unwrap(), original);
    }

    #[test]
    fn fresh_instances_default_to_ten_second_idle_resume() {
        assert_eq!(
            Session::new(SettingsStore::default())
                .settings()
                .idle_resume,
            AutoResumeDelay::Seconds10
        );
    }

    #[test]
    fn host_settings_restore_updates_running_capture_without_reactivating() {
        let settings = SettingsStore::default();
        let session = Arc::new(Session::new(settings.clone()));
        let worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();
        let mut input = [0.5; 3_000];
        for _ in 0..5 {
            worker.push(&[&mut input]);
            session.consume();
        }
        assert_eq!(session.snapshot().unwrap().frame_count(), 15_000);

        settings.set(BTreeMap::from([
            ("history_seconds".into(), "10".into()),
            ("auto_resume_delay_seconds".into(), "5".into()),
        ]));
        session.consume();

        assert_eq!(session.snapshot().unwrap().frame_count(), 10_000);
        assert!(!session.is_paused());
        assert_eq!(session.lock().idle_resume, AutoResumeDelay::Seconds5);
    }

    #[test]
    fn clear_discards_pending_and_retained_audio_without_changing_pause_state() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();
        worker.push(&[&mut [0.25; 32]]);
        session.consume();
        assert!(!session.snapshot().unwrap().samples.is_empty());

        worker.push(&[&mut [0.5; 32]]);
        session.clear();
        session.consume();
        assert!(!session.is_paused());
        assert!(session.snapshot().unwrap().samples.is_empty());

        worker.push(&[&mut [0.75; 32]]);
        session.pause();
        assert_eq!(session.snapshot().unwrap().samples, vec![0.75; 32]);
        session.clear();
        assert!(session.is_paused());
        assert!(session.snapshot().unwrap().samples.is_empty());
    }

    #[test]
    fn clear_followed_by_pause_cannot_restore_an_in_flight_block() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();

        session.writer_active.store(true, Ordering::SeqCst);
        session.clear();
        // Model a callback that observed the old accepting flag but published after Clear.
        assert_eq!(worker.queue.push_interleaved(0, 1_000, &[0.5; 32]), 32);
        session.pause();
        session.writer_active.store(false, Ordering::SeqCst);

        assert!(session.is_paused());
        assert!(session.snapshot().unwrap().samples.is_empty());
    }

    #[test]
    fn pauses_without_recording_later_audio_and_appends_after_resuming() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();

        worker.push(&[&mut [0.25; 32]]);
        session.pause();
        worker.push(&[&mut [0.5; 32]]);
        assert_eq!(session.snapshot().unwrap().samples, vec![0.25; 32]);

        session.resume();
        session.consume();
        worker.push(&[&mut [0.75; 32]]);
        session.pause();
        let recorded = session.snapshot().unwrap();
        assert_eq!(&recorded.samples[..32], &[0.25; 32]);
        assert_eq!(&recorded.samples[32..], &[0.75; 32]);
    }

    #[test]
    fn live_snapshot_includes_large_queued_block_without_stopping_capture() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let mut worker = Worker::new(session.clone(), 48_000, 1, 32_768).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();

        worker.push(&[&mut vec![0.25; 40_000]]);
        let snapshot = session.snapshot_live().unwrap();
        assert_eq!(snapshot.frame_count(), 40_000);
        assert!(!session.is_paused());

        worker.push(&[&mut [0.75; 32]]);
        let latest = session.snapshot_live().unwrap();
        assert_eq!(latest.frame_count(), 40_032);
        assert_eq!(&latest.samples[40_000..], &[0.75; 32]);
        assert_eq!(snapshot.samples.last(), Some(&0.25));
    }

    #[test]
    fn recording_indicator_stops_when_host_stops_sending_audio() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let worker = Worker::new(session.clone(), 48_000, 1, 256).unwrap();

        worker.push(&[&mut [0.25; 32]]);
        session.consume();
        assert!(session.is_recording());

        let mut state = session.lock();
        let consumer = state.consumer.as_mut().unwrap();
        consumer.last_input = Some(Instant::now() - consumer.input_timeout);
        drop(state);
        assert!(!session.is_recording());
        assert_eq!(session.snapshot().unwrap().samples, vec![0.25; 32]);
    }

    #[test]
    fn idle_resume_does_not_pause_live_edits_or_release_an_explicit_pause() {
        for delay in AutoResumeDelay::ALL {
            let session = Session::new(SettingsStore::default());
            session.set_idle_resume(delay);
            session.begin_waveform_gesture();
            let after_delay = Instant::now() - Duration::from_secs(u64::from(delay.seconds()) + 1);
            session.lock().last_interaction = after_delay;
            session.consume();
            assert!(!session.is_paused());
            session.end_waveform_gesture(false);

            session.pause();
            session.begin_waveform_gesture();
            session.lock().last_interaction = after_delay;
            session.consume();
            assert!(session.is_paused());

            session.set_snapshot_selection(Some(10..20));
            session.end_waveform_gesture(true);
            session.lock().last_interaction = after_delay;
            session.consume();
            assert!(session.is_paused());

            session.cancel_waveform_selection();
            assert!(session.is_paused());
            session.resume();
            assert!(!session.is_paused());
        }
    }

    #[test]
    fn waveform_gesture_keeps_recording_and_pins_both_original_tracks() {
        let settings = SettingsStore::default();
        settings.lock().capture_input = CaptureInput::Both;
        settings.lock().retention = DurationChoice::Seconds10;
        let session = Arc::new(Session::new(settings));
        let mut worker = Worker::with_sidechain(session.clone(), 10, 2, 1, 256).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        worker.push_block(&[&mut [0.25; 64], &mut [-0.25; 64]], &[&mut [0.75; 64]], None);
        session.consume();
        session.begin_waveform_gesture();
        assert!(!session.is_paused(), "selection must not stop the producer");
        assert!(session.accepting.load(Ordering::SeqCst));
        assert!(session.positioned_snapshots(false).is_none(), "pending edits must not export a live replacement");
        session.consume();
        session.set_snapshot_selection(Some(8..24));
        session.end_waveform_gesture(true);
        worker.push_block(&[&mut [0.5; 256], &mut [-0.5; 256]], &[&mut [0.9; 256]], None);
        session.consume();
        assert!(!session.is_paused());
        let live = session.positioned_snapshots(true).unwrap().1;
        assert!(live[0].first_frame > 24, "the live ring must continue past the old selection");
        let job = session.snapshot_for_shortcut().unwrap();
        assert_eq!(job.range, 8..24);
        assert_eq!(job.snapshots[0].frames(job.relative.clone()).unwrap(), [0.25, -0.25].repeat(16));
        assert_eq!(job.snapshots[1].frames(job.relative.clone()).unwrap(), [0.75; 16]);
        assert!(!session.is_paused());
    }

    #[test]
    fn waveform_gesture_does_not_release_an_explicit_pause() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let mut worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        worker.push(&[&mut [0.25; 32]]);
        session.pause();
        session.begin_waveform_gesture();
        session.consume();
        session.set_snapshot_selection(Some(8..20));
        session.end_waveform_gesture(true);
        session.cancel_waveform_selection();
        assert!(session.is_paused());
        worker.push(&[&mut [0.5; 32]]);
        assert_eq!(session.snapshot().unwrap().samples, [0.25; 32]);
    }

    #[test]
    fn editing_commands_and_paint_reads_return_while_the_capture_mutex_is_busy() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let (locked, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let busy = session.clone();
        let thread = thread::spawn(move || {
            let _guard = busy.lock();
            locked.send(()).unwrap();
            let _ = wait.recv_timeout(Duration::from_millis(250));
        });
        ready.recv().unwrap();
        let started = Instant::now();
        session.mark_interaction();
        session.begin_waveform_gesture();
        assert!(session.editing_pending());
        assert!(session.positioned_snapshots(false).is_none());
        session.set_snapshot_selection(Some(8..20));
        session.end_waveform_gesture(true);
        session.cancel_waveform_selection();
        assert!(!session.is_paused());
        assert!(!session.is_recording());
        let _ = session.meter_readings();
        let _ = session.timeline_epoch();
        let _ = session.sidechain_available();
        let _ = session.host_track_name();
        session.request_snapshot();
        let mut peaks = std::array::from_fn(|_| WaveformPeaks::default());
        peaks[0].maximum.fill(0.75);
        assert!(session.try_waveform_in_range_with_bins(
            WaveformLayout::Merged, None, 128, &mut peaks,
        ).is_none());
        assert!(peaks[0].maximum.iter().all(|&sample| sample == 0.75));
        let elapsed = started.elapsed();
        let _ = release.send(());
        thread.join().unwrap();
        assert!(elapsed < Duration::from_millis(50), "UI waited for the capture mutex: {elapsed:?}");
        assert_eq!(session.snapshot_requests.lock().unwrap().try_recv().unwrap(), session.timeline_epoch());
        session.consume();
        assert!(!session.editing_pending());
        assert!(session.editing_snapshot().is_none());
    }

    #[test]
    fn play_and_shorter_history_keep_the_original_edit_material() {
        let settings = SettingsStore::default();
        settings.lock().retention = DurationChoice::Minutes3;
        let session = Arc::new(Session::new(settings));
        let mut worker = Worker::new(session.clone(), 100, 1, 256).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        worker.push(&[&mut [0.25; 2000]]);
        session.consume();
        session.begin_waveform_gesture();
        session.consume();
        session.set_snapshot_selection(Some(8..24));
        session.end_waveform_gesture(true);
        session.consume();
        session.pause();
        session.resume();
        assert!(!session.is_paused());
        assert_eq!(session.lock().selection, Some(8..24));
        session.set_retention(DurationChoice::Seconds10);
        session.consume();
        assert_eq!(session.positioned_snapshots(true).unwrap().1[0].first_frame, 1000);
        let job = session.snapshot_for_shortcut().unwrap();
        assert_eq!(job.range, 8..24);
        assert_eq!(job.snapshots[0].frames(job.relative).unwrap(), [0.25; 16]);
    }

    #[test]
    fn clear_and_input_changes_cancel_old_edit_tokens_and_queued_actions() {
        let settings = SettingsStore::default();
        settings.lock().capture_input = CaptureInput::Both;
        let session = Arc::new(Session::new(settings));
        let mut worker = Worker::with_sidechain(session.clone(), 1_000, 1, 1, 256).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        worker.push_block(&[&mut [0.25; 32]], &[&mut [0.75; 32]], None);
        session.consume();
        session.begin_waveform_gesture();
        session.consume();
        session.set_snapshot_selection(Some(8..20)); // Queued against the old epoch.
        let epoch = session.timeline_epoch();
        session.clear();
        session.consume();
        assert!(session.timeline_epoch() != epoch);
        assert!(!session.lock().gesture_active);
        assert!(session.lock().selection.is_none());
        assert!(session.editing_snapshot().is_none());
        assert!(!session.editing_pending());

        session.begin_waveform_gesture();
        session.consume();
        let epoch = session.timeline_epoch();
        session.settings.lock().capture_input = CaptureInput::Sidechain;
        session.consume();
        assert!(session.timeline_epoch() != epoch);
        assert!(!session.lock().gesture_active);
        assert!(session.lock().selection.is_none());
        assert!(session.editing_snapshot().is_none());
        assert!(!session.editing_pending());
    }

    #[test]
    fn a_one_second_producer_backlog_survives_a_busy_capture_worker() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let mut worker = Worker::new(session.clone(), 48_000, 2, 256).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        for _ in 0..250 {
            worker.push(&[&mut [0.25; 192], &mut [-0.25; 192]]);
        }
        while worker.queue.pending_packets() > 0 { session.consume(); }
        assert_eq!(session.dropped_frames(), 0);
        assert_eq!(session.snapshot_live().unwrap().frame_count(), 48_000);
    }

    #[test]
    fn tiny_host_packets_keep_a_full_second_of_backlog_without_metadata_loss() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let mut worker = Worker::new(session.clone(), 96_000, 2, 16).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        for _ in 0..6000 { worker.push(&[&mut [0.25; 16], &mut [-0.25; 16]]); }
        while worker.queue.pending_packets() > 0 { session.consume(); }
        assert_eq!(session.dropped_frames(), 0);
        assert_eq!(session.snapshot_live().unwrap().frame_count(), 96_000);
    }

    #[test]
    fn tiny_host_packets_are_consumed_faster_than_the_normal_poll_arrival_rate() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let mut worker = Worker::new(session.clone(), 96_000, 2, 16).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        // A 96 kHz host publishes 24 sixteen-frame packets per four milliseconds.
        for _ in 0..250 {
            for _ in 0..24 { worker.push(&[&mut [0.25; 16], &mut [-0.25; 16]]); }
            session.consume();
        }
        assert_eq!(worker.queue.pending_packets(), 0, "normal host input must not build an ever-growing backlog");
        assert_eq!(session.dropped_frames(), 0);
        assert_eq!(session.snapshot_live().unwrap().frame_count(), 96_000);
    }

    #[test]
    fn immediate_resume_keeps_gesture_audio_and_respects_manual_pause() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let mut worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();
        // Verify the first block after release without relying on the periodic worker.
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        session.set_idle_resume(AutoResumeDelay::Immediate);
        session.begin_waveform_gesture();
        session.consume();
        assert!(!session.is_paused());
        worker.push(&[&mut [0.25; 32]]);

        session.end_waveform_gesture(false);
        assert!(!session.is_paused());
        worker.push(&[&mut [0.5; 32]]);

        session.pause();
        assert_eq!(session.snapshot().unwrap().samples, [vec![0.25; 32], vec![0.5; 32]].concat());
        session.consume();
        assert!(session.is_paused());
        session.end_waveform_gesture(false);
        assert!(session.is_paused());
        session.resume();
        assert!(!session.is_paused());
    }

    #[test]
    fn legacy_idle_resume_values_do_not_enable_immediate_resume() {
        let settings = SettingsStore::default();
        for value in ["0", "10", "20", "30"] {
            settings.set(BTreeMap::from([(
                "idle_resume_seconds".into(),
                value.into(),
            )]));
            assert_eq!(settings.lock().idle_resume, AutoResumeDelay::Seconds10);
        }
    }

    #[test]
    fn mono_sidechain_and_stereo_main_share_bounds_without_changing_input() {
        let settings = SettingsStore::default();
        settings.lock().capture_input = CaptureInput::Both;
        let session = Arc::new(Session::new(settings));
        let worker = Worker::with_sidechain(session.clone(), 1_000, 2, 1, 256).unwrap();
        let mut left = [0.25; 32];
        let mut right = [-0.25; 32];
        let mut side = [0.75; 32];
        worker.push_block(&[&mut left, &mut right], &[&mut side], Some((0., 16.)));
        worker.push_block(&[&mut left, &mut right], &[], Some((0., 16.)));
        session.pause();
        let (_, snapshots) = session.positioned_snapshots(false).unwrap();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].frame_range(), snapshots[1].frame_range());
        assert_eq!(snapshots[0].samples, [0.25, -0.25].repeat(64));
        assert_eq!(&snapshots[1].samples[..32], &[0.75; 32]);
        assert_eq!(&snapshots[1].samples[32..], &[0.; 32]);
        assert_eq!(left, [0.25; 32]);
        assert_eq!(right, [-0.25; 32]);
        let mut peaks = std::array::from_fn(|_| WaveformPeaks::default());
        let wave = session.waveform_in_range(WaveformLayout::Merged, Some(5..25), &mut peaks);
        assert_eq!(wave.bar_markers, [0, 16, 32, 48]);
        assert_eq!(wave.channels, 2);
        assert_eq!(wave.b_channels, Some(1));
        assert_eq!(peaks[1].maximum, [0.75; WAVEFORM_BINS]);
    }

    #[test]
    fn waveform_resolution_follows_requested_bins_without_changing_capture() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();
        let mut samples = [0.25; 128];
        samples[63] = 0.9;
        worker.push(&[&mut samples]);
        session.pause();
        let original = session.snapshot().unwrap();
        let mut peaks = std::array::from_fn(|_| WaveformPeaks::default());
        for bins in [32, 1024, 128] {
            let timeline = session.waveform_in_range_with_bins(
                WaveformLayout::Merged, None, bins, &mut peaks,
            );
            assert_eq!(timeline.frames, original.frame_range());
            assert_eq!(peaks[0].minimum.len(), bins);
            assert_eq!(peaks[0].maximum.len(), bins);
            assert_eq!(peaks[0].maximum.iter().copied().fold(0.0_f32, f32::max), 0.9);
            assert_eq!(session.snapshot().unwrap(), original);
        }
    }

    #[test]
    fn snapshot_hotkey_exports_a_selection_and_resumes_even_without_editor() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();
        worker.push(&[&mut [0.25; 32]]);
        session.consume();
        session.begin_waveform_gesture();
        session.consume();
        session.set_snapshot_selection(Some(8..20));
        session.end_waveform_gesture(true);
        session.consume();
        let job = session.snapshot_for_shortcut().unwrap();
        assert_eq!(job.range, 8..20);
        assert_eq!(job.relative, 8..20);
        assert!(!session.is_paused());
        worker.push(&[&mut [0.75; 32]]);
        let next = session.snapshot_for_shortcut().unwrap();
        assert_eq!(next.range, 0..64);
        assert!(!session.is_paused());
    }

    #[test]
    fn local_snapshot_writes_full_history_without_sender_and_preserves_a_later_selection() {
        let output = std::env::temp_dir().join(format!(
            "backtracking-local-snapshot-test-{}", std::process::id(),
        ));
        let settings = SettingsStore::default();
        settings.lock().saved_dir = output.clone();
        settings.lock().capture_input = CaptureInput::Both;
        let session = Arc::new(Session::new(settings));
        let mut worker = Worker::with_sidechain(session.clone(), 48_000, 1, 1, 256).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        let mut sender = SenderWorker::new(
            worker.sender_queue.clone(), worker.sender_active.clone(), 48_000, 1, 1,
        );
        sender.retry_at = Instant::now() + Duration::from_secs(60);
        worker.push_block(&[&mut [0.25; 32]], &[&mut [0.75; 32]], None);
        session.consume();
        session.request_snapshot();

        // A new gesture between the UI request and the worker must neither
        // narrow the requested full snapshot nor get cancelled by that save.
        session.begin_waveform_gesture();
        session.set_snapshot_selection(Some(8..20));
        session.end_waveform_gesture(true);
        worker.push_block(&[&mut [0.5; 32]], &[&mut [0.9; 32]], None);
        sender.update(&session);
        assert!(sender.endpoint.is_none());
        assert!(!sender.active.load(Ordering::Relaxed));
        assert!(!session.is_paused());
        assert_eq!(session.lock().selection, Some(8..20));
        assert_eq!(session.editing_snapshot().unwrap().timeline.frames, 0..32);
        let deadline = Instant::now() + Duration::from_secs(5);
        let (epoch, range, result) = loop {
            if let Some(result) = session.take_snapshot_result() { break result; }
            assert!(Instant::now() < deadline, "background snapshot writer timed out");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(epoch, session.timeline_epoch());
        assert_eq!(range, 0..64);
        let paths = result.unwrap();
        assert_eq!(paths.len(), 2);
        for (path, expected) in paths.iter().zip([
            [vec![0.25_f32; 32], vec![0.5; 32]].concat(),
            [vec![0.75_f32; 32], vec![0.9; 32]].concat(),
        ]) {
            let wav = std::fs::read(path).unwrap();
            let data = wav.windows(4).position(|chunk| chunk == b"data").unwrap() + 8;
            let actual: Vec<_> = wav[data..].chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap())).collect();
            assert_eq!(actual, expected);
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(output).unwrap();
    }

    #[test]
    fn snapshot_hotkey_observes_editor_commands_between_worker_ticks() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let mut worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        worker.push(&[&mut [0.25; 32]]);
        session.consume();
        session.begin_waveform_gesture();
        session.consume();

        // The UI finished its drag after the audio worker's latest consume().
        session.set_snapshot_selection(Some(8..20));
        session.end_waveform_gesture(true);
        let job = session.snapshot_for_shortcut().unwrap();
        assert_eq!(job.range, 8..20);
        assert_eq!(job.snapshots[0].frames(job.relative).unwrap(), [0.25; 12]);

        session.begin_waveform_gesture();
        session.consume();
        session.set_snapshot_selection(Some(5..10));
        session.end_waveform_gesture(true);
        session.consume();
        // Cancel also takes effect before a later consume(), so it cannot save
        // the selection that the user has already dismissed.
        session.cancel_waveform_selection();
        worker.push(&[&mut [0.75; 32]]);
        let job = session.snapshot_for_shortcut().unwrap();
        assert_eq!(job.range, 0..64);
        assert_eq!(job.snapshots[0].frames(32..64).unwrap(), [0.75; 32]);
    }

    #[test]
    fn snapshot_hotkey_waits_for_pinned_material_and_a_finished_gesture() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let mut worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();
        worker.stop.store(true, Ordering::Relaxed);
        let thread = worker.thread.take().unwrap();
        thread.thread().unpark();
        thread.join().unwrap();
        let mut sender = SenderWorker::new(
            worker.sender_queue.clone(), worker.sender_active.clone(), 1_000, 1, 0,
        );
        worker.push(&[&mut [0.25; 32]]);
        session.consume();
        session.begin_waveform_gesture();
        sender.pending_snapshots = 1;
        sender.pending_snapshot_epoch = Some(session.timeline_epoch());
        assert!(sender.take_ready_snapshot(&session).is_none());
        assert_eq!(sender.pending_snapshots, 1, "pinning must not consume the shortcut");
        worker.push(&[&mut [0.75; 32]]);
        session.consume();
        assert!(sender.take_ready_snapshot(&session).is_none());
        assert_eq!(sender.pending_snapshots, 1, "an active drag must not save the live buffer");
        session.set_snapshot_selection(Some(8..20));
        session.end_waveform_gesture(true);
        let job = sender.take_ready_snapshot(&session).unwrap();
        assert_eq!(job.range, 8..20);
        assert_eq!(job.snapshots[0].frames(job.relative).unwrap(), [0.25; 12]);
        assert_eq!(sender.pending_snapshots, 0);
        assert!(sender.take_ready_snapshot(&session).is_none());
        assert!(!session.is_paused());

        // Clear changes the material generation. A deferred request cannot
        // accidentally save audio that arrived after that operation.
        session.begin_waveform_gesture();
        sender.pending_snapshots = 1;
        sender.pending_snapshot_epoch = Some(session.timeline_epoch());
        assert!(sender.take_ready_snapshot(&session).is_none());
        session.clear();
        worker.push(&[&mut [0.9; 32]]);
        session.consume();
        assert!(sender.take_ready_snapshot(&session).is_none());
        assert_eq!(sender.pending_snapshots, 0);

        // A request with no available audio is consumed, never postponed until
        // unrelated audio is recorded later.
        session.clear();
        sender.pending_snapshots = 1;
        sender.pending_snapshot_epoch = Some(session.timeline_epoch());
        assert!(sender.take_ready_snapshot(&session).is_none());
        assert_eq!(sender.pending_snapshots, 0);
    }
}

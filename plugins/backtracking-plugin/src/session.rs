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

#[derive(Clone, Copy)]
pub(crate) struct WaveformPeaks {
    pub(crate) minimum: [f32; WAVEFORM_BINS],
    pub(crate) maximum: [f32; WAVEFORM_BINS],
}

impl Default for WaveformPeaks {
    fn default() -> Self {
        Self {
            minimum: [0.0; WAVEFORM_BINS],
            maximum: [0.0; WAVEFORM_BINS],
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

#[derive(Default)]
pub(crate) struct WaveformTimeline {
    pub(crate) epoch: u64,
    pub(crate) frames: Range<u64>,
    pub(crate) sample_rate: u32,
    pub(crate) channels: usize,
    pub(crate) b_channels: Option<usize>,
    pub(crate) bar_markers: Vec<u64>,
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
        peaks: &mut [WaveformPeaks; 2],
    ) {
        let cache = &mut self.waveform;
        let revision = self.capture.revision();
        if cache.revision != Some(revision) || cache.layout != layout || cache.range != range {
            let a = self.capture.track_channels(0).unwrap_or(1);
            let dual = self.capture.track_channels(1).is_some();
            let views = waveform_views(layout, a);
            for (index, trace) in cache.peaks.iter_mut().enumerate() {
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
                    *trace = WaveformPeaks::default();
                }
            }
            cache.revision = Some(revision);
            cache.layout = layout;
            cache.range = range;
        }
        *peaks = cache.peaks;
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
    host_track_name: Option<String>,
    last_interaction: Instant,
}

pub(crate) struct Session {
    settings: SettingsStore,
    state: Mutex<State>,
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
                host_track_name: None,
                last_interaction: Instant::now(),
            }),
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
        self.lock().consumer.as_ref().is_some_and(|c| c.sidechain_channels != 0)
    }

    pub(crate) fn host_track_name(&self) -> Option<String> {
        self.lock().host_track_name.clone()
    }

    pub(crate) fn set_host_track_name(&self, name: Option<&str>) {
        self.lock().host_track_name = name.filter(|value| !value.trim().is_empty()).map(str::to_owned);
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
                state.selection = None;
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
        Self::resume_locked(&mut self.lock());
    }

    pub(crate) fn clear(&self) {
        let mut state = self.lock();
        state.last_interaction = Instant::now();
        state.timeline_epoch = state.timeline_epoch.wrapping_add(1);
        self.accepting.store(false, Ordering::SeqCst);
        if let Some(consumer) = &mut state.consumer {
            // Discard audio published before the clear. The worker re-arms only after any
            // in-flight producer has also finished and its remaining frames are discarded.
            consumer.discard();
            consumer.capture.clear();
            consumer.last_input = None;
        }
        state.selection = None;
    }

    fn resume_locked(state: &mut State) {
        if let Some(consumer) = &mut state.consumer {
            consumer.capture.resume();
        }
        state.paused = false;
        state.selection = None;
        state.last_interaction = Instant::now();
    }

    pub(crate) fn mark_interaction(&self) {
        self.lock().last_interaction = Instant::now();
    }

    pub(crate) fn set_gesture_active(&self, active: bool) {
        let mut state = self.lock();
        state.gesture_active = active;
        state.last_interaction = Instant::now();
    }

    pub(crate) fn begin_waveform_gesture(&self) {
        let mut state = self.lock();
        state.gesture_active = true;
        self.pause_locked(&mut state);
    }

    pub(crate) fn set_snapshot_selection(&self, selection: Option<Range<u64>>) {
        self.lock().selection = selection.filter(|range| !range.is_empty());
    }

    pub(crate) fn end_waveform_gesture(&self, keep_selection: bool) {
        let mut state = self.lock();
        if !state.gesture_active {
            return;
        }
        state.gesture_active = false;
        state.last_interaction = Instant::now();
        if !keep_selection && state.paused {
            Self::resume_locked(&mut state);
            if let Some(consumer) = &mut state.consumer {
                self.accept_input_when_safe(consumer);
            }
        }
    }

    pub(crate) fn cancel_waveform_selection(&self) {
        let mut state = self.lock();
        state.gesture_active = false;
        state.selection = None;
        if state.paused {
            Self::resume_locked(&mut state);
            if let Some(consumer) = &mut state.consumer {
                self.accept_input_when_safe(consumer);
            }
        }
    }

    pub(crate) fn timeline_epoch(&self) -> u64 {
        self.lock().timeline_epoch
    }

    pub(crate) fn positioned_snapshots(&self, live: bool) -> Option<(u64, Vec<Snapshot>)> {
        let mut state = self.lock();
        let recording = live && !state.paused && self.accepting.load(Ordering::SeqCst);
        let epoch = state.timeline_epoch;
        let consumer = state.consumer.as_mut()?;
        if recording {
            consumer.drain(usize::MAX);
        }
        Some((epoch, track_snapshots(&consumer.capture)))
    }

    #[cfg(test)]
    fn positioned_snapshot(&self, live: bool) -> Option<(u64, Snapshot)> {
        let (epoch, snapshots) = self.positioned_snapshots(live)?;
        snapshots.into_iter().next().map(|snapshot| (epoch, snapshot))
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

    pub(crate) fn waveform_in_range(
        &self,
        layout: WaveformLayout,
        visible: Option<Range<u64>>,
        peaks: &mut [WaveformPeaks; 2],
    ) -> WaveformTimeline {
        let mut state = self.lock();
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
        consumer.waveform(layout, range, peaks);
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
        self.lock().paused
    }

    pub(crate) fn is_recording(&self) -> bool {
        let state = self.lock();
        state.consumer.as_ref().is_some_and(|consumer| {
            matches!(consumer.capture.state(), CaptureState::Recording)
                && consumer.has_fresh_input()
        })
    }

    pub(crate) fn meter_readings(&self) -> MeterReadings {
        let state = self.lock();
        state
            .consumer
            .as_ref()
            .filter(|consumer| !state.paused && consumer.has_fresh_input())
            .map_or_else(MeterReadings::default, |consumer| {
                consumer.capture.meter_readings()
            })
    }

    pub(crate) fn dropped_frames(&self) -> usize {
        self.dropped_frames.load(Ordering::Relaxed)
    }

    pub(crate) fn take_snapshot_result(&self) -> Option<SnapshotResult> {
        self.snapshot_result.lock().unwrap_or_else(|error| error.into_inner()).take()
    }

    fn snapshot_for_shortcut(&self) -> Option<SnapshotJob> {
        let mut state = self.lock();
        let settings = self.settings();
        let selected = state.selection.clone();
        let epoch = state.timeline_epoch;
        let recording = !state.paused && self.accepting.load(Ordering::SeqCst);
        let consumer = state.consumer.as_mut()?;
        if recording {
            consumer.drain(usize::MAX);
        }
        let available = consumer.capture.frame_range();
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
        let snapshots = track_snapshots(&consumer.capture);
        if selected.is_some() {
            Self::resume_locked(&mut state);
            state.gesture_active = false;
            if let Some(consumer) = &mut state.consumer {
                self.accept_input_when_safe(consumer);
            }
        }
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
            consumer.drain(8);
        } else {
            consumer.discard();
        }
        if !accepting && !paused {
            self.accept_input_when_safe(consumer);
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
    snapshots: Vec<Snapshot>,
    epoch: u64,
    range: Range<u64>,
    relative: Range<usize>,
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
        let (queue, capacity_frames) = {
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
            let capacity_frames = (rate as usize / 4)
                .max(block_capacity)
                .max(4096)
                .next_power_of_two();
            let queue = TimedAudioQueue::new(capacity_frames, 4096, channels + sidechain);
            state.timeline_epoch = state.timeline_epoch.wrapping_add(1);
            state.selection = None;
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
            (queue, capacity_frames)
        };
        session.dropped_frames.store(0, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_session = session.clone();
        let sender_queue = TimedAudioQueue::new(capacity_frames, 4096, channels + sidechain);
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
            rate,
            main,
            sidechain,
            scratch: Vec::new(),
            routed: Vec::new(),
            bars: Vec::new(),
        }
    }

    fn update(&mut self, session: &Session) {
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
        if endpoint.take_snapshot_request()
            && let Some(job) = session.snapshot_for_shortcut()
        {
            self.queue_snapshot(session, job);
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
        let mut peaks = [WaveformPeaks::default(); 2];
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
    fn idle_resume_waits_during_gestures_and_a_held_selection() {
        for delay in AutoResumeDelay::ALL {
            let session = Session::new(SettingsStore::default());
            session.set_idle_resume(delay);
            session.begin_waveform_gesture();
            let after_delay = Instant::now() - Duration::from_secs(u64::from(delay.seconds()) + 1);
            session.lock().last_interaction = after_delay;
            session.consume();
            assert!(session.is_paused());

            session.set_snapshot_selection(Some(10..20));
            session.end_waveform_gesture(true);
            session.lock().last_interaction = after_delay;
            session.consume();
            assert!(session.is_paused());

            session.cancel_waveform_selection();
            assert!(!session.is_paused());
        }
    }

    #[test]
    fn immediate_resume_follows_waveform_release_but_respects_manual_pause() {
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
        assert!(session.is_paused());
        worker.push(&[&mut [0.25; 32]]);

        session.end_waveform_gesture(false);
        assert!(!session.is_paused());
        worker.push(&[&mut [0.5; 32]]);

        session.pause();
        assert_eq!(session.snapshot().unwrap().samples, vec![0.5; 32]);
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
        let mut peaks = [WaveformPeaks::default(); 2];
        let wave = session.waveform_in_range(WaveformLayout::Merged, Some(5..25), &mut peaks);
        assert_eq!(wave.bar_markers, [0, 16, 32, 48]);
        assert_eq!(wave.channels, 2);
        assert_eq!(wave.b_channels, Some(1));
        assert_eq!(peaks[1].maximum, [0.75; WAVEFORM_BINS]);
    }

    #[test]
    fn snapshot_hotkey_exports_a_selection_and_resumes_even_without_editor() {
        let session = Arc::new(Session::new(SettingsStore::default()));
        let worker = Worker::new(session.clone(), 1_000, 1, 256).unwrap();
        worker.push(&[&mut [0.25; 32]]);
        session.begin_waveform_gesture();
        session.set_snapshot_selection(Some(8..20));
        session.end_waveform_gesture(true);
        let job = session.snapshot_for_shortcut().unwrap();
        assert_eq!(job.range, 8..20);
        assert_eq!(job.relative, 8..20);
        assert!(!session.is_paused());
        worker.push(&[&mut [0.75; 32]]);
        let next = session.snapshot_for_shortcut().unwrap();
        assert_eq!(next.range, 0..64);
        assert!(!session.is_paused());
    }
}

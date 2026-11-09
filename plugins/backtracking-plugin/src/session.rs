use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    AudioQueue, AutoResumeDelay, Capture, CaptureState, DurationChoice, MeterReadings, Snapshot,
    WavExportProfile,
};

use crate::settings::{MeterMode, SessionSettings, SettingsStore};

struct Consumer {
    queue: Arc<AudioQueue>,
    capture: Capture,
    scratch: Vec<f32>,
    sample_rate: u32,
    channels: usize,
    waveform: WaveformCache,
    last_input: Option<Instant>,
    input_timeout: Duration,
}

#[derive(Default)]
struct WaveformCache {
    revision: Option<u64>,
    minimum: Vec<f32>,
    maximum: Vec<f32>,
    frames: usize,
}

impl Consumer {
    fn has_fresh_input(&self) -> bool {
        self.last_input
            .is_some_and(|last| last.elapsed() < self.input_timeout)
    }

    fn drain(&mut self, max_blocks: usize) {
        let mut received_input = false;
        for _ in 0..max_blocks {
            let frames = self.queue.pop_interleaved(&mut self.scratch);
            if frames == 0 {
                break;
            }
            received_input = true;
            let _ = self
                .capture
                .push_interleaved(&self.scratch[..frames * self.channels]);
        }
        if received_input {
            self.last_input = Some(Instant::now());
        }
    }

    fn waveform(&mut self, minimum: &mut [f32], maximum: &mut [f32]) -> usize {
        let cache = &mut self.waveform;
        let revision = self.capture.revision();
        if cache.revision != Some(revision)
            || cache.minimum.len() != minimum.len()
            || cache.maximum.len() != maximum.len()
        {
            cache.minimum.resize(minimum.len(), 0.0);
            cache.maximum.resize(maximum.len(), 0.0);
            cache.frames = self
                .capture
                .waveform(&mut cache.minimum, &mut cache.maximum);
            cache.revision = Some(revision);
        }
        minimum.copy_from_slice(&cache.minimum);
        maximum.copy_from_slice(&cache.maximum);
        cache.frames
    }
}

struct State {
    retention: DurationChoice,
    idle_resume: AutoResumeDelay,
    consumer: Option<Consumer>,
    paused: bool,
    gesture_active: bool,
    last_interaction: Instant,
}

pub(crate) struct Session {
    settings: SettingsStore,
    state: Mutex<State>,
    accepting: AtomicBool,
    writer_active: AtomicBool,
    dropped_frames: AtomicUsize,
}

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
                paused: false,
                gesture_active: false,
                last_interaction: Instant::now(),
            }),
            accepting: AtomicBool::new(false),
            writer_active: AtomicBool::new(false),
            dropped_frames: AtomicUsize::new(0),
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

    pub(crate) fn set_panel_transparency(&self, panel_transparency: u8) {
        self.settings.lock().panel_transparency = panel_transparency;
    }

    pub(crate) fn set_meter_mode(&self, meter_mode: MeterMode) {
        self.settings.lock().meter_mode = meter_mode;
    }

    pub(crate) fn set_wav_profile(&self, profile: WavExportProfile) {
        self.settings.lock().wav_profile = profile;
    }

    pub(crate) fn set_show_waveform_only(&self, show_waveform_only: bool) {
        self.settings.lock().show_waveform_only = show_waveform_only;
    }

    pub(crate) fn set_retention(&self, retention: DurationChoice) {
        self.settings.lock().retention = retention;
        self.apply_capture_settings(&mut self.lock());
    }

    pub(crate) fn set_idle_resume(&self, idle_resume: AutoResumeDelay) {
        self.settings.lock().idle_resume = idle_resume;
        self.apply_capture_settings(&mut self.lock());
    }

    fn apply_capture_settings(&self, state: &mut State) {
        let (retention, idle_resume) = {
            let settings = self.settings.lock();
            (settings.retention, settings.idle_resume)
        };
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
        self.apply_capture_settings(&mut state);
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
                consumer.queue.discard_pending();
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
        self.accepting.store(false, Ordering::SeqCst);
        if let Some(consumer) = &mut state.consumer {
            // Discard audio published before the clear. The worker re-arms only after any
            // in-flight producer has also finished and its remaining frames are discarded.
            consumer.queue.discard_pending();
            consumer.capture.clear();
            consumer.last_input = None;
        }
    }

    fn resume_locked(state: &mut State) {
        if let Some(consumer) = &mut state.consumer {
            consumer.capture.resume();
        }
        state.paused = false;
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
        self.apply_capture_settings(&mut state);
        state.gesture_active = true;
        self.pause_locked(&mut state);
    }

    pub(crate) fn end_waveform_gesture(&self) {
        let mut state = self.lock();
        self.apply_capture_settings(&mut state);
        if !state.gesture_active {
            return;
        }
        state.gesture_active = false;
        state.last_interaction = Instant::now();
        if state.paused && state.idle_resume == AutoResumeDelay::Immediate {
            Self::resume_locked(&mut state);
            if let Some(consumer) = &mut state.consumer {
                self.accept_input_when_safe(consumer);
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Option<Snapshot> {
        let mut state = self.lock();
        self.apply_capture_settings(&mut state);
        state
            .consumer
            .as_ref()
            .map(|consumer| consumer.capture.snapshot())
    }

    pub(crate) fn waveform(&self, minimum: &mut [f32], maximum: &mut [f32]) -> (usize, u32) {
        let mut state = self.lock();
        self.apply_capture_settings(&mut state);
        let Some(consumer) = &mut state.consumer else {
            minimum.fill(0.0);
            maximum.fill(0.0);
            return (0, 0);
        };
        (consumer.waveform(minimum, maximum), consumer.sample_rate)
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

    fn consume(&self) {
        let mut state = self.lock();
        self.apply_capture_settings(&mut state);
        if state.paused
            && !state.gesture_active
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
            consumer.queue.discard_pending();
        }
        if !accepting && !paused {
            self.accept_input_when_safe(consumer);
        }
    }

    fn accept_input_when_safe(&self, consumer: &mut Consumer) {
        if !self.writer_active.load(Ordering::SeqCst) {
            // A callback may have started just before Pause. Clear anything it published after
            // the frozen snapshot before accepting input again.
            consumer.queue.discard_pending();
            self.accepting.store(true, Ordering::SeqCst);
        }
    }
}

pub(crate) struct Worker {
    session: Arc<Session>,
    queue: Arc<AudioQueue>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    rate: u32,
    channels: usize,
    capacity_frames: usize,
}

impl Worker {
    pub(crate) fn new(
        session: Arc<Session>,
        rate: u32,
        channels: usize,
        max_buffer: u32,
    ) -> Option<Self> {
        if !(1..=2).contains(&channels) {
            return None;
        }
        let (queue, capacity_frames) = {
            let mut state = session.lock();
            session.apply_capture_settings(&mut state);
            let mut capture = Capture::new(rate, channels, state.retention).ok()?;
            if state.paused {
                capture.pause();
            }
            let block_capacity = (max_buffer as usize).saturating_mul(2).min(rate as usize);
            let capacity_frames = (rate as usize / 4)
                .max(block_capacity)
                .max(4096)
                .next_power_of_two();
            let queue = AudioQueue::new(capacity_frames, channels);
            state.consumer = Some(Consumer {
                queue: queue.clone(),
                capture,
                scratch: vec![0.0; 4096 * channels],
                sample_rate: rate,
                channels,
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
        let thread = match thread::Builder::new()
            .name("backtracking-capture".to_owned())
            .spawn(move || {
                while !worker_stop.load(Ordering::Relaxed) {
                    worker_session.consume();
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
            stop,
            thread: Some(thread),
            rate,
            channels,
            capacity_frames,
        })
    }

    pub(crate) fn matches(&self, rate: u32, channels: usize, max_buffer: u32) -> bool {
        self.rate == rate
            && self.channels == channels
            && self.capacity_frames >= max_buffer.min(rate) as usize
    }

    pub(crate) fn push(&self, audio: &[&mut [f32]]) {
        let (left, right): (&[f32], Option<&[f32]>) = match audio {
            [left] if self.channels == 1 => (left, None),
            [left, right] if self.channels == 2 => (left, Some(right)),
            _ => return,
        };
        self.session.writer_active.store(true, Ordering::SeqCst);
        if self.session.accepting.load(Ordering::SeqCst) {
            let written = self.queue.push_planar(left, right);
            if written < left.len() {
                self.session
                    .dropped_frames
                    .fetch_add(left.len() - written, Ordering::Relaxed);
            }
        }
        self.session.writer_active.store(false, Ordering::SeqCst);
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
        self.session.lock().consumer = None;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use nice_plug::params::persist::PersistentField;

    use super::*;

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
        assert_eq!(worker.queue.push_planar(&[0.5; 32], None), 32);
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
    fn idle_resume_waits_until_the_current_gesture_has_ended() {
        for delay in [AutoResumeDelay::Seconds5, AutoResumeDelay::Seconds10] {
            let session = Session::new(SettingsStore::default());
            session.set_idle_resume(delay);
            session.begin_waveform_gesture();
            let after_delay = Instant::now() - Duration::from_secs(u64::from(delay.seconds()) + 1);
            session.lock().last_interaction = after_delay;
            session.consume();
            assert!(session.is_paused());

            session.end_waveform_gesture();
            session.consume();
            assert!(session.is_paused());
            session.lock().last_interaction = after_delay;
            session.consume();
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

        session.end_waveform_gesture();
        assert!(!session.is_paused());
        worker.push(&[&mut [0.5; 32]]);

        session.pause();
        assert_eq!(session.snapshot().unwrap().samples, vec![0.5; 32]);
        session.consume();
        assert!(session.is_paused());
        session.end_waveform_gesture();
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
}

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{AudioQueue, Capture, CaptureState, DurationChoice, Snapshot};

#[derive(Clone)]
pub(crate) struct SessionSettings {
    pub(crate) retention: DurationChoice,
    pub(crate) idle_resume: Option<DurationChoice>,
    pub(crate) output_dir: PathBuf,
}

struct Consumer {
    queue: Arc<AudioQueue>,
    capture: Capture,
    scratch: Vec<f32>,
    sample_rate: u32,
    channels: usize,
    waveform: WaveformCache,
}

#[derive(Default)]
struct WaveformCache {
    revision: Option<u64>,
    minimum: Vec<f32>,
    maximum: Vec<f32>,
    frames: usize,
}

impl Consumer {
    fn drain(&mut self, keep: bool, max_blocks: usize) {
        for _ in 0..max_blocks {
            let frames = self.queue.pop_interleaved(&mut self.scratch);
            if frames == 0 {
                break;
            }
            if keep {
                let _ = self
                    .capture
                    .push_interleaved(&self.scratch[..frames * self.channels]);
            }
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
    settings: SessionSettings,
    consumer: Option<Consumer>,
    paused: bool,
    gesture_active: bool,
    last_interaction: Instant,
}

pub(crate) struct Session {
    state: Mutex<State>,
    accepting: AtomicBool,
    writer_active: AtomicBool,
    dropped_frames: AtomicUsize,
}

impl Session {
    pub(crate) fn new() -> Self {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            state: Mutex::new(State {
                settings: SessionSettings {
                    retention: DurationChoice::Seconds20,
                    idle_resume: None,
                    output_dir: home.join("Backtracking").join("output"),
                },
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
        self.lock().settings.clone()
    }

    pub(crate) fn set_output_dir(&self, output_dir: PathBuf) {
        self.lock().settings.output_dir = output_dir;
    }

    pub(crate) fn set_retention(&self, retention: DurationChoice) {
        let mut state = self.lock();
        state.settings.retention = retention;
        if let Some(consumer) = &mut state.consumer {
            consumer.capture.set_retention(retention);
        }
    }

    pub(crate) fn set_idle_resume(&self, idle_resume: Option<DurationChoice>) {
        let mut state = self.lock();
        state.settings.idle_resume = idle_resume;
        state.last_interaction = Instant::now();
    }

    pub(crate) fn pause(&self) {
        let mut state = self.lock();
        state.last_interaction = Instant::now();
        if state.paused {
            return;
        }
        self.accepting.store(false, Ordering::SeqCst);
        if let Some(consumer) = &mut state.consumer {
            // Bring the snapshot up to date with the audio already published by the producer.
            consumer.drain(true, usize::MAX);
            consumer.capture.pause();
        }
        state.paused = true;
    }

    pub(crate) fn resume(&self) {
        Self::resume_locked(&mut self.lock());
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

    pub(crate) fn snapshot(&self) -> Option<Snapshot> {
        let state = self.lock();
        state
            .consumer
            .as_ref()
            .map(|consumer| consumer.capture.snapshot())
    }

    pub(crate) fn waveform(&self, minimum: &mut [f32], maximum: &mut [f32]) -> (usize, u32) {
        let mut state = self.lock();
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
        state
            .consumer
            .as_ref()
            .is_some_and(|consumer| matches!(consumer.capture.state(), CaptureState::Recording))
    }

    pub(crate) fn dropped_frames(&self) -> usize {
        self.dropped_frames.load(Ordering::Relaxed)
    }

    fn consume(&self) {
        let mut state = self.lock();
        if state.paused
            && !state.gesture_active
            && state.settings.idle_resume.is_some_and(|delay| {
                state.last_interaction.elapsed() >= Duration::from_secs(u64::from(delay.seconds()))
            })
        {
            Self::resume_locked(&mut state);
        }

        let paused = state.paused;
        let Some(consumer) = &mut state.consumer else {
            return;
        };
        let accepting = self.accepting.load(Ordering::SeqCst);
        // Yield the session lock regularly even when a DAW renders ahead faster than realtime.
        consumer.drain(accepting && !paused, 8);
        if !accepting && !paused && !self.writer_active.load(Ordering::SeqCst) {
            // A callback may have started just before Pause. Clear anything it published after
            // the frozen snapshot before accepting input again.
            consumer.drain(false, usize::MAX);
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
            let mut capture = Capture::new(rate, channels, state.settings.retention).ok()?;
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
    use super::*;

    #[test]
    fn pauses_without_recording_later_audio_and_appends_after_resuming() {
        let session = Arc::new(Session::new());
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
    fn idle_resume_waits_until_the_current_gesture_has_ended() {
        let session = Session::new();
        session.pause();
        session.set_idle_resume(Some(DurationChoice::Seconds10));
        session.set_gesture_active(true);
        session.lock().last_interaction = Instant::now() - Duration::from_secs(11);
        session.consume();
        assert!(session.is_paused());

        session.set_gesture_active(false);
        session.consume();
        assert!(session.is_paused());
        session.lock().last_interaction = Instant::now() - Duration::from_secs(11);
        session.consume();
        assert!(!session.is_paused());
    }
}

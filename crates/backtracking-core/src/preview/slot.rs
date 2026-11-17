//! Hands previews to an audio thread that must never lock, allocate or free.
//!
//! The editor starts, changes and stops a preview from its own thread. A host's audio thread
//! renders it a block at a time. A preview replaced or finished is freed by whichever other
//! thread let it go, once no render can still be reading it.

use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{Preview, PreviewGate};
use crate::capture_clock_ns;

/// Rendering counts as stopped once no frames have played for this long, which is longer than
/// any host's block.
const RENDER_GAP_NS: u64 = 500_000_000;
/// The most output latency the gate is held for on top of its lease.
const MAX_OUTPUT_LATENCY: Duration = Duration::from_secs(5);

/// What one render of a [`PreviewSlot`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotRender {
    /// Frames handed to `emit`.
    pub frames: usize,
    /// Frames the preview still has to play after these, a requested stop ramp included.
    pub remaining: u64,
}

/// Holds the preview an audio thread plays. See the module documentation.
pub struct PreviewSlot {
    /// The preview in the slot, from `Box::into_raw`, or null.
    current: AtomicPtr<Preview>,
    /// Counted up as each render begins and again as it ends, so it is odd while a render may
    /// be reading `current`.
    renders: AtomicU64,
    /// When a render last played frames, on the capture clock, or 0 before the first.
    last_render_ns: AtomicU64,
    /// How much later than it is rendered the output may play a frame, in nanoseconds.
    output_latency_ns: AtomicU64,
    /// Held by the threads that read or replace the preview. Never by the audio thread.
    owner: Mutex<()>,
    gate: &'static PreviewGate,
}

impl Default for PreviewSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewSlot {
    /// A slot whose previews hold the gate every Backtracking recorder on this computer reads.
    pub fn new() -> Self {
        Self::with_gate(PreviewGate::shared())
    }

    pub fn with_gate(gate: &'static PreviewGate) -> Self {
        Self {
            current: AtomicPtr::new(ptr::null_mut()),
            renders: AtomicU64::new(0),
            last_render_ns: AtomicU64::new(0),
            output_latency_ns: AtomicU64::new(0),
            owner: Mutex::new(()),
            gate,
        }
    }

    /// Plays `preview` from its start, in place of whatever was in the slot.
    pub fn play(&self, preview: Preview) {
        let _owner = self.lock();
        drop(self.replace(Box::into_raw(Box::new(preview))));
    }

    /// Asks the preview in the slot to fall silent over a few milliseconds. The slot lets it go
    /// once it has. When nothing is rendering, as in a host that has stopped processing, it lets
    /// the preview go at once instead, so the ramp cannot play whenever rendering resumes.
    pub fn stop(&self) {
        if self.rendering() {
            self.with_current(Preview::stop);
        } else {
            self.clear();
        }
    }

    /// Holds the gate this much longer after each render, for an output that plays a frame
    /// later than it is rendered. At most five seconds count.
    pub fn set_output_latency(&self, latency: Duration) {
        let nanos = latency.min(MAX_OUTPUT_LATENCY).as_nanos() as u64;
        self.output_latency_ns.store(nanos, Ordering::Relaxed);
    }

    /// Lets go of whatever is in the slot at once, without a ramp.
    pub fn clear(&self) {
        let _owner = self.lock();
        drop(self.replace(ptr::null_mut()));
    }

    /// Runs `read` on the preview in the slot, playing or finished, if there is one.
    pub fn with_current<R>(&self, read: impl FnOnce(&Preview) -> R) -> Option<R> {
        let _owner = self.lock();
        // SAFETY: only a thread holding `owner` frees a preview, and this thread holds it.
        unsafe { self.current.load(Ordering::Acquire).as_ref() }.map(read)
    }

    /// Lets go of a preview that has finished, or that was asked to stop and that nothing is
    /// rendering. Returns whether the slot still holds one. The preview is freed on the calling
    /// thread.
    pub fn reap(&self) -> bool {
        let _owner = self.lock();
        // SAFETY: as in `with_current`.
        match unsafe { self.current.load(Ordering::Acquire).as_ref() } {
            Some(preview)
                if preview.is_finished() || (preview.stop_requested() && !self.rendering()) =>
            {
                drop(self.replace(ptr::null_mut()));
                false
            }
            current => current.is_some(),
        }
    }

    /// Whether a render has played frames lately.
    fn rendering(&self) -> bool {
        let last = self.last_render_ns.load(Ordering::Acquire);
        last != 0 && capture_clock_ns().saturating_sub(last) <= RENDER_GAP_NS
    }

    /// Whether the slot holds a preview, playing or not yet let go. One atomic load.
    pub fn is_occupied(&self) -> bool {
        !self.current.load(Ordering::Relaxed).is_null()
    }

    /// Renders up to `frames` of the preview in the slot when it was captured at `sample_rate`,
    /// handing each stereo frame to `emit` with its index, and holds the preview gate while it
    /// plays. For the audio thread: it never locks, allocates, frees or waits.
    pub fn render_with(
        &self,
        frames: usize,
        sample_rate: u32,
        emit: impl FnMut(usize, [f32; 2]),
    ) -> SlotRender {
        let _render = Render::begin(&self.renders);
        let current = self.current.load(Ordering::SeqCst);
        // SAFETY: a preview taken out of the slot is freed only after every render that may
        // have loaded it has ended. See `replace`.
        let Some(preview) = (unsafe { current.as_ref() }) else {
            return SlotRender::default();
        };
        if preview.sample_rate() != sample_rate {
            return SlotRender::default();
        }
        let rendered = preview.render_with(frames, emit);
        if rendered != 0 {
            let now = capture_clock_ns();
            self.last_render_ns.store(now, Ordering::Release);
            let lease = PreviewGate::LEASE.as_nanos() as u64
                + self.output_latency_ns.load(Ordering::Relaxed);
            self.gate.hold(now, now.saturating_add(lease));
        }
        SlotRender { frames: rendered, remaining: preview.remaining() }
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.owner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Puts `next` in the slot and returns what was there, once no render can still be reading
    /// it. The caller holds `owner`.
    fn replace(&self, next: *mut Preview) -> Option<Box<Preview>> {
        let previous = self.current.swap(next, Ordering::SeqCst);
        let renders = self.renders.load(Ordering::SeqCst);
        if renders % 2 == 1 {
            // A render that began before the swap may hold the previous preview. Any render
            // that begins after it loads `next`, so only this one is waited for.
            while self.renders.load(Ordering::Acquire) == renders {
                std::thread::yield_now();
            }
        }
        // SAFETY: `previous` came from `Box::into_raw` in `play`, and no render reads it now.
        (!previous.is_null()).then(|| unsafe { Box::from_raw(previous) })
    }
}

impl Drop for PreviewSlot {
    fn drop(&mut self) {
        let current = *self.current.get_mut();
        if !current.is_null() {
            // SAFETY: as in `replace`; nothing renders a slot that is being dropped.
            drop(unsafe { Box::from_raw(current) });
        }
    }
}

/// Marks a render in progress for as long as it lives, even if the render unwinds.
struct Render<'a>(&'a AtomicU64);

impl<'a> Render<'a> {
    fn begin(renders: &'a AtomicU64) -> Self {
        renders.fetch_add(1, Ordering::SeqCst);
        Self(renders)
    }
}

impl Drop for Render<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}

const _: () = {
    // The slot hands previews between threads.
    const fn shared<T: Send + Sync>() {}
    shared::<Preview>();
};

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::{ChannelRoute, Fades, FrozenTrack};

    fn gate() -> &'static PreviewGate {
        Box::leak(Box::new(PreviewGate::local()))
    }

    fn ones(frames: usize, rate: u32) -> Preview {
        let track = FrozenTrack::from_interleaved(rate, 1, 0, &vec![1.0; frames]).unwrap();
        Preview::new(&[track], 0..frames as u64, &[ChannelRoute::Merged], false, Fades::NONE)
            .unwrap()
    }

    fn render(slot: &PreviewSlot, frames: usize) -> (Vec<f32>, SlotRender) {
        let mut left = Vec::new();
        let rendered = slot.render_with(frames, 48_000, |index, [sample, _]| {
            assert_eq!(index, left.len());
            left.push(sample);
        });
        (left, rendered)
    }

    #[test]
    fn an_empty_slot_renders_nothing_and_a_full_one_plays_its_preview() {
        let slot = PreviewSlot::with_gate(gate());
        assert_eq!(render(&slot, 64).1, SlotRender::default());
        assert!(!slot.is_occupied());
        slot.play(ones(100, 48_000));
        assert!(slot.is_occupied());
        let (left, rendered) = render(&slot, 64);
        assert_eq!(left, vec![1.0; 64]);
        assert_eq!(rendered, SlotRender { frames: 64, remaining: 36 });
        assert_eq!(slot.with_current(Preview::rendered), Some(64));
        assert_eq!(render(&slot, 64).1, SlotRender { frames: 36, remaining: 0 });
    }

    #[test]
    fn a_preview_captured_at_another_rate_stays_silent() {
        let slot = PreviewSlot::with_gate(gate());
        slot.play(ones(100, 44_100));
        assert_eq!(render(&slot, 64).1, SlotRender::default());
        assert_eq!(slot.with_current(Preview::rendered), Some(0));
    }

    #[test]
    fn rendering_holds_the_gate_and_an_empty_render_does_not() {
        let gate = gate();
        let slot = PreviewSlot::with_gate(gate);
        let before = capture_clock_ns();
        render(&slot, 64);
        assert_eq!(gate.covered(before, 48_000, 48_000), 0..0);
        slot.play(ones(1_000, 48_000));
        render(&slot, 64);
        // A quarter of a second captured from now on was possibly played into.
        let covered = gate.covered(capture_clock_ns(), 12_000, 48_000);
        assert_eq!(covered, 0..12_000);
    }

    #[test]
    fn a_finished_preview_is_let_go_and_a_playing_one_is_kept() {
        let slot = PreviewSlot::with_gate(gate());
        slot.play(ones(100, 48_000));
        render(&slot, 50);
        assert!(slot.reap());
        assert!(slot.is_occupied());
        render(&slot, 50);
        assert!(!slot.reap());
        assert!(!slot.is_occupied());
        assert_eq!(slot.with_current(Preview::rendered), None);
    }

    #[test]
    fn stopping_ramps_down_and_then_the_preview_can_be_let_go() {
        let slot = PreviewSlot::with_gate(gate());
        slot.play(ones(48_000, 48_000));
        render(&slot, 100);
        slot.stop();
        // 8 ms at 48 kHz: the ramp is counted before it has begun.
        assert_eq!(slot.with_current(Preview::remaining), Some(384));
        let (tail, rendered) = render(&slot, 1_000);
        assert_eq!(rendered, SlotRender { frames: 384, remaining: 0 });
        assert_eq!(*tail.last().unwrap(), 0.0);
        assert!(!slot.reap());
    }

    #[test]
    fn a_stop_while_nothing_renders_lets_the_preview_go_at_once() {
        let slot = PreviewSlot::with_gate(gate());
        // A host that is not processing never renders the preview.
        slot.play(ones(1_000, 48_000));
        slot.stop();
        assert!(!slot.is_occupied());

        // One stopped while it plays keeps its ramp, until rendering stops before it is done.
        slot.play(ones(48_000, 48_000));
        render(&slot, 100);
        slot.stop();
        assert!(slot.reap());
        let long_ago = capture_clock_ns().saturating_sub(2 * RENDER_GAP_NS).max(1);
        slot.last_render_ns.store(long_ago, Ordering::Release);
        assert!(!slot.reap());
        assert!(!slot.is_occupied());
    }

    #[test]
    fn output_latency_holds_the_gate_that_much_longer() {
        let gate = gate();
        let slot = PreviewSlot::with_gate(gate);
        slot.set_output_latency(Duration::from_secs(1));
        slot.play(ones(1_000, 48_000));
        render(&slot, 64);
        // A second and a quarter from now is still covered; the lease alone ends sooner.
        assert_eq!(gate.covered(capture_clock_ns(), 60_000, 48_000), 0..60_000);
    }

    #[test]
    fn playing_again_restarts_and_clearing_empties_the_slot() {
        let slot = PreviewSlot::with_gate(gate());
        slot.play(ones(100, 48_000));
        render(&slot, 80);
        slot.play(ones(100, 48_000));
        assert_eq!(render(&slot, 10).1, SlotRender { frames: 10, remaining: 90 });
        slot.clear();
        assert!(!slot.is_occupied());
        assert_eq!(render(&slot, 10).1, SlotRender::default());
    }

    #[test]
    fn previews_are_replaced_and_let_go_while_another_thread_renders() {
        let slot = Arc::new(PreviewSlot::with_gate(gate()));
        let done = Arc::new(AtomicBool::new(false));
        let audio = {
            let (slot, done) = (slot.clone(), done.clone());
            std::thread::spawn(move || {
                let mut frames = 0;
                while !done.load(Ordering::Relaxed) {
                    frames += slot.render_with(32, 48_000, |_, [left, _]| {
                        // Full level, or part way down a stop ramp.
                        assert!((0.0..=1.0).contains(&left));
                    }).frames;
                }
                frames
            })
        };
        for round in 0..2_000 {
            slot.play(ones(64 + round % 64, 48_000));
            slot.reap();
            if round % 3 == 0 {
                slot.stop();
            }
            if round % 7 == 0 {
                slot.clear();
            }
        }
        slot.play(ones(48_000, 48_000));
        while slot.with_current(Preview::rendered) == Some(0) {
            std::thread::yield_now();
        }
        done.store(true, Ordering::Relaxed);
        assert!(audio.join().unwrap() > 0);
    }
}

//! Tells every Backtracking recorder on this computer when a Backtracking preview may be
//! audible, so that capturing System playback records silence instead of the preview.
//!
//! The desktop app, its extra capture windows and plug-ins inside a DAW are separate
//! processes, so the state lives in a small piece of memory they share by name. Times are
//! [`capture_clock_ns`](crate::capture_clock_ns), which reads alike in every process and
//! starts again from zero when the computer restarts. The memory is of a kind that a restart
//! clears, a named section on Windows and POSIX shared memory on macOS, so a window from
//! before a restart never meets the new clock. No Backtracking records System playback
//! elsewhere, so there the gate stays within the process, as it does wherever the shared
//! memory cannot be opened.

use std::ops::Range;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// When one preview window ends this close to the start of the next, they are one window.
const JOIN_GAP_NS: u64 = 50_000_000;

#[repr(C)]
#[derive(Default)]
struct Window {
    since: AtomicU64,
    until: AtomicU64,
}

impl Window {
    /// Reads the start before the end, which [`Window::set`] writes the other way round. A
    /// window read while it changes is then never narrower than the new one: at worst it still
    /// starts where the old one did, which silences more rather than letting a preview through.
    fn get(&self) -> Range<u64> {
        let since = self.since.load(Ordering::Acquire);
        since..self.until.load(Ordering::Acquire)
    }

    fn set(&self, window: Range<u64>) {
        self.until.store(window.end, Ordering::Release);
        self.since.store(window.start, Ordering::Release);
    }
}

/// What the processes share. New shared memory starts zeroed, which is an empty gate.
#[repr(C)]
#[derive(Default)]
struct GateState {
    current: Window,
    /// The window before the current one, for packets still being read after it ended.
    previous: Window,
}

/// See the module documentation.
pub struct PreviewGate {
    state: &'static GateState,
}

impl PreviewGate {
    /// How far ahead a playing preview holds the gate each time it renders. A player that
    /// stops without saying so releases it this long after its last render.
    pub const LEASE: Duration = Duration::from_millis(300);

    /// The gate shared by every Backtracking process on this computer.
    pub fn shared() -> &'static PreviewGate {
        static SHARED: OnceLock<PreviewGate> = OnceLock::new();
        SHARED.get_or_init(|| PreviewGate {
            state: shared_state().unwrap_or_else(|| Box::leak(Box::default())),
        })
    }

    /// A gate of this process alone.
    pub fn local() -> PreviewGate {
        PreviewGate { state: Box::leak(Box::default()) }
    }

    /// Marks a preview as possibly audible from `since_ns` until at least `until_ns`.
    ///
    /// Two processes holding at the same instant can undo part of each other's change. Each
    /// holds again with its next render, a few milliseconds later.
    pub fn hold(&self, since_ns: u64, until_ns: u64) {
        let current = &self.state.current;
        let held = current.get();
        if held.end != 0 && held.end.saturating_add(JOIN_GAP_NS) >= since_ns {
            // Both ends only move outwards, so a reader sees the window grow.
            current.until.fetch_max(until_ns, Ordering::AcqRel);
            current.since.fetch_min(since_ns, Ordering::AcqRel);
        } else {
            // A new window. The one it follows is kept first, so a reader that finds the new
            // window also finds the old one. See `covered`.
            self.state.previous.set(held);
            current.set(since_ns..until_ns);
        }
    }

    /// The frames, of `frames` captured from `start_ns` on at `sample_rate`, that a preview
    /// may have played into. Frames partly inside a window count as inside it.
    pub fn covered(&self, start_ns: u64, frames: usize, sample_rate: u32) -> Range<usize> {
        let rate = u128::from(sample_rate.max(1));
        let nanos = |frames: usize| (frames as u128 * 1_000_000_000 / rate) as u64;
        let end_ns = start_ns.saturating_add(nanos(frames));
        let mut covered: Option<Range<usize>> = None;
        // The current window is read first. When it is already the new one, the window it
        // replaced is already kept as the previous one.
        let current = self.state.current.get();
        let previous = self.state.previous.get();
        for window in [previous, current] {
            let from = window.start.max(start_ns);
            let to = window.end.min(end_ns);
            if from >= to {
                continue;
            }
            let first = (u128::from(from - start_ns) * rate / 1_000_000_000) as usize;
            let last = (u128::from(to - start_ns) * rate).div_ceil(1_000_000_000) as usize;
            let range = first.min(frames)..last.min(frames);
            covered = Some(match covered {
                Some(other) => other.start.min(range.start)..other.end.max(range.end),
                None => range,
            });
        }
        covered.unwrap_or(0..0)
    }

    /// Silences the frames of `samples`, whole interleaved frames captured from `start_ns` on,
    /// that a preview may have played into. Returns how many frames it silenced.
    pub fn silence(&self, start_ns: u64, samples: &mut [f32], channels: usize, sample_rate: u32)
        -> usize
    {
        if channels == 0 {
            return 0;
        }
        let covered = self.covered(start_ns, samples.len() / channels, sample_rate);
        samples[covered.start * channels..covered.end * channels].fill(0.0);
        covered.len()
    }
}

#[cfg(test)]
fn shared_state() -> Option<&'static GateState> {
    // Tests never touch the gate of a Backtracking that is recording on this computer.
    None
}

#[cfg(all(windows, not(test)))]
fn shared_state() -> Option<&'static GateState> {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(
            file: *mut c_void,
            attributes: *const c_void,
            protect: u32,
            maximum_size_high: u32,
            maximum_size_low: u32,
            name: *const u16,
        ) -> *mut c_void;
        fn MapViewOfFile(
            mapping: *mut c_void,
            access: u32,
            offset_high: u32,
            offset_low: u32,
            bytes: usize,
        ) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;
    const PAGE_READWRITE: u32 = 0x04;
    const FILE_MAP_WRITE: u32 = 0x02;

    // `Local\` keeps the gate to this sign-in session, as the audio it guards is.
    let name: Vec<u16> = "Local\\Backtracking.PreviewGate.1".encode_utf16().chain([0]).collect();
    let size = std::mem::size_of::<GateState>();
    // SAFETY: the name is NUL-terminated, the mapping is backed by the paging file and at
    // least `size` bytes long, and the view is page-aligned. Both stay open for the life of
    // the process, so the reference never dangles. Every field is an atomic.
    unsafe {
        let mapping = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            0,
            size as u32,
            name.as_ptr(),
        );
        if mapping.is_null() {
            return None;
        }
        let view = MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, size);
        if view.is_null() {
            CloseHandle(mapping);
            return None;
        }
        Some(&*view.cast::<GateState>())
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn shared_state() -> Option<&'static GateState> {
    use std::ffi::{CString, c_char, c_int, c_uint, c_void};

    unsafe extern "C" {
        fn shm_open(name: *const c_char, flags: c_int, ...) -> c_int;
        fn ftruncate(file: c_int, length: i64) -> c_int;
        fn close(file: c_int) -> c_int;
        fn getuid() -> u32;
        fn mmap(
            address: *mut c_void,
            length: usize,
            protection: c_int,
            flags: c_int,
            file: c_int,
            offset: i64,
        ) -> *mut c_void;
    }
    const O_RDWR: c_int = 0x0002;
    const O_CREAT: c_int = 0x0200;
    const PROT_READ_WRITE: c_int = 0x1 | 0x2;
    const MAP_SHARED: c_int = 0x1;
    // One page, whatever this version uses of it, so a later layout never needs to resize.
    const SIZE: usize = 4_096;

    // Shared memory names are seen by every user of the Mac and are at most 31 bytes long.
    // SAFETY: getuid cannot fail.
    let name = CString::new(format!("/bt.preview-gate.1.{}", unsafe { getuid() })).ok()?;
    // SAFETY: the name is NUL-terminated, and the mode is passed as the variadic argument
    // shm_open reads it as. The view is page-aligned and is mapped only if the memory is at
    // least that long. A shared mapping outlives the descriptor, and it is never unmapped.
    // Every field is an atomic.
    unsafe {
        let file = shm_open(name.as_ptr(), O_RDWR | O_CREAT, 0o600 as c_uint);
        if file < 0 {
            return None;
        }
        // Only the first process to open the memory can size it. For the rest this fails, and
        // mapping checks the size instead.
        let _ = ftruncate(file, SIZE as i64);
        let view = mmap(std::ptr::null_mut(), SIZE, PROT_READ_WRITE, MAP_SHARED, file, 0);
        close(file);
        if view as isize == -1 || view.is_null() {
            return None;
        }
        Some(&*view.cast::<GateState>())
    }
}

#[cfg(all(not(any(windows, target_os = "macos")), not(test)))]
fn shared_state() -> Option<&'static GateState> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn an_empty_gate_covers_nothing() {
        let gate = PreviewGate::local();
        assert_eq!(gate.covered(0, 480, 48_000), 0..0);
        assert_eq!(gate.covered(5_000 * MS, 480, 48_000), 0..0);
    }

    #[test]
    fn a_held_window_covers_the_frames_captured_inside_it() {
        let gate = PreviewGate::local();
        gate.hold(1_000 * MS, 1_100 * MS);
        // Ten-millisecond packets at 48 kHz.
        assert_eq!(gate.covered(980 * MS, 480, 48_000), 0..0);
        assert_eq!(gate.covered(995 * MS, 480, 48_000), 240..480);
        assert_eq!(gate.covered(1_050 * MS, 480, 48_000), 0..480);
        assert_eq!(gate.covered(1_095 * MS, 480, 48_000), 0..240);
        assert_eq!(gate.covered(1_100 * MS, 480, 48_000), 0..0);

        let mut samples = vec![1.0f32; 480 * 2];
        assert_eq!(gate.silence(995 * MS, &mut samples, 2, 48_000), 240);
        assert!(samples[..480].iter().all(|&sample| sample == 1.0));
        assert!(samples[480..].iter().all(|&sample| sample == 0.0));
    }

    #[test]
    fn holding_again_extends_the_window_and_a_later_preview_keeps_the_earlier_one() {
        let gate = PreviewGate::local();
        gate.hold(1_000 * MS, 1_100 * MS);
        gate.hold(1_000 * MS, 1_400 * MS);
        assert_eq!(gate.covered(1_300 * MS, 480, 48_000), 0..480);

        // A preview a second later starts a new window and the old one is still answered.
        gate.hold(2_500 * MS, 2_800 * MS);
        assert_eq!(gate.covered(1_300 * MS, 480, 48_000), 0..480);
        assert_eq!(gate.covered(2_600 * MS, 480, 48_000), 0..480);
        // The quiet between them stays recorded.
        assert_eq!(gate.covered(2_000 * MS, 480, 48_000), 0..0);
    }

    #[test]
    fn previews_that_almost_touch_join_into_one_window() {
        let gate = PreviewGate::local();
        gate.hold(1_000 * MS, 1_100 * MS);
        gate.hold(1_120 * MS, 1_300 * MS);
        assert_eq!(gate.covered(1_105 * MS, 480, 48_000), 0..480);
    }
}

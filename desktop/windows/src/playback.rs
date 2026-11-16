//! Plays a selection preview on the default output device.
//!
//! While the preview may be heard it holds the shared preview gate, so every Backtracking
//! recorder that captures System playback records silence instead of the preview.

use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use backtracking_core::{Preview, PreviewGate, capture_clock_ns};
use windows::Win32::Foundation::{CloseHandle, HANDLE, RPC_E_CHANGED_MODE};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient,
    IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Error, PCWSTR};

/// The device buffer: short enough that stopping is heard at once, long enough to ride out
/// a late wake-up of this thread.
const BUFFER_100NS: i64 = 400_000;
/// The longest wait for the device's next request before checking again anyway.
const WAIT_MS: u32 = 200;
/// After the last sample has left the buffer, the gate stays held this long for capture
/// packets that are still on their way.
const TAIL: Duration = Duration::from_millis(150);
/// A device that has taken no audio for this long has stopped playing, whether or not it says
/// so. Long enough for a wireless device that is slow to start.
const STALL: Duration = Duration::from_secs(3);
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// A preview playing on its own thread. Dropping it stops the preview; the thread lets the
/// device drain and ends by itself, so nothing waits on the window's thread.
pub(crate) struct PreviewPlayer {
    preview: Arc<Preview>,
    shared: Arc<Shared>,
}

struct Shared {
    /// Frames of the selection that have left the device buffer.
    heard: AtomicU64,
    done: AtomicBool,
    error: Mutex<Option<String>>,
}

impl PreviewPlayer {
    /// Starts `preview`. `repaint` runs on the player thread once it has finished.
    pub(crate) fn start(preview: Preview, repaint: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let preview = Arc::new(preview);
        let shared = Arc::new(Shared {
            heard: AtomicU64::new(0),
            done: AtomicBool::new(false),
            error: Mutex::new(None),
        });
        let (playing, state) = (Arc::clone(&preview), Arc::clone(&shared));
        thread::Builder::new()
            .name("backtracking-preview".into())
            .spawn(move || {
                if let Err(error) = play(&playing, &state) {
                    playing.stop();
                    *state.error.lock().unwrap_or_else(|error| error.into_inner()) = Some(error);
                }
                state.done.store(true, Ordering::Release);
                repaint();
            })
            .map_err(|error| format!("Could not start the preview: {error}"))?;
        Ok(Self { preview, shared })
    }

    pub(crate) fn preview(&self) -> &Preview {
        &self.preview
    }

    /// The frame being heard, on the retained-audio timeline.
    pub(crate) fn playhead(&self) -> u64 {
        self.preview.frames().start + self.shared.heard.load(Ordering::Acquire).min(self.preview.len())
    }

    /// Whether the preview has played to its end, been stopped, or failed.
    pub(crate) fn is_done(&self) -> bool {
        self.shared.done.load(Ordering::Acquire)
    }

    pub(crate) fn take_error(&self) -> Option<String> {
        self.shared.error.lock().unwrap_or_else(|error| error.into_inner()).take()
    }
}

impl Drop for PreviewPlayer {
    fn drop(&mut self) {
        self.preview.stop();
    }
}

struct Apartment(bool);

impl Apartment {
    fn initialize() -> Result<Self, String> {
        let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if status == RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        status.ok().map_err(|error| failure("Could not start audio output", error))?;
        Ok(Self(true))
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn failure(operation: &str, error: Error) -> String {
    format!("{operation} ({})", error.message())
}

fn play(preview: &Preview, shared: &Shared) -> Result<(), String> {
    let _apartment = Apartment::initialize()?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| failure("Could not list audio devices", error))?;
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|error| failure("No playback device to preview on", error))?;
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|error| failure("Could not open the playback device", error))?;
    let rate = preview.sample_rate();
    // Stereo float at the capture's own rate; Windows converts it to the device's format.
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: 2,
        nSamplesPerSec: rate,
        nAvgBytesPerSec: rate * 8,
        nBlockAlign: 8,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_100NS, 0, &format, None) }
        .map_err(|error| failure("Could not open the playback device", error))?;
    let event = Event(
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map_err(|error| failure("Could not start audio output", error))?,
    );
    unsafe { client.SetEventHandle(event.0) }
        .map_err(|error| failure("Could not start audio output", error))?;
    let capacity = unsafe { client.GetBufferSize() }
        .map_err(|error| failure("Could not start audio output", error))? as usize;
    let render: IAudioRenderClient = unsafe { client.GetService() }
        .map_err(|error| failure("Could not start audio output", error))?;

    let fill = |frames: usize| -> Result<(), String> {
        if frames == 0 {
            return Ok(());
        }
        let data = unsafe { render.GetBuffer(frames as u32) }
            .map_err(|error| failure("Preview playback stopped", error))?;
        // GetBuffer hands out `frames` whole frames of the stereo float format above.
        let samples = unsafe { slice::from_raw_parts_mut(data.cast::<f32>(), frames * 2) };
        preview.render_interleaved(samples, 2);
        unsafe { render.ReleaseBuffer(frames as u32, 0) }
            .map_err(|error| failure("Preview playback stopped", error))
    };

    let gate = PreviewGate::shared();
    let since = capture_clock_ns();
    gate.hold(since, since + PreviewGate::LEASE.as_nanos() as u64);
    fill(capacity)?;
    unsafe { client.Start() }.map_err(|error| failure("Could not start audio output", error))?;
    let mut last_heard = 0;
    let mut progressed = Instant::now();
    let result = loop {
        unsafe { WaitForSingleObject(event.0, WAIT_MS) };
        let padding = match unsafe { client.GetCurrentPadding() } {
            Ok(padding) => padding as usize,
            Err(error) => break Err(failure("Preview playback stopped", error)),
        };
        let heard = preview.rendered().saturating_sub(padding as u64);
        shared.heard.store(heard, Ordering::Release);
        if preview.is_finished() && padding == 0 {
            break Ok(());
        }
        // Otherwise a device that stops taking audio would hold the gate, and keep System
        // capture silent, for as long as this thread waits on it.
        if heard != last_heard {
            last_heard = heard;
            progressed = Instant::now();
        } else if progressed.elapsed() >= STALL {
            break Err("Preview playback stopped (the playback device stopped taking audio)".into());
        }
        gate.hold(since, capture_clock_ns() + PreviewGate::LEASE.as_nanos() as u64);
        if !preview.is_finished()
            && let Err(error) = fill(capacity.saturating_sub(padding))
        {
            break Err(error);
        }
    };
    let _ = unsafe { client.Stop() };
    shared.heard.store(preview.rendered(), Ordering::Release);
    gate.hold(since, capture_clock_ns() + TAIL.as_nanos() as u64);
    result
}

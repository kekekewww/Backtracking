use std::mem::{ManuallyDrop, size_of};
use std::ptr::{self, NonNull};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{ChannelRoute, TimedAudioQueue, TimedPacket};
use windows::Win32::Foundation::{HANDLE, RPC_E_CHANGED_MODE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    ActivateAudioInterfaceAsync, DEVICE_STATE_ACTIVE, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    BLOB, CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, IAgileObject, IAgileObject_Impl,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Error, GUID, HRESULT, HSTRING, Interface, PCWSTR, Ref, implement};

mod sources;
mod timing;
pub(crate) use sources::CaptureSource;
use sources::{OwnedHandle, RunningProcess, target_process};
use timing::PacketClock;

/// Retained silence stops this far behind the newest frame the device could
/// still be holding, so a packet that is being delivered is never overwritten.
const IDLE_CUTOFF_GUARD: Duration = Duration::from_millis(4);
/// Bounds for the timed-polling fallback, which follows the device period.
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(3);
const MAX_POLL_INTERVAL: Duration = Duration::from_millis(20);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Until a stream's notification event has announced a packet, never sleep
/// longer than this (or half the endpoint buffer, whichever is shorter).
const MAX_UNPROVEN_WAIT: Duration = Duration::from_millis(50);
const DEVICE_BUFFER_100NS: i64 = 1_000_000;
const IDLE_PACKET_GAP: Duration = Duration::from_millis(200);
const RETAINED_IDLE_INTERVAL: Duration = Duration::from_millis(20);
const QUIET_MODE_TIMEOUT: Duration = Duration::from_millis(250);
const ENDPOINT_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(3);
pub(crate) const PACKET_QUEUE_CAPACITY: usize = 1024;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

type Reply = mpsc::SyncSender<Result<(), String>>;

/// Called by the capture thread after it queued packets or reported an error,
/// so the consumer can block instead of polling the queue.
pub(crate) type DataWake = Arc<dyn Fn() + Send + Sync>;

/// Auto-reset Win32 event that wakes the capture thread for commands.
struct Signal(OwnedHandle);

// Event handles may be signalled and waited on from any thread.
unsafe impl Send for Signal {}
unsafe impl Sync for Signal {}

impl Signal {
    fn new() -> Result<Self, String> {
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map(|handle| Self(OwnedHandle(handle)))
            .map_err(|error| audio_error("無法建立 WASAPI 喚醒事件", error))
    }

    fn set(&self) {
        let _ = unsafe { SetEvent(self.0.0) };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Woke {
    Signal,
    Audio,
    Timeout,
}

/// Blocks until a command wake, the audio notification event, or the timeout.
/// `None` waits indefinitely.
fn wait_for(signal: &Signal, audio: Option<HANDLE>, timeout: Option<Duration>) -> Woke {
    let milliseconds = timeout.map_or(INFINITE, |timeout| {
        // Round up so a deadline is never serviced early and re-armed at zero.
        timeout.as_nanos().div_ceil(1_000_000).min(u128::from(INFINITE - 1)) as u32
    });
    let handles = [signal.0.0, audio.unwrap_or_default()];
    let count = if audio.is_some() { 2 } else { 1 };
    let result = unsafe { WaitForMultipleObjects(&handles[..count], false, milliseconds) };
    if result == WAIT_OBJECT_0 {
        Woke::Signal
    } else if audio.is_some() && result.0 == WAIT_OBJECT_0.0 + 1 {
        Woke::Audio
    } else {
        if result != WAIT_TIMEOUT {
            // A failed wait must not turn the capture loop into a busy spin.
            thread::sleep(timeout.unwrap_or(MAX_POLL_INTERVAL).min(MAX_POLL_INTERVAL));
        }
        Woke::Timeout
    }
}

/// Consumes a pending signal of an auto-reset event without waiting.
fn event_is_set(event: HANDLE) -> bool {
    unsafe { WaitForSingleObject(event, 0) == WAIT_OBJECT_0 }
}

/// Whether the stream's notification event can be relied on to announce packets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventWatch {
    /// No packet has been announced by the event yet.
    Unproven,
    Trusted,
    /// One packet arrived without its event; look again after one poll interval.
    Suspect,
    /// The event is absent or missed packets; use timed polling for this stream.
    Polling,
}

impl EventWatch {
    fn new(has_event: bool) -> Self {
        if has_event { Self::Unproven } else { Self::Polling }
    }

    /// `by_event` is true when the event was signalled for this read, even if
    /// that was only observed right after a timeout.
    fn observe(&mut self, by_event: bool, had_packets: bool) {
        *self = match (*self, by_event, had_packets) {
            (Self::Polling, ..) => Self::Polling,
            (_, true, true) => Self::Trusted,
            (Self::Unproven | Self::Suspect, false, true) => Self::Polling,
            (Self::Trusted, false, true) => Self::Suspect,
            (Self::Suspect, false, false) => Self::Trusted,
            (state, _, false) => state,
        };
    }

    fn uses_event(self) -> bool {
        self != Self::Polling
    }

    /// Longest sleep that cannot lose audio if the event stays silent.
    fn wait_cap(self, poll_interval: Duration, unproven_wait: Duration) -> Option<Duration> {
        match self {
            Self::Trusted => None,
            Self::Unproven => Some(unproven_wait),
            Self::Suspect | Self::Polling => Some(poll_interval),
        }
    }

    /// Longest time a delivered packet can sit unread in the endpoint buffer.
    fn read_latency(self, poll_interval: Duration) -> Duration {
        match self {
            Self::Trusted => Duration::ZERO,
            // An unproven event is assumed absent until it shows otherwise.
            Self::Unproven | Self::Suspect | Self::Polling => poll_interval,
        }
    }
}

struct WaitState {
    running: bool,
    backlog: bool,
    quiet_pending: bool,
    retain_silence: bool,
    idle_reported: bool,
    idle_for: Duration,
    since_padding: Duration,
    since_source_check: Duration,
    wait_cap: Option<Duration>,
}

/// How long the capture thread may sleep before it has work that no event
/// announces. `None` means only a command can produce work.
fn wait_timeout(state: &WaitState) -> Option<Duration> {
    if !state.running {
        return None;
    }
    if state.backlog {
        return Some(Duration::ZERO);
    }
    let mut timeout = ENDPOINT_CHECK_INTERVAL.saturating_sub(state.since_source_check);
    if let Some(cap) = state.wait_cap {
        timeout = timeout.min(cap);
    }
    if !state.quiet_pending {
        let until_gap = IDLE_PACKET_GAP.saturating_sub(state.idle_for);
        if state.retain_silence {
            let until_padding = RETAINED_IDLE_INTERVAL.saturating_sub(state.since_padding);
            timeout = timeout.min(until_gap.max(until_padding));
        } else if !state.idle_reported {
            timeout = timeout.min(until_gap);
        }
    }
    Some(timeout)
}

fn poll_interval(device_period_100ns: Option<i64>) -> Duration {
    device_period_100ns
        .filter(|&period| period > 0)
        .map_or(DEFAULT_POLL_INTERVAL, |period| {
            Duration::from_nanos(period as u64 * 100).clamp(MIN_POLL_INTERVAL, MAX_POLL_INTERVAL)
        })
}

fn unproven_wait(buffer_frames: usize, sample_rate: u32, poll_interval: Duration) -> Duration {
    let half_buffer =
        Duration::from_nanos(buffer_frames as u64 * 500_000_000 / u64::from(sample_rate.max(1)));
    half_buffer.min(MAX_UNPROVEN_WAIT).max(poll_interval)
}

enum Command {
    Pause(Reply),
    Resume(Reply),
    PrepareRetainSilence(bool, Reply, mpsc::Receiver<u64>),
    Shutdown,
}

pub(crate) struct QuietModeBarrier {
    release: Option<mpsc::SyncSender<u64>>,
    wake: Arc<Signal>,
}

impl QuietModeBarrier {
    pub(crate) fn cancel(mut self) {
        drop(self.release.take());
        self.wake.set();
    }
}

impl Drop for QuietModeBarrier {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(now_ns());
            // A paused capture thread waits without a timeout.
            self.wake.set();
        }
    }
}

struct PendingQuietMode {
    enabled: bool,
    release: mpsc::Receiver<u64>,
}

#[derive(Default)]
struct QuietModeState {
    retain_silence: bool,
    idle_reported: bool,
    pending: Option<PendingQuietMode>,
}

impl QuietModeState {
    fn prepare(&mut self, enabled: bool, reply: Reply, release: mpsc::Receiver<u64>) {
        if self.pending.is_some() {
            let _ = reply.send(Err("音訊來源設定仍在更新".into()));
            return;
        }
        // Freeze only synthetic packets. Real device packets keep draining
        // while the controller commits all old synthetic data before changing gates.
        self.pending = Some(PendingQuietMode { enabled, release });
        if reply.send(Ok(())).is_err() {
            self.pending = None;
        }
    }

    fn poll_release(&mut self) -> Option<(bool, u64)> {
        let pending = self.pending.as_ref()?;
        let result = match pending.release.try_recv() {
            Ok(since_ns) => Some((pending.enabled, since_ns)),
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        self.pending = None;
        if let Some((enabled, _)) = result {
            self.retain_silence = enabled;
        }
        // Keep idle_reported across policy changes. An already-padded idle
        // episode must not append another legacy 200ms tail after switching off.
        result
    }

    fn note_real_packet(&mut self) {
        self.idle_reported = false;
    }

    fn note_synthetic_packet(&mut self) {
        self.idle_reported = true;
    }

    fn restore_after_resume(&mut self, clock: &mut PacketClock, resume_ns: u64) {
        let _ = self.poll_release();
        self.note_real_packet();
        clock.set_retain_silence(self.retain_silence, resume_ns);
    }

    fn should_emit_tail(&self, idle_for: Duration) -> bool {
        !self.retain_silence && !self.idle_reported && self.pending.is_none()
            && idle_for >= IDLE_PACKET_GAP
    }

    fn should_emit_retained(&self, idle_for: Duration, since_attempt: Duration) -> bool {
        self.retain_silence && self.pending.is_none() && idle_for >= IDLE_PACKET_GAP
            && since_attempt >= RETAINED_IDLE_INTERVAL
    }
}

struct Started {
    format: Arc<Format>,
    packets: Arc<TimedAudioQueue>,
}

pub(crate) struct Loopback {
    pub(crate) format: Arc<Format>,
    packets: Arc<TimedAudioQueue>,
    commands: mpsc::Sender<Command>,
    wake: Arc<Signal>,
    errors: mpsc::Receiver<String>,
    worker: Option<JoinHandle<()>>,
}

impl Loopback {
    /// `data_wake` runs on the capture thread after it queued packets or an error.
    pub(crate) fn start(source: CaptureSource, data_wake: DataWake) -> Result<Self, String> {
        let (commands, command_reader) = mpsc::channel();
        let (errors_writer, errors) = mpsc::channel();
        let (startup_writer, startup) = mpsc::sync_channel(1);
        let wake = Arc::new(Signal::new()?);
        let thread_wake = Arc::clone(&wake);
        let worker = thread::Builder::new()
            .name("backtracking-wasapi".into())
            .spawn(move || {
                capture_thread(
                    source,
                    command_reader,
                    &thread_wake,
                    startup_writer,
                    errors_writer,
                    data_wake,
                )
            })
            .map_err(|error| format!("無法建立 WASAPI 擷取執行緒（{error}）"))?;

        match startup.recv() {
            Ok(Ok(Started { format, packets })) => Ok(Self {
                format,
                packets,
                commands,
                wake,
                errors,
                worker: Some(worker),
            }),
            result => {
                let _ = worker.join();
                Err(result
                    .ok()
                    .and_then(Result::err)
                    .unwrap_or_else(|| "WASAPI 擷取執行緒在啟動時結束".into()))
            }
        }
    }

    pub(crate) fn pause(&self) -> Result<(), String> {
        self.request(Command::Pause)
    }

    pub(crate) fn resume(&self) -> Result<(), String> {
        self.request(Command::Resume)
    }

    /// Call from the capture worker; the UI only queues its controller command.
    pub(crate) fn prepare_retain_silence(&mut self, enabled: bool) -> Result<QuietModeBarrier, String> {
        let (reply, ready) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        self.commands.send(Command::PrepareRetainSilence(enabled, reply, released))
            .map_err(|_| "WASAPI 連線已中斷".to_string())?;
        self.wake.set();
        match ready.recv_timeout(QUIET_MODE_TIMEOUT) {
            Ok(Ok(())) => Ok(QuietModeBarrier {
                release: Some(release),
                wake: Arc::clone(&self.wake),
            }),
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => Err("WASAPI 音訊來源設定更新逾時".into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("WASAPI 連線已中斷".into()),
        }
    }

    pub(crate) fn set_retain_silence(&mut self, enabled: bool) -> Result<(), String> {
        drop(self.prepare_retain_silence(enabled)?);
        Ok(())
    }

    pub(crate) fn packet(&self, scratch: &mut Vec<f32>) -> Option<TimedPacket> {
        self.packets.pop(scratch)
    }

    pub(crate) fn discard_pending(&self, scratch: &mut Vec<f32>) {
        for _ in 0..self.packets.pending_packets() {
            self.packets.pop(scratch);
        }
    }

    pub(crate) fn take_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }

    pub(crate) fn available_sources() -> Vec<CaptureSource> {
        sources::available_sources()
    }

    fn request(&self, command: fn(Reply) -> Command) -> Result<(), String> {
        let (reply, result) = mpsc::sync_channel(1);
        if self.commands.send(command(reply)).is_ok() {
            self.wake.set();
            if let Ok(result) = result.recv() {
                return result;
            }
        }
        Err(self
            .take_error()
            .unwrap_or_else(|| "WASAPI 連線已中斷，請重新連線".into()))
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        self.wake.set();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
pub(crate) fn resolved_process_id_for_test(path: &str) -> Result<u32, String> {
    Ok(target_process(path)?.id)
}

fn capture_thread(
    source: CaptureSource,
    commands: mpsc::Receiver<Command>,
    wake: &Signal,
    startup: mpsc::SyncSender<Result<Started, String>>,
    errors: mpsc::Sender<String>,
    data_wake: DataWake,
) {
    let mut stream = match WasapiStream::open(source) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    let started = Started {
        format: Arc::clone(&stream.format),
        packets: Arc::clone(&stream.packets),
    };
    if startup.send(Ok(started)).is_err() {
        return;
    }

    let mut running = true;
    let mut last_packet = Instant::now();
    let mut last_source_check = last_packet;
    let mut quiet_mode = QuietModeState::default();
    let mut last_padding_attempt = last_packet;
    let mut watch = EventWatch::new(stream.audio_event.is_some());
    let report = |error: String| {
        let _ = errors.send(error);
        data_wake();
    };
    loop {
        if let Some((enabled, since_ns)) = quiet_mode.poll_release() {
            stream.clock.set_retain_silence(enabled, since_ns);
            last_padding_attempt = Instant::now();
        }
        // Commands are taken before every wait; the wake signal only ends a wait.
        let command = match commands.try_recv() {
            Ok(command) => command,
            Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {
                let timeout = wait_timeout(&WaitState {
                    running,
                    backlog: stream.backlog,
                    quiet_pending: quiet_mode.pending.is_some(),
                    retain_silence: quiet_mode.retain_silence,
                    idle_reported: quiet_mode.idle_reported,
                    idle_for: last_packet.elapsed(),
                    since_padding: last_padding_attempt.elapsed(),
                    since_source_check: last_source_check.elapsed(),
                    wait_cap: watch.wait_cap(stream.poll_interval, stream.unproven_wait),
                });
                let audio_event = stream
                    .audio_event
                    .as_ref()
                    .filter(|_| running && watch.uses_event())
                    .map(|event| event.0);
                let woke = wait_for(wake, audio_event, timeout);
                if woke == Woke::Signal || !running {
                    continue;
                }
                if let Some((enabled, since_ns)) = quiet_mode.poll_release() {
                    stream.clock.set_retain_silence(enabled, since_ns);
                    last_padding_attempt = Instant::now();
                }
                if last_source_check.elapsed() >= ENDPOINT_CHECK_INTERVAL {
                    last_source_check = Instant::now();
                    if let Err(error) = stream.verify_source() {
                        report(error);
                        break;
                    }
                }
                let read = stream.read_packets();
                if let Ok(had_packets) = read {
                    // An event that fires between the timeout and this read was on time.
                    let by_event = woke == Woke::Audio
                        || (had_packets && audio_event.is_some_and(event_is_set));
                    watch.observe(by_event, had_packets);
                }
                match read {
                    Ok(true) => {
                        last_packet = Instant::now();
                        quiet_mode.note_real_packet();
                        data_wake();
                    }
                    Ok(false) if quiet_mode.should_emit_retained(last_packet.elapsed(), last_padding_attempt.elapsed()) => {
                        last_padding_attempt = Instant::now();
                        let guard = IDLE_CUTOFF_GUARD.max(watch.read_latency(stream.poll_interval));
                        if stream.push_retained_idle_silence(guard) {
                            quiet_mode.note_synthetic_packet();
                            data_wake();
                        }
                    }
                    Ok(false) if quiet_mode.should_emit_tail(last_packet.elapsed()) => {
                        stream.push_idle_silence();
                        quiet_mode.note_synthetic_packet();
                        data_wake();
                    }
                    Ok(false) => {}
                    Err(error) => {
                        report(error);
                        break;
                    }
                }
                continue;
            }
        };
        match command {
            Command::Shutdown => break,
            Command::PrepareRetainSilence(enabled, reply, release) => {
                quiet_mode.prepare(enabled, reply, release);
            }
            Command::Pause(reply) => {
                let result = if running { stream.pause() } else { Ok(()) };
                let failed = result.is_err();
                let _ = reply.send(result);
                if failed {
                    break;
                }
                running = false;
            }
            Command::Resume(reply) => {
                let result = if running {
                    Ok(())
                } else {
                    stream.verify_source().and_then(|()| stream.start())
                };
                let failed = result.is_err();
                let _ = reply.send(result);
                if failed {
                    break;
                }
                running = true;
                last_packet = Instant::now();
                last_source_check = last_packet;
                last_padding_attempt = last_packet;
                quiet_mode.restore_after_resume(&mut stream.clock, now_ns());
            }
        }
    }
}

struct Apartment(bool);

impl Apartment {
    fn initialize() -> Result<Self, String> {
        let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if status == RPC_E_CHANGED_MODE {
            // UI-side device enumeration can reuse a previously initialized STA.
            return Ok(Self(false));
        }
        status
            .ok()
            .map_err(|error| audio_error("無法初始化 COM", error))?;
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

struct MixFormat(NonNull<WAVEFORMATEX>);

impl Drop for MixFormat {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.as_ptr().cast())) };
    }
}

enum SourceBinding {
    DefaultRender(String),
    DefaultCapture(String),
    Microphone(String),
    Process(RunningProcess),
}

struct WasapiStream {
    capture: IAudioCaptureClient,
    client: IAudioClient,
    enumerator: Option<IMMDeviceEnumerator>,
    binding: SourceBinding,
    format: Arc<Format>,
    packets: Arc<TimedAudioQueue>,
    packet_capacity: usize,
    scratch: Vec<f32>,
    clock: PacketClock,
    queue_loss: bool,
    /// More packets were waiting when the last read reached its per-pass limit.
    backlog: bool,
    /// Timed-polling interval for streams whose event is absent or unreliable.
    poll_interval: Duration,
    unproven_wait: Duration,
    // COM clients are dropped before the event handle and COM apartment.
    audio_event: Option<OwnedHandle>,
    _apartment: Apartment,
}

impl WasapiStream {
    fn open(source: CaptureSource) -> Result<Self, String> {
        let apartment = Apartment::initialize()?;
        let (client, format, binding, enumerator, event) = match source {
            CaptureSource::Application { path, .. } => {
                let process = target_process(&path)?;
                let (client, format, event) = open_process_client(process.id)?;
                (
                    client,
                    format,
                    SourceBinding::Process(process),
                    None,
                    Some(event),
                )
            }
            _ => {
                let enumerator = endpoint_enumerator()?;
                let (device, flags) = match &source {
                    CaptureSource::System => (
                        unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                            .map_err(|error| audio_error("找不到 Windows 預設輸出裝置", error))?,
                        AUDCLNT_STREAMFLAGS_LOOPBACK,
                    ),
                    CaptureSource::DefaultMicrophone => (
                        unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) }
                            .map_err(|error| audio_error("找不到 Windows 預設麥克風", error))?,
                        0,
                    ),
                    CaptureSource::Microphone { id, .. } => (
                        unsafe { enumerator.GetDevice(&HSTRING::from(id.as_str())) }
                            .map_err(|error| audio_error("找不到已選取的麥克風", error))?,
                        0,
                    ),
                    CaptureSource::Application { .. } | CaptureSource::Sender { .. } => unreachable!(),
                };
                let binding = match source {
                    CaptureSource::System => SourceBinding::DefaultRender(endpoint_id(&device)?),
                    CaptureSource::DefaultMicrophone => {
                        SourceBinding::DefaultCapture(endpoint_id(&device)?)
                    }
                    CaptureSource::Microphone { id, .. } => SourceBinding::Microphone(id),
                    CaptureSource::Application { .. } | CaptureSource::Sender { .. } => unreachable!(),
                };
                let (client, format, event) = open_device_client(&device, flags)?;
                (client, format, binding, Some(enumerator), event)
            }
        };

        let capture = unsafe { client.GetService() }
            .map_err(|error| audio_error("無法建立 WASAPI 音訊擷取", error))?;
        let packet_capacity = unsafe { client.GetBufferSize() }
            .map_err(|error| audio_error("無法讀取 WASAPI 緩衝區大小", error))?
            as usize;
        if packet_capacity == 0 || packet_capacity > format.sample_rate as usize * 4 {
            return Err("WASAPI 回傳不支援的緩衝區大小".into());
        }
        let mut device_period = 0;
        let poll_interval = poll_interval(
            unsafe { client.GetDevicePeriod(Some(&mut device_period), None) }
                .ok()
                .map(|()| device_period),
        );
        let unproven_wait = unproven_wait(packet_capacity, format.sample_rate, poll_interval);
        // Short device periods can produce many packets before filling the audio ring.
        let packets = TimedAudioQueue::new(
            format.sample_rate as usize,
            PACKET_QUEUE_CAPACITY,
            format.channels,
        );
        let scratch_frames = packet_capacity.max(format.sample_rate.div_ceil(5) as usize);
        let scratch = vec![0.0; scratch_frames * format.channels];
        let clock = PacketClock::new(format.sample_rate);
        let mut stream = Self {
            capture,
            client,
            enumerator,
            binding,
            format: Arc::new(format),
            packets,
            packet_capacity,
            scratch,
            clock,
            queue_loss: false,
            backlog: false,
            poll_interval,
            unproven_wait,
            audio_event: event,
            _apartment: apartment,
        };
        stream.start()?;
        Ok(stream)
    }

    fn start(&mut self) -> Result<(), String> {
        self.clock = PacketClock::new(self.format.sample_rate);
        self.queue_loss = false;
        self.backlog = false;
        if let Some(event) = &self.audio_event {
            // WASAPI expects the notification event to be nonsignaled at Start.
            let _ = unsafe { ResetEvent(event.0) };
        }
        unsafe { self.client.Start() }.map_err(|error| audio_error("無法開始擷取音訊", error))
    }

    fn verify_source(&self) -> Result<(), String> {
        let enumerator = self.enumerator.as_ref();
        match &self.binding {
            SourceBinding::Process(process) => {
                if !process.running() {
                    return Err("應用程式已結束，等待重新啟動".into());
                }
            }
            SourceBinding::DefaultRender(id) | SourceBinding::DefaultCapture(id) => {
                let flow = if matches!(self.binding, SourceBinding::DefaultRender(_)) {
                    eRender
                } else {
                    eCapture
                };
                let current = unsafe {
                    enumerator
                        .expect("endpoint enumerator")
                        .GetDefaultAudioEndpoint(flow, eConsole)
                }
                .map_err(|error| audio_error("找不到 Windows 預設音訊裝置", error))?;
                if endpoint_id(&current)? != *id {
                    return Err("Windows 預設音訊裝置已切換，正在重新連線".into());
                }
            }
            SourceBinding::Microphone(id) => {
                let device = unsafe {
                    enumerator
                        .expect("endpoint enumerator")
                        .GetDevice(&HSTRING::from(id.as_str()))
                }
                .map_err(|error| audio_error("等待已選取的麥克風重新連線", error))?;
                let state = unsafe { device.GetState() }
                    .map_err(|error| audio_error("無法讀取麥克風狀態", error))?;
                if state != DEVICE_STATE_ACTIVE {
                    return Err("等待已選取的麥克風重新連線".into());
                }
            }
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        let read = self.read_packets();
        let stop =
            unsafe { self.client.Stop() }.map_err(|error| audio_error("無法暫停音訊擷取", error));
        read.and(stop)?;
        unsafe { self.client.Reset() }
            .map_err(|error| audio_error("無法清空 WASAPI 端點緩衝區", error))
    }

    fn read_packets(&mut self) -> Result<bool, String> {
        let mut had_packet = false;
        // The notification event fires once per device period, so a pass that
        // stops at its limit must be continued without waiting for the next one.
        self.backlog = true;
        for _ in 0..64 {
            let next = unsafe { self.capture.GetNextPacketSize() }
                .map_err(|error| audio_error("無法讀取音訊封包", error))?;
            if next == 0 {
                self.backlog = false;
                break;
            }

            let mut data = ptr::null_mut();
            let mut frames = 0;
            let mut flags = 0;
            let mut position = 0;
            let mut qpc_100ns = 0;
            unsafe {
                self.capture.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut position),
                    Some(&mut qpc_100ns),
                )
            }
            .map_err(|error| audio_error("無法取出音訊封包", error))?;
            if frames == 0 {
                self.backlog = false;
                break;
            }

            let result = self.copy_packet(data, frames as usize, flags, position, qpc_100ns);
            let release = unsafe { self.capture.ReleaseBuffer(frames) }
                .map_err(|error| audio_error("無法釋放音訊封包", error));
            result.and(release)?;
            had_packet = true;
        }
        Ok(had_packet)
    }

    fn copy_packet(
        &mut self,
        data: *mut u8,
        frames: usize,
        flags: u32,
        position: u64,
        qpc_100ns: u64,
    ) -> Result<(), String> {
        if frames > self.packet_capacity {
            return Err("WASAPI 封包超出緩衝區容量".into());
        }
        let samples = &mut self.scratch[..frames * self.format.channels];
        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
            samples.fill(0.0);
        } else {
            if data.is_null() {
                return Err("WASAPI 音訊封包沒有資料".into());
            }
            // GetBuffer keeps exactly this packet accessible until ReleaseBuffer.
            let input =
                unsafe { std::slice::from_raw_parts(data, frames * self.format.frame_bytes) };
            self.format.decode(input, samples);
        }
        let valid_timestamp =
            qpc_100ns != 0 && flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0;
        let stamp = self.clock.timestamp(
            position,
            frames,
            valid_timestamp.then(|| qpc_100ns.saturating_mul(100)),
            flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0,
            now_ns,
        );
        // Never block the device when the consumer is busy. Tell it explicitly
        // if a previous queue write discarded audio, including a partial packet.
        self.queue_loss = self.packets.push_interleaved_with_discontinuity(
            stamp.timestamp_ns,
            self.format.sample_rate,
            samples,
            stamp.discontinuity || self.queue_loss,
        ) != frames;
        Ok(())
    }

    fn push_idle_silence(&mut self) {
        let frames = self.format.sample_rate.div_ceil(5) as usize;
        let stamp = self.clock.idle_stamp(frames, now_ns);
        let samples = &mut self.scratch[..frames * self.format.channels];
        samples.fill(0.0);
        let accepted = self.packets.push_interleaved_with_discontinuity(
            stamp.timestamp_ns,
            self.format.sample_rate,
            samples,
            stamp.discontinuity || self.queue_loss,
        );
        if accepted != 0 {
            self.queue_loss = false;
        }
    }

    fn push_retained_idle_silence(&mut self, guard: Duration) -> bool {
        let max_frames = self.format.sample_rate.div_ceil(5) as usize;
        // GetBuffer timestamps refer to each packet's first frame. Leave a
        // complete device buffer plus one read in flight rather than replacing
        // samples that the engine has not made available yet with virtual zeroes.
        let until_ns = self.clock.idle_cutoff_ns(now_ns(), self.packet_capacity,
            guard.as_nanos() as u64);
        let Some((stamp, frames)) = self.clock.retained_idle_stamp(until_ns, max_frames) else {
            return false;
        };
        let samples = &mut self.scratch[..frames * self.format.channels];
        samples.fill(0.0);
        self.queue_loss = self.packets.push_interleaved_with_discontinuity(
            stamp.timestamp_ns,
            self.format.sample_rate,
            samples,
            stamp.discontinuity || self.queue_loss,
        ) != frames;
        true
    }
}

impl Drop for WasapiStream {
    fn drop(&mut self) {
        let _ = unsafe { self.client.Stop() };
    }
}

fn endpoint_enumerator() -> Result<IMMDeviceEnumerator, String> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|error| audio_error("無法取得音訊裝置清單", error))
}

#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct ActivationComplete {
    sender: mpsc::SyncSender<()>,
    // Windows holds a COM reference until it delivers the callback. Keep the
    // activation BLOB alive with it, even when the calling worker times out.
    _arguments: Box<ActivationArguments>,
}

struct ActivationArguments {
    params: AUDIOCLIENT_ACTIVATION_PARAMS,
    // This BLOB borrows params in the same Box. PROPVARIANT's owning Drop
    // would call PropVariantClear and free that borrowed Rust allocation.
    variant: ManuallyDrop<PROPVARIANT>,
}

impl ActivationArguments {
    fn new(pid: u32) -> Box<Self> {
        let mut arguments = Box::new(Self {
            params: AUDIOCLIENT_ACTIVATION_PARAMS {
                ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                    ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                        TargetProcessId: pid,
                        ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            },
            variant: ManuallyDrop::new(PROPVARIANT::default()),
        });
        arguments.variant = ManuallyDrop::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: ptr::from_mut(&mut arguments.params).cast(),
                        },
                    },
                }),
            },
        });
        arguments
    }

    fn property(&self) -> *const PROPVARIANT {
        ptr::from_ref(&*self.variant)
    }
}

impl IAgileObject_Impl for ActivationComplete_Impl {}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationComplete_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.sender.try_send(());
        Ok(())
    }
}

fn activate_process(pid: u32) -> Result<IAudioClient, String> {
    let arguments = ActivationArguments::new(pid);
    let property = arguments.property();
    let (complete, result) = mpsc::sync_channel(1);
    let handler: IActivateAudioInterfaceCompletionHandler = ActivationComplete {
        sender: complete,
        _arguments: arguments,
    }
    .into();
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(property),
            &handler,
        )
    }
    .map_err(|error| audio_error("無法啟用應用程式音訊擷取", error))?;
    result
        .recv_timeout(ACTIVATION_TIMEOUT)
        .map_err(|_| "應用程式音訊啟用逾時")?;
    let mut status = HRESULT(0);
    let mut interface = None;
    unsafe { operation.GetActivateResult(&mut status, &mut interface) }
        .and_then(|()| status.ok())
        .map_err(|error| audio_error("無法取得應用程式音訊介面", error))?;
    interface
        .ok_or_else(|| "Windows 未提供應用程式音訊介面".to_owned())?
        .cast()
        .map_err(|error| audio_error("應用程式音訊介面不支援 WASAPI", error))
}

/// Opens a shared-mode device stream, preferring event-driven delivery and
/// falling back to a stream that is read on a timer.
fn open_device_client(
    device: &IMMDevice,
    flags: u32,
) -> Result<(IAudioClient, Format, Option<OwnedHandle>), String> {
    let loopback = flags & AUDCLNT_STREAMFLAGS_LOOPBACK != 0;
    // Keep today's 100 ms endpoint buffer first. Microsoft documents a zero
    // duration for event-driven shared streams, so try that before giving up
    // on events. A client whose Initialize failed cannot be reused.
    let attempts: &[(bool, i64)] = if !loopback || sources::supports_event_loopback() {
        &[(true, DEVICE_BUFFER_100NS), (true, 0), (false, DEVICE_BUFFER_100NS)]
    } else {
        &[(false, DEVICE_BUFFER_100NS)]
    };
    let mut last_error = None;
    for &(with_event, buffer_100ns) in attempts {
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| audio_error("無法開啟已選取的音訊來源", error))?;
        let native = unsafe { client.GetMixFormat() }
            .map_err(|error| audio_error("無法讀取來源格式", error))?;
        let native = MixFormat(NonNull::new(native).ok_or("WASAPI 未提供有效的來源格式")?);
        let format = unsafe { Format::parse(native.0) }?;
        let stream_flags = if with_event {
            flags | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        } else {
            flags
        };
        if let Err(error) = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                stream_flags,
                buffer_100ns,
                0,
                native.0.as_ptr(),
                None,
            )
        } {
            last_error = Some(error);
            continue;
        }
        if !with_event {
            return Ok((client, format, None));
        }
        if let Ok(event) = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map(OwnedHandle)
            && unsafe { client.SetEventHandle(event.0) }.is_ok()
        {
            return Ok((client, format, Some(event)));
        }
    }
    Err(audio_error(
        "無法初始化 WASAPI 音訊擷取，請檢查裝置和麥克風權限",
        last_error.expect("the timed attempt only ends by returning or failing Initialize"),
    ))
}

fn open_process_client(pid: u32) -> Result<(IAudioClient, Format, OwnedHandle), String> {
    let mut last_error = None;
    // Prefer float so 32-bit capture stays lossless. Retry with Microsoft's documented
    // PCM format on systems whose virtual audio client rejects float conversion.
    for (tag, rate, bits) in [
        (WAVE_FORMAT_IEEE_FLOAT, 48_000, 32),
        (WAVE_FORMAT_PCM, 44_100, 16),
    ] {
        let client = activate_process(pid)?;
        let bytes = bits / 8 * 2;
        let mut native = WAVEFORMATEX {
            wFormatTag: tag,
            nChannels: 2,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * u32::from(bytes),
            nBlockAlign: bytes,
            wBitsPerSample: bits,
            cbSize: 0,
        };
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        match unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, &native, None) } {
            Ok(()) => {
                let event = OwnedHandle(
                    unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
                        .map_err(|error| audio_error("無法建立 WASAPI 通知事件", error))?,
                );
                unsafe { client.SetEventHandle(event.0) }
                    .map_err(|error| audio_error("無法設定 WASAPI 通知事件", error))?;
                let format = unsafe { Format::parse(NonNull::from(&mut native)) }?;
                return Ok((client, format, event));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(audio_error(
        "無法初始化應用程式音訊擷取",
        last_error.expect("format attempt"),
    ))
}

pub(crate) fn now_ns() -> u64 {
    static FREQUENCY: OnceLock<u64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut value = 0;
        let _ = unsafe { QueryPerformanceFrequency(&mut value) };
        u64::try_from(value).unwrap_or(0).max(1)
    });
    let mut ticks = 0;
    let _ = unsafe { QueryPerformanceCounter(&mut ticks) };
    (u128::from(ticks.max(0) as u64) * 1_000_000_000 / u128::from(frequency)) as u64
}

pub(crate) struct Format {
    pub(crate) sample_rate: u32,
    pub(crate) channels: usize,
    frame_bytes: usize,
    sample_bytes: usize,
    encoding: Encoding,
    stereo_mix: Vec<[f32; 2]>,
    left: Option<usize>,
    right: Option<usize>,
    center: Option<usize>,
}
#[derive(Clone, Copy)]
enum Encoding {
    Float32,
    Float64,
    Unsigned8,
    Signed16,
    Signed24,
    Signed32,
}

impl Encoding {
    fn read(self, sample: &[u8]) -> f32 {
        let value = match self {
            Self::Float32 => f32::from_le_bytes(sample.try_into().unwrap()),
            Self::Float64 => f64::from_le_bytes(sample.try_into().unwrap()) as f32,
            Self::Unsigned8 => (f32::from(sample[0]) - 128.0) / 128.0,
            Self::Signed16 => f32::from(i16::from_le_bytes(sample.try_into().unwrap())) / 32_768.0,
            Self::Signed24 => {
                let value = i32::from_le_bytes([0, sample[0], sample[1], sample[2]]);
                value as f32 / 2_147_483_648.0
            }
            Self::Signed32 => {
                i32::from_le_bytes(sample.try_into().unwrap()) as f32 / 2_147_483_648.0
            }
        };
        if value.is_finite() { value } else { 0.0 }
    }
}

impl Format {
    pub(crate) fn sender(sample_rate: u32, channels: usize) -> Self {
        Self {
            sample_rate,
            channels,
            frame_bytes: channels * size_of::<f32>(),
            sample_bytes: size_of::<f32>(),
            encoding: Encoding::Float32,
            stereo_mix: Vec::new(),
            left: Some(0),
            right: (channels == 2).then_some(1),
            center: None,
        }
    }

    // GetMixFormat provides a WAVEFORMATEX followed by its stated extension bytes.
    unsafe fn parse(native: NonNull<WAVEFORMATEX>) -> Result<Self, String> {
        let base = unsafe { native.as_ptr().read_unaligned() };
        let bits = base.wBitsPerSample;
        let sample_rate = base.nSamplesPerSec;
        let channels = usize::from(base.nChannels);
        let frame_bytes = usize::from(base.nBlockAlign);
        let (tag, mask) = if base.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
            if usize::from(base.cbSize)
                < size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()
            {
                return Err("WASAPI 輸出格式的延伸欄位不完整".into());
            }
            let extended = unsafe {
                native
                    .cast::<WAVEFORMATEXTENSIBLE>()
                    .as_ptr()
                    .read_unaligned()
            };
            let subtype = extended.SubFormat;
            let tag = match subtype {
                SUBTYPE_PCM => WAVE_FORMAT_PCM,
                SUBTYPE_IEEE_FLOAT => WAVE_FORMAT_IEEE_FLOAT,
                _ => return Err("WASAPI 輸出裝置使用不支援的音訊編碼".into()),
            };
            (tag, extended.dwChannelMask)
        } else {
            (base.wFormatTag, 0)
        };

        let encoding = match (tag, bits) {
            (WAVE_FORMAT_IEEE_FLOAT, 32) => Encoding::Float32,
            (WAVE_FORMAT_IEEE_FLOAT, 64) => Encoding::Float64,
            (WAVE_FORMAT_PCM, 8) => Encoding::Unsigned8,
            (WAVE_FORMAT_PCM, 16) => Encoding::Signed16,
            (WAVE_FORMAT_PCM, 24) => Encoding::Signed24,
            (WAVE_FORMAT_PCM, 32) => Encoding::Signed32,
            _ => {
                return Err(format!(
                    "WASAPI 輸出格式不支援（格式 {tag:#x}，{bits} bit）"
                ));
            }
        };
        let sample_bytes = usize::from(bits / 8);
        if !(1..=32).contains(&channels)
            || !(8_000..=384_000).contains(&sample_rate)
            || frame_bytes != channels * sample_bytes
        {
            return Err("WASAPI 輸出格式的聲道數、取樣率或區塊大小不支援".into());
        }
        let stereo_mix = if channels > 2 {
            stereo_weights(channels, mask)
        } else {
            Vec::new()
        };
        let speaker_index = |speaker: u32| {
            (mask.count_ones() as usize == channels && mask & speaker != 0)
                .then_some((mask & (speaker - 1)).count_ones() as usize)
        };
        let unlabelled_stereo = channels >= 2 && mask.count_ones() as usize != channels;

        Ok(Self {
            sample_rate,
            channels,
            frame_bytes,
            sample_bytes,
            encoding,
            stereo_mix,
            left: speaker_index(1).or(unlabelled_stereo.then_some(0)),
            right: speaker_index(2).or(unlabelled_stereo.then_some(1)),
            center: speaker_index(4),
        })
    }

    pub(crate) fn route_channels(&self, route: ChannelRoute) -> Option<usize> {
        match route {
            ChannelRoute::Left => self.left.map(|_| 1),
            ChannelRoute::Right => self.right.map(|_| 1),
            ChannelRoute::Center => self.center.map(|_| 1),
            ChannelRoute::Mid | ChannelRoute::Side => self.left.zip(self.right).map(|_| 1),
            ChannelRoute::Merged => Some(self.channels.min(2)),
        }
    }

    fn decode(&self, input: &[u8], output: &mut [f32]) {
        for (sample, output) in input.chunks_exact(self.sample_bytes).zip(output) {
            *output = self.encoding.read(sample);
        }
    }

    pub(crate) fn route(&self, route: ChannelRoute, input: &[f32], output: &mut Vec<f32>) {
        if self.route_channels(route).is_none() {
            output.clear();
            return;
        }
        self.route_tracks(&[route], input, output);
    }

    pub(crate) fn route_tracks(
        &self,
        routes: &[ChannelRoute],
        input: &[f32],
        output: &mut Vec<f32>,
    ) {
        output.clear();
        if routes == [ChannelRoute::Merged] && self.stereo_mix.is_empty() {
            output.extend_from_slice(input);
            return;
        }
        let channels: usize = routes
            .iter()
            .map(|&route| self.route_channels(route).unwrap_or(1))
            .sum();
        output.reserve(input.len() / self.channels * channels);
        for frame in input.chunks_exact(self.channels) {
            let sample = |index: Option<usize>| index.map_or(0.0, |index| frame[index]);
            for &route in routes {
                match route {
                    ChannelRoute::Left => output.push(sample(self.left)),
                    ChannelRoute::Right => output.push(sample(self.right)),
                    ChannelRoute::Center => output.push(sample(self.center)),
                    ChannelRoute::Mid | ChannelRoute::Side => {
                        let value = self.left.zip(self.right).map_or(0.0, |(left, right)| {
                            let left = frame[left] * 0.5;
                            let right = frame[right] * 0.5;
                            match route {
                                ChannelRoute::Mid => left + right,
                                _ => left - right,
                            }
                        });
                        output.push(value);
                    }
                    ChannelRoute::Merged if self.stereo_mix.is_empty() => {
                        output.extend_from_slice(frame)
                    }
                    ChannelRoute::Merged => {
                        let mut mixed = [0.0; 2];
                        for (&sample, weight) in frame.iter().zip(&self.stereo_mix) {
                            mixed[0] += sample * weight[0];
                            mixed[1] += sample * weight[1];
                        }
                        output.extend(mixed);
                    }
                }
            }
        }
    }
}

fn stereo_weights(channels: usize, mask: u32) -> Vec<[f32; 2]> {
    let mut weights = Vec::with_capacity(channels);
    let center = std::f32::consts::FRAC_1_SQRT_2;
    for speaker in (0..32)
        .filter(|bit| mask & (1u32 << bit) != 0)
        .take(channels)
    {
        weights.push(match speaker {
            0 => [1.0, 0.0], // Front left
            1 => [0.0, 1.0], // Front right
            4 | 6 | 9 | 12 | 15 => [center, 0.0],
            5 | 7 | 10 | 14 | 17 => [0.0, center],
            3 => [0.5, 0.5], // Low-frequency effects
            _ => [center, center],
        });
    }
    if weights.len() != channels {
        weights.clear();
        weights.push([1.0, 0.0]);
        weights.push([0.0, 1.0]);
        weights.resize(channels, [center, center]);
    }
    let peak_left = weights.iter().map(|weight| weight[0]).sum::<f32>();
    let peak_right = weights.iter().map(|weight| weight[1]).sum::<f32>();
    let scale = peak_left.max(peak_right).max(1.0);
    for weight in &mut weights {
        weight[0] /= scale;
        weight[1] /= scale;
    }
    weights
}

fn audio_error(operation: &str, error: Error) -> String {
    format!("{operation}（{error}）")
}

fn endpoint_id(device: &IMMDevice) -> Result<String, String> {
    let id =
        unsafe { device.GetId() }.map_err(|error| audio_error("無法讀取音訊裝置識別碼", error))?;
    if id.is_null() {
        return Err("WASAPI 未提供音訊裝置識別碼".into());
    }
    let text = unsafe { id.to_string() };
    unsafe { CoTaskMemFree(Some(id.as_ptr().cast())) };
    text.map_err(|error| format!("無效的音訊裝置識別碼（{error}）"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_blob_borrows_boxed_params_until_the_last_completion_reference_drops() {
        for pid in 1..65 {
            let arguments = ActivationArguments::new(pid);
            let property = arguments.property();
            let property_ref = unsafe { &*property };
            let contents = unsafe { &property_ref.Anonymous.Anonymous };
            assert_eq!(contents.vt, VT_BLOB);
            let blob = unsafe { &contents.Anonymous.blob };
            assert_eq!(blob.cbSize as usize, size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>());
            assert_eq!(blob.pBlobData.cast_const(), ptr::from_ref(&arguments.params).cast());
            assert_eq!(unsafe { arguments.params.Anonymous.ProcessLoopbackParams.TargetProcessId }, pid);

            let (sender, _result) = mpsc::sync_channel(1);
            let handler: IActivateAudioInterfaceCompletionHandler = ActivationComplete {
                sender, _arguments: arguments,
            }.into();
            let callback_reference = handler.clone();
            drop(handler);
            let retained_blob = unsafe { &contents.Anonymous.blob };
            let params = unsafe { &*retained_blob.pBlobData.cast::<AUDIOCLIENT_ACTIVATION_PARAMS>() };
            assert_eq!(unsafe { params.Anonymous.ProcessLoopbackParams.TargetProcessId }, pid);
            drop(callback_reference);
        }
    }

    fn prepared_mode(state: &mut QuietModeState, enabled: bool) -> QuietModeBarrier {
        let (reply, ready) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        state.prepare(enabled, reply, released);
        assert!(matches!(ready.try_recv(), Ok(Ok(()))));
        QuietModeBarrier { release: Some(release), wake: Arc::new(Signal::new().unwrap()) }
    }

    fn waiting(state: impl FnOnce(&mut WaitState)) -> Option<Duration> {
        let mut inputs = WaitState {
            running: true,
            backlog: false,
            quiet_pending: false,
            retain_silence: false,
            idle_reported: true,
            idle_for: Duration::from_secs(60),
            since_padding: Duration::from_secs(60),
            since_source_check: Duration::ZERO,
            wait_cap: None,
        };
        state(&mut inputs);
        wait_timeout(&inputs)
    }

    #[test]
    fn an_idle_event_driven_stream_sleeps_until_the_next_endpoint_check() {
        assert_eq!(waiting(|_| {}), Some(ENDPOINT_CHECK_INTERVAL));
        assert_eq!(
            waiting(|state| state.since_source_check = Duration::from_millis(1_500)),
            Some(Duration::from_millis(500)),
        );
        assert_eq!(
            waiting(|state| state.since_source_check = Duration::from_secs(9)),
            Some(Duration::ZERO),
        );
    }

    #[test]
    fn a_paused_stream_waits_only_for_commands() {
        assert_eq!(waiting(|state| state.running = false), None);
        assert_eq!(
            waiting(|state| {
                state.running = false;
                state.backlog = true;
                state.quiet_pending = true;
                state.wait_cap = Some(Duration::from_millis(10));
            }),
            None,
        );
    }

    #[test]
    fn the_wait_ends_when_the_silence_tail_or_retained_padding_is_due() {
        let tail = |idle_ms| {
            waiting(|state| {
                state.idle_reported = false;
                state.idle_for = Duration::from_millis(idle_ms);
            })
        };
        assert_eq!(tail(0), Some(IDLE_PACKET_GAP));
        assert_eq!(tail(150), Some(Duration::from_millis(50)));
        assert_eq!(tail(900), Some(Duration::ZERO));

        let retained = |idle_ms, padding_ms| {
            waiting(|state| {
                state.retain_silence = true;
                state.idle_for = Duration::from_millis(idle_ms);
                state.since_padding = Duration::from_millis(padding_ms);
            })
        };
        // Padding starts after the gap and then repeats at its own interval.
        assert_eq!(retained(50, 50), Some(Duration::from_millis(150)));
        assert_eq!(retained(5_000, 0), Some(RETAINED_IDLE_INTERVAL));
        assert_eq!(retained(5_000, 15), Some(Duration::from_millis(5)));
        assert_eq!(retained(5_000, 500), Some(Duration::ZERO));
        // Each due state is one that the capture loop services and re-arms.
        let state = QuietModeState { retain_silence: true, ..Default::default() };
        assert!(state.should_emit_retained(Duration::from_secs(5), Duration::from_millis(500)));
        assert!(QuietModeState::default().should_emit_tail(Duration::from_millis(900)));
    }

    #[test]
    fn a_pending_quiet_change_and_a_reported_idle_tail_schedule_no_padding() {
        let frozen = waiting(|state| {
            state.quiet_pending = true;
            state.idle_reported = false;
            state.retain_silence = true;
        });
        assert_eq!(frozen, Some(ENDPOINT_CHECK_INTERVAL));
        assert_eq!(waiting(|state| state.idle_reported = true), Some(ENDPOINT_CHECK_INTERVAL));
    }

    #[test]
    fn polling_caps_and_backlogs_shorten_the_wait() {
        let capped = waiting(|state| state.wait_cap = Some(Duration::from_millis(10)));
        assert_eq!(capped, Some(Duration::from_millis(10)));
        assert_eq!(
            waiting(|state| {
                state.wait_cap = Some(Duration::from_millis(10));
                state.idle_reported = false;
                state.idle_for = Duration::from_millis(197);
            }),
            Some(Duration::from_millis(3)),
        );
        assert_eq!(waiting(|state| state.backlog = true), Some(Duration::ZERO));
    }

    #[test]
    fn the_polling_interval_follows_the_device_period_within_bounds() {
        assert_eq!(poll_interval(Some(100_000)), Duration::from_millis(10));
        assert_eq!(poll_interval(Some(26_667)), MIN_POLL_INTERVAL);
        assert_eq!(poll_interval(Some(53_333)), Duration::from_nanos(5_333_300));
        assert_eq!(poll_interval(Some(1_000_000)), MAX_POLL_INTERVAL);
        assert_eq!(poll_interval(Some(0)), DEFAULT_POLL_INTERVAL);
        assert_eq!(poll_interval(Some(-1)), DEFAULT_POLL_INTERVAL);
        assert_eq!(poll_interval(None), DEFAULT_POLL_INTERVAL);
    }

    #[test]
    fn an_unproven_event_never_waits_longer_than_half_the_endpoint_buffer() {
        let poll = Duration::from_millis(10);
        assert_eq!(unproven_wait(4_800, 48_000, poll), Duration::from_millis(50));
        assert_eq!(unproven_wait(48_000, 48_000, poll), MAX_UNPROVEN_WAIT);
        assert_eq!(unproven_wait(1_056, 48_000, poll), Duration::from_millis(11));
        // A tiny buffer cannot be read more often than the polling fallback would.
        assert_eq!(unproven_wait(480, 48_000, poll), poll);
    }

    #[test]
    fn the_event_watch_trusts_events_only_after_they_announce_a_packet() {
        let poll = Duration::from_millis(10);
        let unproven = Duration::from_millis(50);
        let mut watch = EventWatch::new(true);
        assert_eq!(watch.wait_cap(poll, unproven), Some(unproven));
        assert!(watch.uses_event());
        // Timeouts and empty event wakes prove nothing either way.
        watch.observe(false, false);
        watch.observe(true, false);
        assert_eq!(watch, EventWatch::Unproven);
        watch.observe(true, true);
        assert_eq!(watch, EventWatch::Trusted);
        assert_eq!(watch.wait_cap(poll, unproven), None);
        assert_eq!(watch.read_latency(poll), Duration::ZERO);
    }

    #[test]
    fn packets_that_arrive_without_their_event_switch_the_stream_to_polling() {
        let poll = Duration::from_millis(10);
        let unproven = Duration::from_millis(50);
        let mut never_signalled = EventWatch::new(true);
        never_signalled.observe(false, true);
        assert_eq!(never_signalled, EventWatch::Polling);

        let mut watch = EventWatch::Trusted;
        watch.observe(false, true);
        assert_eq!(watch, EventWatch::Suspect);
        assert_eq!(watch.wait_cap(poll, unproven), Some(poll));
        // One isolated miss followed by a quiet poll is forgiven.
        watch.observe(false, false);
        assert_eq!(watch, EventWatch::Trusted);
        watch.observe(false, true);
        watch.observe(true, true);
        assert_eq!(watch, EventWatch::Trusted);
        // Two consecutive misses are not.
        watch.observe(false, true);
        watch.observe(false, true);
        assert_eq!(watch, EventWatch::Polling);
        assert!(!watch.uses_event());
        assert_eq!(watch.wait_cap(poll, unproven), Some(poll));
        assert_eq!(watch.read_latency(poll), poll);
        for (by_event, had_packets) in [(true, true), (true, false), (false, false)] {
            watch.observe(by_event, had_packets);
            assert_eq!(watch, EventWatch::Polling);
        }
        assert_eq!(EventWatch::new(false), EventWatch::Polling);
    }

    #[test]
    fn the_capture_wait_reports_which_object_ended_it() {
        let signal = Signal::new().unwrap();
        let audio = Signal::new().unwrap();
        let audio_handle = Some(audio.0.0);
        let brief = Some(Duration::from_millis(1));
        assert_eq!(wait_for(&signal, audio_handle, brief), Woke::Timeout);
        audio.set();
        assert_eq!(wait_for(&signal, audio_handle, None), Woke::Audio);
        // Auto-reset: the audio event was consumed by the wait it ended.
        assert!(!event_is_set(audio.0.0));
        audio.set();
        signal.set();
        // A command wake wins and leaves the audio event signalled for the next wait.
        assert_eq!(wait_for(&signal, audio_handle, None), Woke::Signal);
        assert_eq!(wait_for(&signal, audio_handle, brief), Woke::Audio);
        signal.set();
        assert_eq!(wait_for(&signal, None, None), Woke::Signal);
        assert_eq!(wait_for(&signal, None, Some(Duration::ZERO)), Woke::Timeout);
    }

    fn commit_mode_at(mut barrier: QuietModeBarrier, since_ns: u64) {
        assert!(barrier.release.take().unwrap().send(since_ns).is_ok());
    }

    #[test]
    fn default_idle_policy_emits_one_tail_per_real_packet_episode() {
        let mut state = QuietModeState::default();
        assert!(!state.should_emit_tail(Duration::from_millis(199)));
        assert!(state.should_emit_tail(IDLE_PACKET_GAP));
        assert!(!state.should_emit_retained(Duration::from_secs(60), Duration::from_secs(60)));
        state.note_synthetic_packet();
        assert!(!state.should_emit_tail(Duration::from_secs(60)));
        state.note_real_packet();
        assert!(state.should_emit_tail(IDLE_PACKET_GAP));
    }

    #[test]
    fn canceling_a_prepared_quiet_change_restores_each_previous_policy() {
        for previous in [false, true] {
            let mut state = QuietModeState { retain_silence: previous, ..Default::default() };
            let barrier = prepared_mode(&mut state, !previous);
            assert!(!state.should_emit_tail(Duration::from_secs(60)));
            assert!(!state.should_emit_retained(Duration::from_secs(60), Duration::from_secs(60)));
            barrier.cancel();
            assert!(state.poll_release().is_none());
            assert!(state.pending.is_none());
            assert_eq!(state.retain_silence, previous);
        }
    }

    #[test]
    fn an_abandoned_prepare_ack_does_not_leave_synthetic_padding_frozen() {
        let mut state = QuietModeState::default();
        let (reply, ready) = mpsc::sync_channel(1);
        let (_release, released) = mpsc::sync_channel(1);
        drop(ready);
        state.prepare(true, reply, released);
        assert!(state.pending.is_none());
        assert!(!state.retain_silence);
        assert!(state.should_emit_tail(IDLE_PACKET_GAP));
    }

    #[test]
    fn a_paused_worker_observes_guard_release_without_another_control_command() {
        let (commands, receiver) = mpsc::channel();
        let (reply, ready) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let (done, finished) = mpsc::channel();
        let wake = Arc::new(Signal::new().unwrap());
        let thread_wake = Arc::clone(&wake);
        let worker = thread::spawn(move || {
            let mut state = QuietModeState::default();
            state.prepare(true, reply, released);
            loop {
                if let Some((enabled, _)) = state.poll_release() {
                    let _ = done.send(enabled && state.pending.is_none());
                    break;
                }
                if matches!(receiver.try_recv(),
                    Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected)) {
                    break;
                }
                // A paused stream has no timeout: only the barrier can end this wait.
                let paused = wait_timeout(&WaitState {
                    running: false,
                    backlog: false,
                    quiet_pending: state.pending.is_some(),
                    retain_silence: state.retain_silence,
                    idle_reported: state.idle_reported,
                    idle_for: Duration::ZERO,
                    since_padding: Duration::ZERO,
                    since_source_check: Duration::ZERO,
                    wait_cap: None,
                });
                assert_eq!(paused, None);
                wait_for(&thread_wake, None, paused);
            }
        });
        assert!(matches!(ready.recv(), Ok(Ok(()))));
        drop(QuietModeBarrier { release: Some(release), wake: Arc::clone(&wake) });
        let completed_without_wake = finished.recv_timeout(Duration::from_secs(2));
        let _ = commands.send(Command::Shutdown);
        wake.set();
        worker.join().unwrap();
        assert!(matches!(completed_without_wake, Ok(true)));
    }

    #[test]
    fn a_quiet_prepare_timeout_can_be_consumed_later_without_stranding_the_old_mode() {
        let (commands, receiver) = mpsc::channel();
        let (_errors, errors) = mpsc::channel();
        let mut stream = Loopback {
            format: Arc::new(native_format(2, 1 | 2)),
            packets: TimedAudioQueue::new(48_000, PACKET_QUEUE_CAPACITY, 2),
            commands,
            wake: Arc::new(Signal::new().unwrap()),
            errors,
            worker: None,
        };
        let result = stream.prepare_retain_silence(true);
        assert!(matches!(result, Err(error) if error.contains("逾時")));
        let Ok(Command::PrepareRetainSilence(enabled, reply, release)) = receiver.try_recv() else {
            panic!("expected the timed-out request to remain queued");
        };
        let mut state = QuietModeState::default();
        state.prepare(enabled, reply, release);
        assert!(state.pending.is_none());
        assert!(!state.retain_silence);
        assert!(state.should_emit_tail(IDLE_PACKET_GAP));
    }

    #[test]
    fn turning_off_after_retained_padding_does_not_append_another_legacy_tail() {
        let mut state = QuietModeState::default();
        let mut clock = PacketClock::new(48_000);
        let start = 1_000_000_000;
        let barrier = prepared_mode(&mut state, true);
        commit_mode_at(barrier, start);
        let (enabled, since_ns) = state.poll_release().unwrap();
        clock.set_retain_silence(enabled, since_ns);
        clock.timestamp(0, 480, Some(start), false, || unreachable!());
        let now = start + 1_000_000_000;
        let cutoff = clock.idle_cutoff_ns(now, 4_800, IDLE_CUTOFF_GUARD.as_nanos() as u64);
        while clock.retained_idle_stamp(cutoff, 9_600).is_some() {
            state.note_synthetic_packet();
        }
        let barrier = prepared_mode(&mut state, false);
        commit_mode_at(barrier, now);
        let (enabled, since_ns) = state.poll_release().unwrap();
        clock.set_retain_silence(enabled, since_ns);
        assert!(!state.should_emit_tail(Duration::from_secs(2)));
        let measured = now + 50_000_000;
        let packet = clock.timestamp(480, 480, Some(measured), false, || unreachable!());
        assert_eq!(packet.timestamp_ns, measured);
    }

    #[test]
    fn a_queued_quiet_commit_followed_by_resume_uses_resume_as_the_new_floor() {
        let mut state = QuietModeState::default();
        let barrier = prepared_mode(&mut state, true);
        let switched_while_paused = 5_000_000_000;
        commit_mode_at(barrier, switched_while_paused);
        let resumed = 30_000_000_000;
        let mut recreated_clock = PacketClock::new(48_000);
        state.restore_after_resume(&mut recreated_clock, resumed);
        assert!(state.pending.is_none());
        assert!(state.retain_silence);
        let (stamp, _) = recreated_clock.retained_idle_stamp(resumed + 1_000_000_000, 9_600).unwrap();
        assert_eq!(stamp.timestamp_ns, resumed);
    }

    #[test]
    fn retained_padding_and_real_return_keep_the_exact_gap_and_original_samples() {
        let rate = 48_000;
        let start = 1_000_000_000;
        let mut clock = PacketClock::new(rate);
        clock.set_retain_silence(true, start);
        let mut capture = backtracking_core::SynchronizedCapture::new(
            rate, 1, None, backtracking_core::DurationChoice::Seconds10,
        ).unwrap();
        capture.set_retain_silence(true);
        let first = clock.timestamp(0, 480, Some(start), false, || unreachable!());
        capture.push_native_interleaved(first.timestamp_ns, &[0.5; 480], first.discontinuity).unwrap();
        let now = start + 1_000_000_000;
        let cutoff = clock.idle_cutoff_ns(now, 4_800, IDLE_CUTOFF_GUARD.as_nanos() as u64);
        while let Some((stamp, frames)) = clock.retained_idle_stamp(cutoff, 9_600) {
            assert!(!stamp.discontinuity);
            capture.push_native_interleaved(stamp.timestamp_ns, &vec![0.; frames], stamp.discontinuity).unwrap();
        }
        let real = clock.timestamp(480, 480, Some(now - 10_000_000), false, || unreachable!());
        capture.push_native_interleaved(real.timestamp_ns, &[0.25; 480], real.discontinuity).unwrap();
        assert_eq!(capture.capture().frame_range(), 0..48_000);
        let snapshot = capture.capture().track_snapshot_range(0, 0..48_000).unwrap();
        let samples = snapshot.frames(0..48_000).unwrap();
        assert_eq!(&samples[..480], &[0.5; 480]);
        assert!(samples[480..47_520].iter().all(|&sample| sample == 0.));
        assert_eq!(&samples[47_520..], &[0.25; 480]);
    }

    #[test]
    fn a_long_known_idle_jump_keeps_the_latest_retention_with_two_bounded_packets() {
        let rate = 1_000;
        let start = 1_000_000_000;
        let until = start + 3_600_000_000_000;
        let mut clock = PacketClock::new(rate);
        clock.set_retain_silence(true, start);
        let mut capture = backtracking_core::SynchronizedCapture::new(
            rate, 1, None, backtracking_core::DurationChoice::Seconds10,
        ).unwrap();
        capture.set_retain_silence(true);
        let first = clock.timestamp(0, 10, Some(start), false, || unreachable!());
        capture.push_native_interleaved(first.timestamp_ns, &[0.5; 10], first.discontinuity).unwrap();
        for _ in 0..2 {
            let (stamp, frames) = clock.retained_idle_stamp(until, 200).unwrap();
            assert!(frames <= 200);
            assert!(!stamp.discontinuity);
            capture.push_native_interleaved(stamp.timestamp_ns, &vec![0.; frames], stamp.discontinuity).unwrap();
        }
        assert!(clock.retained_idle_stamp(until, 200).is_none());
        let last = clock.timestamp(10, 10, Some(until), false, || unreachable!());
        capture.push_native_interleaved(last.timestamp_ns, &[0.25; 10], last.discontinuity).unwrap();
        let snapshot = capture.capture().snapshot();
        assert_eq!(snapshot.frame_range(), 3_590_010..3_600_010);
        assert!(snapshot.samples[..9_990].iter().all(|&sample| sample == 0.));
        assert_eq!(&snapshot.samples[9_990..], &[0.25; 10]);
    }

    fn native_format(channels: u16, mask: u32) -> Format {
        let mut native = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE,
                nChannels: channels,
                nSamplesPerSec: 48_000,
                nAvgBytesPerSec: 48_000 * u32::from(channels) * 4,
                nBlockAlign: channels * 4,
                wBitsPerSample: 32,
                cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
            },
            dwChannelMask: mask,
            SubFormat: SUBTYPE_IEEE_FLOAT,
            ..Default::default()
        };
        unsafe { Format::parse(NonNull::from(&mut native).cast()).unwrap() }
    }

    #[test]
    fn native_front_channels_are_routed_before_downmix() {
        let format = native_format(3, 1 | 2 | 4);
        let samples = [0.5, -0.25, 0.75, -0.5, 0.25, -0.75];
        let mut output = Vec::new();
        for (route, expected) in [
            (ChannelRoute::Left, [0.5, -0.5]),
            (ChannelRoute::Right, [-0.25, 0.25]),
            (ChannelRoute::Center, [0.75, -0.75]),
            (ChannelRoute::Mid, [0.125, -0.125]),
            (ChannelRoute::Side, [0.375, -0.375]),
        ] {
            format.route(route, &samples, &mut output);
            assert_eq!(output, expected);
        }
        format.route(ChannelRoute::Merged, &samples, &mut output);
        assert_eq!(output.len(), 4);
        assert!(output[0] > output[1]);
        assert_eq!(output[0], -output[2]);
        assert_eq!(output[1], -output[3]);
    }

    #[test]
    fn unavailable_routes_do_not_read_another_physical_channel() {
        let format = native_format(2, 1 | 4);
        let mut output = vec![42.0];
        assert_eq!(format.route_channels(ChannelRoute::Right), None);
        assert_eq!(format.route_channels(ChannelRoute::Side), None);
        format.route(ChannelRoute::Right, &[0.5, 0.75], &mut output);
        assert!(output.is_empty());
        format.route(ChannelRoute::Center, &[0.5, 0.75], &mut output);
        assert_eq!(output, [0.75]);
        assert_eq!(
            native_format(2, 1 | 2).route_channels(ChannelRoute::Center),
            None
        );
    }

    #[test]
    fn grouped_routes_use_the_same_native_frames_and_keep_missing_lanes_silent() {
        let format = native_format(2, 1 | 2);
        let samples = [0.5, -0.25, -0.5, 0.75];
        let mut output = Vec::new();
        format.route_tracks(
            &[ChannelRoute::Side, ChannelRoute::Mid],
            &samples,
            &mut output,
        );
        assert_eq!(output, [0.375, 0.125, -0.625, 0.125]);
        format.route_tracks(
            &[ChannelRoute::Merged, ChannelRoute::Center],
            &samples,
            &mut output,
        );
        assert_eq!(output, [0.5, -0.25, 0.0, -0.5, 0.75, 0.0]);
    }

    #[test]
    fn short_period_packets_do_not_exhaust_metadata_before_a_ui_poll() {
        let packets = TimedAudioQueue::new(96_000, PACKET_QUEUE_CAPACITY, 2);
        let period = [0.25; 64 * 2];
        for index in 0..256 {
            let timestamp_ns = index * 64 * 1_000_000_000 / 96_000;
            assert_eq!(packets.push_interleaved(timestamp_ns, 96_000, &period), 64);
        }
        assert_eq!(packets.pending_packets(), 256);
    }
}

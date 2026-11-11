use std::mem::{ManuallyDrop, size_of};
use std::ptr::{self, NonNull};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{ChannelRoute, TimedAudioQueue, TimedPacket};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
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
use windows::Win32::System::Threading::CreateEventW;
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Error, GUID, HRESULT, HSTRING, Interface, PCWSTR, Ref, implement};

mod sources;
mod timing;
pub(crate) use sources::CaptureSource;
use sources::{OwnedHandle, RunningProcess, target_process};
use timing::PacketClock;

const POLL_INTERVAL: Duration = Duration::from_millis(4);
const IDLE_PACKET_GAP: Duration = Duration::from_millis(200);
const ENDPOINT_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(3);
pub(crate) const PACKET_QUEUE_CAPACITY: usize = 1024;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

type Reply = mpsc::SyncSender<Result<(), String>>;

enum Command {
    Pause(Reply),
    Resume(Reply),
    Shutdown,
}

struct Started {
    format: Arc<Format>,
    packets: Arc<TimedAudioQueue>,
}

pub(crate) struct Loopback {
    pub(crate) format: Arc<Format>,
    packets: Arc<TimedAudioQueue>,
    commands: mpsc::Sender<Command>,
    errors: mpsc::Receiver<String>,
    worker: Option<JoinHandle<()>>,
}

impl Loopback {
    pub(crate) fn start(source: CaptureSource) -> Result<Self, String> {
        let (commands, command_reader) = mpsc::channel();
        let (errors_writer, errors) = mpsc::channel();
        let (startup_writer, startup) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("backtracking-wasapi".into())
            .spawn(move || capture_thread(source, command_reader, startup_writer, errors_writer))
            .map_err(|error| format!("無法建立 WASAPI 擷取執行緒（{error}）"))?;

        match startup.recv() {
            Ok(Ok(Started { format, packets })) => Ok(Self {
                format,
                packets,
                commands,
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
        if self.commands.send(command(reply)).is_ok()
            && let Ok(result) = result.recv()
        {
            return result;
        }
        Err(self
            .take_error()
            .unwrap_or_else(|| "WASAPI 連線已中斷，請重新連線".into()))
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn capture_thread(
    source: CaptureSource,
    commands: mpsc::Receiver<Command>,
    startup: mpsc::SyncSender<Result<Started, String>>,
    errors: mpsc::Sender<String>,
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
    let mut idle_reported = false;
    loop {
        let command = if running {
            commands.recv_timeout(POLL_INTERVAL)
        } else {
            commands
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        match command {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Command::Pause(reply)) => {
                let result = if running { stream.pause() } else { Ok(()) };
                let failed = result.is_err();
                let _ = reply.send(result);
                if failed {
                    break;
                }
                running = false;
            }
            Ok(Command::Resume(reply)) => {
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
                idle_reported = false;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_source_check.elapsed() >= ENDPOINT_CHECK_INTERVAL {
                    last_source_check = Instant::now();
                    if let Err(error) = stream.verify_source() {
                        let _ = errors.send(error);
                        break;
                    }
                }
                match stream.read_packets() {
                    Ok(true) => {
                        last_packet = Instant::now();
                        idle_reported = false;
                    }
                    Ok(false) if !idle_reported && last_packet.elapsed() >= IDLE_PACKET_GAP => {
                        stream.push_idle_silence();
                        idle_reported = true;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        let _ = errors.send(error);
                        break;
                    }
                }
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
    // COM clients are dropped before the event handle and COM apartment.
    _event: Option<OwnedHandle>,
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
                    CaptureSource::Application { .. } => unreachable!(),
                };
                let binding = match source {
                    CaptureSource::System => SourceBinding::DefaultRender(endpoint_id(&device)?),
                    CaptureSource::DefaultMicrophone => {
                        SourceBinding::DefaultCapture(endpoint_id(&device)?)
                    }
                    CaptureSource::Microphone { id, .. } => SourceBinding::Microphone(id),
                    CaptureSource::Application { .. } => unreachable!(),
                };
                let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
                    .map_err(|error| audio_error("無法開啟已選取的音訊來源", error))?;
                let native = unsafe { client.GetMixFormat() }
                    .map_err(|error| audio_error("無法讀取來源格式", error))?;
                let native = MixFormat(NonNull::new(native).ok_or("WASAPI 未提供有效的來源格式")?);
                let format = unsafe { Format::parse(native.0) }?;
                unsafe {
                    client.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        flags,
                        1_000_000,
                        0,
                        native.0.as_ptr(),
                        None,
                    )
                }
                .map_err(|error| {
                    audio_error("無法初始化 WASAPI 音訊擷取，請檢查裝置和麥克風權限", error)
                })?;
                (client, format, binding, Some(enumerator), None)
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
            _event: event,
            _apartment: apartment,
        };
        stream.start()?;
        Ok(stream)
    }

    fn start(&mut self) -> Result<(), String> {
        self.clock = PacketClock::new(self.format.sample_rate);
        self.queue_loss = false;
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
        for _ in 0..64 {
            let next = unsafe { self.capture.GetNextPacketSize() }
                .map_err(|error| audio_error("無法讀取音訊封包", error))?;
            if next == 0 {
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
    variant: PROPVARIANT,
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
    let mut arguments = Box::new(ActivationArguments {
        params: AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        },
        variant: PROPVARIANT::default(),
    });
    // The BLOB points at activation parameters, rather than owning a COM allocation.
    arguments.variant = PROPVARIANT {
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
    };
    let property = ptr::from_ref(&arguments.variant);
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

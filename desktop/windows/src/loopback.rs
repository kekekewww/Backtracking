use std::mem::size_of;
use std::ptr::{self, NonNull};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::AudioQueue;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::core::{Error, GUID};

const POLL_INTERVAL: Duration = Duration::from_millis(4);
const IDLE_PACKET_GAP: Duration = Duration::from_millis(200);
const ENDPOINT_CHECK_INTERVAL: Duration = Duration::from_secs(2);
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
    sample_rate: u32,
    channels: usize,
    queue: Arc<AudioQueue>,
}

pub struct Loopback {
    pub sample_rate: u32,
    pub channels: usize,
    pub queue: Arc<AudioQueue>,
    commands: mpsc::Sender<Command>,
    errors: mpsc::Receiver<String>,
    worker: Option<JoinHandle<()>>,
}

impl Loopback {
    pub fn start() -> Result<Self, String> {
        let (commands, command_reader) = mpsc::channel();
        let (errors_writer, errors) = mpsc::channel();
        let (startup_writer, startup) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("backtracking-wasapi".into())
            .spawn(move || capture_thread(command_reader, startup_writer, errors_writer))
            .map_err(|error| format!("無法建立 WASAPI 擷取執行緒（{error}）"))?;

        match startup.recv() {
            Ok(Ok(started)) => Ok(Self {
                sample_rate: started.sample_rate,
                channels: started.channels,
                queue: started.queue,
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

    pub fn pause(&self) -> Result<(), String> {
        self.request(Command::Pause)
    }

    pub fn resume(&self) -> Result<(), String> {
        self.request(Command::Resume)
    }

    pub fn take_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
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
    commands: mpsc::Receiver<Command>,
    startup: mpsc::SyncSender<Result<Started, String>>,
    errors: mpsc::Sender<String>,
) {
    let mut stream = match WasapiStream::open() {
        Ok(stream) => stream,
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    let started = Started {
        sample_rate: stream.format.sample_rate,
        channels: stream.format.output_channels(),
        queue: stream.queue.clone(),
    };
    if startup.send(Ok(started)).is_err() {
        return;
    }

    let mut running = true;
    let mut last_packet = Instant::now();
    let mut last_endpoint_check = last_packet;
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
                    stream
                        .verify_default_endpoint()
                        .and_then(|()| stream.start())
                };
                let failed = result.is_err();
                let _ = reply.send(result);
                if failed {
                    break;
                }
                running = true;
                last_packet = Instant::now();
                last_endpoint_check = last_packet;
                idle_reported = false;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_endpoint_check.elapsed() >= ENDPOINT_CHECK_INTERVAL {
                    last_endpoint_check = Instant::now();
                    if let Err(error) = stream.verify_default_endpoint() {
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
                        // WASAPI may stop delivering packets entirely when no app plays to the
                        // endpoint. Let the core finish its short tail and return to waiting.
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

struct Apartment;

impl Apartment {
    fn initialize() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| audio_error("無法初始化 COM", error))?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct MixFormat(NonNull<WAVEFORMATEX>);

impl Drop for MixFormat {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.as_ptr().cast())) };
    }
}

struct WasapiStream {
    capture: IAudioCaptureClient,
    client: IAudioClient,
    enumerator: IMMDeviceEnumerator,
    endpoint: String,
    format: Format,
    queue: Arc<AudioQueue>,
    scratch: Vec<f32>,
    // Struct fields are dropped in declaration order, after the COM clients.
    _apartment: Apartment,
}

impl WasapiStream {
    fn open() -> Result<Self, String> {
        let apartment = Apartment::initialize()?;
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|error| audio_error("無法取得音訊裝置清單", error))?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map_err(|error| audio_error("找不到 Windows 預設輸出裝置", error))?;
        let endpoint = endpoint_id(&device)?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| audio_error("無法開啟預設輸出裝置", error))?;
        let native_format = unsafe { client.GetMixFormat() }
            .map_err(|error| audio_error("無法讀取預設輸出格式", error))?;
        let native_format =
            MixFormat(NonNull::new(native_format).ok_or("WASAPI 未提供有效的輸出格式")?);
        let format = unsafe { Format::parse(native_format.0) }?;
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                1_000_000, // 100 ms in REFERENCE_TIME units.
                0,
                native_format.0.as_ptr(),
                None,
            )
        }
        .map_err(|error| audio_error("無法初始化 WASAPI 迴路擷取", error))?;
        let capture = unsafe { client.GetService() }
            .map_err(|error| audio_error("無法建立 WASAPI 迴路擷取", error))?;
        let packet_capacity = unsafe { client.GetBufferSize() }
            .map_err(|error| audio_error("無法讀取 WASAPI 緩衝區大小", error))?
            as usize;
        if packet_capacity == 0 || packet_capacity > format.sample_rate as usize * 4 {
            return Err("WASAPI 回傳不支援的緩衝區大小".into());
        }
        drop(native_format);
        drop(device);
        let channels = format.output_channels();
        let stream = Self {
            capture,
            client,
            enumerator,
            endpoint,
            queue: AudioQueue::new(format.sample_rate as usize, channels),
            scratch: vec![0.0; packet_capacity * channels],
            format,
            _apartment: apartment,
        };
        stream.start()?;
        Ok(stream)
    }

    fn start(&self) -> Result<(), String> {
        unsafe { self.client.Start() }.map_err(|error| audio_error("無法開始擷取系統聲音", error))
    }

    fn verify_default_endpoint(&self) -> Result<(), String> {
        let current = unsafe { self.enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map_err(|error| audio_error("找不到 Windows 預設輸出裝置", error))?;
        if endpoint_id(&current)? != self.endpoint {
            return Err("Windows 預設輸出裝置已切換，請重新連線".into());
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        let read = self.read_packets();
        let stop = unsafe { self.client.Stop() }
            .map_err(|error| audio_error("無法暫停系統音訊擷取", error));
        read.and(stop)?;
        unsafe { self.client.Reset() }
            .map_err(|error| audio_error("無法清空 WASAPI 端點緩衝區", error))
    }

    fn read_packets(&mut self) -> Result<bool, String> {
        let mut had_packet = false;
        for _ in 0..64 {
            let next = unsafe { self.capture.GetNextPacketSize() }
                .map_err(|error| audio_error("無法讀取系統音訊封包", error))?;
            if next == 0 {
                break;
            }

            let mut data = ptr::null_mut();
            let mut frames = 0;
            let mut flags = 0;
            unsafe {
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            }
            .map_err(|error| audio_error("無法取出系統音訊封包", error))?;

            if frames == 0 {
                break;
            }
            let result = self.copy_packet(data, frames as usize, flags);
            let release = unsafe { self.capture.ReleaseBuffer(frames) }
                .map_err(|error| audio_error("無法釋放系統音訊封包", error));
            result.and(release)?;
            had_packet = true;
        }
        Ok(had_packet)
    }

    fn copy_packet(&mut self, data: *mut u8, frames: usize, flags: u32) -> Result<(), String> {
        let sample_count = frames * self.format.output_channels();
        let output = self
            .scratch
            .get_mut(..sample_count)
            .ok_or("WASAPI 封包超出預先配置的緩衝區")?;
        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
            output.fill(0.0);
        } else {
            if data.is_null() {
                return Err("WASAPI 音訊封包沒有資料".into());
            }
            // GetBuffer keeps exactly this packet accessible until ReleaseBuffer.
            let input =
                unsafe { std::slice::from_raw_parts(data, frames * self.format.frame_bytes) };
            self.format.decode(input, output);
        }
        self.queue.push_interleaved(output);
        Ok(())
    }

    fn push_idle_silence(&mut self) {
        let channels = self.format.output_channels();
        let sample_count = self.format.sample_rate.div_ceil(5) as usize * channels;
        self.scratch.fill(0.0);
        let mut written = 0;
        while written < sample_count {
            let count = (sample_count - written).min(self.scratch.len());
            let accepted = self.queue.push_interleaved(&self.scratch[..count]) * channels;
            written += accepted;
            if accepted < count {
                break;
            }
        }
    }
}

impl Drop for WasapiStream {
    fn drop(&mut self) {
        let _ = unsafe { self.client.Stop() };
    }
}

struct Format {
    sample_rate: u32,
    channels: usize,
    frame_bytes: usize,
    sample_bytes: usize,
    encoding: Encoding,
    stereo_mix: Vec<[f32; 2]>,
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

        Ok(Self {
            sample_rate,
            channels,
            frame_bytes,
            sample_bytes,
            encoding,
            stereo_mix,
        })
    }

    fn output_channels(&self) -> usize {
        self.channels.min(2)
    }

    fn decode(&self, input: &[u8], output: &mut [f32]) {
        if self.stereo_mix.is_empty() {
            for (sample, output) in input.chunks_exact(self.sample_bytes).zip(output) {
                *output = self.encoding.read(sample);
            }
            return;
        }

        for (frame, output) in input
            .chunks_exact(self.frame_bytes)
            .zip(output.as_chunks_mut::<2>().0)
        {
            let mut mixed = [0.0; 2];
            for (sample, weight) in frame.chunks_exact(self.sample_bytes).zip(&self.stereo_mix) {
                let value = self.encoding.read(sample);
                mixed[0] += value * weight[0];
                mixed[1] += value * weight[1];
            }
            *output = mixed;
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

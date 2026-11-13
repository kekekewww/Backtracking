use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEARTBEAT: Duration = Duration::from_secs(2);
const EXPIRES: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const MAX_CLIENTS: usize = 16;
const MAX_FRAMES: usize = 8192;
const MAX_MARKERS: usize = MAX_FRAMES;
const HEADER_SIZE: usize = 24;
const MAX_PENDING: usize = 1024 * 1024;
const MAGIC: [u8; 4] = *b"BTA1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderInfo {
    pub id: String,
    pub name: String,
    pub sample_rate: u32,
    pub channels: usize,
}

struct Registration {
    info: SenderInfo,
    port: u16,
    audio_enabled: bool,
}

pub struct SenderEndpoint {
    id: String,
    listener: TcpListener,
    registration: Option<(String, bool, u32, usize)>,
    heartbeat_at: Instant,
    pending: Vec<PendingClient>,
    clients: Vec<Client>,
    snapshot_requested: bool,
    encoded: Vec<u8>,
}

struct PendingClient {
    stream: TcpStream,
    deadline: Instant,
    token: Vec<u8>,
}

struct Client {
    stream: TcpStream,
    pending: Vec<u8>,
    offset: usize,
}

impl Client {
    fn flush(&mut self) -> bool {
        while self.offset < self.pending.len() {
            match self.stream.write(&self.pending[self.offset..]) {
                Ok(0) => return false,
                Ok(written) => self.offset += written,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return true,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
        self.pending.clear();
        self.offset = 0;
        let mut probe = [0];
        self.stream.peek(&mut probe).is_err_and(|error| {
            matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
        })
    }

    fn append(&mut self, packet: &[u8]) -> bool {
        if !self.flush() || self.pending.len() - self.offset + packet.len() > MAX_PENDING {
            return false;
        }
        if self.offset > 0 {
            self.pending.drain(..self.offset);
            self.offset = 0;
        }
        self.pending.extend_from_slice(packet);
        self.flush()
    }
}

pub fn new_sender_id() -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{:x}-{:x}-{:x}", std::process::id(), now.as_nanos(), NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

impl SenderEndpoint {
    pub fn new() -> io::Result<Self> {
        Self::with_id("")
    }

    pub fn with_id(preferred: &str) -> io::Result<Self> {
        let directory = sender_directory()?;
        create_sender_directory(&directory)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let id = if valid_id(preferred) && reserve_id(preferred).is_ok() {
            preferred.to_owned()
        } else {
            let id = new_sender_id();
            reserve_id(&id)?;
            id
        };
        Ok(Self {
            id,
            listener,
            registration: None,
            heartbeat_at: Instant::now(),
            pending: Vec::new(),
            clients: Vec::new(),
            snapshot_requested: false,
            encoded: Vec::new(),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn update(&mut self, name: &str, sender_enabled: bool, sample_rate: u32, channels: usize) {
        let name = name.trim();
        let name = if name.is_empty() { "Backtracking" } else { name };
        let now = Instant::now();
        let unchanged = self.registration.as_ref().is_some_and(|(old, active, rate, width)| {
            old == name && *active == sender_enabled && *rate == sample_rate && *width == channels
        });
        if !unchanged || now >= self.heartbeat_at {
            if self.registration.as_ref().is_some_and(|(_, active, rate, width)| {
                !sender_enabled || !*active || *rate != sample_rate || *width != channels
            }) {
                self.clients.clear();
            }
            let _ = self.register(name, sender_enabled, sample_rate, channels);
            self.registration = Some((name.to_owned(), sender_enabled, sample_rate, channels));
            self.heartbeat_at = now + HEARTBEAT;
        }
        self.service(sender_enabled);
    }

    fn register(&self, name: &str, enabled: bool, rate: u32, channels: usize) -> io::Result<()> {
        let path = registration_path(&self.id)?;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let name: String = name.chars().filter(|c| !c.is_control()).take(120).collect();
        let port = self.listener.local_addr()?.port();
        fs::write(&temporary, format!("{port}\t{}\t{rate}\t{channels}\t{name}", u8::from(enabled)))?;
        fs::rename(temporary, path)
    }

    fn service(&mut self, audio_enabled: bool) {
        for _ in 0..MAX_CLIENTS {
            let Ok((stream, _)) = self.listener.accept() else { break };
            if self.pending.len() == MAX_CLIENTS || stream.set_nonblocking(true).is_err() {
                continue;
            }
            let _ = stream.set_nodelay(true);
            self.pending.push(PendingClient {
                stream,
                deadline: Instant::now() + CONNECT_TIMEOUT,
                token: Vec::new(),
            });
        }
        let mut index = 0;
        while index < self.pending.len() {
            let pending = &mut self.pending[index];
            if pending.deadline <= Instant::now() {
                self.pending.swap_remove(index);
                continue;
            }
            let mut input = [0; 96];
            let remaining = self.id.len() + 1 - pending.token.len();
            match pending.stream.read(&mut input[..remaining]) {
                Ok(0) => { self.pending.swap_remove(index); }
                Ok(read) => {
                    pending.token.extend_from_slice(&input[..read]);
                    if pending.token.len() < self.id.len() + 1 {
                        index += 1;
                        continue;
                    }
                    let pending = self.pending.swap_remove(index);
                    if &pending.token[1..] != self.id.as_bytes() { continue; }
                    match pending.token[0] {
                        b'A' if audio_enabled && self.clients.len() < MAX_CLIENTS => {
                            self.clients.push(Client { stream: pending.stream, pending: Vec::new(), offset: 0 });
                        }
                        b'S' => self.snapshot_requested = true,
                        _ => {}
                    }
                }
                Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {
                    index += 1;
                }
                Err(_) => { self.pending.swap_remove(index); }
            }
        }
        self.clients.retain_mut(Client::flush);
    }

    pub fn take_snapshot_request(&mut self) -> bool {
        std::mem::take(&mut self.snapshot_requested)
    }

    pub fn publish(
        &mut self,
        name: &str,
        sample_rate: u32,
        channels: usize,
        timestamp_ns: u64,
        audio: &[f32],
        bar_offsets: &[u32],
    ) -> io::Result<()> {
        if !valid_format(sample_rate, channels) || audio.len() % channels != 0
            || bar_offsets.windows(2).any(|pair| pair[0] >= pair[1])
            || bar_offsets.last().is_some_and(|&offset| offset as usize >= audio.len() / channels)
        {
            return Err(invalid("invalid Sender audio format"));
        }
        self.update(name, true, sample_rate, channels);
        if self.clients.is_empty() { return Ok(()); }
        for (chunk_index, chunk) in audio.chunks(MAX_FRAMES * channels).enumerate() {
            let start = chunk_index * MAX_FRAMES;
            let frames = chunk.len() / channels;
            let end = start + frames;
            let bars = bar_offsets.partition_point(|&offset| (offset as usize) < start)
                ..bar_offsets.partition_point(|&offset| (offset as usize) < end);
            let packet_time = timestamp_ns.saturating_add(
                (start as u128 * 1_000_000_000 / u128::from(sample_rate)) as u64,
            );
            self.encoded.clear();
            self.encoded.extend_from_slice(&MAGIC);
            self.encoded.extend_from_slice(&packet_time.to_le_bytes());
            self.encoded.extend_from_slice(&sample_rate.to_le_bytes());
            self.encoded.extend_from_slice(&(frames as u32).to_le_bytes());
            self.encoded.extend_from_slice(&(bars.len() as u32).to_le_bytes());
            for offset in &bar_offsets[bars] {
                self.encoded.extend_from_slice(&(*offset - start as u32).to_le_bytes());
            }
            for sample in chunk {
                self.encoded.extend_from_slice(&sample.to_le_bytes());
            }
            self.clients.retain_mut(|client| client.append(&self.encoded));
        }
        Ok(())
    }
}

impl Drop for SenderEndpoint {
    fn drop(&mut self) {
        if let Ok(path) = registration_path(&self.id) { let _ = fs::remove_file(path); }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct SenderPacket {
    pub timestamp_ns: u64,
    pub sample_rate: u32,
    pub frames: usize,
}

pub struct SenderReceiver {
    pub info: SenderInfo,
    stream: TcpStream,
    pending: Vec<u8>,
    offset: usize,
    eof: bool,
}

impl SenderReceiver {
    pub fn connect(id: &str) -> io::Result<Self> {
        let registration = read_registration(id).filter(|entry| entry.audio_enabled)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Sender is unavailable"))?;
        let stream = connect(&registration, b'A')?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            info: registration.info,
            stream,
            pending: Vec::new(),
            offset: 0,
            eof: false,
        })
    }

    pub fn packet(&mut self, samples: &mut Vec<f32>, markers: &mut Vec<usize>) -> io::Result<Option<SenderPacket>> {
        if self.pending.len() - self.offset < HEADER_SIZE {
            self.receive()?;
        }
        if self.pending.len() - self.offset < HEADER_SIZE {
            return self.incomplete();
        }
        let header = &self.pending[self.offset..self.offset + HEADER_SIZE];
        if header[..4] != MAGIC { return Err(invalid("invalid Sender packet header")); }
        let timestamp_ns = u64::from_le_bytes(header[4..12].try_into().unwrap());
        let sample_rate = u32::from_le_bytes(header[12..16].try_into().unwrap());
        let frames = read_u32(&header[16..20]) as usize;
        let bars = read_u32(&header[20..24]) as usize;
        if sample_rate != self.info.sample_rate || !(1..=MAX_FRAMES).contains(&frames) || bars > MAX_MARKERS {
            return Err(invalid("invalid Sender packet size or sample rate"));
        }
        let bytes = HEADER_SIZE + (bars + frames * self.info.channels) * 4;
        if self.pending.len() - self.offset < bytes { self.receive()?; }
        if self.pending.len() - self.offset < bytes { return self.incomplete(); }
        let body = &self.pending[self.offset + HEADER_SIZE..self.offset + bytes];
        markers.clear();
        for value in body[..bars * 4].chunks_exact(4) {
            let frame = read_u32(value) as usize;
            if frame >= frames || markers.last().is_some_and(|&last| last >= frame) {
                return Err(invalid("invalid Sender bar offset"));
            }
            markers.push(frame);
        }
        samples.clear();
        samples.extend(body[bars * 4..].chunks_exact(4).map(|value| {
            let sample = f32::from_bits(read_u32(value));
            if sample.is_finite() { sample } else { 0.0 }
        }));
        self.offset += bytes;
        Ok(Some(SenderPacket { timestamp_ns, sample_rate, frames }))
    }

    fn receive(&mut self) -> io::Result<()> {
        if self.offset != 0 {
            self.pending.drain(..self.offset);
            self.offset = 0;
        }
        let mut data = [0; 16 * 1024];
        while !self.eof && self.pending.len() + data.len() <= MAX_PENDING {
            match self.stream.read(&mut data) {
                Ok(0) => self.eof = true,
                Ok(count) => self.pending.extend_from_slice(&data[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn incomplete(&self) -> io::Result<Option<SenderPacket>> {
        if self.eof {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Sender disconnected"))
        } else {
            Ok(None)
        }
    }
}

pub fn discover_senders() -> Vec<SenderInfo> {
    discover(true)
}

pub fn discover_snapshot_targets() -> Vec<SenderInfo> {
    discover(false)
}

pub fn request_sender_snapshot(id: &str) -> io::Result<()> {
    let registration = read_registration(id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Snapshot target is unavailable"))?;
    connect(&registration, b'S').map(|_| ())
}

fn discover(audio_only: bool) -> Vec<SenderInfo> {
    let Ok(directory) = sender_directory().and_then(fs::read_dir) else { return Vec::new() };
    let mut entries: Vec<_> = directory.flatten().filter_map(|entry| {
        let filename = entry.file_name();
        let id = filename.to_str()?.strip_suffix(".sender")?;
        let registration = read_registration(id)?;
        (!audio_only || registration.audio_enabled).then_some(registration.info)
    }).collect();
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    entries
}

fn connect(registration: &Registration, command: u8) -> io::Result<TcpStream> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, registration.port));
    let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)?;
    stream.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let mut request = Vec::with_capacity(registration.info.id.len() + 1);
    request.push(command);
    request.extend_from_slice(registration.info.id.as_bytes());
    stream.write_all(&request)?;
    Ok(stream)
}

fn read_registration(id: &str) -> Option<Registration> {
    let path = registration_path(id).ok()?;
    let metadata = path.metadata().ok()?;
    if metadata.len() > 1024 || metadata.modified().ok()?.elapsed().unwrap_or_default() >= EXPIRES {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let mut parts = text.splitn(5, '\t');
    let port = parts.next()?.parse().ok()?;
    let audio_enabled = match parts.next()? { "1" => true, "0" => false, _ => return None };
    let sample_rate = parts.next()?.parse().ok()?;
    let channels = parts.next()?.parse().ok()?;
    let name = parts.next()?.to_owned();
    if !valid_format(sample_rate, channels) || port == 0 { return None; }
    Some(Registration { info: SenderInfo { id: id.to_owned(), name, sample_rate, channels }, port, audio_enabled })
}

fn valid_format(rate: u32, channels: usize) -> bool {
    (8_000..=384_000).contains(&rate) && (1..=2).contains(&channels)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 72 && id.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

fn reserve_id(id: &str) -> io::Result<()> {
    let path = registration_path(id)?;
    let open = || OpenOptions::new().write(true).create_new(true).open(&path);
    match open() {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let age = path.metadata()?.modified()?.elapsed().unwrap_or_default();
            if age < EXPIRES { return Err(error); }
            fs::remove_file(&path)?;
            open().map(|_| ())
        }
        Err(error) => Err(error),
    }
}

fn create_sender_directory(path: &std::path::Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn registration_path(id: &str) -> io::Result<PathBuf> {
    if !valid_id(id) { return Err(invalid("invalid Sender instance ID")); }
    Ok(sender_directory()?.join(format!("{id}.sender")))
}

fn sender_directory() -> io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_DIRECTORY.with(|directory| directory.borrow().clone()) {
        return Ok(path);
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
    };
    base.filter(|path| path.is_absolute())
        .map(|path| path.join("Backtracking").join("senders-v1"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Backtracking Sender directory unavailable"))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

#[cfg(windows)]
pub fn capture_clock_ns() -> u64 {
    use std::sync::OnceLock;
    #[link(name = "kernel32")]
    extern "system" {
        fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
        fn QueryPerformanceCounter(counter: *mut i64) -> i32;
    }
    static FREQUENCY: OnceLock<u64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut value = 0;
        unsafe { QueryPerformanceFrequency(&mut value) };
        value.max(1) as u64
    });
    let mut ticks = 0;
    unsafe { QueryPerformanceCounter(&mut ticks) };
    (u128::from(ticks.max(0) as u64) * 1_000_000_000 / u128::from(frequency)) as u64
}

#[cfg(target_os = "macos")]
pub fn capture_clock_ns() -> u64 {
    use std::sync::OnceLock;
    #[repr(C)]
    struct Timebase { numerator: u32, denominator: u32 }
    extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    static TIMEBASE: OnceLock<Timebase> = OnceLock::new();
    let base = TIMEBASE.get_or_init(|| {
        let mut value = Timebase { numerator: 1, denominator: 1 };
        unsafe { mach_timebase_info(&mut value) };
        value
    });
    let ticks = unsafe { mach_absolute_time() };
    (u128::from(ticks) * u128::from(base.numerator) / u128::from(base.denominator.max(1))) as u64
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn capture_clock_ns() -> u64 {
    use std::ffi::{c_int, c_long};
    #[repr(C)]
    struct Timespec { seconds: c_long, nanos: c_long }
    extern "C" { fn clock_gettime(clock: c_int, time: *mut Timespec) -> c_int; }
    let mut value = Timespec { seconds: 0, nanos: 0 };
    unsafe { clock_gettime(1, &mut value) };
    (value.seconds as u64).saturating_mul(1_000_000_000).saturating_add(value.nanos as u64)
}

#[cfg(test)]
thread_local! {
    static TEST_DIRECTORY: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires TCP loopback permission"]
    fn same_named_senders_fan_out_and_snapshot_targets_are_independent() {
        let directory = std::env::temp_dir().join(format!("backtracking-ipc-{}", new_sender_id()));
        TEST_DIRECTORY.with(|path| *path.borrow_mut() = Some(directory.clone()));
        let mut first = SenderEndpoint::new().unwrap();
        let mut second = SenderEndpoint::with_id(first.id()).unwrap();
        assert_ne!(first.id(), second.id());
        first.update("Audio 1", true, 48_000, 2);
        second.update("Audio 1", false, 48_000, 2);
        assert_eq!(discover_senders().len(), 1);
        assert_eq!(discover_snapshot_targets().len(), 2);

        let mut a = SenderReceiver::connect(first.id()).unwrap();
        let mut b = SenderReceiver::connect(first.id()).unwrap();
        request_sender_snapshot(second.id()).unwrap();
        let mut audio = Vec::new();
        let mut bars = Vec::new();
        let mut received = [false; 2];
        let timeout = Instant::now() + Duration::from_secs(2);
        let mut requested = false;
        while Instant::now() < timeout {
            first.publish("Audio 1", 48_000, 2, 12_000, &[0.5, -0.5, 0.25, -0.25], &[1]).unwrap();
            second.update("Audio 1", false, 48_000, 2);
            requested |= second.take_snapshot_request();
            for (i, receiver) in [&mut a, &mut b].into_iter().enumerate() {
                if !received[i] {
                    if let Some(packet) = receiver.packet(&mut audio, &mut bars).unwrap() {
                        assert_eq!(packet, SenderPacket { timestamp_ns: 12_000, sample_rate: 48_000, frames: 2 });
                        assert_eq!(audio, [0.5, -0.5, 0.25, -0.25]);
                        assert_eq!(bars, [1]);
                        received[i] = true;
                    }
                }
            }
            if requested && received == [true, true] { break; }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(requested);
        assert!(!first.take_snapshot_request());
        assert_eq!(received, [true, true]);
        first.update("Audio 1", false, 48_000, 2);
        assert!(discover_senders().is_empty());
        drop((first, second));
        assert!(discover_snapshot_targets().is_empty());
        fs::remove_dir(directory).unwrap();
    }
}

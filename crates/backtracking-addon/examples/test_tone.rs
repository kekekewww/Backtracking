//! A complete add-on, including the Sender audio wire format, in one file.
//!
//! Build with `cargo build --release --example test_tone`. Backtracking starts it
//! with the three environment variables described in docs/ADDON_CONTRACT.md.
//! There is no dependency on backtracking-core and no audio device is opened.

use backtracking_addon::client::Client;
use backtracking_addon::protocol::{
    Action, AddonMessage, ChannelGroup, ChoiceOption, Format, HostMessage, MessageLevel, Setting,
    SettingKind, Source,
};
use backtracking_addon::{Error, Result};
use serde_json::Value;
use std::env;
use std::f64::consts::TAU;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ADDON_ID: &str = "com.example.test-tone";
const RATE: u32 = 48_000;
const FRAMES: usize = 480; // Ten milliseconds, even for the twelve-channel format.

fn main() {
    if let Err(error) = run() {
        // A desktop add-on has no console. Keep errors useful when run by hand.
        eprintln!("Test tone: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut client = Client::connect_from_env(ADDON_ID, "0.1.0")?;
    let mut tone = Tone::new();
    for (key, value) in client.settings() {
        // Persisted settings use exactly the same validation as live changes.
        tone.set_setting(key, value)?;
    }
    let mut sender = Sender::new(client.data_dir().to_path_buf(), tone.labels().len())?;
    client.send(&tone.describe(&sender.id))?;
    let block_time = Duration::from_millis(10);
    let mut next_block = Instant::now();

    loop {
        // Limit work per turn so a busy control connection cannot starve audio.
        for _ in 0..64 {
            let Some(message) = client.try_recv()? else {
                break;
            };
            match message {
                HostMessage::Quit => return Ok(()),
                HostMessage::SelectFormat { source, format } => {
                    if source != "main" || !matches!(format.as_str(), "stereo" | "5.1" | "7.1.4") {
                        report_error(&mut client, "Choose a format offered by source main.")?;
                        continue;
                    }
                    tone.format = format;
                    // Disconnect old audio readers and publish the new channel count
                    // before describe tells the host to reconnect.
                    sender.set_channels(tone.labels().len())?;
                    client.send(&tone.describe(&sender.id))?;
                }
                HostMessage::SetSetting { key, value } => match tone.set_setting(&key, &value) {
                    Ok(()) => client.send(&tone.describe(&sender.id))?,
                    Err(error) => report_error(&mut client, &error.to_string())?,
                },
                HostMessage::RunAction { id } => {
                    if id == "restart" {
                        tone.phases.fill(0.0);
                        client.send(&tone.describe(&sender.id))?;
                    } else {
                        report_error(&mut client, "The only action is restart.")?;
                    }
                }
                // Unknown messages are ignored, as required for later versions.
                _ => {}
            }
        }
        if client.is_disconnected() {
            return Ok(());
        }
        sender.poll_connections()?;
        let now = Instant::now();
        if now >= next_block {
            let samples = tone.render();
            sender.publish(monotonic_ns()?, &samples)?;
            next_block += block_time;
            // Resume real-time pacing after a stall rather than emit old audio.
            if now.saturating_duration_since(next_block) >= block_time {
                next_block = now + block_time;
            }
        }
        thread::sleep(
            next_block
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(2)),
        );
    }
}

fn report_error(client: &mut Client, text: &str) -> Result<()> {
    client.send(&AddonMessage::Message {
        level: MessageLevel::Error,
        text: text.to_owned(),
    })
}

struct Tone {
    format: String,
    level: String,
    mute: bool,
    frequency: f64,
    phases: [f64; 12],
}

impl Tone {
    fn new() -> Self {
        Self {
            format: "stereo".into(),
            level: "-12".into(),
            mute: false,
            frequency: 440.0,
            phases: [0.0; 12],
        }
    }

    fn labels(&self) -> &'static [&'static str] {
        match self.format.as_str() {
            "5.1" => &["L", "R", "C", "LFE", "Ls", "Rs"],
            "7.1.4" => &[
                "L", "R", "C", "LFE", "Ls", "Rs", "Lrs", "Rrs", "Ltf", "Rtf", "Ltr", "Rtr",
            ],
            _ => &["L", "R"],
        }
    }

    fn set_setting(&mut self, key: &str, value: &Value) -> Result<()> {
        match key {
            "level" => match value.as_str() {
                Some(level @ ("-24" | "-12" | "-6")) => self.level = level.into(),
                _ => return Err(Error::new("level must be -24, -12 or -6")),
            },
            "mute" => {
                self.mute = value
                    .as_bool()
                    .ok_or_else(|| Error::new("mute must be a boolean"))?
            }
            "frequency" => {
                self.frequency = value
                    .as_f64()
                    .filter(|v| v.is_finite() && (20.0..=20_000.0).contains(v))
                    .ok_or_else(|| Error::new("frequency must be between 20 and 20000 Hz"))?;
            }
            _ => return Err(Error::new("Unknown setting; use level, mute or frequency")),
        }
        Ok(())
    }

    fn describe(&self, sender: &str) -> AddonMessage {
        AddonMessage::Describe {
            sources: vec![Source {
                id: "main".into(),
                name: "Test tone".into(),
                sender: sender.into(),
                formats: [("stereo", "Stereo"), ("5.1", "5.1"), ("7.1.4", "7.1.4")]
                    .into_iter()
                    .map(|(id, label)| Format {
                        id: id.into(),
                        label: label.into(),
                    })
                    .collect(),
                format: self.format.clone(),
                groups: Some(vec![ChannelGroup {
                    id: "channels".into(),
                    label: "Channels".into(),
                    channels: self.labels().iter().map(|v| (*v).into()).collect(),
                }]),
                transport: None,
            }],
            settings: vec![
                Setting {
                    key: "level".into(),
                    label: "Level".into(),
                    help: Some("How loud the tone is.".into()),
                    kind: SettingKind::Choice {
                        default: "-12".into(),
                        options: ["-24", "-12", "-6"]
                            .into_iter()
                            .map(|value| ChoiceOption {
                                value: value.into(),
                                label: format!("{value} dB"),
                            })
                            .collect(),
                    },
                },
                Setting {
                    key: "mute".into(),
                    label: "Mute".into(),
                    help: None,
                    kind: SettingKind::Toggle { default: false },
                },
                Setting {
                    key: "frequency".into(),
                    label: "Frequency of channel 1 (Hz)".into(),
                    help: None,
                    kind: SettingKind::Number {
                        default: 440.0,
                        min: 20.0,
                        max: 20_000.0,
                        step: 1.0,
                    },
                },
            ],
            actions: vec![Action {
                id: "restart".into(),
                label: "Restart".into(),
            }],
        }
    }

    fn render(&mut self) -> Vec<f32> {
        let channels = self.labels().len();
        let gain = if self.mute {
            0.0
        } else {
            10.0_f64.powf(self.level.parse::<f64>().unwrap_or(-12.0) / 20.0)
        };
        let mut samples = Vec::with_capacity(FRAMES * channels);
        for _ in 0..FRAMES {
            for channel in 0..channels {
                let mut frequency = if channel == 0 {
                    self.frequency
                } else {
                    440.0 + 110.0 * channel as f64
                };
                // Keep channels distinct even if the user chooses another channel's tone.
                if channel != 0 && (frequency - self.frequency).abs() < 0.001 {
                    frequency += 55.0;
                }
                samples.push((self.phases[channel].sin() * gain) as f32);
                self.phases[channel] =
                    (self.phases[channel] + TAU * frequency / f64::from(RATE)) % TAU;
            }
        }
        samples
    }
}

// Each peer has a bounded handshake and one pending packet. A reader that stops
// consuming audio is dropped rather than holding up rendering or other readers.
struct AudioPeer {
    stream: TcpStream,
    handshake: Vec<u8>,
    connected: Instant,
    mode: Option<u8>,
    output: Vec<u8>,
    written: usize,
}

struct Sender {
    id: String,
    listener: TcpListener,
    registration: PathBuf,
    channels: usize,
    peers: Vec<AudioPeer>,
    refreshed: Instant,
}

impl Sender {
    fn new(addon_data: PathBuf, channels: usize) -> Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| Error::new(e.to_string()))?
            .as_nanos();
        let id = format!("{:x}-{stamp:x}", std::process::id());
        let directory = sender_directory(addon_data)?;
        fs::create_dir_all(&directory)?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let sender = Self {
            registration: directory.join(format!("{id}.sender")),
            id,
            listener,
            channels,
            peers: Vec::new(),
            refreshed: Instant::now(),
        };
        sender.register()?;
        Ok(sender)
    }

    fn register(&self) -> Result<()> {
        let contents = format!(
            "{}\t1\t{RATE}\t{}\tTest tone",
            self.listener.local_addr()?.port(),
            self.channels
        );
        let temporary = self.registration.with_extension("tmp");
        fs::write(&temporary, contents)?;
        fs::rename(temporary, &self.registration)?;
        Ok(())
    }

    fn set_channels(&mut self, channels: usize) -> Result<()> {
        self.peers.clear();
        self.channels = channels;
        self.register()?;
        self.refreshed = Instant::now();
        Ok(())
    }

    fn poll_connections(&mut self) -> Result<()> {
        for _ in 0..32 {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(true)?;
                    stream.set_nodelay(true)?;
                    if self.peers.len() < 32 {
                        self.peers.push(AudioPeer {
                            stream,
                            handshake: Vec::new(),
                            connected: Instant::now(),
                            mode: None,
                            output: Vec::new(),
                            written: 0,
                        });
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
        }
        let id = &self.id;
        self.peers.retain_mut(|peer| {
            if peer.mode.is_none() {
                if peer.connected.elapsed() > Duration::from_secs(2) {
                    return false;
                }
                let mut bytes = [0; 80];
                match peer
                    .stream
                    .read(&mut bytes[..1 + id.len() - peer.handshake.len()])
                {
                    Ok(0) => return false,
                    Ok(count) => peer.handshake.extend_from_slice(&bytes[..count]),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return true,
                    Err(_) => return false,
                }
                if peer.handshake.len() == 1 + id.len() {
                    // `A` asks for audio. `S` is a request Backtracking sends to its
                    // own plug-in; an add-on closes it, like any other first byte.
                    if peer.handshake[0] != b'A' || &peer.handshake[1..] != id.as_bytes() {
                        return false;
                    }
                    peer.mode = Some(peer.handshake[0]);
                }
            }
            flush_peer(peer)
        });
        if self.refreshed.elapsed() >= Duration::from_secs(1) {
            self.register()?;
            self.refreshed = Instant::now();
        }
        Ok(())
    }

    fn publish(&mut self, timestamp: u64, samples: &[f32]) -> Result<()> {
        let mut packet = Vec::with_capacity(24 + samples.len() * 4);
        packet.extend_from_slice(b"BTA1");
        packet.extend_from_slice(&timestamp.to_le_bytes());
        packet.extend_from_slice(&RATE.to_le_bytes());
        packet.extend_from_slice(&(FRAMES as u32).to_le_bytes());
        packet.extend_from_slice(&0_u32.to_le_bytes()); // No musical bar markers.
        for sample in samples {
            packet.extend_from_slice(&sample.to_le_bytes());
        }
        self.peers.retain_mut(|peer| {
            if peer.mode.is_none() {
                return true;
            }
            if !flush_peer(peer) || peer.written != peer.output.len() {
                return false;
            }
            peer.output.clone_from(&packet);
            peer.written = 0;
            flush_peer(peer)
        });
        Ok(())
    }
}

fn flush_peer(peer: &mut AudioPeer) -> bool {
    while peer.written < peer.output.len() {
        match peer.stream.write(&peer.output[peer.written..]) {
            Ok(0) => return false,
            Ok(count) => peer.written += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return true,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    true
}

impl Drop for Sender {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.registration);
    }
}

fn sender_directory(addon_data: PathBuf) -> Result<PathBuf> {
    #[cfg(windows)]
    if let Some(base) = env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(base).join("Backtracking/senders-v1"));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home).join("Library/Application Support/Backtracking/senders-v1"));
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(base) = env::var_os("XDG_RUNTIME_DIR") {
            return Ok(PathBuf::from(base).join("Backtracking/senders-v1"));
        }
        if let Some(home) = env::var_os("HOME") {
            return Ok(PathBuf::from(home).join(".local/state/Backtracking/senders-v1"));
        }
    }
    addon_data
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("senders-v1"))
        .ok_or_else(|| Error::new("Cannot locate Backtracking's Sender registration folder"))
}

// The wire timestamp must share the host's OS clock. Instant has no portable
// epoch, so convert the actual platform clock rather than time since startup.
#[cfg(windows)]
fn monotonic_ns() -> Result<u64> {
    #[link(name = "Kernel32")]
    extern "system" {
        fn QueryPerformanceCounter(value: *mut i64) -> i32;
        fn QueryPerformanceFrequency(value: *mut i64) -> i32;
    }
    let (mut ticks, mut frequency) = (0_i64, 0_i64);
    // SAFETY: Both functions write one i64 to a valid local pointer.
    if unsafe { QueryPerformanceCounter(&mut ticks) } == 0
        || unsafe { QueryPerformanceFrequency(&mut frequency) } == 0
        || ticks < 0
        || frequency <= 0
    {
        return Err(Error::new("QueryPerformanceCounter is unavailable"));
    }
    Ok((ticks as u128 * 1_000_000_000 / frequency as u128) as u64)
}

#[cfg(target_os = "macos")]
fn monotonic_ns() -> Result<u64> {
    #[repr(C)]
    struct Timebase {
        numerator: u32,
        denominator: u32,
    }
    extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut info = Timebase {
        numerator: 0,
        denominator: 0,
    };
    // SAFETY: The timebase pointer names a correctly laid out writable value.
    if unsafe { mach_timebase_info(&mut info) } != 0 || info.denominator == 0 {
        return Err(Error::new("mach_absolute_time is unavailable"));
    }
    // SAFETY: mach_absolute_time takes no pointers and has no preconditions.
    Ok(
        (u128::from(unsafe { mach_absolute_time() }) * u128::from(info.numerator)
            / u128::from(info.denominator)) as u64,
    )
}

#[cfg(target_os = "linux")]
fn monotonic_ns() -> Result<u64> {
    use std::os::raw::{c_int, c_long};
    #[repr(C)]
    struct Timespec {
        seconds: c_long,
        nanoseconds: c_long,
    }
    extern "C" {
        fn clock_gettime(clock: c_int, time: *mut Timespec) -> c_int;
    }
    let mut time = Timespec {
        seconds: 0,
        nanoseconds: 0,
    };
    // SAFETY: CLOCK_MONOTONIC is 1 on Linux; time is a valid timespec pointer.
    if unsafe { clock_gettime(1, &mut time) } != 0 || time.seconds < 0 || time.nanoseconds < 0 {
        return Err(Error::new("CLOCK_MONOTONIC is unavailable"));
    }
    Ok(time.seconds as u64 * 1_000_000_000 + time.nanoseconds as u64)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn monotonic_ns() -> Result<u64> {
    Err(Error::new("This example supports Windows, macOS and Linux"))
}

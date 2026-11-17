//! Exercise the copied example as a real process and speak both wire protocols.

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct RunningTone {
    child: ChildGuard,
    root: PathBuf,
    control: TcpStream,
    incoming: BufReader<TcpStream>,
}

struct ChildGuard {
    child: Child,
    root: PathBuf,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Guard the process from the moment it is spawned, including startup failures.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl RunningTone {
    fn start() -> Self {
        // Cargo builds examples as part of `cargo test`; their executables sit
        // beside deps even when CARGO_TARGET_DIR points outside the crate.
        let executable = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples")
            .join(format!("test_tone{}", std::env::consts::EXE_SUFFIX));
        assert!(
            executable.is_file(),
            "Cargo did not build {}",
            executable.display()
        );
        let root = std::env::temp_dir().join(format!(
            "backtracking-test-tone-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let data = root.join("Backtracking/addon-data/com.example.test-tone");
        fs::create_dir_all(&data).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let child = Command::new(executable)
            .env(
                "BACKTRACKING_ADDON_PORT",
                listener.local_addr().unwrap().port().to_string(),
            )
            .env("BACKTRACKING_ADDON_TOKEN", "e2e-token")
            .env("BACKTRACKING_ADDON_DATA", &data)
            .env("LOCALAPPDATA", &root)
            .env("XDG_RUNTIME_DIR", &root)
            .env("HOME", &root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut child = ChildGuard {
            child,
            root: root.clone(),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let control = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        child.child.try_wait().unwrap().is_none(),
                        "example exited before connecting"
                    );
                    assert!(
                        Instant::now() < deadline,
                        "example did not connect within five seconds"
                    );
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("accept example: {error}"),
            }
        };
        // Accepted sockets can inherit FIONBIO on Windows. This test host uses
        // blocking, bounded reads rather than the crate's nonblocking host API.
        control.set_nonblocking(false).unwrap();
        control
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let incoming = BufReader::new(control.try_clone().unwrap());
        let mut tone = Self {
            child,
            root,
            control,
            incoming,
        };
        let hello = tone.receive();
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["id"], "com.example.test-tone");
        assert_eq!(hello["contract"], 1);
        assert_eq!(hello["token"], "e2e-token");
        tone.send(json!({"type":"welcome","contract":1,"app":"Backtracking","app_version":"test","settings":{"level":"-12"}}));
        tone
    }

    fn send(&mut self, message: Value) {
        writeln!(self.control, "{message}").unwrap();
    }

    fn receive(&mut self) -> Value {
        let mut line = String::new();
        assert_ne!(
            self.incoming.read_line(&mut line).unwrap(),
            0,
            "example closed its control connection"
        );
        serde_json::from_str(&line).unwrap()
    }

    fn registrations(&self) -> PathBuf {
        #[cfg(target_os = "macos")]
        return self
            .root
            .join("Library/Application Support/Backtracking/senders-v1");
        #[cfg(not(target_os = "macos"))]
        self.root.join("Backtracking/senders-v1")
    }

    fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = self.child.child.try_wait().unwrap() {
                assert!(status.success(), "example exited with {status}");
                break;
            }
            assert!(Instant::now() < deadline, "example did not exit promptly");
            thread::sleep(Duration::from_millis(2));
        }
    }
}

#[test]
fn example_handshakes_selects_surround_and_streams_six_distinct_channels() {
    let mut tone = RunningTone::start();
    let initial = tone.receive();
    assert_eq!(initial["type"], "describe");
    assert_eq!(initial["sources"][0]["name"], "Test tone");
    assert_eq!(
        initial["sources"][0]["groups"][0]["channels"],
        json!(["L", "R"])
    );
    assert_eq!(
        initial["sources"][0]["formats"].as_array().unwrap().len(),
        3
    );
    assert_eq!(initial["settings"].as_array().unwrap().len(), 3);
    assert_eq!(initial["actions"][0]["id"], "restart");

    tone.send(json!({"type":"select_format","source":"main","format":"5.1"}));
    let surround = tone.receive();
    assert_eq!(surround["sources"][0]["format"], "5.1");
    assert_eq!(
        surround["sources"][0]["groups"][0]["channels"],
        json!(["L", "R", "C", "LFE", "Ls", "Rs"])
    );
    let sender = surround["sources"][0]["sender"].as_str().unwrap();
    let registration = tone.registrations().join(format!("{sender}.sender"));
    let contents = fs::read_to_string(&registration).unwrap();
    let fields: Vec<_> = contents.split('\t').collect();
    assert_eq!(fields.len(), 5);
    assert_eq!(&fields[1..], &["1", "48000", "6", "Test tone"]);
    let port = fields[0].parse::<u16>().unwrap();

    // A silent audio peer must not hold up another peer's handshake or stream.
    let _silent = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut audio = TcpStream::connect(("127.0.0.1", port)).unwrap();
    audio
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(audio, "A{sender}").unwrap();
    let (timestamp, samples) = read_packet(&mut audio, 6);
    assert!(timestamp > 0);
    assert!(samples.iter().any(|value| value.abs() > 0.1));
    for channel in 1..6 {
        assert!(
            (1..480).any(|frame| (samples[frame * 6] - samples[frame * 6 + channel]).abs() > 0.01),
            "channel {channel} duplicates channel one"
        );
    }
    let (next_timestamp, _) = read_packet(&mut audio, 6);
    assert!(next_timestamp > timestamp);

    tone.send(json!({"type":"set_setting","key":"mute","value":true}));
    assert_eq!(tone.receive()["type"], "describe");
    assert!(
        (0..20).any(|_| read_packet(&mut audio, 6)
            .1
            .iter()
            .all(|value| *value == 0.0)),
        "mute did not reach the Sender stream"
    );
    tone.send(json!({"type":"run_action","id":"restart"}));
    assert_eq!(tone.receive()["type"], "describe");
    tone.send(json!({"type":"quit"}));
    tone.wait_for_exit();
    assert!(
        !registration.exists(),
        "Sender registration survived normal exit"
    );
}

#[test]
fn example_exits_and_removes_registration_when_host_disconnects() {
    let mut tone = RunningTone::start();
    let description = tone.receive();
    let sender = description["sources"][0]["sender"].as_str().unwrap();
    let registration = tone.registrations().join(format!("{sender}.sender"));
    assert!(registration.exists());
    tone.control.shutdown(std::net::Shutdown::Both).unwrap();
    tone.wait_for_exit();
    assert!(!registration.exists());
}

fn read_packet(audio: &mut TcpStream, channels: usize) -> (u64, Vec<f32>) {
    let mut header = [0; 24];
    audio.read_exact(&mut header).unwrap();
    assert_eq!(&header[..4], b"BTA1");
    let timestamp = u64::from_le_bytes(header[4..12].try_into().unwrap());
    assert_eq!(
        u32::from_le_bytes(header[12..16].try_into().unwrap()),
        48_000
    );
    let frames = u32::from_le_bytes(header[16..20].try_into().unwrap());
    assert_eq!(frames, 480);
    assert_eq!(u32::from_le_bytes(header[20..24].try_into().unwrap()), 0);
    let mut bytes = vec![0; frames as usize * channels * 4];
    audio.read_exact(&mut bytes).unwrap();
    let samples: Vec<_> = bytes
        .chunks_exact(4)
        .map(|value| f32::from_le_bytes(value.try_into().unwrap()))
        .collect();
    assert!(samples.iter().all(|value| value.is_finite()));
    (timestamp, samples)
}

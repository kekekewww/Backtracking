//! Launch the example as an add-on and exercise the control connection.

use backtracking_addon::host::{Connection, Host, HostEvent};
use backtracking_addon::manifest::{Capability, Manifest};
use backtracking_addon::protocol::{encode_addon, HostMessage};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn receive_appearance(connection: &mut Connection) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(message) = connection.try_recv().unwrap() {
            let wire: Value = serde_json::from_slice(&encode_addon(&message).unwrap()).unwrap();
            assert_eq!(wire["type"], "appearance");
            return wire;
        }
        assert!(!connection.is_disconnected(), "example disconnected");
        assert!(
            Instant::now() < deadline,
            "example did not send its appearance"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn theme_example_offers_an_appearance_animates_its_border_and_quits() {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/theme-package");
    let manifest: Manifest =
        serde_json::from_str(&fs::read_to_string(package.join("addon.json")).unwrap()).unwrap();
    assert_eq!(manifest.contract, 1);
    assert_eq!(manifest.capabilities, [Capability::Appearance]);

    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("theme_example{}", std::env::consts::EXE_SUFFIX));
    assert!(
        executable.is_file(),
        "Cargo did not build {}",
        executable.display()
    );

    let mut host = Host::bind("test").unwrap();
    host.expect_addon(&manifest.id, "theme-test", BTreeMap::new())
        .unwrap();
    let mut child = ChildGuard(
        Command::new(executable)
            .current_dir(package)
            .env("BACKTRACKING_ADDON_PORT", host.port().to_string())
            .env("BACKTRACKING_ADDON_TOKEN", "theme-test")
            .env("BACKTRACKING_ADDON_DATA", std::env::temp_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    let mut connection = loop {
        if let Some(event) = host.poll().unwrap().into_iter().next() {
            match event {
                HostEvent::Connected(connection) => break connection,
                HostEvent::Rejected { reason, .. } => {
                    panic!("example handshake rejected: {reason}")
                }
            }
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "example exited before connecting"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "example did not connect"
        );
        thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(connection.id(), manifest.id);

    let mut original = receive_appearance(&mut connection);
    assert!(original["colors"]["waveform"].is_string());
    let mut border = original["colors"]
        .as_object_mut()
        .unwrap()
        .remove("border")
        .unwrap();
    assert!(border.is_string());
    for _ in 0..3 {
        let mut update = receive_appearance(&mut connection);
        let next = update["colors"]
            .as_object_mut()
            .unwrap()
            .remove("border")
            .unwrap();
        assert!(next.is_string());
        assert_ne!(next, border);
        assert_eq!(update, original);
        border = next;
    }
    assert!(started.elapsed() >= Duration::from_millis(500));

    connection.send(&HostMessage::Quit).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "example exited with {status}");
            break;
        }
        assert!(Instant::now() < deadline, "example did not exit promptly");
        thread::sleep(Duration::from_millis(2));
    }
}

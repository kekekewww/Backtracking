//! Launch the control example against a stand-in host: it waits until control is allowed,
//! then runs its sequence through `Client::control`, matching each reply to its request.

use backtracking_addon::host::{Connection, Host, HostEvent};
use backtracking_addon::manifest::{Capability, Manifest};
use backtracking_addon::protocol::{
    control_codes, AddonMessage, ControlCommand, ControlError, ControlReply, ControlResult,
    ControlState, HostMessage, SavedFile, SelectionState,
};
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

fn receive(connection: &mut Connection) -> AddonMessage {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(message) = connection.try_recv().unwrap() {
            return message;
        }
        assert!(!connection.is_disconnected(), "example disconnected");
        assert!(Instant::now() < deadline, "example sent nothing");
        thread::sleep(Duration::from_millis(2));
    }
}

fn request(connection: &mut Connection) -> (String, ControlCommand) {
    match receive(connection) {
        AddonMessage::Control { id, command } => (id, command),
        other => panic!("expected a control request, got {other:?}"),
    }
}

#[test]
fn control_example_waits_for_consent_then_pauses_selects_saves_and_resumes() {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/control-package");
    let manifest: Manifest =
        serde_json::from_str(&fs::read_to_string(package.join("addon.json")).unwrap()).unwrap();
    assert_eq!(manifest.contract, 1);
    assert_eq!(
        manifest.capabilities,
        [Capability::Control, Capability::Settings, Capability::Actions]
    );

    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("control_example{}", std::env::consts::EXE_SUFFIX));
    assert!(
        executable.is_file(),
        "Cargo did not build {}",
        executable.display()
    );

    let data = std::env::temp_dir().join(format!("control-example-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&data);
    fs::create_dir_all(&data).unwrap();
    let folder = data.join("out");

    let mut host = Host::bind("test").unwrap();
    host.expect_addon(&manifest.id, "control-test", BTreeMap::new())
        .unwrap();
    let mut child = ChildGuard(
        Command::new(executable)
            .current_dir(&package)
            .env("BACKTRACKING_ADDON_PORT", host.port().to_string())
            .env("BACKTRACKING_ADDON_TOKEN", "control-test")
            .env("BACKTRACKING_ADDON_DATA", &data)
            .env("BACKTRACKING_CONTROL_EXAMPLE_FOLDER", &folder)
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
                HostEvent::Rejected { reason, .. } => panic!("handshake rejected: {reason}"),
            }
        }
        assert!(child.0.try_wait().unwrap().is_none(), "example exited early");
        assert!(started.elapsed() < Duration::from_secs(5), "no connection");
        thread::sleep(Duration::from_millis(2));
    };
    assert!(matches!(
        receive(&mut connection),
        AddonMessage::Describe { .. }
    ));

    // Off by default: the example tries once, is refused, and waits.
    connection
        .send(&HostMessage::ControlAccess { allowed: false })
        .unwrap();
    let (id, command) = request(&mut connection);
    assert_eq!(command, ControlCommand::GetState);
    connection
        .send(&HostMessage::ControlReply(ControlReply::failure(
            Some(id),
            ControlError::new(control_codes::NOT_ALLOWED, "not allowed"),
        )))
        .unwrap();
    thread::sleep(Duration::from_millis(300));
    assert!(connection.try_recv().unwrap().is_none(), "asked again while refused");

    connection
        .send(&HostMessage::ControlAccess { allowed: true })
        .unwrap();
    let mut state = ControlState {
        capturing: true,
        recording: true,
        sample_rate: 48_000,
        ..ControlState::default()
    };
    let expected = [
        ControlCommand::GetState,
        ControlCommand::PauseCapture,
        ControlCommand::select_seconds_ago(2.0, 0.0),
        ControlCommand::save(Some(folder.display().to_string())),
        ControlCommand::ClearSelection,
        ControlCommand::StartCapture,
        ControlCommand::GetState,
    ];
    let mut ids = Vec::new();
    for wanted in expected {
        let (id, command) = request(&mut connection);
        assert_eq!(command, wanted);
        assert!(!ids.contains(&id), "id {id} used twice");
        ids.push(id.clone());
        let mut result = ControlResult::default();
        match command {
            ControlCommand::PauseCapture => state.capturing = false,
            ControlCommand::StartCapture => state.capturing = true,
            ControlCommand::Select { .. } => {
                state.selection = Some(SelectionState {
                    start_frame: 0,
                    end_frame: 96_000,
                    seconds: 2.0,
                    from_seconds_ago: 2.0,
                    to_seconds_ago: 0.0,
                })
            }
            ControlCommand::ClearSelection => state.selection = None,
            ControlCommand::Save { .. } => {
                assert!(folder.is_dir(), "the example makes its folder");
                result.folder = Some(folder.display().to_string());
                result.files = Some(vec![SavedFile {
                    path: folder.join("A.wav").display().to_string(),
                    kind: "wav".into(),
                    lane: Some("A".into()),
                    channels: Some(2),
                    sample_rate: Some(48_000),
                    frames: Some(96_000),
                }]);
            }
            _ => {}
        }
        result.state = state.clone();
        // A stray reply to an id never used is ignored, not taken for this one.
        connection
            .send(&HostMessage::ControlReply(ControlReply::success(
                "unused",
                &result,
            )))
            .unwrap();
        connection
            .send(&HostMessage::ControlReply(ControlReply::success(id, &result)))
            .unwrap();
    }
    match receive(&mut connection) {
        AddonMessage::Message { text, .. } => {
            assert_eq!(text, format!("Saved 1 file(s) to {}", folder.display()))
        }
        other => panic!("expected the summary message, got {other:?}"),
    }

    let transcript = fs::read_to_string(data.join("transcript.log")).unwrap();
    for line in [
        "< error not_allowed: not allowed",
        "> {\"command\":\"pause_capture\"}",
        "> {\"command\":\"select\",\"from_seconds_ago\":2.0,\"to_seconds_ago\":0.0}",
        "lane=A channels=2 rate=48000 frames=96000",
        "> {\"command\":\"start_capture\"}",
    ] {
        assert!(transcript.contains(line), "{line:?} missing from\n{transcript}");
    }

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
    let _ = fs::remove_dir_all(&data);
}

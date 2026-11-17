//! Real add-on coverage runs in a child test process with its own data root.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use backtracking_addon::package;
use backtracking_core::{ChannelRoute, Fades, TakeLabel, WavExportProfile};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

use crate::addons::{AddonHandle, AddonManager, AddonState};
use crate::clip_export::{ExportOptions, export_clip};
use crate::controller::{CaptureSource, Controller, SourceSettings};
use crate::preferences::WideExportMode;

const ADDON_ID: &str = "com.example.test-tone";
const CHILD_ROOT: &str = "BACKTRACKING_DESKTOP_ADDON_TEST_ROOT";
const CHILD_PACKAGE: &str = "BACKTRACKING_DESKTOP_ADDON_TEST_PACKAGE";

struct TestFolder(PathBuf);

impl TestFolder {
    fn new() -> Self {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        fs::create_dir_all(&base).unwrap();
        let root = base.join(format!(
            "backtracking-desktop-addon-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
}

impl Drop for TestFolder {
    fn drop(&mut self) {
        // Only this invocation's newly created temporary directory is removed.
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// A fresh package may be reused locally. Clean CI builds the actual example in
/// the add-on crate's build cache, then packs it in this test's temporary folder.
fn example_package(temporary: &Path) -> PathBuf {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let addon = repository.join("crates/backtracking-addon");
    let ready = addon.join("target/test-tone.btaddon");
    let latest_input = newest_input(&addon.join("src"))
        .max(newest_input(&addon.join("examples")))
        .max(newest_input(&addon.join("Cargo.toml")))
        .max(newest_input(&addon.join("Cargo.lock")));
    if std::env::var_os("BACKTRACKING_TEST_REBUILD_ADDON").is_none()
        && fs::metadata(&ready)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|time| time >= latest_input)
        && package::inspect(&ready).is_ok()
    {
        eprintln!("Using current example package {}", ready.display());
        return ready;
    }

    // Reuse the crate's target directory rather than duplicating Rust build
    // artifacts on the system drive for every desktop test invocation.
    let build = addon.join("target");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let result = Command::new(cargo)
        .args(["build", "--offline", "--locked", "--manifest-path"])
        .arg(addon.join("Cargo.toml"))
        .args(["--example", "test_tone", "--target-dir"])
        .arg(&build)
        .stdin(Stdio::null())
        .output()
        .expect("build the real test_tone example");
    assert!(
        result.status.success(),
        "example build failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let folder = temporary.join("package");
    fs::create_dir_all(folder.join("bin")).unwrap();
    fs::copy(
        addon.join("examples/test-tone-package/addon.json"),
        folder.join("addon.json"),
    )
    .unwrap();
    fs::copy(
        build.join("debug/examples/test_tone.exe"),
        folder.join("bin/test-tone.exe"),
    )
    .unwrap();
    let output = temporary.join("test-tone.btaddon");
    package::pack(&folder, &output).unwrap();
    eprintln!("Built current example package {}", output.display());
    output
}

fn newest_input(path: &Path) -> SystemTime {
    let metadata = fs::metadata(path).unwrap();
    if metadata.is_file() {
        return metadata.modified().unwrap();
    }
    fs::read_dir(path)
        .unwrap()
        .map(|entry| newest_input(&entry.unwrap().path()))
        .max()
        .unwrap_or(UNIX_EPOCH)
}

#[test]
fn real_addon_installs_captures_surround_exports_and_stops() {
    let temporary = TestFolder::new();
    let package = example_package(&temporary.0);
    let local = temporary.0.join("local");
    fs::create_dir(&local).unwrap();
    let log_path = temporary.0.join("child.log");
    let log = fs::File::create(&log_path).unwrap();
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "addon_integration::test_tone_child",
                "--nocapture",
            ])
            .env(CHILD_ROOT, &local)
            .env(CHILD_PACKAGE, &package)
            .env("LOCALAPPDATA", &local)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "isolated add-on test timed out:\n{}",
            fs::read_to_string(&log_path).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(20));
    };
    let log = fs::read_to_string(&log_path).unwrap();
    assert!(status.success(), "isolated add-on test failed:\n{log}");
    assert!(
        log.contains("desktop add-on end-to-end checks passed"),
        "child harness did not execute the test body:\n{log}"
    );
    eprintln!("{log}");
}

fn command<T>(reply: mpsc::Receiver<Result<T, String>>) -> T {
    reply
        .recv_timeout(Duration::from_secs(8))
        .expect("manager command timed out")
        .expect("manager command failed")
}

fn wait_for(mut condition: impl FnMut() -> bool, reason: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if condition() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out: {reason}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_running(handle: &AddonHandle, format: &str) {
    wait_for(
        || {
            handle.snapshot().addon(ADDON_ID).is_some_and(|addon| {
                addon.state == AddonState::Running
                    && addon
                        .sources
                        .iter()
                        .any(|source| source.id == "main" && source.format == format)
            })
        },
        "add-on running with the requested format",
    );
}

#[test]
fn test_tone_child() {
    let Some(local) = std::env::var_os(CHILD_ROOT).map(PathBuf::from) else {
        return;
    };
    assert_eq!(
        std::env::var_os("LOCALAPPDATA").as_deref(),
        Some(local.as_os_str())
    );
    let package = PathBuf::from(std::env::var_os(CHILD_PACKAGE).expect("isolated test package"));
    let root = local.join("Backtracking");
    let manager = AddonManager::new(root.clone()).unwrap();
    let handle = manager.handle();
    let reviewed = command(handle.inspect(package.clone()));
    let installed = command(handle.install(package.clone(), reviewed.identity));
    assert_eq!(installed.manifest.id, ADDON_ID);
    assert!(
        root.join("addons")
            .join(ADDON_ID)
            .join("addon.json")
            .is_file()
    );
    command(handle.set_enabled(ADDON_ID, true));
    wait_running(&handle, "stereo");
    command(handle.set_setting(ADDON_ID, "level", serde_json::json!("-6")));
    assert!(root.join("addon-data").join(ADDON_ID).is_dir());

    let pid = handle.snapshot().addon(ADDON_ID).unwrap().pid.unwrap();
    let process = ProcessHandle(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.unwrap());
    assert_eq!(unsafe { WaitForSingleObject(process.0, 0) }, WAIT_TIMEOUT);
    let secondary = AddonManager::connect_existing(root.clone()).unwrap();
    let secondary_handle = secondary.handle();
    wait_running(&secondary_handle, "stereo");
    assert_eq!(
        secondary_handle.snapshot().addon(ADDON_ID).unwrap().pid,
        Some(pid),
        "secondary window duplicated the add-on process"
    );
    let listed = Controller::available_sources_with_addons(&handle.snapshot());
    let source = listed.iter().find(|source| matches!(source,
        CaptureSource::Addon { addon_id, source_id, .. } if addon_id == ADDON_ID && source_id == "main"
    )).cloned().expect("add-on source appears in capture list");
    let sender = handle.snapshot().addon(ADDON_ID).unwrap().sources[0]
        .sender
        .clone();
    assert!(
        !listed.iter().any(|source| matches!(source,
        CaptureSource::Sender { id, .. } if id == &sender)),
        "add-on Sender was listed twice"
    );

    let mut controller = Controller::with_addons(handle.clone()).unwrap();
    controller.set_sources([
        Some(SourceSettings {
            source,
            route: ChannelRoute::Merged,
        }),
        None,
    ]);
    controller.set_watching(false);
    controller.resume();
    wait_for(
        || {
            controller.poll();
            controller.track_channels(0) == Some(2)
        },
        "stereo lane starts",
    );
    let generation = controller.generation();
    command(secondary_handle.select_format(ADDON_ID, "main", "5.1"));
    wait_running(&handle, "5.1");
    drop(secondary);
    assert_eq!(handle.snapshot().addon(ADDON_ID).unwrap().pid, Some(pid));
    assert_eq!(
        unsafe { WaitForSingleObject(process.0, 0) },
        WAIT_TIMEOUT,
        "closing a secondary window stopped the primary add-on"
    );
    wait_for(
        || {
            controller.poll();
            controller.track_channels(0) == Some(6) && controller.generation() > generation
        },
        "lane restarts automatically for six channels",
    );

    let capture_deadline = Instant::now() + Duration::from_secs(8);
    let snapshot = loop {
        let snapshot = controller
            .request_edit_snapshot()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        if let Some(snapshot) = snapshot {
            if snapshot.snapshots.first().is_some_and(|track| {
                track.channels() == 6 && track.frame_range().end - track.frame_range().start >= 4096
            }) {
                break snapshot;
            }
        }
        assert!(
            Instant::now() < capture_deadline,
            "six-channel lane did not retain audio: {:?}",
            controller.source_status(0)
        );
        // The capture worker continues to run without any visible-window poll.
        thread::sleep(Duration::from_millis(20));
    };
    let track = &snapshot.snapshots[0];
    let range = track.frame_range();
    let visible = (range.end - 2048)..range.end;
    let mut frozen = Vec::new();
    for channel in 0..6 {
        let (mut low, mut high) = (vec![0.0; 128], vec![0.0; 128]);
        assert_eq!(
            track.channel_waveform(channel, visible.clone(), &mut low, &mut high),
            2048
        );
        assert!(
            high.iter().any(|value| *value > 0.4),
            "channel {channel} has no waveform at the changed level"
        );
        frozen.push((low, high));
    }
    for channel in 1..6 {
        for other in 0..channel {
            assert!(
                frozen[channel]
                    .0
                    .iter()
                    .zip(&frozen[other].0)
                    .any(|(a, b)| (a - b).abs() > 0.01),
                "channel {channel} duplicates channel {other}"
            );
        }
    }
    let mut live: Vec<_> = (0..6).map(|_| (vec![0.0; 128], vec![0.0; 128])).collect();
    let channels: Vec<_> = (0..6).map(|channel| (0, channel)).collect();
    wait_for(
        || {
            let mut traces: Vec<_> = live
                .iter_mut()
                .map(|(low, high)| (low.as_mut_slice(), high.as_mut_slice()))
                .collect();
            controller
                .try_render_channel_waveforms(
                    &channels,
                    &[],
                    Some(visible.clone()),
                    &mut traces,
                    &mut [],
                )
                .is_some()
        },
        "live per-channel waveforms",
    );
    assert_eq!(
        live, frozen,
        "live and frozen channel waveforms disagree for identical frames"
    );

    let group = snapshot.groups[0]
        .iter()
        .find(|group| snapshot.shown_groups[0].as_deref() == Some(group.id.as_str()))
        .cloned()
        .expect("frozen snapshot includes the group on show");
    assert_eq!(group.channels, 0..6);
    assert_eq!(group.labels, ["L", "R", "C", "LFE", "Ls", "Rs"]);
    let options = ExportOptions {
        groups: [Some(group.clone()), None],
        wide_mode: WideExportMode::MonoFiles,
        addons: Some(handle.clone()),
    };
    let files = export_clip(
        &root.join("exports"),
        &snapshot.snapshots,
        (visible.start - range.start) as usize..(visible.end - range.start) as usize,
        Fades::NONE,
        WavExportProfile::default(),
        &[ChannelRoute::Merged],
        false,
        &[TakeLabel::default()],
        &options,
    )
    .unwrap();
    assert_eq!(files.len(), 6);
    let captured = track.snapshot_range(visible.clone()).unwrap();
    for (channel, (file, label)) in files.iter().zip(&group.labels).enumerate() {
        assert!(
            file.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(&format!("-Ch{:02}_{label}", channel + 1))
        );
        let expected: Vec<_> = captured
            .samples
            .iter()
            .skip(channel)
            .step_by(6)
            .copied()
            .collect();
        check_mono_wav(file, &expected);
    }

    command(handle.set_enabled(ADDON_ID, false));
    wait_for(
        || {
            handle.snapshot().addon(ADDON_ID).is_some_and(|addon| {
                addon.state == AddonState::Stopped && addon.pid.is_none() && !addon.enabled
            })
        },
        "disabled add-on stopped",
    );
    assert_eq!(
        unsafe { WaitForSingleObject(process.0, 5000) },
        WAIT_OBJECT_0,
        "disabled add-on process still exists"
    );
    wait_for(
        || {
            controller.poll();
            controller.source_status(0) == Some("Add-on is stopped")
        },
        "stopped source status",
    );
    assert!(
        controller.error().is_none(),
        "stopped add-on should not put capture in an error loop"
    );
    assert!(
        !root
            .join("senders-v1")
            .join(format!("{sender}.sender"))
            .exists()
    );
    drop(controller);

    // Updating a manually started add-on preserves that live state even though
    // automatic startup is disabled. An install failure also restarts the old version.
    command(handle.start(ADDON_ID));
    wait_running(&handle, "5.1");
    let before_update = handle.snapshot().addon(ADDON_ID).unwrap().pid.unwrap();
    command(handle.install(package.clone(), reviewed.identity));
    wait_for(
        || {
            handle.snapshot().addon(ADDON_ID).is_some_and(|addon| {
                addon.state == AddonState::Running
                    && addon.pid.is_some_and(|pid| pid != before_update)
            })
        },
        "updated manually started add-on restarted",
    );
    wait_running(&handle, "5.1");
    assert!(!handle.snapshot().addon(ADDON_ID).unwrap().enabled);
    let before_failure = handle.snapshot().addon(ADDON_ID).unwrap().pid.unwrap();
    let lock_path = root.join("addons").join(format!(".{ADDON_ID}.lock"));
    fs::write(&lock_path, b"isolated test owns this install lock").unwrap();
    let failed_update = handle
        .install(package, reviewed.identity)
        .recv_timeout(Duration::from_secs(8))
        .unwrap();
    fs::remove_file(lock_path).unwrap();
    assert!(
        failed_update.is_err(),
        "install lock did not reject the update"
    );
    wait_for(
        || {
            handle.snapshot().addon(ADDON_ID).is_some_and(|addon| {
                addon.state == AddonState::Running
                    && addon.pid.is_some_and(|pid| pid != before_failure)
            })
        },
        "old add-on restarted after failed update",
    );
    wait_running(&handle, "5.1");
    drop(manager);

    let restored = AddonManager::new(root.clone()).unwrap();
    let restored_handle = restored.handle();
    wait_for(
        || {
            restored_handle
                .snapshot()
                .addon(ADDON_ID)
                .is_some_and(|addon| {
                    !addon.enabled && addon.state == AddonState::Stopped && addon.pid.is_none()
                })
        },
        "disabled preference restored after restarting the manager",
    );
    command(restored_handle.set_enabled(ADDON_ID, true));
    wait_running(&restored_handle, "5.1");
    assert!(
        restored_handle
            .snapshot()
            .addon(ADDON_ID)
            .unwrap()
            .settings
            .iter()
            .any(|setting| setting.declaration.key == "level"
                && setting.value == serde_json::json!("-6")),
        "changed setting was not restored"
    );
    // Dropping an enabled manager stops its process but leaves the preference enabled.
    drop(restored);
    let restarted = AddonManager::new(root).unwrap();
    let restarted_handle = restarted.handle();
    wait_running(&restarted_handle, "5.1");
    let before_toggle = restarted_handle
        .snapshot()
        .addon(ADDON_ID)
        .unwrap()
        .pid
        .unwrap();
    command(restarted_handle.set_enabled(ADDON_ID, false));
    command(restarted_handle.set_enabled(ADDON_ID, true));
    wait_for(
        || {
            restarted_handle
                .snapshot()
                .addon(ADDON_ID)
                .is_some_and(|addon| {
                    addon.enabled
                        && addon.state == AddonState::Running
                        && addon.pid.is_some_and(|pid| pid != before_toggle)
                })
        },
        "rapid disable and enable restarts the add-on",
    );
    wait_running(&restarted_handle, "5.1");
    command(restarted_handle.set_enabled(ADDON_ID, false));
    wait_for(
        || {
            restarted_handle
                .snapshot()
                .addon(ADDON_ID)
                .is_some_and(|addon| addon.pid.is_none() && addon.state == AddonState::Stopped)
        },
        "restored add-on stopped",
    );
    drop(restarted);
    eprintln!("desktop add-on end-to-end checks passed");
}

fn check_mono_wav(path: &Path, expected: &[f32]) {
    let wav = fs::read(path).unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    let mut offset = 12;
    let mut format = false;
    let mut audio = false;
    while offset + 8 <= wav.len() {
        let id = &wav[offset..offset + 4];
        let len = u32::from_le_bytes(wav[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let bytes = &wav[offset + 8..offset + 8 + len];
        if id == b"fmt " {
            assert_eq!(u16::from_le_bytes(bytes[2..4].try_into().unwrap()), 1);
            assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 48_000);
            format = true;
        } else if id == b"data" {
            assert_eq!(bytes.len(), expected.len() * 4);
            let actual: Vec<_> = bytes
                .chunks_exact(4)
                .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
                .collect();
            assert_eq!(
                actual, expected,
                "exported mono file does not match its labelled channel"
            );
            audio = true;
        }
        offset += 8 + len + len % 2;
    }
    assert!(format && audio, "WAV is missing format or audio data");
}

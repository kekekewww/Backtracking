//! Prepares the separate data folder a test copy of the app runs in, and feeds it audio that
//! needs no sound device. Both are ignored tests, run by hand with `LOCALAPPDATA` pointing at
//! that folder (the test refuses the user's own folder):
//!
//! - `seed_test_profile` installs and enables the test tone add-on from
//!   `crates/backtracking-addon/target/test-tone.btaddon`, chooses its format and writes the
//!   capture sources and a few settings.
//! - `serve_stereo_sender` publishes a two-channel tone as a Sender for a while, so lane B can
//!   capture something too.
//!
//! Settings, all optional:
//! - `BACKTRACKING_TEST_FORMAT`: `stereo`, `5.1` or `7.1.4` (default) for the test tone.
//! - `BACKTRACKING_TEST_LANE_B`: `off` (default), `sender` (the stereo Sender) or `tone`.
//! - `BACKTRACKING_TEST_PREFERENCES`: `name=value` pairs separated by `;`, named like the files
//!   in `Settings`, for example `split-waveform=1;history-seconds=600;meter-unit=LUFS`.
//! - `BACKTRACKING_TEST_SENDER_SECONDS`: how long the Sender runs (default 40).

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use backtracking_core::{ChannelRoute, SenderEndpoint, capture_clock_ns};

use crate::addons::{AddonManager, AddonState};
use crate::controller::{CaptureSource, SourceSettings};
use crate::preferences::{Key, Preferences, serialize_sources};

const ADDON_ID: &str = "com.example.test-tone";
/// Sender ids are hexadecimal digits and dashes.
const STEREO_SENDER_ID: &str = "c1c1-5e7d-0002";
const STEREO_SENDER_NAME: &str = "Stereo test tone";

/// The folder in `LOCALAPPDATA`, after checking that it is not the user's own.
fn test_folder() -> PathBuf {
    let folder = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").expect("run with LOCALAPPDATA set to a test folder"),
    );
    fs::create_dir_all(&folder).unwrap();
    crate::startup::check_test_data_folder().expect("LOCALAPPDATA must be a separate test folder");
    dunce::canonicalize(folder).unwrap()
}

fn wait_for(mut condition: impl FnMut() -> bool, reason: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {reason}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn preference_key(name: &str) -> Option<Key> {
    Some(match name {
        "history-seconds" => Key::History,
        "split-waveform" => Key::SplitWaveform,
        "waveform-only" => Key::WaveformOnly,
        "meter-unit" => Key::Meter,
        "meter-b-unit" => Key::MeterB,
        "transparency-percent" => Key::Transparency,
        "show-level-meters" => Key::ShowMeters,
        "always-on-top" => Key::AlwaysOnTop,
        "wide-lane-export-v1" => Key::WideExport,
        "show-second-lines" => Key::SecondsLines,
        _ => return None,
    })
}

#[test]
#[ignore = "prepares a test copy's data folder; run by hand with LOCALAPPDATA set to it"]
fn seed_test_profile() {
    let folder = test_folder();
    let root = folder.join("Backtracking");
    let format = std::env::var("BACKTRACKING_TEST_FORMAT").unwrap_or_else(|_| "7.1.4".into());
    let package = std::env::var_os("BACKTRACKING_TEST_ADDON_PACKAGE").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/backtracking-addon/target/test-tone.btaddon")
        },
        PathBuf::from,
    );
    let manager = AddonManager::new(root.clone()).unwrap();
    let handle = manager.handle();
    fn reply<T>(reply: std::sync::mpsc::Receiver<Result<T, String>>) -> T {
        reply
            .recv_timeout(Duration::from_secs(15))
            .expect("add-on manager answers")
            .unwrap()
    }
    reply(handle.install(package));
    reply(handle.set_enabled(ADDON_ID, true));
    let running = |format: Option<&str>| {
        handle.snapshot().addon(ADDON_ID).is_some_and(|addon| {
            addon.state == AddonState::Running
                && addon
                    .sources
                    .iter()
                    .any(|source| format.is_none_or(|format| source.format == format))
        })
    };
    wait_for(|| running(None), "the test tone runs");
    reply(handle.select_format(ADDON_ID, "main", &format));
    wait_for(
        || running(Some(&format)),
        "the test tone uses the chosen format",
    );

    let tone = SourceSettings {
        source: CaptureSource::Addon {
            addon_id: ADDON_ID.into(),
            source_id: "main".into(),
            name: "Test tone".into(),
        },
        route: ChannelRoute::Merged,
    };
    let lane_b = match std::env::var("BACKTRACKING_TEST_LANE_B").as_deref() {
        Ok("sender") => Some(SourceSettings {
            source: CaptureSource::Sender {
                id: STEREO_SENDER_ID.into(),
                name: STEREO_SENDER_NAME.into(),
            },
            route: ChannelRoute::Merged,
        }),
        Ok("tone") => Some(tone.clone()),
        Ok("off") | Err(_) => None,
        Ok(other) => panic!("BACKTRACKING_TEST_LANE_B is off, sender or tone, not {other}"),
    };
    let preferences = Preferences::in_data_root(&root);
    preferences
        .save(
            Key::CaptureSources,
            &serialize_sources(&[Some(tone), lane_b]),
        )
        .unwrap();
    // Exports stay in the test folder, and the window does not sit above the user's.
    let exports = folder.join("exports");
    preferences
        .save(Key::Output, exports.join("output").to_str().unwrap())
        .unwrap();
    preferences
        .save(Key::SavedSnapshots, exports.join("saved").to_str().unwrap())
        .unwrap();
    preferences.save(Key::AlwaysOnTop, "0").unwrap();
    for pair in std::env::var("BACKTRACKING_TEST_PREFERENCES")
        .unwrap_or_default()
        .split(';')
    {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let key =
            preference_key(name.trim()).unwrap_or_else(|| panic!("unknown preference {name}"));
        preferences.save(key, value.trim()).unwrap();
    }
    drop(manager);
    eprintln!(
        "test profile ready in {} with the test tone at {format}",
        folder.display()
    );
}

#[test]
#[ignore = "feeds a test copy a stereo Sender; run by hand with LOCALAPPDATA set to its folder"]
fn serve_stereo_sender() {
    test_folder();
    let seconds = std::env::var("BACKTRACKING_TEST_SENDER_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(40.0f64);
    let mut sender = SenderEndpoint::with_id(STEREO_SENDER_ID).unwrap();
    assert_eq!(
        sender.id(),
        STEREO_SENDER_ID,
        "an earlier Sender with this id is still registered"
    );
    const RATE: u32 = 48_000;
    const FRAMES: usize = 480;
    let started = Instant::now();
    let clock = capture_clock_ns();
    let mut frame = 0u64;
    let mut block = vec![0.0f32; FRAMES * 2];
    while started.elapsed().as_secs_f64() < seconds {
        // Windows sleeps in coarse steps, so catch up with every block that is due.
        while frame * 1_000_000 / u64::from(RATE) <= started.elapsed().as_micros() as u64 {
            for (index, pair) in block.chunks_exact_mut(2).enumerate() {
                let time = (frame + index as u64) as f64 / f64::from(RATE);
                // A slow swell tells this lane apart from the steady test tone.
                let swell = 0.35 + 0.3 * (std::f64::consts::TAU * 0.5 * time).sin();
                pair[0] = (swell * (std::f64::consts::TAU * 300.0 * time).sin()) as f32;
                pair[1] = (swell * 0.6 * (std::f64::consts::TAU * 500.0 * time).sin()) as f32;
            }
            let timestamp = clock + frame * 1_000_000_000 / u64::from(RATE);
            sender
                .publish(STEREO_SENDER_NAME, RATE, 2, timestamp, &block, &[])
                .unwrap();
            frame += FRAMES as u64;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

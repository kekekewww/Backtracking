//! Prepares the separate data folder a test copy of the app runs in, and feeds it audio that
//! needs no sound device. Both are ignored tests, run by hand with `LOCALAPPDATA` pointing at
//! that folder (the test refuses the user's own folder):
//!
//! - `seed_test_profile` installs and enables the test tone add-on from
//!   `crates/backtracking-addon/target/test-tone.btaddon`, chooses its format and writes the
//!   capture sources and a few settings.
//! - `serve_stereo_sender` publishes a two-channel tone as a Sender for a while, so lane B can
//!   capture something too.
//! - `serve_wide_sender` publishes 12 steady tones, with or without 7.1.4 speaker labels.
//!
//! Settings, all optional:
//! - `BACKTRACKING_TEST_SYSTEM_FORMAT`: select System with All channels in lane A. A test
//!   instance accepts `5.1`, `7.1` or `7.1.4` followed by `:all` or `:stereo`.
//! - `BACKTRACKING_TEST_FORMAT`: `stereo`, `5.1` or `7.1.4` (default) for the test tone.
//! - `BACKTRACKING_TEST_LANE_B`: `off` (default), `sender` (the stereo Sender), `sender-7.1.4`
//!   (labelled), `sender-7.1.4-unlabelled`, `tone` or `atmos`. For either 7.1.4 value, run
//!   `serve_wide_sender` in the test profile alongside the test copy.
//! - `BACKTRACKING_TEST_LANE_B_ROUTE`: `all` stores All channels as lane B's route, as a choice
//!   kept from a wider device would be.
//! - `BACKTRACKING_TEST_PREFERENCES`: `name=value` pairs separated by `;`, named like the files
//!   in `Settings`, for example `split-waveform=1;history-seconds=600;meter-unit=LUFS`.
//! - `BACKTRACKING_TEST_SENDER_SECONDS`: how long the Sender runs (default 40).
//! - `BACKTRACKING_TEST_ADDONS`: which add-ons are installed, separated by `,`: `tone` (the
//!   default), `quits` (a package whose program exits at once, so it fails), `broken` (an
//!   installed folder whose `addon.json` cannot be read), or `none`. Add-ons from an earlier
//!   seed are removed first. Two more: `tone-opens` installs the test tone, under the same id,
//!   with the `open-files` capability for `.ec3` files (it ignores the file, but it makes a
//!   second add-on that opens them), and `atmos` installs the Dolby Atmos add-on from
//!   `BACKTRACKING_TEST_ATMOS_PACKAGE` (the path of a built `backtracking-joc.btaddon`), switches its
//!   `play_through_speakers` setting off as soon as it runs (and `capture_objects` on when `BACKTRACKING_TEST_ATMOS_OBJECTS=1`) and waits until the host holds
//!   that value. Nothing may make sound, so no file is ever opened before that.
//!   `theme` installs the example appearance add-on (`com.example.theme`) from
//!   `BACKTRACKING_TEST_THEME_PACKAGE`, or else packs
//!   `crates/backtracking-addon/examples/theme-package` with the `theme_example` example built
//!   in that crate (`cargo build --locked --release --example theme_example`) into
//!   `<LOCALAPPDATA>\packages\theme.btaddon`, and waits until it offers its look. Choose the look with `addon-look=com.example.theme` in
//!   `BACKTRACKING_TEST_PREFERENCES`, or with the harness's `look=` step.
//! - `BACKTRACKING_TEST_LANE_A`: `tone` (default when the test tone is installed), `atmos`
//!   (default when only the Atmos add-on is) or `off`. Lane B takes `atmos` as well. To see a
//!   missing source: `stopped` (the add-on from `quits`, which has stopped), `gone` (the test
//!   tone no longer offers it) or `uninstalled` (no such add-on).
//!
//! Seeding also writes packages to drop on the window into `<LOCALAPPDATA>\packages`:
//! `tone-0.2.0.btaddon` (the test tone with a newer version), `quits.btaddon`,
//! `not-a-package.btaddon` (not a ZIP file) and `not-audio.ec3` (text, for the Atmos add-on).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use backtracking_addon::package;
use backtracking_core::{ChannelRoute, SenderEndpoint, capture_clock_ns};

use crate::addons::{AddonManager, AddonState};
use crate::controller::{CaptureSource, SourceSettings};
use crate::preferences::{Key, Preferences, serialize_sources};

const ADDON_ID: &str = "com.example.test-tone";
const QUITS_ID: &str = "com.example.quits";
const BROKEN_ID: &str = "com.example.broken";
const ATMOS_ID: &str = "com.kamelkyp.backtracking.joc";
const ATMOS_SETTING: &str = "play_through_speakers";
const THEME_ID: &str = "com.example.theme";
/// Sender ids are hexadecimal digits and dashes.
const STEREO_SENDER_ID: &str = "c1c1-5e7d-0002";
const STEREO_SENDER_NAME: &str = "Stereo test tone";
const WIDE_SENDER_ID: &str = "c3c1-5e7d-0012";
const WIDE_SENDER_NAME: &str = "7.1.4 test tone";
const WIDE_SENDER_LABELS: [&str; 12] = [
    "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "TFL", "TFR", "TBL", "TBR",
];

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
        "keep-quiet-sections" => Key::RetainSilence,
        "hide-silent-channels" => Key::HideSilentChannels,
        "multichannel-pair-render" => Key::PairRender,
        "hotkeys-v1" => Key::Hotkeys,
        "addon-look" => Key::AddonLook,
        _ => return None,
    })
}

/// Writes a copy of the unpacked test tone with other manifest fields, packed as `output`.
fn repack(unpacked: &Path, work: &Path, changes: &[(&str, &str)], output: &Path) {
    repack_manifest(
        unpacked,
        work,
        |manifest| {
            for (field, value) in changes {
                manifest[*field] = serde_json::Value::from(*value);
            }
        },
        output,
    );
}

/// The same, with any change to the manifest.
fn repack_manifest(
    unpacked: &Path,
    work: &Path,
    change: impl FnOnce(&mut serde_json::Value),
    output: &Path,
) {
    let copy = work.join(output.file_stem().unwrap());
    if copy.exists() {
        fs::remove_dir_all(&copy).unwrap();
    }
    fs::create_dir_all(copy.join("bin")).unwrap();
    fs::copy(
        unpacked.join("bin/test-tone.exe"),
        copy.join("bin/test-tone.exe"),
    )
    .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(unpacked.join("addon.json")).unwrap()).unwrap();
    change(&mut manifest);
    fs::write(copy.join("addon.json"), manifest.to_string()).unwrap();
    if output.exists() {
        fs::remove_file(output).unwrap();
    }
    package::pack(&copy, output).unwrap();
    fs::remove_dir_all(&copy).unwrap();
}

/// The example appearance add-on as a package: `BACKTRACKING_TEST_THEME_PACKAGE`, or one
/// packed from its package folder and the built example program into `output`.
fn theme_package(work: &Path, output: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("BACKTRACKING_TEST_THEME_PACKAGE") {
        return PathBuf::from(path);
    }
    let addon = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/backtracking-addon");
    let program = ["release", "debug"]
        .into_iter()
        .map(|profile| addon.join("target").join(profile).join("examples/theme_example.exe"))
        .find(|program| program.is_file())
        .unwrap_or_else(|| {
            panic!(
                "build the example first: cargo build --locked --release --manifest-path \
                 crates/backtracking-addon/Cargo.toml --example theme_example"
            )
        });
    let folder = work.join("theme-package");
    if folder.exists() {
        fs::remove_dir_all(&folder).unwrap();
    }
    fs::create_dir_all(folder.join("bin")).unwrap();
    fs::copy(
        addon.join("examples/theme-package/addon.json"),
        folder.join("addon.json"),
    )
    .unwrap();
    fs::copy(program, folder.join("bin/theme-example.exe")).unwrap();
    if output.exists() {
        fs::remove_file(output).unwrap();
    }
    package::pack(&folder, output).unwrap();
    fs::remove_dir_all(&folder).unwrap();
    output.to_path_buf()
}

#[test]
#[ignore = "prepares a test copy's data folder; run by hand with LOCALAPPDATA set to it"]
fn seed_test_profile() {
    let folder = test_folder();
    let root = folder.join("Backtracking");
    let format = std::env::var("BACKTRACKING_TEST_FORMAT").unwrap_or_else(|_| "7.1.4".into());
    let tone_package = std::env::var_os("BACKTRACKING_TEST_ADDON_PACKAGE").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/backtracking-addon/target/test-tone.btaddon")
        },
        PathBuf::from,
    );
    let wanted = std::env::var("BACKTRACKING_TEST_ADDONS").unwrap_or_else(|_| "tone".into());
    let wanted: Vec<&str> = wanted
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    for name in &wanted {
        assert!(
            ["tone", "tone-opens", "quits", "broken", "atmos", "theme", "none"].contains(name),
            "BACKTRACKING_TEST_ADDONS takes tone, tone-opens, quits, broken, atmos, theme or none, \
             not {name}"
        );
    }
    let tone_installed = wanted.contains(&"tone") || wanted.contains(&"tone-opens");

    // Packages to drop on the window, made from the test tone's own program.
    let packages = folder.join("packages");
    let work = packages.join("work");
    fs::create_dir_all(&work).unwrap();
    let unpacked = package::install(&tone_package, &work)
        .map(|info| work.join(info.manifest.id))
        .unwrap();
    repack(
        &unpacked,
        &work,
        &[("version", "0.2.0")],
        &packages.join("tone-0.2.0.btaddon"),
    );
    repack(
        &unpacked,
        &work,
        &[
            ("id", QUITS_ID),
            ("name", "Quits at once"),
            (
                "description",
                "A test package whose program stops as soon as it starts.",
            ),
        ],
        &packages.join("quits.btaddon"),
    );
    repack_manifest(
        &unpacked,
        &work,
        |manifest| {
            manifest["name"] = "Test tone with files".into();
            manifest["capabilities"]
                .as_array_mut()
                .unwrap()
                .push("open-files".into());
            manifest["open_files"] =
                serde_json::json!({"description": "E-AC-3 audio", "extensions": ["ec3"]});
        },
        &packages.join("tone-opens.btaddon"),
    );
    let theme = wanted
        .contains(&"theme")
        .then(|| theme_package(&work, &packages.join("theme.btaddon")));
    // A file the Atmos add-on cannot decode, to see an add-on's error message.
    fs::write(
        packages.join("not-audio.ec3"),
        "This is not an E-AC-3 stream.",
    )
    .unwrap();
    fs::write(
        packages.join("not-a-package.btaddon"),
        "This is not a ZIP file.",
    )
    .unwrap();
    fs::remove_dir_all(&work).unwrap();

    // Only the add-ons asked for this time.
    for old in ["addons", "addon-data"] {
        let path = root.join(old);
        if path.exists() {
            fs::remove_dir_all(path).unwrap();
        }
    }
    let state = root.join("addon-host-v1.json");
    if state.exists() {
        fs::remove_file(state).unwrap();
    }
    if wanted.contains(&"broken") {
        let broken = root.join("addons").join(BROKEN_ID);
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join("addon.json"), "{ \"contract\": 1, \"id\": ").unwrap();
    }

    let manager = AddonManager::new(root.clone()).unwrap();
    let handle = manager.handle();
    fn reply<T>(reply: std::sync::mpsc::Receiver<Result<T, String>>) -> T {
        reply
            .recv_timeout(Duration::from_secs(15))
            .expect("add-on manager answers")
            .unwrap_or_else(|error| panic!("the add-on manager refused: {error}"))
    }
    if tone_installed {
        let path = if wanted.contains(&"tone-opens") {
            packages.join("tone-opens.btaddon")
        } else {
            tone_package
        };
        let inspected = reply(handle.inspect(path.clone()));
        reply(handle.install(path, inspected.identity));
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
    }
    if wanted.contains(&"atmos") {
        let atmos_package = PathBuf::from(
            std::env::var_os("BACKTRACKING_TEST_ATMOS_PACKAGE")
                .expect("set BACKTRACKING_TEST_ATMOS_PACKAGE to the Atmos add-on package"),
        );
        let inspected = reply(handle.inspect(atmos_package.clone()));
        reply(handle.install(atmos_package, inspected.identity));
        reply(handle.set_enabled(ATMOS_ID, true));
        wait_for(
            || {
                handle
                    .snapshot()
                    .addon(ATMOS_ID)
                    .is_some_and(|addon| addon.state == AddonState::Running)
            },
            "the Atmos add-on runs",
        );
        // Before anything else can happen: it plays through the speakers unless told not to.
        // The add-on is Running before it has described its settings; ask again until they exist.
        let mut settings = vec![(ATMOS_SETTING, false)];
        if std::env::var("BACKTRACKING_TEST_ATMOS_OBJECTS").is_ok_and(|value| value == "1") {
            settings.push(("capture_objects", true));
        }
        for (key, value) in settings {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let answer = handle
                    .set_setting(ATMOS_ID, key, serde_json::json!(value))
                    .recv_timeout(Duration::from_secs(15))
                    .expect("add-on manager answers");
                match answer {
                    Ok(()) => break,
                    Err(error) => {
                        assert!(Instant::now() < deadline, "{key} refused: {error}");
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            wait_for(
                || {
                    handle.snapshot().addon(ATMOS_ID).is_some_and(|addon| {
                        addon.settings.iter().any(|setting| {
                            setting.declaration.key == key
                                && setting.value == serde_json::json!(value)
                        })
                    })
                },
                "the Atmos add-on holds the setting",
            );
        }
    }
    if let Some(path) = theme {
        let inspected = reply(handle.inspect(path.clone()));
        reply(handle.install(path, inspected.identity));
        reply(handle.set_enabled(THEME_ID, true));
        wait_for(
            || {
                handle
                    .try_appearances()
                    .is_some_and(|offers| offers.contains_key(THEME_ID))
            },
            "the example appearance add-on offers its look",
        );
    }
    if wanted.contains(&"quits") {
        // Its program says hello as the test tone, is refused and exits with an error.
        let path = packages.join("quits.btaddon");
        let inspected = reply(handle.inspect(path.clone()));
        reply(handle.install(path, inspected.identity));
        reply(handle.set_enabled(QUITS_ID, true));
    }

    let tone = SourceSettings {
        source: CaptureSource::Addon {
            addon_id: ADDON_ID.into(),
            source_id: "main".into(),
            name: "Test tone".into(),
        },
        route: ChannelRoute::Merged,
    };
    let atmos = SourceSettings {
        source: CaptureSource::Addon {
            addon_id: ATMOS_ID.into(),
            source_id: "main".into(),
            name: "Dolby Atmos file".into(),
        },
        route: ChannelRoute::Merged,
    };
    // A lane whose add-on is stopped, whose source the add-on no longer offers, and whose
    // add-on is not installed: the three ways a saved add-on source can be missing.
    let missing = |addon_id: &str, source_id: &str, name: &str| SourceSettings {
        source: CaptureSource::Addon {
            addon_id: addon_id.into(),
            source_id: source_id.into(),
            name: name.into(),
        },
        route: ChannelRoute::Merged,
    };
    let lane_a = match std::env::var("BACKTRACKING_TEST_LANE_A").as_deref() {
        _ if std::env::var_os("BACKTRACKING_TEST_SYSTEM_FORMAT").is_some() => {
            Some(SourceSettings {
                source: CaptureSource::System,
                route: ChannelRoute::AllChannels,
            })
        }
        Ok("tone") => Some(tone.clone()),
        Ok("atmos") => Some(atmos.clone()),
        Ok("off") => None,
        Ok("stopped") => Some(missing(QUITS_ID, "main", "Quits at once stream")),
        Ok("gone") => Some(missing(ADDON_ID, "old-stream", "Old stream")),
        Ok("uninstalled") => Some(missing("com.example.removed", "main", "Removed stream")),
        Ok(other) => panic!(
            "BACKTRACKING_TEST_LANE_A is tone, atmos, stopped, gone, uninstalled or off, not {other}"
        ),
        Err(_) if tone_installed => Some(tone.clone()),
        Err(_) if wanted.contains(&"atmos") => Some(atmos.clone()),
        Err(_) => Some(tone.clone()),
    };
    let lane_b = match std::env::var("BACKTRACKING_TEST_LANE_B").as_deref() {
        Ok("sender") => Some(SourceSettings {
            source: CaptureSource::Sender {
                id: STEREO_SENDER_ID.into(),
                name: STEREO_SENDER_NAME.into(),
            },
            route: ChannelRoute::Merged,
        }),
        Ok("sender-7.1.4" | "sender-7.1.4-unlabelled") => Some(SourceSettings {
            source: CaptureSource::Sender {
                id: WIDE_SENDER_ID.into(),
                name: WIDE_SENDER_NAME.into(),
            },
            route: ChannelRoute::Merged,
        }),
        Ok("tone") => Some(tone.clone()),
        Ok("atmos") => Some(atmos.clone()),
        Ok("off") | Err(_) => None,
        Ok(other) => panic!(
            "BACKTRACKING_TEST_LANE_B is off, sender, sender-7.1.4, sender-7.1.4-unlabelled, tone or atmos, not {other}"
        ),
    };
    // A stored All channels choice on a source of two channels, which falls back to Merged.
    let lane_b = lane_b.map(
        |settings| match std::env::var("BACKTRACKING_TEST_LANE_B_ROUTE") {
            Ok(route) if route == "all" => SourceSettings {
                route: ChannelRoute::AllChannels,
                ..settings
            },
            _ => settings,
        },
    );
    let preferences = Preferences::in_data_root(&root);
    preferences
        .save(Key::CaptureSources, &serialize_sources(&[lane_a, lane_b]))
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
        "test profile ready in {} with add-ons {wanted:?} (test tone at {format}); packages in {}",
        folder.display(),
        packages.display()
    );
}

#[test]
#[ignore = "feeds a test copy a stereo Sender; run by hand with LOCALAPPDATA set to its folder"]
fn serve_stereo_sender() {
    serve_sender(
        STEREO_SENDER_ID,
        STEREO_SENDER_NAME,
        2,
        &[],
        |time, pair| {
            // A slow swell tells this lane apart from the steady test tone.
            let swell = 0.35 + 0.3 * (std::f64::consts::TAU * 0.5 * time).sin();
            pair[0] = (swell * (std::f64::consts::TAU * 300.0 * time).sin()) as f32;
            pair[1] = (swell * 0.6 * (std::f64::consts::TAU * 500.0 * time).sin()) as f32;
        },
    );
}

#[test]
#[ignore = "feeds a test copy a wide Sender; run by hand with LOCALAPPDATA set to its folder"]
fn serve_wide_sender() {
    let labels = match std::env::var("BACKTRACKING_TEST_LANE_B").as_deref() {
        Ok("sender-7.1.4") => WIDE_SENDER_LABELS.as_slice(),
        Ok("sender-7.1.4-unlabelled") => &[],
        _ => panic!("choose sender-7.1.4 or sender-7.1.4-unlabelled for lane B"),
    };
    serve_sender(
        WIDE_SENDER_ID,
        WIDE_SENDER_NAME,
        12,
        labels,
        |time, frame| {
            for (channel, sample) in frame.iter_mut().enumerate() {
                let frequency = 181.0 + 37.0 * channel as f64;
                // Start between zero crossings, including at the 48 kHz sample positions.
                *sample =
                    (0.25 * (std::f64::consts::TAU * (frequency * time + 0.12345)).sin()) as f32;
            }
        },
    );
}

fn serve_sender(
    id: &str,
    name: &str,
    channels: usize,
    labels: &[&str],
    fill_frame: impl Fn(f64, &mut [f32]),
) {
    test_folder();
    let seconds = std::env::var("BACKTRACKING_TEST_SENDER_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(40.0f64);
    let mut sender = SenderEndpoint::with_id(id).unwrap();
    assert_eq!(
        sender.id(),
        id,
        "an earlier Sender with this id is still registered"
    );
    const RATE: u32 = 48_000;
    const FRAMES: usize = 480;
    if !labels.is_empty() {
        sender
            .update_with_labels(name, true, RATE, channels, labels)
            .unwrap();
    }
    let started = Instant::now();
    let clock = capture_clock_ns();
    let mut frame = 0u64;
    let mut block = vec![0.0f32; FRAMES * channels];
    while started.elapsed().as_secs_f64() < seconds {
        // Windows sleeps in coarse steps, so catch up with every block that is due.
        while frame * 1_000_000 / u64::from(RATE) <= started.elapsed().as_micros() as u64 {
            for (index, samples) in block.chunks_exact_mut(channels).enumerate() {
                let time = (frame + index as u64) as f64 / f64::from(RATE);
                fill_frame(time, samples);
            }
            let timestamp = clock + frame * 1_000_000_000 / u64::from(RATE);
            sender
                .publish(name, RATE, channels, timestamp, &block, &[])
                .unwrap();
            frame += FRAMES as u64;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

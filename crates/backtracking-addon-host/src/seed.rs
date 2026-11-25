//! Test-profile add-on state, corresponding to desktop/windows/src/ui/test_profile.rs
//! at e9a4efc0d2918bccadeecf2c7e586b84ddd5a4f4.
use super::addons::{AddonHandle, AddonManager, AddonReply};
use backtracking_addon::{manifest::current_platform, package};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
type Error = Box<dyn std::error::Error>;
fn reply<T>(r: AddonReply<T>) -> Result<T, Error> {
    Ok(r.recv_timeout(Duration::from_secs(30))??)
}
fn install(h: &AddonHandle, path: &Path) -> Result<String, Error> {
    let inspected = reply(h.inspect(path.into()))?;
    let info = reply(h.install(path.into(), inspected.identity))?;
    let id = info.manifest.id;
    reply(h.set_enabled(&id, true))?;
    Ok(id)
}
fn wait(mut condition: impl FnMut() -> bool) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        if Instant::now() >= deadline {
            return Err("Timed out waiting for the test add-on".into());
        }
        std::thread::sleep(Duration::from_millis(20))
    }
    Ok(())
}
fn example(work: &Path, kind: &str) -> Result<PathBuf, Error> {
    let (fixture, binary) = match kind {
        "tone" | "tone-opens" | "quits" => ("test-tone", "test_tone"),
        "theme" => ("theme", "theme_example"),
        "control" => ("control", "control_example"),
        _ => return Err(format!("Unknown test add-on: {kind}").into()),
    };
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../backtracking-addon");
    let stage = work.join(kind);
    fs::create_dir(&stage)?;
    let mut manifest: Value = serde_json::from_slice(&fs::read(
        sdk.join(format!("examples/{fixture}-package/addon.json")),
    )?)?;
    manifest["run"] = json!({(current_platform()? ):{"program":binary,"args":[]}});
    if kind == "tone-opens" {
        manifest["capabilities"]
            .as_array_mut()
            .unwrap()
            .push("open-files".into());
        manifest["open_files"] = json!({"description":"E-AC-3 audio","extensions":["ec3"]});
    }
    if kind == "quits" {
        manifest["id"] = "com.example.quits".into();
    }
    fs::copy(
        sdk.join("target/debug/examples").join(binary),
        stage.join(binary),
    )?;
    fs::write(stage.join("addon.json"), serde_json::to_vec(&manifest)?)?;
    let output = work.join(format!("{kind}.btaddon"));
    package::pack(stage, &output)?;
    Ok(output)
}
/// Populate a new test-only root. Never replace a pre-existing profile.
pub fn seed(root: &Path) -> Result<(), Error> {
    use std::os::unix::fs::DirBuilderExt;
    if !root.is_absolute() {
        return Err("Pass an absolute test data root".into());
    }
    fs::DirBuilder::new().mode(0o700).create(root)?;
    let work = root.join("packages");
    fs::create_dir(&work)?;
    let wanted = std::env::var("BACKTRACKING_TEST_ADDONS").unwrap_or_else(|_| "tone".into());
    if wanted.split(',').any(|s| s.trim() == "broken") {
        let broken = root.join("addons/com.example.broken");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&broken)?;
        fs::write(broken.join("addon.json"), b"{")?;
    }
    let manager = AddonManager::new(root.into(), "0.2.1")?;
    let handle = manager.handle();
    for kind in wanted
        .split(',')
        .map(str::trim)
        .filter(|s| !matches!(*s, "none" | "broken" | ""))
    {
        let key = match kind {
            "tone" => "BACKTRACKING_TEST_ADDON_PACKAGE",
            "atmos" => "BACKTRACKING_TEST_ATMOS_PACKAGE",
            "theme" => "BACKTRACKING_TEST_THEME_PACKAGE",
            _ => "",
        };
        let path = match std::env::var_os(key).filter(|p| !p.is_empty()) {
            Some(p) => p.into(),
            None => example(&work, kind)?,
        };
        let id = install(&handle, &path)?;
        if kind == "tone" || kind == "tone-opens" {
            wait(|| handle.snapshot().source(&id, "main").is_some())?;
            let format =
                std::env::var("BACKTRACKING_TEST_FORMAT").unwrap_or_else(|_| "7.1.4".into());
            reply(handle.select_format(&id, "main", &format))?;
            wait(|| {
                handle
                    .snapshot()
                    .source(&id, "main")
                    .is_some_and(|s| s.format == format)
            })?;
        }
        if kind == "atmos" {
            wait(|| {
                handle.snapshot().addon(&id).is_some_and(|a| {
                    a.settings
                        .iter()
                        .any(|s| s.declaration.key == "play_through_speakers")
                })
            })?;
            reply(handle.set_setting(&id, "play_through_speakers", false.into()))?;
            if std::env::var("BACKTRACKING_TEST_ATMOS_OBJECTS").as_deref() == Ok("1") {
                reply(handle.set_setting(&id, "capture_objects", true.into()))?;
            }
        }
    }
    for path in std::env::var("BACKTRACKING_TEST_EXTRA_PACKAGES")
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        install(&handle, Path::new(path))?;
    }
    for id in std::env::var("BACKTRACKING_TEST_ALLOW_CONTROL")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        reply(handle.set_control_allowed(id, true))?;
    }
    Ok(())
}

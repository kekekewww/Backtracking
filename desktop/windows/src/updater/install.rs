use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command};
use std::ptr::null;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenMutexW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};

use crate::update_protocol::{MAX_ARTIFACT, MAX_MANIFEST, Manifest, stable_version};

use super::{CURRENT, Download, dialog, key};

const APPLY: &str = "--backtracking-apply-update";
const BOOT: &str = "--backtracking-updated";
const RECOVERY: &str = "--backtracking-update-recovery";
const LOCK_NAME: &str = "Local\\Backtracking.Desktop.Update.7c1311ad-1d75-4b4b-846a-ff7d0ef2dc79";
const ATTACH_TIMEOUT: Duration = Duration::from_secs(4);
const BOOT_TIMEOUT: Duration = Duration::from_secs(18);
const MAX_STATE: u64 = 128 * 1024;
const REQUEST: &str = "request.json";
const HEALTHY: &str = "healthy";
const COMMITTED: &str = "committed";
const HELPER: &str = "helper.exe";
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize)]
struct Request {
    target: PathBuf,
    parent_pid: u32,
    version: String,
    notes: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct LastUpdate {
    pub(super) version: String,
    pub(super) notes: String,
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn directory() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join("Backtracking").join("Updates"))
        .ok_or_else(|| "Windows did not provide a per-user application data directory.".into())
}

pub(super) fn state<T: for<'a> Deserialize<'a>>(name: &str) -> Result<T, String> {
    let bytes = read_bounded(&directory()?.join(name), MAX_STATE)?;
    serde_json::from_slice(&bytes).map_err(display)
}

pub(super) fn last_update() -> Option<LastUpdate> {
    if let Some((false, _, request)) = boot_transaction() {
        return Some(LastUpdate {
            version: request.version,
            notes: request.notes,
        });
    }
    state("last-update.json").ok()
}

pub(super) fn save_state<T: Serialize>(name: &str, value: &T) -> Result<(), String> {
    let root = directory()?;
    fs::create_dir_all(&root).map_err(display)?;
    let data = serde_json::to_vec(value).map_err(display)?;
    let tmp = root.join(format!("state-{}-{}.tmp", process::id(), next()));
    write_new(&tmp, &data)?;
    let result = fs::rename(&tmp, root.join(name)).map_err(display);
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

fn mutex_name() -> Vec<u16> {
    wide(OsStr::new(LOCK_NAME))
}

fn next() -> u64 {
    SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(display)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(display)?;
    if bytes.len() as u64 > limit {
        return Err("The update file exceeds its size limit.".into());
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(display)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(display)
}

fn target_path() -> Result<PathBuf, String> {
    std::env::current_exe()
        .and_then(dunce::canonicalize)
        .map_err(display)
}

fn transaction(path: &Path) -> Result<PathBuf, String> {
    let root = dunce::canonicalize(directory()?).map_err(display)?;
    let dir = dunce::canonicalize(path).map_err(display)?;
    if dir.parent() == Some(root.as_path())
        && dir
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with("txn-"))
    {
        return Ok(dir);
    }
    Err("The update transaction directory is invalid.".into())
}

fn request(dir: &Path) -> Result<Request, String> {
    let bytes = read_bounded(&dir.join(REQUEST), MAX_STATE)?;
    serde_json::from_slice(&bytes).map_err(display)
}

fn lock(allow_existing: bool) -> Result<Handle, String> {
    let raw = unsafe { CreateMutexW(null(), 0, mutex_name().as_ptr()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error().to_string());
    }
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let handle = Handle(raw);
    if !allow_existing && already_exists {
        return Err("Another Backtracking update is already running.".into());
    }
    Ok(handle)
}

fn active() -> bool {
    let raw = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, mutex_name().as_ptr()) };
    if raw.is_null() {
        return false;
    }
    drop(Handle(raw));
    true
}

fn launch_argument() -> Option<(std::ffi::OsString, PathBuf)> {
    let mut args = std::env::args_os().skip(1);
    let mode = args.next()?;
    let dir = args.next()?.into();
    if args.next().is_some() {
        return None;
    }
    Some((mode, dir))
}

fn boot_transaction() -> Option<(bool, PathBuf, Request)> {
    let (mode, dir) = launch_argument()?;
    if mode != BOOT && mode != RECOVERY {
        return None;
    }
    let dir = transaction(&dir).ok()?;
    let request = request(&dir).ok()?;
    if request.target != target_path().ok()? || (mode == BOOT && request.version != CURRENT) {
        return None;
    }
    Some((mode == RECOVERY, dir, request))
}

pub(super) fn suppress_check() -> bool {
    boot_transaction().is_some()
}

pub(super) fn other_launch_during_update() -> bool {
    active() && !suppress_check()
}

pub(super) fn helper_entry() -> Result<bool, String> {
    let Some((mode, dir)) = launch_argument() else {
        return Ok(false);
    };
    if mode != APPLY {
        return Ok(false);
    }
    let dir = transaction(&dir)?;
    if target_path()? != dir.join(HELPER) {
        return Err("The update helper was not started from its transaction directory.".into());
    }
    apply(&dir)?;
    Ok(true)
}

pub(super) fn schedule(download: Download) -> Result<(), String> {
    let _lock = lock(false)?;
    let target = target_path()?;
    let root = directory()?;
    fs::create_dir_all(&root).map_err(display)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let dir = root.join(format!(
        "txn-{}-{}-{}",
        process::id(),
        now.as_nanos(),
        next()
    ));
    fs::create_dir(&dir).map_err(display)?;
    let Download { update, bytes } = download;
    let result = (|| {
        write_new(&dir.join("download.exe"), &bytes)?;
        write_new(&dir.join("manifest.json"), &update.manifest)?;
        write_new(&dir.join("manifest.sig"), &update.signature)?;
        let request = Request {
            target: target.clone(),
            parent_pid: process::id(),
            version: update.version.to_string(),
            notes: update.notes,
        };
        write_new(
            &dir.join(REQUEST),
            &serde_json::to_vec(&request).map_err(display)?,
        )?;
        fs::copy(target, dir.join(HELPER)).map_err(display)?;

        let mut helper = Command::new(dir.join(HELPER))
            .arg(APPLY)
            .arg(&dir)
            .spawn()
            .map_err(display)?;
        if wait_for(&mut helper, &dir.join("attached"), None, ATTACH_TIMEOUT)? {
            return Ok(());
        }
        let _ = helper.kill();
        let _ = helper.wait();
        Err("The Backtracking update helper did not start.".into())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    result
}

fn wait_for(
    child: &mut Child,
    signal: &Path,
    expected: Option<&str>,
    timeout: Duration,
) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child.try_wait().map_err(display)?.is_some() {
            return Ok(false);
        }
        if let Ok(value) = fs::read_to_string(signal)
            && expected.is_none_or(|value_required| value == value_required)
        {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(35));
    }
    Ok(false)
}

fn apply(dir: &Path) -> Result<(), String> {
    // The original process owns the first handle until this helper has attached.
    let _lock = lock(true)?;
    let request = request(dir)?;
    let version = stable_version(&request.version).ok_or("The staged version is invalid.")?;
    if version <= stable_version(CURRENT).ok_or("The installed version is invalid.")? {
        return Err("The staged update would not upgrade Backtracking.".into());
    }
    let parent = parent_process(&request)?;
    let key = key().ok_or("The update helper has no trusted signing key.")?;
    let manifest = read_bounded(&dir.join("manifest.json"), MAX_MANIFEST as u64)?;
    let signature = read_bounded(&dir.join("manifest.sig"), 64)?;
    let manifest = Manifest::verify(&key, &manifest, &signature, &version)?;
    let bytes = read_bounded(&dir.join("download.exe"), manifest.size as u64)?;
    manifest.verify_artifact(&bytes)?;
    write_new(&dir.join("attached"), b"")?;
    if unsafe { WaitForSingleObject(parent.0, 60_000) } != WAIT_OBJECT_0 {
        return Err("Backtracking did not exit before the update deadline.".into());
    }

    let result = replace_and_launch(dir, &request, &bytes);
    let _ = fs::remove_file(dir.join("download.exe"));
    match result {
        Ok(()) => {
            let _ = save_state(
                "last-update.json",
                &LastUpdate {
                    version: request.version,
                    notes: request.notes,
                },
            );
        }
        Err(error) => {
            let original = read_bounded(&dir.join(HELPER), MAX_ARTIFACT as u64)?;
            if read_bounded(&request.target, MAX_ARTIFACT as u64).as_ref() != Ok(&original) {
                return Err(error);
            }
            let info = format!("已啟動原版本。\n\n{error}");
            write_new(&dir.join("recovered"), info.as_bytes())?;
            Command::new(&request.target)
                .arg(RECOVERY)
                .arg(dir)
                .spawn()
                .map_err(display)?;
        }
    }
    Ok(())
}

fn parent_process(request: &Request) -> Result<Handle, String> {
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            request.parent_pid,
        )
    };
    if raw.is_null() {
        return Err(io::Error::last_os_error().to_string());
    }
    let process = Handle(raw);
    let mut path = vec![0u16; 32_768];
    let mut len = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut len) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    use std::os::windows::ffi::OsStringExt;
    let path = std::ffi::OsString::from_wide(&path[..len as usize]);
    if dunce::canonicalize(path).map_err(display)? != request.target {
        return Err("The update target does not match the running Backtracking executable.".into());
    }
    Ok(process)
}

fn replace(target: &Path, incoming: &Path, backup: Option<&Path>) -> io::Result<()> {
    let target = wide(target.as_os_str());
    let incoming = wide(incoming.as_os_str());
    let backup = backup.map(|path| wide(path.as_os_str()));
    let ok = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            incoming.as_ptr(),
            backup.as_ref().map_or(null(), |path| path.as_ptr()),
            0,
            null(),
            null(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn rollback(target: &Path, backup: &Path) -> Result<(), String> {
    let result = if target.exists() {
        replace(target, backup, None)
    } else {
        fs::rename(backup, target)
    };
    result.map_err(|error| format!("無法還原原版本。備份位於 {}。\n{error}", backup.display()))
}

fn replace_and_launch(dir: &Path, request: &Request, bytes: &[u8]) -> Result<(), String> {
    let target = &request.target;
    let parent = target
        .parent()
        .ok_or("Backtracking has no installation directory.")?;
    let current = read_bounded(target, MAX_ARTIFACT as u64)?;
    if current != read_bounded(&dir.join(HELPER), MAX_ARTIFACT as u64)? {
        return Err("The Backtracking executable changed while the update was staged.".into());
    }
    let nonce = dir
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("The transaction name is invalid.")?;
    let incoming = parent.join(format!(".backtracking-{nonce}.new"));
    let backup = parent.join(format!(".backtracking-{nonce}.old"));
    let replacement = (|| {
        write_new(&incoming, bytes)?;
        if let Err(error) = replace(target, &incoming, Some(&backup)) {
            if backup.is_file() {
                rollback(target, &backup)?;
            }
            return Err(error.to_string());
        }
        let health = Command::new(target)
            .arg(BOOT)
            .arg(dir)
            .spawn()
            .map_err(display)
            .and_then(|mut candidate| {
                let health = wait_for(
                    &mut candidate,
                    &dir.join(HEALTHY),
                    Some(&request.version),
                    BOOT_TIMEOUT,
                );
                if !matches!(health, Ok(true)) {
                    let _ = candidate.kill();
                    let _ = candidate.wait();
                }
                health
            });
        if matches!(health, Ok(true)) {
            if let Err(error) = write_new(&dir.join(COMMITTED), b"") {
                // The new executable was already confirmed healthy.
                eprintln!("Unable to record the update notification: {error}");
            }
            let _ = fs::remove_file(&backup);
            return Ok(());
        }
        rollback(target, &backup)?;
        Err("The new Backtracking version did not report a healthy startup.".into())
    })();
    let _ = fs::remove_file(incoming);
    replacement
}

pub(super) fn app_ready() {
    let Some((recovered, dir, request)) = boot_transaction() else {
        return;
    };
    if recovered {
        let _ = thread::Builder::new()
            .name("update-notice".into())
            .spawn(move || {
                let message = fs::read_to_string(dir.join("recovered"))
                    .unwrap_or_else(|_| "已啟動原版本。".into());
                dialog::recovered(&message);
            });
        return;
    }
    if write_new(&dir.join(HEALTHY), CURRENT.as_bytes()).is_err() {
        return;
    }
    let _ = thread::Builder::new()
        .name("update-notice".into())
        .spawn(move || {
            let deadline = Instant::now() + BOOT_TIMEOUT;
            while Instant::now() < deadline {
                if dir.join(COMMITTED).is_file() {
                    dialog::completed(&request.version, &request.notes);
                    return;
                }
                thread::sleep(Duration::from_millis(70));
            }
        });
}

pub(super) fn clean_old_transactions() {
    let Ok(root) = directory() else { return };
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("txn-")
            || !entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
        {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.elapsed().ok());
        if age.is_some_and(|age| age > Duration::from_secs(86_400)) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            let path = std::env::temp_dir().join(format!(
                "backtracking-update-test-{}-{}-{}",
                process::id(),
                now.as_nanos(),
                next()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            for _ in 0..20 {
                if fs::remove_dir_all(&self.0).is_ok() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }

    #[test]
    fn native_replace_keeps_an_independent_backup_until_commit() {
        let dir = TestDirectory::new();
        let target = dir.0.join("Backtracking.exe");
        let incoming = dir.0.join("incoming");
        let backup = dir.0.join("backup");
        fs::write(&target, b"original").unwrap();
        fs::write(&incoming, b"candidate").unwrap();
        replace(&target, &incoming, Some(&backup)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"candidate");
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        rollback(&target, &backup).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(!backup.exists());
    }

    #[test]
    fn failure_to_launch_candidate_restores_original_at_same_path() {
        let dir = TestDirectory::new();
        let target = dir.0.join("Backtracking.exe");
        let transaction = dir.0.join("txn-test");
        fs::create_dir(&transaction).unwrap();
        fs::write(&target, b"original").unwrap();
        fs::write(transaction.join(HELPER), b"original").unwrap();
        let request = Request {
            target: target.clone(),
            parent_pid: process::id(),
            version: "9.9.9".into(),
            notes: "test".into(),
        };
        assert!(replace_and_launch(&transaction, &request, b"invalid executable").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(!transaction.join(COMMITTED).exists());
        assert!(!dir.0.join(".backtracking-txn-test.old").exists());
    }

    #[test]
    fn healthy_native_candidate_commits_the_replacement() {
        let dir = TestDirectory::new();
        let target = dir.0.join("Backtracking.exe");
        let transaction = dir.0.join("txn-healthy");
        fs::create_dir(&transaction).unwrap();
        fs::write(&target, b"original").unwrap();
        fs::write(transaction.join(HELPER), b"original").unwrap();
        let source = dir.0.join("fixture.rs");
        fs::write(
            &source,
            r#"
fn main() {
    let mut args = std::env::args_os().skip(1);
    assert_eq!(args.next().unwrap(), "--backtracking-updated");
    let dir = std::path::PathBuf::from(args.next().unwrap());
    std::fs::write(dir.join("healthy"), b"9.9.9").unwrap();
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if dir.join("committed").is_file() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::process::exit(1);
}
"#,
        )
        .unwrap();
        let fixture = dir.0.join("candidate.exe");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        assert!(
            Command::new(rustc)
                .arg(&source)
                .arg("-o")
                .arg(&fixture)
                .status()
                .unwrap()
                .success()
        );
        let candidate = fs::read(&fixture).unwrap();
        let request = Request {
            target: target.clone(),
            parent_pid: process::id(),
            version: "9.9.9".into(),
            notes: "test".into(),
        };
        replace_and_launch(&transaction, &request, &candidate).unwrap();
        assert_eq!(fs::read(&target).unwrap(), candidate);
        assert!(transaction.join(COMMITTED).is_file());
        assert!(!dir.0.join(".backtracking-txn-healthy.old").exists());
    }
}

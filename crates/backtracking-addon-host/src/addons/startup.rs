//! Ported from desktop/windows/src/addons/startup.rs at e9a4efc0d2918bccadeecf2c7e586b84ddd5a4f4.
//! Holds primary ownership while cleaning up interrupted package operations.

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use backtracking_addon::{manifest::validate_id, package::transaction_id};

#[derive(Clone)]
pub(super) struct PrimaryLease {
    _file: Arc<File>,
    root: PathBuf,
}

impl PrimaryLease {
    pub(super) fn acquire(root: &Path) -> Result<Self, String> {
        // Keep this file in place. Windows releases its exclusive handle after a
        // crash, and task clones keep it locked until every operation has ended.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(".addon-host.lock"))
            .map_err(|error| {
                format!("Cannot own the add-on folder; another add-on host may be running: {error}")
            })?;
        file.try_lock().map_err(|e| {
            format!("Cannot own the add-on folder; another add-on host may be running: {e}")
        })?;
        Ok(Self {
            _file: Arc::new(file),
            root: root.to_owned(),
        })
    }

    pub(super) fn cleanup(&self) -> Result<(), String> {
        let root = &self.root;
        let addons = root.join("addons");
        match fs::symlink_metadata(&addons) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("The add-ons folder is a symbolic link or reparse point".into());
            }
            Ok(_) => {
                recover_backups(&addons)?;
                cleanup_entries(&addons, stale_addon_entry)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        cleanup_entries(root, |name| {
            name.strip_prefix(".install-")
                .and_then(|tail| tail.strip_suffix(".btaddon"))
                .is_some_and(|token| {
                    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .then_some(false)
        })?;
        Ok(())
    }
}

fn stale_addon_entry(name: &str) -> Option<bool> {
    let Some(name) = name.strip_prefix('.') else {
        return None;
    };
    if let Some(id) = name.strip_suffix(".lock") {
        return validate_id(id).is_ok().then_some(false);
    }
    transaction_id(name, ".install-").map(|_| true)
}

fn recover_backups(addons: &Path) -> Result<(), String> {
    use backtracking_addon::package::validate_installed_folder;
    let mut groups = std::collections::BTreeMap::<String, Vec<PathBuf>>::new();
    for entry in fs::read_dir(addons).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_prefix('.'))
            .and_then(|n| transaction_id(n, ".backup-"))
        {
            groups.entry(id.into()).or_default().push(entry.path());
        }
    }
    for (id, backups) in groups {
        let target = addons.join(&id);
        match fs::symlink_metadata(&target) {
            Ok(_) if validate_installed_folder(&target, &id).is_ok() => {
                for backup in backups {
                    if fs::symlink_metadata(&backup)
                        .map_err(|e| e.to_string())?
                        .is_dir()
                    {
                        fs::remove_dir_all(backup).map_err(|e| e.to_string())?;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut valid = backups
                    .iter()
                    .filter(|p| validate_installed_folder(p, &id).is_ok());
                let Some(previous) = valid.next() else {
                    return Err(format!("Cannot recover {id}: no valid backup"));
                };
                if valid.next().is_some() {
                    return Err(format!("Cannot recover {id}: multiple valid backups"));
                }
                fs::rename(previous, target).map_err(|e| e.to_string())?;
            }
            Ok(_) => {} // Preserve the backup if the visible package needs repair.
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn cleanup_entries(root: &Path, matches: impl Fn(&str) -> Option<bool>) -> Result<(), String> {
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        // An unreadable listing entry is reported by package discovery separately.
        let Ok(entry) = entry else { continue };
        let Some(directory) = entry.file_name().to_str().and_then(&matches) else {
            continue;
        };
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if directory != metadata.is_dir() && !metadata.is_symlink() {
            continue;
        }
        let result = if metadata.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        result.map_err(|error| {
            format!(
                "Cannot remove interrupted add-on operation {}: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

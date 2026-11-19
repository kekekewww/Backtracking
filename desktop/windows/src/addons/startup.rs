//! Holds primary ownership while cleaning up interrupted package operations.

use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use backtracking_addon::manifest::validate_id;

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
            .share_mode(0)
            .open(root.join(".addon-host.lock"))
            .map_err(|error| {
                format!("Cannot own the add-on folder; another add-on host may be running: {error}")
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
            Ok(metadata) if metadata.file_attributes() & 0x400 != 0 => {
                return Err("The add-ons folder is a symbolic link or reparse point".into());
            }
            Ok(_) => cleanup_entries(&addons, stale_addon_entry)?,
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
    [".backup-", ".install-"]
        .iter()
        .any(|separator| {
            name.rsplit_once(separator).is_some_and(|(id, suffix)| {
                validate_id(id).is_ok()
                    && suffix.split_once('-').is_some_and(|(pid, sequence)| {
                        pid.bytes().all(|byte| byte.is_ascii_digit())
                            && sequence.bytes().all(|byte| byte.is_ascii_digit())
                            && pid.parse::<u32>().is_ok()
                            && sequence.parse::<u64>().is_ok()
                    })
            })
        })
        .then_some(true)
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
        let attributes = metadata.file_attributes();
        // symlink_metadata reports a junction as a link, not a directory.
        let is_directory = attributes & 0x10 != 0;
        if is_directory != directory {
            continue;
        }
        let result = if is_directory {
            if attributes & 0x400 != 0 {
                fs::remove_dir(&path)
            } else {
                fs::remove_dir_all(&path)
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap())
                .join(format!("cleanup-{}", super::super::random_token().unwrap()));
            fs::create_dir_all(root.join("addons")).unwrap();
            Self(root)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn primary_removes_stale_operations_but_keeps_installed_addons_and_other_files() {
        let root = Directory::new();
        let addons = root.0.join("addons");
        for name in [
            ".com.example.test.backup-100-1",
            ".com.example.test.install-100-2",
        ] {
            fs::create_dir(addons.join(name)).unwrap();
            fs::write(addons.join(name).join("old-file"), b"old").unwrap();
        }
        fs::write(addons.join(".com.example.test.lock"), b"").unwrap();
        let private_package = root.0.join(format!(".install-{}.btaddon", "a".repeat(64)));
        fs::write(&private_package, b"old").unwrap();
        let private_named_directory = root.0.join(format!(".install-{}.btaddon", "b".repeat(64)));
        fs::create_dir(&private_named_directory).unwrap();
        fs::create_dir(addons.join(".com.foo.backup-notes")).unwrap();
        fs::write(root.0.join(".install-user-download.btaddon"), b"keep").unwrap();
        fs::create_dir(addons.join("com.example.test")).unwrap();
        fs::write(addons.join("com.example.test/addon.json"), b"keep").unwrap();
        fs::write(addons.join(".notes.txt"), b"keep").unwrap();
        fs::write(root.0.join("download.btaddon"), b"keep").unwrap();

        let primary = PrimaryLease::acquire(&root.0).unwrap();
        primary.cleanup().unwrap();

        assert!(!addons.join(".com.example.test.lock").exists());
        assert!(!addons.join(".com.example.test.backup-100-1").exists());
        assert!(!addons.join(".com.example.test.install-100-2").exists());
        assert!(!private_package.exists());
        assert!(private_named_directory.is_dir());
        assert!(addons.join(".com.foo.backup-notes").is_dir());
        assert_eq!(
            fs::read(root.0.join(".install-user-download.btaddon")).unwrap(),
            b"keep"
        );
        assert_eq!(
            fs::read(addons.join("com.example.test/addon.json")).unwrap(),
            b"keep"
        );
        assert_eq!(fs::read(addons.join(".notes.txt")).unwrap(), b"keep");
        assert_eq!(fs::read(root.0.join("download.btaddon")).unwrap(), b"keep");
    }

    #[test]
    fn competing_primary_cannot_clean_while_an_install_worker_keeps_ownership() {
        let root = Directory::new();
        let primary = PrimaryLease::acquire(&root.0).unwrap();
        let install_worker = primary.clone();
        drop(primary);
        let stage = root.0.join("addons").join(".com.example.test.install-1-1");
        fs::create_dir(&stage).unwrap();
        assert!(PrimaryLease::acquire(&root.0).is_err());
        assert!(stage.exists());
        drop(install_worker);
        let restarted = PrimaryLease::acquire(&root.0).unwrap();
        restarted.cleanup().unwrap();
        assert!(!stage.exists());
    }

    fn junction(link: &Path, target: &Path) {
        let output = Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Cannot create test junction: {output:?}"
        );
    }

    #[test]
    fn stale_junction_removal_keeps_its_target() {
        let root = Directory::new();
        let target = root.0.join("keep-target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), b"keep").unwrap();
        let stage = root.0.join("addons").join(".com.example.test.install-1-1");
        junction(&stage, &target);

        PrimaryLease::acquire(&root.0).unwrap().cleanup().unwrap();

        assert!(!stage.exists());
        assert_eq!(fs::read(target.join("keep")).unwrap(), b"keep");
    }

    #[test]
    fn cleanup_does_not_follow_an_addons_folder_junction() {
        let root = Directory::new();
        let target = root.0.join("keep-target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join(".com.example.test.lock"), b"keep").unwrap();
        fs::remove_dir(root.0.join("addons")).unwrap();
        junction(&root.0.join("addons"), &target);

        assert!(PrimaryLease::acquire(&root.0).unwrap().cleanup().is_err());

        assert_eq!(
            fs::read(target.join(".com.example.test.lock")).unwrap(),
            b"keep"
        );
    }
}

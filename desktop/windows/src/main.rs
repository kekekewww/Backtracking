#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(all(target_os = "windows", test))]
mod addon_integration;
#[cfg(target_os = "windows")]
mod addons;
#[cfg(target_os = "windows")]
mod clip_export;
#[cfg(target_os = "windows")]
mod context_menu;
#[cfg(target_os = "windows")]
mod controller;
#[cfg(target_os = "windows")]
mod data_paths;
#[cfg(target_os = "windows")]
mod file_drag;
#[cfg(target_os = "windows")]
mod loopback;
#[cfg(target_os = "windows")]
mod preferences;
#[cfg(target_os = "windows")]
mod snapshot_hotkey;
#[cfg(target_os = "windows")]
mod startup;
#[cfg(target_os = "windows")]
mod tray;
#[cfg(target_os = "windows")]
mod ui;
#[cfg(any(target_os = "windows", test))]
mod update_protocol;
#[cfg(target_os = "windows")]
mod updater;
#[cfg(target_os = "windows")]
mod window_order;

#[cfg(target_os = "windows")]
fn main() {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::{HSTRING, PCWSTR, w};

    // A test copy runs beside the installed app and never touches its updates.
    let test_instance = startup::test_instance().is_some();
    let result = (|| {
        if startup::test_instance_invalid() {
            return Err(format!(
                "{} takes 1 to 40 letters, digits, - or _.",
                startup::TEST_INSTANCE_VARIABLE
            )
            .into());
        }
        if test_instance {
            startup::check_test_data_folder()?;
        } else if updater::run_helper()? || updater::other_launch_during_update() {
            return Ok(());
        }
        let primary = !std::env::args_os().any(|arg| arg == "--new-capture-window");
        let _instance = if primary {
            let Some(instance) = startup::Instance::acquire()? else {
                return Ok(());
            };
            if !test_instance && updater::install_before_start() {
                return Ok(());
            }
            Some(instance)
        } else {
            None
        };
        let data = data_paths::DataPaths::for_current_user()?;
        let addons = start_addons(data.root().to_path_buf(), primary);
        let controller = controller::Controller::with_addons(addons.handle())?;
        ui::run(controller, primary)?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    if let Err(error) = result {
        if test_instance || startup::test_instance_invalid() {
            // Nobody is there to close a message box. The harness reads standard error.
            eprintln!("Backtracking test copy could not start: {error}");
            std::process::exit(2);
        }
        let message = HSTRING::from(format!("無法開啟 Backtracking。\n\n{error}"));
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                w!("Backtracking"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(target_os = "windows")]
fn start_addons(data_root: std::path::PathBuf, primary: bool) -> addons::AddonManager {
    let manager = if primary {
        addons::AddonManager::new(data_root)
    } else {
        addons::AddonManager::connect_existing(data_root)
    };
    manager.unwrap_or_else(addons::AddonManager::unavailable)
}

#[cfg(all(target_os = "windows", test))]
mod addon_startup_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!(
                    "addon-startup-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn assert_recorder_can_start(manager: addons::AddonManager) {
        let handle = manager.handle();
        let snapshot = handle.snapshot();
        assert!(snapshot.addons.is_empty());
        let reason = snapshot.error.as_deref().expect("show the startup failure");
        assert!(reason.starts_with("Add-ons are unavailable:"));
        assert!(reason.len() > "Add-ons are unavailable:".len());
        assert!(
            handle
                .start("test.addon")
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_err()
        );
        let controller = controller::Controller::with_addons(handle).unwrap();
        assert!(controller.addons().unwrap().snapshot().error.is_some());
    }

    #[test]
    fn broken_addon_root_does_not_prevent_recorder_startup() {
        let folder = Folder::new();
        let root = folder.0.join("blocked");
        fs::write(&root, b"not a folder").unwrap();
        let manager = start_addons(root, true);
        assert_recorder_can_start(manager);
    }

    #[test]
    fn unavailable_primary_does_not_prevent_secondary_recorder_startup() {
        let folder = Folder::new();
        let manager = start_addons(folder.0.clone(), false);
        assert_recorder_can_start(manager);
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Backtracking desktop requires Windows 10 or later.");
    std::process::exit(1);
}

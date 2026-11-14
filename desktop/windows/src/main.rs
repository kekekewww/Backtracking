#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
mod context_menu;
#[cfg(target_os = "windows")]
mod controller;
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

    let result = (|| {
        if updater::run_helper()? || updater::other_launch_during_update() {
            return Ok(());
        }
        let primary = !std::env::args_os().any(|arg| arg == "--new-capture-window");
        let _instance = if primary {
            let Some(instance) = startup::Instance::acquire()? else {
                return Ok(());
            };
            if updater::install_before_start() {
                return Ok(());
            }
            Some(instance)
        } else {
            None
        };
        ui::run(controller::Controller::new()?, primary)?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    if let Err(error) = result {
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

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Backtracking desktop requires Windows 10 or later.");
    std::process::exit(1);
}

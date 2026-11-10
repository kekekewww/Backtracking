#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
mod controller;
#[cfg(target_os = "windows")]
mod loopback;
#[cfg(target_os = "windows")]
mod preferences;
#[cfg(target_os = "windows")]
mod startup;
#[cfg(target_os = "windows")]
mod tray;
#[cfg(target_os = "windows")]
mod ui;
#[cfg(target_os = "windows")]
mod window_order;

#[cfg(target_os = "windows")]
fn main() {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::{HSTRING, PCWSTR, w};

    let result = (|| {
        let Some(_instance) = startup::Instance::acquire()? else {
            return Ok(());
        };
        ui::run(controller::Controller::new())?;
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

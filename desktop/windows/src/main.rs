#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
mod controller;
#[cfg(target_os = "windows")]
mod loopback;
#[cfg(target_os = "windows")]
mod ui;

#[cfg(target_os = "windows")]
fn main() {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::{HSTRING, PCWSTR, w};

    if let Err(error) = ui::run(controller::Controller::new()) {
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

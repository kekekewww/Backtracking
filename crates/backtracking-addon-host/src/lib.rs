//! Combined archive for the macOS application (core + add-on host).
#![cfg(unix)]
mod addons;
pub use backtracking_core as core;
use serde_json::Value;
use std::ffi::{CStr, CString, c_char};
use std::path::Path;

mod commands;
mod host;
pub use host::BtAddonHost;
unsafe fn input<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

fn output(value: Value) -> *mut c_char {
    CString::new(value.to_string())
        .expect("JSON escapes NUL")
        .into_raw()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bt_addon_host_new(
    root: *const c_char,
    version: *const c_char,
    role: u32,
) -> *mut BtAddonHost {
    let (Some(root), Some(version)) = (unsafe { input(root) }, unsafe { input(version) }) else {
        return std::ptr::null_mut();
    };
    if role > 1 || !Path::new(root).is_absolute() || version.is_empty() {
        return std::ptr::null_mut();
    }
    BtAddonHost::new(root.into(), version.into(), role)
        .map_or(std::ptr::null_mut(), |h| Box::into_raw(Box::new(h)))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bt_addon_host_generation(h: *const BtAddonHost) -> u64 {
    unsafe { h.as_ref() }.map_or(0, BtAddonHost::generation)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bt_addon_host_snapshot(h: *const BtAddonHost) -> *mut c_char {
    output(unsafe { h.as_ref() }.map_or_else(
        || host::failure("invalid_request", "Missing host"),
        BtAddonHost::snapshot,
    ))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bt_addon_host_command(
    h: *const BtAddonHost,
    p: *const c_char,
) -> *mut c_char {
    let request = unsafe { input(p) }.and_then(|s| serde_json::from_str(s).ok());
    output(match (unsafe { h.as_ref() }, request) {
        (Some(h), Some(v)) => h.command(v),
        _ => host::failure("invalid_request", "Invalid host or command JSON"),
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bt_addon_string_free(p: *mut c_char) {
    if !p.is_null() {
        drop(unsafe { CString::from_raw(p) });
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bt_addon_host_shutdown(h: *mut BtAddonHost, timeout_ms: u32) -> bool {
    unsafe { h.as_ref() }
        .is_some_and(|host| host.shutdown(std::time::Duration::from_millis(u64::from(timeout_ms))))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bt_addon_host_free(h: *mut BtAddonHost) {
    if !h.is_null() {
        drop(unsafe { Box::from_raw(h) });
    }
}

pub mod seed;
pub mod test_home;

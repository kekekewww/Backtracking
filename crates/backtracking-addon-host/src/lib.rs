//! Combined archive for the macOS application (core + add-on host).
pub use backtracking_core as core;
use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char};
use std::path::Path;

pub struct BtAddonHost {
    _root: String,
    _version: String,
    _role: u32,
}

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
    Box::into_raw(Box::new(BtAddonHost {
        _root: root.into(),
        _version: version.into(),
        _role: role,
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn bt_addon_host_generation(_: *const BtAddonHost) -> u64 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn bt_addon_host_snapshot(_: *const BtAddonHost) -> *mut c_char {
    output(
        json!({"schema":1,"ready":false,"error":"Host implementation pending","generation":0,"addons":[],"appearances":{},"completed":[]}),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn bt_addon_host_command(_: *const BtAddonHost, _: *const c_char) -> *mut c_char {
    output(json!({"schema":1,"ok":false,"code":"failed","message":"Host implementation pending"}))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bt_addon_string_free(p: *mut c_char) {
    if !p.is_null() {
        drop(unsafe { CString::from_raw(p) });
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bt_addon_host_free(h: *mut BtAddonHost) {
    if !h.is_null() {
        drop(unsafe { Box::from_raw(h) });
    }
}

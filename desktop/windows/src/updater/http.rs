use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::time::Instant;

use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts,
};

struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void) -> Result<Self, String> {
        if raw.is_null() {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(Self(raw))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn check(ok: i32) -> Result<(), String> {
    if ok == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(())
}

pub(super) fn get(
    host: &str,
    path: &str,
    max_bytes: usize,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    if Instant::now() >= deadline {
        return Err("The update request timed out.".into());
    }
    let agent = wide(concat!("Backtracking/", env!("CARGO_PKG_VERSION")));
    let host = wide(host);
    let path = wide(path);
    let verb = wide("GET");
    let session = Handle::new(unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        )
    })?;
    check(unsafe { WinHttpSetTimeouts(session.0, 1700, 1700, 2400, 2400) })?;
    let connect = Handle::new(unsafe { WinHttpConnect(session.0, host.as_ptr(), 443, 0) })?;
    let request = Handle::new(unsafe {
        WinHttpOpenRequest(
            connect.0,
            verb.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    let redirects = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;
    check(unsafe {
        WinHttpSetOption(
            request.0,
            WINHTTP_OPTION_REDIRECT_POLICY,
            (&raw const redirects).cast(),
            size_of::<u32>() as u32,
        )
    })?;
    check(unsafe { WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) })?;
    check(unsafe { WinHttpReceiveResponse(request.0, null_mut()) })?;

    let mut status: u32 = 0;
    let mut status_size = size_of::<u32>() as u32;
    check(unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&raw mut status).cast(),
            &mut status_size,
            null_mut(),
        )
    })?;
    if status != 200 {
        return Err(format!("GitHub returned HTTP {status}."));
    }

    let mut bytes = Vec::new();
    let mut buffer = [0u8; 32 * 1024];
    loop {
        if Instant::now() >= deadline {
            return Err("The update download timed out.".into());
        }
        let mut count = 0;
        check(unsafe {
            WinHttpReadData(
                request.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut count,
            )
        })?;
        let count = count as usize;
        if count == 0 {
            return Ok(bytes);
        }
        if count > buffer.len() || bytes.len().saturating_add(count) > max_bytes {
            return Err("The update response exceeds its size limit.".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

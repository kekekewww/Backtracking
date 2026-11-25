//! The few things done differently per platform: files only the user can read, the
//! clipboard, and whether a process is still running.

use std::io::{self, Write};
use std::path::Path;

/// Writes `bytes` to `path` through a new file that only the current user may open, then
/// moves it into place. On Windows the file gets a protected access list naming the user
/// alone; elsewhere its mode is 0600.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("new");
    let _ = std::fs::remove_file(&temporary);
    {
        let mut file = create_private(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

#[cfg(windows)]
fn create_private(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{
        LocalFree, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL};

    let sid = windows::current_user_sid()?;
    // Protected (no inherited entries), full access for the user and nobody else.
    let sddl: Vec<u16> = format!("D:P(A;;FA;;;{sid})\0").encode_utf16().collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: `sddl` is a terminated UTF-16 string; the descriptor is freed below.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: every pointer is valid for the call.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    let error = io::Error::last_os_error();
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(descriptor) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(error);
    }
    // SAFETY: a valid handle that this function owns.
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
}

#[cfg(not(windows))]
fn create_private(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Puts `text` on the clipboard.
#[cfg(windows)]
pub fn copy_to_clipboard(text: &str) -> io::Result<()> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    // Another program may hold the clipboard for a moment.
    let mut opened = false;
    for _ in 0..20 {
        // SAFETY: no window owns the clipboard while it is open here.
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if !opened {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        // SAFETY: the clipboard is open; the memory is handed to it on success.
        unsafe {
            if EmptyClipboard() == 0 {
                return Err(io::Error::last_os_error());
            }
            let memory = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            if memory.is_null() {
                return Err(io::Error::last_os_error());
            }
            let target = GlobalLock(memory).cast::<u16>();
            if target.is_null() {
                let error = io::Error::last_os_error();
                GlobalFree(memory);
                return Err(error);
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
            GlobalUnlock(memory);
            if SetClipboardData(u32::from(CF_UNICODETEXT), memory).is_null() {
                let error = io::Error::last_os_error();
                GlobalFree(memory);
                return Err(error);
            }
            Ok(())
        }
    })();
    // SAFETY: opened above.
    unsafe { CloseClipboard() };
    result
}

#[cfg(target_os = "macos")]
pub fn copy_to_clipboard(text: &str) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/bin/pbcopy")
        .env("LC_CTYPE", "UTF-8")
        .stdin(Stdio::piped())
        .spawn()?;
    let written = child.stdin.take().unwrap().write_all(text.as_bytes());
    let status = child.wait()?;
    written?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("pbcopy exited with {status}")))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn copy_to_clipboard(_text: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copying is only offered on Windows",
    ))
}

/// Whether process `pid` is running the program at `program`. Used to tell a connection file
/// left behind by a run that ended from one that is current.
#[cfg(windows)]
pub fn process_runs(pid: u32, program: &Path) -> bool {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: plain calls with valid pointers; the handle is closed below.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
        let mut buffer = vec![0u16; 32_768];
        let mut length = buffer.len() as u32;
        let named = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &mut length,
        ) != 0;
        CloseHandle(handle);
        if !alive || !named {
            return false;
        }
        let running =
            std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
        same_file(&running, program)
    }
}

#[cfg(target_os = "macos")]
pub fn process_runs(pid: u32, program: &Path) -> bool {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    #[link(name = "proc")]
    extern "C" {
        fn proc_pidpath(pid: i32, buf: *mut std::ffi::c_void, size: u32) -> i32;
    }
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    let mut buf = [0u8; 4096];
    let count = unsafe { proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if count <= 0 {
        return false;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let running = std::fs::canonicalize(OsStr::from_bytes(&buf[..end]));
    matches!((running,std::fs::canonicalize(program)),(Ok(a),Ok(b)) if a==b)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn process_runs(pid: u32, _program: &Path) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(windows)]
fn same_file(a: &Path, b: &Path) -> bool {
    let canonical = |path: &Path| std::fs::canonicalize(path).ok();
    match (canonical(a), canonical(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// The current user's SID, as text (`S-1-5-21-…`).
    pub fn current_user_sid() -> io::Result<String> {
        // SAFETY: plain calls with valid pointers; handles and strings are freed below.
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut size = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
            // u64 keeps the buffer aligned for TOKEN_USER.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                &mut size,
            );
            let error = io::Error::last_os_error();
            CloseHandle(token);
            if ok == 0 {
                return Err(error);
            }
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut text = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text.cast());
            Ok(sid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_private_file_is_written_whole_and_replaces_the_old_one() {
        let folder = std::env::temp_dir().join(format!("agent-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("endpoint.json");
        write_private(&path, b"first").unwrap();
        write_private(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(!path.with_extension("new").exists());
        #[cfg(windows)]
        {
            // Only the user is named, and nothing is inherited from the folder.
            let output = std::process::Command::new("icacls")
                .arg(&path)
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
            let lines: Vec<&str> = text.lines().filter(|line| line.contains(":(")).collect();
            assert_eq!(lines.len(), 1, "{text}");
            assert!(!text.contains("(i)"), "{text}");
        }
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn this_process_is_found_running_its_own_program() {
        let me = std::env::current_exe().unwrap();
        assert!(process_runs(std::process::id(), &me));
        assert!(!process_runs(
            std::process::id(),
            Path::new("C:\\nothing\\else.exe")
        ));
    }
}

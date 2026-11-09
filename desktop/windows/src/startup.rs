use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use windows::core::{PCWSTR, w};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GetLastError,
    HANDLE,
};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, PostMessageW, RegisterWindowMessageW,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("Backtracking");
const INSTANCE_NAME: PCWSTR =
    w!("Local\\Backtracking.Desktop.7c1311ad-1d75-4b4b-846a-ff7d0ef2dc79");
const SHOW_MESSAGE: PCWSTR = w!("Backtracking.Desktop.Show.7c1311ad-1d75-4b4b-846a-ff7d0ef2dc79");

pub(crate) fn enabled() -> io::Result<bool> {
    let mut length = 0;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY.as_ptr(),
            VALUE.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut length,
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(false),
        0 => Ok(true),
        _ => Err(io::Error::from_raw_os_error(status as i32)),
    }
}

pub(crate) fn set_enabled(enabled: bool) -> io::Result<()> {
    let status = if enabled {
        let executable = std::env::current_exe()?;
        let command = startup_command(executable.as_os_str())?;
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY.as_ptr(),
                VALUE.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                size_of_val(command.as_slice()) as u32,
            )
        }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY.as_ptr(), VALUE.as_ptr()) } {
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => return Ok(()),
            status => status,
        }
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

fn startup_command(path: &OsStr) -> io::Result<Vec<u16>> {
    let quote = u16::from(b'"');
    let command: Vec<_> = [quote]
        .into_iter()
        .chain(path.encode_wide())
        .chain([quote, 0])
        .collect();
    if command.len() > 261 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "The Backtracking executable path is too long for Windows startup. Move it to a shorter permanent path.",
        ));
    }
    Ok(command)
}

pub(crate) fn show_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(SHOW_MESSAGE.as_ptr()) })
}

pub(crate) struct Instance(HANDLE);

impl Instance {
    pub(crate) fn acquire() -> io::Result<Option<Self>> {
        // Windows scopes Local kernel objects to the current interactive session.
        let handle = unsafe { CreateMutexW(null(), 0, INSTANCE_NAME.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let instance = Self(handle);
        if already_running {
            let show = show_message();
            if show != 0 {
                unsafe { PostMessageW(HWND_BROADCAST, show, 0, 0) };
            }
            Ok(None)
        } else {
            Ok(Some(instance))
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_path_is_quoted_with_unicode_preserved() {
        let command = startup_command(OsStr::new(r"C:\Audio Tools\錄音\Backtracking.exe")).unwrap();
        assert_eq!(command.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&command[..command.len() - 1]).unwrap(),
            r#""C:\Audio Tools\錄音\Backtracking.exe""#,
        );
    }

    #[test]
    fn startup_rejects_paths_outside_the_run_value_limit() {
        assert!(startup_command(OsStr::new(&"x".repeat(258))).is_ok());
        assert!(startup_command(OsStr::new(&"x".repeat(259))).is_err());
    }
}

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

/// Names a diagnostic copy of the app that runs beside the installed one.
pub(crate) const TEST_INSTANCE_VARIABLE: &str = "BACKTRACKING_TEST_INSTANCE";

/// The name in `BACKTRACKING_TEST_INSTANCE`, when the app was started as a test copy.
///
/// A test copy has its own instance mutex and show message, so it never finds or wakes the
/// installed app. It registers no global shortcut, adds no tray icon, never reads or writes
/// the Windows startup entry and never checks for updates. Its settings, add-ons and Sender
/// registrations are kept apart by starting it with `LOCALAPPDATA` pointing at another folder,
/// which `check_test_data_folder` insists on.
pub(crate) fn test_instance() -> Option<&'static str> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::var(TEST_INSTANCE_VARIABLE)
            .ok()
            .filter(|name| valid_test_instance(name))
    })
    .as_deref()
}

/// True when the variable is set to something that is not a usable name. The app refuses to
/// start rather than run as the normal instance.
pub(crate) fn test_instance_invalid() -> bool {
    std::env::var_os(TEST_INSTANCE_VARIABLE).is_some() && test_instance().is_none()
}

fn valid_test_instance(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// A test copy must not share the user's data folder: it would read their settings and
/// add-ons and write over them.
pub(crate) fn check_test_data_folder() -> io::Result<()> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

    let separate = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "A test copy needs LOCALAPPDATA set to a separate, existing folder.",
        )
    };
    let requested = std::env::var_os("LOCALAPPDATA").ok_or_else(separate)?;
    let requested = dunce::canonicalize(requested).map_err(|_| separate())?;
    let known = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None) }
        .map_err(io::Error::other)?;
    let user = unsafe { known.to_string() };
    unsafe { CoTaskMemFree(Some(known.0.cast())) };
    let user = user.map_err(io::Error::other)?;
    let user = dunce::canonicalize(&user).unwrap_or_else(|_| user.into());
    let same = requested.to_string_lossy().to_lowercase() == user.to_string_lossy().to_lowercase();
    if same { Err(separate()) } else { Ok(()) }
}

fn instance_object_name(base: PCWSTR, instance: Option<&str>) -> Vec<u16> {
    let mut name = unsafe { base.as_wide() }.to_vec();
    if let Some(instance) = instance {
        name.extend(".test.".encode_utf16().chain(instance.encode_utf16()));
    }
    name.push(0);
    name
}

pub(crate) fn show_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    *MESSAGE.get_or_init(|| {
        let name = instance_object_name(SHOW_MESSAGE, test_instance());
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    })
}

pub(crate) struct Instance(HANDLE);

impl Instance {
    pub(crate) fn acquire() -> io::Result<Option<Self>> {
        let name = instance_object_name(INSTANCE_NAME, test_instance());
        // Windows scopes Local kernel objects to the current interactive session.
        let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
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
    fn a_test_copy_uses_its_own_instance_and_show_message_names() {
        let plain = instance_object_name(INSTANCE_NAME, None);
        assert_eq!(
            plain,
            unsafe { INSTANCE_NAME.as_wide() }
                .iter()
                .copied()
                .chain([0])
                .collect::<Vec<_>>()
        );
        let test = String::from_utf16(&instance_object_name(SHOW_MESSAGE, Some("c1"))).unwrap();
        assert_eq!(
            test,
            "Backtracking.Desktop.Show.7c1311ad-1d75-4b4b-846a-ff7d0ef2dc79.test.c1\0"
        );
        assert_ne!(instance_object_name(INSTANCE_NAME, Some("c1")), plain);
        for name in ["c1", "ui-check_2", &"x".repeat(40)] {
            assert!(valid_test_instance(name), "{name}");
        }
        for name in ["", "a b", "a\\b", "名", &"x".repeat(41)] {
            assert!(!valid_test_instance(name), "{name}");
        }
    }

    #[test]
    fn startup_rejects_paths_outside_the_run_value_limit() {
        assert!(startup_command(OsStr::new(&"x".repeat(258))).is_ok());
        assert!(startup_command(OsStr::new(&"x".repeat(259))).is_err());
    }
}

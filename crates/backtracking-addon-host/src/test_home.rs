//! Test helpers must use a separate home because Sender discovery uses HOME.
#[cfg(target_os = "macos")]
pub fn check() -> std::io::Result<()> {
    use std::{
        ffi::{CStr, OsStr},
        io,
        os::unix::ffi::OsStrExt,
        path::Path,
    };
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut entry = std::ptr::null_mut();
    let mut buffer = vec![0u8; 16 * 1024];
    let result = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            pwd.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut entry,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    if entry.is_null() {
        return Err(io::Error::other("Cannot resolve the account home"));
    }
    let account = unsafe { CStr::from_ptr((*entry).pw_dir) };
    let account = Path::new(OsStr::from_bytes(account.to_bytes())).canonicalize()?;
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::other("Set HOME to an existing test directory"))?;
    if Path::new(&home).canonicalize()? == account {
        return Err(io::Error::other(
            "Set HOME to a separate test directory before publishing test Senders",
        ));
    }
    Ok(())
}
#[cfg(not(target_os = "macos"))]
pub fn check() -> std::io::Result<()> {
    Ok(())
}

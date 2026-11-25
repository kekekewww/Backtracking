//! Preserve macOS quarantine metadata across both the private review copy and
//! extraction. Read it from the same open file as the package bytes.
use std::{
    ffi::{c_char, c_void},
    fs::File,
    io,
    os::fd::AsRawFd,
};
extern "C" {
    fn fgetxattr(
        fd: i32,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
        position: u32,
        options: i32,
    ) -> isize;
    fn fsetxattr(
        fd: i32,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        position: u32,
        options: i32,
    ) -> i32;
}
const NAME: &std::ffi::CStr = c"com.apple.quarantine";
const ENOATTR: i32 = 93;
pub struct Quarantine(Option<Vec<u8>>);
impl Quarantine {
    pub fn read(file: &File) -> io::Result<Self> {
        let len = unsafe {
            fgetxattr(
                file.as_raw_fd(),
                NAME.as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                0,
            )
        };
        if len < 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(ENOATTR) {
                Ok(Self(None))
            } else {
                Err(err)
            };
        }
        let mut bytes = vec![0; len as usize];
        let copied = unsafe {
            fgetxattr(
                file.as_raw_fd(),
                NAME.as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                0,
                0,
            )
        };
        if copied < 0 {
            return Err(io::Error::last_os_error());
        }
        if copied != len {
            return Err(io::Error::other("Package quarantine metadata changed"));
        }
        Ok(Self(Some(bytes)))
    }
    pub fn apply(&self, file: &File) -> io::Result<()> {
        if let Some(bytes) = &self.0 {
            let result = unsafe {
                fsetxattr(
                    file.as_raw_fd(),
                    NAME.as_ptr(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    0,
                    0,
                )
            };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

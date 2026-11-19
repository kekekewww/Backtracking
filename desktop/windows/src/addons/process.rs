use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

/// Owns the process and its descendants. The non-inherited job handle closes
/// even when the host crashes, so a child cannot outlive the host.
pub(super) struct AddonProcess {
    process: OwnedHandle,
    job: OwnedHandle,
    id: u32,
    #[cfg(test)]
    pub(super) fail_termination: bool,
}

impl AddonProcess {
    pub(super) fn launch(
        program: &Path,
        args: &[String],
        cwd: &Path,
        environment: &[(OsString, OsString)],
    ) -> Result<Self, String> {
        if !program.is_absolute() || !cwd.is_absolute() {
            return Err("The add-on program and working directory must be absolute paths".into());
        }
        let program_wide = terminated(program.as_os_str())?;
        let cwd_wide = terminated(cwd.as_os_str())?;
        let mut command_line = quote_argument(program.as_os_str())?;
        for argument in args {
            command_line.push(b' ' as u16);
            command_line.extend(quote_argument(OsStr::new(argument))?);
        }
        command_line.push(0);
        if command_line.len() > 32_767 {
            return Err("The add-on command line is too long".into());
        }
        let environment = environment_block(environment)?;

        // The job is assigned by CreateProcess itself. Assigning it only after
        // creation would leave an orphan if the host crashed in between.
        let job = owned(
            unsafe { CreateJobObjectW(null(), null()) },
            "Create add-on job",
        )?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(last_error("Set add-on job limits"));
        }

        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let null_name = [b'N' as u16, b'U' as u16, b'L' as u16, 0];
        let null_stream = owned(
            unsafe {
                CreateFileW(
                    null_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    &security,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            },
            "Open add-on standard streams",
        )?;
        let job_handle = job.as_raw_handle();
        let stream_handle = null_stream.as_raw_handle();
        let mut attributes = Attributes::new(2)?;
        attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST as usize, &job_handle)?;
        // Restrict inheritance to NUL, rather than passing unrelated host handles.
        attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, &stream_handle)?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = stream_handle;
        startup.StartupInfo.hStdOutput = stream_handle;
        startup.StartupInfo.hStdError = stream_handle;
        startup.lpAttributeList = attributes.as_mut_ptr();
        let mut information = PROCESS_INFORMATION::default();
        let created = unsafe {
            CreateProcessW(
                program_wide.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW
                    | CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                cwd_wide.as_ptr(),
                &startup.StartupInfo,
                &mut information,
            )
        };
        if created == 0 {
            return Err(last_error("Start add-on"));
        }
        // CreateProcess succeeded, so both handles are owned and valid.
        let process = unsafe { OwnedHandle::from_raw_handle(information.hProcess) };
        let thread = unsafe { OwnedHandle::from_raw_handle(information.hThread) };
        let child = Self {
            process,
            job,
            id: information.dwProcessId,
            #[cfg(test)]
            fail_termination: false,
        };
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            return Err(last_error("Resume add-on"));
        }
        Ok(child)
    }

    pub(super) fn id(&self) -> u32 {
        self.id
    }

    pub(super) fn try_wait(&mut self) -> Result<Option<u32>, String> {
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                    return Err(last_error("Read add-on exit status"));
                }
                Ok(Some(code))
            }
            _ => Err(last_error("Check add-on process")),
        }
    }

    pub(super) fn terminate(&mut self) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_termination {
            return Err("Injected termination failure".into());
        }
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(last_error("End add-on process tree"));
        }
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 2_000) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err("The add-on did not end after termination".into()),
            _ => Err(last_error("Wait for add-on termination")),
        }
    }
}

// Closing `job` kills any descendants, even when the main process already exited.
// All handles are non-inherited except the explicitly listed NUL stream handle.

pub(super) fn random_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    let status = unsafe {
        BCryptGenRandom(
            null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err(format!(
            "Generate add-on token: Windows status 0x{status:08x}"
        ));
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push(HEX[usize::from(byte >> 4)] as char);
        token.push(HEX[usize::from(byte & 15)] as char);
    }
    Ok(token)
}

struct Attributes {
    // usize gives the allocation the alignment required by the Windows API.
    storage: Vec<usize>,
}

impl Attributes {
    fn new(count: u32) -> Result<Self, String> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes) };
        if bytes == 0 {
            return Err(last_error("Size add-on process attributes"));
        }
        let mut attributes = Self {
            storage: vec![0; bytes.div_ceil(size_of::<usize>())],
        };
        if unsafe {
            InitializeProcThreadAttributeList(attributes.as_mut_ptr(), count, 0, &mut bytes)
        } == 0
        {
            // DeleteProcThreadAttributeList requires successful initialization.
            let error = last_error("Initialize add-on process attributes");
            attributes.storage.clear();
            return Err(error);
        }
        Ok(attributes)
    }

    fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }

    fn set(&mut self, kind: usize, handle: &HANDLE) -> Result<(), String> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                kind,
                (handle as *const HANDLE).cast(),
                size_of::<HANDLE>(),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(last_error("Set add-on process attribute"));
        }
        Ok(())
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        if !self.storage.is_empty() {
            unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
        }
    }
}

fn owned(handle: HANDLE, operation: &str) -> Result<OwnedHandle, String> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(last_error(operation))
    } else {
        // The creating API transferred this handle to the caller.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

fn last_error(operation: &str) -> String {
    format!("{operation}: {}", io::Error::last_os_error())
}

fn terminated(value: &OsStr) -> Result<Vec<u16>, String> {
    let mut result: Vec<_> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err("An add-on process argument contains a NUL character".into());
    }
    result.push(0);
    Ok(result)
}

/// Quote one argument using the Windows C runtime's backslash/quote rules.
fn quote_argument(argument: &OsStr) -> Result<Vec<u16>, String> {
    let mut result = vec![b'"' as u16];
    let mut backslashes = 0;
    for value in argument.encode_wide() {
        match value {
            0 => return Err("An add-on process argument contains a NUL character".into()),
            92 => backslashes += 1,
            34 => {
                result.extend(std::iter::repeat_n(92, backslashes * 2 + 1));
                result.push(34);
                backslashes = 0;
            }
            value => {
                result.extend(std::iter::repeat_n(92, backslashes));
                result.push(value);
                backslashes = 0;
            }
        }
    }
    result.extend(std::iter::repeat_n(92, backslashes * 2));
    result.push(34);
    Ok(result)
}

fn environment_block(overrides: &[(OsString, OsString)]) -> Result<Vec<u16>, String> {
    let mut entries: Vec<_> = std::env::vars_os().collect();
    for (key, value) in overrides {
        let name = key.to_string_lossy();
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            return Err("An add-on environment variable has an invalid name".into());
        }
        entries.retain(|(existing, _)| !existing.to_string_lossy().eq_ignore_ascii_case(&name));
        entries.push((key.clone(), value.clone()));
    }
    entries.sort_by_cached_key(|(key, _)| key.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (key, value) in entries {
        let mut entry = key;
        entry.push("=");
        entry.push(value);
        block.extend(terminated(&entry)?);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};

    const CHILD_FLAG: &str = "BACKTRACKING_PROCESS_UNIT_TEST_CHILD";

    #[test]
    fn random_tokens_are_full_length_and_differ() {
        let first = random_token().unwrap();
        let second = random_token().unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn arguments_preserve_spaces_quotes_and_trailing_slashes() {
        let quoted =
            |value: &str| String::from_utf16(&quote_argument(OsStr::new(value)).unwrap()).unwrap();
        assert_eq!(quoted(""), "\"\"");
        assert_eq!(quoted("a b"), "\"a b\"");
        assert_eq!(quoted("a\"b"), "\"a\\\"b\"");
        assert_eq!(quoted("a\\"), "\"a\\\\\"");
        assert!(quote_argument(OsStr::new("a\0b")).is_err());
    }

    #[test]
    fn environment_override_does_not_change_the_host() {
        let before = std::env::var_os(CHILD_FLAG);
        let block = environment_block(&[(CHILD_FLAG.into(), "isolated".into())]).unwrap();
        let text = String::from_utf16(&block).unwrap();
        assert!(text.contains(&format!("{CHILD_FLAG}=isolated\0")));
        assert!(text.ends_with("\0\0"));
        assert_eq!(std::env::var_os(CHILD_FLAG), before);
    }

    fn child() -> AddonProcess {
        let executable = std::env::current_exe().unwrap();
        AddonProcess::launch(
            &executable,
            &[
                "--exact".into(),
                "addons::process::tests::helper_child".into(),
                "--nocapture".into(),
            ],
            executable.parent().unwrap(),
            &[(CHILD_FLAG.into(), "1".into())],
        )
        .unwrap()
    }

    #[test]
    fn helper_child() {
        match std::env::var(CHILD_FLAG).as_deref() {
            Ok("crash-parent") => {
                let directory = std::path::PathBuf::from(
                    std::env::var_os("BACKTRACKING_PROCESS_UNIT_TEST_DIRECTORY").unwrap(),
                );
                let child = child();
                std::fs::write(directory.join("child-pid"), child.id().to_string()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while !directory.join("crash-now").exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                // This exits without running AddonProcess's field destructors.
                std::process::exit(31);
            }
            Ok(_) => std::thread::sleep(Duration::from_secs(120)),
            Err(_) => {}
        }
    }

    #[test]
    fn terminate_ends_an_owned_process() {
        let mut child = child();
        std::thread::sleep(Duration::from_millis(100));
        assert!(child.id() > 0);
        assert_eq!(child.try_wait().unwrap(), None);
        child.terminate().unwrap();
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn closing_the_job_ends_the_process() {
        let child = child();
        let process = child.process.try_clone().unwrap();
        drop(child);
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 2_000) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn abrupt_host_exit_ends_the_addon() {
        let directory = std::env::temp_dir().join(format!(
            "backtracking-addon-crash-{}",
            random_token().unwrap()
        ));
        std::fs::create_dir(&directory).unwrap();
        let executable = std::env::current_exe().unwrap();
        let parent = AddonProcess::launch(
            &executable,
            &[
                "--exact".into(),
                "addons::process::tests::helper_child".into(),
                "--nocapture".into(),
            ],
            executable.parent().unwrap(),
            &[
                (CHILD_FLAG.into(), "crash-parent".into()),
                (
                    "BACKTRACKING_PROCESS_UNIT_TEST_DIRECTORY".into(),
                    directory.as_os_str().to_owned(),
                ),
            ],
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = loop {
            if let Ok(contents) = std::fs::read_to_string(directory.join("child-pid")) {
                if let Ok(pid) = contents.parse::<u32>() {
                    break pid;
                }
            }
            assert!(Instant::now() < deadline, "The child did not start");
            std::thread::sleep(Duration::from_millis(10));
        };
        let child_process = owned(
            unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) },
            "Open test child process",
        )
        .unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(child_process.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
        std::fs::write(directory.join("crash-now"), b"").unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(parent.process.as_raw_handle(), 2_000) },
            WAIT_OBJECT_0
        );
        // Keep the outer job open: only the crashed parent's job can have ended
        // this child, rather than cleanup by the test's own outer job.
        assert_eq!(
            unsafe { WaitForSingleObject(child_process.as_raw_handle(), 2_000) },
            WAIT_OBJECT_0
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}

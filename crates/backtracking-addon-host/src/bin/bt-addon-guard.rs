//! Private helper. Its stdin is the lifetime of the parent host. The guard owns
//! a process group so the group number cannot be reused before cleanup.
#[cfg(unix)]
mod unix {
    use std::{
        io::{self, Write},
        os::unix::process::ExitStatusExt,
        process::{Command, Stdio},
    };
    #[cfg(target_os = "macos")]
    fn close_inheritable(fd: libc::c_int) -> io::Result<()> {
        if fd <= 2 {
            return Ok(());
        }
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            let err = io::Error::last_os_error();
            // The listing can include its own (now closed) directory descriptor.
            return if err.raw_os_error() == Some(libc::EBADF) {
                Ok(())
            } else {
                Err(err)
            };
        }
        if flags & libc::FD_CLOEXEC == 0 && unsafe { libc::close(fd) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn open_descriptors() -> io::Result<Vec<libc::c_int>> {
        let mut fds = Vec::new();
        for entry in std::fs::read_dir("/dev/fd")? {
            if let Some(fd) = entry?.file_name().to_str().and_then(|s| s.parse().ok()) {
                fds.push(fd);
            }
        }
        Ok(fds)
    }

    #[cfg(target_os = "macos")]
    fn close_inherited_files() -> io::Result<()> {
        // The standalone guard is single threaded and has opened no app files.
        open_descriptors()?
            .into_iter()
            .try_for_each(close_inheritable)
    }

    #[cfg(all(test, target_os = "macos"))]
    #[test]
    fn rc1_guard_drops_inheritable_file_and_preserves_stdio() {
        use std::os::fd::AsRawFd;
        let file = std::fs::File::open("/dev/null").unwrap();
        let dup = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 3) };
        assert!(dup >= 3);
        assert!(open_descriptors().unwrap().contains(&dup));
        close_inheritable(dup).unwrap();
        assert_eq!(unsafe { libc::fcntl(dup, libc::F_GETFD) }, -1);
        close_inheritable(file.as_raw_fd()).unwrap();
        assert!(file.metadata().is_ok());
        close_inheritable(1).unwrap();
        assert_ne!(unsafe { libc::fcntl(1, libc::F_GETFD) }, -1);
    }

    pub(super) fn run() {
        #[cfg(target_os = "macos")]
        if let Err(err) = close_inherited_files() {
            println!("Cannot isolate add-on descriptors: {err}");
            return;
        }
        // Enter an isolated process group before spawning any add-on code.
        if unsafe { libc::setpgid(0, 0) } != 0 {
            return;
        }
        let mut args = std::env::args_os().skip(1);
        let Some(program) = args.next() else { return };
        let mut child = match Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                println!("{e}");
                return;
            }
        };
        // Even if the host died during spawn, continue directly to group cleanup.
        let reported = writeln!(io::stdout().lock(), "{}", child.id()).is_ok();
        let mut fd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        while reported {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let _ = writeln!(
                        io::stdout().lock(),
                        "{}",
                        status
                            .code()
                            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
                    );
                    break;
                }
                Err(_) => break,
                Ok(None) => {}
            }
            let result = unsafe { libc::poll(&mut fd, 1, 10) };
            if result > 0
                || (result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted)
            {
                break;
            }
        }
        unsafe {
            libc::killpg(0, libc::SIGKILL);
        }
    }
}
#[cfg(unix)]
fn main() {
    unix::run()
}
#[cfg(not(unix))]
fn main() {
    eprintln!("This add-on host requires Unix");
    std::process::exit(1)
}

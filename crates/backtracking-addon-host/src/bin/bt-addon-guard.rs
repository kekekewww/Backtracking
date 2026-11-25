//! Private helper. Its stdin is the lifetime of the parent host. The guard owns
//! a process group so the group number cannot be reused before cleanup.
#[cfg(unix)]
mod unix {
    use std::{
        io::{self, Write},
        os::unix::process::ExitStatusExt,
        process::{Command, Stdio},
    };
    pub(super) fn run() {
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

#![cfg(unix)]
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
#[test]
fn closing_the_guard_pipe_ends_its_child() {
    let mut guard = Command::new(env!("CARGO_BIN_EXE_bt-addon-guard"))
        .args(["/bin/sleep", "60"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pid = String::new();
    BufReader::new(guard.stdout.take().unwrap())
        .read_line(&mut pid)
        .unwrap();
    let pid: i32 = pid.trim().parse().unwrap();
    drop(guard.stdin.take());
    guard.wait().unwrap();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(std::time::Instant::now() < end, "child survived guard EOF");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

use std::{io::Write, path::PathBuf, time::{Duration, Instant}};

// A process-loopback target that never creates an audio stream or plays audio.
fn main() {
    let stop_marker = std::env::args_os().nth(1).map(PathBuf::from)
        .expect("workspace stop marker required");
    println!("{{\"event\":\"ready\",\"pid\":{}}}", std::process::id());
    std::io::stdout().flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline && !stop_marker.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
}

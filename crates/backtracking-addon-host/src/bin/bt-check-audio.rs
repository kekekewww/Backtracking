//! Assert that a Sender delivers a non-silent packet within five seconds.
use backtracking_core::SenderReceiver;
use std::time::{Duration, Instant};
fn main() {
    let id = std::env::args().nth(1).expect("pass the Sender id");
    let until = Instant::now() + Duration::from_secs(5);
    let mut r = loop {
        if let Ok(r) = SenderReceiver::connect(&id) {
            break r;
        }
        assert!(Instant::now() < until, "Sender did not appear");
        std::thread::sleep(Duration::from_millis(10))
    };
    let (mut samples, mut markers) = (Vec::new(), Vec::new());
    while Instant::now() < until {
        if let Some(p) = r.packet(&mut samples, &mut markers).unwrap() {
            if samples.iter().any(|n| n.abs() > 0.001) {
                println!(
                    "{} Hz, {} frames, {} channels",
                    p.sample_rate, p.frames, r.info.channels
                );
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("No non-silent audio received")
}

//! Connection boundaries use raw peers so invalid input does not pass through our codec first.
use backtracking_addon::host::{Connection, Host, HostEvent};
use backtracking_addon::protocol::{AddonMessage, HostMessage};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const ID: &str = "com.example.test";

fn host() -> Host {
    let mut host = Host::bind("test").unwrap();
    host.expect_addon(ID, "secret", BTreeMap::new()).unwrap();
    host
}

fn raw_peer(host: &Host) -> TcpStream {
    let stream = TcpStream::connect(("127.0.0.1", host.port())).unwrap();
    stream.set_nodelay(true).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
}

fn hello(id: &str) -> Vec<u8> {
    format!(
        "{}\n",
        json!({"type":"hello","contract":1,"token":"secret","id":id,"version":"1"})
    )
    .into_bytes()
}

fn connect(host: &mut Host, id: &str) -> (TcpStream, Connection) {
    let mut raw = raw_peer(host);
    raw.write_all(&hello(id)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        for event in host.poll().unwrap() {
            if let HostEvent::Connected(connection) = event {
                assert_eq!(connection.id(), id);
                return (raw, connection);
            }
        }
        assert!(Instant::now() < deadline, "handshake did not finish");
        thread::sleep(Duration::from_millis(1));
    }
}

fn wait_rejected(host: &mut Host) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        for event in host.poll().unwrap() {
            if let HostEvent::Rejected { reason, .. } = event {
                return reason;
            }
        }
        assert!(Instant::now() < deadline, "invalid peer was not rejected");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn oversized_unterminated_line_is_rejected_and_closed() {
    // Losing the frame limit would retain this peer indefinitely instead of closing it.
    let mut host = host();
    let mut raw = raw_peer(&host);
    raw.write_all(&vec![b' '; 65_536]).unwrap();
    assert!(wait_rejected(&mut host).contains("65536"));
    let mut byte = [0];
    match raw.read(&mut byte) {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        result => panic!("oversized peer remained connected: {result:?}"),
    }
}

#[test]
fn fragmented_hello_connects_only_after_the_newline_arrives() {
    // Treating a partial read as a complete JSON document would reject this valid peer.
    let mut host = host();
    let mut raw = raw_peer(&host);
    let line = hello(ID);
    for fragment in line[..line.len() - 1].chunks(7) {
        raw.write_all(fragment).unwrap();
        assert!(host.poll().unwrap().is_empty());
    }
    raw.write_all(b"\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if host
            .poll()
            .unwrap()
            .into_iter()
            .any(|event| matches!(event, HostEvent::Connected(_)))
        {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let mut response = String::new();
    BufReader::new(raw).read_line(&mut response).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).unwrap()["type"],
        "welcome"
    );
}

#[test]
fn a_successful_token_cannot_authenticate_a_second_connection() {
    // Leaving an expected token reusable would authorize two peers for a single process run.
    let mut host = host();
    let (_raw, _connection) = connect(&mut host, ID);
    let mut duplicate = raw_peer(&host);
    duplicate.write_all(&hello(ID)).unwrap();
    assert!(wait_rejected(&mut host).contains("already connected"));
    let mut response = String::new();
    BufReader::new(duplicate).read_line(&mut response).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).unwrap()["type"],
        "rejected"
    );
}

#[test]
fn a_peer_that_does_not_read_fills_its_queue_without_stalling_another_peer() {
    // Blocking writes or an unbounded output queue would prevent this sender from reporting back.
    let mut host = host();
    host.expect_addon("com.example.other", "secret", BTreeMap::new())
        .unwrap();
    let (slow_raw, mut slow) = connect(&mut host, ID);
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let message = HostMessage::SetSetting {
            key: "payload".into(),
            value: json!("x".repeat(60_000)),
        };
        for _ in 0..1024 {
            if let Err(error) = slow.send(&message) {
                tx.send(error.to_string()).unwrap();
                return;
            }
        }
        tx.send("The outgoing queue did not fill after 60 MiB".into())
            .unwrap();
    });
    let (mut fast_raw, mut fast) = connect(&mut host, "com.example.other");
    fast.send(&HostMessage::Quit).unwrap();
    let mut reader = BufReader::new(&mut fast_raw);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["type"],
        "welcome"
    );
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["type"],
        "quit"
    );
    let result = rx.recv_timeout(Duration::from_secs(5));
    // Closing only the socket created by this test also releases a broken blocking implementation.
    drop(slow_raw);
    worker.join().unwrap();
    assert!(result.unwrap().contains("queue is full"));
}

#[test]
fn queued_transport_updates_are_not_mistaken_for_an_invalid_send_rate() {
    // Receiver timing cannot distinguish delayed UI polling from a sender that emitted a burst.
    let mut host = host();
    let (mut raw, mut connection) = connect(&mut host, ID);
    for position in 0..8 {
        writeln!(
            raw,
            "{}",
            json!({"type":"transport","source":"main","state":"playing","position":position})
        )
        .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut received = 0;
    while received < 8 {
        if let Some(AddonMessage::Transport { position, .. }) = connection.try_recv().unwrap() {
            assert_eq!(position, f64::from(received));
            received += 1;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!connection.is_disconnected());
}

#[test]
fn a_large_parser_error_does_not_discard_another_pending_handshake() {
    // Echoing the entire invalid enum value into rejected would exceed the outgoing line limit.
    let mut host = host();
    let mut malicious = raw_peer(&host);
    writeln!(
        malicious,
        "{}",
        json!({"type":"transport","source":"main","state":"x".repeat(60_000),"position":0})
    )
    .unwrap();
    let mut good = raw_peer(&host);
    good.write_all(&hello(ID)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut rejected, mut connected) = (false, false);
    while !rejected || !connected {
        for event in host.poll().unwrap() {
            match event {
                HostEvent::Connected(connection) => {
                    assert_eq!(connection.id(), ID);
                    connected = true;
                }
                HostEvent::Rejected { reason, .. } => {
                    assert!(reason.chars().count() <= 1000);
                    rejected = true;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "one malformed peer discarded another handshake"
        );
        thread::sleep(Duration::from_millis(1));
    }
    let mut response = String::new();
    BufReader::new(malicious).read_line(&mut response).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).unwrap()["type"],
        "rejected"
    );
}

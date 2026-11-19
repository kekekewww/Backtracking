use backtracking_addon::client::Client;
use backtracking_addon::host::{Connection, Host, HostEvent};
use backtracking_addon::protocol::{AddonMessage, HostMessage, MessageLevel, TransportState};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn wait_connected(host: &mut Host) -> Connection {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        for event in host.poll().unwrap() {
            if let HostEvent::Connected(connection) = event {
                return connection;
            }
        }
        assert!(Instant::now() < deadline, "handshake did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn handshake_delivers_stored_settings_messages_and_disconnection() {
    let mut host = Host::bind("test").unwrap();
    host.expect_addon(
        "com.example.test",
        "secret",
        BTreeMap::from([("level".into(), json!("-6"))]),
    )
    .unwrap();
    let port = host.port();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut client = Client::connect(
            port,
            "secret",
            std::env::temp_dir(),
            "com.example.test",
            "1",
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(client.settings().get("level"), Some(&json!("-6")));
        assert!(client.try_recv().unwrap().is_none());
        assert!(client
            .recv_timeout(Duration::from_millis(5))
            .unwrap()
            .is_none());
        client
            .send(&AddonMessage::Message {
                level: MessageLevel::Info,
                text: "ready".into(),
            })
            .unwrap();
        tx.send(()).unwrap();
        assert!(matches!(
            client.recv_timeout(Duration::from_secs(3)).unwrap(),
            Some(HostMessage::Quit)
        ));
        assert!(client
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .is_none());
        assert!(client.is_disconnected());
    });
    let mut connection = wait_connected(&mut host);
    assert_eq!(connection.id(), "com.example.test");
    rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(AddonMessage::Message { text, .. }) = connection.try_recv().unwrap() {
            assert_eq!(text, "ready");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    connection.send(&HostMessage::Quit).unwrap();
    connection.close();
    worker.join().unwrap();
}

#[test]
fn wrong_token_id_and_contract_receive_rejected_without_consuming_expected_token() {
    let mut host = Host::bind("test").unwrap();
    host.expect_addon("com.example.test", "secret", BTreeMap::new())
        .unwrap();
    for hello in [
        json!({"type":"hello","contract":1,"token":"wrong","id":"com.example.test","version":"1"}),
        json!({"type":"hello","contract":1,"token":"secret","id":"com.example.other","version":"1"}),
        json!({"type":"hello","contract":2,"token":"secret","id":"com.example.test","version":"1"}),
    ] {
        let mut raw = TcpStream::connect(("127.0.0.1", host.port())).unwrap();
        raw.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        writeln!(raw, "{hello}").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if host
                .poll()
                .unwrap()
                .into_iter()
                .any(|e| matches!(e, HostEvent::Rejected { .. }))
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut response = String::new();
        raw.read_to_string(&mut response).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&response).unwrap()["type"],
            "rejected"
        );
    }
    let mut good = TcpStream::connect(("127.0.0.1", host.port())).unwrap();
    good.write_all(b"{\"type\":\"hello\",\"contract\":1,\"token\":\"secret\",\"id\":\"com.example.test\",\"version\":\"1\"}\n").unwrap();
    assert_eq!(wait_connected(&mut host).id(), "com.example.test");
}

#[test]
fn silent_and_fragmented_clients_do_not_block_other_handshakes() {
    let mut host = Host::bind("test").unwrap();
    host.expect_addon("com.example.test", "secret", BTreeMap::new())
        .unwrap();
    let _silent = TcpStream::connect(("127.0.0.1", host.port())).unwrap();
    let mut partial = TcpStream::connect(("127.0.0.1", host.port())).unwrap();
    partial.write_all(b"{\"type\":").unwrap();
    let start = Instant::now();
    assert!(host.poll().unwrap().is_empty());
    assert!(start.elapsed() < Duration::from_millis(200));
    let mut good = TcpStream::connect(("127.0.0.1", host.port())).unwrap();
    good.write_all(b"{\"type\":\"hello\",\"contract\":1,\"token\":\"secret\",\"id\":\"com.example.test\",\"version\":\"1\"}\n").unwrap();
    let _connection = wait_connected(&mut host);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn handshake_timeout_and_truncated_line_are_reported() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        drop(stream);
    });
    let error = match Client::connect(
        port,
        "secret",
        std::env::temp_dir(),
        "com.example.test",
        "1",
        Duration::from_millis(20),
    ) {
        Ok(_) => panic!("silent host accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Timed out"));
    worker.join().unwrap();
    let mut host = Host::bind("test").unwrap();
    let mut raw = TcpStream::connect(("127.0.0.1", host.port())).unwrap();
    raw.write_all(b"{\"type\":").unwrap();
    raw.shutdown(std::net::Shutdown::Write).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(HostEvent::Rejected { reason, .. }) = host.poll().unwrap().into_iter().next() {
            assert!(reason.contains("middle"));
            break;
        }
        assert!(Instant::now() < deadline);
    }
}

#[test]
fn client_refuses_fifth_transport_update_in_one_second() {
    let mut host = Host::bind("test").unwrap();
    host.expect_addon("com.example.test", "secret", BTreeMap::new())
        .unwrap();
    let port = host.port();
    let worker = std::thread::spawn(move || {
        let mut client = Client::connect(
            port,
            "secret",
            std::env::temp_dir(),
            "com.example.test",
            "1",
            Duration::from_secs(3),
        )
        .unwrap();
        let update = AddonMessage::Transport {
            source: "main".into(),
            state: TransportState::Playing,
            position: 1.0,
            duration: None,
        };
        for _ in 0..4 {
            client.send(&update).unwrap();
        }
        assert!(client
            .send(&update)
            .unwrap_err()
            .to_string()
            .contains("4 transport"));
        assert!(!client.is_disconnected());
        client
            .send(&AddonMessage::Message {
                level: MessageLevel::Info,
                text: "still connected".into(),
            })
            .unwrap();
    });
    let mut connection = wait_connected(&mut host);
    let mut received = 0;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match connection.try_recv().unwrap() {
            Some(AddonMessage::Transport { .. }) => received += 1,
            Some(AddonMessage::Message { text, .. }) => {
                assert_eq!(received, 4);
                assert_eq!(text, "still connected");
                break;
            }
            None => {}
            _ => panic!("unexpected message"),
        }
        assert!(Instant::now() < deadline);
    }
    worker.join().unwrap();
}

fn raw_peer(host: &mut Host, id: &str) -> (TcpStream, Connection) {
    host.expect_addon(id, "secret", BTreeMap::new()).unwrap();
    let mut peer = TcpStream::connect(("127.0.0.1", host.port())).unwrap();
    let hello = json!({"type":"hello","contract":1,"token":"secret","id":id,"version":"1"});
    writeln!(peer, "{hello}").unwrap();
    let connection = wait_connected(host);
    (peer, connection)
}

fn receive_next(connection: &mut Connection) -> AddonMessage {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(message) = connection.try_recv().unwrap() {
            return message;
        }
        assert!(!connection.is_disconnected(), "add-on disconnected");
        assert!(Instant::now() < deadline, "add-on message did not arrive");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn malformed_appearance_envelopes_leave_the_connection_open() {
    let mut host = Host::bind("test").unwrap();
    let (mut peer, mut connection) = raw_peer(&mut host, "com.example.theme");
    writeln!(peer, r#"{{"type":"appearance","rows":null}}"#).unwrap();
    assert_eq!(receive_next(&mut connection), AddonMessage::Unknown);

    let good = json!({"type":"appearance","colors":{"waveform":"#67e7b5"}});
    writeln!(peer, "{good}").unwrap();
    let message = receive_next(&mut connection);
    let encoded = backtracking_addon::protocol::encode_addon(&message).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&encoded).unwrap(),
        good
    );
    assert!(!connection.is_disconnected());
}

#[test]
fn host_thins_appearance_floods_per_addon_and_recovers_after_one_second() {
    let mut host = Host::bind("test").unwrap();
    let (mut first, mut first_connection) = raw_peer(&mut host, "com.example.one");
    let (mut second, mut second_connection) = raw_peer(&mut host, "com.example.two");
    let update = "{\"type\":\"appearance\"}\n";
    let marker = "{\"type\":\"message\",\"level\":\"info\",\"text\":\"done\"}\n";
    let burst = update.repeat(11) + marker;
    first.write_all(burst.as_bytes()).unwrap();
    second.write_all(burst.as_bytes()).unwrap();
    for connection in [&mut first_connection, &mut second_connection] {
        let mut updates = 0;
        loop {
            match receive_next(connection) {
                AddonMessage::Unknown => {}
                AddonMessage::Message { text, .. } => {
                    assert_eq!(text, "done");
                    break;
                }
                _ => updates += 1,
            }
        }
        assert_eq!(updates, 10);
        assert!(!connection.is_disconnected());
    }

    std::thread::sleep(Duration::from_secs(1));
    first.write_all(update.as_bytes()).unwrap();
    assert_ne!(receive_next(&mut first_connection), AddonMessage::Unknown);
}

#[test]
fn client_refuses_the_eleventh_appearance_update_in_a_rolling_second() {
    use backtracking_addon::protocol::decode_addon;

    let mut host = Host::bind("test").unwrap();
    host.expect_addon("com.example.theme", "secret", BTreeMap::new())
        .unwrap();
    let port = host.port();
    let worker = std::thread::spawn(move || {
        let mut client = Client::connect(
            port,
            "secret",
            std::env::temp_dir(),
            "com.example.theme",
            "1",
            Duration::from_secs(3),
        )
        .unwrap();
        let update =
            decode_addon(br##"{"type":"appearance","colors":{"waveform":"#67e7b5"}}"##).unwrap();
        for _ in 0..10 {
            client.send(&update).unwrap();
        }
        let error = client.send(&update).unwrap_err().to_string();
        assert!(error.contains("10 appearance"), "{error}");
        assert!(!client.is_disconnected());

        std::thread::sleep(Duration::from_secs(1));
        client.send(&update).unwrap();
        client
            .send(&AddonMessage::Message {
                level: MessageLevel::Info,
                text: "still connected".into(),
            })
            .unwrap();
    });
    let mut connection = wait_connected(&mut host);
    for _ in 0..11 {
        assert_ne!(receive_next(&mut connection), AddonMessage::Unknown);
    }
    assert!(matches!(
        receive_next(&mut connection),
        AddonMessage::Message { text, .. } if text == "still connected"
    ));
    worker.join().unwrap();
}

use backtracking_addon::client::Client;
use backtracking_addon::host::{Connection, Host, HostEvent};
use backtracking_addon::protocol::{
    encode_addon, AddonMessage, Appearance, HostMessage, MessageLevel, TransportState,
};
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

fn appearance_update(step: u8) -> AddonMessage {
    AddonMessage::Appearance(Appearance {
        colors: BTreeMap::from([("waveform".into(), [step, 231, 181, 255])]),
        ..Appearance::default()
    })
}

fn send_appearance_burst(peer: &mut TcpStream, steps: std::ops::RangeInclusive<u8>) {
    let mut burst = Vec::new();
    for step in steps {
        burst.extend(encode_addon(&appearance_update(step)).unwrap());
    }
    peer.write_all(&burst).unwrap();
}

fn receive_appearance_burst(connection: &mut Connection, steps: std::ops::RangeInclusive<u8>) {
    for (index, step) in steps.enumerate() {
        let expected = if index < 10 {
            appearance_update(step)
        } else {
            AddonMessage::Unknown
        };
        assert_eq!(receive_next(connection), expected);
    }
}

#[test]
fn host_coalesces_appearance_floods_to_the_latest_update_per_addon() {
    let mut host = Host::bind("test").unwrap();
    let (mut first, mut first_connection) = raw_peer(&mut host, "com.example.one");
    let (mut second, mut second_connection) = raw_peer(&mut host, "com.example.two");
    send_appearance_burst(&mut first, 1..=25);
    send_appearance_burst(&mut second, 26..=50);
    receive_appearance_burst(&mut first_connection, 1..=25);
    receive_appearance_burst(&mut second_connection, 26..=50);

    assert_eq!(receive_next(&mut first_connection), appearance_update(25));
    assert_eq!(receive_next(&mut second_connection), appearance_update(50));
    assert!(first_connection.try_recv().unwrap().is_none());
    assert!(second_connection.try_recv().unwrap().is_none());
}

#[test]
fn host_delivers_a_quiet_addons_late_appearance_update() {
    let mut host = Host::bind("test").unwrap();
    let (mut peer, mut connection) = raw_peer(&mut host, "com.example.theme");
    send_appearance_burst(&mut peer, 1..=10);
    receive_appearance_burst(&mut connection, 1..=10);
    peer.write_all(&encode_addon(&appearance_update(11)).unwrap())
        .unwrap();
    assert_eq!(receive_next(&mut connection), AddonMessage::Unknown);
    std::thread::sleep(Duration::from_secs(1));

    assert_eq!(connection.try_recv().unwrap(), Some(appearance_update(11)));
}

#[test]
fn host_delivers_a_due_appearance_with_other_messages_waiting() {
    let mut host = Host::bind("test").unwrap();
    let (mut peer, mut connection) = raw_peer(&mut host, "com.example.theme");
    send_appearance_burst(&mut peer, 1..=11);
    receive_appearance_burst(&mut connection, 1..=11);
    let message = AddonMessage::Message {
        level: MessageLevel::Info,
        text: "still connected".into(),
    };
    peer.write_all(&encode_addon(&message).unwrap()).unwrap();
    std::thread::sleep(Duration::from_secs(1));

    assert_eq!(receive_next(&mut connection), appearance_update(11));
    assert_eq!(receive_next(&mut connection), message);
}

#[test]
fn host_never_delivers_an_older_pending_appearance_after_a_new_update() {
    let mut host = Host::bind("test").unwrap();
    let (mut peer, mut connection) = raw_peer(&mut host, "com.example.theme");
    send_appearance_burst(&mut peer, 1..=11);
    receive_appearance_burst(&mut connection, 1..=11);
    peer.write_all(&encode_addon(&appearance_update(12)).unwrap())
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));

    assert_eq!(receive_next(&mut connection), appearance_update(12));
    assert!(connection.try_recv().unwrap().is_none());
}

#[test]
fn host_discards_appearance_updates_on_disconnect() {
    let mut host = Host::bind("test").unwrap();
    let (mut local_peer, mut local) = raw_peer(&mut host, "com.example.local");
    let (mut remote_peer, mut remote) = raw_peer(&mut host, "com.example.remote");
    send_appearance_burst(&mut local_peer, 1..=12);
    send_appearance_burst(&mut remote_peer, 1..=11);
    receive_appearance_burst(&mut local, 1..=11);
    receive_appearance_burst(&mut remote, 1..=11);

    local.close();
    remote_peer.shutdown(std::net::Shutdown::Write).unwrap();
    std::thread::sleep(Duration::from_secs(1));

    for connection in [&mut local, &mut remote] {
        assert!(connection.try_recv().unwrap().is_none());
        assert!(connection.is_disconnected());
        assert!(connection.try_recv().unwrap().is_none());
    }
}

#[test]
fn client_refuses_the_eleventh_appearance_update_in_a_rolling_second() {
    use backtracking_addon::protocol::decode_addon;

    let mut host = Host::bind("test").unwrap();
    host.expect_addon("com.example.theme", "secret", BTreeMap::new())
        .unwrap();
    let port = host.port();
    let (burst_read, wait_for_burst) = mpsc::channel();
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

        // Let both rolling windows expire, starting after the host reads the burst.
        wait_for_burst.recv_timeout(Duration::from_secs(3)).unwrap();
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
    for _ in 0..10 {
        assert!(matches!(
            receive_next(&mut connection),
            AddonMessage::Appearance(_)
        ));
    }
    burst_read.send(()).unwrap();
    assert!(matches!(
        receive_next(&mut connection),
        AddonMessage::Appearance(_)
    ));
    assert!(matches!(
        receive_next(&mut connection),
        AddonMessage::Message { text, .. } if text == "still connected"
    ));
    worker.join().unwrap();
}

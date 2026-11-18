//! Nonblocking host listener and authenticated add-on connections.
use crate::protocol::{self, AddonMessage, ChannelPosition, HostMessage, Source};
use crate::transport::Wire;
use crate::{Error, Result};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::net::TcpListener;
use std::time::{Duration, Instant};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PENDING: usize = 128;
const ACCEPT_BUDGET: usize = 32;

struct Expected {
    token: String,
    settings: BTreeMap<String, Value>,
}
struct Pending {
    wire: Wire,
    started: Instant,
    rejecting: bool,
}

/// A listener that never waits for a peer during [`Host::poll`].
pub struct Host {
    listener: TcpListener,
    expected: BTreeMap<String, Expected>,
    pending: Vec<Pending>,
    app_version: String,
}

/// A completed handshake or a failed incoming connection.
pub enum HostEvent {
    Connected(Connection),
    Rejected { id: Option<String>, reason: String },
}

/// An authenticated connection. Poll it regularly to flush queued writes.
pub struct Connection {
    id: String,
    wire: Wire,
}

/// Bounds and validates position updates independently for each declared source.
#[derive(Default)]
pub struct PositionFilter {
    received: BTreeMap<String, VecDeque<Instant>>,
}

impl PositionFilter {
    /// Thin excess updates without treating them as a connection failure.
    /// `now_ns` uses the Sender packet clock; `received_at` measures the receive rate.
    pub fn accept(
        &mut self,
        source: &Source,
        time: u64,
        now_ns: u64,
        received_at: Instant,
        mut channels: Vec<ChannelPosition>,
    ) -> Option<Vec<ChannelPosition>> {
        if time > now_ns.saturating_add(5_000_000_000) {
            return None;
        }
        let received = self.received.entry(source.id.clone()).or_default();
        while received.front().is_some_and(|previous| {
            received_at.saturating_duration_since(*previous) >= Duration::from_secs(1)
        }) {
            received.pop_front();
        }
        if received.len() >= 50 {
            return None;
        }
        received.push_back(received_at);

        let mut positioned = [false; 64];
        let mut first = 0_usize;
        if let Some(groups) = &source.groups {
            for group in groups {
                let end = first.saturating_add(group.channels.len()).min(64);
                if group.positioned {
                    positioned[first.min(64)..end].fill(true);
                }
                first = end;
            }
        }
        channels.retain_mut(|position| {
            if !positioned
                .get(position.channel as usize)
                .copied()
                .unwrap_or(false)
                || !position.is_finite()
            {
                return false;
            }
            position.x = position.x.clamp(-1.0, 1.0);
            position.y = position.y.clamp(-1.0, 1.0);
            position.z = position.z.clamp(-1.0, 1.0);
            position.size = position.size.map(|size| size.clamp(0.0, 1.0));
            true
        });
        Some(channels)
    }

    /// Keep rate history only for sources in the latest `describe`.
    pub fn retain_sources(&mut self, sources: &[Source]) {
        self.received
            .retain(|id, _| sources.iter().any(|source| source.id == *id));
    }

    /// Forget rate history when an add-on starts a new run.
    pub fn clear(&mut self) {
        self.received.clear();
    }
}

impl Host {
    pub fn bind(app_version: impl Into<String>) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            expected: BTreeMap::new(),
            pending: Vec::new(),
            app_version: app_version.into(),
        })
    }

    pub fn port(&self) -> u16 {
        // A bound listener retains its local address until it is dropped.
        self.listener
            .local_addr()
            .expect("bound listener has an address")
            .port()
    }

    /// Authorize one connection for a process run. Generate tokens outside this crate.
    pub fn expect_addon(
        &mut self,
        id: &str,
        token: &str,
        settings: BTreeMap<String, Value>,
    ) -> Result<()> {
        crate::manifest::validate_id(id)?;
        if token.is_empty() {
            return Err(Error::new("The add-on token must not be empty"));
        }
        protocol::encode_host(&HostMessage::Welcome {
            contract: 1,
            app: "Backtracking".into(),
            app_version: self.app_version.clone(),
            settings: settings.clone(),
        })?;
        self.expected.insert(
            id.into(),
            Expected {
                token: token.into(),
                settings,
            },
        );
        Ok(())
    }

    pub fn forget_addon(&mut self, id: &str) {
        self.expected.remove(id);
    }

    /// Accept and advance a bounded number of handshakes. Returned connections are caller-owned.
    pub fn poll(&mut self) -> Result<Vec<HostEvent>> {
        accept_pending(&mut self.pending, || self.listener.accept())?;
        let mut events = Vec::new();
        for mut pending in std::mem::take(&mut self.pending) {
            if pending.started.elapsed() >= HANDSHAKE_TIMEOUT {
                pending.wire.close();
                events.push(HostEvent::Rejected {
                    id: None,
                    reason: "Add-on did not complete hello within 10 seconds".into(),
                });
                continue;
            }
            if pending.rejecting {
                if pending.wire.flush().is_ok() && !pending.wire.drained() {
                    self.pending.push(pending);
                }
                continue;
            }
            let line = match pending.wire.receive() {
                Ok(Some(line)) => line,
                Ok(None) => {
                    if !pending.wire.disconnected() {
                        self.pending.push(pending);
                    }
                    continue;
                }
                Err(e) => {
                    events.push(HostEvent::Rejected {
                        id: None,
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            let (id, reason) = match protocol::decode_addon(&line) {
                Ok(AddonMessage::Hello {
                    contract,
                    token,
                    id,
                    ..
                }) => {
                    let reason = if contract != 1 {
                        Some("Unsupported add-on contract; expected 1".to_string())
                    } else {
                        match self.expected.get(&id) {
                            None => Some(
                                "This add-on id is not expected or has already connected"
                                    .to_string(),
                            ),
                            Some(expected) if expected.token != token => {
                                Some("The add-on token is incorrect".to_string())
                            }
                            Some(_) => None,
                        }
                    };
                    if let Some(reason) = reason {
                        (Some(id), reason)
                    } else {
                        let expected = self.expected.get(&id).expect("authorized id exists");
                        let welcome = HostMessage::Welcome {
                            contract: 1,
                            app: "Backtracking".into(),
                            app_version: self.app_version.clone(),
                            settings: expected.settings.clone(),
                        };
                        if let Err(error) = protocol::encode_host(&welcome)
                            .and_then(|line| pending.wire.queue(line))
                        {
                            events.push(HostEvent::Rejected {
                                id: Some(id),
                                reason: error.to_string(),
                            });
                            continue;
                        }
                        self.expected.remove(&id);
                        events.push(HostEvent::Connected(Connection {
                            id,
                            wire: pending.wire,
                        }));
                        continue;
                    }
                }
                Ok(_) => (None, "The first message must be hello".to_string()),
                Err(e) => (None, e.to_string()),
            };
            // A parser error can quote a peer-controlled string almost as large as a line.
            let reason: String = reason.chars().take(1000).collect();
            let rejection = HostMessage::Rejected {
                reason: reason.clone(),
            };
            if protocol::encode_host(&rejection)
                .and_then(|line| pending.wire.queue(line))
                .is_ok()
                && !pending.wire.drained()
            {
                pending.rejecting = true;
                self.pending.push(pending);
            }
            events.push(HostEvent::Rejected { id, reason });
        }
        Ok(events)
    }
}

fn accept_pending(
    pending: &mut Vec<Pending>,
    mut accept: impl FnMut() -> std::io::Result<(std::net::TcpStream, std::net::SocketAddr)>,
) -> Result<()> {
    for _ in 0..ACCEPT_BUDGET {
        match accept() {
            Ok((stream, _)) => {
                if pending.len() < MAX_PENDING {
                    if let Ok(wire) = Wire::new(stream) {
                        pending.push(Pending {
                            wire,
                            started: Instant::now(),
                            rejecting: false,
                        });
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => break,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) =>
            {
                // A peer can disconnect before accept without damaging the listener.
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

impl Connection {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Queue one line and try to send it. A full queue returns an error without blocking.
    pub fn send(&mut self, message: &HostMessage) -> Result<()> {
        self.wire.queue(protocol::encode_host(message)?)
    }

    /// Receive at most one message, or return immediately when none is ready.
    pub fn try_recv(&mut self) -> Result<Option<AddonMessage>> {
        let Some(line) = self.wire.receive()? else {
            return Ok(None);
        };
        let message = match protocol::decode_addon(&line) {
            Ok(message) => message,
            Err(e) => {
                self.wire.close();
                return Err(e);
            }
        };
        Ok(Some(message))
    }

    /// Reflects closure observed by the most recent send or receive attempt.
    pub fn is_disconnected(&self) -> bool {
        self.wire.disconnected()
    }
    pub fn close(&mut self) {
        self.wire.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    fn positioned_source() -> Source {
        Source {
            id: "main".into(),
            name: "Objects".into(),
            sender: "a-b".into(),
            formats: vec![protocol::Format {
                id: "objects".into(),
                label: "Objects".into(),
            }],
            format: "objects".into(),
            groups: Some(vec![
                protocol::ChannelGroup {
                    id: "channels".into(),
                    label: "Channels".into(),
                    channels: vec!["L".into(), "R".into()],
                    positioned: false,
                },
                protocol::ChannelGroup {
                    id: "objects".into(),
                    label: "Objects".into(),
                    channels: vec!["Object 1".into(), "Object 2".into()],
                    positioned: true,
                },
            ]),
            transport: None,
        }
    }

    #[test]
    fn positions_drop_unpositioned_outside_and_nonfinite_entries() {
        let source = positioned_source();
        let mut filter = PositionFilter::default();
        let mut entries: Vec<_> = (0..5)
            .map(|channel| ChannelPosition::new(channel, 0.0, 1.0, 0.0))
            .collect();
        entries[3].gain = Some(f64::INFINITY);
        let kept = filter
            .accept(&source, 100, 100, Instant::now(), entries)
            .unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].channel, 2);
        for field in 0..5 {
            let mut entry = ChannelPosition::new(2, 0.0, 1.0, 0.0);
            match field {
                0 => entry.x = f64::NAN,
                1 => entry.y = f64::INFINITY,
                2 => entry.z = f64::NEG_INFINITY,
                3 => entry.size = Some(f64::NAN),
                _ => entry.gain = Some(f64::NAN),
            }
            assert!(filter
                .accept(&source, 100, 100, Instant::now(), vec![entry])
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn positions_clamp_coordinates_and_size_but_keep_gain_and_defaults() {
        let source = positioned_source();
        let mut filter = PositionFilter::default();
        let mut a = ChannelPosition::new(2, -3.0, 2.0, -2.0);
        a.size = Some(4.0);
        a.gain = Some(2.5);
        let mut b = ChannelPosition::new(3, 3.0, -2.0, 2.0);
        b.size = Some(-4.0);
        let kept = filter
            .accept(&source, 0, 0, Instant::now(), vec![a, b])
            .unwrap();
        assert_eq!(
            (kept[0].x, kept[0].y, kept[0].z, kept[0].size, kept[0].gain),
            (-1.0, 1.0, -1.0, Some(1.0), Some(2.5))
        );
        assert_eq!(
            (kept[1].x, kept[1].y, kept[1].z, kept[1].size, kept[1].gain),
            (1.0, -1.0, 1.0, Some(0.0), None)
        );
        let kept = filter
            .accept(
                &source,
                0,
                0,
                Instant::now(),
                vec![ChannelPosition::new(2, 0.0, 0.0, 0.0)],
            )
            .unwrap();
        assert_eq!((kept[0].size, kept[0].gain), (None, None));
    }

    #[test]
    fn positions_without_positioned_groups_keep_no_channels() {
        let mut source = positioned_source();
        let mut filter = PositionFilter::default();
        source.groups = None;
        assert!(filter
            .accept(
                &source,
                0,
                0,
                Instant::now(),
                vec![ChannelPosition::new(2, 0.0, 0.0, 0.0)]
            )
            .unwrap()
            .is_empty());
        source.groups = positioned_source().groups;
        source.groups.as_mut().unwrap()[1].positioned = false;
        assert!(filter
            .accept(
                &source,
                0,
                0,
                Instant::now(),
                vec![ChannelPosition::new(2, 0.0, 0.0, 0.0)]
            )
            .unwrap()
            .is_empty());
    }

    #[test]
    fn positions_five_second_future_limit_includes_boundary_and_saturates() {
        let source = positioned_source();
        let mut filter = PositionFilter::default();
        let now = Instant::now();
        assert!(filter
            .accept(&source, 5_000_000_100, 100, now, vec![])
            .is_some());
        assert!(filter
            .accept(&source, 5_000_000_101, 100, now, vec![])
            .is_none());
        assert!(filter
            .accept(&source, u64::MAX, u64::MAX - 10, now, vec![])
            .is_some());
        assert!(filter.accept(&source, 0, 100, now, vec![]).is_some());
    }

    #[test]
    fn positions_flood_is_thinned_per_source_in_a_rolling_second() {
        let mut source = positioned_source();
        let mut filter = PositionFilter::default();
        let now = Instant::now();
        for _ in 0..50 {
            assert!(filter.accept(&source, 0, 0, now, vec![]).is_some());
        }
        assert!(filter
            .accept(&source, 0, 0, now + Duration::from_millis(999), vec![])
            .is_none());
        source.id = "other".into();
        assert!(filter.accept(&source, 0, 0, now, vec![]).is_some());
        source.id = "main".into();
        assert!(filter
            .accept(&source, 0, 0, now + Duration::from_secs(1), vec![])
            .is_some());
        filter.clear();
        assert!(filter
            .accept(&source, 0, 0, now + Duration::from_secs(1), vec![])
            .is_some());
    }

    #[test]
    fn positions_describe_keeps_existing_rate_history_and_forgets_removed_sources() {
        let source = positioned_source();
        let now = Instant::now();
        let mut filter = PositionFilter::default();
        for _ in 0..50 {
            filter.accept(&source, 0, 0, now, vec![]).unwrap();
        }
        filter.retain_sources(std::slice::from_ref(&source));
        assert!(filter.accept(&source, 0, 0, now, vec![]).is_none());
        filter.retain_sources(&[]);
        assert!(filter.received.is_empty());
    }

    #[test]
    fn malformed_positions_do_not_close_the_connection() {
        use std::io::Write;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut connection = Connection {
            id: "com.example.test-tone".into(),
            wire: Wire::new(stream).unwrap(),
        };
        let lines = [
            r#"{"type":"positions","time":-1}"#,
            r#"{"type":"positions","source":"main","time":0,"channels":[{"channel":2,"x":null,"y":0,"z":0},{"channel":3,"x":-2,"y":2,"z":0,"size":null}]}"#,
            r#"{"type":"message","level":"info","text":"Still running"}"#,
        ];
        for line in lines {
            writeln!(peer, "{line}").unwrap();
        }
        let mut filter = PositionFilter::default();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut received = 0;
        while received < 3 && Instant::now() < deadline {
            match connection.try_recv().unwrap() {
                Some(AddonMessage::Positions { time, channels, .. }) => {
                    assert!(filter
                        .accept(&positioned_source(), time, 0, Instant::now(), channels)
                        .unwrap()
                        .is_empty());
                    received += 1;
                }
                Some(AddonMessage::Message { text, .. }) => {
                    assert_eq!(text, "Still running");
                    received += 1;
                }
                Some(AddonMessage::Unknown) if received == 0 => received += 1,
                Some(_) => panic!("unexpected message"),
                None => std::thread::yield_now(),
            }
        }
        assert_eq!(received, 3);
        assert!(!connection.is_disconnected());
    }

    #[test]
    fn positions_receive_cost_16_objects() {
        let mut source = positioned_source();
        source.groups = Some(vec![protocol::ChannelGroup {
            id: "objects".into(),
            label: "Objects".into(),
            channels: (0..16).map(|i| format!("Object {i}")).collect(),
            positioned: true,
        }]);
        let message = AddonMessage::Positions {
            source: source.id.clone(),
            time: 0,
            channels: (0..16)
                .map(|channel| ChannelPosition::new(channel, -0.5, 0.8, 0.0))
                .collect(),
        };
        let line = protocol::encode_addon(&message).unwrap();
        let mut filter = PositionFilter::default();
        let received_at = Instant::now();
        let started = Instant::now();
        let messages = 31 * 600;
        for i in 0..messages {
            let AddonMessage::Positions { time, channels, .. } =
                protocol::decode_addon(std::hint::black_box(&line)).unwrap()
            else {
                unreachable!()
            };
            let kept = filter
                .accept(
                    &source,
                    time,
                    0,
                    received_at + Duration::from_nanos(i * 1_000_000_000 / 31),
                    channels,
                )
                .unwrap();
            std::hint::black_box(kept);
        }
        let elapsed = started.elapsed();
        println!("positions receive parse+filter: {messages} messages, {} bytes/message, {:.3} ms total, {:.3} us/message, {:.3} ms/second at 31 Hz", line.len(), elapsed.as_secs_f64() * 1000.0, elapsed.as_secs_f64() * 1_000_000.0 / messages as f64, elapsed.as_secs_f64() * 1000.0 * 31.0 / messages as f64);
    }

    #[test]
    fn reset_or_aborted_peer_does_not_fail_accepting_the_next_peer() {
        for kind in [ErrorKind::ConnectionReset, ErrorKind::ConnectionAborted] {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
            let _peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            listener.set_nonblocking(true).unwrap();
            let mut calls = 0;
            let mut pending = Vec::new();
            accept_pending(&mut pending, || {
                calls += 1;
                if calls == 1 {
                    Err(std::io::Error::from(kind))
                } else {
                    listener.accept()
                }
            })
            .unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(calls, 3);
        }
    }

    #[test]
    fn reset_peers_still_obey_the_accept_budget() {
        let mut calls = 0;
        accept_pending(&mut Vec::new(), || {
            calls += 1;
            Err(std::io::Error::from(ErrorKind::ConnectionReset))
        })
        .unwrap();
        assert_eq!(calls, ACCEPT_BUDGET);
    }

    #[test]
    fn listener_failures_are_still_reported() {
        let result = accept_pending(&mut Vec::new(), || {
            Err(std::io::Error::from(ErrorKind::PermissionDenied))
        });
        assert!(result.is_err());
    }
}

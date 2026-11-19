//! Nonblocking host listener and authenticated add-on connections.
use crate::protocol::{self, AddonMessage, HostMessage};
use crate::transport::Wire;
use crate::{Error, Result};
use serde_json::Value;
use std::collections::BTreeMap;
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

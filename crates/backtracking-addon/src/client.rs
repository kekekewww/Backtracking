//! A synchronous connection helper for add-on authors.
use crate::protocol::{self, AddonMessage, HostMessage};
use crate::transport::Wire;
use crate::{Error, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct Client {
    wire: Wire,
    settings: BTreeMap<String, Value>,
    data_dir: PathBuf,
    transport_updates: std::collections::VecDeque<Instant>,
    /// When `positions` were sent in the last second, for each source.
    position_updates: BTreeMap<String, std::collections::VecDeque<Instant>>,
}

impl Client {
    /// Read all three environment variables and complete hello/welcome within ten seconds.
    pub fn connect_from_env(id: &str, version: &str) -> Result<Self> {
        let port = std::env::var("BACKTRACKING_ADDON_PORT")
            .map_err(|_| Error::new("Set BACKTRACKING_ADDON_PORT to the host's loopback port"))?
            .parse::<u16>()
            .map_err(|_| {
                Error::new("BACKTRACKING_ADDON_PORT must be a TCP port from 1 to 65535")
            })?;
        let token = std::env::var("BACKTRACKING_ADDON_TOKEN")
            .map_err(|_| Error::new("Set BACKTRACKING_ADDON_TOKEN to the host's token"))?;
        let data_dir = std::env::var_os("BACKTRACKING_ADDON_DATA").ok_or_else(|| {
            Error::new("Set BACKTRACKING_ADDON_DATA to the existing writable data folder")
        })?;
        Self::connect(
            port,
            &token,
            PathBuf::from(data_dir),
            id,
            version,
            Duration::from_secs(10),
        )
    }

    /// Connect explicitly, useful when the caller already owns its launch configuration.
    pub fn connect(
        port: u16,
        token: &str,
        data_dir: impl Into<PathBuf>,
        id: &str,
        version: &str,
        timeout: Duration,
    ) -> Result<Self> {
        if port == 0 {
            return Err(Error::new("The host port must be from 1 to 65535"));
        }
        if token.is_empty() {
            return Err(Error::new("The add-on token must not be empty"));
        }
        if timeout.is_zero() {
            return Err(Error::new(
                "The handshake timeout must be greater than zero",
            ));
        }
        let data_dir = data_dir.into();
        if !data_dir.is_dir() {
            return Err(Error::new(
                "BACKTRACKING_ADDON_DATA must name an existing data folder",
            ));
        }
        let hello = protocol::encode_addon(&AddonMessage::Hello {
            contract: 1,
            token: token.into(),
            id: id.into(),
            version: version.into(),
        })?;
        let started = Instant::now();
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut wire = Wire::new(TcpStream::connect_timeout(&address, timeout)?)?;
        wire.queue(hello)?;
        loop {
            if let Some(line) = wire.receive()? {
                match protocol::decode_host(&line)? {
                    HostMessage::Welcome {
                        contract: 1,
                        settings,
                        ..
                    } => {
                        return Ok(Self {
                            wire,
                            settings,
                            data_dir,
                            transport_updates: std::collections::VecDeque::new(),
                            position_updates: BTreeMap::new(),
                        })
                    }
                    HostMessage::Welcome { .. } => {
                        return Err(Error::new("The host uses an unsupported add-on contract"))
                    }
                    HostMessage::Rejected { reason } => {
                        return Err(Error::new(format!("Host rejected the add-on: {reason}")))
                    }
                    HostMessage::Unknown => {}
                    _ => {
                        return Err(Error::new(
                            "The host must send welcome before other messages",
                        ))
                    }
                }
            }
            if wire.disconnected() {
                return Err(Error::new("The host disconnected before welcome"));
            }
            if started.elapsed() >= timeout {
                return Err(Error::new("Timed out waiting for welcome from the host"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn settings(&self) -> &BTreeMap<String, Value> {
        &self.settings
    }
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn send(&mut self, message: &AddonMessage) -> Result<()> {
        let line = protocol::encode_addon(message)?;
        if matches!(message, AddonMessage::Transport { .. }) {
            let now = Instant::now();
            while self
                .transport_updates
                .front()
                .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(1))
            {
                self.transport_updates.pop_front();
            }
            if self.transport_updates.len() >= 4 {
                return Err(Error::new("Send at most 4 transport messages per second"));
            }
            self.wire.queue(line)?;
            self.transport_updates.push_back(now);
            Ok(())
        } else if let AddonMessage::Positions {
            source, channels, ..
        } = message
        {
            if !channels.iter().all(protocol::ChannelPosition::is_finite) {
                return Err(Error::new("Every number in positions must be finite"));
            }
            let now = Instant::now();
            let sent = self.position_updates.entry(source.clone()).or_default();
            while sent
                .front()
                .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(1))
            {
                sent.pop_front();
            }
            if sent.len() >= 50 {
                return Err(Error::new(
                    "Send at most 50 positions messages per second for one source",
                ));
            }
            self.wire.queue(line)?;
            sent.push_back(now);
            Ok(())
        } else {
            self.wire.queue(line)
        }
    }

    /// Says where the channels of a source's positioned groups are at `time`, in nanoseconds
    /// on the clock of the Sender packets: the moment in the audio at which the positions hold.
    /// List every positioned channel whose position is known, not only the ones that moved.
    pub fn send_positions(
        &mut self,
        source: &str,
        time: u64,
        channels: &[protocol::ChannelPosition],
    ) -> Result<()> {
        self.send(&AddonMessage::Positions {
            source: source.into(),
            time,
            channels: channels.to_vec(),
        })
    }

    /// Return immediately when no complete host message is available; also flush queued writes.
    pub fn try_recv(&mut self) -> Result<Option<HostMessage>> {
        self.wire
            .receive()?
            .map(|line| protocol::decode_host(&line))
            .transpose()
    }

    /// Wait for at most `timeout`, returning None on timeout or disconnection.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<Option<HostMessage>> {
        let started = Instant::now();
        loop {
            if let Some(message) = self.try_recv()? {
                return Ok(Some(message));
            }
            if self.is_disconnected() || started.elapsed() >= timeout {
                return Ok(None);
            }
            std::thread::sleep(
                Duration::from_millis(1).min(timeout.saturating_sub(started.elapsed())),
            );
        }
    }

    pub fn is_disconnected(&self) -> bool {
        self.wire.disconnected()
    }
}

use crate::{Error, Result};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};

// Bound both memory and the amount of socket work done during one UI poll.
const IO_BUDGET: usize = 65536;
const MAX_QUEUED_BYTES: usize = 1024 * 1024;

pub(crate) struct Wire {
    stream: TcpStream,
    input: VecDeque<u8>,
    output: VecDeque<Vec<u8>>,
    offset: usize,
    queued: usize,
    disconnected: bool,
}

impl Wire {
    pub(crate) fn new(stream: TcpStream) -> Result<Self> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            input: VecDeque::new(),
            output: VecDeque::new(),
            offset: 0,
            queued: 0,
            disconnected: false,
        })
    }

    pub(crate) fn queue(&mut self, line: Vec<u8>) -> Result<()> {
        if self.disconnected {
            return Err(Error::new("The other end of the connection has closed"));
        }
        if self.queued + line.len() > MAX_QUEUED_BYTES {
            return Err(Error::new(
                "The peer is not reading messages; outgoing queue is full",
            ));
        }
        self.queued += line.len();
        self.output.push_back(line);
        self.flush()
    }

    pub(crate) fn flush(&mut self) -> Result<()> {
        let mut remaining = IO_BUDGET;
        while remaining > 0 {
            let Some(front) = self.output.front() else {
                break;
            };
            let end = front.len().min(self.offset + remaining);
            match self.stream.write(&front[self.offset..end]) {
                Ok(0) => {
                    self.close();
                    return Err(Error::new("The peer closed while receiving a message"));
                }
                Ok(n) => {
                    self.offset += n;
                    self.queued -= n;
                    remaining -= n;
                    if self.offset == front.len() {
                        self.output.pop_front();
                        self.offset = 0;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => break,
                Err(e) => {
                    self.close();
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn receive(&mut self) -> Result<Option<Vec<u8>>> {
        if !self.disconnected {
            self.flush()?;
        }
        if let Some(line) = self.take_line()? {
            return Ok(Some(line));
        }
        if self.disconnected {
            return Ok(None);
        }
        let mut remaining = IO_BUDGET;
        let mut buffer = [0u8; 8192];
        while remaining > 0 {
            let size = buffer.len().min(remaining);
            match self.stream.read(&mut buffer[..size]) {
                Ok(0) => {
                    self.close();
                    if !self.input.is_empty() {
                        return Err(Error::new("The peer closed in the middle of a JSON line"));
                    }
                    return Ok(None);
                }
                Ok(n) => {
                    self.input.extend(&buffer[..n]);
                    remaining -= n;
                    if let Some(line) = self.take_line()? {
                        return Ok(Some(line));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => break,
                Err(e) => {
                    self.close();
                    return Err(e.into());
                }
            }
        }
        Ok(None)
    }

    fn take_line(&mut self) -> Result<Option<Vec<u8>>> {
        if let Some(end) = self.input.iter().position(|b| *b == b'\n') {
            if end + 1 > crate::protocol::MAX_LINE_BYTES {
                self.close();
                return Err(Error::new("Message exceeds the 65536-byte line limit"));
            }
            return Ok(Some(self.input.drain(..=end).collect()));
        }
        if self.input.len() >= crate::protocol::MAX_LINE_BYTES {
            self.close();
            return Err(Error::new("Message exceeds the 65536-byte line limit"));
        }
        Ok(None)
    }

    pub(crate) fn disconnected(&self) -> bool {
        self.disconnected
    }
    pub(crate) fn drained(&self) -> bool {
        self.output.is_empty()
    }
    pub(crate) fn close(&mut self) {
        self.disconnected = true;
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

//! Shared contract types and local connections for Backtracking add-ons.

pub mod client;
pub mod host;
pub mod manifest;
#[cfg(feature = "install")]
pub mod package;
pub mod protocol;
mod transport;

/// An invalid package, message, or failed local operation.
#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self(format!("Invalid JSON: {value}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

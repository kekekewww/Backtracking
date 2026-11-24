//! How the MCP side, on Tokio's threads, hands control requests to the one thread that owns
//! the connection to Backtracking, and gets each reply back.

use std::sync::mpsc;
use std::time::Duration;

use backtracking_addon::protocol::{ControlCommand, ControlError};
use serde_json::Value;
use tokio::sync::oneshot;

/// How one request ended.
#[derive(Debug)]
pub enum Outcome {
    /// Backtracking carried it out; its raw `result`.
    Done(Value),
    /// Backtracking answered with an error.
    Refused(ControlError),
    /// The request never got an answer from Backtracking: the add-on is stopping, the
    /// connection closed, or the reply did not come in time. The text is for the model.
    Lost(String),
}

/// One request waiting for the connection thread.
pub struct Job {
    pub command: ControlCommand,
    pub reply: oneshot::Sender<Outcome>,
}

/// A cheap handle the MCP tools use to reach Backtracking.
#[derive(Clone)]
pub struct Bridge {
    jobs: mpsc::Sender<Job>,
}

/// How much longer than the host's own bound the tools wait, beyond what the connection
/// thread already allows, so that its own timeout answer arrives first.
const BACKSTOP: Duration = Duration::from_secs(10);

impl Bridge {
    pub fn new() -> (Self, mpsc::Receiver<Job>) {
        let (jobs, receiver) = mpsc::channel();
        (Self { jobs }, receiver)
    }

    /// Send `command` to Backtracking and wait for how it ended.
    pub async fn run(&self, command: ControlCommand) -> Outcome {
        let wait = command.reply_within() + crate::addon::REPLY_MARGIN + BACKSTOP;
        let (reply, answer) = oneshot::channel();
        if self.jobs.send(Job { command, reply }).is_err() {
            return Outcome::Lost(STOPPING.into());
        }
        match tokio::time::timeout(wait, answer).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Outcome::Lost(STOPPING.into()),
            Err(_) => Outcome::Lost(
                "Backtracking did not answer in time. Call get_state to see whether it happened."
                    .into(),
            ),
        }
    }
}

pub const STOPPING: &str =
    "Backtracking Agent is stopping (Backtracking is closing, or the add-on was switched off).";

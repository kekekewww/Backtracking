use crate::{
    addons::{AddonHandle, AddonManager, AddonState},
    commands::{Pending, Request, dispatch},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
struct Shared {
    closing: OnceLock<Instant>,
    finished: Mutex<bool>,
    finished_changed: Condvar,
    generation: AtomicU64,
    manager: Mutex<Result<Option<AddonHandle>, String>>,
    tickets: Mutex<BTreeMap<u64, Option<Value>>>,
}
pub struct BtAddonHost {
    shared: Arc<Shared>,
    tx: mpsc::SyncSender<(u64, Request)>,
    next: AtomicU64,
}
pub(crate) fn failure(code: &str, message: &str) -> Value {
    json!({"schema":1,"ok":false,"code":code,"message":message})
}
fn success(v: Value) -> Value {
    json!({"schema":1,"ok":true,"value":v})
}
impl BtAddonHost {
    pub(crate) fn new(root: PathBuf, version: String, role: u32) -> Option<Self> {
        let shared = Arc::new(Shared {
            closing: OnceLock::new(),
            finished: Mutex::new(false),
            finished_changed: Condvar::new(),
            generation: AtomicU64::new(0),
            manager: Mutex::new(Ok(None)),
            tickets: Mutex::new(BTreeMap::new()),
        });
        let (tx, rx) = mpsc::sync_channel(128);
        let state = shared.clone();
        std::thread::Builder::new()
            .name("addon API".into())
            .spawn(move || {
                state.run(root, version, role, rx);
                *state.finished.lock().unwrap() = true;
                state.finished_changed.notify_all();
            })
            .ok()?;
        Some(Self {
            shared,
            tx,
            next: AtomicU64::new(1),
        })
    }
    fn request_shutdown(&self) {
        self.shared
            .closing
            .get_or_init(|| Instant::now() + crate::addons::QUIT_TIMEOUT);
    }

    pub(crate) fn shutdown(&self, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        self.request_shutdown();
        let mut finished = self.shared.finished.lock().unwrap();
        while !*finished {
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            // Darwin can coalesce a long timed wait past its requested deadline.
            // Approach the same monotonic deadline with progressively shorter waits.
            let wait = if remaining > Duration::from_millis(1) {
                remaining - remaining / 4
            } else {
                remaining
            };
            (finished, _) = self
                .shared
                .finished_changed
                .wait_timeout(finished, wait)
                .unwrap();
        }
        *finished
    }

    pub(crate) fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }
    pub(crate) fn snapshot(&self) -> Value {
        let generation = self.generation();
        let state = self.shared.manager.lock().unwrap().clone();
        let completed = self
            .shared
            .tickets
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(id, r)| r.as_ref().map(|_| *id))
            .collect::<Vec<_>>();
        let mut v = json!({"schema":1,"generation":generation,"completed":completed,"ready":false,"error":null,"addons":[],"appearances":{}});
        match state {
            Ok(Some(h)) => {
                let (s, looks) = h.published();
                v["ready"] = true.into();
                v["error"] = json!(s.error);
                v["appearances"] = json!(looks);
                v["addons"] = Value::Array(
                    s.addons
                        .iter()
                        .map(|a| {
                            let (name, reason) = match &a.state {
                                AddonState::Stopped => ("stopped", None),
                                AddonState::Running => ("running", None),
                                AddonState::Starting => ("starting", None),
                                AddonState::Stopping => ("stopping", None),
                                AddonState::NotResponding => ("not_responding", None),
                                AddonState::Failed(e) => ("failed", Some(e)),
                                AddonState::BrokenPackage(e) => ("broken_package", Some(e)),
                            };
                            let mut value = json!(a);
                            value["state"] = name.into();
                            value["reason"] = json!(reason);
                            value
                        })
                        .collect(),
                )
            }
            Err(e) => v["error"] = e.into(),
            Ok(None) => {}
        }
        v
    }
    pub(crate) fn command(&self, value: Value) -> Value {
        if self.shared.closing.get().is_some() {
            return failure("failed", "The add-on manager is shutting down");
        }
        if value["schema"].as_u64() != Some(1) {
            return failure("invalid_request", "Unsupported command schema");
        }
        let request = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(e) => return failure("invalid_request", &e.to_string()),
        };
        if let Request::TakeResult { ticket } = request {
            let mut tickets = self.shared.tickets.lock().unwrap();
            return match tickets.get(&ticket) {
                Some(Some(_)) => {
                    let result = tickets.remove(&ticket).unwrap().unwrap();
                    self.shared.generation.fetch_add(1, Ordering::Release);
                    result
                }
                Some(None) => failure("busy", "Command is pending"),
                None => failure("not_found", "Unknown command ticket"),
            };
        }
        let h = match self.shared.manager.lock().unwrap().clone() {
            Ok(Some(h)) => h,
            Ok(None) => return failure("busy", "Host is starting"),
            Err(e) => return failure("failed", &e),
        };
        match request {
            Request::PositionsSince { mut cursor } => {
                let positions = h.positions_since(&mut cursor);
                return success(json!({"cursor":cursor,"positions":positions}));
            }
            Request::TakeControl => return success(json!(h.take_control_calls())),
            Request::ControlReply {
                addon_id,
                run,
                request_id,
                result,
                error,
            } => {
                let outcome = match (result, error) {
                    (Some(r), None) => Ok(r),
                    (None, Some(e)) => Err(e),
                    _ => return failure("invalid_request", "Expected one control result or error"),
                };
                return match h.answer_control(addon_id, run, request_id, outcome) {
                    Ok(()) => success(Value::Null),
                    Err(e) => failure("failed", &e),
                };
            }
            _ => {}
        }
        let mut tickets = self.shared.tickets.lock().unwrap();
        if tickets.len() >= 128 {
            return failure("busy", "Too many pending command results");
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        if self.tx.try_send((id, request)).is_err() {
            return failure("busy", "Host queue is unavailable");
        }
        tickets.insert(id, None);
        json!({"schema":1,"ok":true,"ticket":id})
    }
}
impl Drop for BtAddonHost {
    fn drop(&mut self) {
        self.request_shutdown();
        // Dropping tx wakes the worker; its Arc keeps shared state alive.
    }
}

impl Shared {
    fn run(
        self: &Arc<Self>,
        root: PathBuf,
        version: String,
        role: u32,
        rx: mpsc::Receiver<(u64, Request)>,
    ) {
        if self.closing.get().is_some() {
            return;
        }
        let result = if role == 0 {
            AddonManager::new(root, &version)
        } else {
            AddonManager::connect_existing(root)
        };
        let mut manager = match result {
            Ok(manager) => manager,
            Err(e) => {
                *self.manager.lock().unwrap() = Err(e);
                self.generation.fetch_add(1, Ordering::Release);
                return;
            }
        };
        let handle = manager.handle();
        let weak = Arc::downgrade(self);
        let changed = Arc::new(move || {
            if let Some(s) = weak.upgrade() {
                s.generation.fetch_add(1, Ordering::Release);
            }
        });
        handle.subscribe(changed.clone());
        handle.subscribe_appearances(changed.clone());
        handle.subscribe_control(changed);
        *self.manager.lock().unwrap() = Ok(Some(handle.clone()));
        self.generation.fetch_add(1, Ordering::Release);
        let mut pending: Vec<(u64, Pending)> = Vec::new();
        while self.closing.get().is_none() {
            match rx.recv_timeout(Duration::from_millis(5)) {
                Ok((id, request)) => match dispatch(&handle, request) {
                    Ok(job) => pending.push((id, job)),
                    Err(e) => self.complete(id, Err(e)),
                },
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            pending.retain_mut(|(id, poll)| match poll() {
                None => true,
                Some(result) => {
                    self.complete(*id, result);
                    false
                }
            });
        }
        if let Some(&deadline) = self.closing.get() {
            if let Some(worker) = manager.stop_worker(deadline) {
                let _ = worker.join();
            }
        }
    }

    fn complete(&self, id: u64, result: Result<Value, String>) {
        let value = result.map_or_else(|e| failure("failed", &e), success);
        self.tickets.lock().unwrap().insert(id, Some(value));
        self.generation.fetch_add(1, Ordering::Release);
    }
}

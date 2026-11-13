use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

use eframe::egui::{Context, ViewportId};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        GetKeyboardLayout, MAPVK_VSC_TO_VK_EX, MOD_NOREPEAT, MOD_WIN, MapVirtualKeyExW,
        RegisterHotKey, UnregisterHotKey, VK_OEM_3,
    },
    WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, MSG, MWMO_INPUTAVAILABLE,
        MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_HOTKEY, WM_HOTKEY,
    },
};

const HOTKEY_ID: i32 = 1;
const GRAVE_SCAN_CODE: u32 = 0x29;

pub(crate) enum Event {
    Availability(Result<(), String>),
    Save,
}

pub(crate) struct SnapshotHotkey {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    events: Receiver<Event>,
}

impl SnapshotHotkey {
    pub(crate) fn start(context: Context) -> io::Result<Self> {
        let (sender, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("backtracking-snapshot-hotkey".into())
            .spawn(move || {
                let publish = |event| {
                    if sender.send(event).is_ok() {
                        context.request_repaint_of(ViewportId::ROOT);
                    }
                };
                let mut mapped_key = None;
                let mut registered = false;
                let mut message = MSG::default();
                while !worker_stop.load(Ordering::Relaxed) {
                    unsafe {
                        // A dedicated queue keeps WM_HOTKEY delivery independent of focus,
                        // egui's shortcut handling, and the visibility of the recorder.
                        while PeekMessageW(&mut message, None, WM_HOTKEY, WM_HOTKEY, PM_REMOVE)
                            .as_bool()
                        {
                            if message.wParam.0 == HOTKEY_ID as usize {
                                publish(Event::Save);
                            }
                        }
                        let foreground = GetForegroundWindow();
                        let layout = GetKeyboardLayout(GetWindowThreadProcessId(foreground, None));
                        let mapped = MapVirtualKeyExW(
                            GRAVE_SCAN_CODE,
                            MAPVK_VSC_TO_VK_EX,
                            Some(layout),
                        );
                        let key = if mapped == 0 { u32::from(VK_OEM_3.0) } else { mapped };
                        if mapped_key != Some(key) {
                            if registered {
                                let _ = UnregisterHotKey(None, HOTKEY_ID);
                            }
                            mapped_key = Some(key);
                            let result = RegisterHotKey(None, HOTKEY_ID, MOD_WIN | MOD_NOREPEAT, key);
                            registered = result.is_ok();
                            publish(Event::Availability(result.map_err(|error| error.to_string())));
                        }
                        MsgWaitForMultipleObjectsEx(None, 250, QS_HOTKEY, MWMO_INPUTAVAILABLE);
                    }
                }
                if registered {
                    let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
                }
            })?;
        Ok(Self { stop, worker: Some(worker), events })
    }

    pub(crate) fn try_next(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for SnapshotHotkey {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, VK_F11};

    fn registration(hotkey: &SnapshotHotkey) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(Event::Availability(result)) = hotkey.try_next() {
                return result;
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err("Windows did not report the registration result".into())
    }

    #[test]
    #[ignore = "temporarily reserves the real Win+grave shortcut in the Windows session"]
    fn win_grave_registers_reports_conflicts_and_is_released_on_exit() {
        // Distinguish a restricted/noninteractive desktop from a Win+grave collision.
        unsafe {
            let modifier = MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT;
            RegisterHotKey(None, HOTKEY_ID, modifier, u32::from(VK_F11.0))
                .expect("this Windows desktop must allow global hotkey registration");
            UnregisterHotKey(None, HOTKEY_ID).unwrap();
        }
        let first = SnapshotHotkey::start(Context::default()).unwrap();
        registration(&first).expect("Win+grave must be available in this Windows session");
        let second = SnapshotHotkey::start(Context::default()).unwrap();
        assert!(registration(&second).is_err());
        drop(second);
        drop(first);
        let restarted = SnapshotHotkey::start(Context::default()).unwrap();
        registration(&restarted).expect("the previous listener must release Win+grave");
    }
}

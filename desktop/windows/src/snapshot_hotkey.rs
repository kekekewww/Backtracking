use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use eframe::egui::{Context, ViewportId};
use windows::Win32::{
    Foundation::HANDLE,
    System::Threading::{CreateEventW, SetEvent},
    UI::{
        Input::KeyboardAndMouse::{
            GetKeyboardLayout, HOT_KEY_MODIFIERS, MAPVK_VSC_TO_VK_EX, MOD_NOREPEAT,
            MapVirtualKeyExW, RegisterHotKey, UnregisterHotKey, VK_OEM_3,
        },
        WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, MSG, MWMO_INPUTAVAILABLE,
            MsgWaitForMultipleObjectsEx, PM_NOREMOVE, PM_REMOVE, PeekMessageW, QS_HOTKEY,
            WM_HOTKEY,
        },
    },
};
use windows::core::PCWSTR;

use crate::hotkeys::{GlobalBinding, GlobalKey, Hotkeys, MAX_BINDINGS};

const HOTKEY_ID: i32 = 1;
const LAYOUT_POLL_MS: u32 = 250;

pub(crate) enum Event {
    Availability(Result<(), String>),
    Save,
}

#[derive(Clone, Copy)]
enum Command {
    Set([Option<GlobalBinding>; MAX_BINDINGS]),
    Suspend,
    Resume,
}

pub(crate) struct SnapshotHotkey {
    stop: Arc<AtomicBool>,
    wake: Arc<OwnedHandle>,
    worker: Option<JoinHandle<()>>,
    commands: Sender<Command>,
    events: Receiver<Event>,
}

impl SnapshotHotkey {
    pub(crate) fn start(context: Context) -> io::Result<Self> {
        Self::start_with(context, Hotkeys::defaults().global_bindings())
    }

    /// Starts the worker with the configured bindings, including an empty configuration.
    pub(crate) fn start_with(
        context: Context,
        initial: [Option<GlobalBinding>; MAX_BINDINGS],
    ) -> io::Result<Self> {
        Self::start_with_platform(context, initial, WindowsHotkeys)
    }

    fn start_with_platform(
        context: Context,
        initial: [Option<GlobalBinding>; MAX_BINDINGS],
        mut platform: impl HotkeyPlatform + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, events) = mpsc::channel();
        let (commands, pending) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map_err(io::Error::other)?;
        let wake = Arc::new(unsafe { OwnedHandle::from_raw_handle(handle.0) });
        let worker_wake = wake.clone();
        let worker = thread::Builder::new()
            .name("backtracking-snapshot-hotkey".into())
            .spawn(move || {
                let publish = |event| {
                    if sender.send(event).is_ok() {
                        context.request_repaint_of(ViewportId::ROOT);
                    }
                };
                let mut state = Worker::new(initial);
                let mut message = MSG::default();
                // Make the thread's message queue before its first registration.
                let _ =
                    unsafe { PeekMessageW(&mut message, None, WM_HOTKEY, WM_HOTKEY, PM_NOREMOVE) };
                let handles = [HANDLE(worker_wake.as_raw_handle())];
                if !worker_stop.load(Ordering::Relaxed)
                    && let Some(result) = state.reconcile(&mut platform, true)
                {
                    publish(Event::Availability(result));
                }
                while !worker_stop.load(Ordering::Relaxed) {
                    for command in pending.try_iter() {
                        if let Some(result) = state.apply(command, &mut platform) {
                            // UnregisterHotKey leaves messages already waiting in the queue.
                            if matches!(command, Command::Suspend) {
                                while take_hotkey_message(&mut message) {}
                            }
                            publish(Event::Availability(result));
                        }
                    }
                    while take_hotkey_message(&mut message) {
                        if state.accepts_message(message.wParam.0, message.lParam.0) {
                            publish(Event::Save);
                        }
                    }
                    if let Some(result) = state.refresh(&mut platform) {
                        publish(Event::Availability(result));
                    }
                    // Hotkey messages and command wakes are independent of the UI's focus.
                    unsafe {
                        MsgWaitForMultipleObjectsEx(
                            Some(&handles),
                            LAYOUT_POLL_MS,
                            QS_HOTKEY,
                            MWMO_INPUTAVAILABLE,
                        )
                    };
                }
                state.suspended = true;
                let _ = state.reconcile(&mut platform, true);
            })?;
        Ok(Self {
            stop,
            wake,
            worker: Some(worker),
            commands,
            events,
        })
    }

    pub(crate) fn try_next(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// Applies both alternatives together. A suspended worker applies them on resume.
    /// Availability reports Windows' result once the change has been applied.
    pub(crate) fn set_bindings(
        &self,
        bindings: [Option<GlobalBinding>; MAX_BINDINGS],
    ) -> io::Result<()> {
        self.send(Command::Set(bindings))
    }

    /// Requests release of the registrations during shortcut capture.
    pub(crate) fn suspend(&self) -> io::Result<()> {
        self.send(Command::Suspend)
    }

    /// Requests registration of the current bindings after capture ends.
    pub(crate) fn resume(&self) -> io::Result<()> {
        self.send(Command::Resume)
    }

    fn send(&self, command: Command) -> io::Result<()> {
        self.commands
            .send(command)
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        self.wake_worker()
    }

    fn wake_worker(&self) -> io::Result<()> {
        unsafe { SetEvent(HANDLE(self.wake.as_raw_handle())) }.map_err(io::Error::other)
    }
}

impl Drop for SnapshotHotkey {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.wake_worker();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn take_hotkey_message(message: &mut MSG) -> bool {
    unsafe { PeekMessageW(message, None, WM_HOTKEY, WM_HOTKEY, PM_REMOVE) }.as_bool()
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ResolvedBinding {
    modifiers: HOT_KEY_MODIFIERS,
    key: u32,
}

#[derive(Default)]
struct Registration {
    desired: Option<ResolvedBinding>,
    active: Option<ResolvedBinding>,
    error: Option<String>,
}

struct Worker {
    bindings: [Option<GlobalBinding>; MAX_BINDINGS],
    suspended: bool,
    registrations: [Registration; MAX_BINDINGS],
}

impl Worker {
    fn new(bindings: [Option<GlobalBinding>; MAX_BINDINGS]) -> Self {
        Self {
            bindings,
            suspended: false,
            registrations: Default::default(),
        }
    }

    fn apply(
        &mut self,
        command: Command,
        platform: &mut impl HotkeyPlatform,
    ) -> Option<Result<(), String>> {
        match command {
            Command::Set(bindings) => {
                self.bindings = bindings;
                if self.suspended && bindings.iter().any(Option::is_some) {
                    return None;
                }
            }
            Command::Suspend if !self.suspended => self.suspended = true,
            Command::Resume if self.suspended => self.suspended = false,
            _ => return None,
        }
        self.reconcile(platform, true)
    }

    fn refresh(&mut self, platform: &mut impl HotkeyPlatform) -> Option<Result<(), String>> {
        self.reconcile(platform, false)
    }

    fn reconcile(
        &mut self,
        platform: &mut impl HotkeyPlatform,
        force: bool,
    ) -> Option<Result<(), String>> {
        let mut desired = if self.suspended {
            [None; MAX_BINDINGS]
        } else {
            self.bindings.map(|binding| {
                binding.map(|binding| {
                    let key = match binding.key() {
                        GlobalKey::VirtualKey(key) => key,
                        GlobalKey::ScanCode(code) => match platform.map_scan_code(code) {
                            0 => u32::from(VK_OEM_3.0),
                            mapped => mapped,
                        },
                    };
                    ResolvedBinding {
                        modifiers: binding.modifiers(),
                        key,
                    }
                })
            })
        };
        // The physical and virtual alternatives may resolve to the same chord on this layout.
        if desired[0] == desired[1] {
            desired[1] = None;
        }
        let changed: [bool; MAX_BINDINGS] = std::array::from_fn(|index| {
            force || self.registrations[index].desired != desired[index]
        });
        if !changed.contains(&true) {
            return None;
        }

        // Release changing slots first so the two bindings can trade places.
        for (index, slot) in self.registrations.iter_mut().enumerate() {
            if !changed[index] {
                continue;
            }
            slot.desired = desired[index];
            slot.error = None;
            if slot.active.is_some() {
                match platform.unregister(HOTKEY_ID + index as i32) {
                    Ok(()) => slot.active = None,
                    Err(error) => slot.error = Some(error),
                }
            }
        }
        for (index, slot) in self.registrations.iter_mut().enumerate() {
            if changed[index]
                && slot.error.is_none()
                && let Some(binding) = slot.desired
            {
                match platform.register(HOTKEY_ID + index as i32, binding) {
                    Ok(()) => slot.active = Some(binding),
                    Err(error) => slot.error = Some(error),
                }
            }
        }
        Some(
            self.registrations
                .iter()
                .find_map(|slot| slot.error.clone())
                .map_or(Ok(()), Err),
        )
    }

    fn accepts_message(&self, id: usize, lparam: isize) -> bool {
        let binding = id
            .checked_sub(HOTKEY_ID as usize)
            .and_then(|index| self.registrations.get(index))
            .filter(|slot| slot.error.is_none())
            .and_then(|slot| slot.active);
        let Some(binding) = binding else {
            return false;
        };
        let packed = lparam as u32;
        let modifiers = packed & 0xffff & !MOD_NOREPEAT.0;
        !self.suspended
            && binding.key == packed >> 16
            && binding.modifiers.0 & !MOD_NOREPEAT.0 == modifiers
    }
}

trait HotkeyPlatform {
    fn map_scan_code(&mut self, scan_code: u32) -> u32;
    fn register(&mut self, id: i32, binding: ResolvedBinding) -> Result<(), String>;
    fn unregister(&mut self, id: i32) -> Result<(), String>;
}

struct WindowsHotkeys;

impl HotkeyPlatform for WindowsHotkeys {
    fn map_scan_code(&mut self, scan_code: u32) -> u32 {
        unsafe {
            let foreground = GetForegroundWindow();
            let layout = GetKeyboardLayout(GetWindowThreadProcessId(foreground, None));
            MapVirtualKeyExW(scan_code, MAPVK_VSC_TO_VK_EX, Some(layout))
        }
    }

    fn register(&mut self, id: i32, binding: ResolvedBinding) -> Result<(), String> {
        unsafe { RegisterHotKey(None, id, binding.modifiers, binding.key) }
            .map_err(|error| error.to_string())
    }

    fn unregister(&mut self, id: i32) -> Result<(), String> {
        unsafe { UnregisterHotKey(None, id) }.map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkeys::{Binding, Key, Modifiers};
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

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Operation {
        Map(u32),
        Register(i32, HOT_KEY_MODIFIERS, u32),
        Unregister(i32),
    }

    #[derive(Default)]
    struct FakePlatform {
        mapped_key: u32,
        refuse_key: Option<u32>,
        refuse_release: bool,
        operations: Vec<Operation>,
        observed_registrations: Option<Sender<Operation>>,
    }

    impl HotkeyPlatform for FakePlatform {
        fn map_scan_code(&mut self, scan_code: u32) -> u32 {
            self.operations.push(Operation::Map(scan_code));
            self.mapped_key
        }

        fn register(&mut self, id: i32, binding: ResolvedBinding) -> Result<(), String> {
            let operation = Operation::Register(id, binding.modifiers, binding.key);
            self.operations.push(operation);
            if let Some(observer) = &self.observed_registrations {
                observer.send(operation).unwrap();
            }
            if self.refuse_key == Some(binding.key) {
                return Err("already registered".into());
            }
            Ok(())
        }

        fn unregister(&mut self, id: i32) -> Result<(), String> {
            self.operations.push(Operation::Unregister(id));
            if self.refuse_release {
                return Err("could not release".into());
            }
            Ok(())
        }
    }

    fn ctrl(key: Key) -> GlobalBinding {
        Binding::Chord {
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
            key,
        }
        .to_win32()
        .unwrap()
    }

    fn message_param(modifiers: HOT_KEY_MODIFIERS, virtual_key: u32) -> isize {
        ((virtual_key << 16) | modifiers.0) as isize
    }

    fn startup_registrations(initial: [Option<GlobalBinding>; MAX_BINDINGS]) -> Vec<Operation> {
        let (observer, observations) = mpsc::channel();
        let platform = FakePlatform {
            observed_registrations: Some(observer),
            ..FakePlatform::default()
        };
        let hotkey =
            SnapshotHotkey::start_with_platform(Context::default(), initial, platform).unwrap();
        assert_eq!(registration(&hotkey), Ok(()));
        drop(hotkey);
        observations.into_iter().collect()
    }

    #[test]
    fn startup_registers_only_the_initial_bindings() {
        assert_eq!(
            startup_registrations([Some(ctrl(Key::S)), Some(ctrl(Key::Q))]),
            [
                Operation::Register(1, MOD_CONTROL | MOD_NOREPEAT, 0x53),
                Operation::Register(2, MOD_CONTROL | MOD_NOREPEAT, 0x51),
            ]
        );
    }

    #[test]
    fn startup_never_registers_a_shortcut_when_initial_bindings_are_empty() {
        assert!(startup_registrations([None; MAX_BINDINGS]).is_empty());
    }

    #[test]
    fn changing_and_clearing_the_binding_releases_the_old_registration() {
        let mut worker = Worker::new(Hotkeys::defaults().global_bindings());
        let mut platform = FakePlatform::default();
        assert_eq!(worker.refresh(&mut platform), Some(Ok(())));
        assert_eq!(
            platform.operations,
            [
                Operation::Map(0x29),
                Operation::Register(1, MOD_ALT | MOD_NOREPEAT, 0xc0)
            ]
        );
        platform.operations.clear();

        assert_eq!(
            worker.apply(Command::Set([Some(ctrl(Key::S)), None]), &mut platform),
            Some(Ok(()))
        );
        assert_eq!(
            platform.operations,
            [
                Operation::Unregister(1),
                Operation::Register(1, MOD_CONTROL | MOD_NOREPEAT, 0x53)
            ]
        );
        assert!(!worker.accepts_message(1, message_param(MOD_ALT, 0xc0)));
        assert!(worker.accepts_message(1, message_param(MOD_CONTROL, 0x53)));
        assert!(!worker.accepts_message(2, message_param(MOD_CONTROL, 0x53)));
        platform.operations.clear();

        assert_eq!(
            worker.apply(Command::Set([None; MAX_BINDINGS]), &mut platform),
            Some(Ok(()))
        );
        assert_eq!(platform.operations, [Operation::Unregister(1)]);
        assert!(!worker.accepts_message(1, message_param(MOD_CONTROL, 0x53)));
    }

    #[test]
    fn suspend_releases_both_bindings_and_resume_uses_the_latest_edit() {
        let mut worker = Worker::new([None; MAX_BINDINGS]);
        let mut platform = FakePlatform::default();
        let bindings = [Some(ctrl(Key::S)), Some(ctrl(Key::Q))];
        assert_eq!(
            worker.apply(Command::Set(bindings), &mut platform),
            Some(Ok(()))
        );
        assert!(worker.accepts_message(2, message_param(MOD_CONTROL, 0x51)));
        platform.operations.clear();

        assert_eq!(worker.apply(Command::Suspend, &mut platform), Some(Ok(())));
        assert!(!worker.accepts_message(2, message_param(MOD_CONTROL, 0x51)));
        assert_eq!(worker.apply(Command::Suspend, &mut platform), None);
        assert_eq!(
            worker.apply(Command::Set([Some(ctrl(Key::O)), None]), &mut platform),
            None
        );
        assert_eq!(worker.refresh(&mut platform), None);
        assert_eq!(
            platform.operations,
            [Operation::Unregister(1), Operation::Unregister(2)]
        );

        assert_eq!(worker.apply(Command::Resume, &mut platform), Some(Ok(())));
        assert_eq!(worker.apply(Command::Resume, &mut platform), None);
        assert_eq!(
            platform.operations.last(),
            Some(&Operation::Register(1, MOD_CONTROL | MOD_NOREPEAT, 0x4f))
        );
        assert!(worker.accepts_message(1, message_param(MOD_CONTROL, 0x4f)));
        assert!(!worker.accepts_message(2, message_param(MOD_CONTROL, 0x51)));
    }

    #[test]
    fn layout_updates_only_reregister_a_changed_scan_code_mapping() {
        let mut worker = Worker::new(Hotkeys::defaults().global_bindings());
        let mut platform = FakePlatform {
            mapped_key: 0xc0,
            ..FakePlatform::default()
        };
        let [scan, _] = Hotkeys::defaults().global_bindings();
        let virtual_grave = Binding::Chord {
            modifiers: Modifiers {
                alt: true,
                ..Modifiers::NONE
            },
            key: Key::Backtick,
        }
        .to_win32()
        .unwrap();
        assert_eq!(
            worker.apply(Command::Set([scan, Some(virtual_grave)]), &mut platform),
            Some(Ok(()))
        );
        assert_eq!(
            platform.operations,
            [
                Operation::Map(0x29),
                Operation::Register(1, MOD_ALT | MOD_NOREPEAT, 0xc0)
            ]
        );
        platform.operations.clear();

        platform.mapped_key = 0xde;
        assert_eq!(worker.refresh(&mut platform), Some(Ok(())));
        assert_eq!(
            platform.operations,
            [
                Operation::Map(0x29),
                Operation::Unregister(1),
                Operation::Register(1, MOD_ALT | MOD_NOREPEAT, 0xde),
                Operation::Register(2, MOD_ALT | MOD_NOREPEAT, 0xc0),
            ]
        );
        platform.operations.clear();
        assert_eq!(worker.refresh(&mut platform), None);
        assert_eq!(platform.operations, [Operation::Map(0x29)]);
    }

    #[test]
    fn a_failed_registration_reports_the_reason_and_can_be_retried() {
        let mut worker = Worker::new([None; MAX_BINDINGS]);
        let mut platform = FakePlatform {
            refuse_key: Some(0x53),
            ..FakePlatform::default()
        };
        let bindings = [Some(ctrl(Key::S)), Some(ctrl(Key::Q))];
        assert_eq!(
            worker.apply(Command::Set(bindings), &mut platform),
            Some(Err("already registered".into()))
        );
        assert!(!worker.accepts_message(1, message_param(MOD_CONTROL, 0x53)));
        assert!(worker.accepts_message(2, message_param(MOD_CONTROL, 0x51)));
        platform.operations.clear();
        assert_eq!(worker.refresh(&mut platform), None);
        assert!(platform.operations.is_empty());

        platform.refuse_key = None;
        assert_eq!(
            worker.apply(Command::Set(bindings), &mut platform),
            Some(Ok(()))
        );
        assert!(worker.accepts_message(1, message_param(MOD_CONTROL, 0x53)));
    }

    #[test]
    fn a_failed_release_is_reported_and_blocks_dispatch_until_released() {
        let mut worker = Worker::new([None; MAX_BINDINGS]);
        let mut platform = FakePlatform::default();
        assert_eq!(
            worker.apply(Command::Set([Some(ctrl(Key::S)), None]), &mut platform),
            Some(Ok(()))
        );
        platform.refuse_release = true;
        assert_eq!(
            worker.apply(Command::Set([Some(ctrl(Key::Q)), None]), &mut platform),
            Some(Err("could not release".into()))
        );
        assert!(!worker.accepts_message(1, message_param(MOD_CONTROL, 0x53)));
        assert!(!worker.accepts_message(1, message_param(MOD_CONTROL, 0x51)));
        platform.refuse_release = false;
        assert_eq!(
            worker.apply(Command::Set([None; MAX_BINDINGS]), &mut platform),
            Some(Ok(()))
        );
        assert_eq!(platform.operations.last(), Some(&Operation::Unregister(1)));
    }

    #[test]
    fn worker_commands_publish_availability_without_registering_system_hotkeys() {
        let platform = FakePlatform {
            refuse_key: Some(0x53),
            ..FakePlatform::default()
        };
        let hotkey = SnapshotHotkey::start_with_platform(
            Context::default(),
            Hotkeys::defaults().global_bindings(),
            platform,
        )
        .unwrap();
        assert_eq!(registration(&hotkey), Ok(()));
        hotkey.set_bindings([Some(ctrl(Key::S)), None]).unwrap();
        assert_eq!(registration(&hotkey), Err("already registered".into()));
        hotkey.suspend().unwrap();
        assert_eq!(registration(&hotkey), Ok(()));
        hotkey.set_bindings([Some(ctrl(Key::Q)), None]).unwrap();
        hotkey.resume().unwrap();
        assert_eq!(registration(&hotkey), Ok(()));
        hotkey.set_bindings([None; MAX_BINDINGS]).unwrap();
        assert_eq!(registration(&hotkey), Ok(()));
    }

    #[test]
    #[ignore = "temporarily reserves the real Alt+grave shortcut in the Windows session"]
    fn alt_grave_registers_reports_conflicts_and_is_released_on_exit() {
        // Distinguish a restricted/noninteractive desktop from an Alt+grave collision.
        unsafe {
            let modifier = MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT;
            RegisterHotKey(None, HOTKEY_ID, modifier, u32::from(VK_F11.0))
                .expect("this Windows desktop must allow global hotkey registration");
            UnregisterHotKey(None, HOTKEY_ID).unwrap();
        }
        let first = SnapshotHotkey::start(Context::default()).unwrap();
        registration(&first).expect("Alt+grave must be available in this Windows session");
        let second = SnapshotHotkey::start(Context::default()).unwrap();
        assert!(registration(&second).is_err());
        drop(second);
        drop(first);
        let restarted = SnapshotHotkey::start(Context::default()).unwrap();
        registration(&restarted).expect("the previous listener must release Alt+grave");
    }
}

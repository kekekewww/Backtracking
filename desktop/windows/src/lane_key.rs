//! A tap of the right Ctrl key on the recorder window, which separates lanes A and B.
//!
//! egui reports Ctrl without saying which side it is on, so the window's own key messages are
//! read. A tap is Right Ctrl going down and coming up again soon, with nothing else pressed or
//! held: Ctrl kept down for a Ctrl-drag selection or for a shortcut is not one. AltGr arrives
//! as a left Ctrl, so typing with it never counts either. A key is timed by the moments
//! Windows stamped on its messages, so a window that was busy and read them late still sees
//! a tap as one.

use std::cell::Cell;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VIRTUAL_KEY, VK_LBUTTON, VK_LCONTROL, VK_LWIN, VK_MBUTTON, VK_MENU, VK_RBUTTON,
    VK_RWIN, VK_SHIFT, VK_XBUTTON1, VK_XBUTTON2,
};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageTime, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_NCDESTROY, WM_NCLBUTTONDOWN, WM_NCRBUTTONDOWN,
    WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// "BTLN". Identifies the subclass on the recorder window.
const SUBCLASS_ID: usize = 0x4254_4C4E;
/// Windows names both Ctrl keys this in key messages; the right one is an extended key.
const VK_CONTROL: WPARAM = 0x11;
/// A key held longer than this many milliseconds was not tapped.
const TAP_LIMIT_MS: u32 = 500;

/// Follows the keys of one window and remembers a tap of Right Ctrl until it is taken.
#[derive(Debug, Default)]
struct Tap {
    /// When Right Ctrl went down, in the milliseconds since startup that message times count.
    pressed: Option<u32>,
}

impl Tap {
    /// Takes in one message stamped `time`, in the milliseconds since startup that Windows
    /// counts and lets wrap around. True when it ends a tap. `nothing_else_held` is asked when
    /// Right Ctrl goes down.
    fn observe(
        &mut self,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        time: u32,
        nothing_else_held: impl FnOnce() -> bool,
    ) -> bool {
        let right_ctrl = wparam == VK_CONTROL && lparam & (1 << 24) != 0;
        match message {
            WM_KEYDOWN | WM_SYSKEYDOWN if right_ctrl => {
                // A held key repeats its down message; only the first one starts a tap.
                let repeat = lparam & (1 << 30) != 0;
                if !repeat {
                    self.pressed = nothing_else_held().then_some(time);
                }
                false
            }
            WM_KEYUP | WM_SYSKEYUP if right_ctrl => self
                .pressed
                .take()
                .is_some_and(|pressed| time.wrapping_sub(pressed) <= TAP_LIMIT_MS),
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
            | WM_XBUTTONDOWN | WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN | WM_MOUSEWHEEL
            | WM_MOUSEHWHEEL | WM_KILLFOCUS => {
                self.pressed = None;
                false
            }
            _ => false,
        }
    }
}

/// What the subclass shares with the window. Both run on the window's own thread.
#[derive(Default)]
struct Shared {
    tap: Cell<Option<u32>>,
    tapped: Cell<bool>,
}

/// Watches the recorder window for taps of Right Ctrl.
pub(crate) struct LaneKey {
    hwnd: HWND,
    // Boxed so the subclass can hold its address until it is removed.
    shared: Box<Shared>,
}

impl LaneKey {
    pub(crate) fn attach(window: &Window) -> Option<Self> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return None;
        };
        let hwnd = handle.hwnd.get() as HWND;
        let shared = Box::new(Shared::default());
        let data = &*shared as *const Shared as usize;
        // The subclass removes itself when the window is destroyed.
        (unsafe { SetWindowSubclass(hwnd, Some(lane_key_message), SUBCLASS_ID, data) } != 0)
            .then_some(Self { hwnd, shared })
    }

    /// Whether Right Ctrl was tapped since the last call.
    pub(crate) fn take_tap(&self) -> bool {
        self.shared.tapped.replace(false)
    }
}

impl Drop for LaneKey {
    fn drop(&mut self) {
        unsafe { RemoveWindowSubclass(self.hwnd, Some(lane_key_message), SUBCLASS_ID) };
    }
}

/// Whether any other modifier or a mouse button is down as Right Ctrl goes down.
fn nothing_else_held() -> bool {
    let held = |key: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(key.0)) } < 0;
    ![
        VK_SHIFT, VK_MENU, VK_LCONTROL, VK_LWIN, VK_RWIN, VK_LBUTTON, VK_RBUTTON, VK_MBUTTON,
        VK_XBUTTON1, VK_XBUTTON2,
    ]
    .into_iter()
    .any(held)
}

unsafe extern "system" fn lane_key_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(hwnd, Some(lane_key_message), id) };
    } else if let Some(shared) = unsafe { (data as *const Shared).as_ref() } {
        let mut tap = Tap { pressed: shared.tap.get() };
        // The tick count is a DWORD handed back as a LONG; its bits are the time.
        let time = unsafe { GetMessageTime() } as u32;
        // Set before the window handles the key, as that may run a frame at once.
        if tap.observe(message, wparam, lparam, time, nothing_else_held) {
            shared.tapped.set(true);
        }
        shared.tap.set(tap.pressed);
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTENDED: LPARAM = 1 << 24;
    const REPEAT: LPARAM = 1 << 30;
    const VK_A: WPARAM = 0x41;

    /// Plays `messages`, each `(message, wparam, lparam, message time)`, and counts the taps.
    fn taps(messages: &[(u32, WPARAM, LPARAM, u32)], nothing_else_held: bool) -> usize {
        let mut tap = Tap::default();
        messages
            .iter()
            .filter(|&&(message, wparam, lparam, time)| {
                tap.observe(message, wparam, lparam, time, || nothing_else_held)
            })
            .count()
    }

    #[test]
    fn a_quick_press_of_right_ctrl_alone_is_a_tap() {
        let press = [(WM_KEYDOWN, VK_CONTROL, EXTENDED, 0), (WM_KEYUP, VK_CONTROL, EXTENDED, 120)];
        assert_eq!(taps(&press, true), 1);
        // Held keys repeat their down message without ending the tap.
        let held = [
            (WM_KEYDOWN, VK_CONTROL, EXTENDED, 0),
            (WM_KEYDOWN, VK_CONTROL, EXTENDED | REPEAT, 300),
            (WM_KEYUP, VK_CONTROL, EXTENDED, 400),
        ];
        assert_eq!(taps(&held, true), 1);
        // The count of milliseconds wraps around after 49.7 days.
        let wrapped = [
            (WM_KEYDOWN, VK_CONTROL, EXTENDED, u32::MAX - 49),
            (WM_KEYUP, VK_CONTROL, EXTENDED, 70),
        ];
        assert_eq!(taps(&wrapped, true), 1);
    }

    #[test]
    fn left_ctrl_a_long_hold_and_ctrl_with_other_input_are_not_taps() {
        // Left Ctrl, and AltGr's own left Ctrl, are not extended keys.
        let left = [(WM_KEYDOWN, VK_CONTROL, 0, 0), (WM_KEYUP, VK_CONTROL, 0, 100)];
        assert_eq!(taps(&left, true), 0);
        let long = [(WM_KEYDOWN, VK_CONTROL, EXTENDED, 0), (WM_KEYUP, VK_CONTROL, EXTENDED, 900)];
        assert_eq!(taps(&long, true), 0);
        let long_wrapped = [
            (WM_KEYDOWN, VK_CONTROL, EXTENDED, u32::MAX - 449),
            (WM_KEYUP, VK_CONTROL, EXTENDED, 450),
        ];
        assert_eq!(taps(&long_wrapped, true), 0);
        // A shortcut, a Ctrl-drag, a scroll or leaving the window cancels it.
        for other in [
            (WM_KEYDOWN, VK_A, 0, 50),
            (WM_LBUTTONDOWN, 0, 0, 50),
            (WM_RBUTTONDOWN, 0, 0, 50),
            (WM_MOUSEWHEEL, 0, 0, 50),
            (WM_KILLFOCUS, 0, 0, 50),
        ] {
            let messages =
                [(WM_KEYDOWN, VK_CONTROL, EXTENDED, 0), other, (WM_KEYUP, VK_CONTROL, EXTENDED, 100)];
            assert_eq!(taps(&messages, true), 0, "{other:?}");
        }
        // Shift, Alt, a Windows key or a mouse button already down.
        let press = [(WM_KEYDOWN, VK_CONTROL, EXTENDED, 0), (WM_KEYUP, VK_CONTROL, EXTENDED, 100)];
        assert_eq!(taps(&press, false), 0);
        // A release without its press, as after the window gains focus with Ctrl down.
        assert_eq!(taps(&[(WM_KEYUP, VK_CONTROL, EXTENDED, 0)], true), 0);
    }
}

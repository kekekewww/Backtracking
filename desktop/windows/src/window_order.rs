use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
    UI::WindowsAndMessaging::{
        GA_ROOTOWNER, GW_HWNDPREV, GWL_EXSTYLE, GetAncestor, GetWindow, GetWindowLongW,
        GetWindowRect, GetWindowThreadProcessId, HWND_TOPMOST, IsIconic, IsWindowVisible,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SetWindowPos, WS_EX_TOPMOST,
    },
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer, WM_NCDESTROY, WM_TIMER};
use winit::{
    dpi::PhysicalPosition,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

/// "BTPI". Identifies both the subclass and its timer on the capture window.
const PIN_WATCH_ID: usize = 0x4254_5049;
/// Windows does not tell a window that another topmost window was placed above
/// it, so a pinned window looks for itself. This costs no repaint.
const PIN_CHECK_MS: u32 = 250;
/// After a raise, give the other window time to settle before trying again.
const PIN_RETRY_MS: u32 = 320;

/// Keeps a pinned window above other topmost windows from a native timer on
/// its own message queue, so the check never needs a UI pass.
pub(crate) struct PinWatch {
    hwnd: windows_sys::Win32::Foundation::HWND,
    active: bool,
}

impl PinWatch {
    pub(crate) fn attach(window: &Window) -> Option<Self> {
        let hwnd = window_hwnd(window)?.0;
        // The subclass removes itself when the window is destroyed.
        (unsafe { SetWindowSubclass(hwnd, Some(pin_watch_message), PIN_WATCH_ID, 0) } != 0)
            .then_some(Self { hwnd, active: false })
    }

    pub(crate) fn set_active(&mut self, active: bool) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            // Check at once, as a newly pinned window should rise immediately.
            unsafe { check_pinned(self.hwnd) };
        } else {
            unsafe { KillTimer(self.hwnd, PIN_WATCH_ID) };
        }
    }
}

impl Drop for PinWatch {
    fn drop(&mut self) {
        unsafe {
            KillTimer(self.hwnd, PIN_WATCH_ID);
            RemoveWindowSubclass(self.hwnd, Some(pin_watch_message), PIN_WATCH_ID);
        }
    }
}

/// Raises the window if it is covered and arms the timer for the next check.
unsafe fn check_pinned(hwnd: windows_sys::Win32::Foundation::HWND) {
    let attempted = raise_hwnd_if_covered(HWND(hwnd));
    let delay = if attempted { PIN_RETRY_MS } else { PIN_CHECK_MS };
    // Setting a timer that already exists replaces its period.
    unsafe { SetTimer(hwnd, PIN_WATCH_ID, delay, None) };
}

unsafe extern "system" fn pin_watch_message(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
    id: usize,
    _data: usize,
) -> windows_sys::Win32::Foundation::LRESULT {
    if message == WM_TIMER && wparam == PIN_WATCH_ID {
        unsafe { check_pinned(hwnd) };
        return 0;
    }
    if message == WM_NCDESTROY {
        unsafe {
            KillTimer(hwnd, PIN_WATCH_ID);
            RemoveWindowSubclass(hwnd, Some(pin_watch_message), id);
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn window_hwnd(window: &Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(HWND(handle.hwnd.get() as *mut _))
}

/// Restore a physical desktop coordinate only when a usable portion of the window is visible.
/// Otherwise, keep the OS placement, for example when a display has been unplugged.
pub(crate) fn restore_position(window: &Window, [x, y]: [i32; 2]) {
    let size = window.outer_size();
    let (x64, y64) = (i64::from(x), i64::from(y));
    let width = i64::from(size.width);
    let height = i64::from(size.height);
    let visible = window.available_monitors().any(|monitor| {
        let origin = monitor.position();
        let size = monitor.size();
        let (left, top) = (i64::from(origin.x), i64::from(origin.y));
        let right = left + i64::from(size.width);
        let bottom = top + i64::from(size.height);
        (right.min(x64 + width) - left.max(x64)) >= width.min(96)
            && (bottom.min(y64 + height) - top.max(y64)) >= height.min(28)
    });
    if visible {
        window.set_outer_position(PhysicalPosition::new(x, y));
    }
}

/// Reassert the app's topmost order when another visible window covers it.
/// Returns whether a raise was attempted, so callers can throttle retries.
pub(crate) fn raise_if_covered(window: &Window) -> bool {
    window_hwnd(window).is_some_and(raise_hwnd_if_covered)
}

fn raise_hwnd_if_covered(hwnd: HWND) -> bool {
    // The handle belongs to the live window and is used on its event-loop thread.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() || is_cloaked(hwnd) {
            return false;
        }
        let mut bounds = RECT::default();
        if GetWindowRect(hwnd, &mut bounds).is_err() {
            return false;
        }
        let is_topmost = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0;
        if is_topmost && !has_window_above(hwnd, bounds) {
            return false;
        }
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER,
        );
    }
    true
}

unsafe fn has_window_above(hwnd: HWND, bounds: RECT) -> bool {
    let mut current = hwnd;
    let own_process = std::process::id();
    // Other processes may create or reorder windows while this scan is in progress.
    for _ in 0..512 {
        let Ok(previous) = (unsafe { GetWindow(current, GW_HWNDPREV) }) else {
            return false;
        };
        if previous == hwnd || previous == current {
            return false;
        }
        current = previous;
        let mut other = RECT::default();
        unsafe {
            if !IsWindowVisible(current).as_bool()
                || IsIconic(current).as_bool()
                || GetWindowRect(current, &mut other).is_err()
                || !overlaps(bounds, other)
            {
                continue;
            }
            let mut process = 0;
            GetWindowThreadProcessId(current, Some(&mut process));
            if process != own_process
                && GetAncestor(current, GA_ROOTOWNER) != hwnd
                && !is_cloaked(current)
            {
                return true;
            }
        }
    }
    false
}

fn overlaps(a: RECT, b: RECT) -> bool {
    a.left.max(b.left) < a.right.min(b.right) && a.top.max(b.top) < a.bottom.min(b.bottom)
}

unsafe fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    }
    .is_ok()
        && cloaked != 0
}

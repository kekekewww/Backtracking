use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
    UI::WindowsAndMessaging::{
        GA_ROOTOWNER, GW_HWNDPREV, GWL_EXSTYLE, GetAncestor, GetWindow, GetWindowLongW,
        GetWindowRect, GetWindowThreadProcessId, HWND_TOPMOST, IsIconic, IsWindowVisible,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SetWindowPos, WS_EX_TOPMOST,
    },
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

/// Reassert the app's topmost order when another visible window covers it.
/// Returns whether a raise was attempted, so callers can throttle retries.
pub(crate) fn raise_if_covered(window: &Window) -> bool {
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    // The handle is borrowed from the live window on its event-loop thread.
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

use std::cell::Cell;
use std::io;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::time::{Duration, Instant};

use eframe::egui::Context;
use windows::core::w;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_SETFOCUS, NIM_SETVERSION, NIN_SELECT, NINF_KEY, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    NOTIFYICONIDENTIFIER, RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconGetRect,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, GWL_EXSTYLE, GetCursorPos,
    GetSystemMetrics, GetWindowLongPtrW, HICON, HMENU, IMAGE_ICON, IsIconic, IsWindowVisible,
    LR_DEFAULTCOLOR, LoadImageW, MF_CHECKED, MF_SEPARATOR, MF_STRING, PostMessageW,
    RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, STYLESTRUCT, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP,
    WM_CONTEXTMENU, WM_LBUTTONUP, WM_NCDESTROY, WM_NULL, WM_RBUTTONUP, WM_STYLECHANGING,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use winit::platform::windows::WindowExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::startup;

const CALLBACK: u32 = WM_APP + 0x42a;
const ICON_ID: u32 = 1;
const SUBCLASS_ID: usize = 1;
const RETRY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
pub(crate) enum Action {
    Show,
    Hide,
    ToggleStartup,
    Quit,
}

pub(crate) struct Tray {
    hwnd: HWND,
    state: Rc<TrayState>,
}

struct TrayState {
    icon: HICON,
    ctx: Context,
    action: Cell<Option<Action>>,
    attached: Cell<bool>,
    registered: Cell<bool>,
    version_four: Cell<bool>,
    next_registration: Cell<Instant>,
    taskbar_created: u32,
    show_existing: u32,
    initial_taskbar_style: u32,
}

impl Tray {
    pub(crate) fn attach(window: &Window, ctx: &Context) -> io::Result<Self> {
        let handle = window
            .window_handle()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("Windows window handle unavailable"));
        };
        let hwnd = handle.hwnd.get() as HWND;
        let icon = unsafe {
            let module = GetModuleHandleW(null());
            if module.is_null() {
                return Err(io::Error::last_os_error());
            }
            let icon = LoadImageW(
                module,
                1usize as *const u16,
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_DEFAULTCOLOR,
            );
            if icon.is_null() {
                return Err(io::Error::last_os_error());
            }
            icon
        };
        let state = Rc::new(TrayState {
            icon,
            ctx: ctx.clone(),
            action: Cell::new(None),
            attached: Cell::new(true),
            registered: Cell::new(false),
            version_four: Cell::new(false),
            next_registration: Cell::new(Instant::now()),
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated").as_ptr()) },
            show_existing: startup::show_message(),
            initial_taskbar_style: unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
                & (WS_EX_APPWINDOW | WS_EX_TOOLWINDOW),
        });
        // The subclass owns a strong reference until it is removed or the HWND is destroyed.
        let data = Rc::into_raw(Rc::clone(&state));
        if unsafe { SetWindowSubclass(hwnd, Some(tray_message), SUBCLASS_ID, data as usize) } == 0 {
            drop(unsafe { Rc::from_raw(data) });
            return Err(io::Error::other("Could not attach the tray to the window"));
        }
        let tray = Self { hwnd, state };
        tray.refresh();
        Ok(tray)
    }

    pub(crate) fn refresh(&self) {
        if self.state.attached.get()
            && !self.state.registered.get()
            && Instant::now() >= self.state.next_registration.get()
        {
            self.state.next_registration.set(Instant::now() + RETRY);
            let mut data = self.state.icon_data(self.hwnd);
            data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
            data.uCallbackMessage = CALLBACK;
            data.hIcon = self.state.icon;
            for (output, unit) in data.szTip.iter_mut().zip("Backtracking".encode_utf16()) {
                *output = unit;
            }
            if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0 {
                self.state.registered.set(true);
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                self.state
                    .version_four
                    .set(unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) } != 0);
            }
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.state.registered.get()
    }

    pub(crate) fn apply_taskbar_mode(&self, window: &Window) {
        let active = self.is_active();
        unsafe {
            let current = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
            let taskbar_style = if active {
                WS_EX_TOOLWINDOW
            } else {
                self.state.initial_taskbar_style
            };
            let next = (current & !(WS_EX_APPWINDOW | WS_EX_TOOLWINDOW)) | taskbar_style;
            if current != next {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, next as isize);
                SetWindowPos(
                    self.hwnd,
                    null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                );
            }
        }
        window.set_skip_taskbar(active);
    }

    pub(crate) fn take_action(&self) -> Option<Action> {
        self.state.action.take()
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if self.state.attached.get()
            && unsafe { RemoveWindowSubclass(self.hwnd, Some(tray_message), SUBCLASS_ID) } != 0
        {
            self.state.detach(self.hwnd);
            drop(unsafe { Rc::from_raw(Rc::as_ptr(&self.state)) });
        }
    }
}

impl TrayState {
    fn icon_data(&self, hwnd: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: ICON_ID,
            ..Default::default()
        }
    }

    fn detach(&self, hwnd: HWND) {
        self.attached.set(false);
        if self.registered.replace(false) {
            unsafe { Shell_NotifyIconW(NIM_DELETE, &self.icon_data(hwnd)) };
        }
    }

    fn dispatch(&self, action: Action) {
        self.action.set(Some(action));
        self.ctx.request_repaint();
    }
}

impl Drop for TrayState {
    fn drop(&mut self) {
        let _ = unsafe { DestroyIcon(self.icon) };
    }
}

unsafe extern "system" fn tray_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    // Keep the state alive if a native menu dispatches a nested window destruction.
    let data = data as *const TrayState;
    let state = unsafe {
        Rc::increment_strong_count(data);
        Rc::from_raw(data)
    };
    if message == WM_NCDESTROY {
        state.detach(hwnd);
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(tray_message), id) };
        drop(unsafe { Rc::from_raw(data) });
    } else if message == WM_STYLECHANGING
        && wparam as i32 == GWL_EXSTYLE
        && state.registered.get()
        && lparam != 0
    {
        // Winit rebuilds extended styles on state changes, including when showing the window.
        let style = unsafe { &mut *(lparam as *mut STYLESTRUCT) };
        style.styleNew = (style.styleNew & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW;
    } else if message == CALLBACK {
        const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
        let notification = (lparam as u32) & 0xffff;
        let action = match (state.version_four.get(), notification) {
            (true, NIN_SELECT | NIN_KEYSELECT) | (false, WM_LBUTTONUP) => Some(Action::Show),
            (true, WM_CONTEXTMENU) | (false, WM_RBUTTONUP) => tray_menu(hwnd, &state),
            _ => None,
        };
        if let Some(action) = action.filter(|_| state.attached.get()) {
            state.dispatch(action);
        }
        return 0;
    } else if message != 0 && message == state.taskbar_created {
        // Explorer lost all icons during restart. Keep processing the original Winit message
        // so its taskbar visibility is restored too.
        state.registered.set(false);
        state.next_registration.set(Instant::now());
        state.ctx.request_repaint();
    } else if message != 0 && message == state.show_existing {
        state.dispatch(Action::Show);
        return 0;
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn tray_menu(hwnd: HWND, state: &TrayState) -> Option<Action> {
    let id = NOTIFYICONIDENTIFIER {
        cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    };
    let mut rect = RECT::default();
    let mut position = POINT::default();
    if unsafe { Shell_NotifyIconGetRect(&id, &mut rect) } >= 0 {
        position.x = rect.left + (rect.right - rect.left) / 2;
        position.y = rect.top;
    } else if unsafe { GetCursorPos(&mut position) } == 0 {
        return None;
    }
    let visible = unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 };
    let startup_checked = startup::enabled().unwrap_or_default();
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return None;
    }
    let menu = Menu(menu);
    unsafe {
        let visibility = if visible {
            w!("Hide Backtracking")
        } else {
            w!("Show Backtracking")
        };
        let startup_flags = if startup_checked {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        if AppendMenuW(menu.0, MF_STRING, 1, visibility.as_ptr()) == 0
            || AppendMenuW(menu.0, startup_flags, 2, w!("Start with Windows").as_ptr()) == 0
            || AppendMenuW(menu.0, MF_SEPARATOR, 0, null()) == 0
            || AppendMenuW(menu.0, MF_STRING, 3, w!("Quit").as_ptr()) == 0
        {
            return None;
        }
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu.0,
            TPM_NONOTIFY | TPM_RETURNCMD | TPM_RIGHTBUTTON,
            position.x,
            position.y,
            0,
            hwnd,
            null(),
        );
        if state.attached.get() {
            let _ = PostMessageW(hwnd, WM_NULL, 0, 0);
            if command == 0 {
                let _ = Shell_NotifyIconW(NIM_SETFOCUS, &state.icon_data(hwnd));
            }
        }
        match command {
            1 if visible => Some(Action::Hide),
            1 => Some(Action::Show),
            2 => Some(Action::ToggleStartup),
            3 => Some(Action::Quit),
            _ => None,
        }
    }
}

struct Menu(HMENU);

impl Drop for Menu {
    fn drop(&mut self) {
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, LPARAM, LRESULT, POINT,
    RECT, S_OK, WPARAM,
};
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize,
    OleUninitialize,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetCapture, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::Shell::{
    BHID_DataObject, DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, GetWindowRect, PostMessageW, SM_SWAPBUTTON, SendMessageW,
    WM_APP, WM_LBUTTONUP, WM_NCDESTROY,
};
use windows::core::{BOOL, HRESULT, PCWSTR, implement};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{SHCreateShellItemArrayFromIDLists, SHParseDisplayName};

use super::ActiveDrag;

const START_DRAG: u32 = WM_APP + 0x425;
static NEXT_DRAG_ID: AtomicUsize = AtomicUsize::new(1);

fn editor_hwnd(window: &impl HasWindowHandle) -> Result<HWND, String> {
    let RawWindowHandle::Win32(handle) =
        window.window_handle().map_err(|e| e.to_string())?.as_raw()
    else {
        return Err("editor window is unavailable".into());
    };
    Ok(HWND(handle.hwnd.get() as _))
}

pub(super) fn pointer_outside_editor(window: &impl HasWindowHandle) -> bool {
    editor_hwnd(window).is_ok_and(pointer_outside)
}

fn pointer_outside(hwnd: HWND) -> bool {
    let mut cursor = POINT::default();
    let mut bounds = RECT::default();
    unsafe {
        if GetCursorPos(&mut cursor).is_err() || GetWindowRect(hwnd, &mut bounds).is_err() {
            return false;
        }
    }
    cursor.x < bounds.left
        || cursor.x >= bounds.right
        || cursor.y < bounds.top
        || cursor.y >= bounds.bottom
}

pub(super) fn primary_button_pressed() -> bool {
    let button = unsafe {
        if GetSystemMetrics(SM_SWAPBUTTON) == 0 {
            VK_LBUTTON
        } else {
            VK_RBUTTON
        }
    };
    unsafe { GetAsyncKeyState(i32::from(button.0)) < 0 }
}

pub(super) fn schedule(window: &impl HasWindowHandle, drag: ActiveDrag) -> Result<(), String> {
    let hwnd = editor_hwnd(window)?;
    let id = NEXT_DRAG_ID.fetch_add(1, Ordering::Relaxed);

    // OLE dispatches window messages, including drag-enter events to this editor. Post the
    // operation so the editor's current egui draw and its mutable borrows can finish first.
    let data = Box::into_raw(Box::new(drag));
    unsafe {
        if !SetWindowSubclass(hwnd, Some(drag_message), id, data as usize).as_bool() {
            drop(Box::from_raw(data));
            return Err("could not schedule drag on the editor window".into());
        }
        if let Err(error) = PostMessageW(Some(hwnd), START_DRAG, WPARAM(id), LPARAM(0)) {
            if RemoveWindowSubclass(hwnd, Some(drag_message), id).as_bool() {
                drop(Box::from_raw(data));
            }
            return Err(error.to_string());
        }
    }
    Ok(())
}

unsafe extern "system" fn drag_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    if message == START_DRAG && wparam.0 == id {
        // Detach before entering OLE's nested message loop. The drag owns its state even if
        // the host closes the editor while it is running.
        if unsafe { RemoveWindowSubclass(hwnd, Some(drag_message), id) }.as_bool() {
            let mut drag = unsafe { Box::from_raw(data as *mut ActiveDrag) };
            match drag_files(hwnd, &drag.file.wav.paths) {
                Ok(true) => drag.accept(),
                Ok(false) => {}
                Err(error) => {
                    let _ = drag.results.send(Err(error.to_string()));
                }
            }
        }
        return LRESULT(0);
    }
    if message == WM_NCDESTROY {
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(drag_message), id);
            drop(Box::from_raw(data as *mut ActiveDrag));
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

struct OleApartment;

impl Drop for OleApartment {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

struct ShellId(*mut ITEMIDLIST);

impl ShellId {
    fn from_path(path: &Path) -> windows::core::Result<Self> {
        let wide_path: Vec<_> = dunce::simplified(path)
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        let mut id = Self(null_mut());
        unsafe { SHParseDisplayName(PCWSTR(wide_path.as_ptr()), None, &mut id.0, 0, None)? };
        Ok(id)
    }
}

impl Drop for ShellId {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.cast())) };
    }
}

fn drag_files(hwnd: HWND, paths: &[PathBuf]) -> windows::core::Result<bool> {
    unsafe {
        if paths.is_empty() || !primary_button_pressed() || !pointer_outside(hwnd) {
            return Ok(false);
        }
        OleInitialize(None)?;
        let _ole = OleApartment;
        let ids = paths
            .iter()
            .map(|path| ShellId::from_path(path))
            .collect::<windows::core::Result<Vec<_>>>()?;
        let pointers: Vec<_> = ids.iter().map(|id| id.0.cast_const()).collect();
        let items = SHCreateShellItemArrayFromIDLists(&pointers)?;
        let data: IDataObject = items.BindToHandler(None, &BHID_DataObject)?;
        if !primary_button_pressed() || !pointer_outside(hwnd) {
            return Ok(false);
        }
        if GetCapture() == hwnd {
            // Finish the editor's local mouse gesture and release its capture before OLE
            // takes over. OLE reads the physical button state independently of this message.
            SendMessageW(hwnd, WM_LBUTTONUP, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        let source: IDropSource = DropSource {
            hwnd: hwnd.0 as usize,
        }
        .into();
        let mut effect = DROPEFFECT::default();
        let result = DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect);
        result.ok()?;
        Ok(result == DRAGDROP_S_DROP && effect.contains(DROPEFFECT_COPY))
    }
}

#[implement(IDropSource)]
struct DropSource {
    hwnd: usize,
}

#[allow(non_snake_case)]
impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if !keys.contains(MK_LBUTTON) {
            if pointer_outside(HWND(self.hwnd as _)) {
                DRAGDROP_S_DROP
            } else {
                DRAGDROP_S_CANCEL
            }
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

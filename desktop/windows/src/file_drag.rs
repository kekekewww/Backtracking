use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINT, RECT, S_OK,
};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize,
    OleUninitialize,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::{
    BHID_DataObject, IShellItem, IShellItemArray, SHCreateItemFromParsingName,
    SHCreateShellItemArrayFromShellItem,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId, SM_SWAPBUTTON,
    WindowFromPoint,
};
use windows::core::{BOOL, HRESULT, PCWSTR, implement};

pub(crate) fn primary_button_down() -> bool {
    let button = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } == 0 {
        VK_LBUTTON
    } else {
        VK_RBUTTON
    };
    unsafe { GetAsyncKeyState(i32::from(button.0)) < 0 }
}

pub(crate) fn cursor_outside_app(source: HWND) -> bool {
    let mut cursor = POINT::default();
    let mut bounds = RECT::default();
    unsafe {
        if GetCursorPos(&mut cursor).is_err() || GetWindowRect(source, &mut bounds).is_err() {
            return false;
        }
        if cursor.x >= bounds.left
            && cursor.x < bounds.right
            && cursor.y >= bounds.top
            && cursor.y < bounds.bottom
        {
            return false;
        }
        let target = WindowFromPoint(cursor);
        let mut process = 0;
        GetWindowThreadProcessId(target, Some(&mut process)) != 0 && process != std::process::id()
    }
}

struct OleApartment;

impl Drop for OleApartment {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

pub(crate) fn drag_file(source: HWND, path: &Path) -> windows::core::Result<bool> {
    let wide_path: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        OleInitialize(None)?;
        let _ole = OleApartment;
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None)?;
        let items: IShellItemArray = SHCreateShellItemArrayFromShellItem(&item)?;
        let data: IDataObject = items.BindToHandler(None, &BHID_DataObject)?;
        if !primary_button_down() || !cursor_outside_app(source) {
            return Ok(false);
        }
        let drop_source: IDropSource = DropSource(source).into();
        let mut effect = DROPEFFECT::default();
        let result = DoDragDrop(&data, &drop_source, DROPEFFECT_COPY, &mut effect);
        result.ok()?;
        Ok(result == DRAGDROP_S_DROP && effect.contains(DROPEFFECT_COPY))
    }
}

#[implement(IDropSource)]
struct DropSource(HWND);

#[allow(non_snake_case)]
impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.contains(MK_LBUTTON) {
            S_OK
        } else if cursor_outside_app(self.0) {
            DRAGDROP_S_DROP
        } else {
            DRAGDROP_S_CANCEL
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

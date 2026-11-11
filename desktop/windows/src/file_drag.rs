use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINT, RECT, S_OK,
};
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize,
    OleUninitialize,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::{
    BHID_DataObject, Common::ITEMIDLIST, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
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

struct ShellId(*mut ITEMIDLIST);

impl ShellId {
    fn from_path(path: &Path) -> windows::core::Result<Self> {
        let wide_path: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
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

pub(crate) fn drag_files(source: HWND, paths: &[PathBuf]) -> windows::core::Result<bool> {
    if paths.is_empty() {
        return Ok(false);
    }
    unsafe {
        OleInitialize(None)?;
        let _ole = OleApartment;
        let data = shell_data_for_files(paths)?;
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

fn shell_data_for_files(paths: &[PathBuf]) -> windows::core::Result<IDataObject> {
    let ids = paths
        .iter()
        .map(|path| ShellId::from_path(path))
        .collect::<windows::core::Result<Vec<_>>>()?;
    let pointers: Vec<_> = ids.iter().map(|id| id.0.cast_const()).collect();
    unsafe { SHCreateShellItemArrayFromIDLists(&pointers)?.BindToHandler(None, &BHID_DataObject) }
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

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL};
    use windows::Win32::System::Ole::{CF_HDROP, ReleaseStgMedium};
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    use super::*;

    struct Medium(STGMEDIUM);

    impl Drop for Medium {
        fn drop(&mut self) {
            unsafe { ReleaseStgMedium(&mut self.0) };
        }
    }

    #[test]
    fn shell_drag_offers_both_wav_paths_in_one_payload() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("Backtracking-drag-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let paths = [
            directory.join("recording-A.wav"),
            directory.join("recording-B.wav"),
        ];
        for path in &paths {
            std::fs::write(path, b"test").unwrap();
        }

        unsafe { OleInitialize(None).unwrap() };
        let _ole = OleApartment;
        let data = shell_data_for_files(&paths).unwrap();
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
            ..Default::default()
        };
        let medium = Medium(unsafe { data.GetData(&format) }.unwrap());
        let drop_handle = HDROP(unsafe { medium.0.u.hGlobal }.0);
        assert_eq!(unsafe { DragQueryFileW(drop_handle, u32::MAX, None) }, 2);
        for (index, path) in paths.iter().enumerate() {
            let length = unsafe { DragQueryFileW(drop_handle, index as u32, None) };
            let mut wide = vec![0; length as usize + 1];
            let written = unsafe { DragQueryFileW(drop_handle, index as u32, Some(&mut wide)) };
            assert_eq!(written, length);
            let received = PathBuf::from(OsString::from_wide(&wide[..written as usize]));
            assert_eq!(
                std::fs::canonicalize(received).unwrap(),
                std::fs::canonicalize(path).unwrap()
            );
        }
        drop(medium);
        drop(data);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

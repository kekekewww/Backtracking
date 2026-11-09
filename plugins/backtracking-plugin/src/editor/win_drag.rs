use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, LPARAM, LRESULT, S_OK,
    WPARAM,
};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize,
    OleUninitialize,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetCapture, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::Shell::{
    BHID_DataObject, DefSubclassProc, IShellItem, IShellItemArray, RemoveWindowSubclass,
    SHCreateItemFromParsingName, SHCreateShellItemArrayFromShellItem, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, PostMessageW, SM_SWAPBUTTON, SendMessageW, WM_APP, WM_LBUTTONUP, WM_NCDESTROY,
};
use windows::core::{BOOL, HRESULT, PCWSTR, implement};

use crate::session::Session;

const START_DRAG: u32 = WM_APP + 0x425;
static NEXT_DRAG_ID: AtomicUsize = AtomicUsize::new(1);

pub(super) fn schedule(
    window: &impl HasWindowHandle,
    path: PathBuf,
    session: Arc<Session>,
    active: Arc<AtomicBool>,
) -> Result<Receiver<String>, String> {
    let (errors, receiver) = mpsc::channel();
    active.store(true, Ordering::Release);
    let drag = Box::new(PendingDrag {
        path,
        session,
        active,
        errors,
    });
    let RawWindowHandle::Win32(handle) =
        window.window_handle().map_err(|e| e.to_string())?.as_raw()
    else {
        return Err("editor window is unavailable".into());
    };
    let hwnd = HWND(handle.hwnd.get() as _);
    let id = NEXT_DRAG_ID.fetch_add(1, Ordering::Relaxed);

    // OLE dispatches window messages, including drag-enter events to this editor. Post the
    // operation so the editor's current egui draw and its mutable borrows can finish first.
    let data = Box::into_raw(drag);
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
    Ok(receiver)
}

struct PendingDrag {
    path: PathBuf,
    session: Arc<Session>,
    active: Arc<AtomicBool>,
    errors: Sender<String>,
}

impl Drop for PendingDrag {
    fn drop(&mut self) {
        self.session.end_waveform_gesture();
        self.active.store(false, Ordering::Release);
    }
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
            let drag = unsafe { Box::from_raw(data as *mut PendingDrag) };
            if let Err(error) = drag_file(hwnd, &drag.path) {
                let _ = drag.errors.send(error.to_string());
            }
        }
        return LRESULT(0);
    }
    if message == WM_NCDESTROY {
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(drag_message), id);
            drop(Box::from_raw(data as *mut PendingDrag));
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

fn drag_file(hwnd: HWND, path: &Path) -> windows::core::Result<()> {
    unsafe {
        OleInitialize(None)?;
        let _ole = OleApartment;
        let wide_path: Vec<_> = dunce::simplified(path)
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None)?;
        let items: IShellItemArray = SHCreateShellItemArrayFromShellItem(&item)?;
        let data: IDataObject = items.BindToHandler(None, &BHID_DataObject)?;
        let primary_button = if GetSystemMetrics(SM_SWAPBUTTON) == 0 {
            VK_LBUTTON
        } else {
            VK_RBUTTON
        };
        if GetAsyncKeyState(i32::from(primary_button.0)) >= 0 {
            return Ok(());
        }
        if GetCapture() == hwnd {
            // Finish the editor's local mouse gesture and release its capture before OLE
            // takes over. OLE reads the physical button state independently of this message.
            SendMessageW(hwnd, WM_LBUTTONUP, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        let source: IDropSource = DropSource.into();
        let mut effect = DROPEFFECT::default();
        DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect).ok()
    }
}

#[implement(IDropSource)]
struct DropSource;

#[allow(non_snake_case)]
impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if !keys.contains(MK_LBUTTON) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

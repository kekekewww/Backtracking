//! Keeps the Windows input method away from the editor's window unless text is being edited.
//!
//! Baseview takes every key message for its windows out of the queue in a message hook, before
//! the host's `TranslateMessage` runs. A Chinese, Japanese or Korean input method in its native
//! mode claims keys such as Space as they are read (they arrive as `VK_PROCESSKEY`), so those
//! presses never reach egui; only their releases do. The desktop app's window (winit) allows the
//! input method only while egui edits text, and this does the same for the editor's window.

use raw_window_handle::HasWindowHandle;
use windows::Win32::UI::Input::Ime::{HIMC, IACE_CHILDREN, IACE_DEFAULT, ImmAssociateContextEx};

use super::win_drag::editor_hwnd;

/// What the editor last told Windows about its window.
#[derive(Default)]
pub(super) struct InputMethod {
    applied: Option<(usize, bool)>,
}

impl InputMethod {
    /// Allows the input method for the editor's window while `editing_text`, and keeps it off
    /// otherwise.
    pub(super) fn follow(&mut self, window: &impl HasWindowHandle, editing_text: bool) {
        let Ok(hwnd) = editor_hwnd(window) else {
            return;
        };
        let wanted = Some((hwnd.0 as usize, editing_text));
        if self.applied == wanted {
            return;
        }
        self.applied = wanted;
        // No context, for the window and its children, turns the input method off there;
        // IACE_DEFAULT gives the window back the context Windows made for it.
        let flags = if editing_text { IACE_DEFAULT } else { IACE_CHILDREN };
        // SAFETY: the handle names the editor's open window, and egui draws it on the thread
        // that owns that window.
        let _ = unsafe { ImmAssociateContextEx(hwnd, HIMC(std::ptr::null_mut()), flags) };
    }
}

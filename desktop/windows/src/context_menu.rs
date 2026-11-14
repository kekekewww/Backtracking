//! The recorder window's right-click menu and the commands it shares with the keyboard.
//!
//! The menu is a native popup rather than an egui one: egui draws inside its own window, and the
//! recorder, above all in waveform-only mode, is far shorter than the menu.

use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, MF_CHECKED, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, PostMessageW, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// An everyday action that the menu, the keyboard and the toolbar all run the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowCommand {
    ToggleCapture,
    Clear,
    SaveSnapshot,
    WaveformOnly,
    KeepOnTop,
    Options,
    Quit,
}

impl WindowCommand {
    /// The key that runs this command while the recorder window has keyboard focus.
    pub(crate) fn shortcut(self) -> Option<&'static str> {
        match self {
            Self::ToggleCapture => Some("Space"),
            Self::Clear => Some("Delete"),
            Self::SaveSnapshot => Some("Ctrl+S"),
            Self::WaveformOnly | Self::KeepOnTop | Self::Options | Self::Quit => None,
        }
    }
}

/// What the transport item will do, which is the opposite of what capture is doing now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transport {
    Pause,
    Resume,
    Retry,
}

impl Transport {
    fn label(self) -> &'static str {
        match self {
            Self::Pause => "Pause",
            Self::Resume => "Resume",
            Self::Retry => "Retry capture",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct State {
    pub(crate) transport: Transport,
    /// False while a waveform gesture owns the capture, as for the toolbar buttons.
    pub(crate) capture_controls: bool,
    pub(crate) waveform_only: bool,
    pub(crate) keep_on_top: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Separator,
    Item {
        command: WindowCommand,
        label: &'static str,
        checked: bool,
        enabled: bool,
    },
}

impl Entry {
    /// The native menu text. A tab right-aligns the shortcut, as in any Windows menu.
    fn text(&self) -> Option<String> {
        let Self::Item { command, label, .. } = self else {
            return None;
        };
        Some(match command.shortcut() {
            Some(shortcut) => format!("{label}\t{shortcut}"),
            None => (*label).to_owned(),
        })
    }
}

pub(crate) fn entries(state: State) -> Vec<Entry> {
    let item = |command, label, checked, enabled| Entry::Item {
        command,
        label,
        checked,
        enabled,
    };
    let capture = state.capture_controls;
    vec![
        item(WindowCommand::ToggleCapture, state.transport.label(), false, capture),
        item(WindowCommand::Clear, "Clear", false, capture),
        item(WindowCommand::SaveSnapshot, "Save snapshot", false, capture),
        Entry::Separator,
        item(WindowCommand::WaveformOnly, "Show waveform only", state.waveform_only, true),
        item(WindowCommand::KeepOnTop, "Keep on top", state.keep_on_top, true),
        Entry::Separator,
        item(WindowCommand::Options, "Options…", false, true),
        item(WindowCommand::Quit, "Quit", false, true),
    ]
}

struct Menu(HMENU);

impl Drop for Menu {
    fn drop(&mut self) {
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

/// Shows the menu at the pointer and waits for the user to choose or dismiss it.
/// Capture carries on in its own threads meanwhile. Only this window's painting waits.
pub(crate) fn show(window: &Window, entries: &[Entry]) -> Option<WindowCommand> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    let hwnd = handle.hwnd.get() as HWND;
    let mut position = POINT::default();
    let chosen = unsafe {
        if GetCursorPos(&mut position) == 0 {
            return None;
        }
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return None;
        }
        let menu = Menu(menu);
        for (index, entry) in entries.iter().enumerate() {
            let appended = match entry {
                Entry::Separator => AppendMenuW(menu.0, MF_SEPARATOR, 0, null()),
                Entry::Item {
                    checked, enabled, ..
                } => {
                    let text: Vec<u16> = entry
                        .text()?
                        .encode_utf16()
                        .chain(std::iter::once(0))
                        .collect();
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    // Identifier zero means "dismissed", so entries are numbered from one.
                    AppendMenuW(menu.0, flags, index + 1, text.as_ptr())
                }
            };
            if appended == 0 {
                return None;
            }
        }
        // A popup menu only closes on an outside click when its owner is the foreground window.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu.0,
            TPM_NONOTIFY | TPM_RETURNCMD | TPM_RIGHTBUTTON,
            position.x,
            position.y,
            0,
            hwnd,
            null(),
        );
        let _ = PostMessageW(hwnd, WM_NULL, 0, 0);
        chosen
    };
    let index = usize::try_from(chosen).ok()?.checked_sub(1)?;
    match entries.get(index)? {
        Entry::Item {
            command,
            enabled: true,
            ..
        } => Some(*command),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            transport: Transport::Pause,
            capture_controls: true,
            waveform_only: false,
            keep_on_top: false,
        }
    }

    fn item(entries: &[Entry], wanted: WindowCommand) -> (&'static str, bool, bool) {
        entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Item {
                    command,
                    label,
                    checked,
                    enabled,
                } if *command == wanted => Some((*label, *checked, *enabled)),
                _ => None,
            })
            .expect("the menu offers this command")
    }

    #[test]
    fn menu_lists_everyday_commands_in_order_with_their_shortcuts() {
        let texts: Vec<_> = entries(state())
            .iter()
            .map(|entry| entry.text().unwrap_or_else(|| "-".into()))
            .collect();
        assert_eq!(
            texts,
            [
                "Pause\tSpace",
                "Clear\tDelete",
                "Save snapshot\tCtrl+S",
                "-",
                "Show waveform only",
                "Keep on top",
                "-",
                "Options…",
                "Quit",
            ]
        );
    }

    #[test]
    fn transport_item_names_what_it_will_do_and_toggles_show_their_state() {
        for (transport, label) in [
            (Transport::Pause, "Pause"),
            (Transport::Resume, "Resume"),
            (Transport::Retry, "Retry capture"),
        ] {
            let entries = entries(State {
                transport,
                ..state()
            });
            assert_eq!(item(&entries, WindowCommand::ToggleCapture), (label, false, true));
        }

        let off = entries(state());
        assert_eq!(
            item(&off, WindowCommand::WaveformOnly),
            ("Show waveform only", false, true)
        );
        assert_eq!(item(&off, WindowCommand::KeepOnTop), ("Keep on top", false, true));
        let on = entries(State {
            waveform_only: true,
            keep_on_top: true,
            ..state()
        });
        assert!(item(&on, WindowCommand::WaveformOnly).1);
        assert!(item(&on, WindowCommand::KeepOnTop).1);
    }

    #[test]
    fn capture_commands_are_unavailable_during_a_waveform_gesture() {
        let entries = entries(State {
            capture_controls: false,
            ..state()
        });
        for command in [WindowCommand::ToggleCapture, WindowCommand::Clear, WindowCommand::SaveSnapshot] {
            assert!(!item(&entries, command).2, "{command:?}");
        }
        for command in [
            WindowCommand::WaveformOnly,
            WindowCommand::KeepOnTop,
            WindowCommand::Options,
            WindowCommand::Quit,
        ] {
            assert!(item(&entries, command).2, "{command:?}");
        }
    }
}

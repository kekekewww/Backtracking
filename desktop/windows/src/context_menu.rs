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

use crate::hotkeys::Action;

/// An everyday action that the menu, the keyboard and the toolbar all run the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowCommand {
    ToggleCapture,
    Clear,
    SaveSnapshot,
    SeparateLanes,
    WaveformOnly,
    KeepOnTop,
    Options,
    Quit,
}

impl WindowCommand {
    /// The shortcut action that runs this command while the recorder window has keyboard focus.
    pub(crate) fn action(self) -> Action {
        match self {
            Self::ToggleCapture => Action::ToggleCapture,
            Self::Clear => Action::Clear,
            Self::SaveSnapshot => Action::SaveSnapshot,
            Self::SeparateLanes => Action::SeparateLanes,
            Self::WaveformOnly => Action::WaveformOnly,
            Self::KeepOnTop => Action::KeepOnTop,
            Self::Options => Action::OpenOptions,
            Self::Quit => Action::Quit,
        }
    }

    /// The command a shortcut of the recorder window runs. Cancelling a selection is no command:
    /// it works only while there is one. The system-wide save is not the window's.
    pub(crate) fn for_action(action: Action) -> Option<Self> {
        Some(match action {
            Action::ToggleCapture => Self::ToggleCapture,
            Action::Clear => Self::Clear,
            Action::SaveSnapshot => Self::SaveSnapshot,
            Action::SeparateLanes => Self::SeparateLanes,
            Action::WaveformOnly => Self::WaveformOnly,
            Action::KeepOnTop => Self::KeepOnTop,
            Action::OpenOptions => Self::Options,
            Action::Quit => Self::Quit,
            Action::SaveSnapshotGlobal | Action::CancelSelection => return None,
        })
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
    /// The window shows more than one row, so rows can be edited on their own.
    pub(crate) several_rows: bool,
    pub(crate) separate_rows: bool,
    /// The item's words in the window's language ("Edit separately").
    pub(crate) separate_label: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Separator,
    Item {
        command: WindowCommand,
        label: &'static str,
        /// The key that runs the command, as the Hotkeys page names it; none when unbound.
        shortcut: Option<String>,
        checked: bool,
        enabled: bool,
    },
}

impl Entry {
    /// The native menu text. A tab right-aligns the shortcut, as in any Windows menu.
    fn text(&self) -> Option<String> {
        let Self::Item {
            label, shortcut, ..
        } = self
        else {
            return None;
        };
        Some(match shortcut {
            Some(shortcut) => format!("{label}\t{shortcut}"),
            None => (*label).to_owned(),
        })
    }
}

/// `shortcut` names the current key of a command, if it has one.
pub(crate) fn entries(
    state: State,
    shortcut: impl Fn(WindowCommand) -> Option<String>,
) -> Vec<Entry> {
    let item = |command, label, checked, enabled| Entry::Item {
        command,
        label,
        shortcut: shortcut(command),
        checked,
        enabled,
    };
    let capture = state.capture_controls;
    vec![
        item(
            WindowCommand::ToggleCapture,
            state.transport.label(),
            false,
            capture,
        ),
        item(WindowCommand::Clear, "Clear", false, capture),
        item(WindowCommand::SaveSnapshot, "Save snapshot", false, capture),
        Entry::Separator,
        item(
            WindowCommand::SeparateLanes,
            state.separate_label,
            state.several_rows && state.separate_rows,
            state.several_rows,
        ),
        item(
            WindowCommand::WaveformOnly,
            "Show waveform only",
            state.waveform_only,
            true,
        ),
        item(
            WindowCommand::KeepOnTop,
            "Keep on top",
            state.keep_on_top,
            true,
        ),
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
    use super::{Entry, State, Transport, WindowCommand};
    use crate::hotkeys::{Action, Binding, Hotkeys, Key, Modifiers};

    fn state() -> State {
        State {
            transport: Transport::Pause,
            capture_controls: true,
            waveform_only: false,
            keep_on_top: false,
            several_rows: true,
            separate_rows: false,
            separate_label: "Edit separately",
        }
    }

    /// Names each command's first binding, as the recorder window does in English.
    fn shortcuts(hotkeys: &Hotkeys) -> impl Fn(WindowCommand) -> Option<String> + '_ {
        |command| {
            hotkeys
                .bindings(command.action())
                .first()
                .map(ToString::to_string)
        }
    }

    fn entries(state: State) -> Vec<Entry> {
        super::entries(state, shortcuts(&Hotkeys::defaults()))
    }

    fn texts(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| entry.text().unwrap_or_else(|| "-".into()))
            .collect()
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
                    ..
                } if *command == wanted => Some((*label, *checked, *enabled)),
                _ => None,
            })
            .expect("the menu offers this command")
    }

    #[test]
    fn menu_lists_everyday_commands_in_order_with_their_shortcuts() {
        assert_eq!(
            texts(&entries(state())),
            [
                "Pause\tSpace",
                "Clear\tDelete",
                "Save snapshot",
                "-",
                "Edit separately\tRight Ctrl",
                "Show waveform only",
                "Keep on top",
                "-",
                "Options…",
                "Quit",
            ]
        );
    }

    #[test]
    fn menu_shortcuts_follow_changed_and_cleared_bindings() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        };
        let mut hotkeys = Hotkeys::defaults();
        hotkeys.clear(Action::ToggleCapture);
        hotkeys
            .set(
                Action::Clear,
                &[
                    Binding::Chord {
                        key: Key::K,
                        modifiers: ctrl,
                    },
                    Binding::Chord {
                        key: Key::Delete,
                        modifiers: Modifiers::NONE,
                    },
                ],
            )
            .unwrap();
        hotkeys
            .set(
                Action::SeparateLanes,
                &[Binding::Chord {
                    key: Key::E,
                    modifiers: Modifiers::NONE,
                }],
            )
            .unwrap();
        hotkeys
            .set(
                Action::OpenOptions,
                &[Binding::Chord {
                    key: Key::Comma,
                    modifiers: ctrl,
                }],
            )
            .unwrap();
        assert_eq!(
            texts(&super::entries(state(), shortcuts(&hotkeys))),
            [
                "Pause",
                // The menu names the first of two.
                "Clear\tCtrl + K",
                "Save snapshot",
                "-",
                "Edit separately\tE",
                "Show waveform only",
                "Keep on top",
                "-",
                "Options…\tCtrl + ,",
                "Quit",
            ]
        );
    }

    #[test]
    fn every_command_has_its_action_and_back() {
        for command in [
            WindowCommand::ToggleCapture,
            WindowCommand::Clear,
            WindowCommand::SaveSnapshot,
            WindowCommand::SeparateLanes,
            WindowCommand::WaveformOnly,
            WindowCommand::KeepOnTop,
            WindowCommand::Options,
            WindowCommand::Quit,
        ] {
            assert_eq!(WindowCommand::for_action(command.action()), Some(command));
        }
        assert_eq!(WindowCommand::for_action(Action::CancelSelection), None);
        assert_eq!(WindowCommand::for_action(Action::SaveSnapshotGlobal), None);
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
            assert_eq!(
                item(&entries, WindowCommand::ToggleCapture),
                (label, false, true)
            );
        }

        let off = entries(state());
        assert_eq!(
            item(&off, WindowCommand::WaveformOnly),
            ("Show waveform only", false, true)
        );
        assert_eq!(
            item(&off, WindowCommand::KeepOnTop),
            ("Keep on top", false, true)
        );
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
        for command in [
            WindowCommand::ToggleCapture,
            WindowCommand::Clear,
            WindowCommand::SaveSnapshot,
        ] {
            assert!(!item(&entries, command).2, "{command:?}");
        }
        for command in [
            WindowCommand::SeparateLanes,
            WindowCommand::WaveformOnly,
            WindowCommand::KeepOnTop,
            WindowCommand::Options,
            WindowCommand::Quit,
        ] {
            assert!(item(&entries, command).2, "{command:?}");
        }
    }

    #[test]
    fn edit_separately_needs_more_than_one_row_and_shows_when_it_is_on() {
        let label = "Edit separately";
        let on = entries(State {
            separate_rows: true,
            ..state()
        });
        assert_eq!(item(&on, WindowCommand::SeparateLanes), (label, true, true));
        let off = entries(state());
        assert_eq!(
            item(&off, WindowCommand::SeparateLanes),
            (label, false, true)
        );
        // One merged row has nothing to keep apart.
        let one = entries(State {
            several_rows: false,
            separate_rows: true,
            ..state()
        });
        assert_eq!(
            item(&one, WindowCommand::SeparateLanes),
            (label, false, false)
        );
        // The label is the window's own.
        let other = entries(State {
            separate_label: "分開編輯",
            ..state()
        });
        assert_eq!(item(&other, WindowCommand::SeparateLanes).0, "分開編輯");
    }
}

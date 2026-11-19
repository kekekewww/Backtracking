//! The Hotkeys page of the Options window: the keyboard shortcuts, which the user can change,
//! and the mouse gestures, which are fixed.
//!
//! It also says which shortcut key an egui key press is, for the page that records one and for
//! the recorder window that runs it, so a key that records is a key that works.

use std::io;

use backtracking_ui::{options_field, options_group_heading, options_note, options_value};
use eframe::egui;

use super::addons_page::problem_line;
use super::text::Language;
use crate::hotkeys::{
    Action, Binding, BindingError, GlobalBinding, Hotkeys, Key, MAX_BINDINGS, Modifiers,
    ReservedShortcut, Scope,
};
use crate::snapshot_hotkey::SnapshotHotkey;

/// The shortcut keys egui reports, by egui's name. The key left of 1 is known by where it is
/// instead (see `shortcut_key`).
const KEYS: [(egui::Key, Key); 86] = {
    use egui::Key as E;
    [
        (E::A, Key::A),
        (E::B, Key::B),
        (E::C, Key::C),
        (E::D, Key::D),
        (E::E, Key::E),
        (E::F, Key::F),
        (E::G, Key::G),
        (E::H, Key::H),
        (E::I, Key::I),
        (E::J, Key::J),
        (E::K, Key::K),
        (E::L, Key::L),
        (E::M, Key::M),
        (E::N, Key::N),
        (E::O, Key::O),
        (E::P, Key::P),
        (E::Q, Key::Q),
        (E::R, Key::R),
        (E::S, Key::S),
        (E::T, Key::T),
        (E::U, Key::U),
        (E::V, Key::V),
        (E::W, Key::W),
        (E::X, Key::X),
        (E::Y, Key::Y),
        (E::Z, Key::Z),
        (E::Num0, Key::Digit0),
        (E::Num1, Key::Digit1),
        (E::Num2, Key::Digit2),
        (E::Num3, Key::Digit3),
        (E::Num4, Key::Digit4),
        (E::Num5, Key::Digit5),
        (E::Num6, Key::Digit6),
        (E::Num7, Key::Digit7),
        (E::Num8, Key::Digit8),
        (E::Num9, Key::Digit9),
        (E::F1, Key::F1),
        (E::F2, Key::F2),
        (E::F3, Key::F3),
        (E::F4, Key::F4),
        (E::F5, Key::F5),
        (E::F6, Key::F6),
        (E::F7, Key::F7),
        (E::F8, Key::F8),
        (E::F9, Key::F9),
        (E::F10, Key::F10),
        (E::F11, Key::F11),
        (E::F12, Key::F12),
        (E::F13, Key::F13),
        (E::F14, Key::F14),
        (E::F15, Key::F15),
        (E::F16, Key::F16),
        (E::F17, Key::F17),
        (E::F18, Key::F18),
        (E::F19, Key::F19),
        (E::F20, Key::F20),
        (E::F21, Key::F21),
        (E::F22, Key::F22),
        (E::F23, Key::F23),
        (E::F24, Key::F24),
        (E::Space, Key::Space),
        (E::Enter, Key::Enter),
        (E::Tab, Key::Tab),
        (E::Escape, Key::Escape),
        (E::Backspace, Key::Backspace),
        (E::Delete, Key::Delete),
        (E::Insert, Key::Insert),
        (E::Home, Key::Home),
        (E::End, Key::End),
        (E::PageUp, Key::PageUp),
        (E::PageDown, Key::PageDown),
        (E::ArrowUp, Key::ArrowUp),
        (E::ArrowDown, Key::ArrowDown),
        (E::ArrowLeft, Key::ArrowLeft),
        (E::ArrowRight, Key::ArrowRight),
        (E::Backtick, Key::Backtick),
        (E::Minus, Key::Minus),
        (E::Equals, Key::Equals),
        (E::OpenBracket, Key::OpenBracket),
        (E::CloseBracket, Key::CloseBracket),
        (E::Backslash, Key::Backslash),
        (E::Semicolon, Key::Semicolon),
        (E::Quote, Key::Quote),
        (E::Comma, Key::Comma),
        (E::Period, Key::Period),
        (E::Slash, Key::Slash),
    ]
};

fn named_key(key: egui::Key) -> Option<Key> {
    KEYS.iter()
        .find_map(|&(egui_key, shortcut)| (egui_key == key).then_some(shortcut))
}

/// The shortcut key of a key press. The key left of 1 is known by where it is, as the
/// system-wide shortcut knows it, whatever the layout prints on it. Any other key is known by
/// what the layout makes of it, or else by where it is: Shift + 1 reads as `!`, which is no
/// shortcut key, and is taken as 1.
pub(super) fn shortcut_key(key: egui::Key, physical: Option<egui::Key>) -> Option<Key> {
    if physical == Some(egui::Key::Backtick) {
        return Some(Key::KeyLeftOf1);
    }
    named_key(key).or_else(|| physical.and_then(named_key))
}

/// egui does not report the Windows key, so a recorded or pressed chord never holds it. Windows
/// reports Ctrl as both `ctrl` and `command`.
pub(super) fn shortcut_modifiers(modifiers: egui::Modifiers) -> Modifiers {
    Modifiers {
        ctrl: modifiers.ctrl || modifiers.command,
        shift: modifiers.shift,
        alt: modifiers.alt,
        win: false,
    }
}

/// A key going down, not a repeat or a release, as a shortcut chord.
pub(super) fn pressed_chord(event: &egui::Event) -> Option<(Modifiers, Key)> {
    let egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        repeat: false,
        modifiers,
    } = event
    else {
        return None;
    };
    Some((
        shortcut_modifiers(*modifiers),
        shortcut_key(*key, *physical_key)?,
    ))
}

/// The key press egui-winit would report for a chord on a US layout; for the test harness.
pub(super) fn key_event(modifiers: Modifiers, key: Key, pressed: bool) -> egui::Event {
    let (key, physical_key) = if key == Key::KeyLeftOf1 {
        (egui::Key::Backtick, Some(egui::Key::Backtick))
    } else {
        let egui_key = KEYS
            .iter()
            .find_map(|&(egui_key, shortcut)| (shortcut == key).then_some(egui_key))
            .expect("every other shortcut key is an egui key");
        // A logical backtick away from the key left of 1, as on some layouts.
        (egui_key, (key != Key::Backtick).then_some(egui_key))
    };
    egui::Event::Key {
        key,
        physical_key,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers {
            alt: modifiers.alt,
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
            mac_cmd: false,
            // Windows reports Ctrl as both.
            command: modifiers.ctrl,
        },
    }
}

/// A binding as the page and the menu name it.
pub(super) fn binding_text(language: Language, binding: Binding) -> String {
    let text = binding.to_string();
    match binding {
        Binding::Chord {
            key: Key::KeyLeftOf1,
            ..
        } => text.replace(&Key::KeyLeftOf1.to_string(), language.key_left_of_1()),
        _ => text,
    }
}

/// All of an action's bindings on one line, or that it has none.
pub(super) fn bindings_text(language: Language, bindings: &[Binding]) -> String {
    if bindings.is_empty() {
        return language.hotkey_not_set().to_owned();
    }
    bindings
        .iter()
        .map(|&binding| binding_text(language, binding))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the right-click menu shows beside a command: its first binding, nothing when unbound.
pub(super) fn shortcut_label(
    language: Language,
    hotkeys: &Hotkeys,
    action: Action,
) -> Option<String> {
    hotkeys
        .bindings(action)
        .first()
        .map(|&binding| binding_text(language, binding))
}

/// An action by its id in the settings file.
pub(super) fn parse_action(id: &str) -> Result<Action, String> {
    Action::ALL
        .into_iter()
        .find(|action| action.id() == id.trim())
        .ok_or_else(|| format!("{id} is not a shortcut action"))
}

/// A binding written as modifiers and a key joined by `+`, the key named as in the settings file:
/// `ctrl+shift+s`, `space`, `alt+key-left-of-1`, `f6`. `right-ctrl-tap` is the tap.
pub(super) fn parse_binding(text: &str) -> Result<Binding, String> {
    let text = text.trim().to_ascii_lowercase();
    if text == "right-ctrl-tap" {
        return Ok(Binding::RightCtrlTap);
    }
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let key = parts.pop().filter(|key| !key.is_empty());
    let mut modifiers = Modifiers::NONE;
    for part in parts {
        match part {
            "ctrl" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" => modifiers.alt = true,
            "win" => modifiers.win = true,
            other => return Err(format!("{other} is not ctrl, shift, alt or win")),
        }
    }
    let key = key
        .map(|key| {
            serde_json::from_value::<Key>(serde_json::Value::String(key.to_owned()))
                .map_err(|_| format!("{key} is not a key"))
        })
        .transpose()?;
    Binding::from_key(modifiers, key).map_err(|_| format!("{text} has no key"))
}

/// What the page asks of the system-wide shortcut. `SnapshotHotkey` does it for real; tests use
/// a fake, so no real shortcut is registered.
pub(super) trait GlobalShortcut {
    fn set_bindings(&self, bindings: [Option<GlobalBinding>; MAX_BINDINGS]) -> io::Result<()>;
    fn suspend(&self) -> io::Result<()>;
    fn resume(&self) -> io::Result<()>;
}

impl GlobalShortcut for SnapshotHotkey {
    fn set_bindings(&self, bindings: [Option<GlobalBinding>; MAX_BINDINGS]) -> io::Result<()> {
        SnapshotHotkey::set_bindings(self, bindings)
    }

    fn suspend(&self) -> io::Result<()> {
        SnapshotHotkey::suspend(self)
    }

    fn resume(&self) -> io::Result<()> {
        SnapshotHotkey::resume(self)
    }
}

/// The system-wide shortcut, where this window has one, for the page.
pub(super) fn global(hotkey: Option<&SnapshotHotkey>) -> Option<&dyn GlobalShortcut> {
    hotkey.map(|hotkey| hotkey as &dyn GlobalShortcut)
}

/// One of an action's bindings, counted from 0. The one past the last adds a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Slot {
    pub(super) action: Action,
    pub(super) index: usize,
}

/// What the user did on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edit {
    /// Starts recording into the slot, or stops when it is already recording.
    Record(Slot),
    /// Puts a tap of Right Ctrl in the slot being recorded.
    RightCtrl(Slot),
    Clear(Action),
    Reset(Action),
    ResetAll,
}

/// What reached the Options window while a row was recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Recorded {
    Chord(Modifiers, Key),
    /// egui turns the copy, cut and paste keys into those commands, so their keys never arrive.
    ClipboardKey,
    /// The window lost the keyboard, so the system-wide shortcut must not stay released.
    FocusLost,
}

/// Takes the keys out of the Options window's input while a row records, so no button and not
/// the window's own Escape sees them, and says what the user pressed.
pub(super) fn take_recorded(events: &mut Vec<egui::Event>) -> Option<Recorded> {
    let mut recorded = None;
    events.retain(|event| {
        let (keep, found) = match event {
            egui::Event::Key { .. } => (
                false,
                pressed_chord(event).map(|(modifiers, key)| Recorded::Chord(modifiers, key)),
            ),
            egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) => {
                (false, Some(Recorded::ClipboardKey))
            }
            egui::Event::Text(_) => (false, None),
            egui::Event::WindowFocused(false) => (true, Some(Recorded::FocusLost)),
            _ => (true, None),
        };
        if found == Some(Recorded::FocusLost) || recorded.is_none() {
            recorded = found.or(recorded);
        }
        keep
    });
    recorded
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Problem {
    Binding(BindingError),
    /// The system-wide shortcut's worker could not be asked.
    Unavailable(String),
}

impl Problem {
    fn text(&self, language: Language) -> String {
        match self {
            Self::Binding(error) => language.hotkey_problem(*error),
            Self::Unavailable(error) => language.could_not_register(error),
        }
    }
}

/// The page's own state: which row records, and why the last change was refused. The bindings
/// themselves belong to the recorder window, which runs them.
#[derive(Debug, Default)]
pub(super) struct HotkeysPage {
    recording: Option<Slot>,
    problem: Option<(Action, Problem)>,
    /// Whether the page was drawn since the last `end_pass`.
    shown: bool,
}

impl HotkeysPage {
    pub(super) fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Ends the window's pass. A row records only while the page is on screen, so when this
    /// pass did not draw the page (another page, a closed or hidden Options window), recording
    /// stops here.
    pub(super) fn end_pass(&mut self, global: Option<&dyn GlobalShortcut>) {
        if !std::mem::take(&mut self.shown) {
            self.stop_recording(global);
        }
    }

    /// Ends recording without a change and lets the system-wide shortcut work again. Every way
    /// out of recording comes here.
    pub(super) fn stop_recording(&mut self, global: Option<&dyn GlobalShortcut>) {
        if self.recording.take().is_some() {
            self.ask(global, |global| global.resume());
        }
    }

    fn ask(
        &mut self,
        global: Option<&dyn GlobalShortcut>,
        request: impl FnOnce(&dyn GlobalShortcut) -> io::Result<()>,
    ) {
        if let Some(Err(error)) = global.map(request) {
            self.problem = Some((
                Action::SaveSnapshotGlobal,
                Problem::Unavailable(error.to_string()),
            ));
        }
    }

    /// Does one edit. True when the bindings changed and need saving.
    pub(super) fn apply(
        &mut self,
        edit: Edit,
        hotkeys: &mut Hotkeys,
        global: Option<&dyn GlobalShortcut>,
    ) -> bool {
        let before = hotkeys.clone();
        match edit {
            Edit::Record(slot) if self.recording == Some(slot) => self.stop_recording(global),
            Edit::Record(slot) => {
                self.problem = None;
                // The system-wide shortcut would take its own keys before this window saw them.
                if self.recording.is_none() {
                    self.ask(global, |global| global.suspend());
                }
                self.recording = Some(slot);
            }
            Edit::RightCtrl(slot) => {
                self.bind(slot, Binding::RightCtrlTap, hotkeys, global);
                self.stop_recording(global);
            }
            Edit::Clear(action) => {
                self.stop_recording(global);
                let _ = self.set_bindings(action, &[], hotkeys, global);
            }
            Edit::Reset(action) => {
                self.stop_recording(global);
                let result = hotkeys.reset(action);
                self.changed(action, result, hotkeys, global);
            }
            Edit::ResetAll => {
                self.stop_recording(global);
                hotkeys.reset_all();
                self.changed(Action::SaveSnapshotGlobal, Ok(()), hotkeys, global);
            }
        }
        *hotkeys != before
    }

    /// Takes what was pressed while a row records. True when the bindings changed.
    pub(super) fn recorded(
        &mut self,
        recorded: Recorded,
        hotkeys: &mut Hotkeys,
        global: Option<&dyn GlobalShortcut>,
    ) -> bool {
        let Some(slot) = self.recording else {
            return false;
        };
        let before = hotkeys.clone();
        match recorded {
            // Escape stops recording, so it can only come back to a row through Reset.
            Recorded::Chord(_, Key::Escape) | Recorded::FocusLost => {}
            Recorded::Chord(modifiers, key) => {
                let binding = Binding::from_key(modifiers, Some(key));
                match binding {
                    Ok(binding) => self.bind(slot, binding, hotkeys, global),
                    Err(error) => self.problem = Some((slot.action, Problem::Binding(error))),
                }
            }
            Recorded::ClipboardKey => {
                let error = BindingError::Reserved(ReservedShortcut::Clipboard);
                self.problem = Some((slot.action, Problem::Binding(error)));
            }
        }
        // A new system-wide chord is set while released, and registers once on resume.
        self.stop_recording(global);
        *hotkeys != before
    }

    /// Puts `binding` in the slot, keeping the action's other binding.
    fn bind(
        &mut self,
        slot: Slot,
        binding: Binding,
        hotkeys: &mut Hotkeys,
        global: Option<&dyn GlobalShortcut>,
    ) {
        let mut bindings = hotkeys.bindings(slot.action).to_vec();
        if slot.index < bindings.len() {
            bindings[slot.index] = binding;
        } else {
            bindings.push(binding);
        }
        // The same binding twice is refused, and the row says so.
        let _ = self.set_bindings(slot.action, &bindings, hotkeys, global);
    }

    /// Replaces an action's bindings, as the page does and as a test copy's `bind=` step does.
    /// A refused change leaves every binding as it was and shows why under the action.
    pub(super) fn set_bindings(
        &mut self,
        action: Action,
        bindings: &[Binding],
        hotkeys: &mut Hotkeys,
        global: Option<&dyn GlobalShortcut>,
    ) -> Result<(), BindingError> {
        let result = hotkeys.set(action, bindings);
        self.changed(action, result, hotkeys, global);
        result
    }

    fn changed(
        &mut self,
        action: Action,
        result: Result<(), BindingError>,
        hotkeys: &Hotkeys,
        global: Option<&dyn GlobalShortcut>,
    ) {
        match result {
            Ok(()) => {
                self.problem = None;
                if action.scope() == Scope::Global {
                    self.ask(global, |global| {
                        global.set_bindings(hotkeys.global_bindings())
                    });
                }
            }
            Err(error) => self.problem = Some((action, Problem::Binding(error))),
        }
    }

    /// Draws the page and does what the user asked. True when the bindings changed and need
    /// saving. `primary` is the main window, which has the system-wide shortcut and keeps
    /// changes; `global_error` is why Windows refused the shortcut.
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        language: Language,
        hotkeys: &mut Hotkeys,
        global: Option<&dyn GlobalShortcut>,
        global_error: Option<&str>,
        primary: bool,
    ) -> bool {
        self.shown = true;
        let edits = self.draw(ui, language, hotkeys, global_error, primary);
        let mut changed = false;
        for edit in edits {
            changed |= self.apply(edit, hotkeys, global);
        }
        changed
    }

    fn draw(
        &self,
        ui: &mut egui::Ui,
        language: Language,
        hotkeys: &Hotkeys,
        global_error: Option<&str>,
        primary: bool,
    ) -> Vec<Edit> {
        let defaults = Hotkeys::defaults();
        let mut edits = Vec::new();
        if !primary {
            options_note(ui, language.hotkeys_this_window_only());
        }
        options_note(ui, language.hotkeys_explained());
        if primary {
            options_group_heading(ui, language.hotkeys_system_wide());
            options_note(ui, language.hotkeys_system_wide_note());
            self.row(
                ui,
                language,
                hotkeys,
                &defaults,
                Action::SaveSnapshotGlobal,
                global_error,
                &mut edits,
            );
            ui.separator();
        }
        options_group_heading(ui, language.hotkeys_in_window());
        options_note(ui, language.hotkeys_in_window_note());
        for action in Action::ALL {
            if action.scope() == Scope::Window {
                self.row(ui, language, hotkeys, &defaults, action, None, &mut edits);
            }
        }
        // Reset all also restores the system-wide shortcut, even where it is not shown.
        if ui
            .add_enabled(
                *hotkeys != defaults,
                egui::Button::new(language.hotkeys_reset_all()),
            )
            .clicked()
        {
            edits.push(Edit::ResetAll);
        }
        ui.separator();
        options_group_heading(ui, language.mouse_heading());
        for (gesture, meaning) in language.mouse_gestures() {
            options_field(ui, gesture, |ui| options_value(ui, meaning));
        }
        edits
    }

    /// One action: a button per binding, which records a new one, then Clear and Reset.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        ui: &mut egui::Ui,
        language: Language,
        hotkeys: &Hotkeys,
        defaults: &Hotkeys,
        action: Action,
        global_error: Option<&str>,
        edits: &mut Vec<Edit>,
    ) {
        let bindings = hotkeys.bindings(action);
        let recording = self.recording.filter(|slot| slot.action == action);
        options_field(ui, language.hotkey_action(action), |ui| {
            ui.horizontal_wrapped(|ui| {
                // A second button appears once there is a first binding.
                let slots = (bindings.len() + 1).clamp(1, MAX_BINDINGS);
                for index in 0..slots {
                    let slot = Slot { action, index };
                    let text = if recording == Some(slot) {
                        language.hotkey_press_keys().to_owned()
                    } else if let Some(&binding) = bindings.get(index) {
                        binding_text(language, binding)
                    } else if index == 0 {
                        language.hotkey_not_set().to_owned()
                    } else {
                        language.hotkey_add().to_owned()
                    };
                    let button = egui::Button::new(text).selected(recording == Some(slot));
                    if ui.add(button).clicked() {
                        edits.push(Edit::Record(slot));
                    }
                }
                if action == Action::SeparateLanes
                    && let Some(slot) = recording
                    && ui.button(language.hotkey_right_ctrl()).clicked()
                {
                    edits.push(Edit::RightCtrl(slot));
                }
                if ui
                    .add_enabled(
                        !bindings.is_empty(),
                        egui::Button::new(language.hotkey_clear()),
                    )
                    .clicked()
                {
                    edits.push(Edit::Clear(action));
                }
                if ui
                    .add_enabled(
                        bindings != defaults.bindings(action),
                        egui::Button::new(language.hotkey_reset()),
                    )
                    .clicked()
                {
                    edits.push(Edit::Reset(action));
                }
            });
            if recording.is_some() {
                options_note(ui, language.hotkey_recording_note());
            }
            if let Some((_, problem)) = self.problem.as_ref().filter(|(at, _)| *at == action) {
                problem_line(ui, &problem.text(language));
            }
            if let Some(error) = global_error {
                problem_line(ui, &language.could_not_register(error));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    use super::*;
    use crate::hotkeys::GlobalKey;
    use Language::{English, TraditionalChinese};

    /// Writes down what the page asked of the system-wide shortcut.
    #[derive(Default)]
    struct FakeGlobal {
        calls: RefCell<Vec<String>>,
        fail: bool,
    }

    impl FakeGlobal {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.calls.borrow_mut())
        }

        fn answer(&self, call: String) -> io::Result<()> {
            self.calls.borrow_mut().push(call);
            if self.fail {
                Err(io::Error::other("worker stopped"))
            } else {
                Ok(())
            }
        }
    }

    impl GlobalShortcut for FakeGlobal {
        fn set_bindings(&self, bindings: [Option<GlobalBinding>; MAX_BINDINGS]) -> io::Result<()> {
            let keys: Vec<String> = bindings
                .iter()
                .map(|binding| match binding.map(GlobalBinding::key) {
                    Some(GlobalKey::VirtualKey(code)) => format!("vk {code:#x}"),
                    Some(GlobalKey::ScanCode(code)) => format!("scan {code:#x}"),
                    None => "none".into(),
                })
                .collect();
            self.answer(format!("set {}", keys.join(", ")))
        }

        fn suspend(&self) -> io::Result<()> {
            self.answer("suspend".into())
        }

        fn resume(&self) -> io::Result<()> {
            self.answer("resume".into())
        }
    }

    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    const ALT: Modifiers = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };

    fn chord(modifiers: Modifiers, key: Key) -> Binding {
        Binding::Chord { modifiers, key }
    }

    fn slot(action: Action, index: usize) -> Slot {
        Slot { action, index }
    }

    #[test]
    fn every_shortcut_key_comes_from_an_egui_key_and_goes_back() {
        let mut keys = BTreeSet::new();
        for &key in egui::Key::ALL {
            for physical in [None, Some(key)] {
                if let Some(shortcut) = shortcut_key(key, physical) {
                    keys.insert(serde_json::to_string(&shortcut).unwrap());
                    let (modifiers, back) =
                        pressed_chord(&key_event(Modifiers::NONE, shortcut, true)).unwrap();
                    assert_eq!((modifiers, back), (Modifiers::NONE, shortcut), "{key:?}");
                }
            }
        }
        // 26 letters, 10 digits, 24 function keys, 15 named keys, 11 symbols and the key left
        // of 1: every key the settings file can name.
        assert_eq!(keys.len(), 87, "{keys:?}");
        assert_eq!(
            KEYS.iter()
                .map(|(key, _)| key)
                .collect::<BTreeSet<_>>()
                .len(),
            KEYS.len()
        );
        // The key left of 1 is known by where it is, whatever the layout calls it.
        assert_eq!(
            shortcut_key(egui::Key::Backtick, Some(egui::Key::Backtick)),
            Some(Key::KeyLeftOf1)
        );
        assert_eq!(
            shortcut_key(egui::Key::Num2, Some(egui::Key::Backtick)),
            Some(Key::KeyLeftOf1)
        );
        assert_eq!(shortcut_key(egui::Key::Backtick, None), Some(Key::Backtick));
        // Shift + 1 reads as `!`, and Shift + = as `+`: the key is the one under the finger.
        assert_eq!(
            shortcut_key(egui::Key::Exclamationmark, Some(egui::Key::Num1)),
            Some(Key::Digit1)
        );
        assert_eq!(
            shortcut_key(egui::Key::Plus, Some(egui::Key::Equals)),
            Some(Key::Equals)
        );
        assert_eq!(shortcut_key(egui::Key::F25, Some(egui::Key::F25)), None);
        assert_eq!(shortcut_key(egui::Key::MediaPlayPause, None), None);
    }

    #[test]
    fn a_chord_is_a_press_with_its_modifiers_and_never_a_repeat() {
        let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT | egui::Modifiers::COMMAND;
        let press = |pressed, repeat| egui::Event::Key {
            key: egui::Key::S,
            physical_key: Some(egui::Key::S),
            pressed,
            repeat,
            modifiers: ctrl_shift,
        };
        let both = Modifiers {
            shift: true,
            ..CTRL
        };
        assert_eq!(pressed_chord(&press(true, false)), Some((both, Key::S)));
        assert_eq!(pressed_chord(&press(true, true)), None);
        assert_eq!(pressed_chord(&press(false, false)), None);
        assert_eq!(pressed_chord(&egui::Event::Text("s".into())), None);
        assert_eq!(
            pressed_chord(&key_event(both, Key::S, true)),
            Some((both, Key::S))
        );
    }

    #[test]
    fn bindings_read_as_the_menu_and_the_actions_page_name_them() {
        let defaults = Hotkeys::defaults();
        let menu = |action| shortcut_label(English, &defaults, action);
        assert_eq!(menu(Action::ToggleCapture).as_deref(), Some("Space"));
        assert_eq!(menu(Action::Clear).as_deref(), Some("Delete"));
        assert_eq!(menu(Action::SeparateLanes).as_deref(), Some("Right Ctrl"));
        assert_eq!(menu(Action::SaveSnapshot), None);
        assert_eq!(
            bindings_text(English, defaults.bindings(Action::SaveSnapshotGlobal)),
            "Alt + the key left of 1"
        );
        assert_eq!(
            bindings_text(
                TraditionalChinese,
                defaults.bindings(Action::SaveSnapshotGlobal)
            ),
            "Alt + 1 左邊的鍵"
        );
        assert_eq!(
            bindings_text(English, defaults.bindings(Action::CancelSelection)),
            "Escape, Backspace"
        );
        assert_eq!(bindings_text(English, &[]), "Not set");
        assert_eq!(bindings_text(TraditionalChinese, &[]), "未設定");
        assert_eq!(
            shortcut_label(TraditionalChinese, &defaults, Action::ToggleCapture).as_deref(),
            Some("Space")
        );
    }

    #[test]
    fn harness_bindings_are_written_as_modifiers_and_a_key() {
        assert_eq!(
            parse_binding("space"),
            Ok(chord(Modifiers::NONE, Key::Space))
        );
        assert_eq!(
            parse_binding(" Ctrl+Shift+S "),
            Ok(chord(
                Modifiers {
                    shift: true,
                    ..CTRL
                },
                Key::S
            ))
        );
        assert_eq!(
            parse_binding("alt + key-left-of-1"),
            Ok(chord(ALT, Key::KeyLeftOf1))
        );
        assert_eq!(parse_binding("f6"), Ok(chord(Modifiers::NONE, Key::F6)));
        assert_eq!(
            parse_binding("page-down"),
            Ok(chord(Modifiers::NONE, Key::PageDown))
        );
        assert_eq!(
            parse_binding("digit1"),
            Ok(chord(Modifiers::NONE, Key::Digit1))
        );
        assert_eq!(parse_binding("right-ctrl-tap"), Ok(Binding::RightCtrlTap));
        for bad in ["", "ctrl+", "ctrl", "hyper+s", "ctrl+banana", "f25"] {
            assert!(parse_binding(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_action("open-options"), Ok(Action::OpenOptions));
        assert!(parse_action("open").is_err());
    }

    #[test]
    fn recording_binds_the_next_chord_and_releases_the_global_shortcut_meanwhile() {
        let global = FakeGlobal::default();
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        let target = slot(Action::OpenOptions, 0);
        assert!(!page.apply(Edit::Record(target), &mut hotkeys, Some(&global)));
        assert!(page.is_recording());
        assert_eq!(global.take(), ["suspend"]);
        assert!(page.recorded(Recorded::Chord(CTRL, Key::O), &mut hotkeys, Some(&global)));
        assert!(!page.is_recording());
        assert_eq!(hotkeys.bindings(Action::OpenOptions), [chord(CTRL, Key::O)]);
        // An in-window binding leaves the system-wide one alone.
        assert_eq!(global.take(), ["resume"]);

        // A second binding goes beside the first; recording over the first replaces it.
        page.apply(
            Edit::Record(slot(Action::OpenOptions, 1)),
            &mut hotkeys,
            Some(&global),
        );
        page.recorded(
            Recorded::Chord(Modifiers::NONE, Key::F6),
            &mut hotkeys,
            Some(&global),
        );
        page.apply(Edit::Record(target), &mut hotkeys, Some(&global));
        page.recorded(Recorded::Chord(ALT, Key::O), &mut hotkeys, Some(&global));
        assert_eq!(
            hotkeys.bindings(Action::OpenOptions),
            [chord(ALT, Key::O), chord(Modifiers::NONE, Key::F6)]
        );
        // The same chord in both slots is refused: both stay, and the row says why.
        page.apply(Edit::Record(target), &mut hotkeys, Some(&global));
        assert!(!page.recorded(
            Recorded::Chord(Modifiers::NONE, Key::F6),
            &mut hotkeys,
            Some(&global),
        ));
        assert_eq!(
            hotkeys.bindings(Action::OpenOptions),
            [chord(ALT, Key::O), chord(Modifiers::NONE, Key::F6)]
        );
        assert_eq!(
            page.problem
                .as_ref()
                .map(|(at, problem)| (*at, problem.text(English))),
            Some((
                Action::OpenOptions,
                English.hotkey_problem(BindingError::Reserved(ReservedShortcut::DuplicateBinding))
            ))
        );
        assert_eq!(
            global.take(),
            [
                "suspend", "resume", "suspend", "resume", "suspend", "resume"
            ]
        );
    }

    #[test]
    fn a_new_global_chord_is_set_while_released_and_registers_on_resume() {
        let global = FakeGlobal::default();
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        let target = slot(Action::SaveSnapshotGlobal, 0);
        page.apply(Edit::Record(target), &mut hotkeys, Some(&global));
        assert!(page.recorded(Recorded::Chord(CTRL, Key::F9), &mut hotkeys, Some(&global)));
        assert_eq!(global.take(), ["suspend", "set vk 0x78, none", "resume"]);
        // A system-wide shortcut needs a modifier: the old one stays and the row says why.
        page.apply(Edit::Record(target), &mut hotkeys, Some(&global));
        assert!(!page.recorded(
            Recorded::Chord(Modifiers::NONE, Key::F9),
            &mut hotkeys,
            Some(&global)
        ));
        assert_eq!(
            hotkeys.bindings(Action::SaveSnapshotGlobal),
            [chord(CTRL, Key::F9)]
        );
        assert_eq!(
            page.problem,
            Some((
                Action::SaveSnapshotGlobal,
                Problem::Binding(BindingError::GlobalNeedsModifier)
            ))
        );
        assert_eq!(global.take(), ["suspend", "resume"]);
        assert!(page.apply(
            Edit::Reset(Action::SaveSnapshotGlobal),
            &mut hotkeys,
            Some(&global)
        ));
        assert_eq!(global.take(), ["set scan 0x29, none"]);
        assert_eq!(page.problem, None);
    }

    #[test]
    fn escape_focus_loss_and_a_second_click_stop_recording_without_a_change() {
        let global = FakeGlobal::default();
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        let target = slot(Action::ToggleCapture, 0);
        for stop in [
            Recorded::Chord(Modifiers::NONE, Key::Escape),
            Recorded::Chord(CTRL, Key::Escape),
            Recorded::FocusLost,
        ] {
            page.apply(Edit::Record(target), &mut hotkeys, Some(&global));
            assert!(!page.recorded(stop, &mut hotkeys, Some(&global)));
            assert!(!page.is_recording());
            assert_eq!(hotkeys, Hotkeys::defaults());
            assert_eq!(global.take(), ["suspend", "resume"], "{stop:?}");
        }
        page.apply(Edit::Record(target), &mut hotkeys, Some(&global));
        // Moving to another row keeps the shortcut released; clicking the same one stops.
        page.apply(
            Edit::Record(slot(Action::Clear, 0)),
            &mut hotkeys,
            Some(&global),
        );
        assert!(page.is_recording());
        page.apply(
            Edit::Record(slot(Action::Clear, 0)),
            &mut hotkeys,
            Some(&global),
        );
        assert!(!page.is_recording());
        assert_eq!(global.take(), ["suspend", "resume"]);
        // Stopping twice resumes once. Leaving the page and closing the window have their own
        // tests, through `end_pass`.
        page.apply(Edit::Record(target), &mut hotkeys, Some(&global));
        page.stop_recording(Some(&global));
        page.stop_recording(Some(&global));
        assert_eq!(global.take(), ["suspend", "resume"]);
        // Clear, Reset and Reset all end it too.
        for edit in [
            Edit::Clear(Action::Quit),
            Edit::Reset(Action::Quit),
            Edit::ResetAll,
        ] {
            page.apply(Edit::Record(target), &mut hotkeys, Some(&global));
            page.apply(edit, &mut hotkeys, Some(&global));
            assert!(!page.is_recording());
            let calls = global.take();
            assert_eq!(&calls[..2], ["suspend", "resume"], "{edit:?}");
        }
        // Nothing is pressed and nothing changes without recording.
        assert!(!page.recorded(
            Recorded::Chord(Modifiers::NONE, Key::F6),
            &mut hotkeys,
            Some(&global)
        ));
        assert!(global.take().is_empty());
    }

    #[test]
    fn a_conflict_names_the_other_action_and_keeps_the_old_binding() {
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        page.apply(Edit::Record(slot(Action::KeepOnTop, 0)), &mut hotkeys, None);
        assert!(!page.recorded(
            Recorded::Chord(Modifiers::NONE, Key::Space),
            &mut hotkeys,
            None
        ));
        assert!(hotkeys.bindings(Action::KeepOnTop).is_empty());
        assert_eq!(
            hotkeys.bindings(Action::ToggleCapture),
            [chord(Modifiers::NONE, Key::Space)]
        );
        let problem = page.problem.clone().unwrap();
        assert_eq!(problem.0, Action::KeepOnTop);
        assert_eq!(
            problem.1.text(English),
            "Already used by Pause or resume capture."
        );
        // Reserved keys and the clipboard keys are refused the same way.
        page.apply(Edit::Record(slot(Action::Quit, 0)), &mut hotkeys, None);
        page.recorded(Recorded::Chord(ALT, Key::F4), &mut hotkeys, None);
        assert_eq!(
            page.problem
                .as_ref()
                .map(|(_, problem)| problem.text(English)),
            Some("Windows keeps this shortcut for itself.".into())
        );
        page.apply(Edit::Record(slot(Action::Quit, 0)), &mut hotkeys, None);
        page.recorded(Recorded::ClipboardKey, &mut hotkeys, None);
        assert_eq!(
            page.problem,
            Some((
                Action::Quit,
                Problem::Binding(BindingError::Reserved(ReservedShortcut::Clipboard))
            ))
        );
        assert!(hotkeys.bindings(Action::Quit).is_empty());
        // Starting again clears the note.
        page.apply(Edit::Record(slot(Action::Quit, 0)), &mut hotkeys, None);
        assert_eq!(page.problem, None);
    }

    #[test]
    fn clear_reset_and_reset_all_change_only_what_they_name() {
        let global = FakeGlobal::default();
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        assert!(page.apply(
            Edit::Clear(Action::CancelSelection),
            &mut hotkeys,
            Some(&global)
        ));
        assert!(hotkeys.bindings(Action::CancelSelection).is_empty());
        assert!(!page.apply(Edit::Clear(Action::CancelSelection), &mut hotkeys, None));
        page.set_bindings(
            Action::ToggleCapture,
            &[chord(Modifiers::NONE, Key::F6)],
            &mut hotkeys,
            Some(&global),
        )
        .unwrap();
        assert!(page.apply(
            Edit::Reset(Action::CancelSelection),
            &mut hotkeys,
            Some(&global)
        ));
        assert_eq!(
            hotkeys.bindings(Action::CancelSelection),
            Hotkeys::defaults().bindings(Action::CancelSelection)
        );
        assert_eq!(
            hotkeys.bindings(Action::ToggleCapture),
            [chord(Modifiers::NONE, Key::F6)]
        );
        // Space went to another action, so ToggleCapture's default cannot come back alone.
        page.set_bindings(
            Action::Quit,
            &[chord(Modifiers::NONE, Key::Space)],
            &mut hotkeys,
            Some(&global),
        )
        .unwrap();
        assert!(!page.apply(
            Edit::Reset(Action::ToggleCapture),
            &mut hotkeys,
            Some(&global)
        ));
        assert_eq!(
            page.problem
                .as_ref()
                .map(|(at, problem)| (*at, problem.text(English))),
            Some((Action::ToggleCapture, "Already used by Quit.".into()))
        );
        // In-window edits never touch the system-wide shortcut.
        assert!(global.take().is_empty());
        assert!(page.apply(Edit::ResetAll, &mut hotkeys, Some(&global)));
        assert_eq!(hotkeys, Hotkeys::defaults());
        assert_eq!(global.take(), ["set scan 0x29, none"]);
        assert_eq!(page.problem, None);
    }

    #[test]
    fn edit_separately_can_have_the_right_ctrl_tap_back() {
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        let target = slot(Action::SeparateLanes, 0);
        page.apply(Edit::Record(target), &mut hotkeys, None);
        page.recorded(Recorded::Chord(Modifiers::NONE, Key::E), &mut hotkeys, None);
        assert_eq!(
            hotkeys.find_binding(Scope::Window, Binding::RightCtrlTap),
            None
        );
        page.apply(
            Edit::Record(slot(Action::SeparateLanes, 1)),
            &mut hotkeys,
            None,
        );
        assert!(page.apply(
            Edit::RightCtrl(slot(Action::SeparateLanes, 1)),
            &mut hotkeys,
            None
        ));
        assert!(!page.is_recording());
        assert_eq!(
            hotkeys.bindings(Action::SeparateLanes),
            [chord(Modifiers::NONE, Key::E), Binding::RightCtrlTap]
        );
    }

    #[test]
    fn a_global_shortcut_that_cannot_be_asked_says_so_under_its_row() {
        let global = FakeGlobal {
            fail: true,
            ..FakeGlobal::default()
        };
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        page.apply(
            Edit::Record(slot(Action::Quit, 0)),
            &mut hotkeys,
            Some(&global),
        );
        assert_eq!(
            page.problem
                .as_ref()
                .map(|(at, problem)| (*at, problem.text(English))),
            Some((
                Action::SaveSnapshotGlobal,
                "Could not register the shortcut: worker stopped".into()
            ))
        );
        page.stop_recording(Some(&global));
        assert!(!page.is_recording());
    }

    #[test]
    fn recording_takes_the_keys_from_the_window_and_leaves_the_rest() {
        let mut events = vec![
            egui::Event::PointerMoved(egui::Pos2::ZERO),
            key_event(Modifiers::NONE, Key::Space, false),
            egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: Some(egui::Key::Escape),
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::NONE,
            },
            key_event(CTRL, Key::K, true),
            egui::Event::Text("k".into()),
            key_event(Modifiers::NONE, Key::Delete, true),
        ];
        assert_eq!(
            take_recorded(&mut events),
            Some(Recorded::Chord(CTRL, Key::K))
        );
        assert_eq!(events, [egui::Event::PointerMoved(egui::Pos2::ZERO)]);
        let mut events = vec![egui::Event::Copy];
        assert_eq!(take_recorded(&mut events), Some(Recorded::ClipboardKey));
        assert!(events.is_empty());
        let mut events = vec![
            key_event(CTRL, Key::K, true),
            egui::Event::WindowFocused(false),
        ];
        assert_eq!(take_recorded(&mut events), Some(Recorded::FocusLost));
        assert_eq!(events, [egui::Event::WindowFocused(false)]);
        assert_eq!(take_recorded(&mut Vec::new()), None);
    }

    /// Every text a pass laid out, in the order it was painted.
    fn laid_out(output: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, texts: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, texts)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Runs one headless pass of the recorder window, in the order `Recorder::ui` runs it: the
    /// Options window draws the Hotkeys page only when `on_page`, else another page or nothing,
    /// then the pass ends. Returns whether the bindings changed and what was laid out.
    fn pass(
        page: &mut HotkeysPage,
        hotkeys: &mut Hotkeys,
        language: Language,
        global: Option<&dyn GlobalShortcut>,
        global_error: Option<&str>,
        primary: bool,
        on_page: bool,
    ) -> (bool, Vec<String>) {
        let mut changed = false;
        let output = egui::Context::default().run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 1400.0),
                )),
                ..Default::default()
            },
            |ui| {
                backtracking_ui::options_list(ui, |ui| {
                    if on_page {
                        changed = page.show(ui, language, hotkeys, global, global_error, primary);
                    } else {
                        options_note(ui, "Another page");
                    }
                });
            },
        );
        page.end_pass(global);
        let texts = laid_out(&output);
        output.drop_without_applying_deltas();
        (changed, texts)
    }

    #[test]
    fn every_screen_draws_in_both_languages() {
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        for language in [English, TraditionalChinese] {
            let (changed, texts) = pass(
                &mut page,
                &mut hotkeys,
                language,
                None,
                Some("busy"),
                true,
                true,
            );
            assert!(!changed);
            let global_key = match language {
                English => "Alt + the key left of 1",
                TraditionalChinese => "Alt + 1 左邊的鍵",
            };
            let busy = language.could_not_register("busy");
            for wanted in [
                language.hotkeys_explained(),
                language.hotkeys_system_wide(),
                language.hotkey_action(Action::SaveSnapshotGlobal),
                global_key,
                language.hotkeys_in_window(),
                language.hotkey_action(Action::ToggleCapture),
                "Space",
                language.hotkey_action(Action::Quit),
                language.hotkey_not_set(),
                &busy,
                language.hotkeys_reset_all(),
                language.mouse_heading(),
            ] {
                assert!(
                    texts.iter().any(|text| text == wanted),
                    "{language:?} lays out {wanted:?}: {texts:?}"
                );
            }
            // The main window keeps its changes, so it does not warn about them.
            assert!(
                !texts
                    .iter()
                    .any(|text| text == language.hotkeys_this_window_only())
            );

            // Another capture window has no system-wide shortcut and says its changes end
            // with it, first thing on the page.
            let (_, texts) = pass(&mut page, &mut hotkeys, language, None, None, false, true);
            assert_eq!(texts[0], language.hotkeys_this_window_only());
            for absent in [
                language.hotkeys_system_wide(),
                language.hotkey_action(Action::SaveSnapshotGlobal),
            ] {
                assert!(!texts.iter().any(|text| text == absent), "{absent}");
            }
            assert!(
                texts
                    .iter()
                    .any(|text| text == language.hotkeys_in_window())
            );
        }
        // A recording row and a refused change are drawn in either language.
        hotkeys.clear(Action::Clear);
        page.apply(
            Edit::Record(slot(Action::SeparateLanes, 0)),
            &mut hotkeys,
            None,
        );
        for language in [English, TraditionalChinese] {
            page.problem = Some((
                Action::SeparateLanes,
                Problem::Binding(BindingError::Conflict(Action::Quit)),
            ));
            let (changed, texts) = pass(
                &mut page,
                &mut hotkeys,
                language,
                None,
                Some("busy"),
                true,
                true,
            );
            assert!(!changed);
            let conflict = language.hotkey_problem(BindingError::Conflict(Action::Quit));
            for wanted in [
                language.hotkey_press_keys(),
                language.hotkey_recording_note(),
                language.hotkey_right_ctrl(),
                &conflict,
            ] {
                assert!(texts.iter().any(|text| text == wanted), "{wanted:?}");
            }
        }
        assert!(page.is_recording(), "drawing alone changes nothing");
        assert!(hotkeys.bindings(Action::Clear).is_empty());
    }

    #[test]
    fn only_the_edit_separately_row_offers_the_right_ctrl_tap() {
        let mut hotkeys = Hotkeys::defaults();
        for action in [
            Action::KeepOnTop,
            Action::CancelSelection,
            Action::SaveSnapshotGlobal,
        ] {
            let mut page = HotkeysPage::default();
            page.apply(Edit::Record(slot(action, 0)), &mut hotkeys, None);
            let (_, texts) = pass(&mut page, &mut hotkeys, English, None, None, true, true);
            assert!(page.is_recording());
            assert!(
                !texts.iter().any(|text| text == English.hotkey_right_ctrl()),
                "{action:?}"
            );
        }
        // And the model refuses the tap for any other action, should one be asked for.
        let mut page = HotkeysPage::default();
        page.apply(Edit::Record(slot(Action::KeepOnTop, 0)), &mut hotkeys, None);
        assert!(!page.apply(
            Edit::RightCtrl(slot(Action::KeepOnTop, 0)),
            &mut hotkeys,
            None
        ));
        assert_eq!(
            page.problem
                .as_ref()
                .map(|(at, problem)| (*at, problem.text(English))),
            Some((
                Action::KeepOnTop,
                "A tap of Right Ctrl works only for Edit separately.".into()
            ))
        );
        assert_eq!(hotkeys, Hotkeys::defaults());
    }

    #[test]
    fn moving_to_another_page_ends_recording_and_resumes_the_global_shortcut_once() {
        let global = FakeGlobal::default();
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        page.apply(
            Edit::Record(slot(Action::ToggleCapture, 0)),
            &mut hotkeys,
            Some(&global),
        );
        assert_eq!(global.take(), ["suspend"]);
        // While the page is drawn, its row keeps recording.
        for _ in 0..2 {
            pass(
                &mut page,
                &mut hotkeys,
                English,
                Some(&global),
                None,
                true,
                true,
            );
            assert!(page.is_recording());
        }
        assert!(global.take().is_empty());
        // A pass that draws another page ends it.
        for _ in 0..2 {
            pass(
                &mut page,
                &mut hotkeys,
                English,
                Some(&global),
                None,
                true,
                false,
            );
            assert!(!page.is_recording());
        }
        assert_eq!(global.take(), ["resume"]);
        assert_eq!(hotkeys, Hotkeys::defaults());
    }

    #[test]
    fn closing_or_hiding_the_options_window_ends_recording_and_resumes_once() {
        let global = FakeGlobal::default();
        let mut page = HotkeysPage::default();
        let mut hotkeys = Hotkeys::defaults();
        page.apply(
            Edit::Record(slot(Action::Quit, 0)),
            &mut hotkeys,
            Some(&global),
        );
        pass(
            &mut page,
            &mut hotkeys,
            English,
            Some(&global),
            None,
            true,
            true,
        );
        assert_eq!(global.take(), ["suspend"]);
        // A closed or hidden Options window draws nothing, and the pass still ends.
        page.end_pass(Some(&global));
        assert!(!page.is_recording());
        assert_eq!(global.take(), ["resume"]);
        page.end_pass(Some(&global));
        assert!(global.take().is_empty());
        // Opened again, the page starts idle and records only when asked.
        pass(
            &mut page,
            &mut hotkeys,
            English,
            Some(&global),
            None,
            true,
            true,
        );
        assert!(!page.is_recording());
        assert!(global.take().is_empty());
    }
}

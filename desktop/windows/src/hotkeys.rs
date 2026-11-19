//! Shortcut bindings shared by the recorder, Options and the global hotkey worker.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use windows::Win32::UI::Input::KeyboardAndMouse::{self as win, HOT_KEY_MODIFIERS};

pub(crate) const MAX_BINDINGS: usize = 2;
const FILE_VERSION: u32 = 1;
const LEFT_OF_1_SCAN_CODE: u32 = 0x29;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Global,
    Window,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Action {
    SaveSnapshotGlobal,
    ToggleCapture,
    Clear,
    CancelSelection,
    SeparateLanes,
    SaveSnapshot,
    WaveformOnly,
    KeepOnTop,
    OpenOptions,
    Quit,
}

impl Action {
    pub(crate) const ALL: [Self; 10] = [
        Self::SaveSnapshotGlobal,
        Self::ToggleCapture,
        Self::Clear,
        Self::CancelSelection,
        Self::SeparateLanes,
        Self::SaveSnapshot,
        Self::WaveformOnly,
        Self::KeepOnTop,
        Self::OpenOptions,
        Self::Quit,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::SaveSnapshotGlobal => "save-snapshot-global",
            Self::ToggleCapture => "toggle-capture",
            Self::Clear => "clear",
            Self::CancelSelection => "cancel-selection",
            Self::SeparateLanes => "separate-lanes",
            Self::SaveSnapshot => "save-snapshot",
            Self::WaveformOnly => "waveform-only",
            Self::KeepOnTop => "keep-on-top",
            Self::OpenOptions => "open-options",
            Self::Quit => "quit",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }

    pub(crate) fn scope(self) -> Scope {
        if self == Self::SaveSnapshotGlobal {
            Scope::Global
        } else {
            Scope::Window
        }
    }

    fn default_bindings(self) -> &'static [Binding] {
        const TOGGLE_CAPTURE: &[Binding] = &[bare(Key::Space)];
        const CLEAR: &[Binding] = &[bare(Key::Delete)];
        const CANCEL_SELECTION: &[Binding] = &[bare(Key::Escape), bare(Key::Backspace)];

        match self {
            Self::SaveSnapshotGlobal => &[Binding::Chord {
                modifiers: Modifiers::ALT,
                key: Key::KeyLeftOf1,
            }],
            Self::ToggleCapture => TOGGLE_CAPTURE,
            Self::Clear => CLEAR,
            Self::CancelSelection => CANCEL_SELECTION,
            Self::SeparateLanes => &[Binding::RightCtrlTap],
            Self::SaveSnapshot
            | Self::WaveformOnly
            | Self::KeepOnTop
            | Self::OpenOptions
            | Self::Quit => &[],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Modifiers {
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
    pub(crate) win: bool,
}

impl Modifiers {
    pub(crate) const NONE: Self = Self {
        ctrl: false,
        shift: false,
        alt: false,
        win: false,
    };
    const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };
    const CTRL_ALT: Self = Self {
        ctrl: true,
        alt: true,
        ..Self::NONE
    };
    const WIN: Self = Self {
        win: true,
        ..Self::NONE
    };

    fn is_empty(&self) -> bool {
        *self == Self::NONE
    }

    fn win32_flags(self) -> HOT_KEY_MODIFIERS {
        [
            (self.ctrl, win::MOD_CONTROL),
            (self.shift, win::MOD_SHIFT),
            (self.alt, win::MOD_ALT),
            (self.win, win::MOD_WIN),
        ]
        .into_iter()
        .filter_map(|(pressed, flag)| pressed.then_some(flag))
        .fold(win::MOD_NOREPEAT, |flags, flag| flags | flag)
    }
}

/// Letters, digits and function keys use their consecutive virtual-key codes.
/// The key left of 1 is identified by scan code instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[repr(u16)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Key {
    A = win::VK_A.0,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0 = win::VK_0.0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1 = win::VK_F1.0,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    Space = win::VK_SPACE.0,
    Enter = win::VK_RETURN.0,
    Tab = win::VK_TAB.0,
    Escape = win::VK_ESCAPE.0,
    Backspace = win::VK_BACK.0,
    Delete = win::VK_DELETE.0,
    Insert = win::VK_INSERT.0,
    Home = win::VK_HOME.0,
    End = win::VK_END.0,
    PageUp = win::VK_PRIOR.0,
    PageDown = win::VK_NEXT.0,
    ArrowUp = win::VK_UP.0,
    ArrowDown = win::VK_DOWN.0,
    ArrowLeft = win::VK_LEFT.0,
    ArrowRight = win::VK_RIGHT.0,
    Backtick = win::VK_OEM_3.0,
    Minus = win::VK_OEM_MINUS.0,
    Equals = win::VK_OEM_PLUS.0,
    OpenBracket = win::VK_OEM_4.0,
    CloseBracket = win::VK_OEM_6.0,
    Backslash = win::VK_OEM_5.0,
    Semicolon = win::VK_OEM_1.0,
    Quote = win::VK_OEM_7.0,
    Comma = win::VK_OEM_COMMA.0,
    Period = win::VK_OEM_PERIOD.0,
    Slash = win::VK_OEM_2.0,
    #[serde(rename = "key-left-of-1")]
    KeyLeftOf1 = 0x100,
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::Space => "Space",
            Self::Enter => "Enter",
            Self::Tab => "Tab",
            Self::Escape => "Escape",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Insert => "Insert",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "Page Up",
            Self::PageDown => "Page Down",
            Self::ArrowUp => "Up Arrow",
            Self::ArrowDown => "Down Arrow",
            Self::ArrowLeft => "Left Arrow",
            Self::ArrowRight => "Right Arrow",
            Self::Backtick => "`",
            Self::Minus => "-",
            Self::Equals => "=",
            Self::OpenBracket => "[",
            Self::CloseBracket => "]",
            Self::Backslash => "\\",
            Self::Semicolon => ";",
            Self::Quote => "'",
            Self::Comma => ",",
            Self::Period => ".",
            Self::Slash => "/",
            Self::KeyLeftOf1 => "key left of 1",
            key => {
                let code = key as u16;
                if (win::VK_F1.0..=win::VK_F24.0).contains(&code) {
                    return write!(f, "F{}", code - win::VK_F1.0 + 1);
                }
                // The remaining keys are the ASCII letters and digits.
                return write!(f, "{}", char::from(code as u8));
            }
        };
        f.write_str(name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum Binding {
    Chord {
        key: Key,
        #[serde(skip_serializing_if = "Modifiers::is_empty")]
        modifiers: Modifiers,
    },
    RightCtrlTap,
}

impl Binding {
    /// A capture that contains only modifiers has no key to bind.
    pub(crate) fn from_key(modifiers: Modifiers, key: Option<Key>) -> Result<Self, BindingError> {
        Ok(Self::Chord {
            key: key.ok_or(BindingError::KeyRequired)?,
            modifiers,
        })
    }

    fn check_reserved(self) -> Result<(), BindingError> {
        let Self::Chord { modifiers, key } = self else {
            return Ok(());
        };
        let shortcut = match (modifiers, key) {
            (Modifiers::ALT, Key::F4) => ReservedShortcut::AltF4,
            (Modifiers::CTRL_ALT, Key::Delete) => ReservedShortcut::CtrlAltDelete,
            (Modifiers::NONE, Key::Tab) => ReservedShortcut::BareTab,
            (Modifiers::NONE, Key::Enter) => ReservedShortcut::BareEnter,
            (Modifiers::WIN, Key::L) => ReservedShortcut::WinL,
            _ => return Ok(()),
        };
        Err(BindingError::Reserved(shortcut))
    }

    /// Validates a global binding and supplies the arguments for RegisterHotKey.
    /// Windows decides whether the chord is available in the current session.
    pub(crate) fn to_win32(self) -> Result<GlobalBinding, BindingError> {
        let Self::Chord { modifiers, key } = self else {
            return Err(BindingError::GlobalRightCtrlTap);
        };
        self.check_reserved()?;
        if modifiers.is_empty() {
            return Err(BindingError::GlobalNeedsModifier);
        }
        let key = match key {
            // RegisterHotKey reserves F12 for the debugger.
            Key::F12 => return Err(BindingError::UnsupportedGlobalKey(key)),
            Key::KeyLeftOf1 => GlobalKey::ScanCode(LEFT_OF_1_SCAN_CODE),
            _ => GlobalKey::VirtualKey(u32::from(key as u16)),
        };
        Ok(GlobalBinding {
            modifiers: modifiers.win32_flags(),
            key,
        })
    }
}

const fn bare(key: Key) -> Binding {
    Binding::Chord {
        modifiers: Modifiers::NONE,
        key,
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Chord { modifiers, key } = self else {
            return f.write_str("Right Ctrl");
        };
        for (pressed, label) in [
            (modifiers.ctrl, "Ctrl"),
            (modifiers.shift, "Shift"),
            (modifiers.alt, "Alt"),
            (modifiers.win, "Win"),
        ] {
            if pressed {
                write!(f, "{label} + ")?;
            }
        }
        write!(f, "{key}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReservedShortcut {
    AltF4,
    CtrlAltDelete,
    BareTab,
    BareEnter,
    WinL,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingError {
    TooManyBindings,
    KeyRequired,
    GlobalNeedsModifier,
    GlobalRightCtrlTap,
    UnsupportedGlobalKey(Key),
    Reserved(ReservedShortcut),
    Conflict(Action),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlobalKey {
    VirtualKey(u32),
    ScanCode(u32),
}

/// A chord that can be registered with Windows after resolving any scan code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GlobalBinding {
    modifiers: HOT_KEY_MODIFIERS,
    key: GlobalKey,
}

impl GlobalBinding {
    pub(crate) fn modifiers(self) -> HOT_KEY_MODIFIERS {
        self.modifiers
    }

    pub(crate) fn key(self) -> GlobalKey {
        self.key
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hotkeys {
    bindings: BTreeMap<Action, Vec<Binding>>,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self::defaults()
    }
}

impl Hotkeys {
    pub(crate) fn defaults() -> Self {
        let bindings = Action::ALL
            .into_iter()
            .filter_map(|action| {
                let bindings = action.default_bindings();
                (!bindings.is_empty()).then(|| (action, bindings.to_vec()))
            })
            .collect();
        Self { bindings }
    }

    pub(crate) fn bindings(&self, action: Action) -> &[Binding] {
        self.bindings.get(&action).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Replaces the action's bindings together. A rejected edit leaves the model intact.
    pub(crate) fn set(&mut self, action: Action, bindings: &[Binding]) -> Result<(), BindingError> {
        if bindings.len() > MAX_BINDINGS {
            return Err(BindingError::TooManyBindings);
        }
        for &binding in bindings {
            self.check_binding(action, binding)?;
        }
        if bindings.is_empty() {
            self.clear(action);
        } else {
            self.bindings.insert(action, bindings.to_vec());
        }
        Ok(())
    }

    pub(crate) fn clear(&mut self, action: Action) {
        self.bindings.remove(&action);
    }

    /// Restoring one action can conflict with another action that uses its default.
    #[allow(dead_code)] // used by the Hotkeys page
    pub(crate) fn reset(&mut self, action: Action) -> Result<(), BindingError> {
        self.set(action, action.default_bindings())
    }

    #[allow(dead_code)] // used by the Hotkeys page
    pub(crate) fn reset_all(&mut self) {
        *self = Self::defaults();
    }

    /// Checks scope, reservations and conflicts against the other actions.
    pub(crate) fn check_binding(
        &self,
        action: Action,
        binding: Binding,
    ) -> Result<(), BindingError> {
        match action.scope() {
            Scope::Global => {
                binding.to_win32()?;
            }
            Scope::Window => binding.check_reserved()?,
        }
        if let Some((&holder, _)) = self
            .bindings
            .iter()
            .find(|(other, bindings)| **other != action && bindings.contains(&binding))
        {
            return Err(BindingError::Conflict(holder));
        }
        Ok(())
    }

    #[allow(dead_code)] // used by the Hotkeys page
    pub(crate) fn find(&self, scope: Scope, modifiers: Modifiers, key: Key) -> Option<Action> {
        self.find_binding(scope, Binding::Chord { key, modifiers })
    }

    /// Also accepts RightCtrlTap from the window's tap detector.
    pub(crate) fn find_binding(&self, scope: Scope, binding: Binding) -> Option<Action> {
        self.bindings.iter().find_map(|(&action, bindings)| {
            (action.scope() == scope && bindings.contains(&binding)).then_some(action)
        })
    }

    pub(crate) fn global_bindings(&self) -> [Option<GlobalBinding>; MAX_BINDINGS] {
        let bindings = self.bindings(Action::SaveSnapshotGlobal);
        std::array::from_fn(|slot| {
            bindings.get(slot).map(|binding| {
                binding
                    .to_win32()
                    .expect("stored global bindings are validated")
            })
        })
    }

    #[allow(dead_code)] // used by the Hotkeys page
    pub(crate) fn to_json(&self) -> String {
        let bindings: BTreeMap<_, _> = Action::ALL
            .into_iter()
            .map(|action| (action.id(), self.bindings(action)))
            .collect();
        serde_json::to_string(&StoredHotkeys {
            version: FILE_VERSION,
            bindings,
        })
        .expect("hotkeys can be encoded as JSON")
    }

    pub(crate) fn from_json(text: &str) -> Option<Self> {
        let stored: StoredHotkeys<BTreeMap<String, Value>> = serde_json::from_str(text).ok()?;
        if stored.version != FILE_VERSION {
            return None;
        }
        let mut overrides: BTreeMap<_, _> = stored
            .bindings
            .into_iter()
            .filter_map(|(id, raw)| Some((Action::from_id(&id)?, decode_bindings(raw)?)))
            .collect();

        // Release the defaults of all candidate overrides first so a saved swap can load.
        // Check overrides in action order when deciding which one owns a conflicting chord.
        // Restart after restoring a default, since it may take a shortcut from another override.
        loop {
            let mut hotkeys = Self::defaults();
            for &action in overrides.keys() {
                hotkeys.clear(action);
            }
            let rejected = overrides.iter().find_map(|(&action, bindings)| {
                hotkeys.set(action, bindings).err().map(|_| action)
            });
            let Some(action) = rejected else {
                return Some(hotkeys);
            };
            overrides.remove(&action);
        }
    }
}

#[derive(Deserialize, Serialize)]
struct StoredHotkeys<B> {
    version: u32,
    bindings: B,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum StoredBinding {
    Chord {
        key: String,
        #[serde(default)]
        modifiers: Modifiers,
    },
    RightCtrlTap,
}

fn decode_bindings(raw: Value) -> Option<Vec<Binding>> {
    let stored: Vec<StoredBinding> = serde_json::from_value(raw).ok()?;
    if stored.is_empty() {
        return Some(Vec::new());
    }
    let bindings: Vec<_> = stored
        .into_iter()
        .filter_map(|binding| match binding {
            StoredBinding::Chord { key, modifiers } => {
                let key = serde_json::from_value(Value::String(key)).ok();
                Binding::from_key(modifiers, key).ok()
            }
            StoredBinding::RightCtrlTap => Some(Binding::RightCtrlTap),
        })
        .collect();
    // Only an explicit empty array clears the action.
    (!bindings.is_empty()).then_some(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, VK_F24, VK_OEM_PLUS, VK_S,
    };

    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    const ALT: Modifiers = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };

    fn bare(key: Key) -> Binding {
        Binding::Chord {
            modifiers: Modifiers::NONE,
            key,
        }
    }

    fn chord(modifiers: Modifiers, key: Key) -> Binding {
        Binding::Chord { modifiers, key }
    }

    #[test]
    fn defaults_match_the_existing_shortcuts() {
        let model = Hotkeys::defaults();
        let expected: &[(Action, &[Binding])] = &[
            (Action::SaveSnapshotGlobal, &[chord(ALT, Key::KeyLeftOf1)]),
            (Action::ToggleCapture, &[bare(Key::Space)]),
            (Action::Clear, &[bare(Key::Delete)]),
            (
                Action::CancelSelection,
                &[bare(Key::Escape), bare(Key::Backspace)],
            ),
            (Action::SeparateLanes, &[Binding::RightCtrlTap]),
            (Action::SaveSnapshot, &[]),
            (Action::WaveformOnly, &[]),
            (Action::KeepOnTop, &[]),
            (Action::OpenOptions, &[]),
            (Action::Quit, &[]),
        ];
        assert_eq!(
            Action::ALL,
            expected
                .iter()
                .map(|(action, _)| *action)
                .collect::<Vec<_>>()[..]
        );
        for &(action, bindings) in expected {
            assert_eq!(model.bindings(action), bindings, "{action:?}");
        }
        assert_eq!(Hotkeys::default(), model);
    }

    #[test]
    fn dispatch_matches_the_full_chord_and_scope() {
        let mut model = Hotkeys::defaults();
        model
            .set(Action::OpenOptions, &[chord(CTRL, Key::O)])
            .unwrap();
        assert_eq!(
            model.find(Scope::Window, CTRL, Key::O),
            Some(Action::OpenOptions)
        );
        assert_eq!(model.find(Scope::Window, Modifiers::NONE, Key::O), None);
        assert_eq!(model.find(Scope::Global, CTRL, Key::O), None);
        assert_eq!(
            model.find(Scope::Global, ALT, Key::KeyLeftOf1),
            Some(Action::SaveSnapshotGlobal)
        );
        assert_eq!(
            model.find(Scope::Window, Modifiers::NONE, Key::Backspace),
            Some(Action::CancelSelection)
        );
        assert_eq!(
            model.find_binding(Scope::Window, Binding::RightCtrlTap),
            Some(Action::SeparateLanes)
        );
        assert_eq!(
            model.find_binding(Scope::Global, Binding::RightCtrlTap),
            None
        );
    }

    #[test]
    fn global_bindings_require_a_modified_key_that_windows_can_register() {
        let mut model = Hotkeys::defaults();
        let action = Action::SaveSnapshotGlobal;
        for (binding, error) in [
            (bare(Key::Q), BindingError::GlobalNeedsModifier),
            (Binding::RightCtrlTap, BindingError::GlobalRightCtrlTap),
            (
                chord(CTRL, Key::F12),
                BindingError::UnsupportedGlobalKey(Key::F12),
            ),
        ] {
            assert_eq!(model.check_binding(action, binding), Err(error));
            assert_eq!(model.set(action, &[binding]), Err(error));
        }
        assert_eq!(model.bindings(action), &[chord(ALT, Key::KeyLeftOf1)]);
        for modifiers in [
            CTRL,
            ALT,
            Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
            Modifiers {
                win: true,
                ..Modifiers::NONE
            },
        ] {
            assert_eq!(model.set(action, &[chord(modifiers, Key::Q)]), Ok(()));
        }
        assert_eq!(model.set(Action::OpenOptions, &[bare(Key::F12)]), Ok(()));
    }

    #[test]
    fn a_modifier_only_capture_requires_a_key() {
        assert_eq!(
            Binding::from_key(CTRL, None),
            Err(BindingError::KeyRequired)
        );
        assert_eq!(
            Binding::from_key(CTRL, Some(Key::S)),
            Ok(chord(CTRL, Key::S))
        );
    }

    #[test]
    fn reserved_shortcuts_are_reported_in_either_scope() {
        let mut model = Hotkeys::defaults();
        for (binding, reserved) in [
            (chord(ALT, Key::F4), ReservedShortcut::AltF4),
            (
                chord(Modifiers { alt: true, ..CTRL }, Key::Delete),
                ReservedShortcut::CtrlAltDelete,
            ),
            (bare(Key::Tab), ReservedShortcut::BareTab),
            (bare(Key::Enter), ReservedShortcut::BareEnter),
            (
                chord(
                    Modifiers {
                        win: true,
                        ..Modifiers::NONE
                    },
                    Key::L,
                ),
                ReservedShortcut::WinL,
            ),
        ] {
            for action in [Action::Quit, Action::SaveSnapshotGlobal] {
                assert_eq!(
                    model.set(action, &[binding]),
                    Err(BindingError::Reserved(reserved))
                );
            }
        }
        assert_eq!(
            model.set(
                Action::Quit,
                &[chord(CTRL, Key::Enter), chord(ALT, Key::Tab)]
            ),
            Ok(())
        );
    }

    #[test]
    fn conflicting_edits_name_the_existing_action_regardless_of_scope() {
        let mut model = Hotkeys::defaults();
        let binding = chord(CTRL, Key::S);
        model.set(Action::SaveSnapshot, &[binding]).unwrap();
        for action in [Action::OpenOptions, Action::SaveSnapshotGlobal] {
            assert_eq!(
                model.check_binding(action, binding),
                Err(BindingError::Conflict(Action::SaveSnapshot))
            );
            assert_eq!(
                model.set(action, &[binding]),
                Err(BindingError::Conflict(Action::SaveSnapshot))
            );
        }
        model.clear(Action::SaveSnapshot);
        model.set(Action::SaveSnapshotGlobal, &[binding]).unwrap();
        assert_eq!(
            model.set(Action::SaveSnapshot, &[binding]),
            Err(BindingError::Conflict(Action::SaveSnapshotGlobal))
        );
        assert_eq!(
            model.set(Action::Quit, &[Binding::RightCtrlTap]),
            Err(BindingError::Conflict(Action::SeparateLanes))
        );
        assert_eq!(model.set(Action::SaveSnapshotGlobal, &[binding]), Ok(()));
    }

    #[test]
    fn replacement_is_atomic_and_has_two_slots() {
        let mut model = Hotkeys::defaults();
        for action in [Action::SaveSnapshotGlobal, Action::OpenOptions] {
            let original = model.bindings(action).to_vec();
            assert_eq!(
                model.set(
                    action,
                    &[
                        chord(CTRL, Key::S),
                        chord(CTRL, Key::O),
                        chord(CTRL, Key::Q)
                    ]
                ),
                Err(BindingError::TooManyBindings)
            );
            assert_eq!(model.bindings(action), original);
        }
        assert_eq!(
            model.set(
                Action::OpenOptions,
                &[chord(CTRL, Key::O), bare(Key::Delete)]
            ),
            Err(BindingError::Conflict(Action::Clear))
        );
        assert!(model.bindings(Action::OpenOptions).is_empty());
        assert_eq!(
            model.set(Action::OpenOptions, &[chord(CTRL, Key::O), bare(Key::F6)]),
            Ok(())
        );
        assert_eq!(
            model.bindings(Action::OpenOptions),
            &[chord(CTRL, Key::O), bare(Key::F6)]
        );
    }

    #[test]
    fn reset_respects_a_reassigned_default_and_reset_all_restores_it() {
        let mut model = Hotkeys::defaults();
        model.set(Action::ToggleCapture, &[bare(Key::F6)]).unwrap();
        model.set(Action::OpenOptions, &[bare(Key::Space)]).unwrap();
        assert_eq!(
            model.reset(Action::ToggleCapture),
            Err(BindingError::Conflict(Action::OpenOptions))
        );
        assert_eq!(model.bindings(Action::ToggleCapture), &[bare(Key::F6)]);
        model.clear(Action::OpenOptions);
        model.reset(Action::ToggleCapture).unwrap();
        assert_eq!(model.bindings(Action::ToggleCapture), &[bare(Key::Space)]);
        model.clear(Action::Clear);
        model.reset_all();
        assert_eq!(model, Hotkeys::defaults());
    }

    #[test]
    fn json_round_trip_preserves_reassigned_defaults_and_empty_bindings() {
        let mut model = Hotkeys::defaults();
        model.clear(Action::Clear);
        model
            .set(Action::ToggleCapture, &[bare(Key::Delete)])
            .unwrap();
        model.set(Action::Clear, &[bare(Key::Space)]).unwrap();
        model.clear(Action::SaveSnapshotGlobal);
        model
            .set(Action::OpenOptions, &[chord(CTRL, Key::O), bare(Key::F24)])
            .unwrap();
        let json = model.to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(
            value["bindings"]["save-snapshot-global"],
            serde_json::json!([])
        );
        assert_eq!(
            value["bindings"]["clear"],
            serde_json::json!([{"type": "chord", "key": "space"}])
        );
        assert_eq!(
            value["bindings"]["separate-lanes"],
            serde_json::json!([{"type": "right-ctrl-tap"}])
        );
        assert_eq!(Hotkeys::from_json(&json), Some(model));
    }

    #[test]
    fn restored_defaults_take_priority_over_conflicting_saved_bindings() {
        let document = r#"{"version":1,"bindings":{
            "save-snapshot-global":[{"type":"chord","key":"s","modifiers":{"ctrl":true}}],
            "toggle-capture":[{"type":"chord","key":"s","modifiers":{"ctrl":true}}],
            "clear":[{"type":"chord","key":"space"}],
            "open-options":[{"type":"chord","key":"f6"}]
        }}"#;
        let model = Hotkeys::from_json(document).unwrap();
        assert_eq!(
            model.bindings(Action::SaveSnapshotGlobal),
            &[chord(CTRL, Key::S)]
        );
        assert_eq!(model.bindings(Action::ToggleCapture), &[bare(Key::Space)]);
        assert_eq!(model.bindings(Action::Clear), &[bare(Key::Delete)]);
        assert_eq!(model.bindings(Action::OpenOptions), &[bare(Key::F6)]);
    }

    #[test]
    fn loading_a_modifier_without_a_key_restores_the_action_default() {
        let document = r#"{"version":1,"bindings":{
            "save-snapshot-global":[{"type":"chord","modifiers":{"alt":true}}],
            "cancel-selection":[{"type":"chord","key":"f6"}]
        }}"#;
        let model = Hotkeys::from_json(document).unwrap();
        assert_eq!(
            model.bindings(Action::SaveSnapshotGlobal),
            &[chord(ALT, Key::KeyLeftOf1)]
        );
        assert_eq!(model.bindings(Action::CancelSelection), &[bare(Key::F6)]);
    }

    #[test]
    fn display_uses_english_modifiers_key_names_and_symbols() {
        for (binding, expected) in [
            (chord(ALT, Key::KeyLeftOf1), "Alt + key left of 1"),
            (
                chord(
                    Modifiers {
                        shift: true,
                        ..CTRL
                    },
                    Key::S,
                ),
                "Ctrl + Shift + S",
            ),
            (
                chord(Modifiers { win: true, ..ALT }, Key::PageDown),
                "Alt + Win + Page Down",
            ),
            (Binding::RightCtrlTap, "Right Ctrl"),
            (bare(Key::Space), "Space"),
            (bare(Key::Digit9), "9"),
            (bare(Key::F24), "F24"),
            (bare(Key::Semicolon), ";"),
        ] {
            assert_eq!(binding.to_string(), expected);
        }
    }

    #[test]
    fn win32_conversion_preserves_flags_and_the_kind_of_key() {
        let model = Hotkeys::defaults();
        let [Some(default), None] = model.global_bindings() else {
            panic!("the default has one global chord");
        };
        assert_eq!(default.modifiers(), MOD_ALT | MOD_NOREPEAT);
        assert_eq!(default.key(), GlobalKey::ScanCode(0x29));

        let all = Modifiers {
            ctrl: true,
            shift: true,
            alt: true,
            win: true,
        };
        let binding = chord(all, Key::S).to_win32().unwrap();
        assert_eq!(
            binding.modifiers(),
            MOD_CONTROL | MOD_SHIFT | MOD_ALT | MOD_WIN | MOD_NOREPEAT
        );
        assert_eq!(binding.key(), GlobalKey::VirtualKey(u32::from(VK_S.0)));
        assert_eq!(
            chord(CTRL, Key::F24).to_win32().unwrap().key(),
            GlobalKey::VirtualKey(u32::from(VK_F24.0))
        );
        assert_eq!(
            chord(CTRL, Key::Equals).to_win32().unwrap().key(),
            GlobalKey::VirtualKey(u32::from(VK_OEM_PLUS.0))
        );
        assert_eq!(
            Binding::RightCtrlTap.to_win32(),
            Err(BindingError::GlobalRightCtrlTap)
        );
    }
}

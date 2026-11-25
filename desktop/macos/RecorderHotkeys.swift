import AppKit
import Carbon

enum RecorderAction: String, Codable, CaseIterable {
    case globalSnapshot, toggleCapture, clear, cancelSelection, separateLanes
    case saveSnapshot, waveformOnly, keepOnTop, openOptions, quit

    var title: String {
        switch self {
        case .globalSnapshot: "Save snapshot from anywhere"
        case .toggleCapture: "Pause or resume capture"
        case .clear: "Clear capture"
        case .cancelSelection: "Cancel selection"
        case .separateLanes: "Edit separately"
        case .saveSnapshot: "Save snapshot"
        case .waveformOnly: "Show waveform only"
        case .keepOnTop: "Keep on top"
        case .openOptions: "Open Options"
        case .quit: "Quit"
        }
    }

    var presets: [RecorderShortcut] {
        switch self {
        case .globalSnapshot: [.init(code: RecorderShortcut.leftOfOne, modifiers: [.function], key: "Key left of 1")]
        case .toggleCapture: [.init(code: UInt16(kVK_Space), key: "Space")]
        case .clear: [.init(code: UInt16(kVK_ForwardDelete), key: "Forward Delete")]
        case .cancelSelection: [.init(code: UInt16(kVK_Escape), key: "Esc"), .init(code: UInt16(kVK_Delete), key: "Backspace")]
        case .separateLanes: [.rightCommand]
        default: []
        }
    }
}

struct RecorderShortcut: Equatable, Codable {
    static let leftOfOne = UInt16.max
    static let rightCommand = Self(code: UInt16(kVK_RightCommand), key: "Right ⌘")

    let code: UInt16
    private let flags: UInt
    let key: String

    var modifiers: NSEvent.ModifierFlags { NSEvent.ModifierFlags(rawValue: flags) }
    private static let allowed: NSEvent.ModifierFlags = [.command, .option, .control, .shift, .function]

    init(code: UInt16, modifiers: NSEvent.ModifierFlags = [], key: String) {
        self.code = code
        var filtered = modifiers.intersection(Self.allowed)
        // On compact keyboards the Fn key produces the standard Forward Delete event.
        if code == UInt16(kVK_ForwardDelete) { filtered.remove(.function) }
        flags = filtered.rawValue
        self.key = key
    }

    init(event: NSEvent) {
        let normalized = Self.normalizedCode(event.keyCode, type: Int16(LMGetKbdType()))
        let label: String
        switch normalized {
        case Self.leftOfOne: label = "Key left of 1"
        case UInt16(kVK_Space): label = "Space"
        case UInt16(kVK_Delete): label = "Backspace"
        case UInt16(kVK_ForwardDelete): label = "Forward Delete"
        case UInt16(kVK_Escape): label = "Esc"
        case UInt16(kVK_Tab): label = "Tab"
        case UInt16(kVK_Return), UInt16(kVK_ANSI_KeypadEnter): label = "Return"
        case UInt16(kVK_LeftArrow): label = "←"
        case UInt16(kVK_RightArrow): label = "→"
        case UInt16(kVK_UpArrow): label = "↑"
        case UInt16(kVK_DownArrow): label = "↓"
        default:
            let text = event.charactersIgnoringModifiers ?? ""
            if let scalar = text.unicodeScalars.first, (0xF704...0xF717).contains(scalar.value) {
                label = "F\(scalar.value - 0xF703)"
            } else {
                label = text.isEmpty || text.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
                    ? "Key \(event.keyCode)" : text.uppercased()
            }
        }
        self.init(code: normalized, modifiers: event.modifierFlags, key: label)
    }

    static func normalizedCode(_ code: UInt16, type: Int16) -> UInt16 {
        let grave = KBGetLayoutType(type) == UInt32(kKeyboardISO) ? kVK_ISO_Section : kVK_ANSI_Grave
        return code == UInt16(grave) ? leftOfOne : code
    }

    func matches(code: UInt16, modifiers: NSEvent.ModifierFlags) -> Bool {
        self == Self(code: code, modifiers: modifiers, key: "")
    }

    static func == (lhs: Self, rhs: Self) -> Bool { lhs.code == rhs.code && lhs.flags == rhs.flags }

    var title: String {
        let names: [(NSEvent.ModifierFlags, String)] = [
            (.function, "Fn/Globe"), (.control, "⌃"), (.option, "⌥"), (.shift, "⇧"), (.command, "⌘")
        ]
        return (names.compactMap { modifiers.contains($0.0) ? $0.1 : nil } + [key]).joined(separator: " + ")
    }

    func error(for action: RecorderAction) -> String? {
        if code == Self.rightCommand.code {
            return action == .separateLanes && self == Self.rightCommand ? nil : "Right Command is reserved for Edit separately."
        }
        if code == UInt16(kVK_Escape) {
            return action == .cancelSelection && modifiers.isEmpty ? nil : "Escape is reserved for cancelling."
        }
        if code == UInt16(kVK_Tab) { return "Tab moves keyboard focus." }
        if modifiers.contains(.command), [
            kVK_ANSI_Q, kVK_ANSI_W, kVK_ANSI_X, kVK_ANSI_C, kVK_ANSI_V, kVK_ANSI_A, kVK_ANSI_Z
        ].contains(Int(code)) {
            return "macOS uses this shortcut."
        }
        if action == .globalSnapshot, modifiers.intersection([.function, .control, .option, .command]).isEmpty {
            return "A system-wide shortcut needs Fn/Globe, Control, Option or Command."
        }
        if flags & ~Self.allowed.rawValue != 0 || key.isEmpty || key.count > 40
            || (code > 127 && code != Self.leftOfOne) {
            return "This key combination is unavailable."
        }
        return nil
    }
}

struct RecorderHotkeys {
    private(set) var bindings: [RecorderAction: [RecorderShortcut]]

    init(data: Data? = nil) {
        bindings = Dictionary(uniqueKeysWithValues: RecorderAction.allCases.map { ($0, $0.presets) })
        if let data, let saved = try? JSONDecoder().decode([String: [RecorderShortcut]].self, from: data) {
            var restored = Self()
            restored.bindings = [:]
            for action in RecorderAction.allCases {
                guard restored.set(saved[action.rawValue] ?? action.presets, for: action) == nil else { return }
            }
            self = restored
        }
    }

    subscript(action: RecorderAction) -> [RecorderShortcut] { bindings[action] ?? [] }

    var data: Data? {
        try? JSONEncoder().encode(Dictionary(uniqueKeysWithValues: bindings.map { ($0.key.rawValue, $0.value) }))
    }

    func windowAction(for shortcut: RecorderShortcut) -> RecorderAction? {
        RecorderAction.allCases.first { $0 != .globalSnapshot && self[$0].contains(shortcut) }
    }

    @discardableResult
    mutating func set(_ shortcuts: [RecorderShortcut], for action: RecorderAction) -> String? {
        guard shortcuts.count <= 2 else { return "Each action can have up to two shortcuts." }
        if shortcuts.count == 2, shortcuts[0] == shortcuts[1] { return "This action already uses that shortcut." }
        for shortcut in shortcuts {
            if let error = shortcut.error(for: action) { return error }
            if let other = RecorderAction.allCases.first(where: { $0 != action && self[$0].contains(shortcut) }) {
                return "This shortcut is already used by \(other.title)."
            }
        }
        bindings[action] = shortcuts
        return nil
    }
}

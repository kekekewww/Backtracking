import AppKit
import AVFoundation
import Carbon
import CoreGraphics
import ServiceManagement
import UniformTypeIdentifiers

private let timeChoices: [UInt32] = [10, 20, 30, 60, 180, 300, 600]
private let autoResumeChoices: [UInt32] = [0, 5, 10]
private let autoResumePreferenceKey = "autoResumeSeconds"
private let transparencyChoices: [UInt32] = [0, 25, 50, 75, 100]
private let exportSampleRates: [UInt32] = [44_100, 48_000, 96_000]
private let exportBitDepths: [UInt32] = [16, 24, 32]
private let frameAutosaveName: NSWindow.FrameAutosaveName = "Backtracking.Panel"
private let receiverWindowArgument = "--new-capture-window"
private let fullPanelMinimum = NSSize(width: 510, height: 132)
private let fullPanelDefault = NSSize(width: 610, height: 144)
private let waveformOnlyPanelMinimum = NSSize(width: 240, height: 48)
private let waveformOnlyPanelDefault = NSSize(width: 610, height: 59)
private let panelVerticalInset: CGFloat = 8
private let statusStripHeight: CGFloat = 26
private let waveformFrameInset: CGFloat = 1
private let secondsLineChoices: [UInt32] = [1, 5, 10]
private let borderWidthChoices: [UInt32] = [1, 2, 3, 4]
// Days before an old drag-and-drop file is deleted. 0 is Off, as in the Windows app.
private let cleanupAgeChoices: [UInt32] = [0, 10, 20, 30, 60, 90]
private let optionLabelWidth: CGFloat = 232
private let problemColor = WaveformTheme.statusUnavailable

/// The default toggle for editing sources separately.
private let rightCommandKeyCode: UInt16 = 54
/// The device-dependent bits AppKit keeps in `modifierFlags` for each Command key (IOLLEvent.h).
private let leftCommandDeviceMask: UInt = 0x08
private let rightCommandDeviceMask: UInt = 0x10
/// A key held longer than this was not tapped.
private let tapLimit: TimeInterval = 0.5
/// As long as a source's name in a file name may be, as in the core.
private let sourceNameLimit = 48
private let sourceFileNameMaxEntries = 64
private let sourceFileNamesKey = "sourceFileNames"
private let sourceFileNameOrderKey = "sourceFileNameOrder"
private let fullWaveformMinimumHeight: CGFloat = 46
private let defaultAccentRGB: UInt32 = WaveformTheme.defaultAccentRGB

private func accentColor(_ rgb: UInt32) -> NSColor {
    NSColor(
        srgbRed: CGFloat((rgb >> 16) & 0xFF) / 255,
        green: CGFloat((rgb >> 8) & 0xFF) / 255,
        blue: CGFloat(rgb & 0xFF) / 255,
        alpha: 1
    )
}

private func movesFromWaveform(_ modifiers: NSEvent.ModifierFlags) -> Bool {
    modifiers.contains([.command, .option]) && modifiers.intersection([.control, .shift]).isEmpty
}

private func selectsWaveform(_ modifiers: NSEvent.ModifierFlags) -> Bool {
    !modifiers.intersection([.option, .control, .shift]).isEmpty
}

#if BACKTRACKING_CAPTURE_VERIFY
@MainActor
func verifyMacOptions() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyOptions()
}

@MainActor
func verifyMacFlagDragKeepsRows() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyFlagDragKeepsRows()
}

@MainActor
func verifyMacChipToolTips() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyChipToolTipsAndTransportLimits()
}

@MainActor
func verifyMacClipExportedAtExportRate() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyClipExportedAtExportRate()
}

@MainActor
func verifyMacCrossSourceEdgeScroll() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyCrossSourceEdgeScroll()
}

@MainActor
func verifyMacChannelLimit() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyChannelLimit()
}

@MainActor
func verifyMacLookGradient() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyLookGradient()
}

@MainActor
func verifyMacControlPauseHolds() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyControlPauseHolds()
}

@MainActor
func verifyMacLookUpdatesKeepControls() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyLookUpdatesKeepControls()
}

@MainActor
func verifyMacFrequencyRows() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyFrequencyRowsFollowIdentity()
}

@MainActor
func verifyMacAddonSettingControls() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyAddonSettingControls()
}

@MainActor
func verifyMacSelectWaitsForFreeze() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifySelectWaitsForFreeze()
}

@MainActor
func verifyMacAddonCaptureStatus() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyAddonCaptureStatus()
}

@MainActor
func verifyMacChannelLimitCaptureOpens() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyChannelLimitCaptureOpens()
}

@MainActor
func verifyMacSavedFileLanes() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifySavedFileLanes()
}

@MainActor
func verifyMacRowsAcrossGroups() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyRowsAcrossGroupsAndActivity()
}

@MainActor
func verifyMacSnapshotRebuilds() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifySnapshotRebuilds()
}

@MainActor
func verifyMacTestCopyIsolation() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyTestCopyIsolation()
}

@MainActor
func verifyMacControlBatchSnapshotSync() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyControlBatchSnapshotSync()
}

@MainActor
func verifyMacTerminationSaves() throws {
    _ = NSApplication.shared
    try BacktrackingController.verifyTerminationSaves()
}
#else
@main
enum BacktrackingApp {
    @MainActor
    static func main() {
        let config = TestCopyConfig.current
        let primary = !CommandLine.arguments.contains(receiverWindowArgument)
        // An invalid name is refused below with its own message.
        if TestCopyConfig.refusesUnisolatedLaunch(arguments: CommandLine.arguments,
                                                  isTestCopy: config.isTestCopy || config.invalidInstanceName) {
            fputs("Backtracking test copy could not start: this window was opened by a test copy, but \(TestCopyConfig.instanceVariable) did not reach it.\n", stderr)
            exit(2)
        }
        if config.isTestCopy, let refusal = TestCopyConfig.productionBundleRefusal(bundleID: Bundle.main.bundleIdentifier) {
            fputs("Backtracking test copy could not start: \(refusal)\n", stderr)
            exit(2)
        }
        if config.isTestCopy || config.invalidInstanceName {
            guard config.validateAndAcquireLock(primary: primary) else { return }
            TestCopyConfig.turnOffStateRestoration(UserDefaults.standard)
        } else {
            if StartupUpdater.runHelperIfRequested() { return }
            if StartupUpdater.otherLaunchDuringUpdate() { return }
        }
        let app = NSApplication.shared
        let delegate = BacktrackingController()
        app.delegate = delegate
        app.setActivationPolicy(.accessory)
        app.run()
        withExtendedLifetime(delegate) {}
    }
}
#endif

private final class SnapshotShortcut {
    private var port: CFMachPort?
    private var source: CFRunLoopSource?
    private let action: () -> Void
    var bindings = RecorderAction.globalSnapshot.presets
    var recording = false

    init(action: @escaping () -> Void) { self.action = action }

    var isListening: Bool { port.map { CGEvent.tapIsEnabled(tap: $0) } ?? false }

    func start() {
        guard !bindings.isEmpty, !isListening, CGPreflightListenEventAccess() else { return }
        stop()
        let keyDown = CGEventMask(1) << CGEventType.keyDown.rawValue
        guard let port = CGEvent.tapCreate(
            tap: .cgSessionEventTap, place: .headInsertEventTap, options: .listenOnly,
            eventsOfInterest: keyDown, callback: { _, type, event, userInfo in
                if let userInfo {
                    Unmanaged<SnapshotShortcut>.fromOpaque(userInfo).takeUnretainedValue().receive(type, event)
                }
                return Unmanaged.passUnretained(event)
            }, userInfo: Unmanaged.passUnretained(self).toOpaque()
        ), let source = CFMachPortCreateRunLoopSource(nil, port, 0) else { return }
        self.port = port
        self.source = source
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: port, enable: true)
    }

    func stop() {
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        if let port { CFMachPortInvalidate(port) }
        source = nil
        port = nil
    }

    private func receive(_ type: CGEventType, _ event: CGEvent) {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            if let port { CGEvent.tapEnable(tap: port, enable: true) }
            return
        }
        guard !recording, type == .keyDown,
              event.getIntegerValueField(.keyboardEventAutorepeat) == 0 else { return }
        let keyboardType = Int16(truncatingIfNeeded: event.getIntegerValueField(.keyboardEventKeyboardType))
        let code = UInt16(truncatingIfNeeded: event.getIntegerValueField(.keyboardEventKeycode))
        let modifiers = NSEvent.ModifierFlags(rawValue: UInt(event.flags.rawValue))
        guard bindings.contains(where: { $0.matches(code: RecorderShortcut.normalizedCode(code, type: keyboardType),
            modifiers: modifiers) }) else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.recording, self.bindings.contains(where: {
                $0.matches(code: RecorderShortcut.normalizedCode(code, type: keyboardType), modifiers: modifiers)
            }) else { return }
            self.action()
        }
    }

    deinit { stop() }
}

private class KeyPanel: NSPanel {
    override var canBecomeKey: Bool { true }

    /// Every window of the app is a KeyPanel; a test copy keeps none of them for restoring.
    override var isRestorable: Bool {
        get { Self.restorable(testCopy: TestCopyConfig.current.isTestCopy, requested: super.isRestorable) }
        set { super.isRestorable = newValue }
    }

    static func restorable(testCopy: Bool, requested: Bool) -> Bool { testCopy ? false : requested }
}

private final class HotkeyOptionsPanel: KeyPanel {
    var recordKey: ((NSEvent) -> Bool)?
    var cancelRecording: (() -> Void)?
    var canCloseWithEscape: (() -> Bool)?

    override func sendEvent(_ event: NSEvent) {
        if event.type == .keyDown {
            if recordKey?(event) == true { return }
            if event.keyCode == UInt16(kVK_Escape), !event.isARepeat {
                if canCloseWithEscape?() == true {
                    performClose(nil)
                }
                return
            }
        }
        super.sendEvent(event)
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.type == .keyDown {
            if recordKey?(event) == true { return true }
            if event.keyCode == UInt16(kVK_Escape), !event.isARepeat {
                if canCloseWithEscape?() == true {
                    performClose(nil)
                    return true
                }
                return false
            }
        }
        return super.performKeyEquivalent(with: event)
    }

    override func resignKey() {
        super.resignKey()
        cancelRecording?()
    }
}

private final class OptionsDocumentView: NSView {
    override var isFlipped: Bool { true }
}

/// A row of buttons in the Options window. Unlike a list or a field, it keeps its own width.
private final class OptionButtonRow: NSStackView {}

private let optionNoteIdentifier = NSUserInterfaceItemIdentifier("option-note")
// What the Source B list stores for its Off entry.
private let offSourceKey = "off"

/// A snapshot or a drag as the status line names it.
private func exportDescription(_ files: [URL]) -> String {
    guard let first = files.first else { return "" }
    let positions = files.filter { $0.lastPathComponent.hasSuffix("-Positions.json") }.count
    let wavs = files.count - positions
    let folder = first.deletingLastPathComponent().path
    if files.count == 1 { return first.path }
    let wavText = wavs == 1 ? "1 WAV file" : "\(wavs) WAV files"
    switch positions {
    case 0: return "\(wavText) in \(folder)"
    case 1: return "\(wavText) and a positions file in \(folder)"
    default: return "\(wavText) and \(positions) positions files in \(folder)"
    }
}

private func extractFileDropURLs(from info: NSDraggingInfo) -> [URL] {
    let pb = info.draggingPasteboard
    let classes: [AnyClass] = [NSURL.self]
    let options: [NSPasteboard.ReadingOptionKey: Any] = [.urlReadingFileURLsOnly: true]
    return (pb.readObjects(forClasses: classes, options: options) as? [URL]) ?? []
}

private func drawPackageDropHint(in bounds: NSRect, accentColor: NSColor) {
    let rect = bounds.insetBy(dx: 3, dy: 3)
    guard rect.width > 40, rect.height > 20 else { return }
    let path = NSBezierPath(roundedRect: rect, xRadius: 6, yRadius: 6)
    NSColor.black.withAlphaComponent(200.0 / 255.0).setFill()
    path.fill()
    accentColor.setStroke()
    path.lineWidth = 2
    path.stroke()
    let fontSize: CGFloat = rect.height < 120 ? 13 : 17
    let style = NSMutableParagraphStyle()
    style.alignment = .center
    let text = NSAttributedString(
        string: "Drop to review this add-on before it is installed",
        attributes: [
            .font: NSFont.systemFont(ofSize: fontSize, weight: .medium),
            .foregroundColor: NSColor(white: 235.0 / 255.0, alpha: 1),
            .paragraphStyle: style,
        ]
    )
    let box = text.boundingRect(with: NSSize(width: max(40, rect.width - 24), height: rect.height - 12), options: [.usesLineFragmentOrigin])
    let drawRect = NSRect(
        x: rect.minX + 12,
        y: rect.midY - box.height / 2,
        width: max(40, rect.width - 24),
        height: box.height
    )
    text.draw(in: drawRect)
}

private final class OptionsDropContainerView: NSView {
    var onDropFiles: (([URL]) -> Bool)?
    var accentColorProvider: (() -> NSColor)?
    private var packageHovered = false { didSet { if packageHovered != oldValue { needsDisplay = true } } }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        registerForDraggedTypes([.fileURL])
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        registerForDraggedTypes([.fileURL])
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        let urls = extractFileDropURLs(from: sender)
        guard !urls.isEmpty else { return [] }
        packageHovered = urls.contains(where: AddonFormatting.isPackageURL)
        return .copy
    }

    override func draggingExited(_ sender: NSDraggingInfo?) {
        packageHovered = false
    }

    override func draggingEnded(_ sender: NSDraggingInfo) {
        packageHovered = false
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        packageHovered = false
        let urls = extractFileDropURLs(from: sender)
        guard !urls.isEmpty else { return false }
        return onDropFiles?(urls) ?? false
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        if packageHovered {
            drawPackageDropHint(in: bounds, accentColor: accentColorProvider?() ?? accentColor(defaultAccentRGB))
        }
    }
}

private final class OptionsChoiceTable: NSTableView {
    var onCommit: ((Int) -> Void)?
    var onCancel: (() -> Void)?
    var canSelectRow: ((Int) -> Bool)?

    override func keyDown(with event: NSEvent) {
        switch event.keyCode {
        case 36, 49, 76: onCommit?(selectedRow) // Return, Space, keypad Enter.
        case 53: onCancel?()
        case 125, 126:
            let direction = event.keyCode == 125 ? 1 : -1
            var row = selectedRow >= 0 ? selectedRow + direction : (direction > 0 ? 0 : numberOfRows - 1)
            while row >= 0 && row < numberOfRows {
                if canSelectRow?(row) != false {
                    selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
                    scrollRowToVisible(row)
                    break
                }
                row += direction
            }
        default: super.keyDown(with: event)
        }
    }
}

// NSPopUpButton keeps the existing tagged menu model and target/action contract.
// A bounded, native table supplies the ten-row limit that NSMenu does not expose.
private final class OptionsPopUpButton: NSPopUpButton, NSTableViewDataSource, NSTableViewDelegate {
    private var choicePopover: NSPopover?
    private weak var choiceTable: OptionsChoiceTable?
    private var choiceItems: [NSMenuItem] = []

    var isChoicePopoverShown: Bool { choicePopover?.isShown == true }

    /// Every list that has opened, so a window can ask whether any is open now, whichever it is.
    private static let opened = NSHashTable<OptionsPopUpButton>.weakObjects()
    static var anyChoicePopoverShown: Bool {
        #if BACKTRACKING_CAPTURE_VERIFY
        if let verifyShown { return verifyShown }
        #endif
        return opened.allObjects.contains(where: \.isChoicePopoverShown)
    }
    #if BACKTRACKING_CAPTURE_VERIFY
    /// Stands in for a list a verification cannot open.
    static var verifyShown: Bool?
    #endif

    func reloadChoicesIfOpen() {
        guard choicePopover?.isShown == true, let table = choiceTable else { return }
        choiceItems = itemArray
        table.reloadData()
        let selected = indexOfSelectedItem
        if choiceItems.indices.contains(selected) {
            table.selectRowIndexes(IndexSet(integer: selected), byExtendingSelection: false)
        }
    }

    override func mouseDown(with event: NSEvent) { showChoices() }
    override func performClick(_ sender: Any?) { showChoices() }

    override func keyDown(with event: NSEvent) {
        if [36, 49, 76, 125, 126].contains(event.keyCode) {
            showChoices()
        } else {
            super.keyDown(with: event)
        }
    }

    private func showChoices() {
        guard isEnabled else { return }
        if choicePopover?.isShown == true {
            choicePopover?.performClose(nil)
            return
        }
        if let menu { menu.delegate?.menuNeedsUpdate?(menu) }
        choiceItems = itemArray
        guard !choiceItems.isEmpty else { return }

        let rowHeight: CGFloat = 30
        let width = max(260, bounds.width)
        let visibleRows = min(10, choiceItems.count)
        let size = NSSize(width: width, height: CGFloat(visibleRows) * rowHeight + 8)
        let table = OptionsChoiceTable(frame: NSRect(
            x: 0, y: 0, width: width - 8, height: CGFloat(choiceItems.count) * rowHeight
        ))
        table.headerView = nil
        table.rowHeight = rowHeight
        table.intercellSpacing = .zero
        table.selectionHighlightStyle = .regular
        table.allowsEmptySelection = false
        table.allowsMultipleSelection = false
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.setAccessibilityLabel("Choices")
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("choice"))
        column.width = width - 8
        table.addTableColumn(column)
        table.dataSource = self
        table.delegate = self
        table.reloadData()
        table.target = self
        table.action = #selector(commitClickedChoice(_:))
        table.onCommit = { [weak self] row in self?.commitChoice(row) }
        table.onCancel = { [weak self] in self?.choicePopover?.performClose(nil) }
        table.canSelectRow = { [weak self] row in
            guard let self, self.choiceItems.indices.contains(row) else { return false }
            return self.choiceItems[row].isEnabled
        }

        let content = NSView(frame: NSRect(origin: .zero, size: size))
        let scroll = NSScrollView(frame: content.bounds.insetBy(dx: 4, dy: 4))
        scroll.autoresizingMask = [.width, .height]
        scroll.borderType = .noBorder
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = choiceItems.count > 10
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.documentView = table
        content.addSubview(scroll)
        let controller = NSViewController()
        controller.view = content
        let popover = NSPopover()
        popover.contentViewController = controller
        popover.contentSize = size
        popover.behavior = .transient
        popover.animates = false
        choicePopover = popover
        Self.opened.add(self)
        choiceTable = table
        popover.show(relativeTo: bounds, of: self, preferredEdge: .minY)
        let selected = indexOfSelectedItem
        if choiceItems.indices.contains(selected) {
            table.selectRowIndexes(IndexSet(integer: selected), byExtendingSelection: false)
            table.scrollRowToVisible(selected)
        }
        popover.contentViewController?.view.window?.makeFirstResponder(table)
    }

    func numberOfRows(in tableView: NSTableView) -> Int { choiceItems.count }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        choiceItems.indices.contains(row) && choiceItems[row].isEnabled
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard choiceItems.indices.contains(row) else { return nil }
        let item = choiceItems[row]
        let label = NSTextField(labelWithString: item.title)
        label.font = .systemFont(ofSize: 15)
        label.textColor = item.isEnabled ? .labelColor : .disabledControlTextColor
        label.lineBreakMode = .byTruncatingMiddle
        label.maximumNumberOfLines = 1
        label.toolTip = item.toolTip ?? item.title
        return label
    }

    @objc private func commitClickedChoice(_ table: NSTableView) { commitChoice(table.clickedRow) }

    private func commitChoice(_ row: Int) {
        guard choiceItems.indices.contains(row), choiceItems[row].isEnabled else { return }
        let chosen = choiceItems[row]
        // Source lists may be refreshed while the list is open. Match the choice,
        // rather than accidentally applying a new item that occupies its old row.
        guard let index = itemArray.firstIndex(where: { current in
            current === chosen || (current.tag == chosen.tag && current.title == chosen.title
                && (current.representedObject as? String) == (chosen.representedObject as? String))
        }), itemArray[index].isEnabled else {
            choicePopover?.performClose(nil)
            return
        }
        selectItem(at: index)
        choicePopover?.performClose(nil)
        if let action { _ = sendAction(action, to: target) }
    }
}

private final class CapturePanel: KeyPanel {
    var onClick: (() -> Void)?
    var onMenu: ((NSEvent) -> Void)?
    var onManualResize: (() -> Void)?
    /// Runs one of the recorder's own keys. False lets the key go on to the focused control.
    var onCommand: ((RecorderAction) -> Bool)?
    var actionForShortcut: ((RecorderShortcut, Bool, Bool) -> RecorderAction?)?
    var hasPopupOpen: (() -> Bool)?
    weak var waveform: WaveformView?
    private var move: MoveGesture?
    private var resize: ResizeGesture?
    private var chromeCursorActive = false
    /// When Right Command went down on its own, until it comes up or anything else happens.
    private var rightCommandPress: TimeInterval?

    private struct Edges: OptionSet {
        let rawValue: UInt8
        static let left = Self(rawValue: 1 << 0)
        static let right = Self(rawValue: 1 << 1)
        static let bottom = Self(rawValue: 1 << 2)
        static let top = Self(rawValue: 1 << 3)

        @MainActor var cursor: NSCursor {
            let horizontal = contains(.left) || contains(.right)
            let vertical = contains(.top) || contains(.bottom)
            if horizontal && vertical {
                return contains(.left) == contains(.top) ? Self.downDiagonal : Self.upDiagonal
            }
            return horizontal ? .resizeLeftRight : .resizeUpDown
        }

        @MainActor private static let downDiagonal = diagonal("arrow.up.left.and.arrow.down.right")
        @MainActor private static let upDiagonal = diagonal("arrow.up.right.and.arrow.down.left")

        @MainActor private static func diagonal(_ symbol: String) -> NSCursor {
            guard let image = NSImage(systemSymbolName: symbol, accessibilityDescription: "Resize window") else {
                return .resizeLeftRight
            }
            return NSCursor(image: image, hotSpot: NSPoint(x: image.size.width / 2, y: image.size.height / 2))
        }
    }

    private struct ResizeGesture {
        let edges: Edges
        let startPointer: NSPoint
        let startFrame: NSRect
    }

    private struct MoveGesture {
        let startPointer: NSPoint
        let startOrigin: NSPoint
    }

    /// A tap is Right Command going down and up again soon, with no other key, modifier or
    /// mouse button in between. Command held for a shortcut, or for Command-Option window
    /// moves, is not one.
    private func followRightCommand(_ event: NSEvent) {
        guard event.type == .flagsChanged, event.keyCode == rightCommandKeyCode else {
            rightCommandPress = nil
            return
        }
        let flags = event.modifierFlags
        if flags.rawValue & rightCommandDeviceMask != 0 {
            let alone = flags.intersection([.shift, .control, .option, .function]).isEmpty
                && flags.rawValue & leftCommandDeviceMask == 0 && NSEvent.pressedMouseButtons == 0
            rightCommandPress = alone ? event.timestamp : nil
        } else if let press = rightCommandPress {
            rightCommandPress = nil
            if event.timestamp - press <= tapLimit, let action = actionForShortcut?(.rightCommand, false, false) {
                _ = onCommand?(action)
            }
        }
    }

    override func resignKey() {
        super.resignKey()
        // Keys pressed while another window has the keyboard are not this window's.
        rightCommandPress = nil
    }

    override func sendEvent(_ event: NSEvent) {
        switch event.type {
        case .flagsChanged, .keyDown, .leftMouseDown, .rightMouseDown, .otherMouseDown, .scrollWheel:
            followRightCommand(event)
        default: break
        }
        switch event.type {
        case .keyDown:
            if handleShortcut(event) { return }
        case .leftMouseDown:
            onClick?()
            move = nil
            resize = nil
            if let area = clientArea, area.contains(event.locationInWindow) {
                let startPointer = convertPoint(toScreen: event.locationInWindow)
                if canMove(at: event.locationInWindow, modifiers: event.modifierFlags) {
                    waveform?.clearPointerFeedback()
                    move = MoveGesture(startPointer: startPointer, startOrigin: frame.origin)
                    setChromeCursor(.closedHand)
                    return
                }
                let edges = resizeEdges(at: event.locationInWindow, in: area)
                let selectionModifier = selectsWaveform(event.modifierFlags)
                let waveformInteraction = waveformContains(event.locationInWindow)
                    && (selectionModifier || waveform?.hasDraggableFlag(at: event.locationInWindow) == true)
                if !edges.isEmpty && !waveformInteraction {
                    waveform?.clearPointerFeedback()
                    resize = ResizeGesture(edges: edges, startPointer: startPointer, startFrame: frame)
                    setChromeCursor(edges.cursor)
                    return
                }
            }
        case .leftMouseDragged, .leftMouseUp:
            if let resize {
                applyResize(resize)
                if event.type == .leftMouseUp {
                    self.resize = nil
                    updateChromeCursor()
                }
                return
            }
            if let move {
                applyMove(move)
                if event.type == .leftMouseUp {
                    self.move = nil
                    updateChromeCursor()
                }
                return
            }
        case .rightMouseDown:
            onClick?()
            let controlSelect = event.modifierFlags.contains(.control) && waveformContains(event.locationInWindow)
            if !controlSelect, let onMenu {
                onMenu(event)
                return
            }
        case .otherMouseDown:
            onClick?()
        default: break
        }
        super.sendEvent(event)
        switch event.type {
        case .mouseMoved, .mouseEntered, .cursorUpdate, .flagsChanged:
            updateChromeCursor()
        default: break
        }
    }

    private func handleShortcut(_ event: NSEvent) -> Bool {
        guard !(firstResponder is NSTextView) else { return false }
        let shortcut = RecorderShortcut(event: event)
        let popupOpen = hasPopupOpen?() == true
        let canCancel = !popupOpen && (waveform?.canCancelSelection == true || waveform?.hasPendingDragWrite == true)
        guard let action = actionForShortcut?(shortcut, event.isARepeat, canCancel) else { return false }
        if action == .cancelSelection {
            // HK-05: consume cancel key when selection or drag gesture is active and no popup is open.
            guard canCancel else { return false }
            _ = onCommand?(action)
            return true
        }
        guard !event.isARepeat else { return false }
        if firstResponder is NSControl, shortcut.modifiers.isEmpty { return false }
        return onCommand?(action) == true
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.type == .keyDown,
           !event.modifierFlags.intersection([.command, .control, .option, .shift]).isEmpty,
           handleShortcut(event) { return true }
        return super.performKeyEquivalent(with: event)
    }

    func trackChrome(in content: NSView) {
        content.addTrackingArea(NSTrackingArea(
            rect: .zero, options: [.mouseEnteredAndExited, .mouseMoved, .activeAlways, .inVisibleRect],
            owner: self, userInfo: nil
        ))
    }

    override func mouseExited(with event: NSEvent) {
        if NSEvent.pressedMouseButtons & 1 == 0 {
            move = nil
            resize = nil
            setChromeCursor(nil)
        }
        super.mouseExited(with: event)
    }

    private var clientArea: NSRect? {
        guard !styleMask.contains(.fullScreen), let contentView else { return nil }
        return contentView.convert(contentView.bounds, to: nil)
    }

    private func canMove(at location: NSPoint, modifiers: NSEvent.ModifierFlags) -> Bool {
        guard modifiers.contains(.option), !modifiers.contains(.control) else { return false }
        if movesFromWaveform(modifiers) { return true }
        return !waveformContains(location)
    }

    private func waveformContains(_ location: NSPoint) -> Bool {
        guard let waveform else { return false }
        return waveform.bounds.contains(waveform.convert(location, from: nil))
    }

    private func resizeEdges(at location: NSPoint, in area: NSRect) -> Edges {
        // The hit area sits inside the existing content gutters.
        let gutter: CGFloat = 7
        var edges: Edges = []
        if location.x - area.minX < gutter { edges.insert(.left) }
        if area.maxX - location.x < gutter { edges.insert(.right) }
        if location.y - area.minY < gutter { edges.insert(.bottom) }
        if area.maxY - location.y < gutter { edges.insert(.top) }
        return edges
    }

    private func applyMove(_ gesture: MoveGesture) {
        let mouse = NSEvent.mouseLocation
        let origin = NSPoint(
            x: gesture.startOrigin.x + mouse.x - gesture.startPointer.x,
            y: gesture.startOrigin.y + mouse.y - gesture.startPointer.y
        )
        if frame.origin != origin { setFrameOrigin(origin) }
    }

    private func applyResize(_ gesture: ResizeGesture) {
        let minimum = frameRect(forContentRect: NSRect(origin: .zero, size: contentMinSize)).size
        let mouse = NSEvent.mouseLocation
        let dx = mouse.x - gesture.startPointer.x
        let dy = mouse.y - gesture.startPointer.y
        let start = gesture.startFrame
        let edges = gesture.edges
        let left = edges.contains(.left) ? min(start.minX + dx, start.maxX - minimum.width) : start.minX
        let right = edges.contains(.right) ? max(start.maxX + dx, start.minX + minimum.width) : start.maxX
        let bottom = edges.contains(.bottom) ? min(start.minY + dy, start.maxY - minimum.height) : start.minY
        let top = edges.contains(.top) ? max(start.maxY + dy, start.minY + minimum.height) : start.maxY
        let proposed = NSRect(x: left, y: bottom, width: right - left, height: top - bottom)
        if frame != proposed {
            setFrame(proposed, display: true)
            onManualResize?()
        }
    }

    private func updateChromeCursor() {
        if let resize {
            if NSEvent.pressedMouseButtons & 1 != 0 {
                setChromeCursor(resize.edges.cursor)
                return
            }
            self.resize = nil
        }
        if NSEvent.pressedMouseButtons & 1 == 0 { move = nil }
        // A waveform gesture owns the pointer until release, even if it crosses a gutter.
        guard NSEvent.pressedMouseButtons == 0 else { return }
        let location = mouseLocationOutsideOfEventStream
        guard let area = clientArea, area.contains(location) else {
            setChromeCursor(nil)
            return
        }
        if canMove(at: location, modifiers: NSEvent.modifierFlags) {
            setChromeCursor(.openHand)
        } else {
            let edges = resizeEdges(at: location, in: area)
            setChromeCursor(edges.isEmpty ? nil : edges.cursor)
        }
    }

    private func setChromeCursor(_ cursor: NSCursor?) {
        if let cursor {
            cursor.set()
            chromeCursorActive = true
        } else if chromeCursorActive {
            NSCursor.arrow.set()
            chromeCursorActive = false
        }
    }
}

private struct WaveformTrace: Sendable {
    var low: [Float]
    var high: [Float]

    init(bins: Int = 0) {
        low = Array(repeating: 0, count: bins)
        high = Array(repeating: 0, count: bins)
    }

    mutating func read(_ copy: (UnsafeMutablePointer<Float>, UnsafeMutablePointer<Float>, UInt32) -> Void) {
        guard !low.isEmpty else { return }
        low.withUnsafeMutableBufferPointer { minimum in
            high.withUnsafeMutableBufferPointer { maximum in
                copy(minimum.baseAddress!, maximum.baseAddress!, UInt32(minimum.count))
            }
        }
    }
}

private struct WaveformRow: Equatable, Sendable {
    let identity: RowIdentity
    let lane: Int
    let view: UInt32
    let title: String?
    let shortLabel: String?
    let fullLabel: String
    let laneChannelCount: Int
    let channelNumber: Int?
    var isPair: Bool { identity.isPair }

    var spec: WaveformRowSpec {
        WaveformRowSpec(
            identity: identity, lane: lane, view: view, shortLabel: shortLabel,
            fullLabel: fullLabel, laneChannelCount: laneChannelCount, channelNumberOneBased: channelNumber
        )
    }

    static func layout(
        widths: [UInt32], labels: [[String]], stereo: Bool,
        activeMasks: [UInt64?] = [],
        shownGroups: [(group: AddonChannelGroup, range: Range<Int>, hasBed: Bool)?] = []
    ) -> [WaveformRow] {
        widths.enumerated().flatMap { lane, width -> [WaveformRow] in
            let name = widths.count > 1 ? captureLaneLetter(lane) : nil
            let count = Int(width)
            if width > 2 {
                let laneLabels = labels.indices.contains(lane) ? labels[lane] : []
                let groupSpec = shownGroups.indices.contains(lane) ? shownGroups[lane] : nil
                let includePair = groupSpec?.hasBed ?? true
                let pair: [WaveformRow] = includePair ? [(0, UInt32(67), "L"), (1, UInt32(68), "R")].map { side, view, label in
                    WaveformRow(
                        identity: .pair(lane: lane, side: side), lane: lane, view: view,
                        title: name.map { "\($0) · \(label)" } ?? label,
                        shortLabel: label, fullLabel: label, laneChannelCount: count, channelNumber: nil
                    )
                } : []
                let shown: [Int]
                if let groupSpec {
                    shown = Array(groupSpec.range.lowerBound..<min(count, groupSpec.range.upperBound))
                } else {
                    let mask: UInt64? = activeMasks.indices.contains(lane) ? activeMasks[lane] : nil
                    shown = ChannelActivityTracker.shownChannels(width: count, labels: laneLabels, activeMask: mask)
                }
                return pair + shown.map { channel in
                    let full = laneLabels.indices.contains(channel) ? laneLabels[channel] : "Channel \(channel + 1)"
                    let short = RowStackLayout.shortLabel(full)
                    let title = name.map { "\($0) · \(full)" } ?? full
                    return WaveformRow(
                        identity: .channel(lane: lane, index: channel), lane: lane, view: UInt32(channel + 3),
                        title: title, shortLabel: short, fullLabel: full,
                        laneChannelCount: count, channelNumber: channel + 1
                    )
                }
            }
            if width == 2, stereo, widths.count == 1 {
                return [
                    WaveformRow(identity: .stereoSide(lane: lane, side: 0), lane: lane, view: 1,
                                title: "L", shortLabel: "L", fullLabel: "L", laneChannelCount: 2, channelNumber: 1),
                    WaveformRow(identity: .stereoSide(lane: lane, side: 1), lane: lane, view: 2,
                                title: "R", shortLabel: "R", fullLabel: "R", laneChannelCount: 2, channelNumber: 2)
                ]
            }
            return [WaveformRow(
                identity: .whole(lane: lane), lane: lane, view: 0,
                title: name, shortLabel: name, fullLabel: name ?? "A",
                laneChannelCount: count, channelNumber: nil
            )]
        }
    }
}

private struct WaveformReadRequest: Equatable, Sendable {
    var view: Range<UInt64>?
    var width: Float
    var height: Float
    var rows: [WaveformRow]
    var interactive: Bool

    func policy(frames: Range<UInt64>, rate: UInt32) -> BtWaveformDisplayPolicy {
        bt_waveform_display_policy(width, height, frames.upperBound - frames.lowerBound,
            rate, interactive ? 1 : 0)
    }
}

private func fittedWaveformRange(_ view: Range<UInt64>?, in available: Range<UInt64>) -> Range<UInt64> {
    guard let view, !available.isEmpty else { return available }
    let span = min(view.upperBound - view.lowerBound, available.upperBound - available.lowerBound)
    let start = min(max(view.lowerBound, available.lowerBound), available.upperBound - span)
    return start..<(start + span)
}

private struct WaveformFrame: Sendable {
    let revision: UInt64
    let sampleRate: UInt32
    let retained: Range<UInt64>
    let visible: Range<UInt64>
    let traces: [WaveformTrace]
    let bars: [UInt64]

    static func read(core: OpaquePointer, request: WaveformReadRequest, unchangedRevision: UInt64?) -> WaveformFrame? {
        let revision = bt_tracks_revision(core)
        if revision == unchangedRevision { return nil }
        let rate = bt_tracks_sample_rate(core)
        let first = bt_tracks_first_frame(core)
        let retained = first..<max(first, bt_tracks_end_frame(core))
        let visible = fittedWaveformRange(request.view, in: retained)
        let bins = Int(request.policy(frames: visible, rate: rate).bins)
        let traces = request.rows.map { row -> WaveformTrace in
            var trace = WaveformTrace(bins: bins)
            trace.read { minimum, maximum, count in
                _ = bt_tracks_waveform_range(core, UInt32(row.lane), row.view, visible.lowerBound, visible.upperBound,
                    minimum, maximum, count)
            }
            return trace
        }
        let count = bt_tracks_copy_bar_markers(core, visible.lowerBound, visible.upperBound, nil, 0)
        var bars = Array<UInt64>(repeating: 0, count: count)
        let actual = bars.withUnsafeMutableBufferPointer {
            bt_tracks_copy_bar_markers(core, visible.lowerBound, visible.upperBound, $0.baseAddress, $0.count)
        }
        if actual < count { bars.removeLast(count - actual) }
        return WaveformFrame(revision: revision, sampleRate: rate, retained: retained,
            visible: visible, traces: traces, bars: bars)
    }
}

// Immutable recording material shared by the edit renderer and WAV worker.
/// One freeze of the capture that is on its way to the main queue. The capture queue fills it in,
/// so quitting can still use the audio after the main-queue completion can no longer run.
private final class EditingMaterialRead: @unchecked Sendable {
    let tap: ObjectIdentifier
    private let lock = NSLock()
    private var done = false
    private var frozen: WAVExport?

    init(tap: ObjectIdentifier) { self.tap = tap }

    func finish(_ material: WAVExport?) {
        lock.lock()
        done = true
        frozen = material
        lock.unlock()
    }

    /// The frozen audio once the read has run; nil before that or when there was none.
    var material: WAVExport? {
        lock.lock()
        defer { lock.unlock() }
        return done ? frozen : nil
    }
}

/// Saved clips whose `clip_exported` has not gone to the add-ons yet. The export worker adds each
/// one as soon as its files are written, so quitting can still announce saves the main queue never saw.
private final class ClipExportLedger: @unchecked Sendable {
    struct Clip {
        let urls: [URL]
        let sampleRate: UInt32
        let frames: UInt64

        /// A clip of `sourceFrames` at `sourceRate` written at `sampleRate`: `frames` counts the
        /// files' own frames, rounded as Windows `clip_export::export_rows` does.
        init(urls: [URL], sampleRate: UInt32, sourceFrames: UInt64, sourceRate: UInt32) {
            self.urls = urls
            self.sampleRate = sampleRate
            let rate = UInt64(max(1, sourceRate))
            let (scaled, overflow) = sourceFrames.multipliedReportingOverflow(by: UInt64(sampleRate))
            let rounded = overflow
                ? UInt64((Double(sourceFrames) * Double(sampleRate) / Double(rate)).rounded())
                : (scaled + rate / 2) / rate
            frames = max(rounded, sourceFrames == 0 ? 0 : 1)
        }
    }
    private let lock = NSLock()
    private var next: UInt64 = 0
    private var clips: [UInt64: Clip] = [:]

    func reserve() -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        next &+= 1
        return next
    }

    func add(_ clip: Clip, as token: UInt64) {
        lock.lock()
        clips[token] = clip
        lock.unlock()
    }

    func take(_ token: UInt64) -> Clip? {
        lock.lock()
        defer { lock.unlock() }
        return clips.removeValue(forKey: token)
    }

    /// Every clip not announced yet, in the order they were reserved.
    func takeAll() -> [Clip] {
        lock.lock()
        defer { lock.unlock() }
        let all = clips.sorted { $0.key < $1.key }.map(\.value)
        clips.removeAll()
        return all
    }
}

private final class WAVExport: @unchecked Sendable {
    private static let releaseQueue = DispatchQueue(label: "Backtracking.material-release", qos: .utility)
    private let frozen: OpaquePointer
    private let routes: [UInt32]
    let span: Range<UInt64>
    let sampleRate: UInt32
    let revision: UInt64
    let widths: [UInt32]
    private let channelNames: [[String]]
    let channelGroups: [[AddonChannelGroup]]
    private let positions: [Int: FrozenPositionsHandle]
    private let bars: [UInt64]
    private let cacheLock = NSLock()
    private var cachedActivity: [Int: UInt64] = [:]
    private struct PeakKey: Equatable {
        let visible: Range<UInt64>
        let bins: Int
        let rows: [WaveformRow]
    }
    private var cachedPeaks: (PeakKey, WaveformFrame)?

    init?(
        core: OpaquePointer,
        routes: [CaptureRoute],
        channelNames: [[String]] = [],
        channelGroups: [[AddonChannelGroup]] = [],
        positions: [Int: FrozenPositionsHandle] = [:],
        range: Range<UInt64>? = nil
    ) {
        let first = bt_tracks_first_frame(core)
        let capturedSpan = range ?? first..<max(first, bt_tracks_end_frame(core))
        guard !capturedSpan.isEmpty,
              let frozen = bt_tracks_freeze(core, capturedSpan.lowerBound, capturedSpan.upperBound) else { return nil }
        self.frozen = frozen
        span = capturedSpan
        sampleRate = bt_tracks_sample_rate(core)
        revision = bt_tracks_revision(core)
        self.routes = routes.map(\.wavRouteCode)
        self.widths = routes.indices.map { bt_tracks_channels(core, UInt32($0)) }
        self.channelNames = channelNames
        self.channelGroups = channelGroups
        self.positions = positions
        let markerCount = bt_tracks_copy_bar_markers(core, capturedSpan.lowerBound, capturedSpan.upperBound, nil, 0)
        var markers = Array<UInt64>(repeating: 0, count: markerCount)
        let actual = markers.withUnsafeMutableBufferPointer {
            bt_tracks_copy_bar_markers(core, capturedSpan.lowerBound, capturedSpan.upperBound, $0.baseAddress, $0.count)
        }
        if actual < markerCount { markers.removeLast(markerCount - actual) }
        bars = markers
    }

    deinit {
        let address = UInt(bitPattern: frozen)
        Self.releaseQueue.async { bt_frozen_free(OpaquePointer(bitPattern: address)) }
    }

    private func bedInfo(for lane: Int) -> (start: UInt32, count: UInt32, labels: [String]) {
        let width = widths.indices.contains(lane) ? Int(widths[lane]) : 0
        if channelGroups.indices.contains(lane), !channelGroups[lane].isEmpty {
            var offset = 0
            for group in channelGroups[lane] {
                let end = offset + group.channels.count
                if !group.positioned && end <= width {
                    return (UInt32(offset), UInt32(group.channels.count), group.channels)
                }
                offset = end
            }
        }
        let all = channelNames.indices.contains(lane) ? channelNames[lane] : []
        return (0, UInt32(width), all)
    }

    func render(_ request: WaveformReadRequest) -> WaveformFrame {
        let visible = fittedWaveformRange(request.view, in: span)
        let bins = Int(request.policy(frames: visible, rate: sampleRate).bins)
        let key = PeakKey(visible: visible, bins: bins, rows: request.rows)
        cacheLock.lock()
        let cached = cachedPeaks
        cacheLock.unlock()
        if let cached, cached.0 == key { return cached.1 }
        let traces = request.rows.map { row -> WaveformTrace in
            var trace = WaveformTrace(bins: bins)
            trace.read { minimum, maximum, count in
                withBedNames(row) { bedStart, bedCount, names in
                    bt_frozen_waveform_bed(
                        frozen, UInt32(row.lane), row.view, bedStart, bedCount,
                        visible.lowerBound, visible.upperBound, minimum, maximum, count, names
                    )
                }
            }
            return trace
        }
        let frame = WaveformFrame(revision: revision, sampleRate: sampleRate, retained: span, visible: visible,
            traces: traces, bars: bars)
        cacheLock.lock()
        cachedPeaks = (key, frame)
        cacheLock.unlock()
        return frame
    }

    func spectrum(range: Range<UInt64>, bins: Int, rows: [WaveformRow]) -> FrequencyWaveform {
        let energies = rows.map { row in
            var energy = [Float](repeating: 0, count: bins * FrequencyGradient.bandCount)
            energy.withUnsafeMutableBufferPointer {
                let pointer = $0.baseAddress
                withBedNames(row) { bedStart, bedCount, names in
                    bt_frozen_spectrum_bed(
                        frozen, UInt32(row.lane), row.view, bedStart, bedCount,
                        range.lowerBound, range.upperBound, pointer, UInt32(bins), names
                    )
                }
            }
            return energy
        }
        return FrequencyWaveform(range: range, rows: energies)
    }

    private func withBedNames(_ row: WaveformRow, _ read: (UInt32, UInt32, UnsafePointer<CChar>?) -> Void) {
        guard row.isPair else { read(0, 0, nil); return }
        let bed = bedInfo(for: row.lane)
        if bed.labels.isEmpty {
            read(bed.start, bed.count, nil)
        } else {
            bed.labels.joined(separator: "\t").withCString { read(bed.start, bed.count, $0) }
        }
    }

    func activeChannels(for lane: Int) -> UInt64 {
        cacheLock.lock()
        if let value = cachedActivity[lane] {
            cacheLock.unlock()
            return value
        }
        cacheLock.unlock()
        let mask = bt_frozen_active_channels(frozen, UInt32(lane))
        cacheLock.lock()
        cachedActivity[lane] = mask
        cacheLock.unlock()
        return mask
    }

    private func resolvedGroupBounds(
        for lane: Int, width: Int, preferredID: String?
    ) -> (shownStart: UInt32, shownCount: UInt32, shownPositioned: UInt32, bedStart: UInt32, bedCount: UInt32) {
        guard width > 2, channelGroups.indices.contains(lane), !channelGroups[lane].isEmpty else {
            return (0, 0, 0, 0, 0)
        }
        let groups = channelGroups[lane]
        var offset = 0
        var shown: (Int, Int, Bool)?
        var bed: (Int, Int)?
        for g in groups {
            let end = offset + g.channels.count
            if end <= width {
                if !g.positioned && bed == nil {
                    bed = (offset, g.channels.count)
                }
                if let preferredID, g.id == preferredID {
                    shown = (offset, g.channels.count, g.positioned)
                }
            }
            offset = end
        }
        if shown == nil, let first = groups.first, first.channels.count <= width {
            shown = (0, first.channels.count, first.positioned)
        }
        let s = shown ?? (0, width, false)
        let b = bed ?? (0, 0)
        return (UInt32(s.0), UInt32(s.1), s.2 ? 1 : 0, UInt32(b.0), UInt32(b.1))
    }

    /// Where each of lane `lane`'s groups lies, in order.
    private func groupSpans(for lane: Int, width: Int) -> [(range: Range<Int>, positioned: Bool)] {
        guard channelGroups.indices.contains(lane) else { return [] }
        var offset = 0
        return channelGroups[lane].compactMap { group in
            let range = offset..<(offset + group.channels.count)
            offset = range.upperBound
            return range.upperBound <= width && !range.isEmpty ? (range, group.positioned) : nil
        }
    }

    /// The group chosen channel rows are written or played with, whatever group is on show now
    /// (Windows `lane_write`, `preview_plan`). Export names them by the objects when any row is an
    /// object, else by the group that holds them; preview plays the group that holds every one.
    /// nil when no group fits (preview then plays the pair).
    private func groupHolding(_ channels: [Int], lane: Int, width: Int, forPreview: Bool) -> (range: Range<Int>, positioned: Bool)? {
        let spans = groupSpans(for: lane, width: width)
        guard !spans.isEmpty, !channels.isEmpty else { return nil }
        if forPreview {
            return spans.first { span in channels.allSatisfy { span.range.contains($0) } }
        }
        let holding = { (objects: Bool) in
            spans.first { $0.positioned == objects && channels.contains(where: $0.range.contains) }
        }
        return holding(true) ?? holding(false)
    }

    private func buildGroupSelectionLanes(
        names: [String],
        selectedRows: [RowIdentity]?,
        hideSilentLanes: Set<Int>,
        shownGroupIDs: [String?],
        forPreview: Bool = false
    ) -> (lanes: [BtFrozenGroupSelectionLane], sources: [UnsafeMutablePointer<CChar>?], labels: [UnsafeMutablePointer<CChar>?]) {
        var lanes: [BtFrozenGroupSelectionLane] = []
        var sources: [UnsafeMutablePointer<CChar>?] = []
        var labels: [UnsafeMutablePointer<CChar>?] = []

        for index in routes.indices {
            let width = widths.indices.contains(index) ? Int(widths[index]) : 0
            let laneLabels = channelNames.indices.contains(index) ? channelNames[index] : []
            let preferredID = shownGroupIDs.indices.contains(index) ? shownGroupIDs[index] : nil
            var gb = resolvedGroupBounds(for: index, width: width, preferredID: preferredID)
            let hasBed = (gb.shownCount == 0 && width > 2) || gb.bedCount > 0
            var maskValid: UInt32 = 0
            var mask: UInt64 = 0
            var includePair: UInt32 = (width > 2 && hasBed) ? 1 : 0
            let forLane = selectedRows?.filter { $0.lane == index } ?? []
            // A wide lane chosen whole is written as it is shown, with rows that came or went since.
            let laneWhole = selectedRows?.isEmpty != false || (width > 2 && forLane.contains(.whole(lane: index)))

            if !laneWhole {
                if forLane.isEmpty { continue }
                if width > 2 {
                    let hasPair = forLane.contains(where: \.isPair)
                    var channels: [Int] = []
                    for row in forLane {
                        if case .channel(_, let ch) = row, ch < 64, ch < width { channels.append(ch) }
                    }
                    var channelMask: UInt64 = 0
                    for ch in channels { channelMask |= (1 << UInt64(ch)) }
                    includePair = (hasPair && hasBed) ? 1 : 0
                    maskValid = 1
                    mask = channelMask
                    if !channels.isEmpty {
                        if let holding = groupHolding(channels, lane: index, width: width, forPreview: forPreview) {
                            gb.shownStart = UInt32(holding.range.lowerBound)
                            gb.shownCount = UInt32(holding.range.count)
                            gb.shownPositioned = holding.positioned ? 1 : 0
                        } else if forPreview, hasBed {
                            // Rows of two groups at once play the pair.
                            mask = 0
                            includePair = 1
                        }
                    }
                } else if width == 2 {
                    var sideMask: UInt64 = 0
                    var whole = false
                    for row in forLane {
                        switch row {
                        case .whole: whole = true
                        case .stereoSide(_, let side): sideMask |= (1 << UInt64(side))
                        default: break
                        }
                    }
                    if !whole && sideMask != 0 && sideMask != 0b11 {
                        maskValid = 1
                        mask = sideMask
                    }
                }
            } else if width > 2, hideSilentLanes.contains(index), gb.shownCount == 0 {
                // Whole lane (no rows chosen, or the lane chosen whole).
                // Whole-lane export with hide-silent: export only active channels in this frozen copy + L/R pair.
                let activeMask = activeChannels(for: index)
                let shown = ChannelActivityTracker.shownChannels(width: width, labels: laneLabels, activeMask: activeMask)
                var channelMask: UInt64 = 0
                for ch in shown where ch < 64 { channelMask |= (1 << UInt64(ch)) }
                maskValid = 1
                mask = channelMask
                includePair = 1
            }

            let src = names.indices.contains(index) ? strdup(names[index]) : nil
            let lbl = !laneLabels.isEmpty ? strdup(laneLabels.joined(separator: "\t")) : nil
            sources.append(src)
            labels.append(lbl)
            lanes.append(BtFrozenGroupSelectionLane(
                index: UInt32(index),
                route: routes[index],
                source_name: UnsafePointer(src),
                channel_labels: UnsafePointer(lbl),
                channel_mask: mask,
                channel_mask_valid: maskValid,
                include_pair: includePair,
                shown_start: gb.shownStart,
                shown_count: gb.shownCount,
                shown_positioned: gb.shownPositioned,
                bed_start: gb.bedStart,
                bed_count: gb.bedCount,
                positions: positions[index]?.pointer
            ))
        }
        return (lanes, sources, labels)
    }

    func play(in slot: OpaquePointer, range: Range<UInt64>, forceMono: Bool, fades: BtFades,
              selectedRows: [RowIdentity]?, hideSilentLanes: Set<Int>, pairBinaural: Bool,
              shownGroupIDs: [String?] = []) -> String? {
        let built = buildGroupSelectionLanes(
            names: [], selectedRows: selectedRows, hideSilentLanes: hideSilentLanes, shownGroupIDs: shownGroupIDs,
            forPreview: true
        )
        defer {
            for s in built.sources { free(s) }
            for l in built.labels { free(l) }
        }
        guard !built.lanes.isEmpty else { return "The selected audio is unavailable." }
        var errorPtr: UnsafeMutablePointer<CChar>?
        let ok = built.lanes.withUnsafeBufferPointer {
            bt_frozen_play_group_selection(
                frozen, slot, $0.baseAddress, $0.count, range.lowerBound,
                range.upperBound, forceMono ? 1 : 0, pairBinaural ? 1 : 0, fades, &errorPtr
            ) == 1
        }
        if let errorPtr {
            let msg = String(cString: errorPtr)
            bt_string_free(errorPtr)
            return ok ? nil : msg
        }
        return ok ? nil : "The selected audio is unavailable."
    }

    func write(
        range: Range<UInt64>, directory: String, sampleRate: UInt32, bitsPerSample: UInt32, forceMono: Bool,
        fades: BtFades, names: [String], selectedRows: [RowIdentity]?, hideSilentLanes: Set<Int>,
        wideMode: WideExportMode, pairBinaural: Bool, shownGroupIDs: [String?] = []
    ) -> (urls: [URL], objectsInMono: Bool, error: String?, lanes: [Int?]) {
        let built = buildGroupSelectionLanes(
            names: names, selectedRows: selectedRows, hideSilentLanes: hideSilentLanes, shownGroupIDs: shownGroupIDs
        )
        defer {
            for s in built.sources { free(s) }
            for l in built.labels { free(l) }
        }
        guard !built.lanes.isEmpty else { return ([], false, "Invalid capture lanes", []) }
        var objectsInMonoRaw: UInt32 = 0
        var errorPtr: UnsafeMutablePointer<CChar>?
        var lanesPtr: OpaquePointer?
        let paths = directory.withCString { destination in
            built.lanes.withUnsafeBufferPointer {
                bt_frozen_export_group_selection_lanes(
                    frozen, $0.baseAddress, $0.count, range.lowerBound, range.upperBound,
                    destination, sampleRate, bitsPerSample, forceMono ? 1 : 0, wideMode.rawValue,
                    pairBinaural ? 1 : 0, fades, &objectsInMonoRaw, &errorPtr, &lanesPtr
                )
            }
        }
        defer { if let lanesPtr { bt_export_lanes_free(lanesPtr) } }
        let errorMessage: String?
        if let errorPtr {
            errorMessage = String(cString: errorPtr)
            bt_string_free(errorPtr)
        } else {
            errorMessage = nil
        }
        guard let paths else { return ([], false, errorMessage, []) }
        defer { bt_export_paths_free(paths) }
        var urls: [URL] = []
        var lanes: [Int?] = []
        for index in 0..<bt_export_paths_count(paths) {
            guard let path = bt_export_path(paths, index) else { continue }
            urls.append(URL(fileURLWithPath: String(cString: path)))
            let lane = lanesPtr.map { bt_export_lane($0, index) } ?? UInt32.max
            lanes.append(lane == UInt32.max ? nil : Int(lane))
        }
        return (urls, objectsInMonoRaw != 0, nil, lanes)
    }
}

private let noFades = BtFades(fade_in: 0, fade_out: 0)

/// Plays a selection through the default output and reports how far it has got, for the
/// playhead. While it may be heard it holds the preview gate, so every Backtracking recorder
/// that captures System playback records silence instead of the preview.
private final class PreviewPlayer {
    private let slot: OpaquePointer
    private var engine: AVAudioEngine?
    private var engineRate: UInt32 = 0
    /// Frames between rendering a frame and hearing it.
    private var latencyFrames: UInt64 = 0
    /// What the slot reads in place, kept until the slot has let the preview go.
    private var material: WAVExport?
    private var timer: Timer?
    /// When the slot was found empty, while the device plays out what it still holds.
    private var idleSince: TimeInterval?
    /// The selection being played, until it ends or is stopped.
    private(set) var range: Range<UInt64>?
    /// Runs on the main thread when the playhead moves or the preview ends.
    var onChange: (() -> Void)?
    /// Runs on the main thread if playback stops due to an output device problem.
    var onProblem: ((String) -> Void)?
    private var lastRenderedFrame: UInt64?
    private var lastFrameProgressAt: TimeInterval = 0

    init?() {
        guard let slot = bt_preview_slot_new() else { return nil }
        self.slot = slot
    }

    deinit {
        timer?.invalidate()
        // Nothing renders the slot once the engine has stopped.
        engine?.stop()
        bt_preview_slot_free(slot)
    }

    /// Plays `range` of `material`, or of its `lane` alone, from its start. Returns a problem to
    /// show, or nil.
    func play(_ material: WAVExport, range: Range<UInt64>, forceMono: Bool, fades: BtFades,
              selectedRows: [RowIdentity]?, hideSilentLanes: Set<Int>, pairBinaural: Bool,
              shownGroupIDs: [String?] = []) -> String? {
        do {
            try startEngine(rate: material.sampleRate)
        } catch let error as CaptureError {
            return error.localizedDescription
        } catch {
            return "Could not open the playback device (\(error.localizedDescription))"
        }
        // Started before playing, so that an engine left with nothing to play is stopped too.
        if timer == nil {
            let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in self?.tick() }
            RunLoop.main.add(timer, forMode: .common)
            self.timer = timer
        }
        if let err = material.play(in: slot, range: range, forceMono: forceMono, fades: fades,
                                   selectedRows: selectedRows, hideSilentLanes: hideSilentLanes,
                                   pairBinaural: pairBinaural, shownGroupIDs: shownGroupIDs) {
            return err
        }
        // Any earlier preview has been let go, and its material with it.
        self.material = material
        self.range = range
        idleSince = nil
        lastRenderedFrame = nil
        lastFrameProgressAt = ProcessInfo.processInfo.systemUptime
        onChange?()
        return nil
    }

    /// Falls silent over a few milliseconds. The playhead goes at once.
    func stop() {
        guard range != nil else { return }
        range = nil
        lastRenderedFrame = nil
        bt_preview_slot_stop(slot)
        onChange?()
    }

    func setFades(_ fades: BtFades) {
        if range != nil { bt_preview_slot_set_fades(slot, fades) }
    }

    /// The frame being heard, while the preview plays.
    var playhead: UInt64? {
        guard let range else { return nil }
        var frame: UInt64 = 0
        guard bt_preview_slot_position(slot, &frame) == 1 else { return nil }
        let heard = frame > latencyFrames ? frame - latencyFrames : 0
        return min(range.upperBound, max(range.lowerBound, heard))
    }

    private func startEngine(rate: UInt32) throws {
        if let engine, engine.isRunning, engineRate == rate { return }
        engine?.stop()
        engine = nil
        // A new engine each time follows the output device the user has chosen since.
        let engine = AVAudioEngine()
        let outFormat = engine.outputNode.outputFormat(forBus: 0)
        if outFormat.channelCount == 0 || outFormat.sampleRate <= 0 {
            throw CaptureError("No playback device to preview on (no active output device)")
        }
        guard let format = AVAudioFormat(standardFormatWithSampleRate: Double(rate), channels: 2) else {
            throw CaptureError("Could not open the playback device (unsupported sample rate \(rate) Hz)")
        }
        let slot = self.slot
        let source = AVAudioSourceNode(format: format) { _, _, frames, buffers -> OSStatus in
            // The audio thread: no allocation, locks or Swift objects here.
            let list = UnsafeMutableAudioBufferListPointer(buffers)
            guard let left = list.first?.mData?.assumingMemoryBound(to: Float.self) else { return noErr }
            let right = list.count > 1 ? list[1].mData?.assumingMemoryBound(to: Float.self) : nil
            _ = bt_preview_slot_render(slot, left, right, frames, rate)
            return noErr
        }
        engine.attach(source)
        engine.connect(source, to: engine.mainMixerNode, format: format)
        engine.prepare()
        try engine.start()
        self.engine = engine
        engineRate = rate
        // Kept within five seconds, so that the conversions below cannot overflow.
        let latency = min(5, max(0, engine.outputNode.presentationLatency))
        latencyFrames = UInt64(latency * Double(rate))
        // A recorder may stamp what it hears that much after it was rendered.
        bt_preview_slot_set_output_latency(slot, UInt64(latency * 1_000_000_000))
    }

    private func tick() {
        let now = ProcessInfo.processInfo.systemUptime
        // The engine stops by itself when the output device changes; the preview ends with it.
        if engine?.isRunning != true {
            let wasPlaying = range != nil
            bt_preview_slot_clear(slot)
            range = nil
            lastRenderedFrame = nil
            if wasPlaying {
                onProblem?("Preview playback stopped (the playback device stopped)")
            }
        } else if range != nil {
            var frame: UInt64 = 0
            if bt_preview_slot_position(slot, &frame) == 1 {
                if lastRenderedFrame != frame {
                    lastRenderedFrame = frame
                    lastFrameProgressAt = now
                } else if now - lastFrameProgressAt >= 3.0 {
                    // Package 13 / PRE-09: 3 s with no audio taken by output device.
                    bt_preview_slot_clear(slot)
                    range = nil
                    lastRenderedFrame = nil
                    engine?.stop()
                    engine = nil
                    onProblem?("Preview playback stopped (the playback device stopped taking audio)")
                }
            } else {
                range = nil
                lastRenderedFrame = nil
            }
        }
        if bt_preview_slot_reap(slot) != 0 {
            idleSince = nil
        } else {
            material = nil
            // The device still holds the last few milliseconds; stopping it at once cuts them off.
            if let idleSince, now - idleSince >= 0.25 || engine?.isRunning != true {
                timer?.invalidate()
                timer = nil
                engine?.stop()
                engine = nil
                self.idleSince = nil
            } else if idleSince == nil {
                idleSince = now
            }
        }
        onChange?()
    }
}

// Accessed only on the WAV worker queue until the main thread takes ownership.
private final class PendingProvisionalWAVs: @unchecked Sendable {
    var urls = Set<URL>()
}

private final class CaptureLamp: NSView {
    enum State: Equatable {
        case unavailable, paused, waiting, recording

        var label: String {
            switch self {
            case .unavailable: "Capture unavailable"
            case .paused: "Paused"
            case .waiting: "Waiting for sound"
            case .recording: "Recording"
            }
        }
    }

    private var state: State?
    private var shown = true
    private var pulseStart: TimeInterval = 0
    private var bright = true

    override var intrinsicContentSize: NSSize { NSSize(width: 14, height: 14) }

    /// The bright half of the one-second recording blink, which starts lit.
    private(set) var isLit = true

    /// `shown` is false while the user's choice hides the mark. The lamp keeps its place then,
    /// so the buttons beside it do not move when it comes on.
    func update(_ state: State, shown: Bool, now: TimeInterval, detail: String) {
        let tip = shown ? detail : nil
        if toolTip != tip { toolTip = tip }
        if accessibilityLabel() != detail { setAccessibilityLabel(detail) }
        if state != self.state { pulseStart = now }
        let nextBright = state != .recording || Int((now - pulseStart) / 0.5).isMultiple(of: 2)
        isLit = nextBright
        guard state != self.state || shown != self.shown || bright != nextBright else { return }
        self.state = state
        self.shown = shown
        bright = nextBright
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard shown, let state else { return }
        let color: NSColor = switch state {
        case .unavailable: WaveformTheme.statusUnavailable
        case .paused: WaveformTheme.statusPaused
        case .waiting: WaveformTheme.statusWaiting
        case .recording: bright ? WaveformTheme.statusRecordingBright : WaveformTheme.statusRecordingDim
        }
        color.setFill()
        NSBezierPath(ovalIn: NSRect(x: bounds.midX - 4, y: bounds.midY - 4, width: 8, height: 8)).fill()
    }
}

/// When the capture status mark is shown, as chosen on the Appearance page.
private enum StatusMark: String, CaseIterable {
    case always, attention, never

    var label: String {
        switch self {
        case .always: "Always"
        case .attention: "Only when paused or failing"
        case .never: "Never"
        }
    }

    func shows(_ state: CaptureLamp.State, problem: Bool) -> Bool {
        switch self {
        case .always: true
        case .attention: problem || state == .paused || state == .unavailable
        case .never: false
        }
    }
}

/// What the mark in the corner of the waveform stands for in waveform-only mode, where there
/// is no toolbar to carry the lamp.
private struct CompactStatus: Equatable {
    var state: CaptureLamp.State
    var lit: Bool
    var problem: String?
    var detail: String
}

private final class CaptureBackground: NSView {
    weak var waveform: NSView?
    var backgroundOpacity: CGFloat = 1 { didSet { needsDisplay = true } }
    var waveformOnly = false { didSet { needsDisplay = true } }
    var showsOutline = false { didSet { needsDisplay = true } }
    var outlineWidth: CGFloat = 1 { didSet { needsDisplay = true } }
    var outlineColor = NSColor.white { didSet { needsDisplay = true } }
    var outlineFollowsOpacity = true { didSet { needsDisplay = true } }
    var look: ResolvedLook = .default { didSet { if look != oldValue { needsDisplay = true } } }
    var onDropFiles: (([URL]) -> Bool)?
    var dropAccentColor: NSColor = accentColor(defaultAccentRGB)
    private var packageHovered = false { didSet { if packageHovered != oldValue { needsDisplay = true } } }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        registerForDraggedTypes([.fileURL])
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        registerForDraggedTypes([.fileURL])
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        let urls = extractFileDropURLs(from: sender)
        guard !urls.isEmpty else { return [] }
        packageHovered = urls.contains(where: AddonFormatting.isPackageURL)
        return .copy
    }

    override func draggingExited(_ sender: NSDraggingInfo?) {
        packageHovered = false
    }

    override func draggingEnded(_ sender: NSDraggingInfo) {
        packageHovered = false
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        packageHovered = false
        let urls = extractFileDropURLs(from: sender)
        guard !urls.isEmpty else { return false }
        return onDropFiles?(urls) ?? false
    }

    override func layout() {
        super.layout()
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        NSGraphicsContext.current?.cgContext.clear(dirtyRect)
        let radius: CGFloat = waveformOnly ? 0 : look.cornerRadius
        let waveRect = waveform.map { convert($0.bounds, from: $0) }
        let waveShape = waveRect.map { NSBezierPath(roundedRect: $0, xRadius: radius, yRadius: radius) }
        if backgroundOpacity > 0 {
            let fill = NSBezierPath(rect: bounds)
            if let waveShape {
                fill.append(waveShape)
                fill.windingRule = .evenOdd
            }
            let panelBg = look.panelBackground?.nsColor ?? WaveformTheme.panelBackground
            panelBg.withAlphaComponent(backgroundOpacity).setFill()
            fill.fill()
            if let waveShape {
                look.plotBackground.nsColor.withAlphaComponent(backgroundOpacity).setFill()
                waveShape.fill()
            }
        }
        if showsOutline, let waveRect, let waveShape {
            // Package 8 / LOOK-08: draw border inside the waveform rect.
            let innerRect = waveRect.insetBy(dx: outlineWidth, dy: outlineWidth)
            let innerRadius = max(0, radius - outlineWidth)
            let outline = NSBezierPath()
            outline.append(waveShape)
            if innerRect.width > 0 && innerRect.height > 0 {
                outline.append(NSBezierPath(roundedRect: innerRect, xRadius: innerRadius, yRadius: innerRadius))
                outline.windingRule = .evenOdd
            }
            outlineColor.withAlphaComponent(outlineFollowsOpacity ? backgroundOpacity : 1).setFill()
            outline.fill()
        }
        if packageHovered {
            drawPackageDropHint(in: bounds, accentColor: dropAccentColor)
        }
    }
}

private enum MeterMode: UInt32, CaseIterable {
    case dbfs, vu, lufs

    var label: String {
        switch self {
        case .dbfs: "dBFS"
        case .vu: "VU"
        case .lufs: "LUFS"
        }
    }

    var detail: String {
        switch self {
        case .dbfs: "Peak level"
        case .vu: "VU level"
        case .lufs: "Momentary loudness"
        }
    }

    func value(from readings: BtMeterReadings) -> Float {
        switch self {
        case .dbfs: readings.peak_dbfs
        case .vu: readings.vu
        case .lufs: readings.momentary_lufs
        }
    }
}

private enum WideExportMode: UInt32, CaseIterable {
    case binauralFiles, monoFiles, multichannel, binauralMix

    var label: String {
        switch self {
        case .binauralFiles: "Binaural, one file per channel"
        case .monoFiles: "Mono, one file per channel"
        case .multichannel: "One multichannel file"
        case .binauralMix: "One binaural mix"
        }
    }
}

private enum ExportFolder: Int {
    case output
    case saved

    var key: String { self == .output ? "outputDirectory" : "savedDirectory" }
    var component: String { self == .output ? "output" : "saved" }
    var label: String { self == .output ? "Drag-and-drop folder" : "Saved snapshots folder" }
    var chooserTitle: String { self == .output ? "Select save folder" : "Select snapshot folder" }
}

@MainActor
private final class BacktrackingController: NSObject, NSApplicationDelegate, NSWindowDelegate, NSMenuDelegate,
    NSTextFieldDelegate {
    private let independentWindow = CommandLine.arguments.contains(receiverWindowArgument)
    #if BACKTRACKING_CAPTURE_VERIFY
    private var testCopy = TestCopyConfig.current.isTestCopy
    #else
    private let testCopy = TestCopyConfig.current.isTestCopy
    #endif
    /// Asks macOS for Input Monitoring; a verification replaces it so it never prompts.
    private var listenAccessRequest: () -> Void = {
        if !CGPreflightListenEventAccess() { _ = CGRequestListenEventAccess() }
    }
    private lazy var defaults: PreferencesStore = .forCurrentProcess(independentWindow: independentWindow)
    private lazy var startupUpdater = StartupUpdater(isTestCopy: testCopy)
    private var harness: TestHarness? = TestHarness()
    private var harnessTimer: Timer?
    private var activeSnapshotSaves = 0
    private struct PendingSnapshotSave {
        let range: Range<UInt64>?
        let fades: BtFades
        let rows: [RowIdentity]?
        let directory: String
        let ticket: UInt64?
        let controlCall: AddonControlCall?
    }
    private var pendingSnapshots: [PendingSnapshotSave] = []
    /// Saves that asked for their own freeze of the capture (Windows `pending_snapshot_reads`).
    private struct SnapshotRead {
        let read: EditingMaterialRead
        let save: PendingSnapshotSave
    }
    private var pendingSnapshotReads: [SnapshotRead] = []
    private var pendingLookControlsRebuild = false
    private var selectFreezeWaits: [(AddonControlCall, TimeInterval)] = []
    private var pendingAddonsTabReload = false
    private var pendingSourcesTabRebuild = false
    #if BACKTRACKING_CAPTURE_VERIFY
    /// Stand-ins for the Options window, which a verification does not open.
    private var verifyOptionsPage: String?
    private var verifyEditingText: Bool?
    private var verifyAddonsTabReloads = 0
    private var verifySourcesTabRebuilds = 0
    private var verifyLookControlRebuilds = 0
    #endif
    /// The window's editing freezes still in flight, oldest first; `pendingSnapshots` wait for them.
    private var editingFreezeReads: [EditingMaterialRead] = []
    private let clipLedger = ClipExportLedger()
    private var addonHost: AddonHostClient?
    private var addonPositionsCursor: UInt64 = 0
    private var lastAddonSnapshotForUI: AddonSnapshot = .empty
    private var shownChannelGroups: [String: String] = [:] // source.key -> group.id
    private var selectedLookAddonID: String?
    private var currentLook: ResolvedLook = .default
    private var lookAddonPopup: OptionsPopUpButton?
    private var lookAddonNote: NSTextField?
    private var lookBorderWidthNote: NSTextField?
    private var selectionByAddon: String?

    private enum AddonReviewStage {
        case reading(URL, note: String?)
        case refused(URL, reason: String, note: String?)
        case ready(URL, inspect: AddonInspectResult, installedVersion: String?, brokenInstalled: Bool, note: String?)
        case installing(URL, inspect: AddonInspectResult, installedVersion: String?, brokenInstalled: Bool, note: String?)
        case failed(URL, inspect: AddonInspectResult, installedVersion: String?, brokenInstalled: Bool, reason: String, note: String?)
    }
    private var addonReview: AddonReviewStage?
    private var addonRemoval: (id: String, name: String, deleteData: Bool, busy: Bool, error: String?)?
    private var addonPageNotice: String?
    private var addonProblems: [String: String] = [:]
    private var addonPendingDropChoice: (url: URL, openers: [(id: String, name: String)])?
    private var addonSeekingTargets: [String: (target: Double, at: TimeInterval)] = [:]
    private var addonPendingFormats: [String: (format: String, at: TimeInterval)] = [:]
    private var addonSourceProblems: [String: String] = [:]
    private var addonBusyRequests: Int = 0
    private var addonSettingDrafts: [String: AddonJSONValue] = [:]
    /// Enabled and control switches whose change the manager has not answered yet, with the value asked for.
    private var addonEnabledChanging: [String: Bool] = [:]
    private var addonControlChanging: [String: Bool] = [:]
    private var addonNumberSteps: [NSButton: (field: NSTextField, up: Bool)] = [:]
    /// The controls of each setting on the Add-ons page, by "addon:key".
    private var addonSettingControls: [String: [NSControl]] = [:]
    private var addonSettingTextFields: [NSTextField: (addonID: String, key: String, label: String)] = [:]
    private var addonSettingNumberFields: [NSTextField: (addonID: String, key: String, label: String, min: Double, max: Double, step: Double)] = [:]
    private var addonTransportTimeLabels: [Int: NSTextField] = [:]
    private var addonTransportPlayButtons: [Int: NSButton] = [:]
    private var addonTransportSliders: [Int: NSSlider] = [:]
    private var addonTransportSeekNotes: [Int: NSTextField] = [:]
    private var lastAddonTransportUIRefresh: TimeInterval = 0
    private var lastManualResize: TimeInterval = 0
    private var autoGrownBaseHeight: CGFloat?
    private var autoGrownTargetHeight: CGFloat?
    private var lastHarnessDragFiles: [URL] = []
    private var lastHarnessDragStatus: String = ""
    private lazy var snapshotShortcut = SnapshotShortcut { [weak self] in self?.dispatchSnapshotShortcut() }
    private var hotkeys = RecorderHotkeys()
    private var recordingShortcut: (action: RecorderAction, index: Int)?
    private var hotkeyErrorAction: RecorderAction?
    private var hotkeyErrorText: String?
    private weak var snapshotShortcutLabel: NSTextField?
    private var lastShortcutCheck = 0.0
    private var lastSnapshotTargetCheck = 0.0
    private var snapshotTargetID: String?
    private weak var snapshotTargetPopup: NSPopUpButton?
    private weak var shortcutPermissionButton: NSButton?
    private var optionTabs: NSTabView?
    private var optionCategoryButtons: [NSButton] = []
    private var optionCategory = 0
    private var optionsOpenedOnce = false
    private let worker = DispatchQueue(label: "backtracking.wav-export", qos: .userInitiated)
    private let sourceScanQueue = DispatchQueue(label: "Backtracking.source-scan", qos: .userInitiated)
    private var sourceScanInFlight = false
    private var sourceScanQueued = false
    private var cachedSourceChoices: [(source: CaptureSource, title: String)]?
    private var sourceScanError: String?
    private let pendingProvisionalWAVs = PendingProvisionalWAVs()
    private var tap: SystemAudioTap?
    private var panel: CapturePanel?
    private var optionsPanel: HotkeyOptionsPanel?
    private var statusItem: NSStatusItem?
    private var background: CaptureBackground?
    private var waveform: WaveformView?
    private var timer: Timer?
    private let captureButton = NSButton(title: "", target: nil, action: nil)
    private let captureLamp = CaptureLamp(frame: .zero)
    private let titleLabel = NSTextField(labelWithString: "Backtracking")
    private let clearButton = NSButton(title: "Clear capture", target: nil, action: nil)
    private let pinButton = NSButton(title: "Keep on top", target: nil, action: nil)
    private let metersStack = NSStackView(views: [])
    private var meterButtons: [NSButton] = []
    private var meterWidthConstraints: [NSLayoutConstraint] = []
    private var meterPopover: NSPopover?
    private var meterPopoverButtons: [NSButton] = []
    private var sourcePopups: [OptionsPopUpButton] = []
    private var routePopups: [NSPopUpButton] = []
    private var sourceProblemNotes: [NSTextField] = []
    private var waveformModePopup: NSPopUpButton?
    private let moreMetersButton = NSButton(title: "", target: nil, action: nil)
    private var addSourcePopup: OptionsPopUpButton?
    private weak var sourcesCapacityNote: NSTextField?
    private weak var sourcesFullNote: NSTextField?
    private weak var sourcesChannelsFullNote: NSTextField?
    private weak var captureCapacityNote: NSTextField?
    private var secondsIntervalPopup: NSPopUpButton?
    private var borderDependents: [NSControl] = []
    private var outlineColorButton: NSButton?
    private var cleanupNote: NSTextField?
    private let statusLabel = NSTextField(labelWithString: "")
    private var optionsCaptureButton: NSButton?
    private var optionsClearButton: NSButton?
    private var optionsSaveButton: NSButton?
    private var optionsCancelSelectionButton: NSButton?
    private var optionsPinButton: NSButton?
    private var optionsWaveformButton: NSButton?
    private var optionsLoginButton: NSButton?
    private var optionsVisibilityButton: NSButton?
    private var optionsMeterPopups: [NSPopUpButton] = []
    private var optionsNotice: NSTextField?
    private var outputPathLabel: NSTextField?
    private var savedPathLabel: NSTextField?
    private var toolbar: NSStackView?
    private var toolbarTrailing: NSLayoutConstraint?
    private var waveformMinimumHeight: NSLayoutConstraint?
    private var waveformTopBelowToolbar: NSLayoutConstraint?
    private var waveformTopWithoutToolbar: NSLayoutConstraint?
    private var waveformLeading: NSLayoutConstraint?
    private var waveformTrailing: NSLayoutConstraint?
    private var waveformBottomWithStatus: NSLayoutConstraint?
    private var waveformBottomWithoutStatus: NSLayoutConstraint?
    private var statusHeightConstraint: NSLayoutConstraint?
    private var fullPanelSize: NSSize?
    private var history: UInt32 = 20
    private var historyChangePending = false
    private var retainSilence = false
    private var idleSeconds: UInt32 = 10
    private var transparency: UInt32 = 0
    private var accentRGB = defaultAccentRGB
    private var frequencyColoring = false
    private var frequencyGradient = FrequencyGradient.standard
    private var hasCustomFrequencyGradient = false
    private var frequencyGradientControls: NSStackView?
    private weak var frequencyGradientPreview: FrequencyGradientPreview?
    private var frequencyStopPercentLabels: [Int: NSTextField] = [:]
    private weak var frequencyIllegibleNote: NSTextField?
    private var showsBars = false
    private var showsSeconds = false
    private var secondsInterval: UInt32 = 5
    private var showsOutline = false
    private var outlineWidth: UInt32 = 1
    private var customOutline = false
    private var outlineRGB = defaultAccentRGB
    private var outlineFollowsOpacity = false
    private var showsTitle = true
    private var showsMeters = true
    private var statusMark: StatusMark = .always
    // The drag-and-drop folder's housekeeping. Extra capture windows leave it to the main one.
    private var cleanup: OpaquePointer?
    private var meterModes: [MeterMode] = [.dbfs]
    private var captureLanes = [CaptureLane(source: .system)]
    private var dualSource: Bool { captureLanes.count > 1 }
    private var multipleRows: Bool {
        let width = tap?.channelCount(for: 0) ?? 0
        return dualSource || width > 2 || (stereoWaveform && width == 2)
    }
    /// The names the user gave sources for their files, by source key. A name belongs to the
    /// device, so it follows it to either lane. Capped at 64 most-recently-named entries (OPT-22).
    private var sourceFileNames: [String: String] = [:]
    private var sourceFileNameOrder: [String] = []
    private var fileNameFields: [NSTextField] = []
    /// The source each File name field shows the name of, so typing never names another one.
    private var fileNameSourceKeys: [String] = []
    private var fileNameNotes: [NSTextField] = []
    /// Select, preview, drag and save one source at a time while multiple sources record.
    private var separateLanes = false
    private var stereoWaveform = false
    private var microphonePromptPending = false
    private var lastSourceCheck = 0.0

    private var exportSampleRate: UInt32 = 48_000
    private var exportBitsPerSample: UInt32 = 32
    private var wideExportMode: WideExportMode = .binauralFiles
    private(set) var forceMonoExport = false
    private(set) var pairBinaural = false
    private(set) var hideSilentChannels = true
    private var optionsHideSilentButton: NSButton?
    private var alwaysOnTop = true
    private var waveformOnly = false
    private var paused = true
    private var captureError: String?
    private var editing = false
    private var lastClick = ProcessInfo.processInfo.systemUptime
    private var notice: String?

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let saved = unsignedSetting("history"), timeChoices.contains(saved) {
            history = saved
        }
        retainSilence = defaults.bool(forKey: "retainSilence")
        hotkeys = RecorderHotkeys(data: defaults.data(forKey: "recorderHotkeys"))
        snapshotShortcut.bindings = hotkeys[.globalSnapshot]
        // The old idleSeconds key stored 0 for manual mode.
        if let seconds = unsignedSetting(autoResumePreferenceKey), autoResumeChoices.contains(seconds) {
            idleSeconds = seconds
        }
        if let saved = unsignedSetting("backgroundTransparency"), transparencyChoices.contains(saved) {
            transparency = saved
        }
        if let saved = unsignedSetting("accentRGB"), saved <= 0xFFFFFF {
            accentRGB = saved
        }
        frequencyColoring = defaults.bool(forKey: "waveformColorByFrequency")
        if let settings = defaults.array(forKey: "waveformFrequencyGradient") as? [[String: Any]],
           let saved = FrequencyGradient(settings: settings) {
            frequencyGradient = saved
            hasCustomFrequencyGradient = true
        }
        showsBars = defaults.bool(forKey: "showsBars")
        showsSeconds = defaults.bool(forKey: "showsSeconds")
        if let saved = unsignedSetting("secondsInterval"), secondsLineChoices.contains(saved) {
            secondsInterval = saved
        }
        showsOutline = defaults.bool(forKey: "showsWaveformOutline")
        customOutline = defaults.bool(forKey: "customWaveformOutline")
        if let saved = unsignedSetting("waveformOutlineRGB"), saved <= 0xFFFFFF { outlineRGB = saved }
        if let saved = unsignedSetting("waveformOutlineWidth"), borderWidthChoices.contains(saved) {
            outlineWidth = saved
        }
        outlineFollowsOpacity = (defaults.object(forKey: "waveformOutlineFollowsOpacity") as? Bool) ?? false
        showsTitle = (defaults.object(forKey: "showsTitle") as? Bool) ?? true
        showsMeters = (defaults.object(forKey: "showsMeters") as? Bool) ?? true
        if let saved = defaults.string(forKey: "statusMark"), let mark = StatusMark(rawValue: saved) {
            statusMark = mark
        }
        if !independentWindow {
            // Anything but an offered age, including a missing setting, is Off.
            cleanup = bt_cleanup_new(unsignedSetting("outputCleanupDays") ?? 0)
        }
        snapshotTargetID = defaults.string(forKey: "snapshotTargetID")
        if let width = defaults.object(forKey: "fullPanelWidth") as? NSNumber,
           let height = defaults.object(forKey: "fullPanelHeight") as? NSNumber,
           width.doubleValue.isFinite, height.doubleValue.isFinite,
           CGFloat(width.doubleValue) >= fullPanelMinimum.width,
           CGFloat(height.doubleValue) >= fullPanelMinimum.height {
            fullPanelSize = NSSize(width: CGFloat(width.doubleValue), height: CGFloat(height.doubleValue))
        }
        if let saved = unsignedSetting("exportSampleRate"), exportSampleRates.contains(saved) {
            exportSampleRate = saved
        }
        if let saved = unsignedSetting("exportBitsPerSample"), exportBitDepths.contains(saved) {
            exportBitsPerSample = saved
        }
        forceMonoExport = defaults.bool(forKey: "forceMonoExport")
        pairBinaural = defaults.bool(forKey: "pairBinaural")
        // Default on: only explicit false (0) turns Hide silent channels off.
        hideSilentChannels = (defaults.object(forKey: "hideSilentChannels") as? Bool) ?? true
        if let raw = unsignedSetting("wideExportMode"), let mode = WideExportMode(rawValue: raw) { wideExportMode = mode }
        alwaysOnTop = (defaults.object(forKey: "alwaysOnTop") as? Bool) ?? true
        waveformOnly = (defaults.object(forKey: "waveformOnly") as? Bool) ?? false
        if let savedGroups = defaults.dictionary(forKey: "shownChannelGroups") as? [String: String] {
            shownChannelGroups = savedGroups
        }
        if let savedLook = defaults.string(forKey: "selectedLookAddon"), !savedLook.isEmpty {
            selectedLookAddonID = savedLook
        }
        let appVersion = (Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String) ?? "0.2.1"
        let addonRoot = TestCopyConfig.current.addonDataRoot.path
        try? FileManager.default.createDirectory(atPath: addonRoot, withIntermediateDirectories: true)
        attach(addonHost: AddonHostClient(dataRoot: addonRoot, appVersion: appVersion, primary: !independentWindow))
        recomputeLook(refreshAppearanceTab: false)
        if independentWindow || testCopy { beginCaptureAfterUpdate() }
        else { startupUpdater.checkAtLaunch { [weak self] in self?.beginCaptureAfterUpdate() } }
    }

    #if BACKTRACKING_CAPTURE_VERIFY
    static func verifyOptions() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-opts-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        let fixture = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        controller.defaults = fixture
        fixture.set("system", forKey: "captureSourceA")
        fixture.set("mic:fixture", forKey: "captureSourceB")
        fixture.set(Int(CaptureRoute.left.rawValue), forKey: "captureRouteB")
        fixture.set(true, forKey: "dualCaptureSource")
        fixture.set(Int(MeterMode.vu.rawValue), forKey: "meterModeB")
        controller.loadCapturePreferences()
        guard controller.captureLanes == [CaptureLane(source: .system), CaptureLane(source: .microphone("fixture"), route: .left)],
              controller.meterModes == [.dbfs, .vu] else { throw CaptureError("Legacy A/B settings were lost") }
        fixture.set([
            ["source": "system", "route": 0], ["source": "mic:fixture", "route": 1],
            ["source": "sender:abc", "route": 6]
        ], forKey: "captureLanes")
        fixture.set([0, 1, 2], forKey: "meterModes")
        controller.loadCapturePreferences()
        let sources = controller.makeSourceTab()
        let appearance = controller.makeAppearanceTab()
        let hotkeys = controller.makeHotkeysTab()
        let tabs = NSTabView(frame: NSRect(x: 0, y: 0, width: 600, height: 360))
        tabs.addTabViewItem(sources)
        tabs.addTabViewItem(appearance)
        tabs.addTabViewItem(hotkeys)
        tabs.selectTabViewItem(sources)
        controller.optionTabs = tabs
        let replacement = controller.makeSourceTab()
        controller.replaceOptionsTab(with: replacement)
        let rows = WaveformRow.layout(widths: [1, 2, 3], labels: [[], [], ["FL", "FR", "FC"]], stereo: false)
        guard tabs.selectedTabViewItem === replacement, replacement.view?.superview != nil,
              tabs.tabViewItems.map(\.label) == ["Sources", "Appearance", "Hotkeys"],
              controller.sourcePopups.count == 3, controller.addSourcePopup != nil,
              controller.optionsMeterPopups.count == 3, controller.meterModes == [.dbfs, .vu, .lufs],
              rows.map(\.view) == [0, 0, 67, 68, 3, 4, 5], rows.last?.title == "C · FC" else {
            throw CaptureError("The Options or waveform rows do not match the loaded sources")
        }
    }

    /// The grant check before each control request can read a newer add-on snapshot than the window
    /// shows. The window must catch up after the batch, and stay caught up when the next poll finds
    /// nothing new; a completion must see the snapshot that carried it.
    static func verifyControlBatchSnapshotSync() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-batch-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        let root = scratch.appendingPathComponent("addons-root", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        guard let host = AddonHostClient(dataRoot: root.path, appVersion: "0.2.1", primary: true) else {
            throw CaptureError("AddonHostClient failed to start")
        }
        controller.attach(addonHost: host)
        defer { host.shutdownAndFree(); controller.addonHost = nil }
        func pump(for seconds: Double, until done: () -> Bool = { false }) {
            let deadline = ProcessInfo.processInfo.systemUptime + seconds
            while !done() && ProcessInfo.processInfo.systemUptime < deadline {
                controller.pollAddonHost()
                Thread.sleep(forTimeInterval: 0.01)
            }
        }
        func waitForHost(past generation: UInt64) -> Bool {
            let deadline = ProcessInfo.processInfo.systemUptime + 2
            while (host.verifyHostGeneration ?? generation) == generation && ProcessInfo.processInfo.systemUptime < deadline {
                Thread.sleep(forTimeInterval: 0.002)
            }
            return (host.verifyHostGeneration ?? generation) != generation
        }
        pump(for: 3) { host.snapshot.ready }
        pump(for: 0.3)
        guard host.snapshot.ready, let quiet = host.verifyHostGeneration else { throw CaptureError("Add-on host did not become ready") }
        let command = AddonControlCommand(command: "get_state", from_seconds_ago: nil, to_seconds_ago: nil,
            start_frame: nil, end_frame: nil, lane: nil, first: nil, last: nil, seconds: nil, folder: nil, source: nil)
        let call = AddonControlCall(addon_id: "verify.batch", addon_name: "Verify", run: 1, request_id: "1", command: command)
        var problems: [String] = []

        // A command finishes while the window is idle; the grant check of the next batch reads it.
        var seenByCompletion: UInt64?
        var carrying: UInt64?
        host.inspect(path: scratch.appendingPathComponent("missing-1.btaddon").path) { _ in
            seenByCompletion = currentAddonSnapshot.generation
            carrying = host.snapshot.generation
        }
        guard waitForHost(past: quiet) else { throw CaptureError("The inspect command never finished") }
        controller.executeControlCalls([call])
        if seenByCompletion == nil || seenByCompletion != carrying {
            problems.append("completion saw snapshot \(seenByCompletion.map(String.init) ?? "none"), carried by \(carrying.map(String.init) ?? "none")")
        }
        if controller.lastAddonSnapshotForUI.generation != host.snapshot.generation {
            problems.append("after the batch the window shows \(controller.lastAddonSnapshotForUI.generation), host snapshot \(host.snapshot.generation)")
        }

        // The poll takes a result, which advances the host once more; the batch's check reads that,
        // and the poll after it finds nothing new.
        pump(for: 0.3)
        var done = false
        host.inspect(path: scratch.appendingPathComponent("missing-2.btaddon").path) { _ in done = true }
        guard let before = host.verifyHostGeneration, waitForHost(past: before) else { throw CaptureError("The second inspect never finished") }
        controller.pollAddonHost()
        guard done else { throw CaptureError("The second inspect's completion did not run") }
        controller.executeControlCalls([call])
        controller.pollAddonHost()
        if controller.lastAddonSnapshotForUI.generation != host.snapshot.generation
            || host.snapshot.generation != host.verifyHostGeneration {
            problems.append("a quiet poll left the window at \(controller.lastAddonSnapshotForUI.generation), host snapshot \(host.snapshot.generation), host \(host.verifyHostGeneration ?? 0)")
        }
        guard problems.isEmpty else {
            throw CaptureError("Control batch left the window behind the add-on host: " + problems.joined(separator: "; "))
        }
    }

    /// Edit separately with three of six rows selected: moving the start flag sideways keeps those
    /// rows (Windows starts row tracking only for a new selection).
    static func verifyFlagDragKeepsRows() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-flag-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        let view = WaveformView(frame: NSRect(x: 0, y: 0, width: 600, height: 300))
        view.controller = controller
        controller.waveform = view
        defer { controller.waveform = nil }
        let widths: [UInt32] = [2, 3]
        view.verifyShow(rows: WaveformRow.layout(widths: widths, labels: [[], ["FL", "FR", "FC"]], stereo: false),
                        laneWidths: widths, frames: 0..<48_000)
        view.separateLanes = true
        guard view.selectProgrammaticRows(firstLane: 0, firstRow: 0, lastLane: 1, lastRow: 1) else {
            throw CaptureError("Flag verification could not choose rows")
        }
        view.applyProgrammaticSelection(12_000..<36_000)
        let chosen = view.selectedRowsForExport
        guard let chosen, chosen.count == 3, let press = view.verifyFlagPoint else {
            throw CaptureError("Flag verification setup chose \(view.selectedRowsForExport ?? []) rows")
        }
        func mouse(_ type: NSEvent.EventType, _ point: NSPoint, _ modifiers: NSEvent.ModifierFlags = []) -> NSEvent {
            NSEvent.mouseEvent(with: type, location: point, modifierFlags: modifiers, timestamp: 0, windowNumber: 0,
                               context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
        }
        view.mouseDown(with: mouse(.leftMouseDown, press))
        // Sideways and down into a channel row of lane B, as a hand does while moving a flag.
        let moved = NSPoint(x: press.x + 40, y: 20)
        view.mouseDragged(with: mouse(.leftMouseDragged, NSPoint(x: press.x + 20, y: press.y)))
        let sideways = view.selectedRowsForExport
        view.mouseDragged(with: mouse(.leftMouseDragged, moved))
        view.mouseUp(with: mouse(.leftMouseUp, moved))
        guard sideways == chosen, view.selectedRowsForExport == chosen, view.selectionForSaving()?.lowerBound != 12_000 else {
            throw CaptureError("Moving the start flag changed the rows from \(chosen) to \(view.selectedRowsForExport ?? view.verifyRememberedRows) (selection \(String(describing: view.selectionForSaving())))")
        }
        // Selections the user makes, moves or drops are no longer the add-on's.
        controller.selectionByAddon = "Verify"
        let flag = view.verifyFlagPoint ?? press
        view.mouseDown(with: mouse(.leftMouseDown, flag))
        view.mouseDragged(with: mouse(.leftMouseDragged, NSPoint(x: flag.x + 20, y: flag.y)))
        view.mouseUp(with: mouse(.leftMouseUp, NSPoint(x: flag.x + 20, y: flag.y)))
        let afterFlag = controller.selectionByAddon
        guard view.hasSelection else { throw CaptureError("The flag drag dropped the selection") }
        controller.selectionByAddon = "Verify"
        view.clearSelection()
        let afterClear = controller.selectionByAddon
        controller.selectionByAddon = "Verify"
        view.mouseDown(with: mouse(.leftMouseDown, NSPoint(x: 300, y: 295), .option))
        view.mouseUp(with: mouse(.leftMouseUp, NSPoint(x: 300, y: 295), .option))
        guard afterFlag == nil, afterClear == nil, controller.selectionByAddon == nil else {
            throw CaptureError("The status still names the add-on after a flag drag (\(afterFlag ?? "-")), a cleared selection (\(afterClear ?? "-")) or a new selection (\(controller.selectionByAddon ?? "-"))")
        }
        view.applyProgrammaticSelection(12_000..<36_000)
        // A new selection (Option-drag) still takes the rows it crosses.
        let start = NSPoint(x: 300, y: 295)
        view.mouseDown(with: mouse(.leftMouseDown, start, .option))
        view.mouseDragged(with: mouse(.leftMouseDragged, NSPoint(x: 360, y: 20), .option))
        view.mouseUp(with: mouse(.leftMouseUp, NSPoint(x: 360, y: 20), .option))
        guard let crossed = view.selectedRowsForExport ?? (view.hasSelection ? view.verifyRememberedRows : nil),
              crossed.count == 6 else {
            throw CaptureError("A new selection across rows took \(view.selectedRowsForExport ?? []) rows")
        }
    }

    /// A selection dragged from source A into stacked source B and past the plot's bottom scrolls
    /// B, the lane the drag is in now, and takes the row that comes into view (Windows `row_drag`).
    static func verifyCrossSourceEdgeScroll() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-edge-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        let view = WaveformView(frame: NSRect(x: 0, y: 0, width: 600, height: 120))
        view.controller = controller
        controller.waveform = view
        defer { controller.waveform = nil }
        let widths: [UInt32] = [2, 8]
        let labels = ["FL", "FR", "FC", "LFE", "Ls", "Rs", "Lb", "Rb"]
        view.verifyShow(rows: WaveformRow.layout(widths: widths, labels: [[], labels], stereo: false),
                        laneWidths: widths, frames: 0..<48_000)
        view.separateLanes = true
        let boxes = view.verifyLaneBoxes
        guard boxes.count == 2, !boxes[0].scrolls, boxes[1].scrolls else {
            throw CaptureError("Edge scroll setup: lanes \(boxes)")
        }
        func mouse(_ type: NSEvent.EventType, _ point: NSPoint) -> NSEvent {
            NSEvent.mouseEvent(with: type, location: point, modifierFlags: [.option], timestamp: 0, windowNumber: 0,
                               context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
        }
        let aY = 120 - (boxes[0].top + boxes[0].height / 2)
        let bY = 120 - (boxes[1].top + boxes[1].height / 2)
        view.mouseDown(with: mouse(.leftMouseDown, NSPoint(x: 200, y: aY)))
        view.mouseDragged(with: mouse(.leftMouseDragged, NSPoint(x: 260, y: bY)))
        view.mouseDragged(with: mouse(.leftMouseDragged, NSPoint(x: 280, y: -10)))
        let first = view.verifyLaneFirstRows
        let rows = view.verifyRememberedRows
        view.verifyStopEdgeScroll()
        view.mouseUp(with: mouse(.leftMouseUp, NSPoint(x: 280, y: 60)))
        let lastShown = first.count == 2 ? first[1] + boxes[1].shown - 1 : -1
        let entered = lastShown >= 2 ? RowIdentity.channel(lane: 1, index: lastShown - 2) : nil
        guard first.count == 2, first[1] == 1, let entered, rows.last == entered, rows.first == .whole(lane: 0) else {
            throw CaptureError("Dragging past B's bottom scrolled lanes to \(first) and chose \(rows)")
        }
    }

    /// A lane past 64 channels holds none of them, and says why it cannot start in Windows' words.
    static func verifyChannelLimit() throws {
        let used = SystemAudioTap.channelsInUse(widths: [32, 30, 1], capErrors: [nil, nil, "Lane C cannot start"])
        let laneB = SystemAudioTap.overLimitMessage(lane: 1, total: 65)
        let laneC = SystemAudioTap.overLimitMessage(lane: 2, total: 68)
        guard used == 62,
              laneB == "Lane B cannot start: lanes A and B would contain 65 channels; the limit is 64 / 來源 B 無法開始：來源 A 與 B 合計會有 65 個聲道，上限為 64。",
              laneC == "Lane C cannot start: lanes A to C would contain 68 channels; the limit is 64 / 來源 C 無法開始：來源 A 到 C 合計會有 68 個聲道，上限為 64。" else {
            throw CaptureError("Over the channel limit: \(used) in use; lane B says \(laneB); lane C says \(laneC)")
        }
    }

    /// The gradient bar on the Appearance page reads against the chosen look's plot, as the
    /// waveform does; a window that opens with a look's gradient draws that gradient at once.
    static func verifyLookGradient() throws {
        // #474747 reads on black (2.3:1) but not on the window's own plot (1.9:1).
        let light = AddonAppearance(colors: ["plot_background": "#000000", "panel_background": "#000000"],
                                    numbers: [:], rows: [:], spectrum: [
            AddonSpectrumStop(color: "#474747", position: 0.0),
            AddonSpectrumStop(color: "#2c68ec", position: 1.0),
        ])
        let look = ResolvedLook.from(appearance: light, accentRGB: WaveformTheme.defaultAccentRGB)
        let preview = FrequencyGradientPreview(frame: NSRect(x: 0, y: 0, width: 200, height: 16))
        guard let offered = ResolvedLook.spectrum(from: light) else { throw CaptureError("The look's gradient did not parse") }
        preview.gradient = offered
        preview.look = look
        let expected = look.legibleSpectrum(for: offered).gradient
        guard expected == offered, offered.readableForDrawing() != offered, preview.drawnGradient == expected else {
            throw CaptureError("The gradient bar draws \(preview.drawnGradient.stops) on the look's plot \(look.plotBackground), the waveform \(expected.stops)")
        }
        let saved = currentAddonSnapshot
        defer { currentAddonSnapshot = saved }
        currentAddonSnapshot = AddonSnapshot(schema: 1, generation: 1, ready: true, error: nil, addons: [],
                                             appearances: ["verify.look": light], completed: [])
        let controller = BacktrackingController()
        controller.selectedLookAddonID = "verify.look"
        controller.frequencyColoring = true
        controller.frequencyGradient = .standard
        controller.hasCustomFrequencyGradient = false
        // What `configurePanel` gives a new window's waveform; no window is made here.
        let opened = controller.openingWaveformGradient
        guard opened == offered else {
            throw CaptureError("A window opened with the look's gradient draws \(opened?.stops ?? [])")
        }
    }

    /// More lanes than the 64 channels can hold, as when sources turn out wider than estimated:
    /// the capture still opens, and only the lanes that do not fit say why.
    static func verifyChannelLimitCaptureOpens() throws {
        let lanes = (0..<33).map { CaptureLane(source: .addon(addonID: "verify.limit", sourceID: "s\($0)"), route: .merged) }
        let tap = try SystemAudioTap(history: 10, lanes: lanes)
        let refused = lanes.indices.filter { tap.sourceProblem(for: $0)?.contains("cannot start") == true }
        guard refused == [31, 32], tap.totalChannelCount == 62,
              tap.sourceProblem(for: 32)?.hasPrefix("Lane AG cannot start: lanes A to AG would contain 66 channels; the limit is 64") == true else {
            throw CaptureError("33 stereo lanes: refused \(refused), \(tap.totalChannelCount) channels in use")
        }
    }

    /// The files of an add-on's save name the lane each came from, from the export itself.
    static func verifySavedFileLanes() throws {
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-lanes-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        let lanes = [
            CaptureLane(source: .addon(addonID: "verify.lanes", sourceID: "a"), route: .merged),
            CaptureLane(source: .addon(addonID: "verify.lanes", sourceID: "b"), route: .merged)
        ]
        let tap = try SystemAudioTap(history: 10, lanes: lanes)
        let samples = [Float](repeating: 0.25, count: 4_800 * 2)
        for lane in 0..<2 {
            _ = tap.withCaptureRead { core in
                samples.withUnsafeBufferPointer {
                    bt_tracks_push_interleaved(core, UInt32(lane), $0.baseAddress, 4_800, 1_000_000_000, SystemAudioTap.timelineRate)
                }
            }
        }
        let routes = tap.captureRoutes
        let labels = tap.captureChannelNames
        guard let frozen = tap.withCaptureSnapshot({
            WAVExport(core: $0, routes: routes, channelNames: labels, channelGroups: [[], []], positions: [:])
        }) else { throw CaptureError("Lane verification could not freeze") }
        let written = frozen.write(range: frozen.span, directory: scratch.path, sampleRate: 48_000, bitsPerSample: 32,
            forceMono: false, fades: noFades, names: ["Lanes", "Lanes"], selectedRows: nil, hideSilentLanes: [],
            wideMode: .monoFiles, pairBinaural: false)
        let files = BacktrackingController.savedFiles(urls: written.urls, lanes: written.lanes)
        let named = files.map { file -> Bool in
            guard let lane = file.lane else { return false }
            return URL(fileURLWithPath: file.path).lastPathComponent.contains("_\(lane)_")
        }
        guard files.count == 2, Set(files.compactMap(\.lane)) == ["A", "B"], !named.contains(false) else {
            throw CaptureError("Saved files carry lanes \(files.map { "\($0.lane ?? "-") \(URL(fileURLWithPath: $0.path).lastPathComponent)" })")
        }
    }

    /// Rows chosen in one channel group keep their identity when another group is shown, and are
    /// still what is written, named by their own group; a lane held whole takes a row that starts
    /// sounding later, and is written whole (Windows `carry_rows`, `lanes_chosen_whole`, `lane_write`).
    static func verifyRowsAcrossGroupsAndActivity() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-groups-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        let bed = AddonChannelGroup(id: "bed", label: "Bed", channels: ["FL", "FR", "FC"], positioned: false)
        let objects = AddonChannelGroup(id: "objects", label: "Objects", channels: ["Object 1", "Object 2"], positioned: true)
        let labels = ["FL", "FR", "FC", "Object 1", "Object 2"]
        func rows(_ shown: AddonChannelGroup, _ range: Range<Int>) -> [WaveformRow] {
            WaveformRow.layout(widths: [5, 2], labels: [labels, []], stereo: false,
                               shownGroups: [(group: shown, range: range, hasBed: true), nil])
        }
        let view = WaveformView(frame: NSRect(x: 0, y: 0, width: 600, height: 400))
        view.controller = controller
        controller.waveform = view
        defer { controller.waveform = nil }
        view.verifyShow(rows: rows(objects, 3..<5), laneWidths: [5, 2], frames: 0..<48_000)
        view.verifyShowRows(rows(objects, 3..<5), groups: ["objects", nil])
        view.separateLanes = true
        // Object 1 alone, chosen while Objects is on show.
        guard view.selectProgrammaticRows(firstLane: 0, firstRow: 2, lastLane: 0, lastRow: 2) else {
            throw CaptureError("Group verification could not choose Object 1")
        }
        view.applyProgrammaticSelection(12_000..<36_000)
        let chosen = view.selectedRowsForExport
        view.onChannelGroupSwitched(lane: 0)
        view.verifyShowRows(rows(bed, 0..<3), groups: ["bed", nil])
        let afterSwitch = view.selectedRowsForExport
        var problems: [String] = []
        if chosen != [.channel(lane: 0, index: 3)] || afterSwitch != chosen {
            problems.append("chosen \(chosen ?? []), after showing Bed \(afterSwitch ?? [])")
        }

        // The export of those rows, with Bed on show, writes Object 1 named by its own group.
        let widths: [UInt32] = [5, 2]
        guard let core = widths.withUnsafeBufferPointer({
            bt_tracks_new_lanes(SystemAudioTap.timelineRate, $0.baseAddress, $0.count, 10, ProcessInfo.processInfo.physicalMemory)
        }) else { throw CaptureError("Group verification could not make a capture") }
        defer { bt_tracks_free(core) }
        bt_tracks_set_retain_silence(core, 1)
        let wide = [Float](repeating: 0.25, count: 4_800 * 5)
        let stereo = [Float](repeating: 0.2, count: 4_800 * 2)
        _ = wide.withUnsafeBufferPointer { bt_tracks_push_interleaved(core, 0, $0.baseAddress, 4_800, 1_000_000_000, SystemAudioTap.timelineRate) }
        _ = stereo.withUnsafeBufferPointer { bt_tracks_push_interleaved(core, 1, $0.baseAddress, 4_800, 1_000_000_000, SystemAudioTap.timelineRate) }
        bt_tracks_pause(core)
        guard let frozen = WAVExport(core: core, routes: [.allChannels, .merged], channelNames: [labels, []],
                                     channelGroups: [[bed, objects], []]) else {
            throw CaptureError("Group verification could not freeze")
        }
        let folder = scratch.appendingPathComponent("rows", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let written = frozen.write(range: frozen.span, directory: folder.path, sampleRate: 48_000, bitsPerSample: 32,
            forceMono: false, fades: noFades, names: ["Groups", "Other"], selectedRows: afterSwitch, hideSilentLanes: [],
            wideMode: .monoFiles, pairBinaural: false, shownGroupIDs: ["bed", nil])
        let names = written.urls.map(\.lastPathComponent)
        if written.error != nil || names.filter({ $0.hasSuffix(".wav") }).count != 1 || !names.contains(where: { $0.contains("Object 1") }) {
            problems.append("export of Object 1 with Bed on show wrote \(names) \(written.error ?? "")")
        }
        let slot = bt_preview_slot_new()
        defer { if let slot { bt_preview_slot_clear(slot); bt_preview_slot_free(slot) } }
        if let slot, let failure = frozen.play(in: slot, range: frozen.span, forceMono: false, fades: noFades,
                                               selectedRows: afterSwitch, hideSilentLanes: [], pairBinaural: false,
                                               shownGroupIDs: ["bed", nil]) {
            problems.append("preview of Object 1 with Bed on show failed: \(failure)")
        }

        // Lane B held whole, then lane A held whole as a channel starts sounding.
        controller.waveform = nil
        let quiet = WaveformRow.layout(widths: [6, 2], labels: [["FL", "FR", "FC", "LFE", "Ls", "Rs"], []], stereo: false,
                                       activeMasks: [0b0000_0111, nil])
        let louder = WaveformRow.layout(widths: [6, 2], labels: [["FL", "FR", "FC", "LFE", "Ls", "Rs"], []], stereo: false,
                                        activeMasks: [0b0001_0111, nil])
        let live = WaveformView(frame: NSRect(x: 0, y: 0, width: 600, height: 400))
        live.verifyShow(rows: quiet, laneWidths: [6, 2], frames: 0..<48_000)
        live.verifyShowRows(quiet, groups: [nil, nil])
        live.separateLanes = true
        guard live.selectProgrammaticLane(0) else { throw CaptureError("Could not choose lane A") }
        live.applyProgrammaticSelection(12_000..<36_000)
        live.verifyShowRows(louder, groups: [nil, nil])
        let sounding = RowIdentity.channel(lane: 0, index: 4)
        if live.verifyWholeLanes != [0] || !live.verifyRememberedRows.contains(sounding)
            || live.selectedRowsForExport != [.whole(lane: 0)] {
            problems.append("lane A held whole: whole \(live.verifyWholeLanes), rows \(live.verifyRememberedRows), export \(live.selectedRowsForExport ?? [])")
        }
        guard problems.isEmpty else { throw CaptureError("Rows across groups: " + problems.joined(separator: "; ")) }
    }

    /// Hovering a group or transport chip shows what it does, and the compact transport keeps
    /// Windows' plot-width and lane-height limits.
    static func verifyChipToolTipsAndTransportLimits() throws {
        let view = WaveformView(frame: NSRect(x: 0, y: 0, width: 600, height: 300))
        let expected = view.verifyShowChips()
        let shown = expected.map { view.verifyChipToolTip(at: $0.0) }
        guard view.verifyChipToolTipTags.count == expected.count, shown == expected.map(\.1) else {
            throw CaptureError("Chip tooltips: \(view.verifyChipToolTipTags.count) registered, hover shows \(shown)")
        }
        let fits = WaveformView.transportChipFits
        guard fits(240, 22, 300, 304), !fits(239, 22, 100, 304), !fits(400, 21.5, 100, 404),
              !fits(400, 40, 301, 304), fits(400, 40, 300, 304) else {
            throw CaptureError("The compact transport ignores Windows' 240 px plot / 22 px lane / 4 px edge limits")
        }
    }

    /// A save at 44.1 kHz tells the add-ons the rate and frame count its WAV files really have.
    /// It goes through `saveSnapshot` and the main queue, so it runs after the other checks.
    static func verifyClipExportedAtExportRate() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-clip-rate-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        let root = scratch.appendingPathComponent("addons-root", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        guard let host = AddonHostClient(dataRoot: root.path, appVersion: "0.2.1", primary: true) else {
            throw CaptureError("AddonHostClient failed to start")
        }
        controller.attach(addonHost: host)
        defer { host.shutdownAndFree(); controller.addonHost = nil }
        func spin(_ seconds: Double, until done: () -> Bool) {
            let deadline = ProcessInfo.processInfo.systemUptime + seconds
            while !done() && ProcessInfo.processInfo.systemUptime < deadline {
                RunLoop.main.run(until: Date().addingTimeInterval(0.01))
                host.pollSnapshot()
            }
        }
        spin(3) { host.snapshot.ready }
        let lanes = [CaptureLane(source: .addon(addonID: "verify.rate", sourceID: "a"), route: .merged)]
        let tap = try SystemAudioTap(history: 10, lanes: lanes)
        controller.captureLanes = lanes
        controller.tap = tap
        defer { controller.tap = nil }
        let frames = 4_800
        let samples = [Float](repeating: 0.25, count: frames * 2)
        let accepted = tap.withCaptureRead { core in
            samples.withUnsafeBufferPointer {
                bt_tracks_push_interleaved(core, 0, $0.baseAddress, UInt32(frames), 1_000_000_000, SystemAudioTap.timelineRate)
            }
        }
        guard accepted == UInt32(frames) else { throw CaptureError("Clip rate verification lost input") }
        controller.exportSampleRate = 44_100
        let folder = scratch.appendingPathComponent("saved", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        controller.saveSnapshot(into: folder.path)
        spin(5) { host.verifyEvents.contains("clip_exported answered") }
        let wavs = try FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "wav" }
        guard let url = wavs.first, wavs.count == 1 else {
            throw CaptureError("The 44.1 kHz save wrote \(wavs.count) WAV files")
        }
        let file = try AVAudioFile(forReading: url)
        let expected = "clip_exported 44100 \(file.length) 1"
        let sent = host.verifyEvents.filter { $0.hasPrefix("clip_exported ") && $0 != "clip_exported answered" }
        guard file.fileFormat.sampleRate == 44_100, sent == [expected] else {
            throw CaptureError("A \(file.fileFormat.sampleRate) Hz WAV of \(file.length) frames was announced as \(sent), expected [\(expected)]")
        }
    }

    /// An add-on's pause_capture holds for the auto-resume delay, as a pause by hand does.
    static func verifyControlPauseHolds() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-pause-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        let lanes = [CaptureLane(source: .addon(addonID: "verify.pause", sourceID: "a"), route: .merged)]
        controller.captureLanes = lanes
        controller.tap = try SystemAudioTap(history: 10, lanes: lanes)
        defer { controller.tap = nil }
        controller.paused = false
        controller.idleSeconds = 5
        controller.lastClick = ProcessInfo.processInfo.systemUptime - 60 // nobody has touched the window for a minute
        guard case .success = controller.runWindowAction(.pauseCapture) else { throw CaptureError("pause_capture was refused") }
        let soon = ProcessInfo.processInfo.systemUptime + 0.4
        guard controller.paused, !controller.shouldAutoResume(now: soon),
              controller.shouldAutoResume(now: soon + 5) else {
            throw CaptureError("An add-on's pause resumes on its own after 0.4 s (paused \(controller.paused))")
        }
    }

    /// Snapshots reach the Options pages through `syncAddonSnapshotUI`. A change that arrives while
    /// the Add-ons page is being typed into reloads it exactly once, after the typing ends.
    static func verifySnapshotRebuilds() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-rebuilds-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        guard let host = AddonHostClient(dataRoot: scratch.path, appVersion: "0.2.1", primary: true) else {
            throw CaptureError("AddonHostClient failed to start")
        }
        controller.attach(addonHost: host)
        defer { host.shutdownAndFree(); controller.addonHost = nil }
        var generation: UInt64 = 1_000
        func receive(enabled: Bool) {
            generation += 1
            let addon = AddonRecord(id: "verify.page", manifest: nil, enabled: enabled, state: "stopped", reason: nil,
                pid: nil, message: nil, sources: [], settings: [], actions: [], control_allowed: false, control_asked: false)
            host.verifyReceive(AddonSnapshot(schema: 1, generation: generation, ready: true, error: nil,
                                             addons: [addon], appearances: [:], completed: []))
            controller.syncAddonSnapshotUI()
        }
        controller.verifyOptionsPage = "Add-ons"
        receive(enabled: false)
        controller.verifyAddonsTabReloads = 0
        controller.verifyEditingText = true
        receive(enabled: true)
        receive(enabled: false)
        controller.syncAddonSnapshotUI() // the next tick, still typing
        let whileTyping = controller.verifyAddonsTabReloads
        controller.verifyEditingText = false
        controller.syncAddonSnapshotUI()
        controller.syncAddonSnapshotUI()
        guard whileTyping == 0, controller.verifyAddonsTabReloads == 1, !controller.pendingAddonsTabReload else {
            throw CaptureError("Add-ons page reloaded \(whileTyping) times while typing and \(controller.verifyAddonsTabReloads) after")
        }

        // An Options list (a popover) is open: neither the Add-ons nor the Sources page is rebuilt
        // under it, nor the look controls; once it closes, the next pass rebuilds each once.
        OptionsPopUpButton.verifyShown = true
        defer { OptionsPopUpButton.verifyShown = nil }
        controller.verifyAddonsTabReloads = 0
        receive(enabled: true)
        let reloadsUnderList = controller.verifyAddonsTabReloads
        let addonsPending = controller.pendingAddonsTabReload
        controller.verifyOptionsPage = "Sources"
        func receiveSource(_ format: String) {
            generation += 1
            let source = AddonSource(id: "main", name: "Main", sender: "s", formats: [], format: format, groups: nil, transport: nil)
            let addon = AddonRecord(id: "verify.page", manifest: nil, enabled: true, state: "running", reason: nil,
                pid: nil, message: nil, sources: [source], settings: [], actions: [], control_allowed: false, control_asked: false)
            let look = AddonAppearance(colors: [:], numbers: [:], rows: [:], spectrum: [
                AddonSpectrumStop(color: "#ff8800", position: 0), AddonSpectrumStop(color: format == "a" ? "#2c68ec" : "#22cc66", position: 1)])
            host.verifyReceive(AddonSnapshot(schema: 1, generation: generation, ready: true, error: nil,
                                             addons: [addon], appearances: ["verify.page": look], completed: []))
            controller.syncAddonSnapshotUI()
        }
        controller.selectedLookAddonID = "verify.page"
        controller.verifyLookControlRebuilds = 0
        receiveSource("a")
        receiveSource("b")
        let sourcesUnderList = controller.verifySourcesTabRebuilds
        let lookUnderList = controller.verifyLookControlRebuilds
        let pending = (controller.pendingSourcesTabRebuild, controller.pendingLookControlsRebuild)
        OptionsPopUpButton.verifyShown = false
        controller.syncAddonSnapshotUI()
        controller.syncAddonSnapshotUI()
        guard reloadsUnderList == 0, addonsPending, sourcesUnderList == 0, lookUnderList == 0, pending == (true, true),
              controller.verifySourcesTabRebuilds == 1, controller.verifyLookControlRebuilds == 1 else {
            throw CaptureError("Under an open list: Add-ons \(reloadsUnderList) (pending \(addonsPending)), Sources \(sourcesUnderList), look \(lookUnderList), pending \(pending); after: Sources \(controller.verifySourcesTabRebuilds), look \(controller.verifyLookControlRebuilds)")
        }
    }

    /// A look that repaints its colours every 250 ms neither rebuilds the Appearance page's look
    /// controls nor reloads the Add-ons page; a new offer or gradient does.
    static func verifyLookUpdatesKeepControls() throws {
        func look(_ waveform: String, spectrum: [AddonSpectrumStop] = []) -> AddonAppearance {
            AddonAppearance(colors: ["waveform": waveform], numbers: [:], rows: [:], spectrum: spectrum)
        }
        func snapshot(_ generation: UInt64, _ looks: [String: AddonAppearance]) -> AddonSnapshot {
            AddonSnapshot(schema: 1, generation: generation, ready: true, error: nil, addons: [], appearances: looks, completed: [])
        }
        let warm = [AddonSpectrumStop(color: "#ff8800", position: 0), AddonSpectrumStop(color: "#2c68ec", position: 1)]
        let a = snapshot(1, ["theme": look("#88ccff", spectrum: warm)])
        let b = snapshot(2, ["theme": look("#99ddff", spectrum: warm)])
        let c = snapshot(3, ["theme": look("#99ddff", spectrum: warm.reversed().map { AddonSpectrumStop(color: $0.color, position: 1 - $0.position) })])
        let d = snapshot(4, ["theme": look("#99ddff"), "other": look("#ffffff")])
        let repaint = lookControlsChanged(from: a, to: b, chosen: "theme") || addonsPageChanged(from: a, to: b)
        guard !repaint, lookControlsChanged(from: b, to: c, chosen: "theme"), lookControlsChanged(from: c, to: d, chosen: "theme"),
              !lookControlsChanged(from: a, to: b, chosen: nil) else {
            throw CaptureError("A look's colour update rebuilds controls (\(repaint)) or a new gradient or offer does not")
        }
    }

    /// A frequency read made just before a silent channel's row went away still colours the rows
    /// that remain, each with its own colours.
    static func verifyFrequencyRowsFollowIdentity() throws {
        let labels = ["FL", "FR", "FC", "LFE", "Ls", "Rs"]
        let before = WaveformRow.layout(widths: [6, 2], labels: [labels, []], stereo: false, activeMasks: [0b11_0111, nil])
        let after = WaveformRow.layout(widths: [6, 2], labels: [labels, []], stereo: false, activeMasks: [0b11_0011, nil])
        let mapped = after.indices.map { WaveformView.frequencyRowIndex(of: $0, displayed: after, read: before) }
        let expected = after.map { row in before.firstIndex(of: row) }
        guard before != after, mapped == expected, !mapped.contains(where: { $0 == nil }) else {
            throw CaptureError("Rows after a silent channel hid map to \(mapped), expected \(expected)")
        }
    }

    /// A Number setting has − and + beside its field. Each kind of setting, changed through its
    /// own control, cannot be changed again until the add-on answers, and is changeable again
    /// after. A homepage holding a control character is not opened. Ticking "Also delete its
    /// saved data" drops the note about keeping it.
    static func verifyAddonSettingControls() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-settings-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        guard let host = AddonHostClient(dataRoot: scratch.path, appVersion: "0.2.1", primary: true) else {
            throw CaptureError("AddonHostClient failed to start")
        }
        controller.attach(addonHost: host)
        defer { host.shutdownAndFree(); controller.addonHost = nil }
        let ready = ProcessInfo.processInfo.systemUptime + 3
        while !host.snapshot.ready && ProcessInfo.processInfo.systemUptime < ready {
            host.pollSnapshot()
            Thread.sleep(forTimeInterval: 0.01)
        }
        func controls(_ view: NSView) -> [NSControl] {
            // Labels are text fields too; only what the user can change counts.
            let own = (view as? NSControl).flatMap { control -> NSControl? in
                if let text = control as? NSTextField, !text.isEditable { return nil }
                return control
            }
            return (own.map { [$0] } ?? []) + view.subviews.flatMap(controls)
        }
        func declaration(_ kind: String, _ key: String) -> AddonSettingEntry {
            switch kind {
            case "number":
                return AddonSettingEntry(declaration: AddonSettingDeclaration(key: key, label: "Gain", help: nil, kind: kind,
                    defaultValue: .double(0.5), options: nil, min: 0, max: 1, step: 0.1), value: .double(0.5))
            case "choice":
                return AddonSettingEntry(declaration: AddonSettingDeclaration(key: key, label: "Mode", help: nil, kind: kind,
                    defaultValue: .string("a"), options: [AddonChoiceOption(value: "a", label: "A"), AddonChoiceOption(value: "b", label: "B")],
                    min: nil, max: nil, step: nil), value: .string("a"))
            default:
                return AddonSettingEntry(declaration: AddonSettingDeclaration(key: key, label: "Loud", help: nil, kind: kind,
                    defaultValue: .bool(false), options: nil, min: nil, max: nil, step: nil), value: .bool(false))
            }
        }
        var problems: [String] = []
        var steps: [String] = []
        for kind in ["number", "toggle", "choice"] {
            let entry = declaration(kind, "k-\(kind)")
            let page = controls(controller.makeAddonSettingView(addonID: "verify.settings", entry: entry))
            if kind == "number" { steps = page.compactMap { ($0 as? NSButton)?.title }.filter { $0 == "−" || $0 == "+" } }
            let reloadsBefore = controller.verifyAddonsTabReloads
            switch kind {
            case "number":
                guard let plus = page.compactMap({ $0 as? NSButton }).first(where: { $0.title == "+" }) else { continue }
                controller.addonSettingNumberStep(plus)
            case "toggle":
                guard let check = page.compactMap({ $0 as? NSButton }).first else { continue }
                check.state = .on
                controller.addonSettingToggleChanged(check)
            default:
                guard let popup = page.compactMap({ $0 as? NSPopUpButton }).first else { continue }
                popup.selectItem(at: 1)
                controller.addonSettingChoiceChanged(popup)
            }
            let rebuilt = controls(controller.makeAddonSettingView(addonID: "verify.settings", entry: entry))
            if page.isEmpty || page.contains(where: \.isEnabled) || rebuilt.contains(where: \.isEnabled)
                || controller.verifyAddonsTabReloads == reloadsBefore {
                problems.append("\(kind) stays changeable while waiting (page \(page.map(\.isEnabled)), rebuilt \(rebuilt.map(\.isEnabled)))")
            }
            // The add-on manager answers (here: there is no such add-on); the setting can be changed again.
            let deadline = ProcessInfo.processInfo.systemUptime + 3
            while controller.addonSettingDrafts["verify.settings:k-\(kind)"] != nil && ProcessInfo.processInfo.systemUptime < deadline {
                host.pollSnapshot()
                Thread.sleep(forTimeInterval: 0.01)
            }
            let answered = controls(controller.makeAddonSettingView(addonID: "verify.settings", entry: entry))
            if answered.isEmpty || !answered.allSatisfy(\.isEnabled) {
                problems.append("\(kind) is not changeable after the answer (\(answered.map(\.isEnabled)))")
            }
        }
        if steps != ["−", "+"] { problems.append("Number steps \(steps)") }
        let link = AddonFormatting.linkIsOpenable
        if link("https://example.com/\u{7}a") || link("https://example.com/a\u{85}") || !link("https://example.com/it's") {
            problems.append("link checks")
        }
        // Removing: ticking "Also delete its saved data" rebuilds the page without the keep-data note.
        let addon = AddonRecord(id: "verify.remove", manifest: nil, enabled: false, state: "stopped", reason: nil,
            pid: nil, message: nil, sources: [], settings: [], actions: [], control_allowed: false, control_asked: false)
        controller.addonRemoval = (id: addon.id, name: "Verify", deleteData: false, busy: false, error: nil)
        func texts(_ view: NSView) -> [String] {
            ((view as? NSTextField).map { [$0.stringValue] } ?? []) + view.subviews.flatMap(texts)
        }
        let keepNote = "Unless you delete it, its saved data stays for a later install."
        let card = controller.makeAddonCardGroup(addon)
        let reloads = controller.verifyAddonsTabReloads
        if let delete = controls(card).compactMap({ $0 as? NSButton }).first(where: { $0.title == "Also delete its saved data" }),
           texts(card).contains(keepNote) {
            delete.state = .on
            controller.addonRemoveDeleteDataChanged(delete)
            if controller.verifyAddonsTabReloads == reloads || texts(controller.makeAddonCardGroup(addon)).contains(keepNote) {
                problems.append("ticking delete data left the keep-data note")
            }
        } else {
            problems.append("the removal card has no delete-data box or note")
        }
        guard problems.isEmpty else { throw CaptureError("Add-on settings: " + problems.joined(separator: "; ")) }
    }

    /// An add-on's `select` is answered once the selection's audio is frozen, not before.
    /// It spins the main queue, so it runs with the other main-queue checks at the end.
    static func verifySelectWaitsForFreeze() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-select-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        let root = scratch.appendingPathComponent("addons-root", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        guard let host = AddonHostClient(dataRoot: root.path, appVersion: "0.2.1", primary: true) else {
            throw CaptureError("AddonHostClient failed to start")
        }
        controller.attach(addonHost: host)
        defer { host.shutdownAndFree(); controller.addonHost = nil }
        let lanes = [CaptureLane(source: .addon(addonID: "verify.select", sourceID: "a"), route: .merged)]
        let tap = try SystemAudioTap(history: 10, lanes: lanes)
        controller.captureLanes = lanes
        controller.tap = tap
        defer { controller.tap = nil }
        let samples = [Float](repeating: 0.25, count: 48_000 * 2)
        _ = tap.withCaptureRead { core in
            samples.withUnsafeBufferPointer { bt_tracks_push_interleaved(core, 0, $0.baseAddress, 48_000, 1_000_000_000, SystemAudioTap.timelineRate) }
        }
        let (first, end) = tap.withCaptureSnapshot { (bt_tracks_first_frame($0), bt_tracks_end_frame($0)) }
        let view = WaveformView(frame: NSRect(x: 0, y: 0, width: 600, height: 200))
        view.controller = controller
        controller.waveform = view
        defer { controller.waveform = nil }
        view.verifyShow(rows: WaveformRow.layout(widths: [2], labels: [[]], stereo: false), laneWidths: [2], frames: first..<end)
        // Something else holds the capture queue, so the freeze is still running when select returns.
        tap.readCapture(poll: false, { _ in Thread.sleep(forTimeInterval: 0.3); return 0 }) { _ in }
        view.verifySilentPreview = true
        func call(_ name: String, _ id: String) -> AddonControlCall {
            let command = AddonControlCommand(command: name, from_seconds_ago: name == "select" ? 0.5 : nil,
                to_seconds_ago: name == "select" ? 0.1 : nil, start_frame: nil, end_frame: nil, lane: nil,
                first: nil, last: nil, seconds: nil, folder: nil, source: nil)
            return AddonControlCall(addon_id: "verify.select", addon_name: "Verify", run: 1, request_id: id, command: command)
        }
        // One batch: select, then play_preview while the freeze is still running.
        controller.executeSingleControlCall(host: host, call: call("select", "sel"))
        controller.executeSingleControlCall(host: host, call: call("play_preview", "early"))
        let early = host.verifyEvents.filter { $0.hasPrefix("control_reply") }
        // Only the main queue runs now, no poll: the freeze's own completion must answer select.
        let deadline = ProcessInfo.processInfo.systemUptime + 5
        while !host.verifyEvents.contains("control_reply sel ok") && ProcessInfo.processInfo.systemUptime < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        controller.executeSingleControlCall(host: host, call: call("play_preview", "late"))
        let replies = host.verifyEvents.filter { $0.hasPrefix("control_reply") }
        guard early == ["control_reply early failed"],
              replies == ["control_reply early failed", "control_reply sel ok", "control_reply late ok"],
              controller.buildControlState().preview_playing, view.hasEditingMaterial, !view.isPreparingMaterial else {
            throw CaptureError("select and play_preview answered \(early) before the freeze and \(replies) in all (playing \(controller.buildControlState().preview_playing))")
        }
    }

    /// An add-on source without audio shows Windows' short capture status, not the Sources page's sentence.
    static func verifyAddonCaptureStatus() throws {
        let status = SystemAudioTap.addonCaptureStatus
        let missing = status(nil, "main")
        guard missing == "Add-on is not installed" else {
            throw CaptureError("A missing add-on's capture status is \(missing)")
        }
    }

    /// A test copy opens only test-copy windows and never offers or asks for Input Monitoring.
    static func verifyTestCopyIsolation() throws {
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-copy-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        let parent = [
            "BACKTRACKING_TEST_INSTANCE": "verify-copy", "BACKTRACKING_TEST_DATA_DIR": scratch.path,
            "LOCALAPPDATA": scratch.path, "BACKTRACKING_TEST_LANGUAGE": "zh-Hant",
            "HOME": "/Users/nobody", "PATH": "/usr/bin"
        ]
        let launch = receiverWindowLaunch(testCopy: true, environment: parent)
        let child = TestCopyConfig(environment: launch.environment, arguments: ["Backtracking"] + launch.arguments)
        guard launch.arguments.contains(receiverWindowArgument),
              launch.arguments.contains(TestCopyConfig.testCopyLaunchArgument),
              launch.environment["HOME"] == nil, launch.environment["PATH"] == nil,
              child.isTestCopy, child.dataDirectory?.path == scratch.resolvingSymlinksInPath().path,
              child.language == .zhHant else {
            throw CaptureError("A test copy's new capture window would not be the same test copy: \(launch)")
        }
        // A test copy never runs from a bundle with the installed app's identifier; a copy made by
        // make-test-copy.sh, and this unbundled verification, are not refused.
        guard TestCopyConfig.productionBundleRefusal(bundleID: "com.backtracking.audio.capture") != nil,
              TestCopyConfig.productionBundleRefusal(bundleID: "com.backtracking.audio.capture.testcopy.verify") == nil,
              TestCopyConfig.productionBundleRefusal(bundleID: Bundle.main.bundleIdentifier) == nil else {
            throw CaptureError("A test copy would run from the installed app's bundle identifier")
        }
        // State restoration stays off whatever a lower domain says, and no window is restorable.
        // Launched with `-ApplePersistenceIgnoreState NO -NSQuitAlwaysKeepsWindows YES`, as a
        // stand-in for any domain above the registration one. Both domains live only in memory.
        let defaults = UserDefaults.standard
        let savedArguments = defaults.volatileDomain(forName: UserDefaults.argumentDomain)
        defer { defaults.setVolatileDomain(savedArguments, forName: UserDefaults.argumentDomain) }
        defaults.setVolatileDomain(savedArguments.merging(["ApplePersistenceIgnoreState": false, "NSQuitAlwaysKeepsWindows": true]) { $1 },
                                   forName: UserDefaults.argumentDomain)
        TestCopyConfig.turnOffStateRestoration(defaults)
        // CapturePanel and HotkeyOptionsPanel, the app's only windows, are KeyPanels.
        guard defaults.bool(forKey: "ApplePersistenceIgnoreState"), !defaults.bool(forKey: "NSQuitAlwaysKeepsWindows"),
              !KeyPanel.restorable(testCopy: true, requested: true), KeyPanel.restorable(testCopy: false, requested: true) else {
            throw CaptureError("A test copy could still save or restore window state")
        }
        // A data folder inside the user's real settings or recordings is refused like the folders themselves.
        let fakeHome = scratch.appendingPathComponent("home", isDirectory: true)
        let inside = ["Library/Preferences/copy", "Library/Application Support/Backtracking/copy",
                      "Backtracking/copy", "Backtracking/output/copy"]
        let allowed = ["work/copy", "Library/Application Support/Other/copy"]
        for path in inside + allowed {
            try FileManager.default.createDirectory(at: fakeHome.appendingPathComponent(path), withIntermediateDirectories: true)
        }
        let accepted = (inside + allowed).filter {
            if case .success = TestCopyConfig.validateDataDirectory(fakeHome.appendingPathComponent($0).path, realHome: fakeHome) { return true }
            return false
        }
        guard accepted == allowed else {
            throw CaptureError("Test-copy data folders accepted \(accepted), expected only \(allowed)")
        }
        // A window whose test variables went missing must refuse rather than open the real data.
        let lost = TestCopyConfig(environment: [:], arguments: ["Backtracking"] + launch.arguments)
        guard TestCopyConfig.refusesUnisolatedLaunch(arguments: launch.arguments, isTestCopy: lost.isTestCopy),
              !TestCopyConfig.refusesUnisolatedLaunch(arguments: launch.arguments, isTestCopy: child.isTestCopy),
              !TestCopyConfig.refusesUnisolatedLaunch(arguments: [receiverWindowArgument], isTestCopy: false) else {
            throw CaptureError("A test-copy window without its test variables would still start")
        }
        let installed = receiverWindowLaunch(testCopy: false, environment: parent)
        guard installed.arguments == [receiverWindowArgument], installed.environment.isEmpty else {
            throw CaptureError("The installed app's new capture window changed")
        }

        let controller = BacktrackingController()
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        controller.testCopy = true
        var requests = 0
        controller.listenAccessRequest = { requests += 1 }
        controller.snapshotShortcut.bindings = []
        _ = controller.makeActionTab()
        guard !controller.hotkeys[.globalSnapshot].isEmpty, controller.shortcutPermissionButton?.isHidden == true else {
            throw CaptureError("A test copy shows the Input Monitoring button")
        }
        controller.requestShortcutPermission()
        guard requests == 0, !controller.snapshotShortcut.isListening else {
            throw CaptureError("A test copy asked for Input Monitoring \(requests) times")
        }
    }

    /// Two explicit saves are waiting for a freeze that has not finished when the app exits.
    /// Both must still reach disk with their own range and every lane's channels.
    static func verifyTerminationSaves() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-exit-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        // Add-on lanes without a running add-on open no system input, so no permission is involved.
        let lanes = [
            CaptureLane(source: .addon(addonID: "verify.exit", sourceID: "a"), route: .merged),
            CaptureLane(source: .addon(addonID: "verify.exit", sourceID: "b"), route: .merged)
        ]
        let tap = try SystemAudioTap(history: 10, lanes: lanes)
        controller.captureLanes = lanes
        controller.tap = tap
        defer { controller.tap = nil }
        let frames = 4_800
        let levels: [[Float]] = [[0.25, -0.5], [0.125, 0.375]]
        for (lane, level) in levels.enumerated() {
            let samples = (0..<frames).flatMap { _ in level }
            let accepted = tap.withCaptureRead { core in
                samples.withUnsafeBufferPointer {
                    bt_tracks_push_interleaved(core, UInt32(lane), $0.baseAddress, UInt32(frames),
                                               1_000_000_000, SystemAudioTap.timelineRate)
                }
            }
            guard accepted == UInt32(frames) else { throw CaptureError("Exit verification lane \(lane) lost input") }
        }
        let (first, end) = tap.withCaptureSnapshot { (bt_tracks_first_frame($0), bt_tracks_end_frame($0)) }
        guard end > first + 1_100 else { throw CaptureError("Exit verification retained too little audio") }
        // The editing freeze is still running on the capture queue when the app quits.
        tap.readCapture(poll: false, { _ in Thread.sleep(forTimeInterval: 0.3); return 0 }) { _ in }
        let selected = scratch.appendingPathComponent("selected", isDirectory: true)
        let whole = scratch.appendingPathComponent("whole", isDirectory: true)
        try FileManager.default.createDirectory(at: selected, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: whole, withIntermediateDirectories: true)
        let noFades = BtFades(fade_in: 0, fade_out: 0)
        controller.pendingSnapshots = [
            PendingSnapshotSave(range: (first + 100)..<(first + 1_100), fades: noFades, rows: nil,
                                directory: selected.path, ticket: nil, controlCall: nil),
            PendingSnapshotSave(range: nil, fades: noFades, rows: nil,
                                directory: whole.path, ticket: nil, controlCall: nil)
        ]
        let started = ProcessInfo.processInfo.systemUptime
        controller.queuePendingSnapshotsForTermination()
        controller.worker.sync {}
        let waited = ProcessInfo.processInfo.systemUptime - started
        guard controller.pendingSnapshots.isEmpty else { throw CaptureError("Exit left saves in the queue") }

        func check(_ folder: URL, frames expected: Int) throws {
            let files = try FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)
                .filter { $0.pathExtension == "wav" }
            guard files.count == levels.count else {
                throw CaptureError("Exit save in \(folder.lastPathComponent) wrote \(files.count) WAV files, expected \(levels.count) (waited \(String(format: "%.2f", waited)) s)")
            }
            var seen: Set<Int> = []
            for url in files {
                let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
                let count = AVAudioFrameCount(file.length)
                guard file.length == Int64(expected), file.processingFormat.channelCount == 2,
                      let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: count) else {
                    throw CaptureError("Exit save \(url.lastPathComponent) has \(file.length) frames / \(file.processingFormat.channelCount) ch, expected \(expected) / 2")
                }
                try file.read(into: buffer)
                guard let data = buffer.floatChannelData,
                      let lane = levels.firstIndex(where: { level in
                          level.indices.allSatisfy { channel in
                              (0..<Int(count)).allSatisfy { abs(data[channel][$0] - level[channel]) < 0.00001 }
                          }
                      }) else {
                    throw CaptureError("Exit save \(url.lastPathComponent) does not carry one lane's channels")
                }
                seen.insert(lane)
            }
            guard seen.count == levels.count else { throw CaptureError("Exit save in \(folder.lastPathComponent) repeated a lane") }
        }
        try check(selected, frames: 1_000)
        try check(whole, frames: Int(end - first))
        try verifyTerminationSnapshotRead()
    }

    /// A save without a selection asks for its own freeze through `saveSnapshot`; the app quits
    /// before that freeze reaches the main queue. Its WAVs must still be written.
    static func verifyTerminationSnapshotRead() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-exit-read-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        let lanes = [
            CaptureLane(source: .addon(addonID: "verify.exit", sourceID: "a"), route: .merged),
            CaptureLane(source: .addon(addonID: "verify.exit", sourceID: "b"), route: .merged)
        ]
        let tap = try SystemAudioTap(history: 10, lanes: lanes)
        controller.captureLanes = lanes
        controller.tap = tap
        defer { controller.tap = nil }
        let frames = 4_800
        let levels: [[Float]] = [[0.25, -0.5], [0.125, 0.375]]
        for (lane, level) in levels.enumerated() {
            let samples = (0..<frames).flatMap { _ in level }
            let accepted = tap.withCaptureRead { core in
                samples.withUnsafeBufferPointer {
                    bt_tracks_push_interleaved(core, UInt32(lane), $0.baseAddress, UInt32(frames),
                                               1_000_000_000, SystemAudioTap.timelineRate)
                }
            }
            guard accepted == UInt32(frames) else { throw CaptureError("Exit read verification lane \(lane) lost input") }
        }
        let (first, end) = tap.withCaptureSnapshot { (bt_tracks_first_frame($0), bt_tracks_end_frame($0)) }
        // Something else is still running on the capture queue, so the save's freeze waits behind it.
        tap.readCapture(poll: false, { _ in Thread.sleep(forTimeInterval: 0.3); return 0 }) { _ in }
        let folder = scratch.appendingPathComponent("whole", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        controller.saveSnapshot(into: folder.path)
        controller.queuePendingSnapshotsForTermination()
        controller.worker.sync {}
        let files = try FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "wav" }
        guard files.count == levels.count else {
            throw CaptureError("A whole-capture save made just before quitting wrote \(files.count) WAV files, expected \(levels.count)")
        }
        for url in files {
            let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
            guard file.length == Int64(end - first), file.processingFormat.channelCount == 2 else {
                throw CaptureError("Exit read save \(url.lastPathComponent) has \(file.length) frames, expected \(end - first)")
            }
        }
        guard controller.pendingSnapshotReads.isEmpty else { throw CaptureError("Exit left a save waiting for its freeze") }
        try verifyTerminationAnnouncesClips()
    }

    /// Quitting while two saves wait for the window's freeze, after the oldest audio they asked for
    /// has aged out of the live history. Both are written from that freeze, and each clip is
    /// announced to the add-ons before their manager closes.
    static func verifyTerminationAnnouncesClips() throws {
        let controller = BacktrackingController()
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-exit-clips-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }
        controller.defaults = PreferencesStore.file(at: scratch.appendingPathComponent("preferences.plist"))
        let root = scratch.appendingPathComponent("addons-root", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        guard let host = AddonHostClient(dataRoot: root.path, appVersion: "0.2.1", primary: true) else {
            throw CaptureError("AddonHostClient failed to start")
        }
        controller.attach(addonHost: host)
        defer { host.shutdownAndFree() }
        let ready = ProcessInfo.processInfo.systemUptime + 3
        while !host.snapshot.ready && ProcessInfo.processInfo.systemUptime < ready {
            host.pollSnapshot()
            Thread.sleep(forTimeInterval: 0.01)
        }
        guard host.snapshot.ready else { throw CaptureError("Add-on host did not become ready") }
        let lanes = [
            CaptureLane(source: .addon(addonID: "verify.exit", sourceID: "a"), route: .merged),
            CaptureLane(source: .addon(addonID: "verify.exit", sourceID: "b"), route: .merged)
        ]
        let history: UInt32 = 10
        let tap = try SystemAudioTap(history: history, lanes: lanes)
        controller.captureLanes = lanes
        controller.tap = tap
        let levels: [[Float]] = [[0.25, -0.5], [0.125, 0.375]]
        let chunk = 4_800
        var pushed: UInt64 = 0
        func push(seconds: Int) throws {
            for _ in 0..<(seconds * Int(SystemAudioTap.timelineRate) / chunk) {
                let timestamp = 1_000_000_000 + pushed * 1_000_000_000 / UInt64(SystemAudioTap.timelineRate)
                pushed += UInt64(chunk)
                for (lane, level) in levels.enumerated() {
                    let samples = [Float]((0..<chunk).flatMap { _ in level })
                    let accepted = tap.withCaptureSnapshot { core in
                        samples.withUnsafeBufferPointer {
                            bt_tracks_push_interleaved(core, UInt32(lane), $0.baseAddress, UInt32(chunk),
                                                       timestamp, SystemAudioTap.timelineRate)
                        }
                    }
                    guard accepted == UInt32(chunk) else { throw CaptureError("Exit clip verification lane \(lane) lost input") }
                }
            }
        }
        try push(seconds: Int(history))
        let (first, end) = tap.withCaptureSnapshot { (bt_tracks_first_frame($0), bt_tracks_end_frame($0)) }
        // The window starts its editing freeze; two saves wait for it.
        controller.requestEditingFreeze { _ in }
        let selected = scratch.appendingPathComponent("selected", isDirectory: true)
        let whole = scratch.appendingPathComponent("whole", isDirectory: true)
        try FileManager.default.createDirectory(at: selected, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: whole, withIntermediateDirectories: true)
        let noFades = BtFades(fade_in: 0, fade_out: 0)
        controller.pendingSnapshots = [
            PendingSnapshotSave(range: (first + 100)..<(first + 1_100), fades: noFades, rows: nil,
                                directory: selected.path, ticket: nil, controlCall: nil),
            PendingSnapshotSave(range: nil, fades: noFades, rows: nil,
                                directory: whole.path, ticket: nil, controlCall: nil)
        ]
        // Capture goes on behind the freeze until the selected audio has left the live history.
        var liveFirst = first
        for _ in 0..<60 where liveFirst <= first + 1_100 {
            try push(seconds: 1)
            liveFirst = tap.withCaptureSnapshot { bt_tracks_first_frame($0) }
        }
        guard liveFirst > first + 1_100 else {
            throw CaptureError("Exit clip verification setup: live history still starts at \(liveFirst), selection ends at \(first + 1_100)")
        }
        controller.applicationWillTerminate(Notification(name: NSApplication.willTerminateNotification))

        var problems: [String] = []
        func wavs(_ folder: URL) throws -> [URL] {
            try FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil).filter { $0.pathExtension == "wav" }
        }
        for (folder, expected) in [(selected, Int64(1_000)), (whole, Int64(end - first))] {
            let files = try wavs(folder)
            if files.count != levels.count {
                problems.append("\(folder.lastPathComponent) has \(files.count) WAV files, expected \(levels.count)")
                continue
            }
            for url in files {
                let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
                if file.length != expected {
                    problems.append("\(url.lastPathComponent) has \(file.length) frames, expected \(expected)")
                }
            }
        }
        let expectedEvents = ["clip_exported 48000 1000 2", "clip_exported 48000 \(end - first) 2"]
        let events = host.verifyEvents
        let sent = events.filter { $0.hasPrefix("clip_exported ") && $0 != "clip_exported answered" }
        let answeredBeforeShutdown = events.firstIndex(of: "shutdown").map { shutdown in
            events[..<shutdown].filter { $0 == "clip_exported answered" }.count
        } ?? 0
        if sent != expectedEvents || answeredBeforeShutdown != expectedEvents.count || events.last != "shutdown" {
            problems.append("host saw \(events), expected \(expectedEvents), each answered, then shutdown")
        }
        guard problems.isEmpty else {
            throw CaptureError("Quitting with saves waiting: " + problems.joined(separator: "; "))
        }
    }
    #endif

    private func beginCaptureAfterUpdate() {
        loadCapturePreferences()
        configureMenu()
        configurePanel()
        if !independentWindow && !testCopy && !TestCopyConfig.current.showOptionsOnStart {
            configureStatusItem()
        }
        if !independentWindow && !testCopy {
            snapshotShortcut.start()
        }
        resume()
        if TestCopyConfig.current.showOptionsOnStart {
            showOptions()
        }
        let timer = Timer(timeInterval: 1.0 / 15.0, target: self, selector: #selector(tick(_:)), userInfo: nil, repeats: true)
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
        if harness != nil {
            let hTimer = Timer(timeInterval: 0.05, target: self, selector: #selector(driveHarnessTick(_:)), userInfo: nil, repeats: true)
            self.harnessTimer = hTimer
            RunLoop.main.add(hTimer, forMode: .common)
        }
    }

    private func loadCapturePreferences() {
        let saved = defaults.array(forKey: "captureLanes") as? [[String: Any]] ?? []
        let loaded: [CaptureLane] = saved.prefix(maximumCaptureLanes).compactMap { item in
            guard let key = item["source"] as? String, let source = CaptureSource(key: key),
                  let number = item["route"] as? NSNumber, let raw = UInt32(exactly: number.doubleValue),
                  let route = CaptureRoute(rawValue: raw) else { return nil }
            return CaptureLane(source: source, route: route)
        }
        if !loaded.isEmpty, loaded.count == saved.count {
            captureLanes = loaded
            let modes = defaults.array(forKey: "meterModes") ?? []
            meterModes = captureLanes.indices.map { index in
                guard modes.indices.contains(index), let number = modes[index] as? NSNumber,
                      let raw = UInt32(exactly: number.doubleValue) else { return .dbfs }
                return MeterMode(rawValue: raw) ?? .dbfs
            }
        } else {
            let defaultRoute: CaptureRoute = (testCopy && TestCopyConfig.current.syntheticSystemFormat != nil
                && defaults.object(forKey: "captureRouteA") == nil) ? .allChannels : .merged
            let first = defaults.string(forKey: "captureSourceA").flatMap(CaptureSource.init(key:)) ?? .system
            let routeA = unsignedSetting("captureRouteA").flatMap(CaptureRoute.init(rawValue:)) ?? defaultRoute
            captureLanes = [CaptureLane(source: first, route: routeA)]
            meterModes = [unsignedSetting("meterMode").flatMap(MeterMode.init(rawValue:)) ?? .dbfs]
            if defaults.bool(forKey: "dualCaptureSource") {
                let second = defaults.string(forKey: "captureSourceB").flatMap(CaptureSource.init(key:)) ?? .microphone("")
                let routeB = unsignedSetting("captureRouteB").flatMap(CaptureRoute.init(rawValue:)) ?? .merged
                captureLanes.append(CaptureLane(source: second, route: routeB))
                meterModes.append(unsignedSetting("meterModeB").flatMap(MeterMode.init(rawValue:)) ?? .dbfs)
            }
        }
        stereoWaveform = defaults.bool(forKey: "stereoWaveform")
        if let names = defaults.dictionary(forKey: sourceFileNamesKey) as? [String: String] {
            sourceFileNames = names.filter { !$0.value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            let savedOrder = (defaults.array(forKey: sourceFileNameOrderKey) as? [String] ?? [])
                .filter { sourceFileNames[$0] != nil }
            let extras = sourceFileNames.keys.filter { !savedOrder.contains($0) }.sorted()
            sourceFileNameOrder = savedOrder + extras
            trimSourceFileNamesIfNeeded()
        }
    }

    /// What `source` is called in file names: the name the user gave it, or its own name when
    /// they gave none or one that leaves nothing a file name can hold.
    private func fileName(for source: CaptureSource) -> String {
        if let custom = sourceFileNames[source.key], filePrefix(custom, lane: 0) != filePrefix("", lane: 0) {
            return custom
        }
        return defaultFileName(for: source)
    }

    /// How the core begins the names of a source's files: everything before the date.
    /// `lane` is 0 for a single source, or the one-based lane index.
    private func filePrefix(_ name: String, lane: UInt32) -> String {
        guard let prefix = name.withCString({ bt_take_file_prefix($0, lane) }) else { return "" }
        defer { bt_string_free(prefix) }
        return String(cString: prefix)
    }

    /// The names each recording lane's files carry.
    private var laneFileNames: [String] { captureLanes.map { fileName(for: $0.source) } }

    /// Looks up each recording source's own name while it is there, so files exported after it
    /// goes away still carry that name.
    private func rememberSourceNames() {
        for lane in captureLanes { _ = defaultFileName(for: lane.source) }
    }

    private func unsignedSetting(_ key: String) -> UInt32? {
        guard let number = defaults.object(forKey: key) as? NSNumber else { return nil }
        return UInt32(exactly: number.doubleValue)
    }

    private func directory(for folder: ExportFolder) -> String {
        if let saved = defaults.string(forKey: folder.key), saved.hasPrefix("/"), !saved.utf8.contains(0) {
            return saved
        }
        return TestCopyConfig.current.defaultExportDirectory(component: folder.component)
    }

    func applicationWillTerminate(_ notification: Notification) {
        if !independentWindow && !testCopy { panel?.saveFrame(usingName: frameAutosaveName) }
        timer?.invalidate()
        timer = nil
        harnessTimer?.invalidate()
        harnessTimer = nil
        if !independentWindow && !testCopy { snapshotShortcut.stop() }
        defaults.cleanup()
        TestCopyConfig.current.releaseLock()
        if let cleanup {
            // Waits for a pass that is still running; a pass only lists one folder.
            bt_cleanup_free(cleanup)
            self.cleanup = nil
        }
        finishSavesForTermination()
        addonHost?.shutdownAndFree(timeoutMs: 2500)
        addonHost = nil
        waveform?.endPreview()
        tap = nil
        waveform?.discardPreparedFile()
        // Worker jobs dispatch their completion asynchronously and never wait for this thread.
        worker.sync {
            for url in pendingProvisionalWAVs.urls { try? FileManager.default.removeItem(at: url) }
            pendingProvisionalWAVs.urls.removeAll()
        }
    }

    /// Explicit saves that are still waiting for frozen audio are written before the app exits.
    private func queuePendingSnapshotsForTermination() {
        let queued = pendingSnapshots
        let ownReads = pendingSnapshotReads
        pendingSnapshots.removeAll()
        pendingSnapshotReads.removeAll()
        guard !queued.isEmpty || !ownReads.isEmpty, let tap else { return }
        // readCapture delivers on the main queue, which this thread is blocking. The capture queue is
        // serial and never waits for the main queue, so this waits for every freeze still in flight.
        tap.withCaptureRead { _ in }
        var refrozen: WAVExport??
        func freezeNow() -> WAVExport? {
            if let refrozen { return refrozen }
            let routes = tap.captureRoutes
            let labels = tap.captureChannelNames
            let groups = (0..<tap.laneCount).map { tap.channelGroups(for: $0) }
            let frozenPos = tap.freezePositions()
            let frozen = tap.withCaptureSnapshot {
                WAVExport(core: $0, routes: routes, channelNames: labels, channelGroups: groups, positions: frozenPos)
            }
            refrozen = .some(frozen)
            return frozen
        }
        let current = ObjectIdentifier(tap)
        // A save that asked for its own freeze writes that freeze, as Windows does with the read it requested.
        var jobs: [(WAVExport, PendingSnapshotSave)] = ownReads.compactMap { entry in
            let own = entry.read.tap == current ? entry.read.material : nil
            return (own ?? freezeNow()).map { ($0, entry.save) }
        }
        // A save waiting for the window's freeze writes the first one that holds it, as it would have
        // when that freeze arrived; freezing again now could miss audio that has aged out since.
        let editing = editingFreezeReads.filter { $0.tap == current }.compactMap(\.material)
        editingFreezeReads.removeAll()
        jobs += queued.compactMap { req in
            let held = editing.first { material in
                req.range.map { $0.lowerBound >= material.span.lowerBound && $0.upperBound <= material.span.upperBound } ?? true
            }
            return (held ?? freezeNow()).map { ($0, req) }
        }
        let sampleRate = exportSampleRate
        let bits = exportBitsPerSample
        let mono = forceMonoExport
        let wide = wideExportMode
        let binaural = pairBinaural
        let names = laneFileNames
        let hide = hideSilentLaneIndices
        let shownIDs = currentShownGroupIDs()
        for (frozen, req) in jobs {
            let range = req.range ?? frozen.span
            guard range.lowerBound >= frozen.span.lowerBound, range.upperBound <= frozen.span.upperBound,
                  !range.isEmpty else { continue }
            let ledger = clipLedger
            let token = ledger.reserve()
            worker.async {
                let written = frozen.write(range: range, directory: req.directory, sampleRate: sampleRate,
                                           bitsPerSample: bits, forceMono: mono, fades: req.fades, names: names,
                                           selectedRows: req.rows, hideSilentLanes: hide, wideMode: wide,
                                           pairBinaural: binaural, shownGroupIDs: shownIDs)
                if !written.urls.isEmpty {
                    ledger.add(ClipExportLedger.Clip(urls: written.urls, sampleRate: sampleRate,
                                                     sourceFrames: range.upperBound - range.lowerBound,
                                                     sourceRate: frozen.sampleRate), as: token)
                }
            }
        }
    }

    /// Writes every save still on its way, then tells the add-ons about each clip before their
    /// manager closes (Windows `Recorder::drop`). An add-on's own `save` gets no `control_reply` here,
    /// as on Windows.
    private func finishSavesForTermination() {
        queuePendingSnapshotsForTermination()
        // Write jobs hand their completion to the main queue and never wait for it.
        worker.sync {}
        let clips = clipLedger.takeAll()
        guard let host = addonHost, !clips.isEmpty else { return }
        var unanswered = clips.count
        for clip in clips {
            notifyClipExported(clip) { _ in unanswered -= 1 }
        }
        // The manager takes requests on its own thread; wait briefly so they are handed on before it closes.
        host.poll(until: ProcessInfo.processInfo.systemUptime + 1.0) { unanswered == 0 }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if independentWindow { NSApp.terminate(nil) }
        else { hidePanel(sender) }
        return false
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if let panel, !panel.isVisible { showPanel(panel) }
        return false
    }

    func applicationDidChangeScreenParameters(_ notification: Notification) {
        if let panel { keepPanelVisible(panel) }
    }

    private func configureMenu() {
        let bar = NSMenu()
        let item = NSMenuItem()
        let menu = NSMenu()
        menu.addItem(quitItem(shortcut: "q"))
        item.submenu = menu
        bar.addItem(item)
        NSApp.mainMenu = bar
    }

    private func configureStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        if let image = NSImage(systemSymbolName: "waveform", accessibilityDescription: "Backtracking") {
            image.isTemplate = true
            item.button?.image = image
        } else {
            item.button?.title = "BT"
        }
        item.button?.toolTip = "Backtracking"
        let menu = NSMenu(title: "Backtracking")
        menu.delegate = self
        item.menu = menu
        statusItem = item
        menuNeedsUpdate(menu)
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        if snapshotTargetPopup?.menu === menu {
            refreshSnapshotTargets()
            return
        }
        if let source = sourcePopups.first(where: { $0.menu === menu })
            ?? (addSourcePopup?.menu === menu ? addSourcePopup : nil) {
            // Package 10 / CAP-06: trigger async background source scan when opening Audio source or Add source.
            startBackgroundSourceScan()
            refreshSourcePopup(source, choices: cachedSourceChoices ?? [], scanning: sourceScanInFlight)
            return
        }
        guard menu === statusItem?.menu else { return }
        menu.removeAllItems()
        let title = panel?.isVisible == true ? "Hide Backtracking" : "Show Backtracking"
        let visibility = NSMenuItem(title: title, action: #selector(togglePanelVisibility), keyEquivalent: "")
        visibility.target = self
        menu.addItem(visibility)
        menu.addItem(captureMenuItem())
        let options = NSMenuItem(title: "Options…", action: #selector(showOptions), keyEquivalent: "")
        options.target = self
        menu.addItem(options)
        menu.addItem(loginItem())
        menu.addItem(.separator())
        menu.addItem(quitItem())
    }

    @objc private func togglePanelVisibility() {
        guard let panel else { return }
        if panel.isVisible {
            hidePanel(panel)
        } else {
            showPanel(panel)
        }
    }

    private func trimSourceFileNamesIfNeeded() {
        while sourceFileNameOrder.count > sourceFileNameMaxEntries {
            let evicted = sourceFileNameOrder.removeFirst()
            sourceFileNames.removeValue(forKey: evicted)
        }
    }

    private func hidePanel(_ panel: NSWindow) {
        // Package 12 / WIN-18: stop preview, close Options and meter popover, abort active gestures.
        waveform?.stopPreview()
        waveform?.abortGestures()
        closeMeterPopover()
        optionsPanel?.orderOut(nil)
        if !independentWindow && !testCopy { panel.saveFrame(usingName: frameAutosaveName) }
        panel.orderOut(nil)
        updateOptionsVisibility()
    }

    private func showPanel(_ panel: NSWindow) {
        keepPanelVisible(panel)
        refreshWaveform()
        updateStatus()
        panel.orderFrontRegardless()
        updateOptionsVisibility()
    }

    private func configurePanel() {
        let initialSize = waveformOnly ? waveformOnlyPanelDefault : fullPanelDefault
        let window = CapturePanel(
            contentRect: NSRect(origin: .zero, size: initialSize),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered, defer: false
        )
        // Package 8 / LOOK-01: window is always dark.
        window.appearance = NSAppearance(named: .darkAqua)
        window.title = TestCopyConfig.current.windowTitle(independentWindow ? "Backtracking Capture" : "Backtracking")
        window.contentMinSize = fullPanelMinimum
        // CapturePanel handles dragging and resizing without AppKit's native window gestures.
        window.isMovable = false
        window.delegate = self
        window.becomesKeyOnlyIfNeeded = true
        window.acceptsMouseMovedEvents = true
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .fullScreenDisallowsTiling]
        window.onClick = { [weak self] in self?.registerClick() }
        window.onMenu = { [weak self] event in self?.showContextMenu(with: event) }
        window.onManualResize = { [weak self] in
            self?.lastManualResize = ProcessInfo.processInfo.systemUptime
            self?.autoGrownBaseHeight = nil
            self?.autoGrownTargetHeight = nil
            self?.updateMeter()
        }
        window.onCommand = { [weak self] command in self?.run(command) ?? false }
        window.actionForShortcut = { [weak self] shortcut, isRepeat, canCancel in
            self?.hotkeys.windowAction(for: shortcut, isRepeat: isRepeat, canCancel: canCancel)
        }
        window.hasPopupOpen = { [weak self] in
            self?.meterPopover?.isShown == true
        }

        let waveform = WaveformView(frame: .zero)
        waveform.controller = self
        waveform.accentRGB = accentRGB
        waveform.stereo = stereoWaveform
        waveform.hideSilentChannels = hideSilentChannels
        waveform.frequencyGradient = openingWaveformGradient
        let minimumWaveformHeight = waveform.heightAnchor.constraint(greaterThanOrEqualToConstant: fullWaveformMinimumHeight)
        minimumWaveformHeight.isActive = true
        waveformMinimumHeight = minimumWaveformHeight
        window.waveform = waveform
        self.waveform = waveform

        captureButton.target = self
        captureButton.action = #selector(toggleCapture(_:))
        captureButton.bezelStyle = .rounded
        captureLamp.setAccessibilityElement(true)
        captureLamp.setAccessibilityRole(.staticText)
        clearButton.target = self
        clearButton.action = #selector(clearCapture)
        clearButton.bezelStyle = .rounded
        setIcon("trash", label: "Clear capture", on: clearButton)
        titleLabel.font = .systemFont(ofSize: 14, weight: .bold)
        titleLabel.setContentCompressionResistancePriority(.required, for: .horizontal)
        titleLabel.isHidden = !showsTitle

        metersStack.orientation = .horizontal
        metersStack.spacing = 4
        metersStack.alignment = .centerY
        moreMetersButton.target = self
        moreMetersButton.action = #selector(showMoreMeters(_:))
        moreMetersButton.bezelStyle = .rounded
        moreMetersButton.font = .monospacedSystemFont(ofSize: 10, weight: .medium)
        moreMetersButton.widthAnchor.constraint(equalToConstant: 34).isActive = true

        pinButton.target = self
        pinButton.action = #selector(pinChanged(_:))
        pinButton.setButtonType(.pushOnPushOff)
        pinButton.bezelStyle = .rounded
        let settingsButton = NSButton(title: "Options", target: self, action: #selector(showOptions))
        settingsButton.bezelStyle = .rounded
        setIcon("gearshape", label: "Options", on: settingsButton)
        let spacer = NSView(frame: .zero)
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        // Package 6 / WIN-08: name, transport, status lamp, clear, pin on left; meters, +N, gear on right.
        let controls = NSStackView(views: [
            titleLabel, captureButton, captureLamp, clearButton, pinButton, spacer,
            metersStack, moreMetersButton, settingsButton
        ])
        controls.orientation = .horizontal
        controls.spacing = 6
        controls.alignment = .centerY

        // Package 6 / TL-02 / WIN-06: 26px status strip below the waveform.
        statusLabel.font = .systemFont(ofSize: 11, weight: .regular)
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.lineBreakMode = .byTruncatingTail
        statusLabel.maximumNumberOfLines = 1
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        statusLabel.isHidden = false
        let content = CaptureBackground(frame: window.contentView?.bounds ?? .zero)
        content.waveform = waveform
        content.look = currentLook
        content.dropAccentColor = accentColor(accentRGB)
        content.onDropFiles = { [weak self] urls in
            self?.handleDroppedFiles(urls) ?? false
        }
        window.contentView = content
        background = content
        window.trackChrome(in: content)
        content.addSubview(waveform)
        content.addSubview(controls)
        content.addSubview(statusLabel)
        waveform.translatesAutoresizingMaskIntoConstraints = false
        controls.translatesAutoresizingMaskIntoConstraints = false
        statusLabel.translatesAutoresizingMaskIntoConstraints = false
        let toolbarTrailing = controls.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -panelVerticalInset)
        self.toolbarTrailing = toolbarTrailing
        let waveformLeading = waveform.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: panelVerticalInset)
        let waveformTrailing = waveform.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -panelVerticalInset)
        let statusHeight = statusLabel.heightAnchor.constraint(equalToConstant: statusStripHeight)
        self.statusHeightConstraint = statusHeight
        let waveformBottomWithStatus = waveform.bottomAnchor.constraint(equalTo: statusLabel.topAnchor, constant: -2)
        let waveformBottomWithoutStatus = waveform.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -frameInset)
        self.waveformLeading = waveformLeading
        self.waveformTrailing = waveformTrailing
        self.waveformBottomWithStatus = waveformBottomWithStatus
        self.waveformBottomWithoutStatus = waveformBottomWithoutStatus
        NSLayoutConstraint.activate([
            controls.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: panelVerticalInset),
            toolbarTrailing,
            controls.topAnchor.constraint(equalTo: content.topAnchor, constant: panelVerticalInset),
            waveformLeading, waveformTrailing, waveformBottomWithStatus,
            statusLabel.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: panelVerticalInset),
            statusLabel.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -panelVerticalInset),
            statusLabel.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: 0),
            statusHeight
        ])
        toolbar = controls
        waveformTopBelowToolbar = waveform.topAnchor.constraint(equalTo: controls.bottomAnchor, constant: 6)
        waveformTopBelowToolbar?.isActive = true
        waveformTopWithoutToolbar = waveform.topAnchor.constraint(equalTo: content.topAnchor, constant: frameInset)
        panel = window
        updateMeter()
        applyPin()
        if waveformOnly { window.contentMinSize = waveformOnlyPanelMinimum }
        let restoredFrame = !independentWindow && !testCopy && window.setFrameUsingName(frameAutosaveName, force: true)
        if !independentWindow && !testCopy { _ = window.setFrameAutosaveName(frameAutosaveName) }
        applyWaveformOnly(initial: true, restoredFrame: restoredFrame)
        if let sizeOverride = TestCopyConfig.current.windowSizeOverride {
            let minSize = window.contentMinSize
            let clamped = NSSize(width: max(minSize.width, sizeOverride.width), height: max(minSize.height, sizeOverride.height))
            window.setContentSize(clamped)
        }
        if let posOverride = TestCopyConfig.current.windowPositionOverride {
            let screenHeight = NSScreen.screens.first?.frame.height ?? 1080
            let appKitY = screenHeight - posOverride.y - window.frame.height
            window.setFrameOrigin(NSPoint(x: posOverride.x, y: appKitY))
        } else if !restoredFrame {
            window.center()
            if independentWindow {
                window.setFrameOrigin(NSPoint(x: window.frame.minX + 34, y: window.frame.minY - 34))
            }
        }
        if testCopy && TestCopyConfig.current.windowPositionOverride != nil {
            refreshWaveform()
            updateStatus()
            window.orderFrontRegardless()
            updateOptionsVisibility()
        } else {
            showPanel(window)
        }
    }

    private func keepPanelVisible(_ window: NSWindow) {
        let screens = NSScreen.screens
        if !screens.isEmpty && !screens.contains(where: {
            let visible = $0.visibleFrame.intersection(window.frame)
            return visible.width >= min(40, window.frame.width) && visible.height >= min(30, window.frame.height)
        }) {
            window.center()
        }
    }

    private func setIcon(_ symbol: String, label: String, on button: NSButton) {
        button.title = label
        button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: label)
        button.imagePosition = button.image == nil ? .noImage : .imageOnly
        button.toolTip = label
        button.setAccessibilityLabel(label)
    }

    @objc private func showOptions() {
        guard !editing else { return }
        registerClick()
        let window = optionsPanel ?? makeOptionsPanel()
        optionsPanel = window
        if !optionsOpenedOnce {
            optionsOpenedOnce = true
            optionCategory = 0
            if let first = optionCategoryButtons.first { selectOptionCategory(first) }
        }
        window.isFloatingPanel = alwaysOnTop
        window.level = alwaysOnTop ? .floating : .normal
        refreshSourceOptions()
        refreshSnapshotTargets()
        refreshShortcutPermission()
        updateOptionsVisibility()
        updateLoginOption()
        updateStatus()
        if testCopy {
            if let size = harness?.optionsSize {
                window.setContentSize(size)
            }
            window.orderFrontRegardless()
        } else {
            keepPanelVisible(window)
            NSApp.activate()
            window.makeKeyAndOrderFront(nil)
        }
    }

    private func makeOptionsPanel() -> HotkeyOptionsPanel {
        let initialSize = harness?.optionsSize ?? NSSize(width: 820, height: 640)
        let window = HotkeyOptionsPanel(
            contentRect: NSRect(origin: .zero, size: initialSize),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false
        )
        // Package 8 / LOOK-01 & Package 12 / OPT-01: dark appearance, 820x640, tracks pin level.
        window.appearance = NSAppearance(named: .darkAqua)
        window.title = TestCopyConfig.current.windowTitle(independentWindow ? "Backtracking Capture Options" : "Backtracking Options")
        window.recordKey = { [weak self] event in self?.recordHotkey(event) ?? false }
        window.cancelRecording = { [weak self] in self?.cancelHotkeyRecording() }
        window.canCloseWithEscape = { [weak self] in
            guard let self else { return true }
            if self.recordingShortcut != nil { return false }
            if NSColorPanel.shared.isVisible { return false }
            if self.optionsPanel?.firstResponder is NSTextView { return false }
            if self.hasOpenOptionsPopover() { return false }
            if self.cancelAddonPromptOnEscape() { return false }
            return true
        }
        window.contentMinSize = NSSize(width: 680, height: 460)
        window.isReleasedWhenClosed = false
        window.isFloatingPanel = alwaysOnTop
        window.level = alwaysOnTop ? .floating : .normal
        window.hidesOnDeactivate = false

        let content = OptionsDropContainerView(frame: .zero)
        content.onDropFiles = { [weak self] urls in
            self?.handleDroppedFiles(urls) ?? false
        }
        content.accentColorProvider = { [weak self] in
            accentColor(self?.accentRGB ?? defaultAccentRGB)
        }
        let tabs = NSTabView(frame: .zero)
        tabs.tabViewType = .noTabsNoBorder
        // Exact Windows order: Actions, Hotkeys, Capture, Sources, Appearance, Export, Add-ons, (Updates).
        var pages = [
            makeActionTab(), makeHotkeysTab(), makeCaptureTab(), makeSourceTab(),
            makeAppearanceTab(), makeExportTab(), makeAddonsTab()
        ]
        if !independentWindow { pages.append(makeUpdateTab()) }
        for page in pages { tabs.addTabViewItem(page) }
        optionTabs = tabs
        optionCategoryButtons = pages.enumerated().map { index, page in
            let button = NSButton(title: page.label, target: self, action: #selector(selectOptionCategory(_:)))
            button.setButtonType(.pushOnPushOff)
            button.bezelStyle = .recessed
            button.alignment = .left
            button.font = .systemFont(ofSize: 15, weight: .semibold)
            button.tag = index
            button.state = index == optionCategory ? .on : .off
            button.heightAnchor.constraint(equalToConstant: 36).isActive = true
            return button
        }
        tabs.selectTabViewItem(at: optionCategory)
        let sidebar = NSStackView(views: optionCategoryButtons)
        sidebar.orientation = .vertical
        sidebar.alignment = .leading
        sidebar.spacing = 4
        for button in optionCategoryButtons {
            button.widthAnchor.constraint(equalTo: sidebar.widthAnchor).isActive = true
        }
        let notice = NSTextField(labelWithString: "")
        notice.textColor = .secondaryLabelColor
        notice.font = .systemFont(ofSize: 14)
        notice.lineBreakMode = .byTruncatingMiddle
        notice.maximumNumberOfLines = 1
        notice.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        optionsNotice = notice
        let separator = NSBox(frame: .zero)
        separator.boxType = .separator
        let footerSeparator = NSBox(frame: .zero)
        footerSeparator.boxType = .separator
        let close = NSButton(title: "Close", target: window, action: #selector(NSWindow.performClose(_:)))
        close.bezelStyle = .rounded
        close.font = .systemFont(ofSize: 15)
        close.setContentHuggingPriority(.required, for: .horizontal)
        close.setContentCompressionResistancePriority(.required, for: .horizontal)
        for view in [sidebar as NSView, separator, tabs, footerSeparator, notice, close] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }
        NSLayoutConstraint.activate([
            sidebar.topAnchor.constraint(equalTo: content.topAnchor, constant: 20),
            sidebar.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 12),
            sidebar.widthAnchor.constraint(equalToConstant: 172),
            sidebar.bottomAnchor.constraint(lessThanOrEqualTo: footerSeparator.topAnchor, constant: -12),
            separator.leadingAnchor.constraint(equalTo: sidebar.trailingAnchor, constant: 10),
            separator.topAnchor.constraint(equalTo: content.topAnchor, constant: 12),
            separator.bottomAnchor.constraint(equalTo: footerSeparator.topAnchor, constant: -10),
            separator.widthAnchor.constraint(equalToConstant: 1),
            tabs.topAnchor.constraint(equalTo: content.topAnchor, constant: 12),
            tabs.leadingAnchor.constraint(equalTo: separator.trailingAnchor, constant: 10),
            tabs.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -12),
            tabs.bottomAnchor.constraint(equalTo: footerSeparator.topAnchor, constant: -10),
            footerSeparator.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 14),
            footerSeparator.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -14),
            footerSeparator.heightAnchor.constraint(equalToConstant: 1),
            footerSeparator.bottomAnchor.constraint(equalTo: close.topAnchor, constant: -10),
            notice.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 14),
            notice.trailingAnchor.constraint(equalTo: close.leadingAnchor, constant: -12),
            notice.heightAnchor.constraint(equalToConstant: 20),
            notice.centerYAnchor.constraint(equalTo: close.centerYAnchor),
            close.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -14),
            close.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -12),
            close.widthAnchor.constraint(greaterThanOrEqualToConstant: 72)
        ])
        window.contentView = content
        window.center()
        return window
    }

    /// Whether any Options list is open, so Escape closes it before the window.
    private func hasOpenOptionsPopover() -> Bool {
        OptionsPopUpButton.anyChoicePopoverShown
    }

    @objc private func selectOptionCategory(_ sender: NSButton) {
        cancelHotkeyRecording()
        optionCategory = sender.tag
        optionTabs?.selectTabViewItem(at: optionCategory)
        for button in optionCategoryButtons { button.state = button === sender ? .on : .off }
    }

    private func makeActionTab() -> NSTabViewItem {
        let capture = NSButton(title: optionsTransportLabel, target: self, action: #selector(toggleCapture(_:)))
        let cancel = NSButton(title: "Cancel selection", target: self, action: #selector(cancelWaveformSelection))
        let clear = NSButton(title: "Clear capture", target: self, action: #selector(clearCapture))
        let save = NSButton(title: "Save snapshot", target: self, action: #selector(saveCurrentSnapshot))
        let newWindow = NSButton(title: "New capture window", target: self, action: #selector(openReceiverWindow))
        let quit = NSButton(title: "Quit Backtracking", target: NSApp, action: #selector(NSApplication.terminate(_:)))
        optionsCaptureButton = capture
        optionsSaveButton = save
        optionsClearButton = clear
        optionsCancelSelectionButton = cancel
        let controls = optionGroup("Capture controls", rows: [buttonRow([capture, cancel, clear])])
        if independentWindow {
            // An extra capture window has no menu bar item to bring it back, so it closes instead
            // of hiding. The shortcut and the login item belong to the main window.
            quit.title = "Close capture window"
            return optionsTab("Actions", groups: [
                controls,
                optionGroup("Snapshots", rows: [buttonRow([save, newWindow])]),
                optionGroup("Application", rows: [buttonRow([quit])])
            ])
        }
        let visibility = NSButton(title: "Hide waveform", target: self, action: #selector(togglePanelVisibility))
        optionsVisibilityButton = visibility
        let login = NSButton(checkboxWithTitle: "Start at login", target: self, action: #selector(toggleLaunchAtLogin))
        optionsLoginButton = login
        let target = OptionsPopUpButton(frame: .zero, pullsDown: false)
        target.target = self
        target.action = #selector(snapshotTargetChanged(_:))
        target.autoenablesItems = false
        target.menu?.delegate = self
        prepareOptionPopup(target)
        snapshotTargetPopup = target
        refreshSnapshotTargets()
        let shortcut = NSTextField(labelWithString: globalShortcutTitle)
        snapshotShortcutLabel = shortcut
        let permission = NSButton(title: "Allow Input Monitoring", target: self, action: #selector(requestShortcutPermission))
        shortcutPermissionButton = permission
        refreshShortcutPermission()
        return optionsTab("Actions", groups: [
            controls,
            optionGroup("Snapshots", rows: [
                buttonRow([save, newWindow]),
                optionField("Global shortcut", shortcut, below: [optionControlRow(permission)]),
                optionField("Shortcut target", target)
            ]),
            optionGroup("Application", rows: [
                login, buttonRow([visibility, quit])
            ])
        ])
    }

    private func refreshSnapshotTargets() {
        guard let popup = snapshotTargetPopup else { return }
        popup.removeAllItems()
        popup.addItem(withTitle: "Desktop recorder")
        var selectedIndex = 0
        for target in availableSnapshotTargets() {
            // The end of the instance id tells two plug-ins on same-named tracks apart.
            let title = "\(target.name) (\(target.id.suffix(6)))"
            popup.addItem(withTitle: title)
            popup.lastItem?.representedObject = target.id
            popup.lastItem?.toolTip = title
            if snapshotTargetID == target.id { selectedIndex = popup.numberOfItems - 1 }
        }
        if let snapshotTargetID, selectedIndex == 0 {
            popup.addItem(withTitle: "Plugin unavailable")
            popup.lastItem?.representedObject = snapshotTargetID
            popup.lastItem?.isEnabled = false
            selectedIndex = popup.numberOfItems - 1
        }
        popup.selectItem(at: selectedIndex)
        popup.toolTip = popup.titleOfSelectedItem
    }

    @objc private func snapshotTargetChanged(_ sender: NSPopUpButton) {
        snapshotTargetID = sender.selectedItem?.representedObject as? String
        defaults.set(snapshotTargetID, forKey: "snapshotTargetID")
        sender.toolTip = sender.titleOfSelectedItem
    }

    private func dispatchSnapshotShortcut() {
        if let id = snapshotTargetID {
            if requestSenderSnapshot(id) {
                setNotice("Snapshot requested from the selected plugin.")
            } else {
                setNotice("Could not request plugin snapshot: the plugin did not accept the request.")
            }
        } else {
            saveCurrentSnapshot()
        }
    }

    private func refreshShortcutPermission() {
        // A test copy has no global shortcut, so it never asks for Input Monitoring.
        guard !testCopy else {
            shortcutPermissionButton?.isHidden = true
            return
        }
        let authorized = CGPreflightListenEventAccess()
        shortcutPermissionButton?.isHidden = snapshotShortcut.isListening || hotkeys[.globalSnapshot].isEmpty
        shortcutPermissionButton?.title = authorized ? "Reconnect the global shortcut" : "Allow Input Monitoring"
    }

    @objc private func requestShortcutPermission() {
        guard !testCopy else { return }
        listenAccessRequest()
        snapshotShortcut.start()
        refreshShortcutPermission()
    }

    private var globalShortcutTitle: String {
        let shortcuts = hotkeys[.globalSnapshot].map(\.title)
        return shortcuts.isEmpty ? "Off" : shortcuts.joined(separator: " / ")
    }

    private func makeHotkeyRow(action: RecorderAction, index: Int) -> NSView {
        let shortcuts = hotkeys[action]
        let isRecordingThis = recordingShortcut?.action == action
        var buttons = shortcuts.enumerated().map { position, shortcut -> NSButton in
            let button = NSButton(title: shortcut.title, target: self, action: #selector(startHotkeyRecording(_:)))
            if isRecordingThis, recordingShortcut?.index == position { button.title = "Press keys…" }
            button.tag = index * 2 + position
            return button
        }
        if shortcuts.count < 2 {
            let title = shortcuts.isEmpty ? "Not set" : "Add another"
            let add = NSButton(title: title, target: self, action: #selector(startHotkeyRecording(_:)))
            if isRecordingThis, recordingShortcut?.index == shortcuts.count { add.title = "Press keys…" }
            add.tag = index * 2 + shortcuts.count
            buttons.append(add)
        }
        let clear = NSButton(title: "Clear", target: self, action: #selector(clearHotkey(_:)))
        clear.tag = index
        clear.isEnabled = !shortcuts.isEmpty
        let reset = NSButton(title: "Reset", target: self, action: #selector(resetHotkey(_:)))
        reset.tag = index
        reset.isEnabled = shortcuts != action.presets

        var below: [NSView] = []
        var extraButtons = [clear, reset]
        // Package 11 / HK-19: offer "Use a tap of Right Command" while recording Edit separately.
        if isRecordingThis && action == .separateLanes {
            let rightCmd = NSButton(title: "Use a tap of Right Command", target: self, action: #selector(useRightCommandTap))
            extraButtons.insert(rightCmd, at: 0)
        }
        below.append(optionControlRow(buttonRow(extraButtons)))
        if isRecordingThis {
            below.append(optionNote("Press the new keys. Escape stops without a change, so Escape itself comes back only with Reset."))
        }
        if hotkeyErrorAction == action, let err = hotkeyErrorText {
            let errNote = optionNote(err)
            errNote.textColor = WaveformTheme.statusUnavailable
            below.append(errNote)
        }
        return optionField(action.title, buttonRow(buttons), below: below)
    }

    private func makeHotkeysTab() -> NSTabViewItem {
        var groups: [NSView] = []
        var introRows: [NSView] = [optionNote("Click a shortcut, then press the keys you want. Changes apply at once.")]
        if independentWindow {
            introRows.append(optionNote("Changes made here apply to this window only and are not kept."))
        }
        groups.append(optionGroup(nil, rows: introRows))

        if !independentWindow {
            let idx = RecorderAction.allCases.firstIndex(of: .globalSnapshot) ?? 0
            groups.append(optionGroup("System-wide", rows: [
                optionNote("Works while another app is in front. Requires Input Monitoring."),
                makeHotkeyRow(action: .globalSnapshot, index: idx)
            ]))
        }

        var windowRows: [NSView] = [
            optionNote("These work while the Backtracking window is in front. With a selection, the key that pauses plays the selection and the key that clears cancels it.")
        ]
        for (index, action) in RecorderAction.allCases.enumerated() where action != .globalSnapshot {
            windowRows.append(makeHotkeyRow(action: action, index: index))
        }
        let resetAll = NSButton(title: "Reset all", target: self, action: #selector(resetAllHotkeys))
        windowRows.append(buttonRow([resetAll]))
        groups.append(optionGroup("In the Backtracking window", rows: windowRows))

        // Package 11 / HK-23: 6 fixed mouse gesture lines (Mac-adapted modifiers).
        let mouseGestures: [(String, String)] = [
            ("Option / Control + drag", "Free selection"),
            ("Shift + drag", "Selection that snaps to flags, bar lines or seconds lines"),
            ("Option + drag", "Move the window (outside waveform, or ⌘⌥ anywhere); on a selection's flag, fade that end"),
            ("Mouse wheel", "Zoom; over the side margins of a track whose rows do not fit, scroll its rows"),
            ("Middle-drag", "Pan while zoomed in"),
            ("Right-click", "Open the menu")
        ]
        let mouseRows: [NSView] = mouseGestures.map { gesture, desc in
            let descLabel = optionNote(desc)
            descLabel.textColor = .labelColor
            return optionField(gesture, descLabel)
        }
        groups.append(optionGroup("Mouse (fixed)", rows: mouseRows))
        return optionsTab("Hotkeys", groups: groups)
    }

    @objc private func startHotkeyRecording(_ sender: NSButton) {
        let action = RecorderAction.allCases[sender.tag / 2]
        recordingShortcut = (action, sender.tag % 2)
        snapshotShortcut.recording = true
        hotkeyErrorAction = nil
        hotkeyErrorText = nil
        reloadHotkeysTab()
    }

    @objc private func useRightCommandTap() {
        guard let editing = recordingShortcut, editing.action == .separateLanes else { return }
        var modified = hotkeys[.separateLanes]
        if modified.indices.contains(editing.index) { modified[editing.index] = .rightCommand }
        else { modified.append(.rightCommand) }
        if let reason = hotkeys.set(modified, for: .separateLanes) {
            hotkeyErrorAction = .separateLanes
            hotkeyErrorText = reason
            reloadHotkeysTab()
            return
        }
        finishHotkeyEdit()
    }

    private func recordHotkey(_ event: NSEvent) -> Bool {
        guard let editing = recordingShortcut else { return false }
        guard !event.isARepeat else { return true }
        if event.keyCode == UInt16(kVK_Escape) {
            cancelHotkeyRecording()
            return true
        }
        let shortcut = RecorderShortcut(event: event)
        var modified = hotkeys[editing.action]
        if modified.indices.contains(editing.index) { modified[editing.index] = shortcut }
        else { modified.append(shortcut) }
        if let reason = hotkeys.set(modified, for: editing.action) {
            hotkeyErrorAction = editing.action
            hotkeyErrorText = reason
            reloadHotkeysTab()
            return true
        }
        finishHotkeyEdit()
        return true
    }

    private func cancelHotkeyRecording() {
        guard recordingShortcut != nil else { return }
        recordingShortcut = nil
        snapshotShortcut.recording = false
        hotkeyErrorAction = nil
        hotkeyErrorText = nil
        reloadHotkeysTab()
    }

    @objc private func clearHotkey(_ sender: NSButton) { changeHotkey(sender.tag, reset: false) }

    @objc private func resetHotkey(_ sender: NSButton) { changeHotkey(sender.tag, reset: true) }

    private func changeHotkey(_ index: Int, reset: Bool) {
        let action = RecorderAction.allCases[index]
        if let reason = hotkeys.set(reset ? action.presets : [], for: action) {
            hotkeyErrorAction = action
            hotkeyErrorText = reason
            reloadHotkeysTab()
            return
        }
        finishHotkeyEdit()
    }

    @objc private func resetAllHotkeys() {
        hotkeys = RecorderHotkeys()
        finishHotkeyEdit()
    }

    private func finishHotkeyEdit() {
        recordingShortcut = nil
        hotkeyErrorAction = nil
        hotkeyErrorText = nil
        snapshotShortcut.recording = false
        defaults.set(hotkeys.data, forKey: "recorderHotkeys")
        snapshotShortcut.bindings = hotkeys[.globalSnapshot]
        if !independentWindow && !testCopy {
            if snapshotShortcut.bindings.isEmpty { snapshotShortcut.stop() }
            else { snapshotShortcut.start() }
        }
        snapshotShortcutLabel?.stringValue = globalShortcutTitle
        refreshShortcutPermission()
        reloadHotkeysTab()
    }

    private func reloadHotkeysTab() {
        guard optionTabs != nil else { return }
        replaceOptionsTab(with: makeHotkeysTab())
    }

    private func replaceOptionsTab(with page: NSTabViewItem) {
        guard let tabs = optionTabs,
              let index = tabs.tabViewItems.firstIndex(where: { $0.label == page.label }) else { return }
        let previous = tabs.tabViewItem(at: index)
        let selected = tabs.selectedTabViewItem === previous
        let offset = (previous.view as? NSScrollView)?.contentView.bounds.origin ?? .zero
        tabs.insertTabViewItem(page, at: index)
        if selected { tabs.selectTabViewItem(page) }
        tabs.removeTabViewItem(previous)
        if selected, let scroll = page.view as? NSScrollView {
            scroll.layoutSubtreeIfNeeded()
            scroll.contentView.scroll(to: offset)
            scroll.reflectScrolledClipView(scroll.contentView)
        }
    }

    private func makeCaptureTab() -> NSTabViewItem {
        let capacity = optionNote("")
        captureCapacityNote = capacity
        updateCaptureCapacity()
        let quietSections = NSButton(
            checkboxWithTitle: "Keep quiet sections", target: self, action: #selector(retainSilenceChanged(_:))
        )
        quietSections.state = retainSilence ? .on : .off
        let hideSilent = NSButton(
            checkboxWithTitle: "Hide silent channels", target: self, action: #selector(hideSilentChannelsChanged(_:))
        )
        hideSilent.state = hideSilentChannels ? .on : .off
        optionsHideSilentButton = hideSilent
        let hideSilentNote = optionNote(
            "System, application and microphone sources show below L and R only the channels that have sound. Add-on sources show every channel."
        )
        return optionsTab("Capture", groups: [
            optionGroup(nil, rows: [
                optionChoice(
                    "Retain the latest audio", choices: timeChoices, selected: history,
                    action: #selector(historyChanged(_:))
                ) { $0 > 60 ? "\($0 / 60) minutes" : "\($0) seconds" },
                capacity,
                quietSections,
                optionChoice(
                    "Auto-resume delay", choices: autoResumeChoices, selected: idleSeconds,
                    action: #selector(idleChanged(_:))
                ) { $0 == 0 ? "Off" : "\($0) seconds" }
            ]),
            optionGroup("Multichannel sources", rows: [
                hideSilent,
                hideSilentNote,
                optionField("L/R of multichannel sources",
                    choiceControl([0, 1], selected: pairBinaural ? 1 : 0, action: #selector(pairBinauralChanged(_:))) {
                        $0 == 0 ? "Stereo" : "Binaural"
                    },
                    below: [optionNote("How L and R sound in preview and on export. Binaural places each channel around your head, for headphones. The waveform is the same either way.")])
            ])
        ])
    }

    @objc private func hideSilentChannelsChanged(_ sender: NSButton) {
        let enabled = sender.state == .on
        guard enabled != hideSilentChannels else { return }
        hideSilentChannels = enabled
        defaults.set(enabled, forKey: "hideSilentChannels")
        waveform?.hideSilentChannels = enabled
        waveform?.discardPreparedFile()
        refreshWaveform()
        updateStatus()
        registerClick()
    }

    private func makeExportTab() -> NSTabViewItem {
        let mono = NSButton(
            checkboxWithTitle: "Export every file as mono", target: self, action: #selector(exportMonoChanged(_:))
        )
        mono.state = forceMonoExport ? .on : .off
        let wide = optionField("Multichannel sources",
            choiceControl(WideExportMode.allCases.map(\.rawValue), selected: wideExportMode.rawValue,
                          action: #selector(wideExportModeChanged(_:))) {
                WideExportMode(rawValue: $0)?.label ?? ""
            },
            below: [optionNote("Binaural files let you hear on headphones where each channel sits.")])
        var folders: [NSView] = [folderOptions(.output)]
        // Other capture windows share the folder but leave its housekeeping to the main one.
        if !independentWindow { folders.append(cleanupOptions()) }
        folders.append(folderOptions(.saved))
        return optionsTab("Export", groups: [
            optionGroup(nil, rows: [
                optionChoice(
                    "WAV sample rate", choices: exportSampleRates, selected: exportSampleRate,
                    action: #selector(exportSampleRateChanged(_:))
                ) { $0 == 44_100 ? "44.1 kHz" : "\($0 / 1_000) kHz" },
                optionChoice(
                    "WAV encoding", choices: exportBitDepths, selected: exportBitsPerSample,
                    action: #selector(exportEncodingChanged(_:))
                ) { $0 == 32 ? "32-bit float" : "\($0)-bit PCM" },
                mono, wide
            ]),
            optionGroup(nil, rows: folders)
        ])
    }

    private func cleanupOptions() -> NSView {
        let popup = choiceControl(
            cleanupAgeChoices, selected: cleanup.map { bt_cleanup_days($0) } ?? 0,
            action: #selector(cleanupChanged(_:))
        ) { $0 == 0 ? "Off" : "After \($0) days" }
        let note = optionNote("")
        cleanupNote = note
        updateCleanupNote()
        return optionField("Delete old drag-and-drop files", popup, below: [note])
    }

    /// The drag-and-drop folder when old exports may be deleted from it. Saved snapshots are
    /// never deleted, so a folder that also holds them is left alone.
    private func cleanupFolder() -> String? {
        let output = directory(for: .output)
        return bt_same_folder(output, directory(for: .saved)) != 0 ? nil : output
    }

    private func updateCleanupNote() {
        guard let cleanupNote else { return }
        let days = cleanup.map { bt_cleanup_days($0) } ?? 0
        if days == 0 {
            cleanupNote.stringValue = "Files you drag into other programs stay in this folder."
        } else if cleanupFolder() == nil {
            cleanupNote.stringValue = "Not active while this is also the saved snapshots folder."
        } else {
            cleanupNote.stringValue = "Backtracking's WAV files in this folder are permanently deleted "
                + "\(days) days after they were written. A project that still uses one loses it."
        }
    }

    @objc private func cleanupChanged(_ sender: NSPopUpButton) {
        guard let cleanup, let days = selectedValue(sender), cleanupAgeChoices.contains(days) else { return }
        // A new choice waits before its first pass, so a wrong pick can be put right first.
        bt_cleanup_set_days(cleanup, days)
        // Off is stored as 0, like the other timed choices.
        defaults.set(Int(days), forKey: "outputCleanupDays")
        updateCleanupNote()
        registerClick()
    }

    private func cleanOutputFolder() {
        // Only the main window cleans, and only when the user chose an age.
        guard let cleanup, bt_cleanup_days(cleanup) != 0 else { return }
        var folder: String?
        var keep: [UnsafeMutablePointer<CChar>?] = []
        // The folder check reads the disk, so it waits until a pass is due.
        if bt_cleanup_due(cleanup) != 0 {
            folder = cleanupFolder()
            // A file still on offer for a drag stays, however long it has waited.
            keep = (waveform?.offeredFiles ?? []).map { strdup($0.path) }
        }
        defer { for file in keep { free(file) } }
        let files = keep.map { UnsafePointer<CChar>($0) }
        let poll: (UnsafePointer<CChar>?) -> Int64 = { path in
            files.withUnsafeBufferPointer { bt_cleanup_poll(cleanup, path, $0.baseAddress, $0.count) }
        }
        let removed = folder.map { $0.withCString(poll) } ?? poll(nil)
        switch removed {
        case -2, 0: break
        case -1: setNotice("Could not clean the drag-and-drop folder")
        case 1: setNotice("Deleted 1 old file from the drag-and-drop folder")
        default: setNotice("Deleted \(removed) old files from the drag-and-drop folder")
        }
    }

    private func startBackgroundSourceScan() {
        if sourceScanInFlight {
            sourceScanQueued = true
            return
        }
        sourceScanInFlight = true
        sourceScanQueued = false
        let excluded = tap?.privatePlaybackDevices ?? []
        let apps = snapshotRunningApps()
        let addonSnap = currentAddonSnapshot
        sourceScanQueue.async { [weak self] in
            let choices = scanCaptureSources(excluding: excluded, runningApps: apps, addonSnapshot: addonSnap)
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.sourceScanInFlight = false
                self.cachedSourceChoices = choices
                self.sourceScanError = nil
                self.reloadOpenSourcePopups(choices: choices)
                if self.sourceScanQueued {
                    self.sourceScanQueued = false
                    self.startBackgroundSourceScan()
                }
            }
        }
    }

    private func reloadOpenSourcePopups(choices: [(source: CaptureSource, title: String)]) {
        for popup in sourcePopups {
            refreshSourcePopup(popup, choices: choices, scanning: false)
            popup.reloadChoicesIfOpen()
        }
        if let add = addSourcePopup {
            refreshSourcePopup(add, choices: choices, scanning: false)
            add.reloadChoicesIfOpen()
        }
    }

    private func makeSourceTab() -> NSTabViewItem {
        sourcePopups.removeAll()
        routePopups.removeAll()
        fileNameFields.removeAll()
        fileNameNotes.removeAll()
        fileNameSourceKeys.removeAll()
        sourceProblemNotes.removeAll()
        addonTransportTimeLabels.removeAll()
        addonTransportPlayButtons.removeAll()
        addonTransportSliders.removeAll()
        addonTransportSeekNotes.removeAll()
        var groups: [NSView] = []
        if let choice = addonPendingDropChoice {
            let fileName = choice.url.lastPathComponent
            let prompt = optionNote("Which add-on should open \(fileName)?")
            prompt.textColor = .labelColor
            var buttons: [NSButton] = choice.openers.enumerated().map { idx, opener in
                let b = NSButton(title: opener.name, target: self, action: #selector(pickAddonDropOpener(_:)))
                b.tag = idx
                return b
            }
            buttons.append(NSButton(title: "Cancel", target: self, action: #selector(cancelAddonDropOpener)))
            groups.append(optionGroup(nil, rows: [prompt, buttonRow(buttons)]))
        }
        for (index, configuration) in captureLanes.enumerated() {
            let label = captureLaneLetter(index)
            let source = OptionsPopUpButton(frame: .zero, pullsDown: false)
            source.target = self
            source.tag = index
            source.action = #selector(captureSourceChanged(_:))
            source.autoenablesItems = false
            source.menu?.delegate = self
            prepareOptionPopup(source)
            sourcePopups.append(source)
            let route = choiceControl(
                CaptureRoute.allCases.map { $0.rawValue }, selected: configuration.route.rawValue,
                action: #selector(captureRouteChanged(_:))
            ) { CaptureRoute(rawValue: $0)?.label ?? "" }
            route.tag = index
            route.autoenablesItems = false
            routePopups.append(route)
            // The name belongs to the device, so it follows it to the other lane.
            let name = NSTextField(string: sourceFileNames[configuration.source.key] ?? "")
            name.placeholderString = defaultFileName(for: configuration.source)
            name.tag = index
            name.delegate = self
            fileNameFields.append(name)
            fileNameSourceKeys.append(configuration.source.key)
            let note = optionNote("")
            fileNameNotes.append(note)
            let problemNote = optionNote("")
            problemNote.textColor = WaveformTheme.statusUnavailable
            problemNote.isHidden = true
            sourceProblemNotes.append(problemNote)
            let sourceField = optionField("Audio source", source)
            let routeField = optionField("Channel route", route)
            let nameField = optionField("File name", name, below: [note])
            source.setAccessibilityLabel("Source \(label) audio source")
            route.setAccessibilityLabel("Source \(label) channel route")
            name.setAccessibilityLabel("Source \(label) file name")
            var rows: [NSView] = [sourceField]
            let isWideAddon: Bool
            if case .addon(let addonID, let sourceID) = configuration.source {
                let addonRows = makeAddonSourceLaneRows(laneIndex: index, addonID: addonID, sourceID: sourceID)
                isWideAddon = addonRows.hideRoute
                if !isWideAddon { rows.append(routeField) }
                rows.append(contentsOf: addonRows.views)
            } else {
                isWideAddon = false
                rows.append(routeField)
            }
            rows.append(nameField)
            if !configuration.source.isAddon || currentAddonSnapshot.source(
                addonID: { if case .addon(let a, _) = configuration.source { return a }; return "" }(),
                sourceID: { if case .addon(_, let s) = configuration.source { return s }; return "" }()
            ) != nil {
                rows.append(problemNote)
            }
            if dualSource {
                let remove = NSButton(title: "Remove", target: self, action: #selector(removeSource(_:)))
                remove.tag = index
                remove.toolTip = "Remove source \(label). The sources after it move up."
                remove.setAccessibilityLabel("Remove source \(label)")
                rows.append(buttonRow([remove]))
            }
            groups.append(optionGroup("Source \(label)", rows: rows))
        }
        let add = OptionsPopUpButton(frame: .zero, pullsDown: false)
        add.target = self
        add.action = #selector(addSource(_:))
        add.autoenablesItems = false
        add.menu?.delegate = self
        add.tag = -1
        prepareOptionPopup(add)
        addSourcePopup = add
        let fullNote = optionNote("64 sources is the most the app can hold.")
        fullNote.isHidden = captureLanes.count < maximumCaptureLanes
        sourcesFullNote = fullNote
        let capacity = optionNote("")
        sourcesCapacityNote = capacity
        let channelsFull = optionNote("All 64 channels are in use. A source added now cannot start.")
        channelsFull.textColor = WaveformTheme.statusUnavailable
        channelsFull.isHidden = true
        sourcesChannelsFullNote = channelsFull
        updateCaptureCapacity()
        let restartNote = dualSource
            ? "Adding, removing or changing a source starts a new history. Removing a source moves the ones after it up a letter."
            : "Adding, removing or changing a source starts a new history."
        groups.append(optionGroup(nil, rows: [
            optionField("Add source", add), fullNote, capacity, channelsFull,
            optionNote(restartNote)
        ]))
        return optionsTab("Sources", groups: groups)
    }

    /// Typing in a File name field names that lane's source, or gives it back its own name when
    /// the field is empty. Capped at 64 most-recently-named sources (OPT-22).
    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField, let index = fileNameFields.firstIndex(of: field),
              index < fileNameSourceKeys.count else { return }
        if field.stringValue.count > sourceNameLimit {
            field.stringValue = String(field.stringValue.prefix(sourceNameLimit))
        }
        let key = fileNameSourceKeys[index]
        let name = String(field.stringValue.map { $0.isNewline ? " " : $0 })
        sourceFileNameOrder.removeAll { $0 == key }
        if name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            sourceFileNames[key] = nil
        } else {
            sourceFileNames[key] = name
            sourceFileNameOrder.append(key)
            trimSourceFileNamesIfNeeded()
        }
        defaults.set(sourceFileNames, forKey: sourceFileNamesKey)
        defaults.set(sourceFileNameOrder, forKey: sourceFileNameOrderKey)
        refreshFileNameNotes()
    }

    /// Shows under each File name field how that lane's files will begin.
    private func refreshFileNameNotes() {
        for (index, lane) in captureLanes.enumerated() where index < fileNameNotes.count {
            let letter: UInt32 = dualSource ? UInt32(index + 1) : 0
            fileNameNotes[index].stringValue = "Files start with \(filePrefix(fileName(for: lane.source), lane: letter))"
        }
    }

    /// How a new capture window is launched. A test copy opens another window of the same test
    /// copy: its test variables go along, and the marker makes the window refuse to start without them.
    static func receiverWindowLaunch(
        testCopy: Bool, environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> (arguments: [String], environment: [String: String]) {
        guard testCopy else { return ([receiverWindowArgument], [:]) }
        return ([receiverWindowArgument, TestCopyConfig.testCopyLaunchArgument],
                TestCopyConfig.inheritedTestEnvironment(environment))
    }

    @objc private func openReceiverWindow() {
        let configuration = NSWorkspace.OpenConfiguration()
        let launch = Self.receiverWindowLaunch(testCopy: testCopy)
        configuration.arguments = launch.arguments
        if !launch.environment.isEmpty { configuration.environment = launch.environment }
        configuration.createsNewApplicationInstance = true
        configuration.activates = false
        configuration.addsToRecentItems = false
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration) {
            [weak self] _, error in
            if let error {
                let detail = error.localizedDescription
                DispatchQueue.main.async { self?.setNotice("Could not open capture window: \(detail)") }
            }
        }
    }

    private func refreshSourceOptions() {
        updateCaptureCapacity()
        guard sourcePopups.count == captureLanes.count, routePopups.count == captureLanes.count else { return }
        let choices = cachedSourceChoices ?? availableCaptureSources(excluding: tap?.privatePlaybackDevices ?? [])
        cachedSourceChoices = choices
        for (index, lane) in captureLanes.enumerated() {
            refreshSourcePopup(sourcePopups[index], choices: choices, scanning: false)
            let route = routePopups[index]
            let wideSender = lane.source.isSenderBacked && (tap?.channelCount(for: index) ?? 0) > 2
            route.isEnabled = !editing && !wideSender
            route.selectItem(withTag: Int((wideSender ? CaptureRoute.merged : lane.route).rawValue))
            for item in route.itemArray {
                guard let raw = UInt32(exactly: item.tag), let selection = CaptureRoute(rawValue: raw) else { continue }
                if wideSender {
                    item.isEnabled = selection == .merged
                } else {
                    item.isEnabled = selection == lane.route || tap?.supports(selection, lane: index) == true
                        || (tap == nil && selection != .center && selection != .allChannels)
                }
            }
            if index < sourceProblemNotes.count {
                let problem = tap?.sourceProblem(for: index)
                sourceProblemNotes[index].stringValue = problem ?? ""
                sourceProblemNotes[index].isHidden = problem == nil
            }
        }
        for (index, lane) in captureLanes.enumerated() where index < fileNameFields.count && index < fileNameSourceKeys.count {
            let field = fileNameFields[index]
            field.placeholderString = defaultFileName(for: lane.source)
            // A field being typed in keeps what it shows, unless its lane now records another
            // source: what was typed is already saved for the old one.
            if fileNameSourceKeys[index] != lane.source.key {
                fileNameSourceKeys[index] = lane.source.key
                if field.currentEditor() != nil { _ = field.abortEditing() }
            }
            if field.currentEditor() == nil { field.stringValue = sourceFileNames[lane.source.key] ?? "" }
        }
        refreshFileNameNotes()
        waveformModePopup?.isEnabled = !dualSource
        let full = captureLanes.count >= maximumCaptureLanes
        sourcesFullNote?.isHidden = !full
        if let addSourcePopup {
            refreshSourcePopup(addSourcePopup, choices: choices, scanning: false)
            addSourcePopup.isEnabled = !full && !editing
            addSourcePopup.toolTip = full
                ? "64 sources is the most the app can hold."
                : "The source you choose is added at the end of the list and recorded with the others."
        }
    }

    private func updateCaptureCapacity() {
        guard let tap else {
            sourcesCapacityNote?.isHidden = true
            sourcesChannelsFullNote?.isHidden = true
            captureCapacityNote?.isHidden = true
            return
        }
        let kept = Int(tap.retainedSeconds)
        let channels = tap.totalChannelCount
        let shortNote = kept < history ? "With \(channels) channels, memory holds the latest \(kept) seconds." : nil
        sourcesCapacityNote?.isHidden = false
        sourcesCapacityNote?.stringValue = ["\(channels) of 64 channels in use", shortNote].compactMap { $0 }.joined(separator: ". ")
        sourcesChannelsFullNote?.isHidden = !(channels >= 64 && captureLanes.count < maximumCaptureLanes)
        captureCapacityNote?.isHidden = shortNote == nil
        captureCapacityNote?.stringValue = shortNote ?? ""
    }

    private func refreshSourcePopup(_ popup: NSPopUpButton, choices: [(source: CaptureSource, title: String)], scanning: Bool = false) {
        let adding = popup.tag == -1
        let full = captureLanes.count >= maximumCaptureLanes
        let configured = captureLanes.indices.contains(popup.tag) ? captureLanes[popup.tag].source : nil
        popup.removeAllItems()
        var selectedIndex: Int?
        if adding {
            popup.addItem(withTitle: full ? "No more sources" : "Choose a source")
            popup.lastItem?.isEnabled = false
            selectedIndex = 0
        } else if popup.tag > 0 {
            popup.addItem(withTitle: "Off")
            popup.lastItem?.representedObject = offSourceKey
        }
        if scanning {
            popup.addItem(withTitle: "⟳ Scanning audio sources…")
            popup.lastItem?.isEnabled = false
        }
        if let err = sourceScanError {
            popup.addItem(withTitle: "Could not list audio sources: \(err)")
            popup.lastItem?.isEnabled = false
        }
        if !adding { popup.isEnabled = !editing }
        var uses: [String: Int] = [:]
        for choice in choices { uses[choice.title, default: 0] += 1 }
        for choice in choices {
            var title = choice.title
            var detail = title
            switch choice.source {
            case .sender(let id):
                // The end of the instance id tells two plug-ins on same-named tracks apart.
                if uses[title, default: 0] > 1 { title += " (\(id.suffix(6)))" }
                detail = "Sender instance \(id)"
            case .application(let identifier):
                if uses[title, default: 0] > 1 { title += " · \(identifier)" }
                detail = identifier
            default:
                if uses[title, default: 0] > 1 { title += " · \(choice.source.key)" }
                detail = title
            }
            popup.addItem(withTitle: title)
            popup.lastItem?.representedObject = choice.source.key
            popup.lastItem?.toolTip = detail
            if selectedIndex == nil, configured == choice.source { selectedIndex = popup.numberOfItems - 1 }
        }
        if selectedIndex == nil, let configured {
            popup.addItem(withTitle: "\(configured.label) (unavailable)")
            popup.lastItem?.isEnabled = false
        }
        popup.selectItem(at: selectedIndex ?? max(0, popup.numberOfItems - 1))
        if !adding { popup.toolTip = popup.titleOfSelectedItem }
    }

    @objc private func captureSourceChanged(_ sender: NSPopUpButton) {
        guard !editing, let key = sender.selectedItem?.representedObject as? String else {
            refreshSourceOptions()
            return
        }
        guard captureLanes.indices.contains(sender.tag) else { return }
        if key == offSourceKey {
            removeLane(at: sender.tag)
            return
        }
        guard let source = CaptureSource(key: key) else { refreshSourceOptions(); return }
        let lane = CaptureLane(source: source)
        guard captureLanes[sender.tag].source != source, canConfigure(lane, at: sender.tag) else { return }
        captureLanes[sender.tag] = lane
        storeLanes()
    }

    @objc private func captureRouteChanged(_ sender: NSPopUpButton) {
        guard !editing, captureLanes.indices.contains(sender.tag),
              let value = selectedValue(sender), let route = CaptureRoute(rawValue: value) else {
            refreshSourceOptions()
            return
        }
        var lane = captureLanes[sender.tag]
        guard lane.route != route else { return }
        lane.route = route
        guard canConfigure(lane, at: sender.tag) else { return }
        captureLanes[sender.tag] = lane
        storeLanes()
    }

    @objc private func addSource(_ sender: NSPopUpButton) {
        guard !editing, captureLanes.count < maximumCaptureLanes,
              let key = sender.selectedItem?.representedObject as? String,
              let source = CaptureSource(key: key) else { refreshSourceOptions(); return }
        let lane = CaptureLane(source: source)
        guard canConfigure(lane, at: captureLanes.count) else { return }
        captureLanes.append(lane)
        meterModes.append(.dbfs)
        storeLanes(rebuildOptions: true)
    }

    private func canConfigure(_ lane: CaptureLane, at index: Int) -> Bool {
        var widths = captureLanes.enumerated().map { current, lane in
            tap?.channelCount(for: current) ?? SystemAudioTap.estimatedWidth(for: lane)
        }
        let width = SystemAudioTap.estimatedWidth(for: lane)
        if widths.indices.contains(index) { widths[index] = width }
        else { widths.append(width) }
        let count = widths.reduce(0, +)
        guard count <= 64 else {
            setNotice("Source \(captureLaneLetter(index)) would bring the total to \(count) channels. The limit is 64.")
            refreshSourceOptions()
            return false
        }
        return true
    }

    @objc private func removeSource(_ sender: NSButton) { removeLane(at: sender.tag) }

    private func removeLane(at index: Int) {
        guard !editing, dualSource, captureLanes.indices.contains(index) else { return }
        captureLanes.remove(at: index)
        meterModes.remove(at: index)
        storeLanes(rebuildOptions: true)
    }

    private func storeLanes(rebuildOptions: Bool = false) {
        defaults.set(captureLanes.map { ["source": $0.source.key, "route": Int($0.route.rawValue)] }, forKey: "captureLanes")
        saveMeterModes()
        if rebuildOptions, optionTabs != nil {
            replaceOptionsTab(with: makeSourceTab())
            replaceOptionsTab(with: makeAppearanceTab())
        }
        reconnectConfiguredSources()
    }

    private func reconnectConfiguredSources() {
        let hadPending = !pendingSnapshots.isEmpty
        if hadPending {
            failPendingSnapshots("The capture source changed before this selection was ready.")
        }
        let wasRunning = !paused
        if wasRunning { pauseCapture() }
        editing = false
        tap = nil
        waveform?.resetTimeline()
        if !hadPending { notice = nil }
        captureError = nil
        registerClick()
        if wasRunning {
            resume()
        } else {
            do {
                tap = try SystemAudioTap(history: history, lanes: captureLanes, retainSilence: retainSilence)
                rememberSourceNames()
            } catch { captureError = error.localizedDescription }
        }
        if !multipleRows { separateLanes = false }
        waveform?.separateLanes = separateLanes
        refreshWaveform()
        refreshSourceOptions()
        updateStatus()
    }

    @objc private func waveformModeChanged(_ sender: NSPopUpButton) {
        guard let value = selectedValue(sender) else { return }
        stereoWaveform = value == 1
        defaults.set(stereoWaveform, forKey: "stereoWaveform")
        waveform?.stereo = stereoWaveform
        if !multipleRows {
            separateLanes = false
            waveform?.separateLanes = false
        }
        refreshWaveform()
    }

    private func makeAppearanceTab() -> NSTabViewItem {
        func checkbox(_ title: String, _ on: Bool, _ action: Selector) -> NSButton {
            let button = NSButton(checkboxWithTitle: title, target: self, action: action)
            button.state = on ? .on : .off
            return button
        }
        let waveform = checkbox("Show waveform only", waveformOnly, #selector(toggleWaveformOnly))
        optionsWaveformButton = waveform
        let pin = checkbox("Keep on top", alwaysOnTop, #selector(pinChanged(_:)))
        optionsPinButton = pin
        let accent = NSButton(title: "Choose…", target: self, action: #selector(chooseAccentColor))
        let resetAccent = NSButton(title: "Reset", target: self, action: #selector(resetAccentColor))

        let lookPopup = OptionsPopUpButton(frame: .zero, pullsDown: false)
        lookPopup.target = self
        lookPopup.action = #selector(lookAddonChanged(_:))
        lookPopup.autoenablesItems = false
        prepareOptionPopup(lookPopup)
        lookAddonPopup = lookPopup
        let lookNote = optionNote("")
        lookAddonNote = lookNote
        refreshLookAddonPopup()

        let marks = StatusMark.allCases
        let mark = choiceControl(
            marks.indices.map { UInt32($0) }, selected: UInt32(marks.firstIndex(of: statusMark) ?? 0),
            action: #selector(statusMarkChanged(_:))
        ) { marks[Int($0)].label }

        let interval = choiceControl(
            secondsLineChoices, selected: secondsInterval, action: #selector(secondsIntervalChanged(_:))
        ) { $0 == 1 ? "1 second" : "\($0) seconds" }
        secondsIntervalPopup = interval

        let width = choiceControl(
            borderWidthChoices, selected: outlineWidth, action: #selector(outlineWidthChanged(_:))
        ) { "\($0) px" }
        let borderLookNote = optionNote("The chosen look sets its own width, which is used while that look is on.")
        borderLookNote.isHidden = currentLook.borderWidth == nil
        lookBorderWidthNote = borderLookNote
        let theme = checkbox("Use theme color", !customOutline, #selector(outlineThemeChanged(_:)))
        let outlineColor = NSButton(title: "Choose…", target: self, action: #selector(chooseOutlineColor))
        outlineColorButton = outlineColor
        let follows = checkbox(
            "Follow background opacity", outlineFollowsOpacity, #selector(outlineOpacityChanged(_:))
        )
        borderDependents = [width, theme, outlineColor, follows]

        let stereo = choiceControl(
            [0, 1], selected: stereoWaveform ? 1 : 0, action: #selector(waveformModeChanged(_:))
        ) { $0 == 0 ? "Merged" : "Left / Right" }
        waveformModePopup = stereo
        optionsMeterPopups = []
        let meterFields: [NSView] = meterModes.enumerated().map { lane, mode -> NSView in
            let meter = choiceControl(
                MeterMode.allCases.map(\.rawValue), selected: mode.rawValue, action: #selector(meterChanged(_:))
            ) { MeterMode(rawValue: $0)?.label ?? "" }
            meter.tag = lane
            optionsMeterPopups.append(meter)
            var below: [NSView] = []
            if mode == .lufs && (tap?.channelCount(for: lane) ?? 0) > 2 {
                below.append(optionNote("Loudness is measured for mono and stereo sources only."))
            }
            return optionField(dualSource ? "Meter \(captureLaneLetter(lane))" : "Meter", meter, below: below)
        }
        let coloring = choiceControl([0, 1], selected: frequencyColoring ? 1 : 0,
            action: #selector(frequencyColoringChanged(_:))) { $0 == 0 ? "Single color" : "By frequency" }
        let frequencyControls = optionsStack([], spacing: 8)
        frequencyGradientControls = frequencyControls
        refreshFrequencyGradientControls()
        // Package 8 & 20 / OPT-16: Window group includes Look from add-on, Waveform color & Frequency colors.
        let tab = optionsTab("Appearance", groups: [
            optionGroup("Window", rows: [
                waveform,
                pin,
                optionChoice(
                    "Background transparency", choices: transparencyChoices, selected: transparency,
                    action: #selector(transparencyChanged(_:))
                ) { "\($0)%" },
                optionField("Theme color", buttonRow([accent, resetAccent])),
                optionField("Look from add-on", lookPopup, below: [lookNote]),
                optionField("Waveform color", coloring),
                frequencyControls
            ]),
            optionGroup("Show", rows: [
                checkbox("App name in the toolbar", showsTitle, #selector(showTitleChanged(_:))),
                checkbox("Level meters", showsMeters, #selector(showMetersChanged(_:))),
                optionField("Capture status mark", mark)
            ]),
            optionGroup("Timeline guides", rows: [
                checkbox("Show bar lines", showsBars, #selector(barsChanged(_:))),
                checkbox("Show seconds lines", showsSeconds, #selector(secondsChanged(_:))),
                optionField("Seconds interval", interval)
            ]),
            optionGroup("Waveform border", rows: [
                checkbox("Show a border", showsOutline, #selector(outlineChanged(_:))),
                optionField("Border width", width, below: [borderLookNote]),
                theme,
                optionField("Custom border color", outlineColor),
                follows
            ]),
            optionGroup("Display", rows: [
                optionField("Single-source waveform", stereo)
            ] + meterFields)
        ])
        updateAppearanceDependencies()
        return tab
    }

    /// Package 9 & 20 / TL-41..43: Frequency colors editor matching Windows spectrum.rs (user > look > default).
    private func refreshFrequencyGradientControls() {
        guard let controls = frequencyGradientControls else { return }
        for view in controls.arrangedSubviews {
            controls.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        frequencyStopPercentLabels.removeAll()
        frequencyIllegibleNote = nil
        controls.isHidden = !frequencyColoring
        guard frequencyColoring else { return }

        let activeGradient = effectiveFrequencyGradient()
        let lookOfferer = lookSpectrumOffererName()
        let preview = FrequencyGradientPreview(frame: .zero)
        preview.gradient = activeGradient
        preview.look = currentLook
        preview.heightAnchor.constraint(equalToConstant: 16).isActive = true
        preview.setAccessibilityElement(true)
        preview.setAccessibilityLabel("Waveform gradient from low to high frequency")
        frequencyGradientPreview = preview

        let lowLabel = optionNote("Low")
        let highLabel = optionNote("High")
        highLabel.alignment = .right
        let endSpacer = NSView(frame: .zero)
        endSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let endsRow = NSStackView(views: [lowLabel, endSpacer, highLabel])
        endsRow.orientation = .horizontal
        endsRow.alignment = .centerY

        var editorRows: [NSView] = [preview, endsRow]
        let stops = activeGradient.stops
        let count = stops.count
        for (index, stop) in stops.enumerated() {
            let color = NSColorWell(frame: .zero)
            color.color = accentColor(stop.rgb)
            color.tag = index
            color.target = self
            color.action = #selector(frequencyStopColorChanged(_:))
            color.widthAnchor.constraint(equalToConstant: 44).isActive = true
            color.heightAnchor.constraint(equalToConstant: 24).isActive = true
            color.setAccessibilityLabel("Frequency color \(index + 1)")
            if index == 0 || index == count - 1 {
                let endName = NSTextField(labelWithString: index == 0 ? "Low" : "High")
                endName.font = .systemFont(ofSize: 14)
                let row = NSStackView(views: [color, endName])
                row.orientation = .horizontal
                row.alignment = .centerY
                row.spacing = 8
                editorRows.append(row)
            } else {
                let minPct = stops[index - 1].position * 100.0
                let maxPct = stops[index + 1].position * 100.0
                let slider = NSSlider(
                    value: min(maxPct, max(minPct, stop.position * 100.0)),
                    minValue: minPct, maxValue: max(minPct + 0.001, maxPct),
                    target: self, action: #selector(frequencyStopPositionChanged(_:))
                )
                slider.tag = index
                slider.isContinuous = true
                slider.toolTip = "Where this color sits between the lowest and the highest frequencies"
                slider.setAccessibilityLabel("Frequency stop \(index + 1) position")
                let pctLabel = NSTextField(labelWithString: "\(Int((stop.position * 100.0).rounded())) %")
                pctLabel.font = .monospacedDigitSystemFont(ofSize: 13, weight: .regular)
                pctLabel.widthAnchor.constraint(equalToConstant: 42).isActive = true
                frequencyStopPercentLabels[index] = pctLabel
                let remove = NSButton(title: "Remove", target: self, action: #selector(removeFrequencyStop(_:)))
                remove.tag = index
                let row = NSStackView(views: [color, slider, pctLabel, remove])
                row.orientation = .horizontal
                row.alignment = .centerY
                row.spacing = 8
                editorRows.append(row)
            }
        }
        let add = NSButton(title: "Add color", target: self, action: #selector(addFrequencyStop))
        add.isEnabled = count < 8
        let reset = NSButton(title: "Reset", target: self, action: #selector(resetFrequencyGradient))
        reset.isEnabled = hasCustomFrequencyGradient
        editorRows.append(buttonRow([add, reset]))
        editorRows.append(optionNote("Each moment of the waveform mixes these colors by how much of its sound is low or high."))
        if let lookOfferer {
            if hasCustomFrequencyGradient {
                editorRows.append(optionNote("Your own colors are used instead of the ones from the look of \(lookOfferer). Reset uses the look's colors again."))
            } else {
                editorRows.append(optionNote("These colors come from the look of \(lookOfferer). Changing one makes them your own."))
            }
        }
        let (_, replacedAny) = currentLook.legibleSpectrum(for: activeGradient)
        let illegible = optionNote("A color too hard to see on the waveform's background is drawn in the default color instead.")
        illegible.textColor = WaveformTheme.statusUnavailable
        illegible.isHidden = !replacedAny
        frequencyIllegibleNote = illegible
        editorRows.append(illegible)

        let stack = optionsStack(editorRows, spacing: 6)
        let field = optionField("Frequency colors", stack)
        controls.addArrangedSubview(field)
        field.widthAnchor.constraint(equalTo: controls.widthAnchor).isActive = true
    }

    @objc private func frequencyColoringChanged(_ sender: NSPopUpButton) {
        frequencyColoring = selectedValue(sender) == 1
        defaults.set(frequencyColoring, forKey: "waveformColorByFrequency")
        waveform?.frequencyGradient = frequencyColoring ? effectiveFrequencyGradient() : nil
        refreshFrequencyGradientControls()
        refreshWaveform()
    }

    private func storeFrequencyGradient(persist: Bool = true, custom: Bool = true, refreshControls: Bool = false) {
        hasCustomFrequencyGradient = custom
        if persist {
            if custom {
                defaults.set(frequencyGradient.settings, forKey: "waveformFrequencyGradient")
            } else {
                defaults.removeObject(forKey: "waveformFrequencyGradient")
            }
        }
        let active = effectiveFrequencyGradient()
        frequencyGradientPreview?.gradient = active
        let (_, replaced) = currentLook.legibleSpectrum(for: active)
        frequencyIllegibleNote?.isHidden = !replaced
        waveform?.frequencyGradient = frequencyColoring ? active : nil
        if refreshControls { refreshFrequencyGradientControls() }
    }

    @objc private func frequencyStopColorChanged(_ sender: NSColorWell) {
        if !hasCustomFrequencyGradient {
            frequencyGradient = effectiveFrequencyGradient()
        }
        guard frequencyGradient.stops.indices.contains(sender.tag),
              let color = sender.color.usingColorSpace(.sRGB) else { return }
        func component(_ value: CGFloat) -> UInt32 {
            UInt32((min(1, max(0, value)) * 255).rounded())
        }
        let wasCustom = hasCustomFrequencyGradient
        frequencyGradient.stops[sender.tag].rgb = (component(color.redComponent) << 16)
            | (component(color.greenComponent) << 8) | component(color.blueComponent)
        let mouseUp = NSApp.currentEvent?.type != .leftMouseDragged
        storeFrequencyGradient(persist: mouseUp, custom: true, refreshControls: !wasCustom)
    }

    @objc private func frequencyStopPositionChanged(_ sender: NSSlider) {
        if !hasCustomFrequencyGradient {
            frequencyGradient = effectiveFrequencyGradient()
        }
        let index = sender.tag
        let stops = frequencyGradient.stops
        guard index > 0 && index < stops.count - 1 else { return }
        let lo = stops[index - 1].position
        let hi = stops[index + 1].position
        let pos = min(hi, max(lo, sender.doubleValue / 100.0))
        let wasCustom = hasCustomFrequencyGradient
        frequencyGradient.stops[index].position = pos
        frequencyStopPercentLabels[index]?.stringValue = "\(Int((pos * 100.0).rounded())) %"
        let isDragging = NSApp.currentEvent?.type == .leftMouseDragged
        storeFrequencyGradient(persist: !isDragging, custom: true, refreshControls: !isDragging && !wasCustom)
        if !isDragging {
            // Update neighbor slider bounds once the user releases the drag.
            refreshFrequencyGradientControls()
        }
    }

    @objc private func removeFrequencyStop(_ sender: NSButton) {
        if !hasCustomFrequencyGradient {
            frequencyGradient = effectiveFrequencyGradient()
        }
        let index = sender.tag
        guard frequencyGradient.stops.count > 2, index > 0 && index < frequencyGradient.stops.count - 1 else { return }
        frequencyGradient.stops.remove(at: index)
        storeFrequencyGradient(persist: true, custom: true, refreshControls: true)
    }

    @objc private func addFrequencyStop() {
        if !hasCustomFrequencyGradient {
            frequencyGradient = effectiveFrequencyGradient()
        }
        var added = frequencyGradient.addingStop()
        added.stops.sort { $0.position < $1.position }
        frequencyGradient = added
        storeFrequencyGradient(persist: true, custom: true, refreshControls: true)
    }

    @objc private func resetFrequencyGradient() {
        frequencyGradient = .standard
        storeFrequencyGradient(persist: true, custom: false, refreshControls: true)
    }

    /// Greys out the settings that do nothing while the one above them is off.
    private func updateAppearanceDependencies() {
        secondsIntervalPopup?.isEnabled = showsSeconds
        for control in borderDependents { control.isEnabled = showsOutline }
        outlineColorButton?.isEnabled = showsOutline && customOutline
    }

    private func makeUpdateTab() -> NSTabViewItem {
        let tab = startupUpdater.makeOptionsTab()
        // Keep the updater's own controls and callbacks, laid out like the other pages.
        func takeRows(from stack: NSStackView) -> [NSView] {
            let children = stack.arrangedSubviews
            var rows: [NSView] = []
            for child in children {
                stack.removeArrangedSubview(child)
                child.removeFromSuperview()
                if let actions = child as? NSStackView {
                    let buttons = actions.arrangedSubviews.compactMap { $0 as? NSButton }
                    for button in buttons {
                        actions.removeArrangedSubview(button)
                        button.removeFromSuperview()
                    }
                    rows.append(buttonRow(buttons))
                } else {
                    rows.append(child)
                }
            }
            return rows
        }
        let rows: [NSView]
        if let stack = tab.view as? NSStackView {
            rows = takeRows(from: stack)
        } else {
            rows = tab.view.map { [$0] } ?? []
        }
        return optionsTab(tab.label, groups: [optionGroup(nil, rows: rows)])
    }

    /// One Options page: its name, a rule, then its groups with a rule between them.
    private func optionsTab(_ title: String, groups: [NSView]) -> NSTabViewItem {
        let tab = NSTabViewItem(identifier: title)
        tab.label = title
        let heading = NSTextField(labelWithString: title)
        heading.font = .systemFont(ofSize: 22, weight: .bold)
        var rows: [NSView] = [heading]
        for group in groups {
            let rule = NSBox(frame: .zero)
            rule.boxType = .separator
            rule.heightAnchor.constraint(equalToConstant: 1).isActive = true
            rows.append(rule)
            rows.append(group)
        }
        let stack = optionsStack(rows, spacing: 12)
        let content = OptionsDocumentView(frame: .zero)
        let scroll = NSScrollView(frame: .zero)
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.setAccessibilityLabel("\(title) options")
        content.translatesAutoresizingMaskIntoConstraints = false
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = content
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            content.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            content.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            content.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            stack.topAnchor.constraint(equalTo: content.topAnchor, constant: 16),
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
            stack.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -20)
        ])
        tab.view = scroll
        return tab
    }

    private func optionsStack(_ views: [NSView], spacing: CGFloat) -> NSStackView {
        let stack = NSStackView(views: views)
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = spacing
        for view in views {
            view.translatesAutoresizingMaskIntoConstraints = false
            view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        return stack
    }

    /// A set of related settings. Pages with a single set give it no name.
    private func optionGroup(_ title: String?, rows: [NSView]) -> NSStackView {
        var views = rows.map { optionControlRow($0) }
        if let title {
            let heading = NSTextField(labelWithString: title)
            heading.font = .systemFont(ofSize: 16, weight: .bold)
            views.insert(heading, at: 0)
        }
        return optionsStack(views, spacing: 10)
    }

    private func optionControlRow(_ control: NSView) -> NSView {
        prepareOptionsText(control)
        control.translatesAutoresizingMaskIntoConstraints = false
        control.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let row = NSStackView(views: [control])
        row.orientation = .vertical
        row.alignment = .leading
        row.spacing = 0
        row.detachesHiddenViews = true
        // Lists, values and fields take the full width; buttons keep their own.
        if control is NSPopUpButton || control is NSTextField
            || (control is NSStackView && !(control is OptionButtonRow)) {
            control.widthAnchor.constraint(equalTo: row.widthAnchor).isActive = true
        }
        return row
    }

    private func buttonRow(_ buttons: [NSButton]) -> NSStackView {
        let row = OptionButtonRow(views: buttons)
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 8
        return row
    }

    private func optionNote(_ text: String) -> NSTextField {
        let note = NSTextField(wrappingLabelWithString: text)
        note.font = .systemFont(ofSize: 13)
        note.textColor = .secondaryLabelColor
        note.isSelectable = false
        note.identifier = optionNoteIdentifier
        return note
    }

    private func optionField(_ title: String, _ control: NSView, below: [NSView] = []) -> NSStackView {
        optionField(NSTextField(labelWithString: title), control, below: below)
    }

    /// A named setting: the name on the left and its control on the right, with any note or
    /// extra buttons under the control.
    private func optionField(_ label: NSTextField, _ control: NSView, below: [NSView] = []) -> NSStackView {
        label.font = .systemFont(ofSize: 15)
        label.lineBreakMode = .byTruncatingTail
        label.maximumNumberOfLines = 1
        label.toolTip = label.stringValue
        control.setAccessibilityLabel(label.stringValue)
        if let text = control as? NSTextField {
            text.lineBreakMode = .byTruncatingMiddle
            text.maximumNumberOfLines = 1
            text.toolTip = text.stringValue
        }
        // The name is centred on the first line of the control beside it.
        let name = NSView(frame: .zero)
        label.translatesAutoresizingMaskIntoConstraints = false
        name.addSubview(label)
        let column = optionsStack([optionControlRow(control)] + below, spacing: 5)
        column.setHuggingPriority(.defaultLow, for: .horizontal)
        let row = NSStackView(views: [name, column])
        row.orientation = .horizontal
        row.distribution = .fill
        row.alignment = .top
        row.spacing = 12
        let isVerticalBlock = (control is NSStackView) && !(control is OptionButtonRow)
        NSLayoutConstraint.activate([
            name.widthAnchor.constraint(equalToConstant: optionLabelWidth),
            isVerticalBlock
                ? name.heightAnchor.constraint(equalToConstant: 20)
                : name.heightAnchor.constraint(equalTo: control.heightAnchor),
            label.leadingAnchor.constraint(equalTo: name.leadingAnchor),
            label.trailingAnchor.constraint(lessThanOrEqualTo: name.trailingAnchor),
            label.centerYAnchor.constraint(equalTo: name.centerYAnchor)
        ])
        return row
    }

    private func optionChoice(
        _ title: String, choices: [UInt32], selected: UInt32, action: Selector, label: (UInt32) -> String
    ) -> NSStackView {
        optionField(title, choiceControl(choices, selected: selected, action: action, label: label))
    }

    private func choiceControl(
        _ choices: [UInt32], selected: UInt32, action: Selector, label: (UInt32) -> String
    ) -> NSPopUpButton {
        let popup = OptionsPopUpButton(frame: .zero, pullsDown: false)
        for choice in choices {
            popup.addItem(withTitle: label(choice))
            popup.lastItem?.tag = Int(choice)
        }
        popup.selectItem(withTag: Int(selected))
        popup.target = self
        popup.action = action
        prepareOptionPopup(popup)
        return popup
    }

    private func prepareOptionPopup(_ popup: NSPopUpButton) {
        popup.font = .systemFont(ofSize: 15)
        popup.cell?.lineBreakMode = .byTruncatingMiddle
        popup.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        popup.setContentHuggingPriority(.defaultLow, for: .horizontal)
    }

    private func prepareOptionsText(_ view: NSView) {
        // Notes stay smaller than the settings they explain.
        if let control = view as? NSControl, control.identifier != optionNoteIdentifier,
           (control.font?.pointSize ?? 0) < 14 {
            control.font = .systemFont(ofSize: 15)
        }
        if let stack = view as? NSStackView {
            for child in stack.arrangedSubviews { prepareOptionsText(child) }
        }
    }

    private func folderOptions(_ folder: ExportFolder) -> NSStackView {
        let choose = NSButton(title: "Choose folder…", target: self, action: #selector(chooseFolder(_:)))
        let reveal = NSButton(title: "Open folder", target: self, action: #selector(openFolder(_:)))
        choose.tag = folder.rawValue
        reveal.tag = folder.rawValue
        choose.setAccessibilityLabel("Choose the \(folder.label.lowercased())")
        reveal.setAccessibilityLabel("Open the \(folder.label.lowercased())")
        let path = NSTextField(labelWithString: directory(for: folder))
        path.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        if folder == .output { outputPathLabel = path } else { savedPathLabel = path }
        return optionField(folder.label, path, below: [optionControlRow(buttonRow([choose, reveal]))])
    }

    private func updateOptionsVisibility() {
        optionsVisibilityButton?.title = panel?.isVisible == true ? "Hide waveform" : "Show waveform"
    }

    @objc private func showMeterOptions(_ sender: NSButton) {
        showMenu(meterMenu(lane: sender.tag), below: sender)
    }

    private func meterMenu(lane: Int) -> NSMenu {
        let menu = NSMenu(title: "Meter")
        for mode in MeterMode.allCases {
            let item = NSMenuItem(title: mode.label, action: #selector(meterChanged(_:)), keyEquivalent: "")
            item.tag = Int(mode.rawValue)
            item.target = self
            item.representedObject = lane
            item.state = meterModes.indices.contains(lane) && mode == meterModes[lane] ? .on : .off
            menu.addItem(item)
        }
        return menu
    }

    private func showMenu(_ menu: NSMenu, below sender: NSButton) {
        let below = sender.isFlipped ? sender.bounds.maxY + 3 : sender.bounds.minY - 3
        _ = menu.popUp(positioning: nil, at: NSPoint(x: sender.bounds.minX, y: below), in: sender)
    }

    private func quitItem(shortcut: String = "") -> NSMenuItem {
        let item = NSMenuItem(title: "Quit Backtracking", action: #selector(NSApplication.terminate(_:)), keyEquivalent: shortcut)
        item.target = NSApp
        return item
    }

    private func captureMenuItem() -> NSMenuItem {
        let item = NSMenuItem(title: menuTransportLabel, action: #selector(toggleCapture(_:)), keyEquivalent: "")
        item.target = self
        item.representedObject = paused
        return item
    }

    private func showContextMenu(with event: NSEvent) {
        guard let view = panel?.contentView else { return }
        NSMenu.popUpContextMenu(makeContextMenu(), with: event, for: view)
        // The menu ran its own event loop. Time spent reading it is not idle time.
        registerClick()
    }

    /// The recorder window's right-click menu: the everyday commands, in the Windows order.
    private func makeContextMenu() -> NSMenu {
        let menu = NSMenu(title: "Backtracking")
        menu.autoenablesItems = false
        // A waveform gesture owns the capture until it ends, as for the toolbar buttons.
        let captureControls = !editing
        @discardableResult
        func add(
            _ title: String, _ action: Selector, target: AnyObject? = nil,
            enabled: Bool = true, checked: Bool = false
        ) -> NSMenuItem {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = target ?? self
            item.isEnabled = enabled
            item.state = checked ? .on : .off
            menu.addItem(item)
            return item
        }
        func showKeys(_ action: RecorderAction, on item: NSMenuItem) {
            let title = item.title
            let keys = hotkeys[action].map(\.title).joined(separator: " / ")
            guard !keys.isEmpty else { return }
            item.title = "\(title)    \(keys)"
            item.setAccessibilityLabel("\(title), \(keys)")
        }
        let transport = add(menuTransportLabel, #selector(toggleCapture(_:)), enabled: captureControls)
        transport.representedObject = paused
        showKeys(.toggleCapture, on: transport)
        showKeys(.clear, on: add("Clear", #selector(clearCapture), enabled: captureControls))
        showKeys(.saveSnapshot, on: add("Save snapshot", #selector(saveCurrentSnapshot), enabled: captureControls))
        menu.addItem(.separator())
        showKeys(.separateLanes, on: add("Edit separately", #selector(toggleSeparateLanes), enabled: multipleRows,
            checked: multipleRows && separateLanes))
        showKeys(.waveformOnly, on: add("Show waveform only", #selector(toggleWaveformOnly), checked: waveformOnly))
        showKeys(.keepOnTop, on: add("Keep on top", #selector(togglePin), checked: alwaysOnTop))
        menu.addItem(.separator())
        showKeys(.openOptions, on: add("Options…", #selector(showOptions)))
        showKeys(.quit, on: add("Quit", #selector(NSApplication.terminate(_:)), target: NSApp))
        return menu
    }

    /// Runs one of the recorder window's own keys through the same methods as the toolbar.
    private func run(_ action: RecorderAction) -> Bool {
        if action == .cancelSelection {
            return waveform?.cancelSelection() == true || waveform?.cancelPendingDrag() == true
        }
        guard !editing else { return true }
        switch action {
        case .toggleCapture:
            if waveform?.togglePreview() != true { toggleCapture(nil) }
        case .clear:
            if waveform?.cancelSelection() != true { clearCapture() }
        case .separateLanes: toggleSeparateLanes()
        case .saveSnapshot: saveCurrentSnapshot()
        case .waveformOnly: toggleWaveformOnly()
        case .keepOnTop: togglePin()
        case .openOptions: showOptions()
        case .quit: NSApp.terminate(nil)
        case .cancelSelection, .globalSnapshot: return false
        }
        return true
    }

    @objc private func toggleSeparateLanes() {
        guard !editing else { return }
        guard multipleRows else {
            setNotice("Editing separately needs more than one row.")
            return
        }
        separateLanes.toggle()
        waveform?.separateLanes = separateLanes
        registerClick()
        setNotice(separateLanes ? "Editing rows separately" : "Editing all rows together")
    }

    private func loginItem() -> NSMenuItem {
        if testCopy {
            let item = NSMenuItem(title: "Start at login", action: #selector(toggleLaunchAtLogin), keyEquivalent: "")
            item.target = self
            item.state = .off
            return item
        }
        let status = SMAppService.mainApp.status
        let title = status == .requiresApproval ? "Start at login (needs approval)" : "Start at login"
        let item = NSMenuItem(title: title, action: #selector(toggleLaunchAtLogin), keyEquivalent: "")
        item.target = self
        item.state = status == .enabled ? .on : (status == .requiresApproval ? .mixed : .off)
        return item
    }

    @objc private func toggleLaunchAtLogin() {
        if testCopy {
            setNotice("A test copy does not change the macOS login item.")
            updateLoginOption()
            return
        }
        defer { updateLoginOption() }
        let service = SMAppService.mainApp
        do {
            if service.status == .enabled || service.status == .requiresApproval {
                try service.unregister()
            } else {
                try service.register()
                if service.status == .requiresApproval { SMAppService.openSystemSettingsLoginItems() }
            }
        } catch {
            if service.status == .requiresApproval {
                SMAppService.openSystemSettingsLoginItems()
            } else {
                let alert = NSAlert(error: error)
                alert.messageText = "Could not update startup setting"
                NSApp.activate()
                _ = alert.runModal()
            }
        }
    }

    private func updateLoginOption() {
        if testCopy {
            optionsLoginButton?.allowsMixedState = false
            optionsLoginButton?.title = "Start at login"
            optionsLoginButton?.state = .off
            return
        }
        let status = SMAppService.mainApp.status
        optionsLoginButton?.allowsMixedState = status == .requiresApproval
        optionsLoginButton?.title = status == .requiresApproval ? "Start at login (needs approval)" : "Start at login"
        optionsLoginButton?.state = status == .enabled ? .on : (status == .requiresApproval ? .mixed : .off)
    }

    @objc private func chooseAccentColor() {
        registerClick()
        let picker = NSColorPanel.shared
        picker.color = accentColor(accentRGB)
        picker.showsAlpha = false
        picker.isContinuous = true
        picker.setTarget(self)
        picker.setAction(#selector(accentColorChanged(_:)))
        NSApp.activate()
        picker.makeKeyAndOrderFront(nil)
    }

    @objc private func accentColorChanged(_ picker: NSColorPanel) {
        guard let color = picker.color.usingColorSpace(.sRGB) else { return }
        func channel(_ value: CGFloat) -> UInt32 {
            UInt32((min(1, max(0, value)) * 255).rounded())
        }
        let rgb = (channel(color.redComponent) << 16) | (channel(color.greenComponent) << 8) | channel(color.blueComponent)
        setAccent(rgb)
    }

    @objc private func resetAccentColor() {
        registerClick()
        setAccent(defaultAccentRGB)
    }

    private func setAccent(_ rgb: UInt32) {
        guard rgb != accentRGB else { return }
        accentRGB = rgb
        defaults.set(Int(rgb), forKey: "accentRGB")
        waveform?.accentRGB = rgb
        background?.dropAccentColor = accentColor(rgb)
        recomputeLook(refreshAppearanceTab: false)
        updateBackground()
    }

    @objc private func showTitleChanged(_ sender: NSButton) {
        showsTitle = sender.state == .on
        defaults.set(showsTitle, forKey: "showsTitle")
        titleLabel.isHidden = !showsTitle
    }

    @objc private func showMetersChanged(_ sender: NSButton) {
        showsMeters = sender.state == .on
        defaults.set(showsMeters, forKey: "showsMeters")
        updateMeter()
    }

    @objc private func statusMarkChanged(_ sender: NSPopUpButton) {
        let marks = StatusMark.allCases
        guard let index = selectedValue(sender), marks.indices.contains(Int(index)) else { return }
        statusMark = marks[Int(index)]
        defaults.set(statusMark.rawValue, forKey: "statusMark")
        updateStatus()
    }

    @objc private func barsChanged(_ sender: NSButton) {
        showsBars = sender.state == .on
        defaults.set(showsBars, forKey: "showsBars")
        updateBackground()
        refreshWaveform()
    }

    @objc private func secondsChanged(_ sender: NSButton) {
        showsSeconds = sender.state == .on
        defaults.set(showsSeconds, forKey: "showsSeconds")
        updateAppearanceDependencies()
        updateBackground()
    }

    @objc private func secondsIntervalChanged(_ sender: NSPopUpButton) {
        guard let interval = selectedValue(sender), secondsLineChoices.contains(interval) else { return }
        secondsInterval = interval
        defaults.set(Int(interval), forKey: "secondsInterval")
        updateBackground()
    }

    @objc private func outlineChanged(_ sender: NSButton) {
        showsOutline = sender.state == .on
        defaults.set(showsOutline, forKey: "showsWaveformOutline")
        updateAppearanceDependencies()
        applyFrameInset()
        updateBackground()
    }

    @objc private func outlineWidthChanged(_ sender: NSPopUpButton) {
        guard let width = selectedValue(sender), borderWidthChoices.contains(width) else { return }
        outlineWidth = width
        defaults.set(Int(width), forKey: "waveformOutlineWidth")
        applyFrameInset()
        updateBackground()
    }

    /// Room around the bare waveform in waveform-only mode: one point inset.
    private var frameInset: CGFloat { waveformFrameInset }

    /// Keeps room around the bare waveform in waveform-only mode. The window keeps its size.
    private func applyFrameInset() {
        guard waveformOnly else { return }
        waveformTopWithoutToolbar?.constant = frameInset
        waveformLeading?.constant = frameInset
        waveformTrailing?.constant = -frameInset
        waveformBottomWithoutStatus?.constant = -frameInset
        waveformMinimumHeight?.constant = waveformOnlyPanelMinimum.height - 2 * frameInset
    }

    @objc private func outlineOpacityChanged(_ sender: NSButton) {
        outlineFollowsOpacity = sender.state == .on
        defaults.set(outlineFollowsOpacity, forKey: "waveformOutlineFollowsOpacity")
        updateBackground()
    }

    @objc private func outlineThemeChanged(_ sender: NSButton) {
        customOutline = sender.state == .off
        defaults.set(customOutline, forKey: "customWaveformOutline")
        updateAppearanceDependencies()
        updateBackground()
    }

    @objc private func chooseOutlineColor() {
        let picker = NSColorPanel.shared
        picker.color = accentColor(outlineRGB)
        picker.showsAlpha = false
        picker.isContinuous = true
        picker.setTarget(self)
        picker.setAction(#selector(outlineColorChanged(_:)))
        picker.makeKeyAndOrderFront(nil)
    }

    @objc private func outlineColorChanged(_ picker: NSColorPanel) {
        guard let color = picker.color.usingColorSpace(.sRGB) else { return }
        func channel(_ value: CGFloat) -> UInt32 {
            UInt32((min(1, max(0, value)) * 255).rounded())
        }
        let red = channel(color.redComponent)
        let green = channel(color.greenComponent)
        let blue = channel(color.blueComponent)
        outlineRGB = (red << 16) | (green << 8) | blue
        defaults.set(Int(outlineRGB), forKey: "waveformOutlineRGB")
        updateBackground()
    }

    private func selectedValue(_ sender: NSPopUpButton) -> UInt32? {
        guard let tag = sender.selectedItem?.tag else { return nil }
        return UInt32(exactly: tag)
    }

    @objc private func historyChanged(_ sender: NSPopUpButton) {
        guard let selected = selectedValue(sender), timeChoices.contains(selected), !historyChangePending else { return }
        registerClick()
        let previous = history
        history = selected
        defaults.set(Int(history), forKey: "history")
        guard let tap else { return }
        historyChangePending = true
        sender.isEnabled = false
        tap.setHistory(selected) { [weak self, weak tap, weak sender] succeeded in
            guard let self else { return }
            self.historyChangePending = false
            sender?.isEnabled = true
            if !succeeded, self.tap === tap {
                self.history = previous
                self.defaults.set(Int(previous), forKey: "history")
                sender?.selectItem(withTag: Int(previous))
                self.setNotice("Could not change how much audio is retained")
            }
            self.updateCaptureCapacity()
            self.refreshWaveform()
            if self.waveform?.clampSelectionToHistory() == false {
                self.resumeAfterSelectionCancellation()
            }
        }
    }

    @objc private func idleChanged(_ sender: NSPopUpButton) {
        guard let selected = selectedValue(sender), autoResumeChoices.contains(selected) else { return }
        idleSeconds = selected
        defaults.set(Int(idleSeconds), forKey: autoResumePreferenceKey)
        registerClick()
    }

    @objc private func retainSilenceChanged(_ sender: NSButton) {
        retainSilence = sender.state == .on
        defaults.set(retainSilence, forKey: "retainSilence")
        tap?.setRetainSilence(retainSilence)
        registerClick()
    }

    @objc private func transparencyChanged(_ sender: NSPopUpButton) {
        guard let selected = selectedValue(sender), transparencyChoices.contains(selected) else { return }
        transparency = selected
        defaults.set(Int(selected), forKey: "backgroundTransparency")
        registerClick()
        updateBackground()
    }

    private func updateBackground() {
        let opacity = 1 - CGFloat(transparency) / 100
        panel?.isOpaque = false
        panel?.backgroundColor = .clear
        panel?.ignoresMouseEvents = false
        panel?.hasShadow = !waveformOnly && opacity > 0
        background?.backgroundOpacity = opacity
        background?.waveformOnly = waveformOnly
        background?.look = currentLook
        background?.showsOutline = false
        let effectiveBorderColor = currentLook.effectiveBorderColor(
            accent: RGBA8(rgb: accentRGB),
            userBorderRGB: customOutline ? outlineRGB : nil
        ).nsColor
        let effectiveBorderWidth = currentLook.effectiveBorderWidth(userWidth: Int(outlineWidth))
        waveform?.look = currentLook
        waveform?.showsOutline = showsOutline
        waveform?.outlineWidth = effectiveBorderWidth
        waveform?.outlineColor = effectiveBorderColor.withAlphaComponent(outlineFollowsOpacity ? opacity : 1)
        waveform?.gridOpacity = opacity
        waveform?.showsBars = showsBars
        waveform?.secondsInterval = showsSeconds ? secondsInterval : nil
    }

    private func saveMeterModes() {
        defaults.set(meterModes.map { Int($0.rawValue) }, forKey: "meterModes")
        if let first = meterModes.first { defaults.set(Int(first.rawValue), forKey: "meterMode") }
        if meterModes.count > 1 { defaults.set(Int(meterModes[1].rawValue), forKey: "meterModeB") }
    }

    @objc private func meterChanged(_ sender: Any) {
        let tag = (sender as? NSPopUpButton)?.selectedItem?.tag ?? (sender as? NSMenuItem)?.tag
        guard let tag, let raw = UInt32(exactly: tag), let mode = MeterMode(rawValue: raw) else { return }
        let lane = (sender as? NSPopUpButton)?.tag ?? ((sender as? NSMenuItem)?.representedObject as? Int) ?? 0
        guard meterModes.indices.contains(lane) else { return }
        meterModes[lane] = mode
        saveMeterModes()
        if optionsMeterPopups.indices.contains(lane) { optionsMeterPopups[lane].selectItem(withTag: tag) }
        registerClick()
        updateMeter()
    }

    @objc private func exportSampleRateChanged(_ sender: NSPopUpButton) {
        guard let rate = selectedValue(sender), exportSampleRates.contains(rate), rate != exportSampleRate else { return }
        exportSampleRate = rate
        defaults.set(Int(rate), forKey: "exportSampleRate")
        invalidateExportFormat()
    }

    @objc private func exportEncodingChanged(_ sender: NSPopUpButton) {
        guard let bits = selectedValue(sender), exportBitDepths.contains(bits), bits != exportBitsPerSample else { return }
        exportBitsPerSample = bits
        defaults.set(Int(bits), forKey: "exportBitsPerSample")
        invalidateExportFormat()
    }

    @objc private func pairBinauralChanged(_ sender: NSPopUpButton) {
        let enabled = selectedValue(sender) == 1
        guard enabled != pairBinaural else { return }
        pairBinaural = enabled
        defaults.set(enabled, forKey: "pairBinaural")
        waveform?.stopPreview()
        invalidateExportFormat()
    }

    @objc private func wideExportModeChanged(_ sender: NSPopUpButton) {
        guard let raw = selectedValue(sender), let mode = WideExportMode(rawValue: raw), mode != wideExportMode else { return }
        wideExportMode = mode
        defaults.set(Int(raw), forKey: "wideExportMode")
        invalidateExportFormat()
    }

    @objc private func exportMonoChanged(_ sender: NSButton) {
        let enabled = sender.state == .on
        guard enabled != forceMonoExport else { return }
        forceMonoExport = enabled
        defaults.set(enabled, forKey: "forceMonoExport")
        invalidateExportFormat()
    }

    private func invalidateExportFormat() {
        registerClick()
        waveform?.discardPreparedFile()
        setNotice(nil)
    }

    private func meterTitle(for lane: Int) -> (text: String, unmeasuredLufs: Bool) {
        let mode = meterModes[lane]
        let wideLufs = mode == .lufs && (tap?.channelCount(for: lane) ?? 0) > 2
        let reading: Float
        if let tap, tap.cachedMeters.indices.contains(lane), !wideLufs {
            reading = mode.value(from: tap.cachedMeters[lane])
        } else {
            reading = -.infinity
        }
        let value: String
        if reading.isFinite {
            if mode == .vu {
                value = String(format: "%+.1f", Double(reading))
            } else {
                value = String(format: "%.1f", Double(reading))
            }
        } else {
            value = "—"
        }
        let prefix = dualSource ? "\(captureLaneLetter(lane)) " : ""
        return ("\(prefix)\(value) \(mode.label)", wideLufs)
    }

    private func meterTooltip(for lane: Int, unmeasuredLufs: Bool) -> String {
        let base = meterModes[lane].detail
        return unmeasuredLufs ? "\(base)\nLoudness is measured for mono and stereo sources only." : base
    }

    private func ensureMeterButtons(count: Int) {
        while meterButtons.count < count {
            let lane = meterButtons.count
            let button = NSButton(title: "— dBFS", target: self, action: #selector(showMeterOptions(_:)))
            button.tag = lane
            button.bezelStyle = .rounded
            button.alignment = .right
            let widthConstraint = button.widthAnchor.constraint(equalToConstant: 91)
            widthConstraint.isActive = true
            meterButtons.append(button)
            meterWidthConstraints.append(widthConstraint)
            metersStack.addArrangedSubview(button)
        }
    }

    /// Package 7 / TL-33 & TL-36: 1-2 sources show all meters; >2 sources show as many as fit (min 1) + "+N" live grid popover.
    private func updateMeter() {
        let total = meterModes.count
        let single = !dualSource
        let meterWidth: CGFloat = single ? 102 : 91
        let font = NSFont.monospacedSystemFont(ofSize: single ? 12 : 10, weight: .medium)
        let shown: Int
        if !showsMeters {
            shown = 0
            closeMeterPopover()
        } else if total <= 2 {
            shown = total
            closeMeterPopover()
        } else {
            let winWidth = panel?.frame.width ?? fullPanelDefault.width
            // Reserve space for title (optional), transport/lamp/clear/pin (~156), gear (38), "+N" (38), padding (32).
            let reserved: CGFloat = (showsTitle ? 110 : 0) + 264
            let available = max(meterWidth, winWidth - reserved)
            let fit = max(1, Int(floor((available + 4) / (meterWidth + 4))))
            shown = min(total, fit)
        }

        ensureMeterButtons(count: shown)
        for (lane, button) in meterButtons.enumerated() {
            let hide = !showsMeters || lane >= shown || !meterModes.indices.contains(lane)
            if button.isHidden != hide { button.isHidden = hide }
            guard !hide else { continue }
            if meterWidthConstraints[lane].constant != meterWidth {
                meterWidthConstraints[lane].constant = meterWidth
            }
            if button.font != font { button.font = font }
            let info = meterTitle(for: lane)
            if button.title != info.text {
                button.title = info.text
                button.setAccessibilityLabel(info.text)
            }
            let tip = meterTooltip(for: lane, unmeasuredLufs: info.unmeasuredLufs)
            if button.toolTip != tip { button.toolTip = tip }
        }

        let extra = max(0, total - shown)
        moreMetersButton.isHidden = !showsMeters || extra == 0
        if extra > 0 {
            let title = "+\(extra)"
            if moreMetersButton.title != title { moreMetersButton.title = title }
            let tip = extra == 1
                ? "1 more meter. Click to see every source's meter."
                : "\(extra) more meters. Click to see every source's meter."
            if moreMetersButton.toolTip != tip { moreMetersButton.toolTip = tip }
            moreMetersButton.setAccessibilityLabel(tip)
        } else {
            closeMeterPopover()
        }
        refreshMeterPopover()
    }

    private func closeMeterPopover() {
        if meterPopover?.isShown == true {
            meterPopover?.performClose(nil)
        }
        meterPopover = nil
        meterPopoverButtons.removeAll()
    }

    private func refreshMeterPopover() {
        guard meterPopover?.isShown == true else { return }
        guard meterPopoverButtons.count == meterModes.count else {
            closeMeterPopover()
            return
        }
        for (lane, button) in meterPopoverButtons.enumerated() {
            let info = meterTitle(for: lane)
            if button.title != info.text {
                button.title = info.text
                button.setAccessibilityLabel(info.text)
            }
            let tip = meterTooltip(for: lane, unmeasuredLufs: info.unmeasuredLufs)
            if button.toolTip != tip { button.toolTip = tip }
        }
    }

    /// Package 7 / TL-36: live-updating 1-6 column scrollable grid popover of all source meters.
    @objc private func showMoreMeters(_ sender: NSButton) {
        registerClick()
        if meterPopover?.isShown == true {
            closeMeterPopover()
            return
        }
        let count = meterModes.count
        guard count > 0 else { return }
        let eachWidth: CGFloat = 95 // 91 + 4 spacing
        let winWidth = panel?.frame.width ?? fullPanelDefault.width
        let columns = max(1, min(6, Int(floor((winWidth - 24) / eachWidth))))
        let rows = (count + columns - 1) / columns
        let gridWidth = CGFloat(columns) * 91 + CGFloat(max(0, columns - 1)) * 4
        let rowHeight: CGFloat = 24
        let contentHeight = CGFloat(rows) * rowHeight + CGFloat(max(0, rows - 1)) * 4
        let maxHeight = min(contentHeight, 260)

        meterPopoverButtons.removeAll()
        let font = NSFont.monospacedSystemFont(ofSize: 10, weight: .medium)
        var rowViews: [NSView] = []
        for r in 0..<rows {
            var cells: [NSView] = []
            for c in 0..<columns {
                let lane = r * columns + c
                guard lane < count else { break }
                let info = meterTitle(for: lane)
                let btn = NSButton(title: info.text, target: self, action: #selector(showMeterOptions(_:)))
                btn.tag = lane
                btn.bezelStyle = .rounded
                btn.alignment = .right
                btn.font = font
                btn.toolTip = meterTooltip(for: lane, unmeasuredLufs: info.unmeasuredLufs)
                btn.setAccessibilityLabel(info.text)
                btn.widthAnchor.constraint(equalToConstant: 91).isActive = true
                btn.heightAnchor.constraint(equalToConstant: rowHeight).isActive = true
                meterPopoverButtons.append(btn)
                cells.append(btn)
            }
            let rowStack = NSStackView(views: cells)
            rowStack.orientation = .horizontal
            rowStack.spacing = 4
            rowStack.alignment = .centerY
            rowViews.append(rowStack)
        }

        let gridStack = NSStackView(views: rowViews)
        gridStack.orientation = .vertical
        gridStack.alignment = .leading
        gridStack.spacing = 4
        gridStack.translatesAutoresizingMaskIntoConstraints = false

        let doc = OptionsDocumentView(frame: NSRect(x: 0, y: 0, width: gridWidth, height: contentHeight))
        doc.addSubview(gridStack)
        NSLayoutConstraint.activate([
            gridStack.topAnchor.constraint(equalTo: doc.topAnchor),
            gridStack.leadingAnchor.constraint(equalTo: doc.leadingAnchor)
        ])

        let popSize = NSSize(width: gridWidth + 16, height: maxHeight + 16)
        let root = NSView(frame: NSRect(origin: .zero, size: popSize))
        root.appearance = NSAppearance(named: .darkAqua)
        let scroll = NSScrollView(frame: root.bounds.insetBy(dx: 8, dy: 8))
        scroll.autoresizingMask = [.width, .height]
        scroll.borderType = .noBorder
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = contentHeight > maxHeight
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.documentView = doc
        root.addSubview(scroll)

        let vc = NSViewController()
        vc.view = root
        let popover = NSPopover()
        popover.contentViewController = vc
        popover.contentSize = popSize
        popover.behavior = .transient
        popover.animates = false
        popover.appearance = NSAppearance(named: .darkAqua)
        meterPopover = popover
        popover.show(relativeTo: sender.bounds, of: sender, preferredEdge: .minY)
    }

    @objc private func togglePin() {
        alwaysOnTop.toggle()
        defaults.set(alwaysOnTop, forKey: "alwaysOnTop")
        applyPin()
    }

    @objc private func pinChanged(_ sender: NSButton) {
        alwaysOnTop = sender.state == .on
        defaults.set(alwaysOnTop, forKey: "alwaysOnTop")
        applyPin()
    }

    private func applyPin() {
        pinButton.state = alwaysOnTop ? .on : .off
        optionsPinButton?.state = pinButton.state
        panel?.isFloatingPanel = alwaysOnTop
        panel?.hidesOnDeactivate = false
        panel?.level = alwaysOnTop ? .floating : .normal
        // Package 12 / OPT-01: Options window follows main window's pin state.
        optionsPanel?.isFloatingPanel = alwaysOnTop
        optionsPanel?.level = alwaysOnTop ? .floating : .normal
        setIcon(
            alwaysOnTop ? "pin.fill" : "pin", label: alwaysOnTop ? "Disable keep on top" : "Keep on top",
            on: pinButton
        )
    }

    @objc private func toggleWaveformOnly() {
        waveformOnly.toggle()
        defaults.set(waveformOnly, forKey: "waveformOnly")
        registerClick()
        applyWaveformOnly()
    }

    private func applyWaveformOnly(initial: Bool = false, restoredFrame: Bool = false) {
        guard let panel else { return }
        optionsWaveformButton?.state = waveformOnly ? .on : .off
        panel.contentView?.layoutSubtreeIfNeeded()
        let previous = panel.contentRect(forFrameRect: panel.frame).size
        let waveformHeight = waveform?.bounds.height ?? fullWaveformMinimumHeight
        var desiredSize: NSSize?
        if waveformOnly {
            closeMeterPopover()
            let restoredCompact = initial && restoredFrame && fullPanelSize != nil
            if !restoredCompact {
                fullPanelSize = NSSize(
                    width: max(previous.width, fullPanelMinimum.width),
                    height: max(previous.height, fullPanelMinimum.height)
                )
                if let fullPanelSize {
                    defaults.set(Double(fullPanelSize.width), forKey: "fullPanelWidth")
                    defaults.set(Double(fullPanelSize.height), forKey: "fullPanelHeight")
                }
                desiredSize = initial
                    ? waveformOnlyPanelDefault
                    : NSSize(width: previous.width, height: waveformHeight + 2 * frameInset)
            }
        } else if !initial {
            desiredSize = fullPanelSize
        }

        toolbarTrailing?.isActive = !waveformOnly
        toolbar?.isHidden = waveformOnly
        statusLabel.isHidden = waveformOnly
        statusHeightConstraint?.constant = waveformOnly ? 0 : statusStripHeight
        waveformLeading?.constant = waveformOnly ? frameInset : panelVerticalInset
        waveformTrailing?.constant = waveformOnly ? -frameInset : -panelVerticalInset
        if waveformOnly {
            waveformTopBelowToolbar?.isActive = false
            waveformBottomWithStatus?.isActive = false
            waveformMinimumHeight?.constant = waveformOnlyPanelMinimum.height - 2 * frameInset
            waveformTopWithoutToolbar?.constant = frameInset
            waveformTopWithoutToolbar?.isActive = true
            waveformBottomWithoutStatus?.constant = -frameInset
            waveformBottomWithoutStatus?.isActive = true
        } else {
            waveformTopWithoutToolbar?.isActive = false
            waveformBottomWithoutStatus?.isActive = false
            waveformMinimumHeight?.constant = fullWaveformMinimumHeight
            waveformTopBelowToolbar?.isActive = true
            waveformBottomWithStatus?.isActive = true
        }
        panel.contentMinSize = waveformOnly ? waveformOnlyPanelMinimum : fullPanelMinimum
        resizePanel(to: desiredSize ?? previous)
        waveform?.showsOverlay = !waveformOnly
        updateBackground()
        updateStatus()
    }

    private func resizePanel(to requested: NSSize) {
        guard let panel else { return }
        let original = panel.frame
        let minimum = panel.contentMinSize
        let size = NSSize(width: max(requested.width, minimum.width), height: max(requested.height, minimum.height))
        let target = panel.frameRect(forContentRect: NSRect(origin: .zero, size: size)).size
        var adjusted = NSRect(x: original.minX, y: original.maxY - target.height, width: target.width, height: target.height)
        if let visible = panel.screen?.visibleFrame {
            if target.width <= visible.width {
                adjusted.origin.x = min(max(adjusted.minX, visible.minX), visible.maxX - target.width)
            }
            if target.height <= visible.height {
                adjusted.origin.y = min(max(adjusted.minY, visible.minY), visible.maxY - target.height)
            }
        }
        if adjusted != original { panel.setFrame(adjusted, display: true) }
    }

    @objc private func chooseFolder(_ sender: NSButton) {
        guard let folder = ExportFolder(rawValue: sender.tag), let window = optionsPanel, !editing else { return }
        let wasPaused = paused
        if folder == .output { beginEditing(retainMaterial: false) }
        let chooser = NSOpenPanel()
        chooser.title = folder.chooserTitle
        chooser.canChooseDirectories = true
        chooser.canChooseFiles = false
        chooser.canCreateDirectories = true
        chooser.allowsMultipleSelection = false
        chooser.directoryURL = URL(fileURLWithPath: directory(for: folder), isDirectory: true)
        chooser.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            if response == .OK, let url = chooser.url {
                self.defaults.set(url.path, forKey: folder.key)
                if folder == .output { self.waveform?.discardPreparedFile() }
                let label = folder == .output ? self.outputPathLabel : self.savedPathLabel
                label?.stringValue = url.path
                label?.toolTip = url.path
                // Either folder can turn the two into one, so the next clean-up waits again.
                if let cleanup = self.cleanup { bt_cleanup_restart(cleanup) }
                self.updateCleanupNote()
                self.setNotice(folder == .output ? "Output: \(url.path)" : "Snapshot folder: \(url.path)")
            }
            if folder == .output {
                self.endEditing(allowImmediateResume: !wasPaused)
            }
        }
    }

    @objc private func openFolder(_ sender: NSButton) {
        guard let folder = ExportFolder(rawValue: sender.tag) else { return }
        registerClick()
        let url = URL(fileURLWithPath: directory(for: folder), isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        } catch {
            reportFolderError(error.localizedDescription)
            return
        }
        if !NSWorkspace.shared.open(url) { reportFolderError(url.path) }
    }

    private func reportFolderError(_ detail: String) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Could not open the folder"
        alert.informativeText = detail
        alert.addButton(withTitle: "OK")
        _ = alert.runModal()
    }

    func registerClick() {
        lastClick = ProcessInfo.processInfo.systemUptime
    }

    func beginEditing(retainMaterial: Bool = true) {
        editing = true
        notice = nil
        registerClick()
        if retainMaterial { waveform?.retainEditingMaterial() }
        updateStatus()
    }

    func endEditing(allowImmediateResume: Bool = true) {
        guard editing else { return }
        editing = false
        registerClick()
        // Selection edits do not change the user's explicit transport state.
        updateStatus()
    }

    func resumeAfterSelectionCancellation() {
        editing = false
        registerClick()
        notice = nil
        refreshWaveform()
        updateStatus()
    }

    private var captureUnavailable: Bool { tap == nil || captureError != nil }

    /// The transport button's name on the toolbar,
    private var toolbarTransportLabel: String {
        captureUnavailable ? "Retry capture" : (paused ? "Start capture" : "Pause capture")
    }

    /// in the Options window,
    private var optionsTransportLabel: String {
        captureUnavailable ? "Retry capture" : (paused ? "Play" : "Pause")
    }

    /// and in the menus, which name what the item will do.
    private var menuTransportLabel: String {
        captureUnavailable ? "Retry capture" : (paused ? "Resume" : "Pause")
    }

    @objc private func toggleCapture(_ sender: Any?) {
        guard !editing else { return }
        // Auto-resume can satisfy a Start action while the contextual menu is open.
        if let offeredWhilePaused = (sender as? NSMenuItem)?.representedObject as? Bool,
           offeredWhilePaused != paused { return }
        registerClick()
        if paused { resume(clearingSelection: false) } else { pauseCapture() }
    }

    @objc private func clearCapture() {
        guard !editing, let core = tap?.core else { return }
        registerClick()
        bt_tracks_clear(core)
        tap?.clearPositionTracks()
        selectionByAddon = nil
        waveform?.resetTimeline()
        refreshWaveform()
        notice = nil
        updateStatus()
    }

    @objc private func cancelWaveformSelection() {
        _ = waveform?.cancelSelection()
    }

    /// Whether a pause has gone untouched for the auto-resume delay.
    private func shouldAutoResume(now: TimeInterval) -> Bool {
        paused && !editing && waveform?.hasSelection != true && tap != nil && idleSeconds > 0
            && now - lastClick >= Double(idleSeconds)
    }

    private func pauseCapture() {
        // A pause, whoever asks for it, starts the auto-resume delay again (Windows `Recorder::freeze`).
        registerClick()
        if !paused {
            do {
                try tap?.pause()
            } catch {
                captureError = error.localizedDescription
                notice = nil
            }
            paused = true
            refreshWaveform()
        }
        updateStatus()
    }

    private func resume(clearingSelection: Bool = true) {
        guard paused, !editing else { return }
        do {
            if tap == nil {
                tap = try SystemAudioTap(history: history, lanes: captureLanes, retainSilence: retainSilence)
                rememberSourceNames()
            }
            requestMicrophonePermissionIfNeeded()
            try tap?.resume()
            if clearingSelection { waveform?.clearSelection() }
            paused = false
            captureError = nil
            notice = nil
        } catch {
            // The callback gate is still closed after a failed start. Let the user retry.
            paused = true
            captureError = error.localizedDescription
            notice = nil
        }
        updateStatus()
    }

    private func requestMicrophonePermissionIfNeeded() {
        guard !microphonePromptPending, captureLanes.contains(where: { $0.source.isMicrophone }),
              SystemAudioTap.requiresMicrophonePrompt else { return }
        microphonePromptPending = true
        SystemAudioTap.requestMicrophonePermission { [weak self] in
            DispatchQueue.main.async {
                self?.microphonePromptPending = false
                self?.lastSourceCheck = 0
            }
        }
    }

    @objc private func tick(_: Timer) {
        let tickStart = ProcessInfo.processInfo.systemUptime
        pollAddonHost()
        // Package 12 / WIN-23: hidden or minimized main window skips waveform/meter drawing.
        let panelVisible = panel?.isVisible == true && panel?.isMiniaturized != true
        let optionsVisible = optionsPanel?.isVisible == true && optionsPanel?.isMiniaturized != true
        if panelVisible { waveform?.refreshHintModifiers() }
        let now = ProcessInfo.processInfo.systemUptime
        if !paused, now - lastSourceCheck >= 1 {
            lastSourceCheck = now
            if tap?.refreshSources() == true { reconnectConfiguredSources() }
        }
        if optionsVisible, now - lastSnapshotTargetCheck >= 1 {
            lastSnapshotTargetCheck = now
            if !independentWindow, (snapshotTargetPopup as? OptionsPopUpButton)?.isChoicePopoverShown != true {
                refreshSnapshotTargets()
            }
        }
        if !independentWindow && !testCopy && now - lastShortcutCheck >= 1 {
            lastShortcutCheck = now
            snapshotShortcut.start()
            if optionsVisible { refreshShortcutPermission() }
            cleanOutputFolder()
        }
        if let tap, !paused {
            if panelVisible {
                refreshWaveform()
                tap.pollCapture()
            } else if optionsVisible {
                tap.pollCapture()
            } else if addonHost != nil {
                tap.pollCapture()
            }
        }
        if shouldAutoResume(now: ProcessInfo.processInfo.systemUptime) {
            registerClick()
            resume()
        }
        if panelVisible || optionsVisible { updateStatus() }
        if let harness, !harness.finished {
            harness.recordFrameWork(ProcessInfo.processInfo.systemUptime - tickStart, now: tickStart)
        }
    }

    func refreshWaveform() {
        guard let waveform, let tap else { return }
        waveform.refresh(tap: tap, worker: worker)
    }

    private func updateStatus() {
        let state: CaptureLamp.State
        if tap == nil || captureError != nil {
            state = .unavailable
        } else if paused {
            state = .paused
        } else if tap?.cachedState == 1 {
            state = .recording
        } else {
            state = .waiting
        }
        let label = toolbarTransportLabel
        if captureButton.title != label {
            setIcon(paused ? "play.fill" : "pause.fill", label: label, on: captureButton)
        }
        captureButton.isEnabled = !editing
        clearButton.isEnabled = tap?.core != nil && !editing
        if let optionsCaptureButton, optionsCaptureButton.title != optionsTransportLabel {
            optionsCaptureButton.title = optionsTransportLabel
        }
        optionsCaptureButton?.isEnabled = captureButton.isEnabled
        optionsClearButton?.isEnabled = clearButton.isEnabled
        optionsSaveButton?.isEnabled = clearButton.isEnabled && waveform?.isPreparingMaterial != true
        optionsCancelSelectionButton?.isEnabled = waveform?.canCancelSelection == true
        updateMeter()

        let now = ProcessInfo.processInfo.systemUptime
        var stateText = state.label
        // A gesture or a selection holds the pause, and Off never resumes.
        if state == .paused && !editing && waveform?.hasSelection != true && idleSeconds > 0 {
            // The tick on which the delay runs out resumes capture and shows no zero.
            let remaining = max(1, Int(ceil(Double(idleSeconds) - (now - lastClick))))
            stateText = "Paused · resumes in \(remaining) s"
        }
        let problem = captureError ?? tap?.sourceMessage
        if let waveform {
            var laneProbs: [Int: String] = [:]
            if let tap {
                for idx in 0..<tap.laneCount {
                    if let p = tap.sourceProblem(for: idx) { laneProbs[idx] = p }
                    if idx < sourceProblemNotes.count {
                        let text = laneProbs[idx] ?? ""
                        if sourceProblemNotes[idx].stringValue != text {
                            sourceProblemNotes[idx].stringValue = text
                        }
                        let hide = text.isEmpty
                        if sourceProblemNotes[idx].isHidden != hide {
                            sourceProblemNotes[idx].isHidden = hide
                        }
                    }
                }
            }
            waveform.laneProblems = laneProbs
            waveform.laneSourceLabels = captureLanes.map { $0.source.label }
        }
        let markShown = statusMark.shows(state, problem: problem != nil)
        // With Never the lamp gives up its place; otherwise it keeps it while it is dark.
        if captureLamp.isHidden != (statusMark == .never) { captureLamp.isHidden = statusMark == .never }
        captureLamp.update(state, shown: markShown, now: now, detail: stateText)

        let message = notice ?? problem ?? ""
        if let optionsNotice, optionsNotice.stringValue != message {
            optionsNotice.stringValue = message
            optionsNotice.toolTip = message.isEmpty ? nil : message
        }
        // Package 6 & 17 / WIN-06, ADD-W27: status strip below waveform (26px, empty when no text, hidden in waveform-only).
        let line: String
        if let captureError {
            line = captureError
        } else if waveform?.isPreparingMaterial == true {
            line = "Preparing selection…"
        } else if waveform?.hasSelection == true && notice == nil {
            let rowLabel = waveform?.selectedRowsLabel ?? ""
            var selection = rowLabel.isEmpty ? "Selection" : "Selection · \(rowLabel)"
            if let by = selectionByAddon {
                selection += " · by \(by)"
            }
            line = paused ? "\(selection) / Paused" : selection
        } else {
            line = [state == .paused ? stateText : nil, message.isEmpty ? nil : message]
                .compactMap { $0 }.joined(separator: " · ")
        }
        if statusLabel.stringValue != line {
            statusLabel.stringValue = line
            statusLabel.toolTip = line.isEmpty ? nil : line
        }
        if statusLabel.isHidden != waveformOnly { statusLabel.isHidden = waveformOnly }

        // Waveform-only mode has no toolbar, so the plot carries the mark and any problem.
        if waveformOnly && markShown {
            let extra = message.isEmpty || message == problem ? nil : message
            waveform?.compactStatus = CompactStatus(
                state: state, lit: captureLamp.isLit, problem: problem,
                detail: [stateText, problem, extra].compactMap { $0 }.joined(separator: " · ")
            )
        } else {
            waveform?.compactStatus = nil
        }
    }

    /// The user moved or dropped the selection, so the status line stops naming the add-on.
    func selectionNoLongerByAddon() {
        guard selectionByAddon != nil else { return }
        selectionByAddon = nil
        updateStatus()
    }

    fileprivate func setNotice(_ message: String?) {
        notice = message
        updateStatus()
    }

    func frozenExport(range: Range<UInt64>? = nil) -> WAVExport? {
        waveform?.materialForExport(range: range)
    }

    /// Freezes the capture. `read` receives the result on the capture queue, before the main queue does.
    func requestEditingMaterial(read: EditingMaterialRead? = nil, _ completion: @escaping (WAVExport?) -> Void) {
        guard let tap else { completion(nil); return }
        let routes = tap.captureRoutes
        let labels = tap.captureChannelNames
        let groups = (0..<tap.laneCount).map { tap.channelGroups(for: $0) }
        let frozenPos = tap.freezePositions()
        tap.readCapture({
            let material = WAVExport(core: $0, routes: routes, channelNames: labels, channelGroups: groups, positions: frozenPos)
            read?.finish(material)
            return material
        }) { [weak self, weak tap] material in
            guard let self, let tap, self.tap === tap else { completion(nil); return }
            completion(material)
        }
    }

    /// The window's freeze for editing, which saves made meanwhile wait for (`pendingSnapshots`).
    func requestEditingFreeze(_ completion: @escaping (WAVExport?) -> Void) {
        guard let tap else { completion(nil); return }
        let read = EditingMaterialRead(tap: ObjectIdentifier(tap))
        editingFreezeReads.append(read)
        requestEditingMaterial(read: read) { [weak self] material in
            self?.editingFreezeReads.removeAll { $0 === read }
            completion(material)
        }
    }

    func retireEditingMaterial(_ material: WAVExport?) {
        guard let material else { return }
        worker.async { withExtendedLifetime(material) {} }
    }

    var hideSilentLaneIndices: Set<Int> {
        guard hideSilentChannels, let tap else { return [] }
        return Set((0..<tap.laneCount).filter { tap.canHideSilentChannels(for: $0) })
    }

    var currentChannelLabels: [[String]] {
        tap?.captureChannelNames ?? []
    }

    @objc private func saveCurrentSnapshot() {
        saveSnapshot(into: directory(for: .saved))
    }

    func prepareDrag(
        _ job: WAVExport, range: Range<UInt64>, fades: BtFades, rows: [RowIdentity]?,
        completion: @escaping ([URL]) -> Bool
    ) -> DispatchWorkItem {
        writeExport(job, range: range, fades: fades, rows: rows, directory: directory(for: .output),
                    provisional: true, completion: completion)
    }

    /// Writes the selected audio on the export worker.
    @discardableResult
    private func writeExport(
        _ job: WAVExport, range: Range<UInt64>, fades: BtFades, rows: [RowIdentity]?, directory: String,
        provisional: Bool, controlCall: AddonControlCall? = nil, completion: @escaping ([URL]) -> Bool
    ) -> DispatchWorkItem {
        let sampleRate = exportSampleRate
        let bitsPerSample = exportBitsPerSample
        let forceMono = forceMonoExport
        let wideMode = wideExportMode
        let pairBinaural = self.pairBinaural
        let names = laneFileNames
        let hideLanes = hideSilentLaneIndices
        let shownIDs = currentShownGroupIDs()
        let frameCount = range.upperBound > range.lowerBound ? (range.upperBound - range.lowerBound) : 0
        if !provisional { activeSnapshotSaves += 1 }
        setNotice(provisional ? "Preparing WAV…" : "Saving snapshot…")
        let ledger = clipLedger
        let token = ledger.reserve()
        let write = DispatchWorkItem { @Sendable [weak self, worker, pendingProvisionalWAVs] in
            let written = job.write(
                range: range, directory: directory, sampleRate: sampleRate, bitsPerSample: bitsPerSample,
                forceMono: forceMono, fades: fades, names: names, selectedRows: rows, hideSilentLanes: hideLanes,
                wideMode: wideMode, pairBinaural: pairBinaural, shownGroupIDs: shownIDs
            )
            let urls = written.urls
            if provisional { pendingProvisionalWAVs.urls.formUnion(urls) }
            // Windows announces a save from the thread that wrote it; listing it here lets quitting do the same.
            if !provisional, !urls.isEmpty {
                ledger.add(ClipExportLedger.Clip(urls: urls, sampleRate: sampleRate, sourceFrames: frameCount,
                                                 sourceRate: job.sampleRate), as: token)
            }
            DispatchQueue.main.async {
                let clip = ledger.take(token)
                if !provisional, let self { self.activeSnapshotSaves = max(0, self.activeSnapshotSaves - 1) }
                if provisional {
                    worker.async { pendingProvisionalWAVs.urls.subtract(urls) }
                }
                guard completion(urls) else {
                    if provisional {
                        for url in urls { try? FileManager.default.removeItem(at: url) }
                    }
                    return
                }
                if urls.isEmpty {
                    let detail = written.error ?? "Check that there is audio and that the folder can be written to."
                    self?.setNotice((provisional ? "Could not prepare WAV: " : "Could not save snapshot: ") + detail)
                    if let controlCall {
                        self?.addonHost?.controlReply(
                            call: controlCall,
                            error: AddonControlError(code: "failed", message: detail)
                        )
                    }
                } else {
                    if !provisional {
                        self?.harness?.noteSaved(urls)
                        if let clip { self?.notifyClipExported(clip) }
                    }
                    if provisional {
                        self?.setNotice(written.objectsInMono ? "Objects without positions were written in mono." : nil)
                    } else {
                        var status = "Saved snapshot \(exportDescription(urls))"
                        if written.objectsInMono {
                            status = "Objects without positions were written in mono. \(status)"
                        }
                        if let controlCall {
                            status += " · by \(controlCall.addon_name)"
                            self?.replyControlSave(call: controlCall, folder: directory, urls: urls, lanes: written.lanes)
                        }
                        self?.setNotice(status)
                    }
                }
            }
        }
        worker.async(execute: write)
        return write
    }

    // MARK: - Unified Action Layer & Test Harness (Package 22 / TEST-07..18)

    @objc private func driveHarnessTick(_: Timer) {
        guard let harness, !harness.finished else { return }
        let now = ProcessInfo.processInfo.systemUptime
        if now - harness.started < harness.delaySeconds { return }
        if harness.appliedAt == nil {
            harness.appliedAt = now
            applyTestSteps()
            return
        }
        if let folder = harness.dragCopyFolder,
           waveform?.hasPendingDragWrite != true,
           waveform?.isPreparingMaterial != true {
            harness.dragCopyFolder = nil
            copyHarnessDragFiles(to: folder)
            return
        }
        let settled = (now - (harness.appliedAt ?? now)) >= TestHarness.settleSeconds + harness.holdSeconds
            && waveform?.isPreparingMaterial != true
            && (!harness.colorWait || waveform?.isFrequencyBusy != true)
            && activeSnapshotSaves == 0
            && waveform?.hasPendingDragWrite != true
        guard settled else { return }
        if !harness.steps.isEmpty {
            harness.appliedAt = now
            applyTestSteps()
            return
        }
        harness.finished = true
        updateStatus()
        harness.note("status: \(statusLabel.stringValue)")
        let targetView: NSView?
        if harness.optionsPicture {
            if let optionsPanel, optionsPanel.isVisible {
                targetView = optionsPanel.contentView
            } else {
                harness.writeLog(picture: .failure("the Options window is not open"))
                NSApp.terminate(nil)
                return
            }
        } else {
            targetView = panel?.contentView
        }
        let result: Result<(Int, Int), MessageError>
        if let targetView {
            result = TestHarness.captureRGBPNG(of: targetView, to: harness.output)
        } else {
            result = .failure("window view is unavailable")
        }
        harness.writeLog(picture: result)
        NSApp.terminate(nil)
    }

    private func applyTestSteps() {
        guard let harness else { return }
        let splitIndex = harness.steps.dropFirst().firstIndex(where: \.waits) ?? harness.steps.endIndex
        let batch = Array(harness.steps[..<splitIndex])
        harness.steps.removeFirst(batch.count)
        harness.holdSeconds = 0
        for step in batch {
            if case .failure(let error) = applyTestStep(step) {
                harness.note("\(step): \(error.message)")
            }
        }
        refreshWaveform()
        updateStatus()
    }

    private func applyTestStep(_ step: TestStep) -> Result<Void, MessageError> {
        switch step {
        case .zoom(let seconds):
            return runWindowAction(.zoomNewest(seconds)).map { _ in () }.mapError { MessageError($0.message) }
        case .select(let from, let to):
            return runWindowAction(.selectBack(from: from, to: to)).map { _ in () }.mapError { MessageError($0.message) }
        case .selectLane(let lane):
            return runWindowAction(.chooseLane(lane)).map { _ in () }.mapError { MessageError($0.message) }
        case .rows(let fl, let fr, let ll, let lr):
            return runWindowAction(.chooseRows(firstLane: fl, firstRow: fr, lastLane: ll, lastRow: lr)).map { _ in () }.mapError { MessageError($0.message) }
        case .separate(let on):
            return runWindowAction(.separate(on)).map { _ in () }.mapError { MessageError($0.message) }
        case .save(let folder):
            let res = runWindowAction(.save(folder: folder))
            if case .failure(let refusal) = res {
                var isDir: ObjCBool = false
                if !FileManager.default.fileExists(atPath: folder.path, isDirectory: &isDir) || !isDir.boolValue {
                    return .failure(MessageError(refusal.message))
                }
            }
            return .success(())
        case .dragPrepare(let folder):
            do {
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            } catch {
                return .failure("\(folder.path): \(error.localizedDescription)")
            }
            let res = runWindowAction(.dragPrepare)
            if case .failure(let refusal) = res { return .failure(MessageError(refusal.message)) }
            harness?.dragCopyFolder = folder
            return .success(())
        case .exportMode(let raw):
            guard let mode = WideExportMode(rawValue: raw) else { return .failure("invalid export mode") }
            wideExportMode = mode
            invalidateExportFormat()
            return .success(())
        case .openExportMode:
            return .success(())
        case .previewPlan:
            switch runWindowAction(.previewPlan) {
            case .success(let note):
                if let note { harness?.note(note) }
                return .success(())
            case .failure(let refusal):
                return .failure(MessageError(refusal.message))
            }
        case .scroll(let lane, let row):
            waveform?.scrollToRow(lane: lane, row: row)
            return .success(())
        case .group(let lane, let groupID):
            guard selectChannelGroup(lane: lane, groupID: groupID) else {
                return .failure("lane \(lane) has no channel group '\(groupID)'")
            }
            return .success(())
        case .hideSilent(let on):
            hideSilentChannels = on
            optionsHideSilentButton?.state = on ? .on : .off
            waveform?.hideSilentChannels = on
            waveform?.discardPreparedFile()
            refreshWaveform()
            return .success(())
        case .pair(let binaural):
            pairBinaural = binaural
            waveform?.stopPreview()
            invalidateExportFormat()
            return .success(())
        case .options(let page, let size):
            if let size { harness?.optionsSize = size }
            if optionsPanel == nil || optionsPanel?.isVisible != true { showOptions() }
            if let size { optionsPanel?.setContentSize(size) }
            let targetIndex = optionCategoryButtons.firstIndex {
                $0.title.lowercased().replacingOccurrences(of: "-", with: "") == page
            }
            if let idx = targetIndex {
                selectOptionCategory(optionCategoryButtons[idx])
            } else {
                return .failure("Options page \(page) is not available")
            }
            return .success(())
        case .optionsScroll(let points):
            guard let tabs = optionTabs, let scroll = tabs.selectedTabViewItem?.view as? NSScrollView else {
                return .failure("the Options window is not open")
            }
            scroll.layoutSubtreeIfNeeded()
            scroll.contentView.scroll(to: NSPoint(x: 0, y: points))
            scroll.reflectScrolledClipView(scroll.contentView)
            return .success(())
        case .bind(let action, let bindings):
            if let reason = hotkeys.set(bindings, for: action) { return .failure(MessageError(reason)) }
            finishHotkeyEdit()
            return .success(())
        case .key(let shortcut):
            if let action = hotkeys.windowAction(for: shortcut) {
                _ = run(action)
            }
            return .success(())
        case .openMeters:
            guard captureLanes.count > 2 else {
                return .failure("the meters all show with one or two sources")
            }
            showMoreMeters(moreMetersButton)
            return .success(())
        case .wait(let ms):
            harness?.holdSeconds = Double(ms) / 1000.0
            return .success(())
        case .look(let addonID):
            chooseLookAddon(addonID)
            return .success(())
        case .waveformColor(let byFrequency):
            frequencyColoring = byFrequency
            defaults.set(frequencyColoring, forKey: "waveformColorByFrequency")
            waveform?.frequencyGradient = frequencyColoring ? effectiveFrequencyGradient() : nil
            refreshFrequencyGradientControls()
            refreshWaveform()
            return .success(())
        case .gradient(let custom):
            frequencyGradient = custom ?? .standard
            storeFrequencyGradient(persist: true, custom: custom != nil, refreshControls: true)
            return .success(())
        case .colorWait(let wait):
            harness?.colorWait = wait
            return .success(())
        case .addSource(let name):
            let choices = availableCaptureSources(excluding: tap?.privatePlaybackDevices ?? [])
            let wanted = name.trimmingCharacters(in: .whitespaces).lowercased()
            let found = choices.first { $0.title.lowercased() == wanted }
                ?? choices.first { $0.title.lowercased().contains(wanted) }
                ?? choices.first { $0.source.key.lowercased() == wanted }
            guard let source = found?.source else { return .failure("no source is called \(name)") }
            return runWindowAction(.addSource(source)).map { _ in () }.mapError { MessageError($0.message) }
        case .removeSource(let lane):
            return runWindowAction(.removeSource(lane)).map { _ in () }.mapError { MessageError($0.message) }
        case .dropPackage(let url):
            _ = handleDroppedFiles([url])
            return .success(())
        case .dropFile(let url):
            _ = handleDroppedFiles([url])
            return .success(())
        case .installConfirm:
            confirmAddonInstall()
            return .success(())
        case .allowControl(let addonID, let allowed):
            addonHost?.setControlAllowed(id: addonID, allowed: allowed) { _ in }
            return .success(())
        case .setAddonSetting(let addonID, let key, let raw):
            let jsonVal: AddonJSONValue
            if raw == "true" { jsonVal = .bool(true) }
            else if raw == "false" { jsonVal = .bool(false) }
            else if let i = Int64(raw) { jsonVal = .int(i) }
            else if let d = Double(raw) { jsonVal = .double(d) }
            else { jsonVal = .string(raw) }
            addonHost?.setSetting(id: addonID, key: key, value: jsonVal) { _ in }
            return .success(())
        case .runAddonAction(let addonID, let actionID):
            addonHost?.runAction(id: addonID, action: actionID) { _ in }
            return .success(())
        case .selectAddonFormat(let lane, let fmt):
            guard captureLanes.indices.contains(lane),
                  case .addon(let addonID, let sourceID) = captureLanes[lane].source else {
                return .failure("lane \(lane) is not an add-on source")
            }
            addonHost?.selectFormat(id: addonID, source: sourceID, format: fmt) { _ in }
            return .success(())
        case .addonTransport(let lane, let cmd, let pos):
            guard captureLanes.indices.contains(lane),
                  case .addon(let addonID, let sourceID) = captureLanes[lane].source else {
                return .failure("lane \(lane) is not an add-on source")
            }
            addonHost?.transport(id: addonID, source: sourceID, command: cmd, position: pos) { _ in }
            return .success(())
        case .removeAddon(let id, let deleteData):
            addonHost?.remove(id: id, deleteData: deleteData) { _ in }
            return .success(())
        case .unsupportedAddon(let step):
            return .failure("step '\(step)' is not supported")
        }
    }

    private func copyHarnessDragFiles(to folder: URL) {
        guard !lastHarnessDragFiles.isEmpty else {
            harness?.note("drag-prepare: nothing was prepared (\(notice ?? ""))")
            return
        }
        var copied: [URL] = []
        var errors: [String] = []
        for url in lastHarnessDragFiles {
            let dest = folder.appendingPathComponent(url.lastPathComponent)
            do {
                try? FileManager.default.removeItem(at: dest)
                try FileManager.default.copyItem(at: url, to: dest)
                copied.append(dest)
            } catch {
                errors.append("drag-prepare: \(url.path): \(error.localizedDescription)")
            }
        }
        harness?.note("drag-prepare: \(lastHarnessDragFiles.count) files")
        harness?.noteSaved(copied)
        for err in errors { harness?.note(err) }
        harness?.note("drag status would be: \(lastHarnessDragStatus)")
        for url in lastHarnessDragFiles { try? FileManager.default.removeItem(at: url) }
        lastHarnessDragFiles.removeAll()
    }

    func runWindowAction(_ action: TestWindowAction) -> Result<String?, TestActionRefusal> {
        let rate = Double(max(1, waveform?.currentSampleRate ?? UInt32(SystemAudioTap.timelineRate)))
        let history = waveform?.currentRetainedFrames ?? (0..<0)
        func toFrames(_ seconds: Double) -> UInt64 { UInt64((seconds * rate).rounded()) }
        switch action {
        case .selectBack, .selectFrames, .zoomNewest, .view:
            if history.isEmpty {
                return .failure(TestActionRefusal(code: "no_audio", message: "no audio yet"))
            }
        default: break
        }
        switch action {
        case .startCapture:
            if paused || captureError != nil { resume(clearingSelection: false) }
            return .success(nil)
        case .pauseCapture:
            if !paused { pauseCapture() }
            return .success(nil)
        case .clearHistory:
            clearCapture()
            return .success(nil)
        case .selectBack(let from, let to):
            let hi = max(from, to), lo = min(from, to)
            let start = max(history.lowerBound, history.upperBound - min(history.upperBound, toFrames(hi)))
            let end = max(start + 1, history.upperBound - min(history.upperBound, toFrames(lo)))
            let clampedEnd = min(history.upperBound, end)
            guard start < clampedEnd else {
                return .failure(TestActionRefusal(code: "no_audio", message: "no audio yet"))
            }
            beginEditing(retainMaterial: true)
            waveform?.applyProgrammaticSelection(start..<clampedEnd)
            endEditing(allowImmediateResume: false)
            return .success(nil)
        case .selectFrames(let range):
            guard range.lowerBound >= history.lowerBound, range.upperBound <= history.upperBound, !range.isEmpty else {
                return .failure(TestActionRefusal(code: "not_found", message: "frames \(range.lowerBound)..\(range.upperBound) are not all in the audio held, \(history.lowerBound)..\(history.upperBound)"))
            }
            beginEditing(retainMaterial: true)
            waveform?.applyProgrammaticSelection(range)
            endEditing(allowImmediateResume: false)
            return .success(nil)
        case .clearSelection:
            _ = waveform?.cancelSelection()
            return .success(nil)
        case .separate(let on):
            guard multipleRows else {
                return .failure(TestActionRefusal(code: "failed", message: "editing separately needs more than one row"))
            }
            if separateLanes != on { toggleSeparateLanes() }
            return .success(nil)
        case .chooseLane(let lane):
            guard let waveform, waveform.selectProgrammaticLane(lane) else {
                return .failure(TestActionRefusal(code: "not_found", message: "the window does not show that lane"))
            }
            updateStatus()
            return .success(nil)
        case .chooseRows(let fl, let fr, let ll, let lr):
            guard let waveform, waveform.selectProgrammaticRows(firstLane: fl, firstRow: fr, lastLane: ll, lastRow: lr) else {
                return .failure(TestActionRefusal(code: "not_found", message: "the window has no matching row range"))
            }
            updateStatus()
            return .success(nil)
        case .zoomNewest(let seconds):
            let count = max(UInt64(1), toFrames(seconds))
            let start = history.upperBound > count ? max(history.lowerBound, history.upperBound - count) : history.lowerBound
            waveform?.applyProgrammaticView(start..<history.upperBound)
            return .success(nil)
        case .view(let range):
            let start = max(history.lowerBound, range.lowerBound)
            let end = min(history.upperBound, max(start + 1, range.upperBound))
            waveform?.applyProgrammaticView(start..<end)
            return .success(nil)
        case .viewAll:
            if !history.isEmpty { waveform?.applyProgrammaticView(history) }
            return .success(nil)
        case .save(let folder):
            do {
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            } catch {
                return .failure(TestActionRefusal(code: "failed", message: "\(folder.path): \(error.localizedDescription)"))
            }
            saveSnapshot(into: folder.path)
            return .success(nil)
        case .dragPrepare:
            guard let waveform else { return .failure(TestActionRefusal(code: "failed", message: "no waveform")) }
            let selected = waveform.selectionForSaving()
            let fades = waveform.fadesForSaving()
            let rows = selected == nil ? nil : waveform.selectedRowsForExport
            let runPrepare: (WAVExport?) -> Void = { [weak self] job in
                guard let self, let job else {
                    self?.lastHarnessDragFiles = []
                    return
                }
                let range = selected ?? job.span
                _ = self.writeExport(job, range: range, fades: fades, rows: rows, directory: self.directory(for: .output),
                                     provisional: true) { [weak self] urls in
                    self?.lastHarnessDragFiles = urls
                    self?.lastHarnessDragStatus = urls.isEmpty ? "" : "Saved \(exportDescription(urls))"
                    return true
                }
            }
            if let material = waveform.materialForExport(range: selected) {
                runPrepare(material)
            } else {
                requestEditingMaterial(runPrepare)
            }
            return .success(nil)
        case .previewPlan:
            return .success(describePreviewPlan())
        case .playPreview:
            if testCopy {
                return .failure(TestActionRefusal(code: "failed", message: "a test copy of Backtracking does not play audio"))
            }
            // A preview already playing goes on; otherwise one that did not start says why
            // (Windows `ui/commands.rs:279-286`).
            if waveform?.isPreviewPlaying != true {
                _ = waveform?.togglePreview()
                if waveform?.isPreviewPlaying != true {
                    return .failure(TestActionRefusal(code: "failed", message: notice ?? "The selected audio is unavailable."))
                }
            }
            return .success(nil)
        case .stopPreview:
            waveform?.stopPreview()
            return .success(nil)
        case .addSource(let source):
            guard captureLanes.count < maximumCaptureLanes else {
                return .failure(TestActionRefusal(code: "failed", message: "The recorder has reached the 64-source limit."))
            }
            let lane = CaptureLane(source: source)
            guard canConfigure(lane, at: captureLanes.count) else {
                return .failure(TestActionRefusal(code: "failed", message: notice ?? "Cannot add source"))
            }
            captureLanes.append(lane)
            meterModes.append(.dbfs)
            storeLanes(rebuildOptions: true)
            return .success(nil)
        case .removeSource(let lane):
            guard dualSource, captureLanes.indices.contains(lane) else {
                return .failure(TestActionRefusal(code: "not_found", message: "cannot remove lane \(lane)"))
            }
            removeLane(at: lane)
            return .success(nil)
        }
    }

    private func describePreviewPlan() -> String {
        guard let selection = waveform?.selectionForSaving() else { return "preview: no selection" }
        guard let material = waveform?.materialForExport(range: selection) else { return "preview: the selection is not frozen" }
        let pairMode = pairBinaural ? "Binaural" : "Stereo"
        let desc = "preview of \(selection.lowerBound)..\(selection.upperBound) (\(pairMode)): lanes \(captureLanes.count), rate \(material.sampleRate); prepared, not played"
        return desc
    }

    /// Package 4 / WIN-12: auto-grow full window for stacked channels or >2 sources (20px/row, capped at min(560, 85% screen)).
    func maybeAutoGrowWindow() {
        guard let panel, let waveform, !waveformOnly, !editing,
              TestCopyConfig.current.windowSizeOverride == nil else { return }
        let now = ProcessInfo.processInfo.systemUptime
        if now - lastManualResize < 1.0 { return }
        let currentContent = panel.contentRect(forFrameRect: panel.frame).size
        let currentAudioH = max(1, waveform.bounds.height)
        let chromeHeight = max(0, currentContent.height - currentAudioH)
        let screenMax = (panel.screen?.visibleFrame.height ?? 900) * 0.85
        let cap = min(560, screenMax)

        if let desiredAudio = waveform.desiredComfortableAudioHeight {
            let desiredTotal = min(cap, desiredAudio + chromeHeight)
            if desiredTotal > currentContent.height + 1 {
                if autoGrownBaseHeight == nil {
                    autoGrownBaseHeight = currentContent.height
                }
                if let prevTarget = autoGrownTargetHeight, desiredTotal <= prevTarget + 1 {
                    return
                }
                autoGrownTargetHeight = desiredTotal
                resizePanel(to: NSSize(width: currentContent.width, height: desiredTotal))
            }
        } else if let base = autoGrownBaseHeight {
            // Shrink back to pre-grow height once no stacked lane and <=2 sources, unless user manually resized.
            autoGrownBaseHeight = nil
            autoGrownTargetHeight = nil
            resizePanel(to: NSSize(width: currentContent.width, height: base))
        }
    }

    func onEditMaterialReady(_ material: WAVExport) {
        // An add-on's select waiting for this freeze is answered now, not at the next poll.
        answerSelectFreezeWaits()
        guard !pendingSnapshots.isEmpty else { return }
        let queued = pendingSnapshots
        pendingSnapshots.removeAll()
        for req in queued {
            executeSnapshotSave(material, request: req)
        }
    }

    func onEditMaterialUnavailable() {
        answerSelectFreezeWaits()
        failPendingSnapshots("No audio available for this selection.")
    }

    private func failPendingSnapshots(_ reason: String) {
        guard !pendingSnapshots.isEmpty else { return }
        let failed = pendingSnapshots
        pendingSnapshots.removeAll()
        for req in failed {
            if let call = req.controlCall {
                addonHost?.controlReply(call: call, error: AddonControlError(code: "failed", message: reason))
            }
        }
        setNotice(reason)
    }

    private func executeSnapshotSave(_ job: WAVExport, request: PendingSnapshotSave) {
        let range = request.range ?? job.span
        guard range.lowerBound >= job.span.lowerBound, range.upperBound <= job.span.upperBound, !range.isEmpty else {
            let msg = "The selected audio is unavailable."
            setNotice(msg)
            if let call = request.controlCall {
                addonHost?.controlReply(call: call, error: AddonControlError(code: "no_audio", message: msg))
            }
            return
        }
        writeExport(job, range: range, fades: request.fades, rows: request.rows, directory: request.directory,
                    provisional: false, controlCall: request.controlCall) { [weak self] urls in
            if !urls.isEmpty, let ticket = request.ticket {
                self?.waveform?.commitMarker(range, rows: request.rows, ticket: ticket)
            }
            return true
        }
    }

    private func saveSnapshot(into directoryPath: String, controlCall: AddonControlCall? = nil) {
        guard !editing else {
            if let call = controlCall {
                addonHost?.controlReply(call: call, error: AddonControlError(code: "busy", message: "Backtracking is busy editing; try again shortly"))
            }
            return
        }
        let selected = waveform?.selectionForSaving()
        let fades = waveform?.fadesForSaving() ?? noFades
        let rows = selected == nil ? nil : waveform?.selectedRowsForExport
        let ticket = waveform?.reserveMarker()
        let req = PendingSnapshotSave(
            range: selected, fades: fades, rows: rows, directory: directoryPath,
            ticket: ticket, controlCall: controlCall
        )
        if waveform?.isPreparingMaterial == true {
            pendingSnapshots.append(req)
            if selected != nil { waveform?.clearSelection() }
            setNotice("Preparing snapshot…")
            return
        }
        let save: (WAVExport?) -> Void = { [weak self] job in
            guard let self else { return }
            guard let job else {
                let msg = "No audio captured yet."
                self.setNotice(msg)
                if let call = controlCall {
                    self.addonHost?.controlReply(call: call, error: AddonControlError(code: "no_audio", message: msg))
                }
                return
            }
            if selected != nil { self.waveform?.clearSelection() }
            self.executeSnapshotSave(job, request: req)
        }
        if let material = waveform?.materialForExport(range: selected) {
            save(material)
        } else if let tap {
            setNotice("Preparing snapshot…")
            // Listed until its freeze arrives, so quitting first still writes it (Windows `pending_snapshot_reads`).
            let read = EditingMaterialRead(tap: ObjectIdentifier(tap))
            pendingSnapshotReads.append(SnapshotRead(read: read, save: req))
            requestEditingMaterial(read: read) { [weak self] job in
                guard let self, let index = self.pendingSnapshotReads.firstIndex(where: { $0.read === read }) else { return }
                self.pendingSnapshotReads.remove(at: index)
                save(job)
            }
        } else {
            save(nil)
        }
    }

    // MARK: - Stage B: Add-on Host Polling, Channel Groups (Pkg 3), Add-ons Page (Pkg 16/21),
    // Add-on Sources (Pkg 18), Positions & Clip Exported (Pkg 19), Look (Pkg 20), Control (Pkg 17)

    private func attach(addonHost host: AddonHostClient?) {
        addonHost = host
        // A completion that redraws must already see the snapshot that carried it.
        host?.onSnapshot = { currentAddonSnapshot = $0 }
        if let snap = host?.snapshot {
            currentAddonSnapshot = snap
            lastAddonSnapshotForUI = snap
        }
    }

    /// Brings the window up to the host's latest snapshot. Any read of the snapshot (a poll, or the
    /// grant check before each control request) can advance it, so this compares generations
    /// with what the window last showed instead of trusting one poll's answer.
    /// Whether the Appearance page's "Look from add-on" list and gradient rows show something new:
    /// another set of offering add-ons or names, or another gradient from the chosen look. A look
    /// that only changes its colours is applied without rebuilding them (Windows `OfferWatch`).
    static func lookControlsChanged(from prev: AddonSnapshot, to snap: AddonSnapshot, chosen: String?) -> Bool {
        let offering = { (s: AddonSnapshot) in s.appearances.keys.sorted().map { "\($0)\u{0}\(s.addonName(for: $0))" } }
        if offering(prev) != offering(snap) { return true }
        guard let chosen else { return false }
        return prev.appearances[chosen]?.spectrum != snap.appearances[chosen]?.spectrum
            || prev.addonName(for: chosen) != snap.addonName(for: chosen)
    }

    /// Whether the Add-ons page shows something new; looks alone do not appear there.
    /// A source's play position moves many times a second and is shown on the Sources page and
    /// the waveform's chip, not here.
    static func addonsPageChanged(from prev: AddonSnapshot, to snap: AddonSnapshot) -> Bool {
        func shown(_ addon: AddonRecord) -> AddonRecord {
            AddonRecord(id: addon.id, manifest: addon.manifest, enabled: addon.enabled, state: addon.state,
                        reason: addon.reason, pid: addon.pid, message: addon.message,
                        sources: addon.sources.map {
                            AddonSource(id: $0.id, name: $0.name, sender: $0.sender, formats: $0.formats,
                                        format: $0.format, groups: $0.groups, transport: nil)
                        },
                        settings: addon.settings, actions: addon.actions,
                        control_allowed: addon.control_allowed, control_asked: addon.control_asked)
        }
        return prev.addons.map(shown) != snap.addons.map(shown) || prev.ready != snap.ready || prev.error != snap.error
    }

    /// A menu is open or a mouse button is down in this app; controls are not swapped under it.
    /// A menu or an Options list is open, or a mouse button is down; controls are not swapped then.
    /// An Options list is a popover, which leaves the run loop in its default mode once the button is up.
    private var userIsHandlingControls: Bool {
        RunLoop.current.currentMode == .eventTracking || NSEvent.pressedMouseButtons != 0
            || OptionsPopUpButton.anyChoicePopoverShown
    }

    private func rebuildSourcesTab() {
        #if BACKTRACKING_CAPTURE_VERIFY
        verifySourcesTabRebuilds += 1
        #endif
        replaceOptionsTab(with: makeSourceTab())
        refreshSourceOptions()
    }

    /// Whether the Options window is on show at page `label`.
    private func optionsPageShown(_ label: String) -> Bool {
        #if BACKTRACKING_CAPTURE_VERIFY
        if let verifyOptionsPage { return verifyOptionsPage == label }
        #endif
        return optionsPanel?.isVisible == true && optionTabs?.selectedTabViewItem?.label == label
    }

    /// Whether a text field of the Options window is being typed into.
    private var editingOptionsText: Bool {
        #if BACKTRACKING_CAPTURE_VERIFY
        if let verifyEditingText { return verifyEditingText }
        #endif
        return optionsPanel?.firstResponder is NSTextView
    }

    private func syncAddonSnapshotUI() {
        guard let host = addonHost else { return }
        let snap = host.snapshot
        currentAddonSnapshot = snap
        // Rebuilds put off while the user held a menu or a control are done once they let go.
        if !userIsHandlingControls {
            if pendingLookControlsRebuild {
                pendingLookControlsRebuild = false
                recomputeLook(refreshAppearanceTab: true)
            }
            if pendingSourcesTabRebuild {
                pendingSourcesTabRebuild = false
                if optionsPageShown("Sources") { rebuildSourcesTab() }
            }
        }
        if pendingAddonsTabReload {
            // A page no longer on show is built afresh when it next shows; one being typed into
            // waits until the typing ends.
            if !optionsPageShown("Add-ons") {
                pendingAddonsTabReload = false
            } else if !userIsHandlingControls && !editingOptionsText {
                pendingAddonsTabReload = false
                reloadAddonsTab()
            }
        }
        guard snap.generation != lastAddonSnapshotForUI.generation else { return }
        let prev = lastAddonSnapshotForUI
        lastAddonSnapshotForUI = snap
        if prev.appearances != snap.appearances || prev.addons.map(\.id) != snap.addons.map(\.id) {
            let rebuild = Self.lookControlsChanged(from: prev, to: snap, chosen: selectedLookAddonID)
            if rebuild && userIsHandlingControls {
                pendingLookControlsRebuild = true
                recomputeLook(refreshAppearanceTab: false)
            } else {
                recomputeLook(refreshAppearanceTab: rebuild)
            }
        }
        // Refresh cached source choices if any running add-on sources changed.
        let sourcesChanged = !prev.addons.elementsEqual(snap.addons) { a, b in
            a.id == b.id && a.state == b.state && a.sources == b.sources
        }
        if sourcesChanged {
            cachedSourceChoices = nil
            if !paused, captureLanes.contains(where: { $0.source.isAddon }) {
                if tap?.refreshSources() == true { reconnectConfiguredSources() }
            }
            refreshWaveform()
        }
        if optionsPageShown("Add-ons") || optionsPageShown("Sources") {
            if optionsPageShown("Add-ons") {
                if Self.addonsPageChanged(from: prev, to: snap) {
                    if userIsHandlingControls || editingOptionsText {
                        pendingAddonsTabReload = true
                    } else {
                        reloadAddonsTab()
                    }
                }
            } else if sourcesChanged {
                let sourcesStructureChanged = !prev.addons.elementsEqual(snap.addons) { a, b in
                    a.id == b.id && a.state == b.state && a.sources.elementsEqual(b.sources) { s1, s2 in
                        s1.id == s2.id && s1.format == s2.format && s1.formats == s2.formats
                            && s1.sender == s2.sender && s1.groups == s2.groups
                    }
                }
                if sourcesStructureChanged {
                    if userIsHandlingControls {
                        pendingSourcesTabRebuild = true
                    } else {
                        rebuildSourcesTab()
                    }
                } else {
                    refreshAddonTransportControls()
                }
            }
        }
        waveform?.needsDisplay = true
    }

    private func pollAddonHost() {
        guard let host = addonHost else { return }
        host.pollSnapshot()
        syncAddonSnapshotUI()
        // Lightweight transport position refresh on Sources tab & main window chip.
        let now = ProcessInfo.processInfo.systemUptime
        if now - lastAddonTransportUIRefresh >= 0.2 {
            lastAddonTransportUIRefresh = now
            refreshAddonTransportControls()
        }
        // Poll positions independently of snapshot generation (Pkg 19).
        if let batch = host.positionsSince(cursor: addonPositionsCursor) {
            addonPositionsCursor = batch.cursor
            if !batch.positions.isEmpty || tap != nil {
                if tap?.ingestAddonPositions(batch.positions) == true {
                    setNotice("Position history was trimmed to its memory limit.")
                }
            }
        }
        answerSelectFreezeWaits()
        // Poll control commands on primary window only (Pkg 17).
        if !independentWindow {
            let calls = host.takeControl()
            if !calls.isEmpty {
                executeControlCalls(calls)
            }
        }
    }

    /// Files dragged out were written at the export rate in force when they were prepared.
    func notifyDraggedClip(urls: [URL], sourceFrames: UInt64, sourceRate: UInt32) {
        notifyClipExported(ClipExportLedger.Clip(urls: urls, sampleRate: exportSampleRate,
                                                 sourceFrames: sourceFrames, sourceRate: sourceRate))
    }

    func notifyClipExported(_ clip: ClipExportLedger.Clip,
                            completion: @escaping (Result<Void, MessageError>) -> Void = { _ in }) {
        guard let addonHost else { completion(.failure("The add-on manager is not running.")); return }
        addonHost.clipExported(files: clip.urls.map(\.path), sampleRate: clip.sampleRate, frames: clip.frames,
                               completion: completion)
    }

    // MARK: Pkg 3 — Channel Groups

    func currentShownGroupIDs() -> [String?] {
        captureLanes.map { shownChannelGroups[$0.source.key] }
    }

    func channelGroups(for lane: Int) -> [AddonChannelGroup] {
        tap?.channelGroups(for: lane) ?? []
    }

    func resolvedShownGroup(for lane: Int) -> (group: AddonChannelGroup, range: Range<Int>, hasBed: Bool)? {
        let groups = channelGroups(for: lane)
        guard !groups.isEmpty, captureLanes.indices.contains(lane) else { return nil }
        let preferred = shownChannelGroups[captureLanes[lane].source.key]
        var offset = 0
        var chosen: (AddonChannelGroup, Range<Int>)?
        var hasBed = false
        for g in groups {
            let end = offset + g.channels.count
            if !g.positioned { hasBed = true }
            if let preferred, g.id == preferred {
                chosen = (g, offset..<end)
            }
            offset = end
        }
        let pair = chosen ?? (groups[0], 0..<groups[0].channels.count)
        return (pair.0, pair.1, hasBed)
    }

    @discardableResult
    func selectChannelGroup(lane: Int, groupID: String) -> Bool {
        guard captureLanes.indices.contains(lane) else { return false }
        let groups = channelGroups(for: lane)
        guard groups.contains(where: { $0.id == groupID }) else { return false }
        let key = captureLanes[lane].source.key
        guard shownChannelGroups[key] != groupID else { return true }
        shownChannelGroups[key] = groupID
        defaults.set(shownChannelGroups, forKey: "shownChannelGroups")
        waveform?.onChannelGroupSwitched(lane: lane)
        refreshWaveform()
        updateStatus()
        return true
    }

    func cycleChannelGroup(lane: Int) {
        let groups = channelGroups(for: lane)
        guard groups.count >= 2, captureLanes.indices.contains(lane) else { return }
        let currentID = resolvedShownGroup(for: lane)?.group.id ?? groups[0].id
        let idx = groups.firstIndex(where: { $0.id == currentID }) ?? 0
        let next = groups[(idx + 1) % groups.count]
        _ = selectChannelGroup(lane: lane, groupID: next.id)
    }

    // MARK: Pkg 20 — Look from add-on

    private func offeredLookSpectrum() -> FrequencyGradient? {
        guard let id = selectedLookAddonID, let appearance = currentAddonSnapshot.appearances[id] else {
            return nil
        }
        return ResolvedLook.spectrum(from: appearance)
    }

    private func lookSpectrumOffererName() -> String? {
        guard let id = selectedLookAddonID, offeredLookSpectrum() != nil else { return nil }
        return currentAddonSnapshot.addonName(for: id)
    }

    /// The gradient a window's waveform starts with: the one in force, as every later change sets.
    private var openingWaveformGradient: FrequencyGradient? {
        frequencyColoring ? effectiveFrequencyGradient() : nil
    }

    func effectiveFrequencyGradient() -> FrequencyGradient {
        if hasCustomFrequencyGradient { return frequencyGradient }
        if let fromLook = offeredLookSpectrum() { return fromLook }
        return .standard
    }

    private func recomputeLook(refreshAppearanceTab: Bool) {
        let nextLook: ResolvedLook
        if let id = selectedLookAddonID, let appearance = currentAddonSnapshot.appearances[id] {
            nextLook = ResolvedLook.from(appearance: appearance, accentRGB: accentRGB)
        } else {
            nextLook = .default
        }
        currentLook = nextLook
        background?.look = nextLook
        waveform?.look = nextLook
        frequencyGradientPreview?.look = nextLook
        waveform?.frequencyGradient = frequencyColoring ? effectiveFrequencyGradient() : nil
        lookBorderWidthNote?.isHidden = nextLook.borderWidth == nil
        updateBackground()
        if refreshAppearanceTab {
            #if BACKTRACKING_CAPTURE_VERIFY
            verifyLookControlRebuilds += 1
            #endif
            refreshLookAddonPopup()
            refreshFrequencyGradientControls()
        }
    }

    private func chooseLookAddon(_ addonID: String?) {
        selectedLookAddonID = addonID
        if let addonID {
            defaults.set(addonID, forKey: "selectedLookAddon")
        } else {
            defaults.removeObject(forKey: "selectedLookAddon")
        }
        recomputeLook(refreshAppearanceTab: true)
        refreshWaveform()
    }

    private func refreshLookAddonPopup() {
        guard let popup = lookAddonPopup else { return }
        let offering: [(id: String, name: String)] = currentAddonSnapshot.appearances.keys.sorted().map {
            ($0, currentAddonSnapshot.addonName(for: $0))
        }
        popup.removeAllItems()
        popup.addItem(withTitle: "None")
        popup.lastItem?.representedObject = ""
        var selectedIndex = 0
        for item in offering {
            popup.addItem(withTitle: item.name)
            popup.lastItem?.representedObject = item.id
            if selectedLookAddonID == item.id { selectedIndex = popup.numberOfItems - 1 }
        }
        var missingChosenName: String?
        if let chosen = selectedLookAddonID, !offering.contains(where: { $0.id == chosen }) {
            let name = currentAddonSnapshot.addonName(for: chosen)
            missingChosenName = name
            popup.addItem(withTitle: name)
            popup.lastItem?.representedObject = chosen
            selectedIndex = popup.numberOfItems - 1
        }
        popup.selectItem(at: selectedIndex)
        if let note = lookAddonNote {
            if let missing = missingChosenName {
                note.stringValue = "\(missing) is not offering a look right now. Backtracking shows its own look until it does."
                note.isHidden = false
            } else if offering.isEmpty {
                note.stringValue = "No running add-on offers a look."
                note.isHidden = false
            } else {
                note.stringValue = ""
                note.isHidden = true
            }
        }
        lookBorderWidthNote?.isHidden = currentLook.borderWidth == nil
    }

    @objc private func lookAddonChanged(_ sender: NSPopUpButton) {
        let raw = (sender.selectedItem?.representedObject as? String) ?? ""
        chooseLookAddon(raw.isEmpty ? nil : raw)
        registerClick()
    }

    // MARK: Pkg 16 & 21 — Add-ons Page, Install Review, Settings, Actions, Remove

    private func reloadAddonsTab() {
        #if BACKTRACKING_CAPTURE_VERIFY
        verifyAddonsTabReloads += 1
        #endif
        guard optionTabs != nil else { return }
        replaceOptionsTab(with: makeAddonsTab())
    }

    private func openAddonsOptionsTab() {
        if optionsPanel == nil || optionsPanel?.isVisible != true { showOptions() }
        if let idx = optionCategoryButtons.firstIndex(where: { $0.title == "Add-ons" }) {
            selectOptionCategory(optionCategoryButtons[idx])
        }
    }

    private func openSourcesOptionsTab() {
        if optionsPanel == nil || optionsPanel?.isVisible != true { showOptions() }
        if let idx = optionCategoryButtons.firstIndex(where: { $0.title == "Sources" }) {
            selectOptionCategory(optionCategoryButtons[idx])
        }
    }

    private func cancelAddonPromptOnEscape() -> Bool {
        if let review = addonReview {
            if case .installing = review { return true }
            addonReview = nil
            reloadAddonsTab()
            return true
        }
        if let removal = addonRemoval {
            if !removal.busy {
                addonRemoval = nil
                reloadAddonsTab()
            }
            return true
        }
        return false
    }

    func handleDroppedFiles(_ urls: [URL]) -> Bool {
        let packages = urls.filter(AddonFormatting.isPackageURL)
        if let firstPkg = packages.first {
            openAddonsOptionsTab()
            beginAddonReview(url: firstPkg, others: packages.count - 1)
            return true
        }
        // Non-package file drop (ADD-W33).
        let snap = currentAddonSnapshot
        for url in urls where !AddonFormatting.isPackageURL(url) {
            let runningOpeners: [(id: String, name: String)] = snap.addons.compactMap { addon in
                guard addon.isRunning, let spec = addon.openFilesSpec, spec.accepts(url: url) else { return nil }
                return (addon.id, addon.displayName)
            }
            if runningOpeners.count == 1 {
                openFileWithAddon(id: runningOpeners[0].id, url: url)
                return true
            } else if runningOpeners.count > 1 {
                addonPendingDropChoice = (url, runningOpeners)
                openSourcesOptionsTab()
                replaceOptionsTab(with: makeSourceTab())
                refreshSourceOptions()
                return true
            } else {
                if let idle = snap.addons.first(where: { !$0.isRunning && ($0.openFilesSpec?.accepts(url: url) == true) }) {
                    setNotice("\(idle.displayName) opens this kind of file but is not running. Turn it on in Options, Add-ons.")
                    return true
                }
            }
        }
        return false
    }

    private func beginAddonReview(url: URL, others: Int) {
        if case .installing = addonReview {
            addonPageNotice = "Wait for the current installation to finish."
            reloadAddonsTab()
            return
        }
        guard let host = addonHost else {
            addonPageNotice = "Add-ons are not available in this window."
            reloadAddonsTab()
            return
        }
        let extraNote: String? = others > 0
            ? (others == 1
                ? "Only the first add-on is shown; the other add-on file was not opened."
                : "Only the first add-on is shown; the other \(others) add-on files were not opened.")
            : nil
        addonPageNotice = nil
        addonReview = .reading(url, note: extraNote)
        reloadAddonsTab()
        host.inspect(path: url.path) { [weak self] result in
            guard let self else { return }
            guard case .reading(let currentURL, let note) = self.addonReview, currentURL == url else { return }
            switch result {
            case .success(let inspected):
                let existing = currentAddonSnapshot.addon(inspected.manifest.id)
                let installedVer = existing?.manifest?.version
                let broken = existing != nil && existing?.manifest == nil
                self.addonReview = .ready(url, inspect: inspected, installedVersion: installedVer, brokenInstalled: broken, note: note)
            case .failure(let reason):
                self.addonReview = .refused(url, reason: reason.message, note: note)
            }
            self.reloadAddonsTab()
        }
    }

    private func makeAddonsTab() -> NSTabViewItem {
        addonSettingTextFields.removeAll()
        addonSettingNumberFields.removeAll()
        addonNumberSteps.removeAll()
        addonSettingControls.removeAll()
        guard addonHost != nil else {
            return optionsTab("Add-ons", groups: [
                optionGroup(nil, rows: [optionNote("Add-ons are not available in this window.")])
            ])
        }
        if let review = addonReview {
            return optionsTab("Add-ons", groups: [makeAddonReviewGroup(review)])
        }

        let snap = currentAddonSnapshot
        var groups: [NSView] = []
        var topRows: [NSView] = []
        if let notice = addonPageNotice, !notice.isEmpty {
            let lbl = optionNote(notice)
            lbl.textColor = .labelColor
            topRows.append(lbl)
        }
        if let err = snap.error, !err.isEmpty {
            let errLbl = optionNote("Add-ons are unavailable: \(err)")
            errLbl.textColor = WaveformTheme.statusUnavailable
            topRows.append(errLbl)
        }
        if snap.addons.isEmpty {
            let emptyHead = optionNote("No add-ons are installed.")
            emptyHead.textColor = .labelColor
            topRows.append(emptyHead)
            topRows.append(optionNote("An add-on is a separate program that adds features to Backtracking or changes how it works, for example a new audio source with its own settings and buttons."))
            topRows.append(optionNote("To add one, choose Install from file, or drop a .btaddon file on a Backtracking window."))
        }
        if !topRows.isEmpty {
            groups.append(optionGroup(nil, rows: topRows))
        }
        for addon in snap.addons {
            groups.append(makeAddonCardGroup(addon))
        }
        let installBtn = NSButton(title: "Install from file…", target: self, action: #selector(installAddonFromFile))
        var footerRows: [NSView] = [buttonRow([installBtn])]
        if !snap.addons.isEmpty {
            footerRows.append(optionNote("You can also drop a .btaddon file on a Backtracking window."))
        }
        groups.append(optionGroup(nil, rows: footerRows))
        return optionsTab("Add-ons", groups: groups)
    }

    private func makeAddonReviewGroup(_ review: AddonReviewStage) -> NSStackView {
        var rows: [NSView] = []
        switch review {
        case .reading(let url, let note):
            if let note { rows.append(optionNote(note)) }
            rows.append(optionNote("Reading \(url.lastPathComponent)…"))
            rows.append(buttonRow([NSButton(title: "Cancel", target: self, action: #selector(cancelAddonReview))]))
            return optionGroup("Install this add-on?", rows: rows)

        case .refused(let url, let reason, let note):
            if let note { rows.append(optionNote(note)) }
            let heading = optionNote("This file cannot be installed.")
            heading.textColor = WaveformTheme.statusUnavailable
            rows.append(heading)
            rows.append(optionNote(reason))
            rows.append(optionField("Package file", optionNote(url.path)))
            rows.append(buttonRow([NSButton(title: "Close", target: self, action: #selector(cancelAddonReview))]))
            return optionGroup("Install this add-on?", rows: rows)

        case .ready(let url, let info, let installedVer, let broken, let note),
             .installing(let url, let info, let installedVer, let broken, let note),
             .failed(let url, let info, let installedVer, let broken, _, let note):
            if let note { rows.append(optionNote(note)) }
            let m = info.manifest
            let title = NSTextField(labelWithString: m.name)
            title.font = .systemFont(ofSize: 16, weight: .bold)
            rows.append(title)
            rows.append(optionNote("Version \(m.version) · by \(m.author)"))
            if !m.description.isEmpty {
                let desc = optionNote(m.description)
                desc.textColor = .labelColor
                rows.append(desc)
            }
            if let homepage = m.homepage, AddonFormatting.linkIsOpenable(homepage) {
                let linkBtn = NSButton(title: homepage, target: self, action: #selector(openAddonHomepage(_:)))
                linkBtn.isBordered = false
                linkBtn.contentTintColor = .linkColor
                linkBtn.alignment = .left
                linkBtn.toolTip = homepage
                rows.append(optionField("Homepage", linkBtn))
            }
            if let license = m.license, !license.isEmpty {
                rows.append(optionField("License", optionNote(license)))
            }
            rows.append(optionField("Program it runs", optionNote(info.program)))
            rows.append(optionField("Unpacked size", optionNote(AddonFormatting.sizeInFiles(bytes: info.unpacked_size, files: info.file_count))))
            rows.append(optionField("Package file", optionNote(url.path)))

            let capsHeading = NSTextField(labelWithString: "What it asks for")
            capsHeading.font = .systemFont(ofSize: 14, weight: .semibold)
            rows.append(capsHeading)
            for capLine in AddonFormatting.capabilityLines(for: m) {
                let l = optionNote("• \(capLine)")
                l.textColor = .labelColor
                rows.append(l)
            }
            let trust = optionNote("An add-on is a program that runs with your own permissions. It is not sandboxed: it can do anything you can do on this computer, whatever the list above says. Install only add-ons you trust.")
            trust.textColor = NSColor(srgbRed: 246 / 255, green: 196 / 255, blue: 118 / 255, alpha: 1)
            rows.append(trust)

            if let installedVer {
                rows.append(optionNote("This replaces version \(installedVer) with version \(m.version). Its settings and saved data are kept."))
            } else if broken {
                rows.append(optionNote("This replaces a broken installation of the same add-on."))
            } else {
                rows.append(optionNote("Once installed, it starts at once and each time Backtracking starts. You can turn it off on the Add-ons page."))
            }

            if case .installing = review {
                rows.append(optionNote("Installing…"))
            } else if case .failed(_, _, _, _, let reason, _) = review {
                let err = optionNote("Could not install it: \(reason)")
                err.textColor = WaveformTheme.statusUnavailable
                rows.append(err)
                if let installedVer {
                    rows.append(optionNote("The previous version, \(installedVer), is still installed."))
                } else if broken {
                    rows.append(optionNote("The broken installation was left as it was."))
                } else {
                    rows.append(optionNote("Nothing was installed."))
                }
                rows.append(buttonRow([NSButton(title: "Close", target: self, action: #selector(cancelAddonReview))]))
            } else {
                let installBtn = NSButton(title: "Install", target: self, action: #selector(confirmAddonInstall))
                let cancelBtn = NSButton(title: "Cancel", target: self, action: #selector(cancelAddonReview))
                rows.append(buttonRow([installBtn, cancelBtn]))
            }
            return optionGroup("Install this add-on?", rows: rows)
        }
    }

    private func makeAddonCardGroup(_ addon: AddonRecord) -> NSStackView {
        var rows: [NSView] = []
        if let m = addon.manifest {
            rows.append(optionNote("Version \(m.version) · by \(m.author)"))
            if !m.description.isEmpty {
                rows.append(optionNote(m.description))
            }
        }
        let status = addon.statusPresentation
        let statusLbl = optionNote(status.text)
        switch status.tone {
        case .running: statusLbl.textColor = accentColor(accentRGB)
        case .busy: statusLbl.textColor = NSColor(white: 170 / 255, alpha: 1)
        case .off: statusLbl.textColor = NSColor(white: 140 / 255, alpha: 1)
        case .problem: statusLbl.textColor = NSColor(srgbRed: 246 / 255, green: 196 / 255, blue: 118 / 255, alpha: 1)
        }
        rows.append(statusLbl)
        if addon.isBroken {
            rows.append(optionNote("Remove it, or install the package again."))
        }
        if let msg = addon.message {
            let msgLbl = optionNote(AddonFormatting.addonMessageText(msg))
            switch msg.level {
            case "error": msgLbl.textColor = WaveformTheme.statusUnavailable
            case "warning": msgLbl.textColor = NSColor(srgbRed: 246 / 255, green: 196 / 255, blue: 118 / 255, alpha: 1)
            default: msgLbl.textColor = .labelColor
            }
            rows.append(msgLbl)
        }

        let enabledCheck = NSButton(checkboxWithTitle: "Enabled", target: self, action: #selector(addonEnabledChanged(_:)))
        // While a change waits for its answer the switch shows it and cannot be changed again (Windows `addons_page`).
        enabledCheck.state = (addonEnabledChanging[addon.id] ?? addon.enabled) ? .on : .off
        enabledCheck.isEnabled = !addon.isBroken && addonEnabledChanging[addon.id] == nil
        enabledCheck.identifier = NSUserInterfaceItemIdentifier("addon-enabled:\(addon.id)")
        var headerButtons: [NSButton] = [enabledCheck]
        if addon.canStartAgain {
            let restartBtn = NSButton(title: "Start again", target: self, action: #selector(addonStartAgain(_:)))
            restartBtn.identifier = NSUserInterfaceItemIdentifier("addon-start:\(addon.id)")
            headerButtons.append(restartBtn)
        }
        rows.append(buttonRow(headerButtons))

        if let problem = addonProblems["\(addon.id):"] {
            let pLbl = optionNote(problem)
            pLbl.textColor = WaveformTheme.statusUnavailable
            rows.append(pLbl)
        }

        // Control consent switch (Pkg 17).
        if addon.declaresControl {
            let controlCheck = NSButton(
                checkboxWithTitle: "Let this add-on control Backtracking (start and pause recording, select, save files, change sources)",
                target: self, action: #selector(addonControlAllowedChanged(_:))
            )
            controlCheck.state = (addonControlChanging[addon.id] ?? addon.control_allowed) ? .on : .off
            controlCheck.isEnabled = addonControlChanging[addon.id] == nil && addonRemoval?.id != addon.id
            controlCheck.identifier = NSUserInterfaceItemIdentifier("addon-control:\(addon.id)")
            rows.append(controlCheck)
            if addon.control_asked && !addon.control_allowed {
                let refused = optionNote("It asked to control Backtracking and was refused")
                refused.textColor = NSColor(srgbRed: 246 / 255, green: 196 / 255, blue: 118 / 255, alpha: 1)
                rows.append(refused)
            }
            if let cp = addonProblems["\(addon.id):control:"] {
                let pLbl = optionNote(cp)
                pLbl.textColor = WaveformTheme.statusUnavailable
                rows.append(pLbl)
            }
        }

        // Settings & Actions while running.
        if addon.isRunning {
            if !addon.settings.isEmpty {
                let heading = NSTextField(labelWithString: "Settings")
                heading.font = .systemFont(ofSize: 14, weight: .semibold)
                rows.append(heading)
                for entry in addon.settings {
                    rows.append(makeAddonSettingView(addonID: addon.id, entry: entry))
                }
            }
            if !addon.actions.isEmpty {
                let heading = NSTextField(labelWithString: "Actions")
                heading.font = .systemFont(ofSize: 14, weight: .semibold)
                rows.append(heading)
                let buttons = addon.actions.map { act -> NSButton in
                    let b = NSButton(title: act.label, target: self, action: #selector(addonRunAction(_:)))
                    b.identifier = NSUserInterfaceItemIdentifier("addon-action:\(addon.id):\(act.id):\(act.label)")
                    return b
                }
                for start in stride(from: 0, to: buttons.count, by: 2) {
                    let end = min(start + 2, buttons.count)
                    rows.append(buttonRow(Array(buttons[start..<end])))
                }
                for act in addon.actions {
                    if let prob = addonProblems["\(addon.id):action:\(act.id)"] {
                        let pLbl = optionNote(prob)
                        pLbl.textColor = WaveformTheme.statusUnavailable
                        rows.append(pLbl)
                    }
                }
            }
        } else if !addon.isBroken && (addon.manifest?.hasCapability("settings") == true || addon.manifest?.hasCapability("actions") == true) {
            rows.append(optionNote("Its settings and buttons appear here while it is running."))
        }

        // Remove inline confirmation or Remove… button.
        if let rem = addonRemoval, rem.id == addon.id {
            let q = optionNote("Remove \(rem.name)?")
            q.textColor = .labelColor
            rows.append(q)
            rows.append(optionNote("Its program is deleted, and its sources leave the source list."))
            let delCheck = NSButton(checkboxWithTitle: "Also delete its saved data", target: self, action: #selector(addonRemoveDeleteDataChanged(_:)))
            delCheck.state = rem.deleteData ? .on : .off
            delCheck.isEnabled = !rem.busy
            rows.append(delCheck)
            if !rem.deleteData {
                rows.append(optionNote("Unless you delete it, its saved data stays for a later install."))
            }
            if let err = rem.error {
                let eLbl = optionNote(err)
                eLbl.textColor = WaveformTheme.statusUnavailable
                rows.append(eLbl)
            }
            if rem.busy {
                rows.append(optionNote("Removing…"))
            } else {
                let confirmBtn = NSButton(title: "Remove", target: self, action: #selector(addonConfirmRemove))
                let cancelBtn = NSButton(title: "Cancel", target: self, action: #selector(addonCancelRemove))
                rows.append(buttonRow([confirmBtn, cancelBtn]))
            }
        } else {
            let removeBtn = NSButton(title: "Remove…", target: self, action: #selector(addonAskRemove(_:)))
            removeBtn.identifier = NSUserInterfaceItemIdentifier("addon-remove:\(addon.id)")
            rows.append(buttonRow([removeBtn]))
        }

        return optionGroup(addon.displayName, rows: rows)
    }

    private func makeAddonSettingView(addonID: String, entry: AddonSettingEntry) -> NSView {
        let decl = entry.declaration
        let draftKey = "\(addonID):\(decl.key)"
        let current = addonSettingDrafts[draftKey] ?? entry.value
        // A change the add-on has not answered yet cannot be changed again (Windows `add_enabled_ui(!pending)`).
        let pending = addonSettingDrafts[draftKey] != nil
        var below: [NSView] = []
        if let help = decl.help, !help.isEmpty {
            below.append(optionNote(help))
        }
        if let prob = addonProblems[draftKey] {
            let pLbl = optionNote(prob)
            pLbl.textColor = WaveformTheme.statusUnavailable
            below.append(pLbl)
        }
        switch decl.kind {
        case "toggle":
            let check = NSButton(checkboxWithTitle: decl.label, target: self, action: #selector(addonSettingToggleChanged(_:)))
            let on = current.boolValue ?? decl.defaultValue.boolValue ?? false
            check.state = on ? .on : .off
            check.isEnabled = !pending
            addonSettingControls[draftKey, default: []].append(check)
            check.identifier = NSUserInterfaceItemIdentifier("addon-toggle:\(addonID):\(decl.key):\(decl.label)")
            if below.isEmpty { return check }
            return optionsStack([check] + below, spacing: 4)

        case "choice":
            let opts = decl.options ?? []
            let popup = OptionsPopUpButton(frame: .zero, pullsDown: false)
            popup.target = self
            popup.action = #selector(addonSettingChoiceChanged(_:))
            popup.autoenablesItems = false
            prepareOptionPopup(popup)
            popup.identifier = NSUserInterfaceItemIdentifier("addon-choice:\(addonID):\(decl.key):\(decl.label)")
            let wanted = current.stringValue ?? decl.defaultValue.stringValue ?? ""
            var selectedIdx = 0
            for (idx, opt) in opts.enumerated() {
                popup.addItem(withTitle: opt.label)
                popup.lastItem?.representedObject = opt.value
                if opt.value == wanted { selectedIdx = idx }
            }
            if !opts.isEmpty { popup.selectItem(at: selectedIdx) }
            popup.isEnabled = !pending
            addonSettingControls[draftKey, default: []].append(popup)
            return optionField(decl.label, popup, below: below)

        case "number":
            let minV = decl.min ?? 0
            let maxV = decl.max ?? 65535
            let stepV = decl.step ?? 1
            let val = AddonJSONValue.snapNumber(
                current.doubleValue ?? decl.defaultValue.doubleValue ?? minV,
                min: minV, max: maxV, step: stepV
            )
            let decimals = AddonJSONValue.stepDecimals(stepV)
            let formatted = decimals == 0 ? String(format: "%.0f", val) : String(format: "%.\(decimals)f", val)
            let field = NSTextField(string: formatted)
            field.target = self
            field.action = #selector(addonSettingNumberChanged(_:))
            field.isEnabled = !pending
            addonSettingNumberFields[field] = (addonID, decl.key, decl.label, minV, maxV, stepV)
            // − and + move one step, as on Windows.
            let down = NSButton(title: "−", target: self, action: #selector(addonSettingNumberStep(_:)))
            let up = NSButton(title: "+", target: self, action: #selector(addonSettingNumberStep(_:)))
            down.isEnabled = !pending && val > minV
            up.isEnabled = !pending && val < maxV
            addonNumberSteps[down] = (field, false)
            addonNumberSteps[up] = (field, true)
            addonSettingControls[draftKey, default: []] += [down, field, up]
            field.widthAnchor.constraint(greaterThanOrEqualToConstant: 96).isActive = true
            let row = NSStackView(views: [down, field, up])
            row.orientation = .horizontal
            row.spacing = 4
            return optionField(decl.label, row, below: below)

        default: // "text"
            let val = current.stringValue ?? decl.defaultValue.stringValue ?? ""
            let field = NSTextField(string: val)
            field.target = self
            field.action = #selector(addonSettingTextChanged(_:))
            field.isEnabled = !pending
            addonSettingControls[draftKey, default: []].append(field)
            addonSettingTextFields[field] = (addonID, decl.key, decl.label)
            return optionField(decl.label, field, below: below)
        }
    }

    @objc private func installAddonFromFile() {
        let dialog = NSOpenPanel()
        dialog.title = "Install an add-on"
        dialog.canChooseFiles = true
        dialog.canChooseDirectories = false
        dialog.allowsMultipleSelection = false
        if let btaddonType = UTType(filenameExtension: "btaddon") {
            dialog.allowedContentTypes = [btaddonType]
        }
        let response = dialog.runModal()
        guard response == .OK, let url = dialog.url else { return }
        beginAddonReview(url: url, others: 0)
    }

    @objc private func confirmAddonInstall() {
        guard let host = addonHost,
              case .ready(let url, let inspect, let installedVer, let broken, let note) = addonReview else { return }
        let wasInstalled = installedVer != nil || broken
        addonReview = .installing(url, inspect: inspect, installedVersion: installedVer, brokenInstalled: broken, note: note)
        reloadAddonsTab()
        host.install(path: url.path, reviewID: inspect.review_id) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let summary):
                self.addonReview = nil
                self.addonPageNotice = "Installed \(summary.manifest.name) \(summary.manifest.version)."
                if !wasInstalled {
                    host.setEnabled(id: summary.manifest.id, enabled: true) { [weak self] enRes in
                        if case .failure(let err) = enRes {
                            self?.addonProblems["\(summary.manifest.id):"] = "Could not turn it on: \(err)"
                            self?.reloadAddonsTab()
                        }
                    }
                }
            case .failure(let reason):
                self.addonReview = .failed(
                    url, inspect: inspect, installedVersion: installedVer,
                    brokenInstalled: broken, reason: reason.message, note: note
                )
            }
            self.reloadAddonsTab()
        }
    }

    @objc private func cancelAddonReview() {
        addonReview = nil
        reloadAddonsTab()
    }

    @objc private func openAddonHomepage(_ sender: NSButton) {
        let link = sender.title
        guard AddonFormatting.linkIsOpenable(link), let url = URL(string: link) else { return }
        if !NSWorkspace.shared.open(url) {
            addonPageNotice = "Could not open the link: \(link)"
            reloadAddonsTab()
        }
    }

    @objc private func addonEnabledChanged(_ sender: NSButton) {
        guard let raw = sender.identifier?.rawValue, raw.hasPrefix("addon-enabled:"), let host = addonHost else { return }
        let id = String(raw.dropFirst("addon-enabled:".count))
        let on = sender.state == .on
        addonProblems.removeValue(forKey: "\(id):")
        addonEnabledChanging[id] = on
        sender.isEnabled = false
        host.setEnabled(id: id, enabled: on) { [weak self] result in
            self?.addonEnabledChanging.removeValue(forKey: id)
            if case .failure(let reason) = result {
                self?.addonProblems["\(id):"] = on ? "Could not turn it on: \(reason)" : "Could not turn it off: \(reason)"
            }
            self?.reloadAddonsTab()
        }
    }

    @objc private func addonControlAllowedChanged(_ sender: NSButton) {
        guard let raw = sender.identifier?.rawValue, raw.hasPrefix("addon-control:"), let host = addonHost else { return }
        let id = String(raw.dropFirst("addon-control:".count))
        let on = sender.state == .on
        addonProblems.removeValue(forKey: "\(id):control:")
        addonControlChanging[id] = on
        sender.isEnabled = false
        host.setControlAllowed(id: id, allowed: on) { [weak self] result in
            self?.addonControlChanging.removeValue(forKey: id)
            if case .failure(let reason) = result {
                self?.addonProblems["\(id):control:"] = on ? "Could not turn it on: \(reason)" : "Could not turn it off: \(reason)"
            }
            self?.reloadAddonsTab()
        }
    }

    @objc private func addonStartAgain(_ sender: NSButton) {
        guard let raw = sender.identifier?.rawValue, raw.hasPrefix("addon-start:"), let host = addonHost else { return }
        let id = String(raw.dropFirst("addon-start:".count))
        addonProblems.removeValue(forKey: "\(id):")
        host.start(id: id) { [weak self] result in
            if case .failure(let reason) = result {
                self?.addonProblems["\(id):"] = "Could not start it: \(reason)"
                self?.reloadAddonsTab()
            }
        }
    }

    @objc private func addonSettingToggleChanged(_ sender: NSButton) {
        guard let raw = sender.identifier?.rawValue, raw.hasPrefix("addon-toggle:"), let host = addonHost else { return }
        let parts = raw.dropFirst("addon-toggle:".count).split(separator: ":", maxSplits: 2).map(String.init)
        guard parts.count == 3 else { return }
        let (id, key, label) = (parts[0], parts[1], parts[2])
        let value = AddonJSONValue.bool(sender.state == .on)
        submitAddonSetting(host: host, id: id, key: key, label: label, value: value)
    }

    @objc private func addonSettingChoiceChanged(_ sender: NSPopUpButton) {
        guard let raw = sender.identifier?.rawValue, raw.hasPrefix("addon-choice:"),
              let val = sender.selectedItem?.representedObject as? String, let host = addonHost else { return }
        let parts = raw.dropFirst("addon-choice:".count).split(separator: ":", maxSplits: 2).map(String.init)
        guard parts.count == 3 else { return }
        submitAddonSetting(host: host, id: parts[0], key: parts[1], label: parts[2], value: .string(val))
    }

    @objc private func addonSettingNumberChanged(_ sender: NSTextField) {
        guard let info = addonSettingNumberFields[sender], let host = addonHost else { return }
        let parsed = Double(sender.stringValue.trimmingCharacters(in: .whitespaces)) ?? info.min
        let json = AddonJSONValue.snappedNumber(parsed, min: info.min, max: info.max, step: info.step)
        submitAddonSetting(host: host, id: info.addonID, key: info.key, label: info.label, value: json)
    }

    @objc private func addonSettingNumberStep(_ sender: NSButton) {
        guard let (field, up) = addonNumberSteps[sender], let info = addonSettingNumberFields[field],
              let host = addonHost else { return }
        let shown = Double(field.stringValue.trimmingCharacters(in: .whitespaces)) ?? info.min
        let json = AddonJSONValue.snappedNumber(shown + (up ? info.step : -info.step), min: info.min, max: info.max, step: info.step)
        submitAddonSetting(host: host, id: info.addonID, key: info.key, label: info.label, value: json)
    }

    @objc private func addonSettingTextChanged(_ sender: NSTextField) {
        guard let info = addonSettingTextFields[sender], let host = addonHost else { return }
        let clipped = String(sender.stringValue.prefix(200))
        submitAddonSetting(host: host, id: info.addonID, key: info.key, label: info.label, value: .string(clipped))
    }

    private func submitAddonSetting(host: AddonHostClient, id: String, key: String, label: String, value: AddonJSONValue) {
        let slot = "\(id):\(key)"
        addonProblems.removeValue(forKey: slot)
        addonSettingDrafts[slot] = value
        // Until the add-on answers, the setting shows the value asked for and cannot be changed again.
        for control in addonSettingControls[slot] ?? [] { control.isEnabled = false }
        reloadAddonsTab()
        host.setSetting(id: id, key: key, value: value) { [weak self] result in
            guard let self else { return }
            self.addonSettingDrafts.removeValue(forKey: slot)
            if case .failure(let reason) = result {
                self.addonProblems[slot] = "Could not change \(label): \(reason)"
            }
            self.reloadAddonsTab()
        }
    }

    @objc private func addonRunAction(_ sender: NSButton) {
        guard let raw = sender.identifier?.rawValue, raw.hasPrefix("addon-action:"), let host = addonHost else { return }
        let parts = raw.dropFirst("addon-action:".count).split(separator: ":", maxSplits: 2).map(String.init)
        guard parts.count == 3 else { return }
        let (id, actionID, label) = (parts[0], parts[1], parts[2])
        let slot = "\(id):action:\(actionID)"
        addonProblems.removeValue(forKey: slot)
        host.runAction(id: id, action: actionID) { [weak self] result in
            if case .failure(let reason) = result {
                self?.addonProblems[slot] = "Could not run \(label): \(reason)"
                self?.reloadAddonsTab()
            }
        }
    }

    @objc private func addonAskRemove(_ sender: NSButton) {
        guard let raw = sender.identifier?.rawValue, raw.hasPrefix("addon-remove:") else { return }
        let id = String(raw.dropFirst("addon-remove:".count))
        let name = currentAddonSnapshot.addonName(for: id)
        addonRemoval = (id: id, name: name, deleteData: false, busy: false, error: nil)
        reloadAddonsTab()
    }

    @objc private func addonRemoveDeleteDataChanged(_ sender: NSButton) {
        guard var rem = addonRemoval else { return }
        rem.deleteData = sender.state == .on
        addonRemoval = rem
        reloadAddonsTab()
    }

    @objc private func addonConfirmRemove() {
        guard var rem = addonRemoval, !rem.busy, let host = addonHost else { return }
        rem.busy = true
        rem.error = nil
        addonRemoval = rem
        reloadAddonsTab()
        let removeID = rem.id
        let removeName = rem.name
        host.remove(id: removeID, deleteData: rem.deleteData) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success:
                self.addonRemoval = nil
                self.addonPageNotice = "Removed \(removeName)."
                self.addonProblems = self.addonProblems.filter { !$0.key.hasPrefix("\(removeID):") }
                self.addonSettingDrafts = self.addonSettingDrafts.filter { !$0.key.hasPrefix("\(removeID):") }
            case .failure(let reason):
                if var cur = self.addonRemoval, cur.id == removeID {
                    cur.busy = false
                    cur.error = "Could not remove it: \(reason)"
                    self.addonRemoval = cur
                }
            }
            self.reloadAddonsTab()
        }
    }

    @objc private func addonCancelRemove() {
        addonRemoval = nil
        reloadAddonsTab()
    }

    // MARK: Pkg 18 — Add-on Sources on Sources Page & Main Window Transport Chip

    private func makeAddonSourceLaneRows(laneIndex: Int, addonID: String, sourceID: String) -> (views: [NSView], hideRoute: Bool) {
        var views: [NSView] = []
        let snap = currentAddonSnapshot
        let fallbackName = defaultFileName(for: .addon(addonID: addonID, sourceID: sourceID))

        guard let addon = snap.addon(addonID) else {
            let msg = optionNote("The add-on that provides \"\(fallbackName)\" is not installed.")
            msg.textColor = WaveformTheme.statusUnavailable
            views.append(msg)
            views.append(optionNote("Install it on the Add-ons page, or choose another source."))
            views.append(buttonRow([NSButton(title: "Open the Add-ons page", target: self, action: #selector(openAddonsFromSources))]))
            return (views, true)
        }
        guard addon.isRunning else {
            let msg = optionNote("\(addon.displayName) is not available. \(addon.statusPresentation.text)")
            msg.textColor = WaveformTheme.statusUnavailable
            views.append(msg)
            views.append(optionNote("Turn it on or start it again on the Add-ons page, or choose another source."))
            views.append(buttonRow([NSButton(title: "Open the Add-ons page", target: self, action: #selector(openAddonsFromSources))]))
            return (views, true)
        }
        guard let source = addon.sources.first(where: { $0.id == sourceID }) else {
            let msg = optionNote("\(addon.displayName) no longer offers \"\(fallbackName)\".")
            msg.textColor = WaveformTheme.statusUnavailable
            views.append(msg)
            views.append(optionNote("Choose another source. If you expected it, restart the add-on on the Add-ons page."))
            views.append(buttonRow([NSButton(title: "Open the Add-ons page", target: self, action: #selector(openAddonsFromSources))]))
            return (views, true)
        }

        let channels = tap?.channelCount(for: laneIndex) ?? UInt32(source.allChannelLabels.count)
        let isWide = channels > 2

        // Format picker.
        if !source.formats.isEmpty {
            let slotKey = "\(addonID):\(sourceID)"
            let now = ProcessInfo.processInfo.systemUptime
            let activeFormat: String
            if let pending = addonPendingFormats[slotKey], now - pending.at < 5.0, source.format != pending.format {
                activeFormat = pending.format
            } else {
                activeFormat = source.format
            }
            let fmtPopup = OptionsPopUpButton(frame: .zero, pullsDown: false)
            fmtPopup.target = self
            fmtPopup.action = #selector(addonFormatChanged(_:))
            fmtPopup.autoenablesItems = false
            fmtPopup.tag = laneIndex
            prepareOptionPopup(fmtPopup)
            var selectedIdx = 0
            var found = false
            for (i, fmt) in source.formats.enumerated() {
                fmtPopup.addItem(withTitle: fmt.label)
                fmtPopup.lastItem?.representedObject = fmt.id
                if fmt.id == activeFormat {
                    selectedIdx = i
                    found = true
                }
            }
            var below: [NSView] = []
            if !found && !activeFormat.isEmpty {
                fmtPopup.addItem(withTitle: activeFormat)
                fmtPopup.lastItem?.isEnabled = false
                selectedIdx = fmtPopup.numberOfItems - 1
                let warn = optionNote("The add-on reports the format \"\(activeFormat)\", which it does not list.")
                warn.textColor = WaveformTheme.statusUnavailable
                below.append(warn)
            }
            fmtPopup.selectItem(at: selectedIdx)
            if channels > 0 {
                below.append(optionNote(channels == 1 ? "1 channel" : "\(channels) channels"))
            }
            if isWide {
                below.append(optionNote("Each channel of a wide format has its own row, so there is no route to choose."))
            }
            views.append(optionField("Format", fmtPopup, below: below))
        }

        // Open file…
        if let spec = addon.openFilesSpec {
            let openBtn = NSButton(title: "Open file…", target: self, action: #selector(addonOpenFileButton(_:)))
            openBtn.identifier = NSUserInterfaceItemIdentifier("addon-open:\(addonID)")
            let help = optionNote("Opens \(spec.description) (\(spec.formattedExtensions)). You can also drop the file on a window.")
            views.append(optionField("File", buttonRow([openBtn]), below: [help]))
        }

        // Playback transport.
        if let tr = source.transport {
            let playBtn = NSButton(title: tr.isPlaying ? "Pause" : "Play", target: self, action: #selector(addonTransportToggle(_:)))
            playBtn.tag = laneIndex
            addonTransportPlayButtons[laneIndex] = playBtn
            let stopBtn = NSButton(title: "Stop", target: self, action: #selector(addonTransportStop(_:)))
            stopBtn.tag = laneIndex
            let pos = shownTransportPosition(addonID: addonID, sourceID: sourceID, transport: tr)
            let timeText = tr.duration.map { "\(AddonFormatting.clock(pos.position)) / \(AddonFormatting.clock($0))" }
                ?? AddonFormatting.clock(pos.position)
            let timeLbl = NSTextField(labelWithString: timeText)
            timeLbl.font = .monospacedDigitSystemFont(ofSize: 13, weight: .regular)
            addonTransportTimeLabels[laneIndex] = timeLbl
            var transportRows: [NSView] = [buttonRow([playBtn, stopBtn]), timeLbl]
            if let dur = tr.duration, dur.isFinite, dur > 0 {
                let slider = NSSlider(value: min(dur, max(0, pos.position)), minValue: 0, maxValue: dur,
                                      target: self, action: #selector(addonTransportSliderChanged(_:)))
                slider.tag = laneIndex
                slider.isContinuous = false
                addonTransportSliders[laneIndex] = slider
                transportRows.append(slider)
            }
            let seekNote = optionNote(pos.seeking ? "Moving to \(AddonFormatting.clock(pos.position)). The sound returns in a moment." : "")
            seekNote.isHidden = !pos.seeking
            addonTransportSeekNotes[laneIndex] = seekNote
            transportRows.append(seekNote)
            views.append(optionField("Playback", optionsStack(transportRows, spacing: 6)))
        }

        if let msg = addon.message {
            let mLbl = optionNote(AddonFormatting.addonMessageText(msg))
            if msg.level == "error" { mLbl.textColor = WaveformTheme.statusUnavailable }
            views.append(mLbl)
            // An error also says what can be done about it (Windows `addon_error_hint`).
            if msg.level == "error" {
                views.append(optionNote("Try another file or format, or restart the add-on on the Add-ons page."))
            }
        }
        if let prob = addonSourceProblems[addonID] {
            let pLbl = optionNote(prob)
            pLbl.textColor = WaveformTheme.statusUnavailable
            views.append(pLbl)
        }
        return (views, isWide)
    }

    private func shownTransportPosition(addonID: String, sourceID: String, transport: AddonTransport) -> (position: Double, seeking: Bool) {
        let key = "\(addonID):\(sourceID)"
        let now = ProcessInfo.processInfo.systemUptime
        if let seek = addonSeekingTargets[key], now - seek.at < 3.0, abs(transport.position - seek.target) > 1.0 {
            return (seek.target, true)
        }
        addonSeekingTargets.removeValue(forKey: key)
        return (transport.position, false)
    }

    private func refreshAddonTransportControls() {
        for (laneIndex, lane) in captureLanes.enumerated() {
            guard case .addon(let addonID, let sourceID) = lane.source,
                  let (_, source) = currentAddonSnapshot.source(addonID: addonID, sourceID: sourceID),
                  let tr = source.transport else { continue }
            let pos = shownTransportPosition(addonID: addonID, sourceID: sourceID, transport: tr)
            let timeText = tr.duration.map { "\(AddonFormatting.clock(pos.position)) / \(AddonFormatting.clock($0))" }
                ?? AddonFormatting.clock(pos.position)
            addonTransportPlayButtons[laneIndex]?.title = tr.isPlaying ? "Pause" : "Play"
            addonTransportTimeLabels[laneIndex]?.stringValue = timeText
            if let slider = addonTransportSliders[laneIndex], let dur = tr.duration, dur > 0 {
                slider.maxValue = dur
                if NSApp.currentEvent?.type != .leftMouseDragged {
                    slider.doubleValue = min(dur, max(0, pos.position))
                }
            }
            if let note = addonTransportSeekNotes[laneIndex] {
                note.stringValue = pos.seeking ? "Moving to \(AddonFormatting.clock(pos.position)). The sound returns in a moment." : ""
                note.isHidden = !pos.seeking
            }
        }
    }

    func compactTransportChip(for lane: Int) -> (playing: Bool, text: String, sourceName: String)? {
        guard captureLanes.indices.contains(lane),
              case .addon(let addonID, let sourceID) = captureLanes[lane].source,
              let (_, source) = currentAddonSnapshot.source(addonID: addonID, sourceID: sourceID),
              let tr = source.transport else { return nil }
        let pos = shownTransportPosition(addonID: addonID, sourceID: sourceID, transport: tr)
        let text = tr.duration.map { "\(AddonFormatting.clock(pos.position)) / \(AddonFormatting.clock($0))" }
            ?? AddonFormatting.clock(pos.position)
        return (tr.isPlaying, text, source.name)
    }

    func toggleAddonTransport(lane: Int) {
        guard captureLanes.indices.contains(lane),
              case .addon(let addonID, let sourceID) = captureLanes[lane].source,
              let (_, source) = currentAddonSnapshot.source(addonID: addonID, sourceID: sourceID),
              let tr = source.transport, let host = addonHost else { return }
        let cmd = tr.isPlaying ? "pause" : "play"
        addonSourceProblems.removeValue(forKey: addonID)
        host.transport(id: addonID, source: sourceID, command: cmd) { [weak self] result in
            if case .failure(let reason) = result {
                self?.addonSourceProblems[addonID] = reason.message
                self?.setNotice(reason.message)
            }
        }
    }

    @objc private func openAddonsFromSources() { openAddonsOptionsTab() }

    @objc private func addonFormatChanged(_ sender: NSPopUpButton) {
        let laneIndex = sender.tag
        guard captureLanes.indices.contains(laneIndex),
              case .addon(let addonID, let sourceID) = captureLanes[laneIndex].source,
              let fmtID = sender.selectedItem?.representedObject as? String,
              let host = addonHost else { return }
        let label = sender.titleOfSelectedItem ?? fmtID
        let slotKey = "\(addonID):\(sourceID)"
        addonSourceProblems.removeValue(forKey: addonID)
        addonPendingFormats[slotKey] = (fmtID, ProcessInfo.processInfo.systemUptime)
        host.selectFormat(id: addonID, source: sourceID, format: fmtID) { [weak self] result in
            guard let self else { return }
            if case .failure(let reason) = result {
                self.addonPendingFormats.removeValue(forKey: slotKey)
                self.addonSourceProblems[addonID] = "Could not choose \(label): \(reason)"
                self.replaceOptionsTab(with: self.makeSourceTab())
                self.refreshSourceOptions()
            }
        }
    }

    @objc private func addonOpenFileButton(_ sender: NSButton) {
        guard let raw = sender.identifier?.rawValue, raw.hasPrefix("addon-open:") else { return }
        let addonID = String(raw.dropFirst("addon-open:".count))
        guard let addon = currentAddonSnapshot.addon(addonID), let spec = addon.openFilesSpec else { return }
        let dialog = NSOpenPanel()
        dialog.title = "Open a file with \(addon.displayName)"
        dialog.canChooseFiles = true
        dialog.canChooseDirectories = false
        dialog.allowsMultipleSelection = false
        let types = spec.extensions.compactMap { UTType(filenameExtension: $0.trimmingCharacters(in: CharacterSet(charactersIn: "."))) }
        if !types.isEmpty {
            dialog.allowedContentTypes = types
        }
        guard dialog.runModal() == .OK, let url = dialog.url else { return }
        openFileWithAddon(id: addonID, url: url)
    }

    private func openFileWithAddon(id: String, url: URL) {
        guard let host = addonHost else { return }
        let addonName = currentAddonSnapshot.addonName(for: id)
        let fileName = url.lastPathComponent
        addonSourceProblems.removeValue(forKey: id)
        host.openFile(id: id, path: url.path) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success:
                var msg = "Sent \(fileName) to \(addonName)."
                let inUse = self.captureLanes.contains {
                    if case .addon(let a, _) = $0.source { return a == id }
                    return false
                }
                if !inUse, let firstSource = currentAddonSnapshot.addon(id)?.sources.first {
                    msg += " Choose \"\(firstSource.name)\" as an audio source in Options, Sources, to see it."
                }
                self.setNotice(msg)
            case .failure(let reason):
                let msg = "Could not open \(fileName): \(reason)"
                self.addonSourceProblems[id] = msg
                self.setNotice(msg)
            }
        }
    }

    @objc private func pickAddonDropOpener(_ sender: NSButton) {
        guard let choice = addonPendingDropChoice, choice.openers.indices.contains(sender.tag) else { return }
        let opener = choice.openers[sender.tag]
        let url = choice.url
        addonPendingDropChoice = nil
        replaceOptionsTab(with: makeSourceTab())
        refreshSourceOptions()
        openFileWithAddon(id: opener.id, url: url)
    }

    @objc private func cancelAddonDropOpener() {
        addonPendingDropChoice = nil
        replaceOptionsTab(with: makeSourceTab())
        refreshSourceOptions()
    }

    @objc private func addonTransportToggle(_ sender: NSButton) {
        toggleAddonTransport(lane: sender.tag)
    }

    @objc private func addonTransportStop(_ sender: NSButton) {
        let laneIndex = sender.tag
        guard captureLanes.indices.contains(laneIndex),
              case .addon(let addonID, let sourceID) = captureLanes[laneIndex].source,
              let host = addonHost else { return }
        addonSeekingTargets.removeValue(forKey: "\(addonID):\(sourceID)")
        host.transport(id: addonID, source: sourceID, command: "stop") { [weak self] result in
            if case .failure(let reason) = result {
                self?.addonSourceProblems[addonID] = reason.message
                self?.setNotice(reason.message)
            }
        }
    }

    @objc private func addonTransportSliderChanged(_ sender: NSSlider) {
        let laneIndex = sender.tag
        guard captureLanes.indices.contains(laneIndex),
              case .addon(let addonID, let sourceID) = captureLanes[laneIndex].source,
              let host = addonHost else { return }
        let target = sender.doubleValue
        addonSeekingTargets["\(addonID):\(sourceID)"] = (target, ProcessInfo.processInfo.systemUptime)
        refreshAddonTransportControls()
        host.transport(id: addonID, source: sourceID, command: "seek", position: target) { [weak self] result in
            if case .failure(let reason) = result {
                self?.addonSourceProblems[addonID] = reason.message
                self?.setNotice(reason.message)
            }
        }
    }

    // MARK: Pkg 17 — Control Command Executor

    private func controlBusyReason() -> String? {
        if editing || waveform?.isPointerGestureCurrentlyActive == true { return "a gesture" }
        if activeSnapshotSaves > 0 || waveform?.hasPendingDragWrite == true { return "saving audio" }
        return nil
    }

    private func parseControlLane(_ letter: String) -> Int? {
        let upper = letter.trimmingCharacters(in: .whitespaces).uppercased()
        for i in captureLanes.indices where captureLaneLetter(i) == upper {
            return i
        }
        return nil
    }

    private func buildControlState() -> AddonControlState {
        let rate = waveform?.currentSampleRate ?? UInt32(SystemAudioTap.timelineRate)
        let rateD = Double(max(1, rate))
        let historyRange = waveform?.currentRetainedFrames ?? (0..<0)
        let viewRange = waveform?.currentVisibleFrames ?? historyRange
        let historySpan = AddonFrameSpan(
            start_frame: historyRange.lowerBound,
            end_frame: historyRange.upperBound,
            seconds: Double(historyRange.upperBound - historyRange.lowerBound) / rateD
        )
        let viewSpan = AddonFrameSpan(
            start_frame: viewRange.lowerBound,
            end_frame: viewRange.upperBound,
            seconds: Double(viewRange.upperBound - viewRange.lowerBound) / rateD
        )
        let selectionState: AddonSelectionState? = waveform?.selectionForSaving().map { sel in
            AddonSelectionState(
                start_frame: sel.lowerBound,
                end_frame: sel.upperBound,
                seconds: Double(sel.upperBound - sel.lowerBound) / rateD,
                from_seconds_ago: Double(historyRange.upperBound >= sel.lowerBound ? (historyRange.upperBound - sel.lowerBound) : 0) / rateD,
                to_seconds_ago: Double(historyRange.upperBound >= sel.upperBound ? (historyRange.upperBound - sel.upperBound) : 0) / rateD
            )
        }
        let laneStates: [AddonLaneState] = captureLanes.enumerated().map { idx, lane in
            let srcName = defaultFileName(for: lane.source)
            let info = AddonSourceInfo(id: lane.source.key, kind: lane.source.controlKind, name: srcName)
            let effectiveRoute = tap?.captureRoutes.indices.contains(idx) == true ? tap!.captureRoutes[idx] : lane.route
            return AddonLaneState(
                lane: captureLaneLetter(idx),
                source: info,
                channels: tap?.channelCount(for: idx),
                route: effectiveRoute.controlName,
                problem: tap?.sourceProblem(for: idx)
            )
        }
        let chosenRows = waveform?.chosenRowsForControlState() ?? []
        return AddonControlState(
            capturing: !paused,
            recording: !paused && tap?.cachedState == 1,
            problem: captureError,
            sample_rate: rate,
            lanes: laneStates,
            history: historySpan,
            selection: selectionState,
            view: viewSpan,
            separate_rows: separateLanes,
            chosen_rows: chosenRows,
            preview_playing: waveform?.isPreviewPlaying == true,
            save_folder: directory(for: .saved),
            drag_folder: directory(for: .output),
            busy: controlBusyReason()
        )
    }

    /// Every file a save wrote, with the letter of the lane it came from (Windows `saved_files`).
    static func savedFiles(urls: [URL], lanes: [Int?]) -> [AddonSavedFile] {
        urls.enumerated().map { index, url in
            let lane = lanes.indices.contains(index) ? lanes[index].map(captureLaneLetter) : nil
            if url.pathExtension.lowercased() == "json" {
                return AddonSavedFile(path: url.path, kind: "positions", lane: lane, channels: nil, sample_rate: nil, frames: nil)
            }
            if let data = try? Data(contentsOf: url), let hdr = WavHeaderInfo.parse(data) {
                return AddonSavedFile(
                    path: url.path, kind: "wav", lane: lane,
                    channels: UInt32(hdr.channels), sample_rate: hdr.sampleRate, frames: hdr.frames
                )
            }
            return AddonSavedFile(path: url.path, kind: "wav", lane: lane, channels: nil, sample_rate: nil, frames: nil)
        }
    }

    private func replyControlSave(call: AddonControlCall, folder: String, urls: [URL], lanes: [Int?]) {
        let savedFiles = Self.savedFiles(urls: urls, lanes: lanes)
        let res = AddonControlResult(
            state: buildControlState(),
            sources: nil,
            folder: folder,
            files: savedFiles,
            history_restarted: false
        )
        addonHost?.controlReply(call: call, result: res)
    }

    private func executeControlCalls(_ calls: [AddonControlCall]) {
        guard let host = addonHost else { return }
        // I6 step 5: re-check current snapshot's control_allowed immediately before executing each request.
        AddonControlGate.run(calls, currentGrant: { call in
            guard let snapshot = host.confirmedSnapshot() else { return nil }
            currentAddonSnapshot = snapshot
            return snapshot.addon(call.addon_id)?.control_allowed ?? false
        }, refuse: { call, error in
            host.controlReply(call: call, error: error)
        }) { call in
            if !call.command.isQuiet, let busy = controlBusyReason() {
                host.controlReply(
                    call: call,
                    error: AddonControlError(code: "busy", message: "Backtracking is busy with \(busy); try again shortly")
                )
                return
            }
            executeSingleControlCall(host: host, call: call)
        }
        // The grant checks may have read a newer snapshot than the window shows.
        syncAddonSnapshotUI()
    }

    /// Answers each `select` whose freeze has finished or run out of time.
    private func answerSelectFreezeWaits() {
        guard !selectFreezeWaits.isEmpty, let host = addonHost else { return }
        let now = ProcessInfo.processInfo.systemUptime
        let preparing = waveform?.isPreparingMaterial == true
        var still: [(AddonControlCall, TimeInterval)] = []
        for (call, until) in selectFreezeWaits {
            if preparing && now < until {
                still.append((call, until))
            } else if !preparing, waveform?.hasEditingMaterial == true, waveform?.hasSelection == true {
                host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))
            } else {
                let message = preparing ? "the selection's audio was not ready in time"
                    : (notice?.isEmpty == false ? notice! : "no audio is available for this selection")
                host.controlReply(call: call, error: AddonControlError(code: "no_audio", message: message))
            }
        }
        selectFreezeWaits = still
    }

    private func executeSingleControlCall(host: AddonHostClient, call: AddonControlCall) {
        let cmd = call.command
        let by = call.addon_name
        switch cmd.command {
        case "get_state":
            host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))

        case "list_sources":
            let choices = availableCaptureSources(excluding: tap?.privatePlaybackDevices ?? [])
            let sources = choices.map { AddonSourceInfo(id: $0.source.key, kind: $0.source.controlKind, name: $0.title) }
            host.controlReply(call: call, result: AddonControlResult(state: buildControlState(), sources: sources))

        case "start_capture":
            switch runWindowAction(.startCapture) {
            case .success:
                setNotice("Capture started · by \(by)")
                host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))
            case .failure(let ref):
                host.controlReply(call: call, error: AddonControlError(code: ref.code, message: ref.message))
            }

        case "pause_capture":
            switch runWindowAction(.pauseCapture) {
            case .success:
                setNotice("by \(by)")
                host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))
            case .failure(let ref):
                host.controlReply(call: call, error: AddonControlError(code: ref.code, message: ref.message))
            }

        case "clear_history":
            switch runWindowAction(.clearHistory) {
            case .success:
                setNotice("Cleared · by \(by)")
                host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))
            case .failure(let ref):
                host.controlReply(call: call, error: AddonControlError(code: ref.code, message: ref.message))
            }

        case "select":
            let action: TestWindowAction
            if let from = cmd.from_seconds_ago {
                action = .selectBack(from: from, to: cmd.to_seconds_ago ?? 0.0)
            } else if let s = cmd.start_frame, let e = cmd.end_frame {
                action = .selectFrames(s..<e)
            } else {
                host.controlReply(call: call, error: AddonControlError(code: "invalid_request", message: "select needs from_seconds_ago or start_frame/end_frame"))
                return
            }
            switch runWindowAction(action) {
            case .success:
                selectionByAddon = by
                updateStatus()
                // The reply waits until the selection's audio is frozen, so a play_preview that
                // follows plays it (Windows `Wait::Freeze`, at most 8 s).
                selectFreezeWaits.append((call, ProcessInfo.processInfo.systemUptime + 8))
                answerSelectFreezeWaits()
            case .failure(let ref):
                host.controlReply(call: call, error: AddonControlError(code: ref.code, message: ref.message))
            }

        case "clear_selection":
            selectionByAddon = nil
            _ = runWindowAction(.clearSelection)
            host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))

        case "choose_rows":
            let result: Result<String?, TestActionRefusal>
            if let laneStr = cmd.lane {
                guard let idx = parseControlLane(laneStr) else {
                    host.controlReply(call: call, error: AddonControlError(code: "not_found", message: "the window does not show lane \(laneStr)"))
                    return
                }
                if !separateLanes, case .failure(let r) = runWindowAction(.separate(true)) {
                    host.controlReply(call: call, error: AddonControlError(code: r.code, message: r.message))
                    return
                }
                result = runWindowAction(.chooseLane(idx))
            } else if let first = cmd.first, let last = cmd.last {
                guard let fl = parseControlLane(first.lane), let ll = parseControlLane(last.lane) else {
                    host.controlReply(call: call, error: AddonControlError(code: "not_found", message: "unknown lane"))
                    return
                }
                if !separateLanes, case .failure(let r) = runWindowAction(.separate(true)) {
                    host.controlReply(call: call, error: AddonControlError(code: r.code, message: r.message))
                    return
                }
                result = runWindowAction(.chooseRows(
                    firstLane: fl, firstRow: Int(max(1, first.row)) - 1,
                    lastLane: ll, lastRow: Int(max(1, last.row)) - 1
                ))
            } else {
                result = separateLanes ? runWindowAction(.separate(false)) : .success(nil)
            }
            switch result {
            case .success:
                host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))
            case .failure(let ref):
                host.controlReply(call: call, error: AddonControlError(code: ref.code, message: ref.message))
            }

        case "set_view":
            let action: TestWindowAction
            if let secs = cmd.seconds {
                action = .zoomNewest(secs)
            } else if let s = cmd.start_frame, let e = cmd.end_frame {
                action = .view(s..<e)
            } else {
                action = .viewAll
            }
            switch runWindowAction(action) {
            case .success:
                host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))
            case .failure(let ref):
                host.controlReply(call: call, error: AddonControlError(code: ref.code, message: ref.message))
            }

        case "save":
            let targetFolder = cmd.folder ?? directory(for: .saved)
            do {
                try FileManager.default.createDirectory(atPath: targetFolder, withIntermediateDirectories: true)
            } catch {
                host.controlReply(call: call, error: AddonControlError(code: "failed", message: "\(targetFolder): \(error.localizedDescription)"))
                return
            }
            saveSnapshot(into: targetFolder, controlCall: call)

        case "play_preview":
            if waveform?.hasSelection != true {
                host.controlReply(call: call, error: AddonControlError(code: "no_selection", message: "there is no selection to play"))
                return
            }
            switch runWindowAction(.playPreview) {
            case .success:
                host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))
            case .failure(let ref):
                host.controlReply(call: call, error: AddonControlError(code: ref.code, message: ref.message))
            }

        case "stop_preview":
            _ = runWindowAction(.stopPreview)
            host.controlReply(call: call, result: AddonControlResult(state: buildControlState()))

        case "add_source":
            guard let sourceID = cmd.source else {
                host.controlReply(call: call, error: AddonControlError(code: "invalid_request", message: "missing source id"))
                return
            }
            let choices = availableCaptureSources(excluding: tap?.privatePlaybackDevices ?? [])
            guard let found = choices.first(where: { $0.source.key == sourceID }) else {
                host.controlReply(call: call, error: AddonControlError(
                    code: "not_found",
                    message: "no source with this id can be added now; list_sources gives the ids"
                ))
                return
            }
            switch runWindowAction(.addSource(found.source)) {
            case .success:
                setNotice("Sources changed, recording started over · by \(by)")
                host.controlReply(call: call, result: AddonControlResult(state: buildControlState(), history_restarted: true))
            case .failure(let ref):
                host.controlReply(call: call, error: AddonControlError(code: ref.code, message: ref.message))
            }

        case "remove_source":
            guard let laneStr = cmd.lane, let idx = parseControlLane(laneStr) else {
                host.controlReply(call: call, error: AddonControlError(code: "not_found", message: "unknown lane"))
                return
            }
            if captureLanes.count < 2 {
                host.controlReply(call: call, error: AddonControlError(code: "failed", message: "the only lane cannot be removed; add another first"))
                return
            }
            switch runWindowAction(.removeSource(idx)) {
            case .success:
                setNotice("Sources changed, recording started over · by \(by)")
                host.controlReply(call: call, result: AddonControlResult(state: buildControlState(), history_restarted: true))
            case .failure(let ref):
                host.controlReply(call: call, error: AddonControlError(code: ref.code, message: ref.message))
            }

        default:
            host.controlReply(call: call, error: AddonControlError(code: "unknown_command", message: "Unknown command \(cmd.command)"))
        }
    }

}

#if BACKTRACKING_CAPTURE_VERIFY
extension WaveformView {
    /// Shows `rows` over `frames` as if a read had just arrived.
    func verifyShow(rows: [WaveformRow], laneWidths: [UInt32], frames: Range<UInt64>) {
        requestedRows = rows
        displayedRows = rows
        displayedLaneCount = laneWidths.count
        displayedLaneWidths = laneWidths
        retainedFrames = frames
        visibleFrames = frames
        sampleRate = UInt32(SystemAudioTap.timelineRate)
        recomputeRowLayout()
    }

    var verifyFlagPoint: NSPoint? {
        guard let selection, let top = flagTopY() else { return nil }
        return NSPoint(x: flagPosition(for: selection.lowerBound), y: top - 5)
    }

    var verifyRememberedRows: [RowIdentity] { rememberedRows }
    var verifyChipToolTipTags: [NSView.ToolTipTag] { chipToolTipTags }
    var verifyLaneFirstRows: [Int] { laneFirstRows }
    func verifyShowRows(_ rows: [WaveformRow], groups: [String?]) { showRows(rows, groups: groups) }
    var verifyWholeLanes: Set<Int> { wholeLanes }
    var verifyLaneBoxes: [LaneLayoutBox] { laneBoxes }
    func verifyStopEdgeScroll() { stopEdgeAutoScroll() }
    /// Puts one group chip and one transport chip on the plot and lets the view register them.
    func verifyShowChips() -> [(NSPoint, String)] {
        let group = OverlayChip(rect: NSRect(x: 40, y: 3, width: 50, height: 14), text: "Objects", current: false,
                                toolTip: "Show Bed", action: .cycleGroup(lane: 0))
        let transport = OverlayChip(rect: NSRect(x: 94, y: 3, width: 80, height: 14), text: "▶ 0:01", current: false,
                                    toolTip: "Play Tone", action: .toggleTransport(lane: 0))
        verifyChips = [group, transport]
        syncChipsForDisplay()
        return [(NSPoint(x: 60, y: 10), "Show Bed"), (NSPoint(x: 120, y: 10), "Play Tone")]
    }
    func verifyChipToolTip(at point: NSPoint) -> String? {
        guard let tag = chipToolTipTags.first else { return nil }
        return view(self, stringForToolTip: tag, point: point, userData: Self.chipToolTipData)
    }
}
#endif

private final class WaveformView: NSView, NSDraggingSource, NSViewToolTipOwner {
    weak var controller: BacktrackingController?
    var showsOverlay = true { didSet { needsDisplay = true } }
    /// The capture state to mark in the corner, in waveform-only mode.
    var compactStatus: CompactStatus? {
        didSet {
            guard compactStatus != oldValue else { return }
            needsDisplay = true
            if compactStatus?.problem != oldValue?.problem || (compactStatus == nil) != (oldValue == nil) {
                updateStatusToolTip()
            }
        }
    }
    private var statusToolTip: NSView.ToolTipTag?
    var gridOpacity: CGFloat = 1 { didSet { needsDisplay = true } }
    var showsOutline = false { didSet { needsDisplay = true } }
    var outlineWidth: CGFloat = 1 { didSet { needsDisplay = true } }
    var outlineColor = NSColor.white { didSet { needsDisplay = true } }
    var showsBars = false {
        didSet {
            if showsBars != oldValue {
                needsDisplay = true
            }
        }
    }
    var secondsInterval: UInt32? { didSet { needsDisplay = true } }
    var accentRGB = WaveformTheme.defaultAccentRGB { didSet { needsDisplay = true } }
    var look: ResolvedLook = .default { didSet { if look != oldValue { needsDisplay = true } } }
    var stereo = false {
        didSet { if stereo != oldValue { revision = nil } }
    }
    var hideSilentChannels = true {
        didSet { if hideSilentChannels != oldValue { revision = nil; needsDisplay = true } }
    }
    var laneProblems: [Int: String] = [:] {
        didSet { if laneProblems != oldValue { needsDisplay = true } }
    }
    var laneSourceLabels: [String] = []

    var frequencyGradient: FrequencyGradient? {
        didSet {
            if frequencyGradient == nil {
                frequencyWaveform = nil
                lastFrequencyRequest = nil
            }
            needsDisplay = true
        }
    }

    private let frequencyQueue = DispatchQueue(label: "Backtracking.waveform-frequency", qos: .utility)
    private let wideWaveformQueue = DispatchQueue(label: "Backtracking.multichannel-waveform", qos: .userInitiated)
    private var frequencyWaveform: FrequencyWaveform?
    /// The rows `frequencyWaveform` was read for, in its order.
    private var frequencyRows: [WaveformRow] = []
    private var frequencyReading = false
    private var nextFrequencyRead = 0.0
    private struct FrequencyRequest: Equatable {
        let revision: UInt64
        let materialVersion: UInt64
        let range: Range<UInt64>
        let bins: Int
        let rows: [WaveformRow]
    }
    private var lastFrequencyRequest: FrequencyRequest?

    private var traces: [WaveformTrace] = []
    private var requestedRows: [WaveformRow] = []
    private var displayedRows: [WaveformRow] = []
    private var displayedLaneCount: Int = 1
    private var displayedLaneWidths: [UInt32] = []
    private var laneFirstRows: [Int] = []
    private var columnFirstRow: Int = 0
    private var laneBoxes: [LaneLayoutBox] = []
    private var columnBox: LaneLayoutBox?
    private var gutterToolTip: NSView.ToolTipTag?
    struct ChipToolTip: Equatable {
        let rect: NSRect
        let text: String
    }
    private static let chipToolTipData = UnsafeMutableRawPointer(bitPattern: 2)
    private var chipToolTips: [ChipToolTip] = []
    private var chipToolTipTags: [NSView.ToolTipTag] = []
    #if BACKTRACKING_CAPTURE_VERIFY
    private var verifyChips: [OverlayChip]?
    #endif
    private struct ScrollDrag {
        let laneIndex: Int? // nil = column scroll (>2 lanes)
        let grabOffset: CGFloat
    }
    private var scrollDrag: ScrollDrag?
    private var edgeScrollTimer: Timer?
    private var edgeScrollPoint: NSPoint?
    private var retainedFrames: Range<UInt64> = 0..<0
    private var visibleFrames: Range<UInt64> = 0..<0
    private var renderedFrames: Range<UInt64> = 0..<0
    private var requestedView: Range<UInt64>?
    private var zoomed = false
    private var panStart: (x: CGFloat, range: Range<UInt64>)?
    private var revision: UInt64?
    private var sampleRate: UInt32 = 0
    private var bars: [UInt64] = []
    private var selection: Range<UInt64>?
    private var marker: Range<UInt64>?
    private var markerRows: [RowIdentity] = []
    private var markerRevision: UInt64 = 0
    private var committedMarkerRevision: UInt64 = 0
    private var pendingDragMarker: (range: Range<UInt64>, rows: [RowIdentity]?, ticket: UInt64)?
    private var anchor: UInt64?
    private var selectionPressX: CGFloat?
    private var selectionMoved = false
    private var pointer: NSPoint?
    private var pointerModifiers: NSEvent.ModifierFlags = []
    private var selectionModifierDown: Bool { selectsWaveform(pointerModifiers) }
    private var pointerArea: NSTrackingArea?
    private var mouseDownEvent: NSEvent?
    private var dragRequested = false
    private var pendingDragWrite: DispatchWorkItem?
    private var dragging = false
    private var readyFiles: [URL] = []
    private var exportVersion: UInt64 = 0
    private var editMaterial: WAVExport?
    private var materialPending = false
    private var materialVersion: UInt64 = 0
    private var renderGate = WaveformRenderGate()
    private var lastRenderedRequest: WaveformReadRequest?
    private var displayPolicy = bt_waveform_display_policy(0, 0, 0, 0, 0)
    private var guideBars: [UInt64] = []
    private var guideSeconds: [UInt64] = []
    /// The selection's fades, in timeline frames. A new selection starts without any.
    private var fades = noFades
    private struct FadeGesture {
        /// 0 drags the fade in from the start flag, 1 the fade out from the end flag.
        let end: UInt32
        let press: UInt64
        let originX: CGFloat
        let initial: BtFades
        var moved = false
    }
    private var fadeGesture: FadeGesture?
    private var player: PreviewPlayer?
    /// The controller enables this when multiple waveform rows are available.
    var separateLanes = false {
        didSet {
            guard separateLanes != oldValue else { return }
            player?.stop()
            if !dragging { discardPreparedFile() }
            needsDisplay = true
        }
    }
    private var selectionAnchorRow: RowIdentity?
    private var selectionCursorRow: RowIdentity?
    private var rememberedRows: [RowIdentity] = []
    /// Lanes the chosen rows held whole (Windows `selection_whole`): such a lane stays whole as
    /// its rows come and go, until it shows another group.
    private var wholeLanes: Set<Int> = []
    /// The group each lane drew its rows with, for `displayedRows`, and for `requestedRows`.
    private var displayedGroupIDs: [String?] = []
    private var requestedGroupIDs: [String?] = []
    /// Only a new selection follows the pointer across rows; moving a flag keeps the rows it has
    /// (Windows `Gesture::Select.rows` is `Some` only for a new selection).
    private var selectionUpdatesRows = false
    private var dragRow: RowIdentity?
    private var waveformCursorActive = false

    private var hasStackedLane: Bool {
        displayedLaneWidths.contains { $0 > 2 }
    }

    private var gutterWidth: CGFloat {
        guard showsOverlay else { return 0 }
        return RowStackLayout.labelGutterWidth(laneCount: displayedLaneCount, hasStacked: hasStackedLane)
    }

    private var audioRect: NSRect {
        let left = bounds.minX + gutterWidth
        return NSRect(x: left, y: bounds.minY, width: max(1, bounds.maxX - left), height: max(1, bounds.height))
    }

    private var isPointerGestureActive: Bool {
        anchor != nil || mouseDownEvent != nil || dragging || fadeGesture != nil || scrollDrag != nil
    }

    private func laneInputsForDisplay() -> [LaneLayoutInput] {
        (0..<max(1, displayedLaneCount)).map { lane in
            let count = displayedRows.filter { $0.lane == lane }.count
            let stacked = displayedLaneWidths.indices.contains(lane) && displayedLaneWidths[lane] > 2
            return LaneLayoutInput(rows: count, stacked: stacked)
        }
    }

    var desiredComfortableAudioHeight: CGFloat? {
        RowStackLayout.comfortableAudioHeight(lanes: laneInputsForDisplay())
    }

    private func recomputeRowLayout() {
        let inputs = laneInputsForDisplay()
        if laneFirstRows.count != inputs.count {
            laneFirstRows = Array(repeating: 0, count: inputs.count)
        }
        if inputs.count > 2 {
            let (boxes, col) = RowStackLayout.layOutColumn(height: audioRect.height, lanes: inputs, first: columnFirstRow)
            laneBoxes = boxes
            columnBox = col
            columnFirstRow = col.first
        } else {
            let boxes = RowStackLayout.layOut(height: audioRect.height, lanes: inputs, first: laneFirstRows)
            laneBoxes = boxes
            columnBox = nil
            for i in boxes.indices { laneFirstRows[i] = boxes[i].first }
        }
        updateGutterToolTip()
    }

    private func rowRect(for index: Int) -> NSRect? {
        guard displayedRows.indices.contains(index) else { return nil }
        let row = displayedRows[index]
        guard laneBoxes.indices.contains(row.lane) else { return nil }
        let box = laneBoxes[row.lane]
        let laneStart = displayedRows.firstIndex { $0.lane == row.lane } ?? index
        let rel = index - laneStart
        guard let topFromPlotTop = box.rowTop(rel) else { return nil }
        let y = audioRect.maxY - topFromPlotTop - box.rowHeight
        return NSRect(x: audioRect.minX, y: y, width: audioRect.width, height: box.rowHeight)
    }

    private func rowIdentity(at point: NSPoint, clampedToVisible: Bool = true) -> RowIdentity? {
        guard !displayedRows.isEmpty else { return nil }
        let yFromTop = audioRect.maxY - point.y
        let hit = clampedToVisible
            ? RowStackLayout.nearestVisibleRow(boxes: laneBoxes, yFromTop: yFromTop)
            : RowStackLayout.rowAt(boxes: laneBoxes, yFromTop: yFromTop)
        guard let (lane, relRow) = hit else { return nil }
        let laneRows = displayedRows.filter { $0.lane == lane }
        guard laneRows.indices.contains(relRow) else { return nil }
        return laneRows[relRow].identity
    }

    private func currentSpecs() -> [WaveformRowSpec] { displayedRows.map(\.spec) }

    /// Rows currently selected when Edit separately is on. Empty means all rows. Rows keep their
    /// identity while scrolled away or while another group is on show (Windows `selected_rows`).
    private var activeSelectedRows: [RowIdentity] {
        guard separateLanes, hasSelection, !rememberedRows.isEmpty else { return [] }
        let chosen = Set(rememberedRows)
        let shown = Set(displayedRows.map(\.identity))
        // Every row drawn, and no other, is the whole capture.
        if chosen == shown { return [] }
        return rememberedRows
    }

    var selectedRowsForExport: [RowIdentity]? {
        let rows = activeSelectedRows
        guard !rows.isEmpty else { return nil }
        return Self.exportRows(rows, wholeLanes: wholeLanes)
    }

    /// The rows an export or preview takes: a lane held whole is named by `.whole`, so it is
    /// written as it is shown then, with any row that sounded since.
    static func exportRows(_ rows: [RowIdentity], wholeLanes: Set<Int>) -> [RowIdentity] {
        var out: [RowIdentity] = []
        for row in rows {
            if wholeLanes.contains(row.lane) {
                if !out.contains(.whole(lane: row.lane)) { out.append(.whole(lane: row.lane)) }
            } else {
                out.append(row)
            }
        }
        return out
    }

    /// By lane: whether `rows` hold every row `displayed` draws of it (Windows `lanes_chosen_whole`).
    static func lanesChosenWhole(displayed: [WaveformRow], rows: [RowIdentity]) -> Set<Int> {
        let chosen = Set(rows)
        let lanes = Set(displayed.map(\.lane))
        return lanes.filter { lane in
            let drawn = displayed.filter { $0.lane == lane }
            return !drawn.isEmpty && drawn.allSatisfy { chosen.contains($0.identity) }
        }
    }

    /// Carries the chosen rows from rows drawn as `old` to rows drawn as `new` (Windows
    /// `carry_rows`). A lane held whole covers every row it draws now while it shows the same
    /// group; once it shows another group it keeps only the rows it held. Other rows stay.
    static func carryRows(
        _ rows: [RowIdentity], whole: Set<Int>, old: [WaveformRow], new: [WaveformRow],
        oldGroups: [String?], newGroups: [String?]
    ) -> (rows: [RowIdentity], whole: Set<Int>) {
        let group = { (groups: [String?], lane: Int) -> String?? in groups.indices.contains(lane) ? groups[lane] : nil }
        let stillWhole = whole.filter { lane in
            old.contains(where: { $0.lane == lane }) && new.contains(where: { $0.lane == lane })
                && group(oldGroups, lane) == group(newGroups, lane)
        }
        var carried: [RowIdentity] = []
        var lanesDrawn: [Int] = []
        for row in new where !lanesDrawn.contains(row.lane) { lanesDrawn.append(row.lane) }
        for lane in lanesDrawn {
            if stillWhole.contains(lane) {
                carried += new.filter { $0.lane == lane }.map(\.identity)
            } else {
                carried += rows.filter { $0.lane == lane }
            }
        }
        carried += rows.filter { !lanesDrawn.contains($0.lane) }
        return (carried, stillWhole)
    }

    /// Draws `rows` from now on, carrying the chosen rows over to them.
    private func showRows(_ rows: [WaveformRow], groups: [String?]) {
        if rows != displayedRows || groups != displayedGroupIDs, !rememberedRows.isEmpty {
            (rememberedRows, wholeLanes) = Self.carryRows(rememberedRows, whole: wholeLanes, old: displayedRows,
                                                          new: rows, oldGroups: displayedGroupIDs, newGroups: groups)
        }
        displayedRows = rows
        displayedGroupIDs = groups
    }

    /// The rows a selection now covers.
    private func chooseRows(_ rows: [RowIdentity]) {
        // Flags left by a save name a wide lane held whole by `.whole`; it covers the lane's rows.
        let rows = rows.flatMap { row -> [RowIdentity] in
            guard case .whole(let lane) = row else { return [row] }
            let drawn = displayedRows.filter { $0.lane == lane }.map(\.identity)
            return drawn.isEmpty || drawn == [row] ? [row] : drawn
        }
        rememberedRows = rows
        wholeLanes = Self.lanesChosenWhole(displayed: displayedRows, rows: rows)
    }

    var selectedRowsLabel: String {
        let rows = activeSelectedRows
        guard !rows.isEmpty else { return "" }
        return RowStackLayout.rowsLabel(rows, dual: displayedLaneCount > 1,
                                        channelLabels: controller?.currentChannelLabels ?? [])
    }

    private func isRowSelected(_ id: RowIdentity) -> Bool {
        let rows = activeSelectedRows
        if rows.isEmpty { return true }
        return rows.contains(id)
    }

    private func rectForRows(_ ids: [RowIdentity]) -> NSRect? {
        var combined: NSRect?
        for (index, row) in displayedRows.enumerated() where ids.contains(row.identity) {
            guard let rect = rowRect(for: index) else { continue }
            combined = combined.map { $0.union(rect) } ?? rect
        }
        return combined
    }

    private func selectedRowsRect() -> NSRect? {
        let rows = activeSelectedRows
        if rows.isEmpty { return audioRect }
        return rectForRows(rows)
    }

    private func flagTopY() -> CGFloat? {
        if selection != nil {
            return selectedRowsRect()?.maxY
        }
        if marker != nil {
            if !markerRows.isEmpty {
                return rectForRows(markerRows)?.maxY
            }
            return audioRect.maxY
        }
        return nil
    }

    var hasEditingMaterial: Bool { editMaterial != nil || materialPending }
    var isPreparingMaterial: Bool { materialPending }
    var isFrequencyBusy: Bool { frequencyReading }
    var hasPendingDragWrite: Bool { pendingDragWrite != nil }
    var currentSampleRate: UInt32 { sampleRate }
    var currentRetainedFrames: Range<UInt64> { retainedFrames }
    var currentVisibleFrames: Range<UInt64> { visibleFrames }
    var isPointerGestureCurrentlyActive: Bool { isPointerGestureActive }
    var isPreviewPlaying: Bool {
        #if BACKTRACKING_CAPTURE_VERIFY
        if verifySilentPreview { return verifyPreviewRange != nil }
        #endif
        return player?.range != nil
    }
    #if BACKTRACKING_CAPTURE_VERIFY
    /// A verification starts no audio: a preview that would play is only noted.
    var verifySilentPreview = false
    var verifyPreviewRange: Range<UInt64>?
    #endif

    func chosenRowsForControlState() -> [AddonRowState] {
        let rows = activeSelectedRows
        guard !rows.isEmpty else { return [] }
        var out: [AddonRowState] = []
        var perLaneCount: [Int: UInt32] = [:]
        for row in displayedRows {
            let num = (perLaneCount[row.lane] ?? 0) + 1
            perLaneCount[row.lane] = num
            if rows.contains(row.identity) {
                out.append(AddonRowState(lane: captureLaneLetter(row.lane), row: num, label: row.fullLabel))
            }
        }
        return out
    }

    func onChannelGroupSwitched(lane: Int) {
        if laneFirstRows.indices.contains(lane) {
            laneFirstRows[lane] = 0
        }
        columnFirstRow = 0
        player?.stop()
        if !dragging { discardPreparedFile() }
        // The chosen rows keep their identity across the switch (Windows `carry_rows`); they are
        // carried when the new group's rows arrive.
        revision = nil
        needsDisplay = true
    }

    func applyProgrammaticView(_ range: Range<UInt64>) {
        changeView(to: range)
    }

    func applyProgrammaticSelection(_ range: Range<UInt64>) {
        selection = range
        fades = noFades
        needsDisplay = true
    }

    func scrollToRow(lane: Int, row: Int) {
        if displayedLaneCount > 2 {
            let laneStart = displayedRows.firstIndex { $0.lane == lane } ?? 0
            columnFirstRow = max(0, laneStart + row)
        } else if laneFirstRows.indices.contains(lane) {
            laneFirstRows[lane] = max(0, row)
        }
        recomputeRowLayout()
        needsDisplay = true
    }

    func selectProgrammaticLane(_ lane: Int) -> Bool {
        let laneRows = displayedRows.filter { $0.lane == lane }
        guard let first = laneRows.first?.identity, let last = laneRows.last?.identity else { return false }
        selectionAnchorRow = first
        selectionCursorRow = last
        chooseRows(RowStackLayout.rowsBetween(allRows: currentSpecs(), anchor: first, cursor: last))
        needsDisplay = true
        return true
    }

    func selectProgrammaticRows(firstLane: Int, firstRow: Int, lastLane: Int, lastRow: Int) -> Bool {
        let aRows = displayedRows.filter { $0.lane == firstLane }
        let bRows = displayedRows.filter { $0.lane == lastLane }
        guard aRows.indices.contains(firstRow), bRows.indices.contains(lastRow) else { return false }
        let a = aRows[firstRow].identity
        let b = bRows[lastRow].identity
        selectionAnchorRow = a
        selectionCursorRow = b
        chooseRows(RowStackLayout.rowsBetween(allRows: currentSpecs(), anchor: a, cursor: b))
        needsDisplay = true
        return true
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var needsPanelToBecomeKey: Bool { true }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        recomputeRowLayout()
        needsDisplay = true
        updateStatusToolTip()
        controller?.refreshWaveform()
    }

    /// Files prepared for a drag that is still under way.
    var offeredFiles: [URL] { readyFiles }

    // Centre of the waveform-only status mark, measured from the plot's top-left corner.
    private var statusMarkCenter: NSPoint { NSPoint(x: bounds.minX + 12, y: bounds.maxY - 11) }

    /// The problem text beside the mark and where it goes, when there is room for any.
    private func statusProblemText(_ problem: String) -> (text: NSAttributedString, rect: NSRect)? {
        let left = statusMarkCenter.x + 11
        let room = bounds.maxX - left - 8
        guard room > 0 else { return nil }
        let style = NSMutableParagraphStyle()
        style.lineBreakMode = .byTruncatingTail
        let text = NSAttributedString(string: problem, attributes: [
            .font: NSFont.systemFont(ofSize: 12), .foregroundColor: problemColor, .paragraphStyle: style
        ])
        let size = text.size()
        return (text, NSRect(
            x: left, y: statusMarkCenter.y - size.height / 2, width: min(ceil(size.width), room), height: size.height
        ))
    }

    private func updateStatusToolTip() {
        if let statusToolTip { removeToolTip(statusToolTip) }
        statusToolTip = nil
        guard let compactStatus else { return }
        let center = statusMarkCenter
        var hover = NSRect(x: center.x - 7.5, y: center.y - 7.5, width: 15, height: 15)
        if let problem = compactStatus.problem, let text = statusProblemText(problem) {
            hover = hover.union(text.rect)
        }
        statusToolTip = addToolTip(hover.intersection(bounds), owner: self, userData: nil)
    }

    private func updateGutterToolTip() {
        if let gutterToolTip { removeToolTip(gutterToolTip) }
        gutterToolTip = nil
        guard showsOverlay, gutterWidth > 0 else { return }
        let rect = NSRect(x: bounds.minX, y: bounds.minY, width: gutterWidth, height: bounds.height)
        gutterToolTip = addToolTip(rect, owner: self, userData: UnsafeMutableRawPointer(bitPattern: 1))
    }

    func view(
        _ view: NSView, stringForToolTip tag: NSView.ToolTipTag, point: NSPoint, userData data: UnsafeMutableRawPointer?
    ) -> String {
        if data == UnsafeMutableRawPointer(bitPattern: 1) {
            return gutterToolTipText(at: point)
        }
        if data == Self.chipToolTipData {
            return chipToolTips.first { $0.rect.contains(point) }?.text ?? ""
        }
        return compactStatus?.detail ?? ""
    }

    private func gutterToolTipText(at point: NSPoint) -> String {
        guard let id = rowIdentity(at: point, clampedToVisible: false),
              let row = displayedRows.first(where: { $0.identity == id }) else { return "" }
        var lines: [String] = []
        if displayedLaneCount > 2 {
            let letter = captureLaneLetter(row.lane)
            let src = laneSourceLabels.indices.contains(row.lane) ? laneSourceLabels[row.lane] : "Source \(letter)"
            lines.append("Source \(letter): \(src)")
            if let problem = laneProblems[row.lane] { lines.append(problem) }
        }
        switch id {
        case .pair(_, let side):
            lines.append(side == 0
                ? "L: the left side of all channels mixed to stereo"
                : "R: the right side of all channels mixed to stereo")
        case .channel(_, let ch):
            lines.append("Channel \(ch + 1) of \(row.laneChannelCount): \(row.fullLabel)")
        case .stereoSide(_, let side):
            lines.append(side == 0 ? "L" : "R")
        case .whole:
            if displayedLaneCount <= 2, let problem = laneProblems[row.lane] { lines.append(problem) }
        }
        let scrollsColumn = columnBox?.scrolls == true
        let scrollsLane = laneBoxes.indices.contains(row.lane) && laneBoxes[row.lane].scrolls
        if scrollsColumn {
            lines.append("Scroll here or drag the bar on the right to see the other sources.")
        } else if scrollsLane {
            lines.append("Scroll here or drag the bar on the right to see the other channels.")
        }
        return lines.joined(separator: "\n")
    }

    private func drawCompactStatus() {
        guard let status = compactStatus else { return }
        let center = statusMarkCenter
        let backing = NSColor.black.withAlphaComponent(0.66)
        backing.setFill()
        NSBezierPath(ovalIn: NSRect(x: center.x - 6.5, y: center.y - 6.5, width: 13, height: 13)).fill()
        let color: NSColor
        if status.problem != nil {
            color = WaveformTheme.statusUnavailable
        } else {
            switch status.state {
            case .unavailable: color = WaveformTheme.statusUnavailable
            case .paused: color = WaveformTheme.compactPaused
            case .waiting: color = WaveformTheme.compactWaiting
            case .recording: color = status.lit ? WaveformTheme.statusRecordingBright : WaveformTheme.statusRecordingDim
            }
        }
        color.setFill()
        if status.state == .paused && status.problem == nil {
            for offset: CGFloat in [-2, 2] {
                NSBezierPath(
                    roundedRect: NSRect(x: center.x + offset - 1, y: center.y - 3.5, width: 2, height: 7),
                    xRadius: 0.5, yRadius: 0.5
                ).fill()
            }
        } else {
            NSBezierPath(ovalIn: NSRect(x: center.x - 3.5, y: center.y - 3.5, width: 7, height: 7)).fill()
        }
        if let problem = status.problem, let text = statusProblemText(problem) {
            backing.setFill()
            NSBezierPath(roundedRect: text.rect.insetBy(dx: -4, dy: -1.5), xRadius: 3, yRadius: 3).fill()
            text.text.draw(in: text.rect)
        }
    }

    private func drawEmptyHint() {
        guard visibleFrames.isEmpty, showsOverlay || compactStatus?.problem == nil else { return }
        let text = NSAttributedString(string: "No captured audio", attributes: [
            .font: NSFont.systemFont(ofSize: showsOverlay ? 13 : 12),
            .foregroundColor: showsOverlay ? NSColor.gray : NSColor(white: 104 / 255, alpha: 1)
        ])
        let size = text.size()
        guard size.width < audioRect.width, size.height < audioRect.height else { return }
        text.draw(at: NSPoint(x: audioRect.midX - size.width / 2, y: audioRect.midY - size.height / 2))
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let pointerArea { removeTrackingArea(pointerArea) }
        let area = NSTrackingArea(
            rect: .zero,
            options: [.mouseEnteredAndExited, .mouseMoved, .activeAlways, .inVisibleRect, .enabledDuringMouseDrag],
            owner: self, userInfo: nil
        )
        addTrackingArea(area)
        pointerArea = area
    }

    private func readRequest() -> WaveformReadRequest {
        WaveformReadRequest(view: requestedView ?? (zoomed || editMaterial != nil ? visibleFrames : nil),
            width: Float(audioRect.width), height: Float(audioRect.height), rows: requestedRows,
            interactive: anchor != nil || mouseDownEvent != nil || dragging || panStart != nil
                || fadeGesture != nil || scrollDrag != nil)
    }

    private func refreshFrequency(tap: SystemAudioTap) {
        guard frequencyGradient != nil, !frequencyReading, !materialPending, !renderedFrames.isEmpty,
              let revision, let bins = traces.first?.low.count, bins > 0,
              ProcessInfo.processInfo.systemUptime >= nextFrequencyRead else { return }
        let request = FrequencyRequest(revision: revision, materialVersion: materialVersion,
            range: renderedFrames, bins: bins, rows: displayedRows)
        guard request != lastFrequencyRequest else { return }
        frequencyReading = true
        let read: (WAVExport?) -> Void = { [weak self] audio in
            guard let self else { return }
            guard let audio, self.materialVersion == request.materialVersion else {
                self.frequencyReading = false
                return
            }
            self.frequencyQueue.async { [weak self] in
                let start = ProcessInfo.processInfo.systemUptime
                let spectrum = audio.spectrum(range: request.range, bins: request.bins, rows: request.rows)
                let elapsed = ProcessInfo.processInfo.systemUptime - start
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.frequencyReading = false
                    self.nextFrequencyRead = ProcessInfo.processInfo.systemUptime + elapsed * 2
                    // Rows that come and go while recording (a channel turning silent or sounding
                    // again) no longer throw the read away; each row drawn finds its own colours.
                    guard self.frequencyGradient != nil, self.materialVersion == request.materialVersion else { return }
                    self.frequencyWaveform = spectrum
                    self.frequencyRows = request.rows
                    self.lastFrequencyRequest = request
                    self.needsDisplay = true
                }
            }
        }
        if let editMaterial {
            read(editMaterial)
        } else {
            let routes = tap.captureRoutes
            let labels = tap.captureChannelNames
            let groups = (0..<tap.laneCount).map { tap.channelGroups(for: $0) }
            tap.readCapture(poll: false, { WAVExport(core: $0, routes: routes, channelNames: labels,
                channelGroups: groups, range: request.range) }, completion: read)
        }
    }

    func refresh(tap: SystemAudioTap, worker: DispatchQueue) {
        guard !materialPending else { return }
        let widths = (0..<tap.laneCount).map { tap.channelCount(for: $0) ?? 0 }
        let material = editMaterial
        if !isPointerGestureActive || requestedRows.isEmpty {
            let activeMasks: [UInt64?] = (0..<tap.laneCount).map { lane in
                guard hideSilentChannels, tap.canHideSilentChannels(for: lane) else { return nil }
                if let material { return material.activeChannels(for: lane) }
                return tap.liveActiveMask(for: lane, hideSilent: true)
            }
            let shownGroups: [(group: AddonChannelGroup, range: Range<Int>, hasBed: Bool)?] = (0..<tap.laneCount).map { lane in
                controller?.resolvedShownGroup(for: lane)
            }
            requestedRows = WaveformRow.layout(widths: widths, labels: tap.captureChannelNames,
                                               stereo: stereo, activeMasks: activeMasks,
                                               shownGroups: shownGroups)
            requestedGroupIDs = shownGroups.map { $0?.group.id }
        }
        refreshFrequency(tap: tap)
        let request = readRequest()
        if material != nil && revision != nil && request == lastRenderedRequest { return }
        guard renderGate.request() else { return }
        let unchangedRevision = request == lastRenderedRequest ? revision : nil
        let version = materialVersion
        let laneCount = tap.laneCount
        let receive: (WaveformFrame?) -> Void = { [weak self] frame in
            guard let self else { return }
            let stale = self.materialVersion != version || self.materialPending
                || self.editMaterial !== material || self.readRequest() != request
            let retry = self.renderGate.complete(stale: stale)
            defer {
                if retry && !self.materialPending { self.controller?.refreshWaveform() }
            }
            guard !stale, let frame else { return }
            let rowsChanged = self.displayedRows != request.rows || self.displayedLaneWidths != widths
            self.revision = frame.revision
            self.sampleRate = frame.sampleRate
            self.retainedFrames = frame.retained
            self.visibleFrames = frame.visible
            self.renderedFrames = frame.visible
            self.requestedView = nil
            self.zoomed = frame.visible != frame.retained
            self.showRows(request.rows, groups: self.requestedGroupIDs)
            self.displayedLaneCount = laneCount
            self.displayedLaneWidths = widths
            self.traces = frame.traces
            self.bars = frame.bars
            self.recomputeRowLayout()
            self.lastRenderedRequest = self.readRequest()
            self.updateDisplayPolicy()
            self.refreshFrequency(tap: tap)
            if rowsChanged { self.controller?.maybeAutoGrowWindow() }
            self.needsDisplay = true
        }
        if let material {
            worker.async {
                let frame = material.render(request)
                DispatchQueue.main.async { receive(frame) }
            }
        } else if request.rows.contains(where: \.isPair) {
            let routes = tap.captureRoutes
            let labels = tap.captureChannelNames
            let groups = (0..<tap.laneCount).map { tap.channelGroups(for: $0) }
            tap.readCapture(poll: false, { core -> WAVExport? in
                guard bt_tracks_revision(core) != unchangedRevision else { return nil }
                return WAVExport(core: core, routes: routes, channelNames: labels, channelGroups: groups)
            }) { [weak self] material in
                guard let self, let material else { receive(nil); return }
                self.wideWaveformQueue.async {
                    let frame = material.render(request)
                    DispatchQueue.main.async { receive(frame) }
                }
            }
        } else {
            tap.readCapture(poll: false, {
                WaveformFrame.read(core: $0, request: request, unchangedRevision: unchangedRevision)
            }, completion: receive)
        }
    }

    func retainEditingMaterial() {
        guard editMaterial == nil, !materialPending else { return }
        materialPending = true
        materialVersion &+= 1
        let version = materialVersion
        controller?.requestEditingFreeze { [weak self] material in
            guard let self else { return }
            guard self.materialVersion == version else {
                if let material {
                    self.controller?.onEditMaterialReady(material)
                }
                self.controller?.retireEditingMaterial(material)
                return
            }
            self.materialPending = false
            guard let material else {
                self.controller?.onEditMaterialUnavailable()
                self.controller?.setNotice("No audio available for this selection.")
                return
            }
            self.editMaterial = material
            self.retainedFrames = material.span
            self.requestedView = fittedWaveformRange(self.requestedView ?? self.visibleFrames, in: material.span)
            if let anchor = self.anchor {
                self.anchor = min(max(anchor, material.span.lowerBound), material.span.upperBound)
            }
            if let selection = self.selection {
                let first = min(max(selection.lowerBound, material.span.lowerBound), material.span.upperBound)
                let end = max(first, min(selection.upperBound, material.span.upperBound))
                self.selection = first..<end
                if first == end && self.anchor == nil {
                    self.clearSelection()
                    self.controller?.setNotice("The selected audio is unavailable.")
                    return
                }
            }
            self.zoomed = self.visibleFrames != self.retainedFrames
            self.revision = nil
            self.controller?.refreshWaveform()
            self.controller?.onEditMaterialReady(material)
            if self.dragRequested && !self.dragging { self.prepareDrag() }
        }
    }

    func materialForExport(range: Range<UInt64>?) -> WAVExport? {
        guard let editMaterial else { return nil }
        if let range, (range.lowerBound < editMaterial.span.lowerBound || range.upperBound > editMaterial.span.upperBound) {
            return nil
        }
        return editMaterial
    }

    private func fittedRange(start: Double, span requested: UInt64) -> Range<UInt64> {
        let span = min(requested, retainedFrames.upperBound - retainedFrames.lowerBound)
        let low = Double(retainedFrames.lowerBound)
        let high = Double(retainedFrames.upperBound - span)
        let first = UInt64(min(max(start.rounded(), low), high))
        return first..<(first + span)
    }

    private func changeView(to range: Range<UInt64>) {
        guard range != (requestedView ?? visibleFrames) else { return }
        requestedView = range
        revision = nil
        controller?.refreshWaveform()
    }

    private func scrollBoxAt(point: NSPoint) -> (laneIndex: Int?, box: LaneLayoutBox)? {
        if let col = columnBox, col.scrolls {
            return (nil, col)
        }
        let yFromTop = audioRect.maxY - point.y
        for (i, box) in laneBoxes.enumerated() where box.scrolls {
            if yFromTop >= box.top && yFromTop <= box.top + box.height {
                return (i, box)
            }
        }
        return nil
    }

    private func isOverScrollbar(point: NSPoint) -> Bool {
        guard point.x >= bounds.maxX - RowStackLayout.scrollbarHitWidth else { return false }
        return scrollBoxAt(point: point) != nil
    }

    private func scrollRows(by steps: Int, at point: NSPoint) -> Bool {
        guard steps != 0, let target = scrollBoxAt(point: point) else { return false }
        if let lane = target.laneIndex {
            let maxFirst = max(0, target.box.rows - target.box.shown)
            let next = min(maxFirst, max(0, target.box.first + steps))
            guard next != laneFirstRows[lane] else { return true }
            laneFirstRows[lane] = next
        } else {
            let maxFirst = max(0, target.box.rows - target.box.shown)
            let next = min(maxFirst, max(0, target.box.first + steps))
            guard next != columnFirstRow else { return true }
            columnFirstRow = next
        }
        recomputeRowLayout()
        needsDisplay = true
        return true
    }

    override func scrollWheel(with event: NSEvent) {
        let loc = convert(event.locationInWindow, from: nil)
        let inGutter = gutterWidth > 0 && loc.x < audioRect.minX
        let inScrollEdge = loc.x >= bounds.maxX - RowStackLayout.scrollbarHitWidth
        if (inGutter || inScrollEdge), event.scrollingDeltaY != 0 {
            let raw = event.hasPreciseScrollingDeltas ? event.scrollingDeltaY / 16.0 : event.scrollingDeltaY
            let mag = max(1, Int(abs(raw).rounded(.up)))
            // Up (positive deltaY on macOS) scrolls to earlier rows (-steps).
            let steps = raw > 0 ? -mag : mag
            if scrollRows(by: steps, at: loc) { return }
        }
        guard !dragging, !visibleFrames.isEmpty, event.scrollingDeltaY != 0 else { return }
        let fraction = Double(fraction(at: loc.x))
        let delta: Double
        if event.hasPreciseScrollingDeltas {
            delta = Double(event.scrollingDeltaY) * 0.014
        } else {
            // Windows: x1.22 per notch, clamped to +-12 notches per frame.
            let notches = max(-12.0, min(12.0, Double(event.scrollingDeltaY)))
            delta = notches * log(1.22)
        }
        changeView(to: zoomedWaveformRange(requestedView ?? visibleFrames, in: retainedFrames,
                                          width: Double(audioRect.width), cursorFraction: fraction, delta: delta))
    }

    override func otherMouseDown(with event: NSEvent) {
        guard event.buttonNumber == 2, zoomed, !dragging else { return }
        panStart = (convert(event.locationInWindow, from: nil).x, requestedView ?? visibleFrames)
        updateWaveformCursor()
    }

    override func otherMouseDragged(with event: NSEvent) {
        guard event.buttonNumber == 2, let panStart else { return }
        let delta = Double((convert(event.locationInWindow, from: nil).x - panStart.x) / max(1, audioRect.width))
        let span = panStart.range.upperBound - panStart.range.lowerBound
        changeView(to: fittedRange(start: Double(panStart.range.lowerBound) - delta * Double(span), span: span))
    }

    override func otherMouseUp(with event: NSEvent) {
        if event.buttonNumber == 2 {
            panStart = nil
            updateWaveformCursor()
        }
    }

    func clearSelection() {
        stopEdgeAutoScroll()
        player?.stop()
        controller?.selectionNoLongerByAddon()
        selection = nil
        selectionPressX = nil
        selectionMoved = false
        selectionAnchorRow = nil
        selectionCursorRow = nil
        rememberedRows = []
        wholeLanes = []
        fades = noFades
        fadeGesture = nil
        requestedView = nil
        materialVersion &+= 1
        materialPending = false
        let previous = editMaterial
        editMaterial = nil
        controller?.retireEditingMaterial(previous)
        revision = nil
        discardPreparedFile()
        needsDisplay = true
    }

    func resetTimeline() {
        clearSelection()
        anchor = nil
        mouseDownEvent = nil
        scrollDrag = nil
        retainedFrames = 0..<0
        visibleFrames = 0..<0
        renderedFrames = 0..<0
        zoomed = false
        panStart = nil
        revision = nil
        frequencyWaveform = nil
        lastFrequencyRequest = nil
        nextFrequencyRead = 0
        requestedRows = []
        displayedRows = []
        displayedLaneWidths = []
        laneFirstRows = []
        columnFirstRow = 0
        laneBoxes = []
        columnBox = nil
        traces = []
        bars.removeAll()
        marker = nil
        markerRows = []
        committedMarkerRevision = reserveMarker()
    }

    func reserveMarker() -> UInt64 {
        markerRevision &+= 1
        return markerRevision
    }

    /// Leaves the marker on the exported range and rows.
    func commitMarker(_ range: Range<UInt64>, rows: [RowIdentity]?, ticket: UInt64) {
        guard ticket > committedMarkerRevision, !range.isEmpty else { return }
        committedMarkerRevision = ticket
        marker = range
        markerRows = rows ?? []
        needsDisplay = true
    }

    func selectionForSaving() -> Range<UInt64>? {
        guard let selection, !selection.isEmpty else { return nil }
        return selection
    }

    func fadesForSaving() -> BtFades {
        guard let selection = selectionForSaving() else { return noFades }
        return bt_fades_fitted(fades, selection.upperBound - selection.lowerBound)
    }

    var hasSelection: Bool { selectionForSaving() != nil }

    func togglePreview() -> Bool {
        guard let selection = selectionForSaving() else { return false }
        guard anchor == nil, fadeGesture == nil, mouseDownEvent == nil, !dragging else { return true }
        if TestCopyConfig.current.isTestCopy {
            controller?.setNotice("a test copy of Backtracking does not play audio")
            return true
        }
        if player?.range != nil {
            player?.stop()
            return true
        }
        guard let editMaterial else {
            controller?.setNotice(materialPending ? "Preparing selection…" : "The selected audio is unavailable.")
            return true
        }
        #if BACKTRACKING_CAPTURE_VERIFY
        if verifySilentPreview { verifyPreviewRange = selection; return true }
        #endif
        if player == nil {
            player = PreviewPlayer()
            player?.onChange = { [weak self] in self?.needsDisplay = true }
            player?.onProblem = { [weak self] msg in self?.controller?.setNotice(msg) }
        }
        guard let player else {
            controller?.setNotice("Could not open the playback device (unavailable)")
            return true
        }
        let problem = player.play(
            editMaterial, range: selection,
            forceMono: controller?.forceMonoExport ?? false, fades: fadesForSaving(),
            selectedRows: selectedRowsForExport,
            hideSilentLanes: controller?.hideSilentLaneIndices ?? [],
            pairBinaural: controller?.pairBinaural ?? false,
            shownGroupIDs: controller?.currentShownGroupIDs() ?? []
        )
        if let problem { controller?.setNotice(problem) }
        return true
    }

    func endPreview() {
        player = nil
        needsDisplay = true
    }

    func stopPreview() {
        player?.stop()
    }

    func clampSelectionToHistory() -> Bool {
        if hasEditingMaterial { return true }
        guard let selection else { return true }
        let start = max(selection.lowerBound, retainedFrames.lowerBound)
        let end = min(selection.upperBound, retainedFrames.upperBound)
        guard start < end else { clearSelection(); return false }
        self.selection = start..<end
        needsDisplay = true
        return true
    }

    var canCancelSelection: Bool { !dragging && (anchor != nil || selection != nil) }

    func cancelSelection() -> Bool {
        guard canCancelSelection else { return false }
        stopEdgeAutoScroll()
        anchor = nil
        mouseDownEvent = nil
        clearSelection()
        controller?.resumeAfterSelectionCancellation()
        return true
    }

    func cancelPendingDrag() -> Bool {
        guard dragRequested, !dragging else { return false }
        mouseDownEvent = nil
        discardPreparedFile()
        controller?.setNotice(nil)
        endGesture()
        return true
    }

    func abortGestures() {
        stopEdgeAutoScroll()
        scrollDrag = nil
        fadeGesture = nil
        panStart = nil
        anchor = nil
        mouseDownEvent = nil
        if !dragging && dragRequested { discardPreparedFile() }
    }

    func discardPreparedFile() {
        pendingDragWrite?.cancel()
        pendingDragWrite = nil
        for file in readyFiles { try? FileManager.default.removeItem(at: file) }
        readyFiles.removeAll()
        pendingDragMarker = nil
        dragRequested = false
        exportVersion &+= 1
        needsDisplay = true
    }

    private func x(for frame: UInt64) -> CGFloat {
        let fraction = Double(frame - visibleFrames.lowerBound)
            / Double(max(1, visibleFrames.upperBound - visibleFrames.lowerBound))
        return audioRect.minX + audioRect.width * CGFloat(fraction)
    }

    private func isVisible(_ frame: UInt64) -> Bool {
        !visibleFrames.isEmpty && frame >= visibleFrames.lowerBound && frame <= visibleFrames.upperBound
    }

    private func flagPosition(for frame: UInt64) -> CGFloat {
        min(audioRect.maxX - 1.5, max(audioRect.minX + 1.5, x(for: frame)))
    }

    /// Picks a visible flag within 8 px horizontally and [top-13, top+2] vertically.
    /// Nearer flag wins; equal distance picks the start flag.
    private func draggableFlag(at position: NSPoint) -> UInt64? {
        guard let flags = selection ?? marker, let top = flagTopY() else { return nil }
        guard position.y >= top - 13 && position.y <= top + 2 else { return nil }
        let candidates: [(frame: UInt64, distance: CGFloat, isStart: Bool)] = [
            (flags.lowerBound, abs(flagPosition(for: flags.lowerBound) - position.x), true),
            (flags.upperBound, abs(flagPosition(for: flags.upperBound) - position.x), false)
        ].filter { isVisible($0.frame) && $0.distance <= 8 }
        return candidates.min { a, b in
            if abs(a.distance - b.distance) < 0.001 { return a.isStart && !b.isStart }
            return a.distance < b.distance
        }?.frame
    }

    func hasDraggableFlag(at windowPoint: NSPoint) -> Bool {
        draggableFlag(at: convert(windowPoint, from: nil)) != nil
    }

    private func snapTarget(near position: CGFloat) -> UInt64? {
        updateDisplayPolicy()
        var flags = marker.map { [$0.lowerBound, $0.upperBound] } ?? []
        if let selection {
            flags += [selection.lowerBound, selection.upperBound].filter { anchor == nil || $0 == anchor }
        }
        flags = flags.filter(isVisible)
        let pointerFraction = Double(fraction(at: position))
        let snapped = flags.withUnsafeBufferPointer { flags in
            guideBars.withUnsafeBufferPointer { bars in
                guideSeconds.withUnsafeBufferPointer { seconds in
                    bt_waveform_snap_visible(pointerFraction, visibleFrames.lowerBound, visibleFrames.upperBound,
                        flags.baseAddress, flags.count, bars.baseAddress, bars.count,
                        seconds.baseAddress, seconds.count, 8 / Double(max(1, audioRect.width)))
                }
            }
        }
        let frame = visibleFrames.lowerBound
            + UInt64((Double(visibleFrames.upperBound - visibleFrames.lowerBound) * snapped).rounded())
        return (flags + guideBars + guideSeconds).contains(frame) ? frame : nil
    }

    private func fraction(at x: CGFloat) -> CGFloat {
        min(1, max(0, (x - audioRect.minX) / max(1, audioRect.width)))
    }

    private func frame(at event: NSEvent) -> UInt64 {
        let position = convert(event.locationInWindow, from: nil).x
        if event.modifierFlags.contains(.shift), let snapped = snapTarget(near: position) { return snapped }
        let count = visibleFrames.upperBound - visibleFrames.lowerBound
        return visibleFrames.lowerBound + UInt64((Double(fraction(at: position)) * Double(count)).rounded())
    }

    func refreshHintModifiers() {
        guard pointer != nil, anchor == nil, mouseDownEvent == nil, !dragging else { return }
        let current = NSEvent.modifierFlags.intersection([.option, .control, .command, .shift])
        guard current != pointerModifiers else { return }
        pointerModifiers = current
        updateWaveformCursor()
        needsDisplay = true
    }

    private func trackPointer(_ event: NSEvent) {
        let position = convert(event.locationInWindow, from: nil)
        pointer = bounds.contains(position) ? position : nil
        pointerModifiers = event.modifierFlags.intersection([.option, .control, .command, .shift])
        updateWaveformCursor()
        needsDisplay = true
    }

    private func trackDragPointer() {
        guard let window else { return }
        let position = convert(window.mouseLocationOutsideOfEventStream, from: nil)
        pointer = bounds.contains(position) ? position : nil
        needsDisplay = true
    }

    private func updateWaveformCursor() {
        guard let pointer else {
            if waveformCursorActive {
                NSCursor.arrow.set()
                waveformCursorActive = false
            }
            return
        }
        if fadeGesture != nil || (selectionForSaving() != nil && dragsFade(pointerModifiers) && draggableFlag(at: pointer) != nil) {
            NSCursor.resizeLeftRight.set()
            waveformCursorActive = true
        } else if isOverOverlayChip(at: pointer) || isOverScrollbar(point: pointer) || scrollDrag != nil {
            NSCursor.pointingHand.set()
            waveformCursorActive = true
        } else if anchor != nil || dragging || panStart != nil || mouseDownEvent != nil {
            NSCursor.closedHand.set()
            waveformCursorActive = true
        } else if audioRect.contains(pointer) {
            NSCursor.openHand.set()
            waveformCursorActive = true
        } else if waveformCursorActive {
            NSCursor.arrow.set()
            waveformCursorActive = false
        }
    }

    override func mouseEntered(with event: NSEvent) { mouseMoved(with: event) }

    override func mouseMoved(with event: NSEvent) { trackPointer(event) }

    func clearPointerFeedback() {
        pointer = nil
        updateWaveformCursor()
        needsDisplay = true
    }

    override func mouseExited(with event: NSEvent) { clearPointerFeedback() }

    private func dragsFade(_ modifiers: NSEvent.ModifierFlags) -> Bool {
        modifiers.contains(.option) && modifiers.intersection([.command, .control]).isEmpty
    }

    private func hasNoSelectionModifiers(_ modifiers: NSEvent.ModifierFlags) -> Bool {
        modifiers.intersection([.command, .option, .control, .shift]).isEmpty
    }

    private func freeFrame(at x: CGFloat) -> UInt64 {
        let count = visibleFrames.upperBound - visibleFrames.lowerBound
        return visibleFrames.lowerBound + UInt64((Double(fraction(at: x)) * Double(count)).rounded())
    }

    /// With one or two sources, the scrolling lane the drag's row is in scrolls while the pointer
    /// is past that lane's top or bottom and it has rows that way (Windows `row_drag`).
    private func laneEdgeScroll(at point: NSPoint) -> (lane: Int, direction: Int)? {
        guard columnBox == nil, let lane = selectionCursorRow?.lane, laneBoxes.indices.contains(lane),
              laneFirstRows.indices.contains(lane) else { return nil }
        let box = laneBoxes[lane]
        guard box.scrolls else { return nil }
        let yFromTop = audioRect.maxY - point.y
        if yFromTop < box.top && laneFirstRows[lane] > 0 { return (lane, -1) }
        if yFromTop >= box.top + box.height && laneFirstRows[lane] + box.shown < box.rows { return (lane, 1) }
        return nil
    }

    private func startEdgeAutoScrollIfNeeded(at point: NSPoint) {
        guard anchor != nil, selectionUpdatesRows else { stopEdgeAutoScroll(); return }
        edgeScrollPoint = point
        let past: Bool
        if columnBox != nil {
            past = point.y > audioRect.maxY || point.y < audioRect.minY
        } else {
            past = laneEdgeScroll(at: point) != nil
        }
        if past {
            if edgeScrollTimer == nil {
                let t = Timer(timeInterval: 0.09, repeats: true) { [weak self] _ in
                    self?.edgeAutoScrollTick()
                }
                RunLoop.main.add(t, forMode: .common)
                edgeScrollTimer = t
                // The first row scrolls in at once, then one every 90 ms (Windows `EDGE_SCROLL_STEP`).
                edgeAutoScrollTick()
            }
        } else {
            stopEdgeAutoScroll()
        }
    }

    private func stopEdgeAutoScroll() {
        edgeScrollTimer?.invalidate()
        edgeScrollTimer = nil
        edgeScrollPoint = nil
    }

    private func edgeAutoScrollTick() {
        // The pointer held still keeps scrolling; it is where the last drag event left it.
        guard anchor != nil, selectionUpdatesRows, let local = edgeScrollPoint else { stopEdgeAutoScroll(); return }
        var cursor: RowIdentity?
        if let col = columnBox {
            let direction: Int
            if local.y > audioRect.maxY { direction = -1 }
            else if local.y < audioRect.minY { direction = 1 }
            else { stopEdgeAutoScroll(); return }
            guard col.scrolls else { return }
            let maxFirst = max(0, col.rows - col.shown)
            let next = min(maxFirst, max(0, columnFirstRow + direction))
            guard next != columnFirstRow else { return }
            columnFirstRow = next
            recomputeRowLayout()
            cursor = rowIdentity(at: local, clampedToVisible: true)
        } else {
            guard let (lane, direction) = laneEdgeScroll(at: local) else { stopEdgeAutoScroll(); return }
            laneFirstRows[lane] += direction
            recomputeRowLayout()
            // The drag takes the row that came into view at that edge.
            let laneRows = displayedRows.filter { $0.lane == lane }
            let box = laneBoxes[lane]
            let index = direction > 0 ? box.first + box.shown - 1 : box.first
            cursor = laneRows.indices.contains(index) ? laneRows[index].identity : nil
        }
        if let anchorRow = selectionAnchorRow, let cursor {
            selectionCursorRow = cursor
            chooseRows(RowStackLayout.rowsBetween(allRows: currentSpecs(), anchor: anchorRow, cursor: cursor))
        }
        controller?.refreshWaveform()
        needsDisplay = true
    }

    override func mouseDown(with event: NSEvent) {
        guard !dragging, let controller else { return }
        if requestedView != nil {
            requestedView = nil
            revision = nil
        }
        trackPointer(event)
        let position = convert(event.locationInWindow, from: nil)

        // Package 3 & 18: clicking a channel-group switcher chip or compact transport chip never starts selection/drag.
        if handleOverlayChipClick(at: position) {
            return
        }

        // Package 4 / SEL-23: pressing on the row scrollbar scrolls rows, never starts selection/drag.
        if isOverScrollbar(point: position), let target = scrollBoxAt(point: position) {
            let yFromTop = audioRect.maxY - position.y
            let thumb = target.box.thumb()
            let grabOffset: CGFloat
            if thumb.contains(yFromTop) {
                grabOffset = yFromTop - thumb.lowerBound
            } else {
                // Click on track centers thumb at click position and begins dragging.
                let thumbLen = thumb.upperBound - thumb.lowerBound
                let centeredTop = yFromTop - thumbLen / 2
                let nextFirst = target.box.firstForThumb(centeredTop)
                if let lane = target.laneIndex { laneFirstRows[lane] = nextFirst }
                else { columnFirstRow = nextFirst }
                recomputeRowLayout()
                grabOffset = thumbLen / 2
            }
            scrollDrag = ScrollDrag(laneIndex: target.laneIndex, grabOffset: grabOffset)
            updateWaveformCursor()
            needsDisplay = true
            return
        }

        // Pressing in left label gutter does not start selection or drag.
        if gutterWidth > 0 && position.x < audioRect.minX { return }

        let flag = draggableFlag(at: position)
        if let flag, let selection = selectionForSaving(), dragsFade(event.modifierFlags) {
            controller.beginEditing()
            window?.makeKey()
            window?.makeFirstResponder(self)
            fadeGesture = FadeGesture(end: flag == selection.lowerBound ? 0 : 1, press: freeFrame(at: position.x),
                                      originX: position.x, initial: fadesForSaving())
            updateWaveformCursor()
            needsDisplay = true
        } else if let flag, let flags = selection ?? marker, hasNoSelectionModifiers(event.modifierFlags) {
            // Package 5: flag pickup requires no modifiers; 2px move required to keep.
            player?.stop()
            controller.beginEditing()
            window?.makeKey()
            window?.makeFirstResponder(self)
            let edge = min(max(flag, retainedFrames.lowerBound), retainedFrames.upperBound)
            let other = flag == flags.lowerBound ? flags.upperBound : flags.lowerBound
            let opposite = min(max(other, retainedFrames.lowerBound), retainedFrames.upperBound)
            selectionUpdatesRows = false
            // The user's own drag: the selection is no longer the add-on's (Windows ui.rs:6768).
            controller.selectionNoLongerByAddon()
            if selection == nil {
                if !markerRows.isEmpty {
                    chooseRows(markerRows)
                    selectionAnchorRow = markerRows.first
                    selectionCursorRow = markerRows.last
                } else if let r = rowIdentity(at: position) {
                    selectionAnchorRow = r
                    selectionCursorRow = r
                    chooseRows(RowStackLayout.rowsBetween(allRows: currentSpecs(), anchor: r, cursor: r))
                }
            }
            anchor = opposite
            selectionPressX = position.x
            selectionMoved = false
            selection = min(edge, opposite)..<max(edge, opposite)
            updateWaveformCursor()
            needsDisplay = true
        } else if selectionModifierDown {
            clearSelection()
            controller.beginEditing()
            window?.makeKey()
            window?.makeFirstResponder(self)
            controller.refreshWaveform()
            anchor = frame(at: event)
            selectionPressX = position.x
            selectionMoved = false
            selection = anchor.map { $0..<$0 }
            selectionUpdatesRows = false
            if let r = rowIdentity(at: position) {
                selectionAnchorRow = r
                selectionCursorRow = r
                chooseRows(RowStackLayout.rowsBetween(allRows: currentSpecs(), anchor: r, cursor: r))
                selectionUpdatesRows = true
            }
            updateWaveformCursor()
            needsDisplay = true
        } else {
            mouseDownEvent = event
            dragRow = rowIdentity(at: position)
            updateWaveformCursor()
        }
    }

    override func rightMouseDown(with event: NSEvent) {
        if event.modifierFlags.contains(.control) { mouseDown(with: event) }
    }

    override func mouseDragged(with event: NSEvent) {
        trackPointer(event)
        let local = convert(event.locationInWindow, from: nil)
        if let scrollDrag {
            let yFromTop = audioRect.maxY - local.y
            let box: LaneLayoutBox?
            if let lane = scrollDrag.laneIndex, laneBoxes.indices.contains(lane) { box = laneBoxes[lane] }
            else { box = columnBox }
            if let box {
                let nextFirst = box.firstForThumb(yFromTop - scrollDrag.grabOffset)
                if let lane = scrollDrag.laneIndex { laneFirstRows[lane] = nextFirst }
                else { columnFirstRow = nextFirst }
                recomputeRowLayout()
                needsDisplay = true
            }
            return
        }
        if var gesture = fadeGesture {
            guard let selection = selectionForSaving() else { return }
            gesture.moved = gesture.moved || abs(local.x - gesture.originX) >= 2
            if gesture.moved {
                fades = bt_fades_dragged(gesture.end, selection.lowerBound, selection.upperBound,
                                         gesture.initial, gesture.press, freeFrame(at: local.x))
                player?.setFades(fades)
            }
            fadeGesture = gesture
            needsDisplay = true
        } else if let anchor {
            if let pressX = selectionPressX, abs(local.x - pressX) >= 2 {
                selectionMoved = true
            }
            if selectionMoved {
                let position = frame(at: event)
                selection = min(anchor, position)..<max(anchor, position)
            }
            // Past the edge of the scrolling lane the drag is in, that lane scrolls instead.
            if selectionUpdatesRows, laneEdgeScroll(at: local) == nil, let anchorRow = selectionAnchorRow,
               let nextRow = rowIdentity(at: local, clampedToVisible: true) {
                selectionCursorRow = nextRow
                chooseRows(RowStackLayout.rowsBetween(allRows: currentSpecs(), anchor: anchorRow, cursor: nextRow))
            }
            startEdgeAutoScrollIfNeeded(at: local)
            needsDisplay = true
        } else if !dragging, let window, let mouseDownEvent {
            let start = mouseDownEvent.locationInWindow
            let current = event.locationInWindow
            if pointerIsInApp(at: window.convertPoint(toScreen: current)) {
                if dragRequested {
                    discardPreparedFile()
                    controller?.setNotice(nil)
                }
                return
            }
            if !dragRequested && hypot(current.x - start.x, current.y - start.y) >= 4 {
                controller?.beginEditing()
                prepareDrag()
            }
        }
    }

    override func rightMouseDragged(with event: NSEvent) {
        if anchor != nil { mouseDragged(with: event) }
    }

    override func mouseUp(with event: NSEvent) {
        trackPointer(event)
        stopEdgeAutoScroll()
        if scrollDrag != nil {
            scrollDrag = nil
            updateWaveformCursor()
            needsDisplay = true
            return
        }
        if fadeGesture != nil {
            mouseDragged(with: event)
            fadeGesture = nil
            controller?.endEditing(allowImmediateResume: false)
            updateWaveformCursor()
            needsDisplay = true
            return
        }
        guard anchor != nil || mouseDownEvent != nil else { return }
        var keepSelection = false
        if anchor != nil {
            mouseDragged(with: event)
            anchor = nil
            selectionUpdatesRows = false
            // Package 5: require at least 2px horizontal movement and non-empty span to keep selection.
            keepSelection = selectionMoved && hasSelection
            selectionPressX = nil
            if keepSelection, let selection = selectionForSaving() {
                fades = bt_fades_fitted(fades, selection.upperBound - selection.lowerBound)
            }
        } else if !dragging && dragRequested {
            discardPreparedFile()
            controller?.setNotice(nil)
        }
        mouseDownEvent = nil
        updateWaveformCursor()
        if !dragging { endGesture(keepingSelection: keepSelection) }
    }

    override func rightMouseUp(with event: NSEvent) {
        if anchor != nil { mouseUp(with: event) }
    }

    private func pointerIsInApp(at point: NSPoint) -> Bool {
        NSApp.windows.contains { $0.isVisible && $0.frame.contains(point) }
    }

    private func prepareDrag() {
        dragRequested = true
        if materialPending { return }
        guard let controller, let job = controller.frozenExport(range: selectionForSaving()) else {
            controller?.setNotice("No audio captured yet.")
            return
        }
        let version = exportVersion
        let range = selectionForSaving() ?? job.span
        guard range.lowerBound >= job.span.lowerBound, range.upperBound <= job.span.upperBound, !range.isEmpty else {
            controller.setNotice("The selected audio is unavailable.")
            return
        }
        let ticket = reserveMarker()
        let rows: [RowIdentity]?
        if selectionForSaving() != nil {
            rows = selectedRowsForExport
        } else if separateLanes, let dragRow {
            rows = RowStackLayout.rowsBetween(allRows: currentSpecs(), anchor: dragRow, cursor: dragRow)
        } else {
            rows = nil
        }
        pendingDragWrite = controller.prepareDrag(job, range: range, fades: fadesForSaving(), rows: rows) {
            [weak self] urls in
            guard let self, self.exportVersion == version else { return false }
            self.pendingDragWrite = nil
            guard let event = self.mouseDownEvent, NSEvent.pressedMouseButtons & 1 != 0,
                  !self.pointerIsInApp(at: NSEvent.mouseLocation) else {
                self.discardPreparedFile()
                self.controller?.setNotice(nil)
                return false
            }
            if !urls.isEmpty {
                self.pendingDragMarker = (range, rows, ticket)
                self.beginFileDrag(urls, event: event)
            }
            return true
        }
    }

    private func beginFileDrag(_ urls: [URL], event: NSEvent) {
        readyFiles = urls
        dragging = true
        let point = convert(event.locationInWindow, from: nil)
        let items = urls.enumerated().map { index, url in
            let item = NSDraggingItem(pasteboardWriter: url as NSURL)
            let icon = NSWorkspace.shared.icon(forFile: url.path)
            let offset = CGFloat(index) * 6
            item.setDraggingFrame(
                NSRect(x: point.x - 16 + offset, y: point.y - 16 - offset, width: 32, height: 32), contents: icon
            )
            return item
        }
        let session = beginDraggingSession(with: items, event: event, source: self)
        session.animatesToStartingPositionsOnCancelOrFail = true
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .outsideApplication ? .copy : []
    }

    func ignoreModifierKeys(for session: NSDraggingSession) -> Bool { true }

    func draggingSession(_ session: NSDraggingSession, movedTo screenPoint: NSPoint) {
        trackDragPointer()
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        dragging = false
        dragRequested = false
        mouseDownEvent = nil
        trackDragPointer()
        let accepted = operation.contains(.copy)
        let exportedURLs = accepted ? readyFiles : []
        let saved = accepted ? exportDescription(exportedURLs) : nil
        if accepted {
            let frames = pendingDragMarker.map { $0.range.upperBound - $0.range.lowerBound }
                ?? (retainedFrames.upperBound - retainedFrames.lowerBound)
            if !exportedURLs.isEmpty {
                controller?.notifyDraggedClip(urls: exportedURLs, sourceFrames: frames, sourceRate: sampleRate)
            }
            readyFiles.removeAll()
            if let pendingDragMarker {
                commitMarker(pendingDragMarker.range, rows: pendingDragMarker.rows, ticket: pendingDragMarker.ticket)
            }
            pendingDragMarker = nil
        } else { discardPreparedFile() }
        endGesture()
        if let saved { controller?.setNotice("Saved \(saved)") }
    }

    private func endGesture(keepingSelection: Bool = false) {
        if keepingSelection {
            controller?.endEditing(allowImmediateResume: false)
        } else {
            clearSelection()
            controller?.resumeAfterSelectionCancellation()
        }
        controller?.refreshWaveform()
    }

    private func waveformRect(for range: Range<UInt64>) -> NSRect? {
        let first = max(range.lowerBound, visibleFrames.lowerBound)
        let end = min(range.upperBound, visibleFrames.upperBound)
        guard first < end else { return nil }
        return NSRect(x: x(for: first), y: audioRect.minY, width: x(for: end) - x(for: first), height: audioRect.height)
    }

    private func drawFlag(_ frame: UInt64, pointsRight: Bool, top: CGFloat) -> NSRect? {
        guard isVisible(frame) else { return nil }
        let lineX = flagPosition(for: frame)
        let poleHeight = min(11, max(4, audioRect.height - 4))
        let flagWidth: CGFloat = pointsRight ? 6 : -6
        let shape = NSBezierPath()
        shape.move(to: NSPoint(x: lineX, y: top - poleHeight))
        shape.line(to: NSPoint(x: lineX, y: top))
        shape.lineWidth = 1.0
        let snapped = pointerModifiers.contains(.shift) && pointer.flatMap { snapTarget(near: $0.x) } == frame
        let color = snapped ? NSColor.systemYellow : look.flag.nsColor
        color.setStroke()
        shape.stroke()

        let flag = NSBezierPath()
        flag.move(to: NSPoint(x: lineX, y: top))
        flag.line(to: NSPoint(x: lineX + flagWidth, y: top - 3))
        flag.line(to: NSPoint(x: lineX, y: top - 6))
        flag.close()
        color.setFill()
        flag.fill()
        let minX = min(lineX, lineX + flagWidth)
        return NSRect(x: minX - 2, y: top - poleHeight - 2, width: abs(flagWidth) + 4, height: poleHeight + 4)
    }

    private struct RowTracePath {
        let rowIndex: Int
        let rowLabel: String?
        let fill: NSBezierPath
        let outline: NSBezierPath
        let coverage: NSBezierPath
    }

    private func tracePaths() -> [RowTracePath] {
        let strokeWidth = CGFloat(displayPolicy.waveform_stroke) * look.waveformStrokeScale
        let rows = min(displayedRows.count, traces.count)
        guard rows > 0 else { return [] }
        let fitted = fadesForSaving()
        let faded = fitted.fade_in > 0 || fitted.fade_out > 0 ? selectionForSaving() : nil
        var result: [RowTracePath] = []
        result.reserveCapacity(rows)
        for row in 0..<rows {
            guard let rect = rowRect(for: row) else { continue }
            let middle = rect.midY
            if showsOverlay {
                if displayedRows[row].isPair {
                    WaveformTheme.pairBacking.setFill()
                    NSBezierPath.fill(rect)
                }
                look.centerLine.nsColor.setStroke()
                let centerLine = NSBezierPath()
                centerLine.lineWidth = 1
                centerLine.move(to: NSPoint(x: rect.minX, y: middle))
                centerLine.line(to: NSPoint(x: rect.maxX, y: middle))
                centerLine.stroke()
            }
            if renderedFrames.isEmpty { continue }
            let fill = NSBezierPath()
            let outline = NSBezierPath()
            let coverage = NSBezierPath()
            outline.lineWidth = strokeWidth
            outline.lineCapStyle = .round
            outline.lineJoinStyle = .round
            var bins = traces[row]
            if let faded, isRowSelected(displayedRows[row].identity) {
                bins.read { minimum, maximum, count in
                    bt_waveform_fade_bins(minimum, maximum, count, renderedFrames.lowerBound,
                        renderedFrames.upperBound, faded.lowerBound, faded.upperBound, fitted)
                }
            }
            func point(_ x: Double, _ value: Float) -> NSPoint {
                NSPoint(x: rect.minX + CGFloat(x) * rect.width,
                        y: middle + CGFloat(value) * rect.height * WaveformTheme.envelopeAmplitudeScale)
            }
            func cover(_ first: Double, _ end: Double) {
                coverage.append(NSBezierPath(rect: NSRect(
                    x: rect.minX + CGFloat(first) * rect.width, y: rect.minY,
                    width: CGFloat(max(0, end - first)) * rect.width, height: rect.height
                )))
            }
            switch waveformDisplayGeometry(minimum: bins.low, maximum: bins.high, visibleFrames: renderedFrames) {
            case .empty:
                continue
            case .envelope(let runs):
                for columns in runs {
                    guard let first = columns.first, let last = columns.last else { continue }
                    cover(first.x, last.x)
                    outline.move(to: point(first.x, first.maximum))
                    for column in columns.dropFirst() { outline.line(to: point(column.x, column.maximum)) }
                    if columns.allSatisfy({ $0.minimum == $0.maximum }) { continue }
                    outline.move(to: point(first.x, first.minimum))
                    for column in columns.dropFirst() { outline.line(to: point(column.x, column.minimum)) }
                    let polygon = NSBezierPath()
                    polygon.move(to: point(first.x, first.maximum))
                    for column in columns.dropFirst() { polygon.line(to: point(column.x, column.maximum)) }
                    for column in columns.reversed() { polygon.line(to: point(column.x, column.minimum)) }
                    polygon.close()
                    fill.append(polygon)
                }
            case .samples(let runs):
                let count = Double(min(bins.low.count, bins.high.count))
                for samples in runs {
                    guard let first = samples.first, let last = samples.last else { continue }
                    cover(first.x, min(1, last.x + 1 / count))
                    let start = point(first.x, first.value)
                    if samples.count == 1 {
                        let half = max(0.4, outline.lineWidth / 4)
                        outline.move(to: NSPoint(x: start.x - half, y: start.y))
                        outline.line(to: NSPoint(x: start.x + half, y: start.y))
                    } else {
                        outline.move(to: start)
                        for sample in samples.dropFirst() { outline.line(to: point(sample.x, sample.value)) }
                    }
                }
            }
            let rowLabel: String? = displayedRows[row].identity.isPair || displayedRows[row].identity.isWhole
                ? nil : displayedRows[row].fullLabel
            result.append(RowTracePath(rowIndex: row, rowLabel: rowLabel, fill: fill, outline: outline, coverage: coverage))
        }
        return result
    }

    /// Where the row drawn at `index` is in a frequency read made for `read`.
    static func frequencyRowIndex(of index: Int, displayed: [WaveformRow], read: [WaveformRow]) -> Int? {
        guard displayed.indices.contains(index) else { return nil }
        if read.indices.contains(index), read[index] == displayed[index] { return index }
        return read.firstIndex(of: displayed[index])
    }

    private func paintTraces(
        _ rowPaths: [RowTracePath],
        selected: Bool,
        dimFrequency: Bool = false
    ) {
        let accent = RGBA8(rgb: accentRGB)
        let fillOpacity = look.waveformFillOpacity
        if let gradient = frequencyGradient, let context = NSGraphicsContext.current?.cgContext {
            let live = editMaterial == nil ? UInt64(sampleRate) / 4 : 0
            let effective = look.legibleSpectrum(for: gradient).gradient
            let fallbackRGB = look.effectiveWaveform(accent: accent).rgbUInt32
            for item in rowPaths where !item.coverage.isEmpty {
                guard let rect = rowRect(for: item.rowIndex), traces.indices.contains(item.rowIndex) else { continue }
                let colors = Self.frequencyRowIndex(of: item.rowIndex, displayed: displayedRows, read: frequencyRows)
                    .flatMap { row in
                        frequencyWaveform?.colors(
                            for: row, in: renderedFrames, bins: traces[item.rowIndex].low.count,
                            gradient: effective, liveFrames: live
                        )
                    } ?? []
                guard let image = FrequencyWaveform.stripeImage(
                    colors.isEmpty ? [nil] : colors, fallback: fallbackRGB, dimmed: dimFrequency
                ) else { continue }
                NSGraphicsContext.saveGraphicsState()
                item.coverage.addClip()

                context.saveGState()
                context.interpolationQuality = .none
                context.addPath(item.fill.cgPath)
                context.clip()
                context.setAlpha(fillOpacity)
                context.draw(image, in: rect)
                context.restoreGState()

                context.saveGState()
                context.interpolationQuality = .none
                context.addPath(item.outline.cgPath)
                context.setLineWidth(item.outline.lineWidth)
                context.setLineCap(.round)
                context.setLineJoin(.round)
                context.replacePathWithStrokedPath()
                context.clip()
                context.draw(image, in: rect)
                context.restoreGState()

                NSGraphicsContext.restoreGraphicsState()
            }
        } else {
            let selectedColor = look.effectiveWaveformSelected(accent: accent).nsColor
            for item in rowPaths where !item.coverage.isEmpty {
                let rowColor = selected
                    ? selectedColor
                    : look.effectiveRowWaveform(label: item.rowLabel, accent: accent).nsColor
                NSGraphicsContext.saveGraphicsState()
                item.coverage.addClip()
                rowColor.withAlphaComponent(rowColor.alphaComponent * fillOpacity).setFill()
                item.fill.fill()
                rowColor.setStroke()
                item.outline.stroke()
                NSGraphicsContext.restoreGraphicsState()
            }
        }
    }

    private func unclampedX(for frame: UInt64) -> CGFloat {
        let fraction = (Double(frame) - Double(visibleFrames.lowerBound))
            / Double(max(1, visibleFrames.upperBound - visibleFrames.lowerBound))
        return audioRect.minX + audioRect.width * CGFloat(fraction)
    }

    private func drawFades(_ selection: Range<UInt64>) {
        let fitted = fadesForSaving()
        let rows = min(displayedRows.count, traces.count)
        guard fitted.fade_in > 0 || fitted.fade_out > 0, rows > 0, !visibleFrames.isEmpty else { return }
        let ramps = [(selection.lowerBound, selection.lowerBound + fitted.fade_in),
                     (selection.upperBound, selection.upperBound - fitted.fade_out)]
        for row in 0..<rows where isRowSelected(displayedRows[row].identity) {
            guard let rect = rowRect(for: row) else { continue }
            let middle = rect.midY
            let extent = rect.height * WaveformTheme.envelopeAmplitudeScale
            for (edge, full) in ramps where edge != full {
                let edgeX = unclampedX(for: edge)
                let fullX = unclampedX(for: full)
                let silent = NSPoint(x: edgeX, y: middle)
                let top = NSPoint(x: fullX, y: middle + extent)
                let bottom = NSPoint(x: fullX, y: middle - extent)
                NSColor.black.withAlphaComponent(110 / 255).setFill()
                for corner in [top, bottom] {
                    let shade = NSBezierPath()
                    shade.move(to: silent)
                    shade.line(to: NSPoint(x: edgeX, y: corner.y))
                    shade.line(to: corner)
                    shade.close()
                    shade.fill()
                }
                let wedge = NSBezierPath()
                wedge.lineWidth = 1
                wedge.move(to: top)
                wedge.line(to: silent)
                wedge.line(to: bottom)
                look.flag.nsColor.setStroke()
                wedge.stroke()
            }
        }
    }

    private func drawPlayhead() {
        guard let frame = player?.playhead, isVisible(frame) else { return }
        let x = min(audioRect.maxX, max(audioRect.minX, x(for: frame)))
        let color = look.playhead.nsColor
        let line = NSBezierPath()
        line.lineWidth = 1.5
        line.move(to: NSPoint(x: x, y: audioRect.minY))
        line.line(to: NSPoint(x: x, y: audioRect.maxY))
        color.setStroke()
        line.stroke()
        let size = min(10, max(4, audioRect.height)) * 0.5
        let head = NSBezierPath()
        head.move(to: NSPoint(x: x - size, y: audioRect.maxY))
        head.line(to: NSPoint(x: x + size, y: audioRect.maxY))
        head.line(to: NSPoint(x: x, y: audioRect.maxY - size))
        head.close()
        color.setFill()
        head.fill()
    }

    // MARK: Pkg 3 & 18 — Bottom-left Group Switcher Chips & Compact Transport Chips

    private struct OverlayChip {
        enum Action {
            case selectGroup(lane: Int, groupID: String)
            case cycleGroup(lane: Int)
            case toggleTransport(lane: Int)
        }
        let rect: NSRect
        let text: String
        let current: Bool
        let toolTip: String
        let action: Action
    }

    /// The least plot width, and lane height, that take the compact transport (Windows
    /// `TRANSPORT_MIN_WIDTH`, `TRANSPORT_MIN_LANE_HEIGHT`).
    static let transportMinWidth: CGFloat = 240
    static let transportMinLaneHeight: CGFloat = 22

    /// Whether a lane's compact transport is drawn at `right`: left out of a plot too narrow, a
    /// lane too short, or where it would reach past 4 px before the plot's right edge.
    static func transportChipFits(plotWidth: CGFloat, laneHeight: CGFloat, right: CGFloat, plotRight: CGFloat) -> Bool {
        plotWidth >= transportMinWidth && laneHeight >= transportMinLaneHeight && right <= plotRight - 4
    }

    private func overlayChips() -> [OverlayChip] {
        #if BACKTRACKING_CAPTURE_VERIFY
        if let verifyChips { return verifyChips }
        #endif
        guard showsOverlay, let controller else { return [] }
        let font = NSFont.systemFont(ofSize: 10, weight: .medium)
        let chipH: CGFloat = 14
        let padX: CGFloat = 5
        // Windows lays group buttons side by side with no gap.
        let gap: CGFloat = 0
        var chips: [OverlayChip] = []

        for (lane, box) in laneBoxes.enumerated() {
            guard box.height >= 18 else { continue }
            let laneBottomY = audioRect.maxY - box.top - box.height + 3
            var cursorX = audioRect.minX + 4

            // Group switcher chips (Pkg 3 / TL-06, TL-07).
            let groups = controller.channelGroups(for: lane)
            if groups.count >= 2 {
                let currentGroupID = controller.resolvedShownGroup(for: lane)?.group.id ?? groups[0].id
                let widths: [CGFloat] = groups.map { g in
                    ceil((g.label as NSString).size(withAttributes: [.font: font]).width) + padX * 2
                }
                let totalW = widths.reduce(0, +) + CGFloat(max(0, groups.count - 1)) * gap
                if totalW <= audioRect.width * 0.6 {
                    for (i, g) in groups.enumerated() {
                        let w = widths[i]
                        let isCurrent = g.id == currentGroupID
                        let r = NSRect(x: cursorX, y: laneBottomY, width: w, height: chipH)
                        let tip = isCurrent ? "\(g.label) is on show and is what you drag out" : "Show \(g.label)"
                        chips.append(OverlayChip(
                            rect: r, text: g.label, current: isCurrent, toolTip: tip,
                            action: .selectGroup(lane: lane, groupID: g.id)
                        ))
                        cursorX += w + gap
                    }
                } else if let index = groups.firstIndex(where: { $0.id == currentGroupID }) ?? (groups.isEmpty ? nil : 0) {
                    // One button names the group on show and moves to the next (Windows `group_chips`).
                    let cur = groups[index]
                    let next = groups[(index + 1) % groups.count]
                    let w = min(audioRect.width * 0.6, ceil((cur.label as NSString).size(withAttributes: [.font: font]).width) + padX * 2)
                    let r = NSRect(x: cursorX, y: laneBottomY, width: w, height: chipH)
                    chips.append(OverlayChip(
                        rect: r, text: cur.label, current: false,
                        toolTip: "Show \(next.label)",
                        action: .cycleGroup(lane: lane)
                    ))
                    cursorX += w + gap
                }
                cursorX += 4
            }

            // Compact add-on transport chip (Pkg 18 / ADD-W35).
            if let tr = controller.compactTransportChip(for: lane) {
                let symbol = tr.playing ? "⏸" : "▶"
                let label = "\(symbol) \(tr.text)"
                let w = ceil((label as NSString).size(withAttributes: [.font: font]).width) + padX * 2
                if Self.transportChipFits(plotWidth: audioRect.width, laneHeight: box.height,
                                          right: cursorX + w, plotRight: audioRect.maxX) {
                    let r = NSRect(x: cursorX, y: laneBottomY, width: w, height: chipH)
                    let tip = tr.playing ? "Pause \(tr.sourceName)" : "Play \(tr.sourceName)"
                    chips.append(OverlayChip(
                        rect: r, text: label, current: false, toolTip: tip,
                        action: .toggleTransport(lane: lane)
                    ))
                }
            }
        }
        return chips
    }

    private func isOverOverlayChip(at point: NSPoint) -> Bool {
        overlayChips().contains { $0.rect.contains(point) }
    }

    private func handleOverlayChipClick(at point: NSPoint) -> Bool {
        guard let controller, let chip = overlayChips().first(where: { $0.rect.contains(point) }) else { return false }
        switch chip.action {
        case .selectGroup(let lane, let groupID):
            _ = controller.selectChannelGroup(lane: lane, groupID: groupID)
        case .cycleGroup(let lane):
            controller.cycleChannelGroup(lane: lane)
        case .toggleTransport(let lane):
            controller.toggleAddonTransport(lane: lane)
        }
        return true
    }

    /// Registers a tooltip over each chip, so hovering one says what it does.
    private func syncChipToolTips(_ chips: [OverlayChip]) {
        let shown = chips.map { ChipToolTip(rect: $0.rect, text: $0.toolTip) }
        guard shown != chipToolTips else { return }
        for tag in chipToolTipTags { removeToolTip(tag) }
        chipToolTips = shown
        chipToolTipTags = shown.map { addToolTip($0.rect, owner: self, userData: Self.chipToolTipData) }
    }

    /// The chips drawing will show, with their tooltips registered.
    @discardableResult
    private func syncChipsForDisplay() -> [OverlayChip] {
        let chips = overlayChips()
        syncChipToolTips(chips)
        return chips
    }

    private func drawOverlayChips() {
        let chips = syncChipsForDisplay()
        guard !chips.isEmpty else { return }
        let accent = RGBA8(rgb: accentRGB)
        let font = NSFont.systemFont(ofSize: 10, weight: .medium)
        let bgColor = look.chipBackground.nsColor
        let currentBg = look.effectiveChipCurrent(accent: accent).nsColor
        let textColor = look.chipText.nsColor
        for chip in chips {
            let hovered = pointer.map { chip.rect.contains($0) } ?? false
            (chip.current ? currentBg : bgColor).setFill()
            NSBezierPath(roundedRect: chip.rect, xRadius: 3, yRadius: 3).fill()
            let fg = (chip.current || hovered) ? NSColor.white : textColor
            let attr = NSAttributedString(string: chip.text, attributes: [.font: font, .foregroundColor: fg])
            let sz = attr.size()
            attr.draw(at: NSPoint(x: chip.rect.midX - sz.width / 2, y: chip.rect.midY - sz.height / 2))
        }
    }

    /// Package 6: length / fade label in top-right (8px inset), avoiding drawn flags.
    private func drawLengthLabel(flagRects: [NSRect], hoverRows: [RowIdentity]?) {
        guard showsOverlay, sampleRate > 0 else { return }
        let fullText: String
        let shortText: String
        if let fadeGesture {
            let fitted = fadesForSaving()
            let frames = fadeGesture.end == 0 ? fitted.fade_in : fitted.fade_out
            let s = String(format: "%@ %.2f s", fadeGesture.end == 0 ? "Fade in" : "Fade out",
                           Double(frames) / Double(sampleRate))
            fullText = s
            shortText = s
        } else if let sel = selection, !sel.isEmpty {
            let sec = String(format: "%.2f s", Double(sel.upperBound - sel.lowerBound) / Double(sampleRate))
            let rowsName = selectedRowsLabel
            fullText = rowsName.isEmpty ? sec : "\(rowsName) · \(sec)"
            shortText = sec
        } else if selection == nil, !retainedFrames.isEmpty, pointer != nil || mouseDownEvent != nil || dragging {
            let sec = String(format: "%.2f s", Double(retainedFrames.upperBound - retainedFrames.lowerBound) / Double(sampleRate))
            let rowsName = hoverRows.map {
                RowStackLayout.rowsLabel($0, dual: displayedLaneCount > 1, channelLabels: controller?.currentChannelLabels ?? [])
            } ?? ""
            fullText = rowsName.isEmpty ? sec : "\(rowsName) · \(sec)"
            shortText = sec
        } else {
            return
        }

        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular),
            .foregroundColor: NSColor.white
        ]
        func attributed(_ s: String) -> (NSAttributedString, NSSize) {
            let a = NSAttributedString(string: s, attributes: attrs)
            return (a, a.size())
        }
        func collides(_ rect: NSRect) -> Bool {
            flagRects.contains { $0.intersects(rect) }
        }
        let topY = audioRect.maxY - 2
        let (fullAttr, fullSize) = attributed(fullText)
        let rightRect = NSRect(x: audioRect.maxX - 8 - fullSize.width, y: topY - fullSize.height, width: fullSize.width, height: fullSize.height)
        if !collides(rightRect) {
            fullAttr.draw(at: rightRect.origin)
            return
        }
        let (shortAttr, shortSize) = attributed(shortText)
        let shortRightRect = NSRect(x: audioRect.maxX - 8 - shortSize.width, y: topY - shortSize.height, width: shortSize.width, height: shortSize.height)
        if !collides(shortRightRect) {
            shortAttr.draw(at: shortRightRect.origin)
            return
        }
        let leftRect = NSRect(x: audioRect.minX + 8, y: topY - shortSize.height, width: shortSize.width, height: shortSize.height)
        if !collides(leftRect) {
            shortAttr.draw(at: leftRect.origin)
            return
        }
        let belowY = max(audioRect.minY + 2, topY - 15 - shortSize.height)
        shortAttr.draw(at: NSPoint(x: audioRect.maxX - 8 - shortSize.width, y: belowY))
    }

    private func drawRowDecorations(separate: Bool) {
        guard showsOverlay else { return }
        let rows = displayedRows.count
        guard rows > 1 else { return }
        // Pair bottom divider (below second row of L/R pair in wide sources).
        for row in 0..<(rows - 1) {
            if displayedRows[row].isPair, !displayedRows[row + 1].isPair,
               displayedRows[row].lane == displayedRows[row + 1].lane,
               let rect = rowRect(for: row) {
                WaveformTheme.pairDivider.setStroke()
                let line = NSBezierPath()
                line.lineWidth = 1
                line.move(to: NSPoint(x: audioRect.minX, y: rect.minY))
                line.line(to: NSPoint(x: audioRect.maxX, y: rect.minY))
                line.stroke()
            }
        }
        // Stacked lane divider (1 px white alpha 36 between lanes when stacked).
        if hasStackedLane && displayedLaneCount > 1 {
            WaveformTheme.laneDivider.setStroke()
            for row in 0..<(rows - 1) where displayedRows[row].lane != displayedRows[row + 1].lane {
                guard let rect = rowRect(for: row) else { continue }
                let line = NSBezierPath()
                line.lineWidth = 1
                line.move(to: NSPoint(x: bounds.minX, y: rect.minY))
                line.line(to: NSPoint(x: bounds.maxX, y: rect.minY))
                line.stroke()
            }
        }
        // Package 1 & 20: when exactly two sources are edited separately, draw lane divider using look.
        if separate && displayedLaneCount == 2 {
            if let splitIndex = (0..<(rows - 1)).first(where: { displayedRows[$0].lane != displayedRows[$0 + 1].lane }),
               let rect = rowRect(for: splitIndex) {
                look.effectiveLaneDivider(accent: RGBA8(rgb: accentRGB)).nsColor.setStroke()
                let line = NSBezierPath()
                line.lineWidth = 1
                line.move(to: NSPoint(x: audioRect.minX, y: rect.minY))
                line.line(to: NSPoint(x: audioRect.maxX, y: rect.minY))
                line.stroke()
            }
        }
    }

    private func drawRowLabels() {
        guard showsOverlay else { return }
        let rowH = laneBoxes.first?.rowHeight ?? 0
        guard rowH >= RowStackLayout.labelledRowHeight else { return }
        let font = NSFont.monospacedSystemFont(ofSize: 8, weight: .regular)
        let labelColor = look.rowLabel.nsColor
        var drawnLaneLetters = Set<Int>()
        for (index, row) in displayedRows.enumerated() {
            guard let rect = rowRect(for: index) else { continue }
            let hasProblem = laneProblems[row.lane] != nil
            let letterColor = hasProblem ? WaveformTheme.statusUnavailable : labelColor
            if gutterWidth > 0 {
                if displayedLaneCount > 2 {
                    if !drawnLaneLetters.contains(row.lane) {
                        drawnLaneLetters.insert(row.lane)
                        let letter = NSAttributedString(string: captureLaneLetter(row.lane),
                                                        attributes: [.font: font, .foregroundColor: letterColor])
                        let sz = letter.size()
                        letter.draw(at: NSPoint(x: bounds.minX + 2, y: rect.midY - sz.height / 2))
                    }
                    if hasStackedLane, let ch = row.shortLabel, !row.identity.isWhole {
                        let chAttr = NSAttributedString(string: ch, attributes: [.font: font, .foregroundColor: labelColor])
                        let sz = chAttr.size()
                        chAttr.draw(at: NSPoint(x: bounds.minX + 14, y: rect.midY - sz.height / 2))
                    }
                } else if let ch = row.shortLabel {
                    let color = row.identity.isWhole ? letterColor : labelColor
                    let chAttr = NSAttributedString(string: ch, attributes: [.font: font, .foregroundColor: color])
                    let sz = chAttr.size()
                    chAttr.draw(at: NSPoint(x: bounds.minX + 2, y: rect.midY - sz.height / 2))
                }
            } else if let short = row.shortLabel {
                // 1-2 non-stacked sources (2 lanes A/B, or single stereo L/R): draw subtle 8pt mono left-aligned.
                let color = hasProblem ? WaveformTheme.statusUnavailable : labelColor
                let attr = NSAttributedString(string: short, attributes: [.font: font, .foregroundColor: color])
                let sz = attr.size()
                attr.draw(at: NSPoint(x: rect.minX + 4, y: rect.midY - sz.height / 2))
            }
        }
    }

    private func drawScrollbars() {
        guard showsOverlay else { return }
        func drawBar(box: LaneLayoutBox, laneIndex: Int?) {
            guard box.scrolls else { return }
            let barX = bounds.maxX - 5
            let trackRect = NSRect(
                x: barX, y: audioRect.maxY - box.top - box.height,
                width: RowStackLayout.scrollbarVisualWidth, height: box.height
            )
            WaveformTheme.scrollbarTrack.setFill()
            NSBezierPath(roundedRect: trackRect, xRadius: 1.5, yRadius: 1.5).fill()
            let thumb = box.thumb()
            let thumbH = thumb.upperBound - thumb.lowerBound
            let thumbRect = NSRect(
                x: barX, y: audioRect.maxY - thumb.upperBound,
                width: RowStackLayout.scrollbarVisualWidth, height: thumbH
            )
            let isActive = (scrollDrag != nil && scrollDrag?.laneIndex == laneIndex)
                || (pointer.map { isOverScrollbar(point: $0) } ?? false)
            look.effectiveScrollThumb(active: isActive).nsColor.setFill()
            NSBezierPath(roundedRect: thumbRect, xRadius: 1.5, yRadius: 1.5).fill()
        }
        if let col = columnBox {
            drawBar(box: col, laneIndex: nil)
        } else {
            for (i, box) in laneBoxes.enumerated() { drawBar(box: box, laneIndex: i) }
        }
    }

    private func updateDisplayPolicy() {
        displayPolicy = readRequest().policy(frames: visibleFrames, rate: sampleRate)
        var policy = displayPolicy
        let counts = bars.withUnsafeBufferPointer { input in
            bt_waveform_timeline_guides(visibleFrames.lowerBound, visibleFrames.upperBound,
                sampleRate, secondsInterval ?? 0, input.baseAddress, input.count,
                showsBars ? 1 : 0, secondsInterval != nil ? 1 : 0, gridOpacity > 0 ? 1 : 0,
                &policy, nil, 0, nil, 0)
        }
        guideBars = Array(repeating: 0, count: counts.bars)
        guideSeconds = Array(repeating: 0, count: counts.seconds)
        bars.withUnsafeBufferPointer { input in
            guideBars.withUnsafeMutableBufferPointer { outputBars in
                guideSeconds.withUnsafeMutableBufferPointer { outputSeconds in
                    _ = bt_waveform_timeline_guides(visibleFrames.lowerBound, visibleFrames.upperBound,
                        sampleRate, secondsInterval ?? 0, input.baseAddress, input.count,
                        showsBars ? 1 : 0, secondsInterval != nil ? 1 : 0, gridOpacity > 0 ? 1 : 0,
                        &policy, outputBars.baseAddress, outputBars.count, outputSeconds.baseAddress, outputSeconds.count)
                }
            }
        }
    }

    private func drawTimeGrid() {
        guard gridOpacity > 0, !visibleFrames.isEmpty else { return }
        func draw(_ positions: [UInt64], color: RGBA8, width: Float) {
            guard !positions.isEmpty else { return }
            let path = NSBezierPath()
            path.lineWidth = CGFloat(width)
            for frame in positions where isVisible(frame) {
                let x = x(for: frame)
                path.move(to: NSPoint(x: x, y: audioRect.minY))
                path.line(to: NSPoint(x: x, y: audioRect.maxY))
            }
            color.nsColor(alphaMultiplier: gridOpacity).setStroke()
            path.stroke()
        }
        draw(guideSeconds, color: look.secondLine, width: displayPolicy.seconds_stroke)
        draw(guideBars, color: look.barLine, width: displayPolicy.bar_stroke)
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        recomputeRowLayout()
        updateDisplayPolicy()
        let cornerRadius: CGFloat = showsOverlay ? look.cornerRadius : 0
        let panel = NSBezierPath(roundedRect: bounds, xRadius: cornerRadius, yRadius: cornerRadius)
        NSGraphicsContext.saveGraphicsState()
        panel.addClip()
        let rows = min(displayedRows.count, traces.count)
        let separate = separateLanes && rows > 1
        var hoverRows: [RowIdentity]?
        if showsOverlay, selection == nil, !visibleFrames.isEmpty,
           pointer != nil || mouseDownEvent != nil || dragging {
            let hoveredID = !separate ? nil
                : (mouseDownEvent != nil || dragging ? dragRow : pointer.flatMap { rowIdentity(at: $0, clampedToVisible: false) })
            let hlColor = look.effectivePreviewHighlight(accent: RGBA8(rgb: accentRGB)).nsColor
            if let hoveredID {
                let unit = RowStackLayout.rowsBetween(allRows: currentSpecs(), anchor: hoveredID, cursor: hoveredID)
                hoverRows = unit
                if let rect = rectForRows(unit) {
                    hlColor.setFill()
                    NSBezierPath(roundedRect: rect, xRadius: 4, yRadius: 4).fill()
                }
            } else {
                hlColor.setFill()
                NSBezierPath(roundedRect: audioRect, xRadius: 4, yRadius: 4).fill()
            }
        }

        drawTimeGrid()
        let rowPaths = tracePaths()
        let selectedArea = selection.flatMap(waveformRect(for:)).flatMap { rect -> NSRect? in
            guard let rowBounds = selectedRowsRect() else { return nil }
            let area = rect.intersection(rowBounds)
            return area.isEmpty ? nil : area
        }
        if let selectedRect = selectedArea {
            let outside = NSBezierPath(rect: bounds)
            outside.append(NSBezierPath(rect: selectedRect))
            outside.windingRule = .evenOdd
            NSGraphicsContext.saveGraphicsState()
            outside.addClip()
            paintTraces(rowPaths, selected: false, dimFrequency: true)
            NSGraphicsContext.restoreGraphicsState()

            NSGraphicsContext.saveGraphicsState()
            NSBezierPath(rect: selectedRect).addClip()
            paintTraces(rowPaths, selected: true)
            NSGraphicsContext.restoreGraphicsState()
        } else {
            paintTraces(rowPaths, selected: false)
        }
        if let selection = selectionForSaving() { drawFades(selection) }
        drawRowDecorations(separate: separate)
        drawRowLabels()
        drawScrollbars()
        drawOverlayChips()

        var drawnFlagRects: [NSRect] = []
        if let marker, marker != selection {
            let mTop = (!markerRows.isEmpty ? rectForRows(markerRows)?.maxY : audioRect.maxY)
            if let mTop {
                if let r = drawFlag(marker.lowerBound, pointsRight: true, top: mTop - 2) { drawnFlagRects.append(r) }
                if let r = drawFlag(marker.upperBound, pointsRight: false, top: mTop - 2) { drawnFlagRects.append(r) }
            }
        }
        if let selection, !selection.isEmpty, let sTop = selectedRowsRect()?.maxY {
            if let r = drawFlag(selection.lowerBound, pointsRight: true, top: sTop - 2) { drawnFlagRects.append(r) }
            if let r = drawFlag(selection.upperBound, pointsRight: false, top: sTop - 2) { drawnFlagRects.append(r) }
        }
        drawPlayhead()
        drawLengthLabel(flagRects: drawnFlagRects, hoverRows: hoverRows)
        drawEmptyHint()
        drawCompactStatus()
        if showsOutline && outlineWidth > 0 {
            let innerRect = bounds.insetBy(dx: outlineWidth, dy: outlineWidth)
            let innerRadius = max(0, cornerRadius - outlineWidth)
            let border = NSBezierPath()
            border.append(panel)
            if innerRect.width > 0 && innerRect.height > 0 {
                border.append(NSBezierPath(roundedRect: innerRect, xRadius: innerRadius, yRadius: innerRadius))
                border.windingRule = .evenOdd
            }
            outlineColor.setFill()
            border.fill()
        }
        NSGraphicsContext.restoreGraphicsState()
    }
}

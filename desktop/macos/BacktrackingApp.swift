import AppKit
import Carbon
import CoreGraphics
import ServiceManagement

private let timeChoices: [UInt32] = [10, 20, 30, 60, 180, 300, 600]
private let autoResumeChoices: [UInt32] = [0, 5, 10]
private let autoResumePreferenceKey = "autoResumeSeconds"
private let transparencyChoices: [UInt32] = [0, 25, 50, 75, 100]
private let exportSampleRates: [UInt32] = [44_100, 48_000, 96_000]
private let exportBitDepths: [UInt32] = [16, 24, 32]
private let frameAutosaveName: NSWindow.FrameAutosaveName = "Backtracking.Panel"
private let receiverWindowArgument = "--new-capture-window"
private let fullPanelMinimum = NSSize(width: 560, height: 100)
private let waveformOnlyPanelMinimum = NSSize(width: 240, height: 48)
private let panelVerticalInset: CGFloat = 8
private let waveformFrameInset: CGFloat = 1
private let secondsLineChoices: [UInt32] = [1, 5, 10]
private let fullWaveformMinimumHeight: CGFloat = 51
private let defaultAccentRGB: UInt32 = 0x52E3D1

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

@main
enum BacktrackingApp {
    @MainActor
    static func main() {
        if StartupUpdater.runHelperIfRequested() { return }
        let app = NSApplication.shared
        let delegate = BacktrackingController()
        app.delegate = delegate
        app.setActivationPolicy(.accessory)
        app.run()
        withExtendedLifetime(delegate) {}
    }
}

private final class SnapshotShortcut {
    private var port: CFMachPort?
    private var source: CFRunLoopSource?
    private let action: () -> Void

    init(action: @escaping () -> Void) { self.action = action }

    var isListening: Bool { port.map { CGEvent.tapIsEnabled(tap: $0) } ?? false }

    func start() {
        guard !isListening, CGPreflightListenEventAccess() else { return }
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
        guard type == .keyDown, event.getIntegerValueField(.keyboardEventAutorepeat) == 0,
              event.flags.intersection([.maskSecondaryFn, .maskShift, .maskCommand, .maskControl, .maskAlternate])
                == .maskSecondaryFn else { return }
        let keyboardType = Int16(truncatingIfNeeded: event.getIntegerValueField(.keyboardEventKeyboardType))
        // macOS swaps the virtual key codes for the top-left and section keys on ISO keyboards.
        let key = KBGetLayoutType(keyboardType) == UInt32(kKeyboardISO) ? kVK_ISO_Section : kVK_ANSI_Grave
        guard event.getIntegerValueField(.keyboardEventKeycode) == Int64(key) else { return }
        DispatchQueue.main.async { [weak self] in self?.action() }
    }

    deinit { stop() }
}

private class KeyPanel: NSPanel {
    override var canBecomeKey: Bool { true }
}

private final class OptionsDocumentView: NSView {
    override var isFlipped: Bool { true }
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
    private var choiceItems: [NSMenuItem] = []

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
        table.setAccessibilityLabel("選擇設定項目")
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
    var onOptions: (() -> Void)?
    weak var waveform: WaveformView?
    private var move: MoveGesture?
    private var resize: ResizeGesture?
    private var chromeCursorActive = false

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
            guard let image = NSImage(systemSymbolName: symbol, accessibilityDescription: "調整視窗大小") else {
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

    override func sendEvent(_ event: NSEvent) {
        switch event.type {
        case .keyDown:
            // Hardware codes 51 and 53 are Backspace and Escape on macOS.
            if !event.modifierFlags.contains(.command), event.keyCode == 51 || event.keyCode == 53 {
                if waveform?.cancelSelection() == true { return }
                if event.keyCode == 53, waveform?.cancelPendingDrag() == true { return }
            }
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
            if !controlSelect, let onOptions {
                onOptions()
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
        if frame != proposed { setFrame(proposed, display: true) }
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

private struct WaveformReadRequest: Equatable, Sendable {
    var view: Range<UInt64>?
    var width: Float
    var height: Float
    var stereo: Bool
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

    static func read(core: OpaquePointer, laneCount: Int, request: WaveformReadRequest,
                     unchangedRevision: UInt64?) -> WaveformFrame? {
        let revision = bt_tracks_revision(core)
        if revision == unchangedRevision { return nil }
        let rate = bt_tracks_sample_rate(core)
        let first = bt_tracks_first_frame(core)
        let retained = first..<max(first, bt_tracks_end_frame(core))
        let visible = fittedWaveformRange(request.view, in: retained)
        let bins = Int(request.policy(frames: visible, rate: rate).bins)
        let layout: [(UInt32, UInt32)] = laneCount == 2 ? [(0, 0), (1, 0)]
            : (request.stereo && bt_tracks_channels(core, 0) == 2 ? [(0, 1), (0, 2)] : [(0, 0)])
        let traces = layout.map { lane, channel -> WaveformTrace in
            var trace = WaveformTrace(bins: bins)
            trace.read { minimum, maximum, count in
                _ = bt_tracks_waveform_range(core, lane, channel, visible.lowerBound, visible.upperBound,
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
private final class WAVExport: @unchecked Sendable {
    private static let releaseQueue = DispatchQueue(label: "Backtracking.material-release", qos: .utility)
    private let snapshots: [OpaquePointer]
    private let routes: [UInt32]
    let span: Range<UInt64>
    private let sampleRate: UInt32
    private let channels: [UInt32]
    private let bars: [UInt64]
    private let cacheLock = NSLock()
    private struct PeakKey: Equatable {
        let visible: Range<UInt64>
        let bins: Int
        let stereo: Bool
    }
    private var cachedPeaks: (PeakKey, WaveformFrame)?

    init?(core: OpaquePointer, routes: [CaptureRoute], range: Range<UInt64>? = nil) {
        guard (1...2).contains(routes.count) else { return nil }
        func snapshot(_ lane: UInt32) -> OpaquePointer? {
            if let range { return bt_tracks_snapshot_range(core, lane, range.lowerBound, range.upperBound) }
            return bt_tracks_snapshot(core, lane)
        }
        guard let first = snapshot(0) else { return nil }
        let count = bt_snapshot_frame_count(first)
        guard count > 0 else {
            bt_snapshot_free(first)
            return nil
        }
        let start = bt_snapshot_first_frame(first)
        let capturedSpan = start..<(start + count)
        span = capturedSpan
        sampleRate = bt_tracks_sample_rate(core)
        self.routes = routes.map(\.wavRouteCode)
        let captured: [OpaquePointer]
        if routes.count == 2 {
            guard let second = snapshot(1) else {
                bt_snapshot_free(first)
                return nil
            }
            captured = [first, second]
        } else {
            captured = [first]
        }
        snapshots = captured
        channels = captured.map { bt_snapshot_channels($0) }
        let markerCount = bt_tracks_copy_bar_markers(core, capturedSpan.lowerBound, capturedSpan.upperBound, nil, 0)
        var markers = Array<UInt64>(repeating: 0, count: markerCount)
        let actual = markers.withUnsafeMutableBufferPointer {
            bt_tracks_copy_bar_markers(core, capturedSpan.lowerBound, capturedSpan.upperBound, $0.baseAddress, $0.count)
        }
        if actual < markerCount { markers.removeLast(markerCount - actual) }
        bars = markers
    }

    deinit {
        let addresses = snapshots.map { UInt(bitPattern: $0) }
        Self.releaseQueue.async {
            for address in addresses { bt_snapshot_free(OpaquePointer(bitPattern: address)) }
        }
    }

    func render(_ request: WaveformReadRequest) -> WaveformFrame {
        let visible = fittedWaveformRange(request.view, in: span)
        let bins = Int(request.policy(frames: visible, rate: sampleRate).bins)
        let key = PeakKey(visible: visible, bins: bins, stereo: request.stereo)
        cacheLock.lock()
        let cached = cachedPeaks
        cacheLock.unlock()
        if let cached, cached.0 == key { return cached.1 }
        let layout: [(Int, UInt32)] = snapshots.count == 2 ? [(0, 0), (1, 0)]
            : (request.stereo && channels[0] == 2 ? [(0, 1), (0, 2)] : [(0, 0)])
        let traces = layout.map { lane, channel -> WaveformTrace in
            var trace = WaveformTrace(bins: bins)
            trace.read { minimum, maximum, count in
                _ = bt_snapshot_waveform_range(snapshots[lane], channel, visible.lowerBound,
                    visible.upperBound, minimum, maximum, count)
            }
            return trace
        }
        let frame = WaveformFrame(revision: 0, sampleRate: sampleRate, retained: span, visible: visible,
            traces: traces, bars: bars)
        cacheLock.lock()
        cachedPeaks = (key, frame)
        cacheLock.unlock()
        return frame
    }

    func write(
        range: Range<UInt64>, directory: String, sampleRate: UInt32, bitsPerSample: UInt32, forceMono: Bool
    ) -> [URL] {
        let mono: UInt32 = forceMono ? 1 : 0
        return directory.withCString { destination -> [URL] in
            if snapshots.count == 2 {
                var a: UnsafeMutablePointer<CChar>?
                var b: UnsafeMutablePointer<CChar>?
                guard bt_snapshots_export_pair_routed_frames_with_profile(
                    snapshots[0], snapshots[1], range.lowerBound, range.upperBound, destination,
                    sampleRate, bitsPerSample, routes[0], routes[1], mono, &a, &b
                ) == 1, let pathA = a, let pathB = b else { return [] }
                defer {
                    bt_string_free(pathA)
                    bt_string_free(pathB)
                }
                return [
                    URL(fileURLWithPath: String(cString: pathA)),
                    URL(fileURLWithPath: String(cString: pathB))
                ]
            }
            guard let path = bt_snapshot_export_routed_frames_with_profile(
                snapshots[0], range.lowerBound, range.upperBound, destination,
                sampleRate, bitsPerSample, routes[0], mono
            ) else { return [] }
            defer { bt_string_free(path) }
            return [URL(fileURLWithPath: String(cString: path))]
        }
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
            case .unavailable: "錄音裝置未連線"
            case .paused: "已暫停"
            case .waiting: "等待聲音"
            case .recording: "錄音中"
            }
        }
    }

    private var state: State?
    private var pulseStart: TimeInterval = 0
    private var bright = true

    override var intrinsicContentSize: NSSize { NSSize(width: 14, height: 14) }

    func update(_ state: State, now: TimeInterval, detail: String) {
        if toolTip != detail {
            toolTip = detail
            setAccessibilityLabel(detail)
        }
        let changed = state != self.state
        if changed { pulseStart = now }
        let nextBright = state != .recording || Int((now - pulseStart) / 0.5).isMultiple(of: 2)
        guard changed || bright != nextBright else { return }
        self.state = state
        bright = nextBright
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard let state else { return }
        let color: NSColor = switch state {
        case .unavailable: .systemOrange
        case .paused: .quaternaryLabelColor
        case .waiting: .tertiaryLabelColor
        case .recording: .systemRed.withAlphaComponent(bright ? 1 : 0.27)
        }
        color.setFill()
        NSBezierPath(ovalIn: NSRect(x: bounds.midX - 4, y: bounds.midY - 4, width: 8, height: 8)).fill()
    }
}

private final class CaptureBackground: NSView {
    weak var waveform: NSView?
    var backgroundOpacity: CGFloat = 1 { didSet { needsDisplay = true } }
    var showsOutline = false { didSet { needsDisplay = true } }
    var outlineColor = NSColor.white { didSet { needsDisplay = true } }
    var outlineFollowsOpacity = true { didSet { needsDisplay = true } }

    override func layout() {
        super.layout()
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        NSGraphicsContext.current?.cgContext.clear(dirtyRect)
        let waveRect = waveform.map { convert($0.bounds, from: $0) }
        let waveShape = waveRect.map { NSBezierPath(roundedRect: $0, xRadius: 6, yRadius: 6) }
        if backgroundOpacity > 0 {
            let fill = NSBezierPath(rect: bounds)
            if let waveShape {
                fill.append(waveShape)
                fill.windingRule = .evenOdd
            }
            NSColor.windowBackgroundColor.withAlphaComponent(backgroundOpacity).setFill()
            fill.fill()
            if let waveShape {
                NSColor(calibratedRed: 0.07, green: 0.09, blue: 0.12, alpha: backgroundOpacity).setFill()
                waveShape.fill()
            }
        }
        if showsOutline, let waveRect, let waveShape {
            let outline = NSBezierPath(roundedRect: waveRect.insetBy(dx: -1, dy: -1), xRadius: 7, yRadius: 7)
            outline.append(waveShape)
            outline.windingRule = .evenOdd
            outlineColor.withAlphaComponent(outlineFollowsOpacity ? backgroundOpacity : 1).setFill()
            outline.fill()
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
        case .dbfs: "峰值電平"
        case .vu: "VU 電平"
        case .lufs: "瞬時響度"
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

private enum ExportFolder: Int {
    case output
    case saved

    var key: String { self == .output ? "outputDirectory" : "savedDirectory" }
    var component: String { self == .output ? "output" : "saved" }
    var label: String { self == .output ? "預設資料夾" : "保存資料夾" }
}

@MainActor
private final class BacktrackingController: NSObject, NSApplicationDelegate, NSWindowDelegate, NSMenuDelegate {
    private let independentWindow = CommandLine.arguments.contains(receiverWindowArgument)
    private lazy var defaults: UserDefaults = {
        guard independentWindow else { return .standard }
        let name = "com.backtracking.audio.capture.receiver.\(UUID().uuidString)"
        guard let preferences = UserDefaults(suiteName: name) else { return .standard }
        receiverPreferencesName = name
        preferences.register(defaults: UserDefaults.standard.dictionaryRepresentation())
        return preferences
    }()
    private var receiverPreferencesName: String?
    private let startupUpdater = StartupUpdater()
    private lazy var snapshotShortcut = SnapshotShortcut { [weak self] in self?.dispatchSnapshotShortcut() }
    private var lastShortcutCheck = 0.0
    private var snapshotTargetID: String?
    private weak var snapshotTargetPopup: NSPopUpButton?
    private weak var shortcutPermissionButton: NSButton?
    private var optionTabs: NSTabView?
    private var optionCategoryButtons: [NSButton] = []
    private var optionCategory = 0
    private let worker = DispatchQueue(label: "backtracking.wav-export", qos: .userInitiated)
    private let pendingProvisionalWAVs = PendingProvisionalWAVs()
    private var tap: SystemAudioTap?
    private var panel: CapturePanel?
    private var optionsPanel: KeyPanel?
    private var statusItem: NSStatusItem?
    private var background: CaptureBackground?
    private var waveform: WaveformView?
    private var timer: Timer?
    private let captureButton = NSButton(title: "", target: nil, action: nil)
    private let captureLamp = CaptureLamp(frame: .zero)
    private let clearButton = NSButton(title: "清除目前錄音", target: nil, action: nil)
    private let pinButton = NSButton(title: "置頂", target: nil, action: nil)
    private let meterButton = NSButton(title: "— dBFS", target: nil, action: nil)
    private let meterBButton = NSButton(title: "B — dBFS", target: nil, action: nil)
    private var sourcePopups: [NSPopUpButton] = []
    private var routePopups: [NSPopUpButton] = []
    private var dualSourceButton: NSButton?
    private var waveformModePopup: NSPopUpButton?
    private let statusLabel = NSTextField(labelWithString: "")
    private var optionsCaptureButton: NSButton?
    private var optionsClearButton: NSButton?
    private var optionsSaveButton: NSButton?
    private var optionsCancelSelectionButton: NSButton?
    private var optionsPinButton: NSButton?
    private var optionsWaveformButton: NSButton?
    private var optionsLoginButton: NSButton?
    private var optionsVisibilityButton: NSButton?
    private var optionsMeterPopup: NSPopUpButton?
    private var optionsMeterBPopup: NSPopUpButton?
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
    private var waveformBottom: NSLayoutConstraint?
    private var fullPanelSize: NSSize?
    private var history: UInt32 = 20
    private var historyChangePending = false
    private var retainSilence = false
    private var idleSeconds: UInt32 = 10
    private var transparency: UInt32 = 0
    private var accentRGB = defaultAccentRGB
    private var showsBars = false
    private var showsSeconds = false
    private var secondsInterval: UInt32 = 5
    private var showsOutline = false
    private var customOutline = false
    private var outlineRGB = defaultAccentRGB
    private var outlineFollowsOpacity = true
    private var meterMode: MeterMode = .dbfs
    private var meterModeB: MeterMode = .dbfs
    private var laneA = CaptureLane(source: .system)
    private var laneB = CaptureLane(source: .microphone(""))
    private var dualSource = false
    private var stereoWaveform = false
    private var microphonePromptPending = false
    private var lastSourceCheck = 0.0

    private var captureLanes: [CaptureLane] { dualSource ? [laneA, laneB] : [laneA] }
    private var exportSampleRate: UInt32 = 48_000
    private var exportBitsPerSample: UInt32 = 32
    private var forceMonoExport = false
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
        showsBars = defaults.bool(forKey: "showsBars")
        showsSeconds = defaults.bool(forKey: "showsSeconds")
        if let saved = unsignedSetting("secondsInterval"), secondsLineChoices.contains(saved) {
            secondsInterval = saved
        }
        showsOutline = defaults.bool(forKey: "showsWaveformOutline")
        customOutline = defaults.bool(forKey: "customWaveformOutline")
        if let saved = unsignedSetting("waveformOutlineRGB"), saved <= 0xFFFFFF { outlineRGB = saved }
        outlineFollowsOpacity = (defaults.object(forKey: "waveformOutlineFollowsOpacity") as? Bool) ?? true
        snapshotTargetID = defaults.string(forKey: "snapshotTargetID")
        if let width = defaults.object(forKey: "fullPanelWidth") as? NSNumber,
           let height = defaults.object(forKey: "fullPanelHeight") as? NSNumber,
           width.doubleValue.isFinite, height.doubleValue.isFinite,
           CGFloat(width.doubleValue) >= fullPanelMinimum.width,
           CGFloat(height.doubleValue) >= fullPanelMinimum.height {
            fullPanelSize = NSSize(width: CGFloat(width.doubleValue), height: CGFloat(height.doubleValue))
        }
        if let saved = unsignedSetting("meterMode"), let mode = MeterMode(rawValue: saved) {
            meterMode = mode
        }
        if let saved = unsignedSetting("exportSampleRate"), exportSampleRates.contains(saved) {
            exportSampleRate = saved
        }
        if let saved = unsignedSetting("exportBitsPerSample"), exportBitDepths.contains(saved) {
            exportBitsPerSample = saved
        }
        forceMonoExport = defaults.bool(forKey: "forceMonoExport")
        alwaysOnTop = (defaults.object(forKey: "alwaysOnTop") as? Bool) ?? true
        waveformOnly = (defaults.object(forKey: "waveformOnly") as? Bool) ?? false
        if independentWindow { beginCaptureAfterUpdate() }
        else { startupUpdater.checkAtLaunch { [weak self] in self?.beginCaptureAfterUpdate() } }
    }

    private func beginCaptureAfterUpdate() {
        loadCapturePreferences()
        configureMenu()
        configurePanel()
        if !independentWindow {
            configureStatusItem()
            snapshotShortcut.start()
        }
        resume()
        let timer = Timer(timeInterval: 1.0 / 15.0, target: self, selector: #selector(tick(_:)), userInfo: nil, repeats: true)
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    private func loadCapturePreferences() {
        if let saved = defaults.string(forKey: "captureSourceA"), let source = CaptureSource(key: saved) {
            laneA.source = source
        }
        if let saved = defaults.string(forKey: "captureSourceB"), let source = CaptureSource(key: saved) {
            laneB.source = source
        }
        if let raw = unsignedSetting("captureRouteA"), let route = CaptureRoute(rawValue: raw) { laneA.route = route }
        if let raw = unsignedSetting("captureRouteB"), let route = CaptureRoute(rawValue: raw) { laneB.route = route }
        if let raw = unsignedSetting("meterModeB"), let mode = MeterMode(rawValue: raw) { meterModeB = mode }
        dualSource = defaults.bool(forKey: "dualCaptureSource")
        stereoWaveform = defaults.bool(forKey: "stereoWaveform")
    }

    private func unsignedSetting(_ key: String) -> UInt32? {
        guard let number = defaults.object(forKey: key) as? NSNumber else { return nil }
        return UInt32(exactly: number.doubleValue)
    }

    private func directory(for folder: ExportFolder) -> String {
        if let saved = defaults.string(forKey: folder.key), saved.hasPrefix("/"), !saved.utf8.contains(0) {
            return saved
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Backtracking/\(folder.component)", isDirectory: true).path
    }

    func applicationWillTerminate(_ notification: Notification) {
        if !independentWindow { panel?.saveFrame(usingName: frameAutosaveName) }
        timer?.invalidate()
        timer = nil
        if !independentWindow { snapshotShortcut.stop() }
        if let receiverPreferencesName { defaults.removePersistentDomain(forName: receiverPreferencesName) }
        tap = nil
        waveform?.discardPreparedFile()
        // Worker jobs dispatch their completion asynchronously and never wait for this thread.
        worker.sync {
            for url in pendingProvisionalWAVs.urls { try? FileManager.default.removeItem(at: url) }
            pendingProvisionalWAVs.urls.removeAll()
        }
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
        if let source = sourcePopups.first(where: { $0.menu === menu }) {
            refreshSourcePopup(source, choices: availableCaptureSources(excluding: tap?.privatePlaybackDevices ?? []))
            return
        }
        guard menu === statusItem?.menu else { return }
        menu.removeAllItems()
        let title = panel?.isVisible == true ? "隱藏波形" : "顯示波形"
        let visibility = NSMenuItem(title: title, action: #selector(togglePanelVisibility), keyEquivalent: "")
        visibility.target = self
        menu.addItem(visibility)
        menu.addItem(captureMenuItem())
        let options = NSMenuItem(title: "設定…", action: #selector(showOptions), keyEquivalent: "")
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

    private func hidePanel(_ panel: NSWindow) {
        if !independentWindow { panel.saveFrame(usingName: frameAutosaveName) }
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
        let window = CapturePanel(
            contentRect: NSRect(x: 0, y: 0, width: 590, height: 116),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered, defer: false
        )
        window.title = independentWindow ? "Backtracking 接收視窗" : "Backtracking"
        window.contentMinSize = fullPanelMinimum
        // CapturePanel handles dragging and resizing without AppKit's native window gestures.
        window.isMovable = false
        window.delegate = self
        window.becomesKeyOnlyIfNeeded = true
        window.acceptsMouseMovedEvents = true
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .fullScreenDisallowsTiling]
        window.onClick = { [weak self] in self?.registerClick() }
        window.onOptions = { [weak self] in self?.showOptions() }

        let waveform = WaveformView(frame: .zero)
        waveform.controller = self
        waveform.accentRGB = accentRGB
        waveform.stereo = stereoWaveform
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
        setIcon("trash", label: "清除目前錄音", on: clearButton)
        clearButton.toolTip = "清除錄音"
        for (lane, button) in [meterButton, meterBButton].enumerated() {
            button.tag = lane
            button.target = self
            button.action = #selector(showMeterOptions(_:))
            button.bezelStyle = .rounded
            button.font = .monospacedDigitSystemFont(ofSize: 11, weight: .medium)
            button.widthAnchor.constraint(equalToConstant: 100).isActive = true
        }
        meterBButton.isHidden = !dualSource

        pinButton.target = self
        pinButton.action = #selector(pinChanged(_:))
        pinButton.setButtonType(.pushOnPushOff)
        pinButton.bezelStyle = .rounded
        let settingsButton = NSButton(title: "設定", target: self, action: #selector(showOptions))
        settingsButton.bezelStyle = .rounded
        setIcon("gearshape", label: "設定", on: settingsButton)
        let spacer = NSView(frame: .zero)
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let controls = NSStackView(views: [captureButton, captureLamp, clearButton, statusLabel, spacer, pinButton, meterButton, meterBButton, settingsButton])
        controls.orientation = .horizontal
        controls.spacing = 6
        controls.alignment = .centerY

        statusLabel.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        statusLabel.lineBreakMode = .byTruncatingTail
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        statusLabel.isHidden = true
        let content = CaptureBackground(frame: window.contentView?.bounds ?? .zero)
        content.waveform = waveform
        window.contentView = content
        background = content
        window.trackChrome(in: content)
        content.addSubview(waveform)
        content.addSubview(controls)
        waveform.translatesAutoresizingMaskIntoConstraints = false
        controls.translatesAutoresizingMaskIntoConstraints = false
        let toolbarTrailing = controls.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -10)
        self.toolbarTrailing = toolbarTrailing
        let waveformLeading = waveform.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 10)
        let waveformTrailing = waveform.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -10)
        let waveformBottom = waveform.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -panelVerticalInset)
        self.waveformLeading = waveformLeading
        self.waveformTrailing = waveformTrailing
        self.waveformBottom = waveformBottom
        NSLayoutConstraint.activate([
            controls.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 10),
            toolbarTrailing,
            controls.topAnchor.constraint(equalTo: content.topAnchor, constant: panelVerticalInset),
            waveformLeading, waveformTrailing, waveformBottom
        ])
        toolbar = controls
        waveformTopBelowToolbar = waveform.topAnchor.constraint(equalTo: controls.bottomAnchor, constant: 6)
        waveformTopBelowToolbar?.isActive = true
        waveformTopWithoutToolbar = waveform.topAnchor.constraint(equalTo: content.topAnchor, constant: waveformFrameInset)
        panel = window
        applyPin()
        if waveformOnly { window.contentMinSize = waveformOnlyPanelMinimum }
        let restoredFrame = !independentWindow && window.setFrameUsingName(frameAutosaveName, force: true)
        if !independentWindow { _ = window.setFrameAutosaveName(frameAutosaveName) }
        applyWaveformOnly(initial: true, restoredFrame: restoredFrame)
        if !restoredFrame {
            window.center()
            if independentWindow {
                window.setFrameOrigin(NSPoint(x: window.frame.minX + 34, y: window.frame.minY - 34))
            }
        }
        showPanel(window)
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
        refreshSourceOptions()
        refreshSnapshotTargets()
        refreshShortcutPermission()
        updateOptionsVisibility()
        updateLoginOption()
        updateStatus()
        keepPanelVisible(window)
        NSApp.activate()
        window.makeKeyAndOrderFront(nil)
    }

    private func makeOptionsPanel() -> KeyPanel {
        let window = KeyPanel(
            contentRect: NSRect(x: 0, y: 0, width: 760, height: 560),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false
        )
        window.title = "Backtracking 設定"
        window.contentMinSize = NSSize(width: 680, height: 460)
        window.isReleasedWhenClosed = false
        window.isFloatingPanel = true
        window.level = .floating
        window.hidesOnDeactivate = false

        let content = NSView(frame: .zero)
        let tabs = NSTabView(frame: .zero)
        tabs.tabViewType = .noTabsNoBorder
        var pages = [makeActionTab(), makeCaptureTab(), makeSourceTab(), makeAppearanceTab(), makeExportTab()]
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
            return button
        }
        tabs.selectTabViewItem(at: optionCategory)
        let sidebar = NSStackView(views: optionCategoryButtons)
        sidebar.orientation = .vertical
        sidebar.alignment = .leading
        sidebar.spacing = 6
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
        let close = NSButton(title: "關閉", target: window, action: #selector(NSWindow.performClose(_:)))
        close.bezelStyle = .rounded
        close.font = .systemFont(ofSize: 15)
        close.keyEquivalent = "\u{1b}"
        for view in [sidebar as NSView, separator, tabs, footerSeparator, notice, close] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }
        NSLayoutConstraint.activate([
            sidebar.topAnchor.constraint(equalTo: content.topAnchor, constant: 22),
            sidebar.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 14),
            sidebar.widthAnchor.constraint(equalToConstant: 136),
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

    @objc private func selectOptionCategory(_ sender: NSButton) {
        optionCategory = sender.tag
        optionTabs?.selectTabViewItem(at: optionCategory)
        for button in optionCategoryButtons { button.state = button === sender ? .on : .off }
    }

    private func makeActionTab() -> NSTabViewItem {
        let capture = NSButton(title: captureActionLabel, target: self, action: #selector(toggleCapture(_:)))
        let save = NSButton(title: "保存快照", target: self, action: #selector(saveCurrentSnapshot))
        let clear = NSButton(title: "清除目前錄音", target: self, action: #selector(clearCapture))
        let cancel = NSButton(title: "取消選取", target: self, action: #selector(cancelWaveformSelection))
        let visibility = NSButton(title: "隱藏波形", target: self, action: #selector(togglePanelVisibility))
        let quit = NSButton(title: "結束 Backtracking", target: NSApp, action: #selector(NSApplication.terminate(_:)))
        let login = NSButton(checkboxWithTitle: "登入時啟動", target: self, action: #selector(toggleLaunchAtLogin))
        optionsLoginButton = login
        optionsCaptureButton = capture
        optionsSaveButton = save
        optionsClearButton = clear
        optionsCancelSelectionButton = cancel
        if independentWindow {
            quit.title = "關閉接收視窗"
            return optionsTab("動作與快捷鍵", rows: [
                optionGroup("錄音動作", rows: [
                    capture, save, clear, cancel
                ]),
                optionGroup("接收視窗", rows: [quit])
            ])
        }
        optionsVisibilityButton = visibility
        let target = OptionsPopUpButton(frame: .zero, pullsDown: false)
        target.target = self
        target.action = #selector(snapshotTargetChanged(_:))
        target.autoenablesItems = false
        target.menu?.delegate = self
        prepareOptionPopup(target)
        snapshotTargetPopup = target
        refreshSnapshotTargets()
        let shortcut = NSTextField(labelWithString: "Fn／Globe + 左上角反引號鍵")
        let permission = NSButton(title: "允許輸入監控", target: self, action: #selector(requestShortcutPermission))
        shortcutPermissionButton = permission
        refreshShortcutPermission()
        return optionsTab("動作與快捷鍵", rows: [
            optionGroup("錄音動作", rows: [
                capture, save, clear, cancel
            ]),
            optionGroup("全域快捷鍵", rows: [
                labeledOption("按鍵組合", shortcut),
                labeledOption("快照目標", target),
                permission
            ]),
            optionGroup("應用程式", rows: [
                login, visibility, quit
            ])
        ])
    }

    private func refreshSnapshotTargets() {
        guard let popup = snapshotTargetPopup else { return }
        popup.removeAllItems()
        popup.addItem(withTitle: "桌面版")
        var selectedIndex = 0
        var usedTitles = Set<String>()
        for target in availableSnapshotTargets() {
            let title = usedTitles.insert(target.name).inserted ? target.name : "\(target.name) · \(target.id)"
            popup.addItem(withTitle: title)
            popup.lastItem?.representedObject = target.id
            popup.lastItem?.toolTip = title
            if snapshotTargetID == target.id { selectedIndex = popup.numberOfItems - 1 }
        }
        if let snapshotTargetID, selectedIndex == 0 {
            popup.addItem(withTitle: "上次選擇的插件（目前不可用）")
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
            if !requestSenderSnapshot(id) { setNotice("目前找不到選定的快照目標") }
        } else {
            saveCurrentSnapshot()
        }
    }

    private func refreshShortcutPermission() {
        let authorized = CGPreflightListenEventAccess()
        shortcutPermissionButton?.isHidden = snapshotShortcut.isListening
        shortcutPermissionButton?.title = authorized ? "重新連線全域快捷鍵" : "允許輸入監控"
    }

    @objc private func requestShortcutPermission() {
        if !CGPreflightListenEventAccess() { _ = CGRequestListenEventAccess() }
        snapshotShortcut.start()
        refreshShortcutPermission()
    }

    private func makeCaptureTab() -> NSTabViewItem {
        let quietSections = NSButton(
            checkboxWithTitle: "保留安靜片段", target: self, action: #selector(retainSilenceChanged(_:))
        )
        quietSections.state = retainSilence ? .on : .off
        return optionsTab("擷取", rows: [
            optionGroup("錄音緩衝", rows: [
                optionChoice(
                    "保留時間", choices: timeChoices, selected: history, action: #selector(historyChanged(_:))
                ) { $0 > 60 ? "\($0 / 60) 分鐘" : "\($0) 秒" },
                optionChoice(
                    "自動續錄", choices: autoResumeChoices, selected: idleSeconds, action: #selector(idleChanged(_:))
                ) { "\($0) 秒" },
                quietSections
            ])
        ])
    }

    private func makeExportTab() -> NSTabViewItem {
        let mono = NSButton(
            checkboxWithTitle: "全部轉成單聲道", target: self, action: #selector(exportMonoChanged(_:))
        )
        mono.state = forceMonoExport ? .on : .off
        return optionsTab("匯出", rows: [
            optionGroup("音訊格式", rows: [
                optionChoice(
                    "WAV 取樣率", choices: exportSampleRates, selected: exportSampleRate,
                    action: #selector(exportSampleRateChanged(_:))
                ) { $0 == 44_100 ? "44.1 kHz" : "\($0 / 1_000) kHz" },
                optionChoice(
                    "WAV 編碼", choices: exportBitDepths, selected: exportBitsPerSample,
                    action: #selector(exportEncodingChanged(_:))
                ) { $0 == 32 ? "32-bit float" : "\($0)-bit PCM" },
                mono
            ]),
            optionGroup("儲存位置", rows: [
                folderOptions(.output), folderOptions(.saved)
            ])
        ])
    }

    private func makeSourceTab() -> NSTabViewItem {
        let dual = NSButton(checkboxWithTitle: "同時錄製 A 與 B", target: self, action: #selector(dualSourceChanged(_:)))
        dualSourceButton = dual
        var rows: [NSView] = [
            optionGroup("擷取模式", rows: [dual])
        ]
        for (index, configuration) in [laneA, laneB].enumerated() {
            let label = index == 0 ? "A" : "B"
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
            rows.append(optionGroup(
                "來源 \(label)", rows: [
                    labeledOption("\(label) 擷取來源", source), labeledOption("\(label) 聲道路由", route)
                ]
            ))
        }
        let stereo = choiceControl(
            [0, 1], selected: stereoWaveform ? 1 : 0, action: #selector(waveformModeChanged(_:))
        ) { $0 == 0 ? "單聲道合併" : "雙聲道 L／R" }
        waveformModePopup = stereo
        let receiver = NSButton(title: "開啟獨立接收視窗", target: self, action: #selector(openReceiverWindow))
        rows.append(optionGroup("顯示與連線", rows: [
            labeledOption("單一來源顯示", stereo), receiver
        ]))
        return optionsTab("來源與連線", rows: rows)
    }

    @objc private func openReceiverWindow() {
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.arguments = [receiverWindowArgument]
        configuration.createsNewApplicationInstance = true
        configuration.activates = false
        configuration.addsToRecentItems = false
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration) {
            [weak self] _, error in
            if let error {
                DispatchQueue.main.async { self?.setNotice(error.localizedDescription) }
            }
        }
    }

    private func refreshSourceOptions() {
        guard sourcePopups.count == 2, routePopups.count == 2 else { return }
        let choices = availableCaptureSources(excluding: tap?.privatePlaybackDevices ?? [])
        for (index, lane) in [laneA, laneB].enumerated() {
            let source = sourcePopups[index]
            refreshSourcePopup(source, choices: choices)
            source.isEnabled = index == 0 || dualSource
            let route = routePopups[index]
            route.isEnabled = source.isEnabled
            route.selectItem(withTag: Int(lane.route.rawValue))
            for item in route.itemArray {
                guard let raw = UInt32(exactly: item.tag), let selection = CaptureRoute(rawValue: raw) else { continue }
                item.isEnabled = selection == lane.route || tap?.supports(selection, lane: index) == true
                    || (tap == nil && selection != .center)
            }
        }
        dualSourceButton?.state = dualSource ? .on : .off
        waveformModePopup?.isEnabled = !dualSource
        optionsMeterBPopup?.isEnabled = dualSource
    }

    private func refreshSourcePopup(_ popup: NSPopUpButton, choices: [(source: CaptureSource, title: String)]) {
        let configured = popup.tag == 0 ? laneA.source : laneB.source
        popup.removeAllItems()
        var selectedIndex: Int?
        var usedTitles = Set<String>()
        for choice in choices {
            let title = usedTitles.insert(choice.title).inserted ? choice.title : "\(choice.title) · \(choice.source.key)"
            popup.addItem(withTitle: title)
            popup.lastItem?.representedObject = choice.source.key
            popup.lastItem?.toolTip = title
            if configured == choice.source { selectedIndex = popup.numberOfItems - 1 }
        }
        if selectedIndex == nil {
            popup.addItem(withTitle: "\(configured.label)（目前不可用）")
            popup.lastItem?.isEnabled = false
        }
        popup.selectItem(at: selectedIndex ?? popup.numberOfItems - 1)
        popup.toolTip = popup.titleOfSelectedItem
    }

    @objc private func dualSourceChanged(_ sender: NSButton) {
        guard !editing else { refreshSourceOptions(); return }
        dualSource = sender.state == .on
        defaults.set(dualSource, forKey: "dualCaptureSource")
        reconnectConfiguredSources()
    }

    @objc private func captureSourceChanged(_ sender: NSPopUpButton) {
        guard !editing, let key = sender.selectedItem?.representedObject as? String,
              let source = CaptureSource(key: key) else { refreshSourceOptions(); return }
        var lane = sender.tag == 0 ? laneA : laneB
        guard lane.source != source else { return }
        lane.source = source
        lane.route = .merged
        storeLane(lane, index: sender.tag)
    }

    @objc private func captureRouteChanged(_ sender: NSPopUpButton) {
        guard !editing, let value = selectedValue(sender), let route = CaptureRoute(rawValue: value) else {
            refreshSourceOptions()
            return
        }
        var lane = sender.tag == 0 ? laneA : laneB
        guard route != lane.route else { return }
        lane.route = route
        storeLane(lane, index: sender.tag)
    }

    private func storeLane(_ lane: CaptureLane, index: Int) {
        let suffix = index == 0 ? "A" : "B"
        if index == 0 { laneA = lane } else { laneB = lane }
        defaults.set(lane.source.key, forKey: "captureSource\(suffix)")
        defaults.set(Int(lane.route.rawValue), forKey: "captureRoute\(suffix)")
        reconnectConfiguredSources()
    }

    private func reconnectConfiguredSources() {
        let wasRunning = !paused
        if wasRunning { pauseCapture() }
        editing = false
        tap = nil
        waveform?.resetTimeline()
        notice = nil
        captureError = nil
        registerClick()
        if wasRunning {
            resume()
        } else {
            do { tap = try SystemAudioTap(history: history, lanes: captureLanes, retainSilence: retainSilence) }
            catch { captureError = error.localizedDescription }
        }
        refreshWaveform()
        refreshSourceOptions()
        updateStatus()
    }

    @objc private func waveformModeChanged(_ sender: NSPopUpButton) {
        guard let value = selectedValue(sender) else { return }
        stereoWaveform = value == 1
        defaults.set(stereoWaveform, forKey: "stereoWaveform")
        waveform?.stereo = stereoWaveform
        refreshWaveform()
    }

    private func makeAppearanceTab() -> NSTabViewItem {
        let waveform = NSButton(checkboxWithTitle: "只顯示波形", target: self, action: #selector(toggleWaveformOnly))
        waveform.state = waveformOnly ? .on : .off
        optionsWaveformButton = waveform
        let pin = NSButton(checkboxWithTitle: "置頂", target: self, action: #selector(pinChanged(_:)))
        pin.state = alwaysOnTop ? .on : .off
        optionsPinButton = pin
        let meter = choiceControl(
            MeterMode.allCases.map(\.rawValue), selected: meterMode.rawValue, action: #selector(meterChanged(_:))
        ) { MeterMode(rawValue: $0)?.label ?? "" }
        optionsMeterPopup = meter
        let meterB = choiceControl(
            MeterMode.allCases.map(\.rawValue), selected: meterModeB.rawValue, action: #selector(meterChanged(_:))
        ) { MeterMode(rawValue: $0)?.label ?? "" }
        meterB.tag = 1
        optionsMeterBPopup = meterB
        let accent = NSButton(title: "選擇…", target: self, action: #selector(chooseAccentColor))
        let bars = NSButton(checkboxWithTitle: "小節線", target: self, action: #selector(barsChanged(_:)))
        bars.state = showsBars ? .on : .off
        let seconds = NSButton(checkboxWithTitle: "秒數線", target: self, action: #selector(secondsChanged(_:)))
        seconds.state = showsSeconds ? .on : .off
        let interval = choiceControl(
            secondsLineChoices, selected: secondsInterval, action: #selector(secondsIntervalChanged(_:))
        ) { "\($0) 秒" }
        let outline = NSButton(checkboxWithTitle: "細外框", target: self, action: #selector(outlineChanged(_:)))
        outline.state = showsOutline ? .on : .off
        let outlineFollows = NSButton(
            checkboxWithTitle: "外框跟隨背景不透明度", target: self, action: #selector(outlineOpacityChanged(_:))
        )
        outlineFollows.state = outlineFollowsOpacity ? .on : .off
        let outlineMode = choiceControl(
            [0, 1], selected: customOutline ? 1 : 0, action: #selector(outlineModeChanged(_:))
        ) { $0 == 0 ? "跟隨主題色" : "自訂色" }
        let outlineColor = NSButton(title: "選色…", target: self, action: #selector(chooseOutlineColor))
        return optionsTab("波形與外觀", rows: [
            optionGroup("視窗", rows: [
                waveform,
                pin,
                optionChoice(
                    "背景透明度", choices: transparencyChoices, selected: transparency,
                    action: #selector(transparencyChanged(_:))
                ) { "\($0)%" },
                labeledOption("主題色", accent)
            ]),
            optionGroup("電平", rows: [
                labeledOption("A 電平顯示", meter), labeledOption("B 電平顯示", meterB)
            ]),
            optionGroup("時間標記", rows: [
                bars, seconds, labeledOption("秒數線間隔", interval)
            ]),
            optionGroup("外框", rows: [
                outline,
                labeledOption("外框配色", outlineMode),
                labeledOption("自訂外框色", outlineColor),
                outlineFollows
            ])
        ])
    }

    private func makeUpdateTab() -> NSTabViewItem {
        let tab = startupUpdater.makeOptionsTab()
        // Keep the updater's existing controls and callbacks, but give each action its own list row.
        func takeRows(from stack: NSStackView) -> [NSView] {
            let children = stack.arrangedSubviews
            var rows: [NSView] = []
            for child in children {
                stack.removeArrangedSubview(child)
                child.removeFromSuperview()
                if let actions = child as? NSStackView {
                    rows.append(contentsOf: takeRows(from: actions))
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
        return optionsTab(tab.label, rows: [
            optionGroup("版本與更新", rows: rows)
        ])
    }

    private func optionsTab(_ title: String, rows: [NSView]) -> NSTabViewItem {
        let tab = NSTabViewItem(identifier: title)
        tab.label = title
        let heading = NSTextField(labelWithString: title)
        heading.font = .systemFont(ofSize: 24, weight: .bold)
        let stack = optionsStack([heading] + rows, spacing: 22)
        let content = OptionsDocumentView(frame: .zero)
        let scroll = NSScrollView(frame: .zero)
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.setAccessibilityLabel("\(title)設定列表")
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

    private func optionGroup(_ title: String, rows: [NSView]) -> NSStackView {
        let heading = NSTextField(labelWithString: title)
        heading.font = .systemFont(ofSize: 18, weight: .bold)
        return optionsStack([heading] + rows.map { optionControlRow($0) }, spacing: 12)
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
        if control is NSPopUpButton || control is NSTextField || control is NSStackView {
            control.widthAnchor.constraint(equalTo: row.widthAnchor).isActive = true
        }
        return row
    }

    private func labeledOption(_ title: String, _ control: NSView) -> NSStackView {
        let label = NSTextField(labelWithString: title)
        label.font = .systemFont(ofSize: 15, weight: .semibold)
        control.setAccessibilityLabel(title)
        if let text = control as? NSTextField {
            text.lineBreakMode = .byTruncatingMiddle
            text.maximumNumberOfLines = 1
            text.toolTip = text.stringValue
        }
        return optionsStack([label, optionControlRow(control)], spacing: 5)
    }

    private func optionChoice(
        _ title: String, choices: [UInt32], selected: UInt32, action: Selector, label: (UInt32) -> String
    ) -> NSStackView {
        labeledOption(title, choiceControl(choices, selected: selected, action: action, label: label))
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
        if let control = view as? NSControl, (control.font?.pointSize ?? 0) < 14 {
            control.font = .systemFont(ofSize: 15)
        }
        if let stack = view as? NSStackView {
            for child in stack.arrangedSubviews { prepareOptionsText(child) }
        }
    }

    private func folderOptions(_ folder: ExportFolder) -> NSStackView {
        let choose = NSButton(title: "選擇…", target: self, action: #selector(chooseFolder(_:)))
        let reveal = NSButton(title: "在 Finder 開啟", target: self, action: #selector(openFolder(_:)))
        choose.tag = folder.rawValue
        reveal.tag = folder.rawValue
        choose.setAccessibilityLabel("選擇\(folder.label)")
        reveal.setAccessibilityLabel("在 Finder 開啟\(folder.label)")
        let label = NSTextField(labelWithString: folder.label)
        label.font = .systemFont(ofSize: 15, weight: .semibold)
        let path = NSTextField(labelWithString: directory(for: folder))
        path.toolTip = path.stringValue
        path.textColor = .secondaryLabelColor
        path.lineBreakMode = .byTruncatingMiddle
        path.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        path.maximumNumberOfLines = 1
        if folder == .output { outputPathLabel = path } else { savedPathLabel = path }
        return optionsStack([label, path, optionControlRow(choose), optionControlRow(reveal)], spacing: 6)
    }

    private func updateOptionsVisibility() {
        optionsVisibilityButton?.title = panel?.isVisible == true ? "隱藏波形" : "顯示波形"
    }

    @objc private func showMeterOptions(_ sender: NSButton) {
        showMenu(meterMenu(lane: sender.tag), below: sender)
    }

    private func meterMenu(lane: Int) -> NSMenu {
        let menu = NSMenu(title: "電平顯示")
        for mode in MeterMode.allCases {
            let item = NSMenuItem(title: mode.label, action: #selector(meterChanged(_:)), keyEquivalent: "")
            item.tag = Int(mode.rawValue)
            item.target = self
            item.representedObject = lane
            item.state = mode == (lane == 0 ? meterMode : meterModeB) ? .on : .off
            menu.addItem(item)
        }
        return menu
    }

    private func showMenu(_ menu: NSMenu, below sender: NSButton) {
        let below = sender.isFlipped ? sender.bounds.maxY + 3 : sender.bounds.minY - 3
        _ = menu.popUp(positioning: nil, at: NSPoint(x: sender.bounds.minX, y: below), in: sender)
    }

    private func quitItem(shortcut: String = "") -> NSMenuItem {
        let item = NSMenuItem(title: "結束 Backtracking", action: #selector(NSApplication.terminate(_:)), keyEquivalent: shortcut)
        item.target = NSApp
        return item
    }

    private func captureMenuItem() -> NSMenuItem {
        let item = NSMenuItem(title: captureActionLabel, action: #selector(toggleCapture(_:)), keyEquivalent: "")
        item.target = self
        item.representedObject = paused
        return item
    }

    private func loginItem() -> NSMenuItem {
        let status = SMAppService.mainApp.status
        let title = status == .requiresApproval ? "登入時啟動（待核准）" : "登入時啟動"
        let item = NSMenuItem(title: title, action: #selector(toggleLaunchAtLogin), keyEquivalent: "")
        item.target = self
        item.state = status == .enabled ? .on : (status == .requiresApproval ? .mixed : .off)
        return item
    }

    @objc private func toggleLaunchAtLogin() {
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
                alert.messageText = "無法更新登入啟動設定"
                NSApp.activate()
                _ = alert.runModal()
            }
        }
    }

    private func updateLoginOption() {
        let status = SMAppService.mainApp.status
        optionsLoginButton?.allowsMixedState = status == .requiresApproval
        optionsLoginButton?.title = status == .requiresApproval ? "登入時啟動（待核准）" : "登入時啟動"
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
        guard rgb != accentRGB else { return }
        accentRGB = rgb
        defaults.set(Int(rgb), forKey: "accentRGB")
        waveform?.accentRGB = rgb
        updateBackground()
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
        updateBackground()
    }

    @objc private func outlineOpacityChanged(_ sender: NSButton) {
        outlineFollowsOpacity = sender.state == .on
        defaults.set(outlineFollowsOpacity, forKey: "waveformOutlineFollowsOpacity")
        updateBackground()
    }

    @objc private func outlineModeChanged(_ sender: NSPopUpButton) {
        customOutline = selectedValue(sender) == 1
        defaults.set(customOutline, forKey: "customWaveformOutline")
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
                self.setNotice("無法調整保留時間")
            }
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
        background?.showsOutline = showsOutline
        background?.outlineColor = accentColor(customOutline ? outlineRGB : accentRGB)
        background?.outlineFollowsOpacity = outlineFollowsOpacity
        waveform?.gridOpacity = opacity
        waveform?.showsBars = showsBars
        waveform?.secondsInterval = showsSeconds ? secondsInterval : nil
    }

    @objc private func meterChanged(_ sender: Any) {
        let tag = (sender as? NSPopUpButton)?.selectedItem?.tag ?? (sender as? NSMenuItem)?.tag
        guard let tag, let raw = UInt32(exactly: tag), let mode = MeterMode(rawValue: raw) else { return }
        let lane = (sender as? NSPopUpButton)?.tag ?? ((sender as? NSMenuItem)?.representedObject as? Int) ?? 0
        if lane == 0 {
            meterMode = mode
            defaults.set(Int(raw), forKey: "meterMode")
            optionsMeterPopup?.selectItem(withTag: tag)
        } else {
            meterModeB = mode
            defaults.set(Int(raw), forKey: "meterModeB")
            optionsMeterBPopup?.selectItem(withTag: tag)
        }
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

    private func updateMeter() {
        meterBButton.isHidden = !dualSource
        for (lane, item) in [(meterButton, meterMode), (meterBButton, meterModeB)].enumerated() {
            let (button, mode) = item
            let reading = tap.map { mode.value(from: $0.cachedMeters[lane]) } ?? -Float.infinity
            let value = reading.isFinite ? String(format: "%.1f", Double(reading)) : "—"
            let prefix = dualSource ? (lane == 0 ? "A " : "B ") : ""
            let title = "\(prefix)\(value) \(mode.label)"
            if button.title != title || button.toolTip == nil {
                button.title = title
                button.setAccessibilityLabel(title)
            }
            if button.toolTip != title { button.toolTip = title }
        }
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
        setIcon(alwaysOnTop ? "pin.fill" : "pin", label: alwaysOnTop ? "取消置頂" : "置頂", on: pinButton)
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
                desiredSize = NSSize(width: previous.width, height: waveformHeight + 2 * waveformFrameInset)
            }
        } else if !initial {
            desiredSize = fullPanelSize
        }

        toolbarTrailing?.isActive = !waveformOnly
        toolbar?.isHidden = waveformOnly
        waveformLeading?.constant = waveformOnly ? waveformFrameInset : 10
        waveformTrailing?.constant = waveformOnly ? -waveformFrameInset : -10
        waveformBottom?.constant = waveformOnly ? -waveformFrameInset : -panelVerticalInset
        if waveformOnly {
            waveformTopBelowToolbar?.isActive = false
            waveformMinimumHeight?.constant = waveformOnlyPanelMinimum.height - 2 * waveformFrameInset
            waveformTopWithoutToolbar?.isActive = true
        } else {
            waveformTopWithoutToolbar?.isActive = false
            waveformMinimumHeight?.constant = fullWaveformMinimumHeight
            waveformTopBelowToolbar?.isActive = true
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
        chooser.title = "選擇\(folder.label)"
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
        alert.messageText = "無法開啟儲存資料夾"
        alert.informativeText = detail
        alert.addButton(withTitle: "好")
        _ = alert.runModal()
    }

    func registerClick() {
        lastClick = ProcessInfo.processInfo.systemUptime
    }

    func beginEditing(retainMaterial: Bool = true) {
        editing = true
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

    private var captureActionLabel: String {
        if tap == nil || captureError != nil { return "重新連線並開始錄音" }
        return paused ? "開始錄音" : "暫停錄音"
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
        waveform?.resetTimeline()
        refreshWaveform()
        notice = nil
        updateStatus()
    }

    @objc private func cancelWaveformSelection() {
        _ = waveform?.cancelSelection()
    }

    private func pauseCapture() {
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
        let panelVisible = panel?.isVisible == true
        if panelVisible { waveform?.refreshHintModifiers() }
        let now = ProcessInfo.processInfo.systemUptime
        if !paused, now - lastSourceCheck >= 1 {
            lastSourceCheck = now
            if tap?.refreshSources() == true { reconnectConfiguredSources() }
        }
        if !independentWindow && now - lastShortcutCheck >= 1 {
            lastShortcutCheck = now
            snapshotShortcut.start()
            if optionsPanel?.isVisible == true { refreshShortcutPermission() }
        }
        if let tap, !paused {
            if panelVisible { refreshWaveform() }
            tap.pollCapture()
        }
        if paused && !editing && waveform?.hasSelection != true && tap != nil && idleSeconds > 0 &&
            ProcessInfo.processInfo.systemUptime - lastClick >= Double(idleSeconds) {
            registerClick()
            resume()
        }
        if panelVisible || optionsPanel?.isVisible == true { updateStatus() }
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
        let label = captureActionLabel
        if captureButton.title != label {
            setIcon(paused ? "play.fill" : "pause.fill", label: label, on: captureButton)
        }
        captureButton.isEnabled = !editing
        clearButton.isEnabled = tap?.core != nil && !editing
        if let optionsCaptureButton, optionsCaptureButton.title != label {
            optionsCaptureButton.title = label
        }
        optionsCaptureButton?.isEnabled = captureButton.isEnabled
        optionsClearButton?.isEnabled = clearButton.isEnabled
        optionsSaveButton?.isEnabled = clearButton.isEnabled && waveform?.isPreparingMaterial != true
        optionsCancelSelectionButton?.isEnabled = waveform?.canCancelSelection == true
        updateMeter()

        let now = ProcessInfo.processInfo.systemUptime
        var detail = state.label
        if paused && !editing && waveform?.hasSelection != true && tap != nil && idleSeconds > 0 {
            let elapsed = now - lastClick
            let remaining = max(0, Int(ceil(Double(idleSeconds) - elapsed)))
            detail += " · \(remaining) 秒後續錄"
        }
        captureLamp.update(state, now: now, detail: detail)
        let message = notice ?? captureError ?? tap?.sourceMessage ?? ""
        if let optionsNotice, optionsNotice.stringValue != message {
            optionsNotice.stringValue = message
            optionsNotice.toolTip = message.isEmpty ? nil : message
        }
        if statusLabel.stringValue != message {
            statusLabel.stringValue = message
            statusLabel.toolTip = message.isEmpty ? nil : message
        }
        let hideStatus = waveformOnly || message.isEmpty
        if statusLabel.isHidden != hideStatus { statusLabel.isHidden = hideStatus }
    }

    fileprivate func setNotice(_ message: String?) {
        notice = message
        updateStatus()
    }

    func frozenExport(range: Range<UInt64>? = nil) -> WAVExport? {
        waveform?.materialForExport(range: range)
    }

    func requestEditingMaterial(_ completion: @escaping (WAVExport?) -> Void) {
        guard let tap else { completion(nil); return }
        let routes = tap.captureRoutes
        tap.readCapture({ WAVExport(core: $0, routes: routes) }) { [weak self, weak tap] material in
            guard let self, let tap, self.tap === tap else { completion(nil); return }
            completion(material)
        }
    }

    func retireEditingMaterial(_ material: WAVExport?) {
        guard let material else { return }
        worker.async { withExtendedLifetime(material) {} }
    }

    @objc private func saveCurrentSnapshot() {
        guard !editing else { return }
        guard waveform?.isPreparingMaterial != true else { setNotice("正在準備選取素材…"); return }
        let selected = waveform?.selectionForSaving()
        let save: (WAVExport?) -> Void = { [weak self] job in
            guard let self else { return }
            guard let job else { self.setNotice("還沒有可保存的錄音"); return }
            let range = selected ?? job.span
            guard range.lowerBound >= job.span.lowerBound, range.upperBound <= job.span.upperBound, !range.isEmpty else {
                self.setNotice("選段已超出可保存的錄音範圍")
                return
            }
            let ticket = self.waveform?.reserveMarker()
            if selected != nil { self.waveform?.clearSelection() }
            self.writeExport(job, range: range, directory: self.directory(for: .saved), provisional: false) { [weak self] urls in
                if !urls.isEmpty, let ticket { self?.waveform?.commitMarker(range, ticket: ticket) }
                return true
            }
        }
        if let material = waveform?.materialForExport(range: selected) {
            save(material)
        } else {
            requestEditingMaterial(save)
        }
    }

    func prepareDrag(
        _ job: WAVExport, range: Range<UInt64>, completion: @escaping ([URL]) -> Bool
    ) -> DispatchWorkItem {
        writeExport(job, range: range, directory: directory(for: .output), provisional: true, completion: completion)
    }

    @discardableResult
    private func writeExport(
        _ job: WAVExport, range: Range<UInt64>, directory: String,
        provisional: Bool, completion: @escaping ([URL]) -> Bool
    ) -> DispatchWorkItem {
        let sampleRate = exportSampleRate
        let bitsPerSample = exportBitsPerSample
        let forceMono = forceMonoExport
        setNotice(provisional ? "正在準備拖放…" : "正在寫入 WAV…")
        let write = DispatchWorkItem { @Sendable [weak self, worker, pendingProvisionalWAVs] in
            let urls = job.write(
                range: range, directory: directory, sampleRate: sampleRate, bitsPerSample: bitsPerSample,
                forceMono: forceMono
            )
            if provisional { pendingProvisionalWAVs.urls.formUnion(urls) }
            DispatchQueue.main.async {
                if provisional {
                    worker.async { pendingProvisionalWAVs.urls.subtract(urls) }
                }
                guard completion(urls) else {
                    if provisional {
                        for url in urls { try? FileManager.default.removeItem(at: url) }
                    }
                    return
                }
                if let first = urls.first {
                    self?.setNotice(provisional ? nil : (urls.count == 2 ? "已存入 A／B WAV" : "已存入 \(first.lastPathComponent)"))
                } else {
                    self?.setNotice("無法匯出 WAV，請確認有錄音及資料夾寫入權限")
                }
            }
        }
        worker.async(execute: write)
        return write
    }

}

private final class WaveformView: NSView, NSDraggingSource {
    weak var controller: BacktrackingController?
    var showsOverlay = true { didSet { needsDisplay = true } }
    var gridOpacity: CGFloat = 1 { didSet { needsDisplay = true } }
    var showsBars = false {
        didSet {
            if showsBars != oldValue {
                needsDisplay = true
            }
        }
    }
    var secondsInterval: UInt32? { didSet { needsDisplay = true } }
    var accentRGB = defaultAccentRGB { didSet { needsDisplay = true } }
    var stereo = false {
        didSet { if stereo != oldValue { revision = nil } }
    }

    private var traces = [WaveformTrace(), WaveformTrace()]
    private var traceCount = 1
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
    private var markerRevision: UInt64 = 0
    private var committedMarkerRevision: UInt64 = 0
    private var pendingDragMarker: (range: Range<UInt64>, ticket: UInt64)?
    private var anchor: UInt64?
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

    var hasEditingMaterial: Bool { editMaterial != nil || materialPending }
    var isPreparingMaterial: Bool { materialPending }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var needsPanelToBecomeKey: Bool { true }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        needsDisplay = true
        controller?.refreshWaveform()
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
            width: Float(bounds.width), height: Float(bounds.height), stereo: stereo,
            interactive: anchor != nil || mouseDownEvent != nil || dragging || panStart != nil)
    }

    func refresh(tap: SystemAudioTap, worker: DispatchQueue) {
        guard !materialPending else { return }
        let request = readRequest()
        let material = editMaterial
        if material != nil && revision != nil && request == lastRenderedRequest { return }
        guard renderGate.request() else { return }
        let unchangedRevision = request == lastRenderedRequest ? revision : nil
        let version = materialVersion
        let receive: (WaveformFrame?) -> Void = { [weak self] frame in
            guard let self else { return }
            let stale = self.materialVersion != version || self.materialPending
                || self.editMaterial !== material || self.readRequest() != request
            let retry = self.renderGate.complete(stale: stale)
            defer {
                if retry && !self.materialPending { self.controller?.refreshWaveform() }
            }
            guard !stale, let frame else { return }
            self.revision = frame.revision
            self.sampleRate = frame.sampleRate
            self.retainedFrames = frame.retained
            self.visibleFrames = frame.visible
            self.renderedFrames = frame.visible
            self.requestedView = nil
            self.zoomed = frame.visible != frame.retained
            self.traceCount = frame.traces.count
            self.traces = frame.traces + Array(repeating: WaveformTrace(), count: max(0, 2 - frame.traces.count))
            self.bars = frame.bars
            self.lastRenderedRequest = self.readRequest()
            self.updateDisplayPolicy()
            self.needsDisplay = true
        }
        if let material {
            worker.async {
                let frame = material.render(request)
                DispatchQueue.main.async { receive(frame) }
            }
        } else {
            let laneCount = tap.laneCount
            tap.readCapture(poll: false, {
                WaveformFrame.read(core: $0, laneCount: laneCount, request: request, unchangedRevision: unchangedRevision)
            }, completion: receive)
        }
    }

    func retainEditingMaterial() {
        guard editMaterial == nil, !materialPending else { return }
        materialPending = true
        materialVersion &+= 1
        let version = materialVersion
        controller?.requestEditingMaterial { [weak self] material in
            guard let self else { return }
            guard self.materialVersion == version else {
                self.controller?.retireEditingMaterial(material)
                return
            }
            self.materialPending = false
            guard let material else {
                self.controller?.setNotice("還沒有可選取的錄音")
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
                    self.controller?.setNotice("選段已超出可保存的錄音範圍")
                    return
                }
            }
            self.zoomed = self.visibleFrames != self.retainedFrames
            self.revision = nil
            self.controller?.refreshWaveform()
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

    override func scrollWheel(with event: NSEvent) {
        guard !dragging, !visibleFrames.isEmpty, event.scrollingDeltaY != 0 else { return }
        let fraction = Double(fraction(at: convert(event.locationInWindow, from: nil).x))
        let delta = Double(event.scrollingDeltaY) * (event.hasPreciseScrollingDeltas ? 0.014 : 0.12)
        changeView(to: zoomedWaveformRange(requestedView ?? visibleFrames, in: retainedFrames,
                                          width: Double(bounds.width), cursorFraction: fraction, delta: delta))
    }

    override func otherMouseDown(with event: NSEvent) {
        guard event.buttonNumber == 2, zoomed, !dragging else { return }
        panStart = (convert(event.locationInWindow, from: nil).x, requestedView ?? visibleFrames)
    }

    override func otherMouseDragged(with event: NSEvent) {
        guard event.buttonNumber == 2, let panStart else { return }
        let delta = Double((convert(event.locationInWindow, from: nil).x - panStart.x) / max(1, bounds.width))
        let span = panStart.range.upperBound - panStart.range.lowerBound
        changeView(to: fittedRange(start: Double(panStart.range.lowerBound) - delta * Double(span), span: span))
    }

    override func otherMouseUp(with event: NSEvent) {
        if event.buttonNumber == 2 { panStart = nil }
    }

    func clearSelection() {
        selection = nil
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
        retainedFrames = 0..<0
        visibleFrames = 0..<0
        renderedFrames = 0..<0
        zoomed = false
        panStart = nil
        revision = nil
        bars.removeAll()
        marker = nil
        committedMarkerRevision = reserveMarker()
    }

    func reserveMarker() -> UInt64 {
        markerRevision &+= 1
        return markerRevision
    }

    func commitMarker(_ range: Range<UInt64>, ticket: UInt64) {
        guard ticket > committedMarkerRevision, !range.isEmpty else { return }
        committedMarkerRevision = ticket
        marker = range
        needsDisplay = true
    }

    func selectionForSaving() -> Range<UInt64>? {
        guard let selection, !selection.isEmpty else { return nil }
        return selection
    }

    var hasSelection: Bool { selectionForSaving() != nil }

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
        return bounds.minX + bounds.width * CGFloat(fraction)
    }

    private func isVisible(_ frame: UInt64) -> Bool {
        !visibleFrames.isEmpty && frame >= visibleFrames.lowerBound && frame <= visibleFrames.upperBound
    }

    private func flagPosition(for frame: UInt64) -> CGFloat {
        min(bounds.maxX - 1.5, max(bounds.minX + 1.5, x(for: frame)))
    }

    private func draggableFlag(at position: NSPoint) -> UInt64? {
        guard let flags = selection ?? marker, position.y >= bounds.maxY - 17 else { return nil }
        return [flags.lowerBound, flags.upperBound].filter(isVisible)
            .map { (frame: $0, distance: abs(flagPosition(for: $0) - position.x)) }
            .filter { $0.distance <= 8 }
            .min { $0.distance < $1.distance }?.frame
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
                        seconds.baseAddress, seconds.count, 8 / Double(max(1, bounds.width)))
                }
            }
        }
        let frame = visibleFrames.lowerBound
            + UInt64((Double(visibleFrames.upperBound - visibleFrames.lowerBound) * snapped).rounded())
        return (flags + guideBars + guideSeconds).contains(frame) ? frame : nil
    }

    private func fraction(at x: CGFloat) -> CGFloat {
        min(1, max(0, (x - bounds.minX) / max(1, bounds.width)))
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
        needsDisplay = true
    }

    private func trackPointer(_ event: NSEvent) {
        let position = convert(event.locationInWindow, from: nil)
        pointer = bounds.contains(position) ? position : nil
        pointerModifiers = event.modifierFlags.intersection([.option, .control, .command, .shift])
        needsDisplay = true
    }

    private func trackDragPointer() {
        guard let window else { return }
        let position = convert(window.mouseLocationOutsideOfEventStream, from: nil)
        pointer = bounds.contains(position) ? position : nil
        needsDisplay = true
    }

    override func mouseEntered(with event: NSEvent) { mouseMoved(with: event) }

    override func mouseMoved(with event: NSEvent) { trackPointer(event) }

    func clearPointerFeedback() {
        pointer = nil
        needsDisplay = true
    }

    override func mouseExited(with event: NSEvent) { clearPointerFeedback() }

    override func mouseDown(with event: NSEvent) {
        guard !dragging, let controller else { return }
        if requestedView != nil {
            requestedView = nil
            revision = nil
        }
        trackPointer(event)
        let flag = draggableFlag(at: convert(event.locationInWindow, from: nil))
        if let flag, let flags = selection ?? marker {
            controller.beginEditing()
            window?.makeKey()
            window?.makeFirstResponder(self)
            let edge = min(max(flag, retainedFrames.lowerBound), retainedFrames.upperBound)
            let other = flag == flags.lowerBound ? flags.upperBound : flags.lowerBound
            let opposite = min(max(other, retainedFrames.lowerBound), retainedFrames.upperBound)
            anchor = opposite
            selection = min(edge, opposite)..<max(edge, opposite)
            needsDisplay = true
        } else if selectionModifierDown {
            clearSelection()
            controller.beginEditing()
            window?.makeKey()
            window?.makeFirstResponder(self)
            controller.refreshWaveform()
            anchor = frame(at: event)
            selection = anchor.map { $0..<$0 }
            needsDisplay = true
        } else {
            mouseDownEvent = event
        }
    }

    override func rightMouseDown(with event: NSEvent) {
        if event.modifierFlags.contains(.control) { mouseDown(with: event) }
    }

    override func mouseDragged(with event: NSEvent) {
        trackPointer(event)
        if let anchor {
            let position = frame(at: event)
            selection = min(anchor, position)..<max(anchor, position)
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
        guard anchor != nil || mouseDownEvent != nil else { return }
        var keepSelection = false
        if anchor != nil {
            mouseDragged(with: event)
            anchor = nil
            keepSelection = hasSelection
        } else if !dragging && dragRequested {
            discardPreparedFile()
            controller?.setNotice(nil)
        }
        mouseDownEvent = nil
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
            controller?.setNotice("還沒有可匯出的錄音")
            return
        }
        let version = exportVersion
        let range = selectionForSaving() ?? job.span
        guard range.lowerBound >= job.span.lowerBound, range.upperBound <= job.span.upperBound, !range.isEmpty else {
            controller.setNotice("選段已超出可匯出的錄音範圍")
            return
        }
        let ticket = reserveMarker()
        // DAWs can keep the file URLs after the external drop. Treat both paths as one take.
        pendingDragWrite = controller.prepareDrag(job, range: range) { [weak self] urls in
            guard let self, self.exportVersion == version else { return false }
            self.pendingDragWrite = nil
            guard let event = self.mouseDownEvent, NSEvent.pressedMouseButtons & 1 != 0,
                  !self.pointerIsInApp(at: NSEvent.mouseLocation) else {
                self.discardPreparedFile()
                self.controller?.setNotice(nil)
                return false
            }
            if !urls.isEmpty {
                self.pendingDragMarker = (range, ticket)
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
        if accepted {
            readyFiles.removeAll()
            if let pendingDragMarker { commitMarker(pendingDragMarker.range, ticket: pendingDragMarker.ticket) }
            pendingDragMarker = nil
        } else { discardPreparedFile() }
        endGesture()
    }

    private func endGesture(keepingSelection: Bool = false) {
        if keepingSelection {
            controller?.endEditing(allowImmediateResume: false)
        } else {
            clearSelection()
            controller?.resumeAfterSelectionCancellation()
        }
    }

    private func waveformRect(for range: Range<UInt64>) -> NSRect? {
        let first = max(range.lowerBound, visibleFrames.lowerBound)
        let end = min(range.upperBound, visibleFrames.upperBound)
        guard first < end else { return nil }
        return NSRect(x: x(for: first), y: bounds.minY, width: x(for: end) - x(for: first), height: bounds.height)
    }

    private func drawFlag(_ frame: UInt64, pointsRight: Bool) {
        guard isVisible(frame) else { return }
        let lineX = flagPosition(for: frame)
        let top = bounds.maxY - 3
        let flagWidth: CGFloat = pointsRight ? 6 : -6
        let shape = NSBezierPath()
        shape.move(to: NSPoint(x: lineX, y: top - 11))
        shape.line(to: NSPoint(x: lineX, y: top))
        shape.lineWidth = 1.5
        let snapped = pointerModifiers.contains(.shift) && pointer.flatMap { snapTarget(near: $0.x) } == frame
        let color = snapped ? NSColor.systemYellow : NSColor.white.withAlphaComponent(0.9)
        color.setStroke()
        shape.stroke()

        let flag = NSBezierPath()
        flag.move(to: NSPoint(x: lineX, y: top))
        flag.line(to: NSPoint(x: lineX + flagWidth, y: top - 2.5))
        flag.line(to: NSPoint(x: lineX, y: top - 5))
        flag.close()
        color.setFill()
        flag.fill()
    }

    private func tracePaths() -> (fill: NSBezierPath, outline: NSBezierPath, coverage: NSBezierPath) {
        let fill = NSBezierPath()
        let outline = NSBezierPath()
        let coverage = NSBezierPath()
        outline.lineWidth = CGFloat(displayPolicy.waveform_stroke)
        outline.lineCapStyle = .round
        outline.lineJoinStyle = .round
        let rows = min(traceCount, traces.count)
        guard rows > 0 else { return (fill, outline, coverage) }
        let totalHeight = bounds.height
        let rowHeight = totalHeight / CGFloat(rows)
        for lane in 0..<rows {
            let middle = bounds.midY + totalHeight / 2 - (CGFloat(lane) + 0.5) * rowHeight
            if showsOverlay {
                NSColor.white.withAlphaComponent(0.17).setStroke()
                NSBezierPath.strokeLine(
                    from: NSPoint(x: bounds.minX, y: middle), to: NSPoint(x: bounds.maxX, y: middle)
                )
            }
            if renderedFrames.isEmpty { continue }
            let bins = traces[lane]
            func point(_ x: Double, _ value: Float) -> NSPoint {
                NSPoint(x: bounds.minX + CGFloat(x) * bounds.width,
                        y: middle + CGFloat(value) * rowHeight * 0.46)
            }
            func cover(_ first: Double, _ end: Double) {
                coverage.append(NSBezierPath(rect: NSRect(
                    x: bounds.minX + CGFloat(first) * bounds.width, y: middle - rowHeight / 2,
                    width: CGFloat(max(0, end - first)) * bounds.width, height: rowHeight
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
        }
        return (fill, outline, coverage)
    }

    private func paintTrace(
        _ paths: (fill: NSBezierPath, outline: NSBezierPath, coverage: NSBezierPath), color: NSColor
    ) {
        guard !paths.coverage.isEmpty else { return }
        NSGraphicsContext.saveGraphicsState()
        paths.coverage.addClip()
        color.withAlphaComponent(0.72).setFill()
        paths.fill.fill()
        color.setStroke()
        paths.outline.stroke()
        NSGraphicsContext.restoreGraphicsState()
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
        func draw(_ positions: [UInt64], alpha: CGFloat, width: Float) {
            guard !positions.isEmpty else { return }
            let path = NSBezierPath()
            path.lineWidth = CGFloat(width)
            for frame in positions where isVisible(frame) {
                let x = x(for: frame)
                path.move(to: NSPoint(x: x, y: bounds.minY))
                path.line(to: NSPoint(x: x, y: bounds.maxY))
            }
            NSColor.white.withAlphaComponent(alpha * gridOpacity).setStroke()
            path.stroke()
        }
        draw(guideSeconds, alpha: 0.16, width: displayPolicy.seconds_stroke)
        draw(guideBars, alpha: 0.30, width: displayPolicy.bar_stroke)
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        updateDisplayPolicy()
        let panel = NSBezierPath(roundedRect: bounds, xRadius: 6, yRadius: 6)
        NSGraphicsContext.saveGraphicsState()
        panel.addClip()
        if showsOverlay, selection == nil, !visibleFrames.isEmpty,
           pointer != nil || mouseDownEvent != nil || dragging {
            accentColor(accentRGB).withAlphaComponent(0.23).setFill()
            NSBezierPath.fill(bounds)
        }

        drawTimeGrid()
        let trace = tracePaths()
        if let selectedRect = selection.flatMap(waveformRect(for:)) {
            let outside = NSBezierPath(rect: bounds)
            outside.append(NSBezierPath(rect: selectedRect))
            outside.windingRule = .evenOdd
            NSGraphicsContext.saveGraphicsState()
            outside.addClip()
            paintTrace(trace, color: accentColor(accentRGB))
            NSGraphicsContext.restoreGraphicsState()

            NSGraphicsContext.saveGraphicsState()
            NSBezierPath(rect: selectedRect).addClip()
            paintTrace(trace, color: accentColor(accentRGB ^ 0xFFFFFF))
            NSGraphicsContext.restoreGraphicsState()
        } else {
            paintTrace(trace, color: accentColor(accentRGB))
        }
        if let marker, marker != selection {
            drawFlag(marker.lowerBound, pointsRight: true)
            drawFlag(marker.upperBound, pointsRight: false)
        }
        if let selection, !selection.isEmpty {
            drawFlag(selection.lowerBound, pointsRight: true)
            drawFlag(selection.upperBound, pointsRight: false)
        }
        NSGraphicsContext.restoreGraphicsState()
    }
}

import AppKit

private let timeChoices: [UInt32] = [10, 20, 30]
private let autoResumeChoices: [UInt32] = [0, 5, 10]
private let autoResumePreferenceKey = "autoResumeSeconds"
private let transparencyChoices: [UInt32] = [0, 25, 50, 75, 100]
private let exportSampleRates: [UInt32] = [44_100, 48_000, 96_000]
private let exportBitDepths: [UInt32] = [16, 24, 32]
private let frameAutosaveName: NSWindow.FrameAutosaveName = "Backtracking.Panel"
private let fullPanelMinimum = NSSize(width: 560, height: 100)
private let waveformOnlyPanelMinimum = NSSize(width: 560, height: 48)
private let panelVerticalInset: CGFloat = 8
private let fullWaveformMinimumHeight: CGFloat = 51
private let waveformMint = (red: CGFloat(0.32), green: CGFloat(0.89), blue: CGFloat(0.82))

private func movesFromWaveform(_ modifiers: NSEvent.ModifierFlags) -> Bool {
    modifiers.contains([.command, .option]) && !modifiers.contains(.control)
}

@main
enum BacktrackingApp {
    @MainActor
    static func main() {
        let app = NSApplication.shared
        let delegate = BacktrackingController()
        app.delegate = delegate
        app.setActivationPolicy(.regular)
        app.run()
        withExtendedLifetime(delegate) {}
    }
}

private final class CapturePanel: NSPanel {
    override var canBecomeKey: Bool { true }

    var onClick: (() -> Void)?
    var onContextMenu: ((NSEvent) -> Void)?
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
                if !edges.isEmpty {
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
            if !controlSelect, let onContextMenu {
                onContextMenu(event)
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

// Created only from frozen history on the UI thread. The worker owns all access to the copy.
private final class WAVExport: @unchecked Sendable {
    let snapshot: OpaquePointer

    init(_ snapshot: OpaquePointer) {
        self.snapshot = snapshot
    }

    deinit { bt_snapshot_free(snapshot) }

    func write(range: ClosedRange<Double>, directory: String, sampleRate: UInt32, bitsPerSample: UInt32) -> URL? {
        directory.withCString { destination in
            guard let path = bt_snapshot_export_with_profile(
                snapshot, range.lowerBound, range.upperBound, destination, sampleRate, bitsPerSample
            ) else {
                return nil
            }
            defer { bt_string_free(path) }
            return String(validatingUTF8: path).map { URL(fileURLWithPath: $0) }
        }
    }
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

    override func layout() {
        super.layout()
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        NSColor.clear.setFill()
        NSRectFillUsingOperation(dirtyRect, .copy)
        guard backgroundOpacity > 0 else { return }
        let fill = NSBezierPath(rect: bounds)
        let waveShape = waveform.map { NSBezierPath(roundedRect: convert($0.bounds, from: $0), xRadius: 6, yRadius: 6) }
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

@MainActor
private final class BacktrackingController: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private let defaults = UserDefaults.standard
    private let worker = DispatchQueue(label: "backtracking.wav-export", qos: .userInitiated)
    private var tap: SystemAudioTap?
    private var panel: CapturePanel?
    private var background: CaptureBackground?
    private var waveform: WaveformView?
    private var timer: Timer?
    private let captureButton = NSButton(title: "", target: nil, action: nil)
    private let captureLamp = CaptureLamp(frame: .zero)
    private let clearButton = NSButton(title: "清除目前錄音", target: nil, action: nil)
    private let meterButton = NSButton(title: "— dBFS", target: nil, action: nil)
    private let statusLabel = NSTextField(labelWithString: "")
    private var toolbar: NSStackView?
    private var waveformMinimumHeight: NSLayoutConstraint?
    private var waveformTopBelowToolbar: NSLayoutConstraint?
    private var waveformTopWithoutToolbar: NSLayoutConstraint?
    private var history: UInt32 = 20
    private var idleSeconds: UInt32 = 10
    private var transparency: UInt32 = 0
    private var meterMode: MeterMode = .dbfs
    private var exportSampleRate: UInt32 = 48_000
    private var exportBitsPerSample: UInt32 = 32
    private var alwaysOnTop = true
    private var waveformOnly = false
    private var paused = true
    private var captureError: String?
    private var editing = false
    private var lastClick = ProcessInfo.processInfo.systemUptime
    private var notice: String?
    private var output = ""

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let saved = unsignedSetting("history"), timeChoices.contains(saved) {
            history = saved
        }
        // The old idleSeconds key stored 0 for manual mode.
        if let seconds = unsignedSetting(autoResumePreferenceKey), autoResumeChoices.contains(seconds) {
            idleSeconds = seconds
        }
        if let saved = unsignedSetting("backgroundTransparency"), transparencyChoices.contains(saved) {
            transparency = saved
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
        alwaysOnTop = defaults.object(forKey: "alwaysOnTop") as? Bool ?? true
        waveformOnly = defaults.object(forKey: "waveformOnly") as? Bool ?? false
        if let saved = defaults.string(forKey: "outputDirectory"), saved.hasPrefix("/"), !saved.utf8.contains(0) {
            output = saved
        } else {
            output = FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent("Backtracking/output", isDirectory: true).path
        }

        configureMenu()
        configurePanel()
        resume()
        let timer = Timer(timeInterval: 1.0 / 15.0, target: self, selector: #selector(tick(_:)), userInfo: nil, repeats: true)
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    private func unsignedSetting(_ key: String) -> UInt32? {
        guard let number = defaults.object(forKey: key) as? NSNumber else { return nil }
        return UInt32(exactly: number.doubleValue)
    }

    func applicationWillTerminate(_ notification: Notification) {
        panel?.saveFrame(usingName: frameAutosaveName)
        timer?.invalidate()
        timer = nil
        tap = nil
    }

    func windowWillClose(_ notification: Notification) {
        NSApp.terminate(nil)
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

    private func configurePanel() {
        let window = CapturePanel(
            contentRect: NSRect(x: 0, y: 0, width: 590, height: 116),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered, defer: false
        )
        window.title = "Backtracking"
        window.contentMinSize = fullPanelMinimum
        // CapturePanel handles dragging and resizing without AppKit's native window gestures.
        window.isMovable = false
        window.delegate = self
        window.becomesKeyOnlyIfNeeded = true
        window.acceptsMouseMovedEvents = true
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .fullScreenDisallowsTiling]
        window.onClick = { [weak self] in self?.registerClick() }
        window.onContextMenu = { [weak self] event in self?.showContextMenu(event) }

        let waveform = WaveformView(frame: .zero)
        waveform.controller = self
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
        clearButton.toolTip = "清除目前錄音，保留已匯出的 WAV"
        meterButton.target = self
        meterButton.action = #selector(showMeterOptions(_:))
        meterButton.bezelStyle = .rounded
        meterButton.font = .monospacedDigitSystemFont(ofSize: 11, weight: .medium)
        meterButton.widthAnchor.constraint(equalToConstant: 88).isActive = true

        let pinButton = NSButton(title: "置頂", target: self, action: #selector(pinChanged(_:)))
        pinButton.setButtonType(.pushOnPushOff)
        pinButton.bezelStyle = .rounded
        let settingsButton = NSButton(title: "設定", target: self, action: #selector(showSettings(_:)))
        settingsButton.bezelStyle = .rounded
        setIcon("gearshape", label: "設定", on: settingsButton)
        let spacer = NSView(frame: .zero)
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let controls = NSStackView(views: [captureButton, captureLamp, clearButton, statusLabel, spacer, pinButton, meterButton, settingsButton])
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
        NSLayoutConstraint.activate([
            controls.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 10),
            controls.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -10),
            controls.topAnchor.constraint(equalTo: content.topAnchor, constant: panelVerticalInset),
            waveform.leadingAnchor.constraint(equalTo: controls.leadingAnchor),
            waveform.trailingAnchor.constraint(equalTo: controls.trailingAnchor),
            waveform.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -panelVerticalInset)
        ])
        toolbar = controls
        waveformTopBelowToolbar = waveform.topAnchor.constraint(equalTo: controls.bottomAnchor, constant: 6)
        waveformTopBelowToolbar?.isActive = true
        waveformTopWithoutToolbar = waveform.topAnchor.constraint(equalTo: content.topAnchor, constant: panelVerticalInset)
        panel = window
        applyPin(to: pinButton)
        applyWaveformOnly()
        let restoredFrame = window.setFrameUsingName(frameAutosaveName, force: true)
        _ = window.setFrameAutosaveName(frameAutosaveName)
        if restoredFrame {
            keepPanelVisible(window)
        } else {
            window.center()
        }
        window.orderFrontRegardless()
    }

    private func keepPanelVisible(_ window: NSWindow) {
        let screens = NSScreen.screens
        if !screens.isEmpty && !screens.contains(where: { $0.visibleFrame.intersects(window.frame) }) {
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

    @objc private func showSettings(_ sender: NSButton) {
        showMenu(settingsMenu(), below: sender)
    }

    private func showContextMenu(_ event: NSEvent) {
        guard !editing, let content = panel?.contentView else { return }
        NSMenu.popUpContextMenu(settingsMenu(), with: event, for: content)
    }

    private func settingsMenu() -> NSMenu {
        let menu = NSMenu(title: "設定")
        if waveformOnly {
            let transportItem = NSMenuItem(title: captureActionLabel, action: #selector(toggleCapture(_:)), keyEquivalent: "")
            transportItem.target = self
            transportItem.representedObject = paused
            menu.addItem(transportItem)
            menu.addItem(.separator())
        }
        menu.addItem(choiceMenu(
            title: "保留時間", choices: timeChoices, selected: history,
            action: #selector(historyChanged(_:))
        ) { "\($0) 秒" })
        menu.addItem(choiceMenu(
            title: "自動續錄", choices: autoResumeChoices, selected: idleSeconds,
            action: #selector(idleChanged(_:))
        ) { "\($0) 秒" })
        menu.addItem(choiceMenu(
            title: "背景透明度", choices: transparencyChoices, selected: transparency,
            action: #selector(transparencyChanged(_:))
        ) { "\($0)%" })
        let meterItem = NSMenuItem(title: "電平顯示", action: nil, keyEquivalent: "")
        meterItem.submenu = meterMenu()
        menu.addItem(meterItem)
        menu.addItem(exportFormatMenuItem())
        let waveformItem = NSMenuItem(title: "只顯示波形", action: #selector(toggleWaveformOnly), keyEquivalent: "")
        waveformItem.target = self
        waveformItem.state = waveformOnly ? .on : .off
        menu.addItem(waveformItem)
        let outputItem = NSMenuItem(title: "選擇儲存資料夾…", action: #selector(chooseOutput), keyEquivalent: "")
        outputItem.target = self
        menu.addItem(outputItem)
        let revealItem = NSMenuItem(title: "前往儲存資料夾", action: #selector(openOutputFolder), keyEquivalent: "")
        revealItem.target = self
        menu.addItem(revealItem)
        menu.addItem(.separator())
        menu.addItem(quitItem())
        return menu
    }

    private func exportFormatMenuItem() -> NSMenuItem {
        let item = NSMenuItem(title: "WAV 匯出格式", action: nil, keyEquivalent: "")
        let menu = NSMenu(title: item.title)
        menu.addItem(choiceMenu(
            title: "取樣率", choices: exportSampleRates, selected: exportSampleRate,
            action: #selector(exportSampleRateChanged(_:))
        ) { $0 == 44_100 ? "44.1 kHz" : "\($0 / 1_000) kHz" })
        menu.addItem(choiceMenu(
            title: "編碼", choices: exportBitDepths, selected: exportBitsPerSample,
            action: #selector(exportEncodingChanged(_:))
        ) { $0 == 32 ? "32-bit float" : "\($0)-bit PCM" })
        item.submenu = menu
        return item
    }

    @objc private func showMeterOptions(_ sender: NSButton) {
        showMenu(meterMenu(), below: sender)
    }

    private func meterMenu() -> NSMenu {
        let menu = NSMenu(title: "電平顯示")
        for mode in MeterMode.allCases {
            let item = NSMenuItem(title: mode.label, action: #selector(meterChanged(_:)), keyEquivalent: "")
            item.tag = Int(mode.rawValue)
            item.target = self
            item.state = mode == meterMode ? .on : .off
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

    private func choiceMenu(title: String, choices: [UInt32], selected: UInt32, action: Selector, label: (UInt32) -> String) -> NSMenuItem {
        let section = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        let submenu = NSMenu(title: title)
        for choice in choices {
            let item = NSMenuItem(title: label(choice), action: action, keyEquivalent: "")
            item.tag = Int(choice)
            item.target = self
            item.state = choice == selected ? .on : .off
            submenu.addItem(item)
        }
        section.submenu = submenu
        return section
    }

    @objc private func historyChanged(_ sender: NSMenuItem) {
        guard let selected = UInt32(exactly: sender.tag), timeChoices.contains(selected) else { return }
        registerClick()
        if let core = tap?.core, bt_set_history(core, selected) != 1 {
            setNotice("無法調整保留時間")
            return
        }
        history = selected
        defaults.set(Int(history), forKey: "history")
        waveform?.clearSelection()
        refreshWaveform()
    }

    @objc private func idleChanged(_ sender: NSMenuItem) {
        guard let selected = UInt32(exactly: sender.tag), autoResumeChoices.contains(selected) else { return }
        idleSeconds = selected
        defaults.set(Int(idleSeconds), forKey: autoResumePreferenceKey)
        registerClick()
    }

    @objc private func transparencyChanged(_ sender: NSMenuItem) {
        guard let selected = UInt32(exactly: sender.tag), transparencyChoices.contains(selected) else { return }
        transparency = selected
        defaults.set(Int(selected), forKey: "backgroundTransparency")
        registerClick()
        updateBackground()
    }

    private func updateBackground() {
        let effectiveTransparency = waveformOnly ? 100 : transparency
        panel?.isOpaque = false
        panel?.backgroundColor = .clear
        // An explicit setting gives the clear window the same input region as the opaque one.
        panel?.ignoresMouseEvents = false
        panel?.hasShadow = effectiveTransparency < 100
        background?.backgroundOpacity = 1 - CGFloat(effectiveTransparency) / 100
    }

    @objc private func meterChanged(_ sender: NSMenuItem) {
        guard let raw = UInt32(exactly: sender.tag), let mode = MeterMode(rawValue: raw) else { return }
        meterMode = mode
        defaults.set(Int(raw), forKey: "meterMode")
        registerClick()
        updateMeter()
    }

    @objc private func exportSampleRateChanged(_ sender: NSMenuItem) {
        guard let rate = UInt32(exactly: sender.tag), exportSampleRates.contains(rate), rate != exportSampleRate else { return }
        exportSampleRate = rate
        defaults.set(Int(rate), forKey: "exportSampleRate")
        invalidateExportFormat()
    }

    @objc private func exportEncodingChanged(_ sender: NSMenuItem) {
        guard let bits = UInt32(exactly: sender.tag), exportBitDepths.contains(bits), bits != exportBitsPerSample else { return }
        exportBitsPerSample = bits
        defaults.set(Int(bits), forKey: "exportBitsPerSample")
        invalidateExportFormat()
    }

    private func invalidateExportFormat() {
        registerClick()
        waveform?.discardPreparedFile()
        setNotice(nil)
    }

    private func updateMeter() {
        let core = tap?.core
        let reading = core.map { meterMode.value(from: bt_meter_readings($0)) } ?? -Float.infinity
        let value = reading.isFinite ? String(format: "%.1f", Double(reading)) : "—"
        let title = "\(value) \(meterMode.label)"
        if meterButton.title != title || meterButton.toolTip == nil {
            meterButton.title = title
            meterButton.setAccessibilityLabel("\(title)，點擊切換單位")
        }
        let detail = "\(meterMode.detail) · 點擊切換單位"
        if meterButton.toolTip != detail { meterButton.toolTip = detail }
    }

    @objc private func pinChanged(_ sender: NSButton) {
        alwaysOnTop = sender.state == .on
        defaults.set(alwaysOnTop, forKey: "alwaysOnTop")
        applyPin(to: sender)
    }

    private func applyPin(to button: NSButton) {
        button.state = alwaysOnTop ? .on : .off
        panel?.isFloatingPanel = alwaysOnTop
        panel?.hidesOnDeactivate = false
        panel?.level = alwaysOnTop ? .floating : .normal
        setIcon(alwaysOnTop ? "pin.fill" : "pin", label: alwaysOnTop ? "取消置頂" : "置頂", on: button)
    }

    @objc private func toggleWaveformOnly() {
        waveformOnly.toggle()
        defaults.set(waveformOnly, forKey: "waveformOnly")
        registerClick()
        applyWaveformOnly()
    }

    private func applyWaveformOnly() {
        if waveformOnly, let waveform {
            panel?.contentView?.layoutSubtreeIfNeeded()
            waveform.maximumTraceHeight = max(waveform.bounds.height, fullWaveformMinimumHeight)
        } else {
            waveform?.maximumTraceHeight = nil
        }
        applyPanelMinimum()
        toolbar?.isHidden = waveformOnly
        if waveformOnly {
            waveformTopBelowToolbar?.isActive = false
            waveformMinimumHeight?.constant = waveformOnlyPanelMinimum.height - 2 * panelVerticalInset
            waveformTopWithoutToolbar?.isActive = true
        } else {
            waveformTopWithoutToolbar?.isActive = false
            waveformMinimumHeight?.constant = fullWaveformMinimumHeight
            waveformTopBelowToolbar?.isActive = true
        }
        waveform?.showsOverlay = !waveformOnly
        updateBackground()
        updateStatus()
    }

    private func applyPanelMinimum() {
        guard let panel else { return }
        let original = panel.frame
        let minimum = waveformOnly ? waveformOnlyPanelMinimum : fullPanelMinimum
        panel.contentMinSize = minimum
        let frameMinimum = panel.frameRect(forContentRect: NSRect(origin: .zero, size: minimum)).size
        let width = max(original.width, frameMinimum.width)
        let height = max(original.height, frameMinimum.height)
        guard width != original.width || height != original.height else { return }

        var adjusted = NSRect(x: original.minX, y: original.maxY - height, width: width, height: height)
        if let visible = panel.screen?.visibleFrame, height <= visible.height {
            adjusted.origin.y = min(max(adjusted.minY, visible.minY), visible.maxY - height)
        }
        panel.setFrame(adjusted, display: true)
    }

    @objc private func chooseOutput() {
        let wasPaused = paused
        beginEditing()
        let chooser = NSOpenPanel()
        chooser.title = "選擇 WAV 儲存資料夾"
        chooser.canChooseDirectories = true
        chooser.canChooseFiles = false
        chooser.canCreateDirectories = true
        chooser.allowsMultipleSelection = false
        chooser.directoryURL = URL(fileURLWithPath: output, isDirectory: true)
        chooser.begin { [weak self] response in
            guard let self else { return }
            if response == .OK, let url = chooser.url {
                self.output = url.path
                self.defaults.set(self.output, forKey: "outputDirectory")
                self.waveform?.discardPreparedFile()
                self.setNotice("輸出位置 \(self.output)")
            }
            self.endEditing(allowImmediateResume: !wasPaused)
        }
    }

    @objc private func openOutputFolder() {
        registerClick()
        let url = URL(fileURLWithPath: output, isDirectory: true)
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

    func beginEditing() {
        editing = true
        registerClick()
        pauseCapture()
    }

    var resumesImmediately: Bool { idleSeconds == 0 }

    func endEditing(allowImmediateResume: Bool = true) {
        guard editing else { return }
        editing = false
        registerClick()
        if allowImmediateResume && resumesImmediately && tap != nil {
            resume(clearingSelection: false)
        } else {
            updateStatus()
        }
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
        if paused { resume() } else { pauseCapture() }
    }

    @objc private func clearCapture() {
        guard !editing, let core = tap?.core else { return }
        registerClick()
        bt_clear(core)
        waveform?.clearSelection()
        refreshWaveform()
        notice = nil
        updateStatus()
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
                tap = try SystemAudioTap(history: history)
            }
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

    @objc private func tick(_: Timer) {
        waveform?.refreshHintModifiers()
        if let core = tap?.core, !paused {
            bt_poll(core)
            refreshWaveform()
        }
        if paused && !editing && tap != nil && idleSeconds > 0 &&
            ProcessInfo.processInfo.systemUptime - lastClick >= Double(idleSeconds) {
            registerClick()
            resume()
        }
        updateStatus()
    }

    func refreshWaveform() {
        guard let waveform, let tap, let core = tap.core else { return }
        waveform.refresh(core: core, sampleRate: tap.sampleRate)
    }

    private func updateStatus() {
        let state: CaptureLamp.State
        if tap == nil || captureError != nil {
            state = .unavailable
        } else if paused {
            state = .paused
        } else if let core = tap?.core, bt_state(core) == 1 {
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
        updateMeter()

        let now = ProcessInfo.processInfo.systemUptime
        var detail = state.label
        if paused && !editing && tap != nil && idleSeconds > 0 {
            let elapsed = now - lastClick
            let remaining = max(0, Int(ceil(Double(idleSeconds) - elapsed)))
            detail += " · \(remaining) 秒後續錄"
        }
        captureLamp.update(state, now: now, detail: detail)
        let message = notice ?? captureError ?? ""
        if statusLabel.stringValue != message {
            statusLabel.stringValue = message
            statusLabel.toolTip = message.isEmpty ? nil : message
        }
        let hideStatus = waveformOnly || message.isEmpty
        if statusLabel.isHidden != hideStatus { statusLabel.isHidden = hideStatus }
    }

    private func setNotice(_ message: String?) {
        notice = message
        updateStatus()
    }

    func frozenExport() -> WAVExport? {
        guard paused, let core = tap?.core, let snapshot = bt_snapshot(core) else { return nil }
        return WAVExport(snapshot)
    }

    func export(_ range: ClosedRange<Double>, snapshot: WAVExport?, completion: @escaping (URL?) -> Bool) {
        guard let job = snapshot ?? frozenExport() else {
            if completion(nil) { setNotice("還沒有可匯出的錄音") }
            return
        }
        let directory = output
        let sampleRate = exportSampleRate
        let bitsPerSample = exportBitsPerSample
        setNotice("正在寫入 WAV…")
        worker.async { [weak self] in
            let url = job.write(range: range, directory: directory, sampleRate: sampleRate, bitsPerSample: bitsPerSample)
            DispatchQueue.main.async {
                guard completion(url) else { return }
                if let url {
                    self?.setNotice("已存入 \(url.lastPathComponent)")
                } else {
                    self?.setNotice("無法匯出 WAV，請確認有錄音及資料夾寫入權限")
                }
            }
        }
    }
}

private final class WaveformView: NSView, NSDraggingSource {
    weak var controller: BacktrackingController?
    var showsOverlay = true { didSet { needsDisplay = true } }
    var maximumTraceHeight: CGFloat? { didSet { needsDisplay = true } }
    private var low = [Float](repeating: 0, count: 256)
    private var high = [Float](repeating: 0, count: 256)
    private var frames: UInt64 = 0
    private var sampleRate = 0.0
    private var revision: UInt64?
    private var selection: ClosedRange<Double>?
    private var selectedSnapshot: WAVExport?
    private var exportRange: ClosedRange<Double> { selection ?? 0...1 }
    private var anchor: Double?
    private var pointer: NSPoint?
    private var pointerModifiers: NSEvent.ModifierFlags = []
    private var selectionModifierDown: Bool { !pointerModifiers.intersection([.option, .control]).isEmpty }
    private var pointerArea: NSTrackingArea?
    private var mouseDownEvent: NSEvent?
    private var dragRequested = false
    private var dragging = false
    private var readyFile: URL?
    private var preparingFile = false
    private var preparedRange: ClosedRange<Double>?
    private var exportVersion: UInt64 = 0

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

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

    func refresh(core: OpaquePointer, sampleRate: Double) {
        // Keep the selection preview aligned with the retained snapshot while capture continues.
        guard selectedSnapshot == nil else { return }
        let currentRevision = bt_revision(core)
        guard currentRevision != revision || sampleRate != self.sampleRate else { return }
        revision = currentRevision
        self.sampleRate = sampleRate
        let bins = UInt32(low.count)
        frames = low.withUnsafeMutableBufferPointer { minimum in
            high.withUnsafeMutableBufferPointer { maximum in
                bt_waveform(core, minimum.baseAddress!, maximum.baseAddress!, bins)
            }
        }
        needsDisplay = true
    }

    func clearSelection() {
        selection = nil
        selectedSnapshot = nil
        discardPreparedFile()
        needsDisplay = true
    }

    func discardPreparedFile() {
        readyFile = nil
        preparingFile = false
        preparedRange = nil
        exportVersion &+= 1
        needsDisplay = true
    }

    private func normalized(_ event: NSEvent) -> Double {
        let position = convert(event.locationInWindow, from: nil).x
        return Double(min(1, max(0, (position - bounds.minX) / max(1, bounds.width))))
    }

    func refreshHintModifiers() {
        guard pointer != nil, anchor == nil, mouseDownEvent == nil, !dragging else { return }
        let current = NSEvent.modifierFlags.intersection([.option, .control, .command])
        guard current != pointerModifiers else { return }
        pointerModifiers = current
        needsDisplay = true
    }

    private func trackPointer(_ event: NSEvent) {
        let position = convert(event.locationInWindow, from: nil)
        pointer = bounds.contains(position) ? position : nil
        pointerModifiers = event.modifierFlags.intersection([.option, .control, .command])
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
        trackPointer(event)
        let selecting = selectionModifierDown
        if selecting { clearSelection() }
        controller.beginEditing()
        if selecting {
            controller.refreshWaveform()
            anchor = normalized(event)
            selection = anchor.map { $0...$0 }
            needsDisplay = true
            return
        }

        mouseDownEvent = event
        dragRequested = false
        prepareExport(exportRange, reuse: true)
    }

    override func rightMouseDown(with event: NSEvent) {
        if event.modifierFlags.contains(.control) { mouseDown(with: event) }
    }

    override func mouseDragged(with event: NSEvent) {
        trackPointer(event)
        if let anchor {
            let position = normalized(event)
            selection = min(anchor, position)...max(anchor, position)
            needsDisplay = true
        } else if let mouseDownEvent {
            let start = mouseDownEvent.locationInWindow
            let current = event.locationInWindow
            if hypot(current.x - start.x, current.y - start.y) >= 4 {
                dragRequested = true
                tryStartDrag()
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
            if let selection, selection.upperBound - selection.lowerBound > 0.001 {
                if frames != 0 { selectedSnapshot = controller?.frozenExport() }
                keepSelection = selectedSnapshot != nil
                prepareExport(selection)
            } else {
                clearSelection()
            }
        }
        mouseDownEvent = nil
        dragRequested = false
        if !dragging { endGesture(keepingSelection: keepSelection) }
    }

    override func rightMouseUp(with event: NSEvent) {
        if anchor != nil { mouseUp(with: event) }
    }

    private func prepareExport(_ range: ClosedRange<Double>, reuse: Bool = false) {
        if reuse && preparedRange == range {
            if preparingFile { return }
            if let readyFile, FileManager.default.fileExists(atPath: readyFile.path) { return }
        }
        discardPreparedFile()
        preparedRange = range
        preparingFile = true
        let version = exportVersion
        controller?.export(range, snapshot: selectedSnapshot) { [weak self] url in
            guard let self, self.exportVersion == version else { return false }
            self.preparingFile = false
            self.readyFile = url
            self.needsDisplay = true
            self.tryStartDrag()
            return true
        }
    }

    private func tryStartDrag() {
        guard dragRequested, !dragging, let event = mouseDownEvent, let url = readyFile,
              NSEvent.pressedMouseButtons & 1 != 0 else { return }
        dragging = true
        let item = NSDraggingItem(pasteboardWriter: url as NSURL)
        let icon = NSWorkspace.shared.icon(forFile: url.path)
        let point = convert(event.locationInWindow, from: nil)
        item.setDraggingFrame(NSRect(x: point.x - 16, y: point.y - 16, width: 32, height: 32), contents: icon)
        let session = beginDraggingSession(with: [item], event: event, source: self)
        session.animatesToStartingPositionsOnCancelOrFail = true
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
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
        endGesture(keepingSelection: operation.isEmpty && selectedSnapshot != nil)
    }

    private func endGesture(keepingSelection: Bool = false) {
        guard let controller else { return }
        if controller.resumesImmediately && !keepingSelection {
            selection = nil
            selectedSnapshot = nil
            // Allow the current WAV write to finish, but take a fresh snapshot on the next click.
            preparedRange = nil
            needsDisplay = true
        }
        controller.endEditing()
    }

    private func drawActionHint() {
        guard showsOverlay, frames != 0, let pointer, bounds.contains(pointer) else { return }
        if anchor == nil && mouseDownEvent == nil && !dragging && movesFromWaveform(pointerModifiers) { return }
        let action: String
        if anchor != nil {
            action = "Drag to Select"
        } else if mouseDownEvent != nil || dragging {
            action = "Drag and Drop"
        } else if selectionModifierDown {
            action = "Drag to Select"
        } else if selection != nil || readyFile != nil {
            action = "Drag and Drop   Option + Click"
        } else {
            action = "Click   Option + Click"
        }

        let label = NSAttributedString(string: action, attributes: [
            .font: NSFont.monospacedSystemFont(ofSize: 10, weight: .bold),
            .foregroundColor: NSColor.white
        ])
        let textSize = label.size()
        let width = ceil(textSize.width) + 14
        let height = ceil(textSize.height) + 8
        let area = bounds.insetBy(dx: 4, dy: 4)
        guard area.width >= width, area.height >= height else { return }

        let gap: CGFloat = 16
        let centeredX = min(max(pointer.x - width / 2, area.minX), area.maxX - width)
        let centeredY = min(max(pointer.y - height / 2, area.minY), area.maxY - height)
        let candidates = [
            NSRect(x: pointer.x + gap, y: centeredY, width: width, height: height),
            NSRect(x: pointer.x - gap - width, y: centeredY, width: width, height: height),
            NSRect(x: centeredX, y: pointer.y - gap - height, width: width, height: height),
            NSRect(x: centeredX, y: pointer.y + gap, width: width, height: height)
        ]
        let rect = candidates.first { area.contains($0) } ?? NSRect(
            x: min(max(pointer.x + gap, area.minX), area.maxX - width),
            y: centeredY,
            width: width,
            height: height
        )
        let bubble = NSBezierPath(roundedRect: rect, xRadius: 5, yRadius: 5)
        NSColor(calibratedRed: 0.09, green: 0.13, blue: 0.17, alpha: 0.96).setFill()
        bubble.fill()
        NSColor.white.withAlphaComponent(0.20).setStroke()
        bubble.lineWidth = 1
        bubble.stroke()
        label.draw(at: NSPoint(x: rect.minX + 7, y: rect.minY + 4))
    }

    private func waveformRect(for range: ClosedRange<Double>) -> NSRect {
        NSRect(
            x: bounds.minX + bounds.width * CGFloat(range.lowerBound), y: bounds.minY,
            width: bounds.width * CGFloat(range.upperBound - range.lowerBound), height: bounds.height
        )
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        let panel = NSBezierPath(roundedRect: bounds, xRadius: 6, yRadius: 6)
        NSGraphicsContext.saveGraphicsState()
        panel.addClip()
        let middle = bounds.midY
        let height = min(bounds.height, maximumTraceHeight ?? bounds.height) * 0.46
        let preview = selection != nil || pointer != nil || mouseDownEvent != nil || dragging ? exportRange : nil
        let selectedRect = selection.map { waveformRect(for: $0) }
        if showsOverlay, let preview, selectedRect == nil, frames != 0 {
            NSColor.systemTeal.withAlphaComponent(0.23).setFill()
            NSBezierPath.fill(waveformRect(for: preview))
        }

        if showsOverlay {
            NSColor.white.withAlphaComponent(0.17).setStroke()
            NSBezierPath.strokeLine(from: NSPoint(x: 0, y: middle), to: NSPoint(x: bounds.width, y: middle))
        }

        if frames != 0 {
            let trace = NSBezierPath()
            let step = bounds.width / CGFloat(low.count)
            for index in low.indices {
                let x = (CGFloat(index) + 0.5) * step
                let bottom = middle + CGFloat(max(-1, min(1, low[index]))) * height
                let top = middle + CGFloat(max(-1, min(1, high[index]))) * height
                trace.move(to: NSPoint(x: x, y: bottom))
                trace.line(to: NSPoint(x: x, y: max(top, bottom + 0.7)))
            }
            let mint = waveformMint
            NSColor(calibratedRed: mint.red, green: mint.green, blue: mint.blue, alpha: 1).setStroke()
            trace.lineWidth = max(1, step * 0.75)
            if let selectedRect, !selectedRect.isEmpty {
                let outside = NSBezierPath(rect: bounds)
                outside.append(NSBezierPath(rect: selectedRect))
                outside.windingRule = .evenOdd
                NSGraphicsContext.saveGraphicsState()
                outside.addClip()
                trace.stroke()
                NSGraphicsContext.restoreGraphicsState()

                NSGraphicsContext.saveGraphicsState()
                NSBezierPath(rect: selectedRect).addClip()
                NSColor(calibratedRed: 1 - mint.red, green: 1 - mint.green, blue: 1 - mint.blue, alpha: 1).setStroke()
                trace.stroke()
                NSGraphicsContext.restoreGraphicsState()
            } else {
                trace.stroke()
            }
        }
        if showsOverlay, let pointer, anchor != nil || (selectionModifierDown && !movesFromWaveform(pointerModifiers)) {
            NSColor.white.withAlphaComponent(0.4).setStroke()
            NSBezierPath.strokeLine(
                from: NSPoint(x: pointer.x, y: bounds.minY),
                to: NSPoint(x: pointer.x, y: bounds.maxY)
            )
        }
        if showsOverlay, let preview, frames != 0, sampleRate > 0 {
            let seconds = Double(frames) / sampleRate * (preview.upperBound - preview.lowerBound)
            let label = NSAttributedString(string: String(format: "%.2f 秒", seconds), attributes: [
                .font: NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .medium),
                .foregroundColor: NSColor.white
            ])
            let size = label.size()
            let origin = NSPoint(x: bounds.maxX - size.width - 7, y: bounds.maxY - size.height - 4)
            label.draw(at: origin)
        }
        drawActionHint()
        NSGraphicsContext.restoreGraphicsState()
    }
}

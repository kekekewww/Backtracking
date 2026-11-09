import AppKit

private let timeChoices: [UInt32] = [10, 20, 30]

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
    var onClick: (() -> Void)?

    override func sendEvent(_ event: NSEvent) {
        switch event.type {
        case .leftMouseDown, .rightMouseDown, .otherMouseDown: onClick?()
        default: break
        }
        super.sendEvent(event)
    }
}

// Created only from frozen history on the UI thread. The worker owns all access to the copy.
private final class WAVExport: @unchecked Sendable {
    let snapshot: OpaquePointer

    init(_ snapshot: OpaquePointer) {
        self.snapshot = snapshot
    }

    deinit { bt_snapshot_free(snapshot) }

    func write(range: ClosedRange<Double>, directory: String) -> URL? {
        directory.withCString { destination in
            guard let path = bt_snapshot_export(snapshot, range.lowerBound, range.upperBound, destination) else {
                return nil
            }
            defer { bt_string_free(path) }
            return String(validatingUTF8: path).map { URL(fileURLWithPath: $0) }
        }
    }
}

@MainActor
private final class BacktrackingController: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private let defaults = UserDefaults.standard
    private let worker = DispatchQueue(label: "backtracking.wav-export", qos: .userInitiated)
    private var tap: SystemAudioTap?
    private var panel: CapturePanel?
    private var waveform: WaveformView?
    private var timer: Timer?
    private let resumeButton = NSButton(title: "重新連線", target: nil, action: nil)
    private let statusLabel = NSTextField(labelWithString: "")
    private var history: UInt32 = 20
    private var idleSeconds: UInt32 = 0
    private var paused = true
    private var editing = false
    private var lastClick = ProcessInfo.processInfo.systemUptime
    private var notice: String?
    private var output = ""

    func applicationDidFinishLaunching(_ notification: Notification) {
        let storedHistory = defaults.integer(forKey: "history")
        if let matching = timeChoices.first(where: { Int($0) == storedHistory }) {
            history = matching
        }
        let storedIdle = defaults.integer(forKey: "idleSeconds")
        if let matching = timeChoices.first(where: { Int($0) == storedIdle }) {
            idleSeconds = matching
        }
        output = defaults.string(forKey: "outputDirectory") ??
            FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent("Backtracking/output", isDirectory: true).path

        configureMenu()
        configurePanel()
        resume()
        let timer = Timer(timeInterval: 1.0 / 15.0, target: self, selector: #selector(tick(_:)), userInfo: nil, repeats: true)
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    func applicationWillTerminate(_ notification: Notification) {
        timer?.invalidate()
        timer = nil
        tap = nil
    }

    func windowWillClose(_ notification: Notification) {
        NSApp.terminate(nil)
    }

    private func configureMenu() {
        let bar = NSMenu()
        let item = NSMenuItem()
        let menu = NSMenu()
        menu.addItem(withTitle: "結束 Backtracking", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        item.submenu = menu
        bar.addItem(item)
        NSApp.mainMenu = bar
    }

    private func configurePanel() {
        let window = CapturePanel(
            contentRect: NSRect(x: 0, y: 0, width: 590, height: 132),
            styleMask: [.titled, .closable, .resizable, .nonactivatingPanel],
            backing: .buffered, defer: false
        )
        window.title = "Backtracking"
        window.contentMinSize = NSSize(width: 560, height: 120)
        window.delegate = self
        window.becomesKeyOnlyIfNeeded = true
        window.isFloatingPanel = true
        window.hidesOnDeactivate = false
        window.acceptsMouseMovedEvents = true
        window.level = .floating
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        window.onClick = { [weak self] in self?.registerClick() }
        window.setFrameAutosaveName("Backtracking.Panel")

        let waveform = WaveformView(frame: .zero)
        waveform.controller = self
        waveform.heightAnchor.constraint(greaterThanOrEqualToConstant: 51).isActive = true
        self.waveform = waveform

        resumeButton.target = self
        resumeButton.action = #selector(resume)
        resumeButton.bezelStyle = .rounded
        let pinButton = NSButton(checkboxWithTitle: "置頂", target: self, action: #selector(pinChanged(_:)))
        pinButton.state = .on
        let settingsButton = NSButton(title: "設定", target: self, action: #selector(showSettings(_:)))
        settingsButton.bezelStyle = .rounded
        settingsButton.toolTip = "設定"
        if let gear = NSImage(systemSymbolName: "gearshape", accessibilityDescription: "設定") {
            settingsButton.image = gear
            settingsButton.imagePosition = .imageOnly
        }
        let spacer = NSView(frame: .zero)
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let controls = NSStackView(views: [resumeButton, spacer, pinButton, settingsButton])
        controls.orientation = .horizontal
        controls.spacing = 6
        controls.alignment = .centerY

        statusLabel.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        statusLabel.lineBreakMode = .byTruncatingTail
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let stack = NSStackView(views: [controls, waveform, statusLabel])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        guard let content = window.contentView else { return }
        content.addSubview(stack)
        stack.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 10),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -10),
            stack.topAnchor.constraint(equalTo: content.topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -8),
            controls.widthAnchor.constraint(equalTo: stack.widthAnchor),
            waveform.widthAnchor.constraint(equalTo: stack.widthAnchor),
            statusLabel.widthAnchor.constraint(equalTo: stack.widthAnchor)
        ])
        panel = window
        if !window.setFrameUsingName("Backtracking.Panel") {
            window.center()
        }
        window.orderFrontRegardless()
    }

    @objc private func showSettings(_ sender: NSButton) {
        let menu = NSMenu(title: "設定")
        menu.addItem(timeMenu(
            title: "保留時間", choices: timeChoices, selected: history,
            action: #selector(historyChanged(_:))
        ))
        menu.addItem(timeMenu(
            title: "自動續錄", choices: [0] + timeChoices, selected: idleSeconds,
            action: #selector(idleChanged(_:))
        ))
        let outputItem = NSMenuItem(title: "專案資料夾…", action: #selector(chooseOutput), keyEquivalent: "")
        outputItem.target = self
        menu.addItem(outputItem)
        let below = sender.isFlipped ? sender.bounds.maxY + 3 : sender.bounds.minY - 3
        _ = menu.popUp(positioning: nil, at: NSPoint(x: sender.bounds.minX, y: below), in: sender)
    }

    private func timeMenu(title: String, choices: [UInt32], selected: UInt32, action: Selector) -> NSMenuItem {
        let section = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        let submenu = NSMenu(title: title)
        for seconds in choices {
            let item = NSMenuItem(title: seconds == 0 ? "手動" : "\(seconds) 秒", action: action, keyEquivalent: "")
            item.tag = Int(seconds)
            item.target = self
            item.state = seconds == selected ? .on : .off
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
        guard let selected = UInt32(exactly: sender.tag), selected == 0 || timeChoices.contains(selected) else { return }
        idleSeconds = selected
        defaults.set(Int(idleSeconds), forKey: "idleSeconds")
        registerClick()
    }

    @objc private func pinChanged(_ sender: NSButton) {
        let pinned = sender.state == .on
        panel?.isFloatingPanel = pinned
        panel?.hidesOnDeactivate = false
        panel?.level = pinned ? .floating : .normal
    }

    @objc private func chooseOutput() {
        beginEditing()
        let chooser = NSOpenPanel()
        chooser.title = "選擇專案資料夾，WAV 會存到其中的 output"
        chooser.canChooseDirectories = true
        chooser.canChooseFiles = false
        chooser.canCreateDirectories = true
        chooser.allowsMultipleSelection = false
        chooser.directoryURL = URL(fileURLWithPath: output, isDirectory: true).deletingLastPathComponent()
        chooser.begin { [weak self] response in
            guard let self else { return }
            if response == .OK, let url = chooser.url {
                self.output = url.appendingPathComponent("output", isDirectory: true).path
                self.defaults.set(self.output, forKey: "outputDirectory")
                self.waveform?.discardPreparedFile()
                self.setNotice("輸出位置 \(self.output)")
            }
            self.endEditing()
        }
    }

    func registerClick() {
        lastClick = ProcessInfo.processInfo.systemUptime
    }

    func beginEditing() {
        editing = true
        registerClick()
        guard !paused else { return }
        do {
            try tap?.pause()
        } catch {
            setNotice(error.localizedDescription)
        }
        paused = true
        refreshWaveform()
        updateStatus()
    }

    func endEditing() {
        editing = false
        registerClick()
    }

    @objc private func resume() {
        guard paused, !editing else { return }
        do {
            if tap == nil {
                tap = try SystemAudioTap(history: history)
            }
            try tap?.resume()
            waveform?.clearSelection()
            paused = false
            setNotice(nil)
        } catch {
            // The callback gate is still closed after a failed start. Let the user retry.
            paused = true
            setNotice(error.localizedDescription)
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

    private func refreshWaveform() {
        guard let waveform, let tap, let core = tap.core else { return }
        waveform.refresh(core: core, sampleRate: tap.sampleRate)
    }

    private func updateStatus() {
        resumeButton.title = tap == nil ? "重新連線" : "繼續錄音"
        resumeButton.isEnabled = (paused || tap == nil) && !editing
        let state: String
        if paused {
            state = tap == nil ? "未連線" : "已暫停"
        } else if let core = tap?.core, bt_state(core) == 1 {
            state = "錄音中"
        } else {
            state = "等待聲音"
        }
        if paused && !editing && tap != nil && idleSeconds > 0 {
            let elapsed = ProcessInfo.processInfo.systemUptime - lastClick
            let remaining = max(0, Int(ceil(Double(idleSeconds) - elapsed)))
            statusLabel.stringValue = "\(state) · \(remaining) 秒後續錄" + (notice.map { " · \($0)" } ?? "")
        } else {
            statusLabel.stringValue = state + (notice.map { " · \($0)" } ?? "")
        }
    }

    private func setNotice(_ message: String?) {
        notice = message
        updateStatus()
    }

    func export(_ range: ClosedRange<Double>, completion: @escaping (URL?) -> Void) {
        guard paused, let core = tap?.core, let snapshot = bt_snapshot(core) else {
            setNotice("還沒有可匯出的錄音")
            completion(nil)
            return
        }
        let job = WAVExport(snapshot)
        let directory = output
        setNotice("正在寫入 WAV…")
        worker.async { [weak self] in
            let url = job.write(range: range, directory: directory)
            DispatchQueue.main.async {
                if let url {
                    self?.setNotice("已存入 \(url.lastPathComponent)")
                } else {
                    self?.setNotice("無法匯出 WAV，請確認有錄音及資料夾寫入權限")
                }
                completion(url)
            }
        }
    }
}

private final class WaveformView: NSView, NSDraggingSource {
    weak var controller: BacktrackingController?
    private var low = [Float](repeating: 0, count: 256)
    private var high = [Float](repeating: 0, count: 256)
    private var frames: UInt64 = 0
    private var sampleRate = 0.0
    private var revision: UInt64?
    private var selection: ClosedRange<Double>?
    private var anchor: Double?
    private var hover: Double?
    private var pointer: NSPoint?
    private var selectionModifierDown = false
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
        discardPreparedFile()
        needsDisplay = true
    }

    func discardPreparedFile() {
        readyFile = nil
        preparingFile = false
        preparedRange = nil
        exportVersion &+= 1
    }

    private func normalized(_ event: NSEvent) -> Double {
        let position = convert(event.locationInWindow, from: nil).x
        return Double(min(1, max(0, (position - bounds.minX) / max(1, bounds.width))))
    }

    private func selectionModifier(_ flags: NSEvent.ModifierFlags) -> Bool {
        flags.contains(.option) || flags.contains(.control)
    }

    func refreshHintModifiers() {
        guard pointer != nil, anchor == nil, mouseDownEvent == nil, !dragging else { return }
        let isDown = selectionModifier(NSEvent.modifierFlags)
        guard isDown != selectionModifierDown else { return }
        selectionModifierDown = isDown
        needsDisplay = true
    }

    private func trackPointer(_ event: NSEvent) {
        let position = convert(event.locationInWindow, from: nil)
        pointer = bounds.contains(position) ? position : nil
        selectionModifierDown = selectionModifier(event.modifierFlags)
        needsDisplay = true
    }

    private func trackDragPointer() {
        guard let window else { return }
        let position = convert(window.mouseLocationOutsideOfEventStream, from: nil)
        pointer = bounds.contains(position) ? position : nil
        needsDisplay = true
    }

    override func mouseEntered(with event: NSEvent) { mouseMoved(with: event) }

    override func mouseMoved(with event: NSEvent) {
        hover = normalized(event)
        trackPointer(event)
    }

    override func mouseExited(with event: NSEvent) {
        hover = nil
        pointer = nil
        needsDisplay = true
    }

    override func mouseDown(with event: NSEvent) {
        guard !dragging, let controller else { return }
        trackPointer(event)
        controller.beginEditing()
        if selectionModifierDown {
            clearSelection()
            anchor = normalized(event)
            selection = anchor.map { $0...$0 }
            needsDisplay = true
            return
        }

        mouseDownEvent = event
        dragRequested = false
        if let selection {
            prepareExport(selection, reuse: true)
        } else {
            let lastFrame = 1 - 1 / Double(max(1, frames))
            prepareExport(min(lastFrame, normalized(event))...1)
        }
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
        if anchor != nil {
            mouseDragged(with: event)
            anchor = nil
            if let selection, selection.upperBound - selection.lowerBound > 0.001 {
                prepareExport(selection)
            } else {
                clearSelection()
            }
        }
        mouseDownEvent = nil
        dragRequested = false
        if !dragging { controller?.endEditing() }
    }

    override func rightMouseUp(with event: NSEvent) {
        if anchor != nil { mouseUp(with: event) }
    }

    private func prepareExport(_ range: ClosedRange<Double>, reuse: Bool = false) {
        if reuse && preparedRange == range && (preparingFile || readyFile != nil) { return }
        discardPreparedFile()
        preparedRange = range
        preparingFile = true
        let version = exportVersion
        controller?.export(range) { [weak self] url in
            guard let self, self.exportVersion == version else { return }
            self.preparingFile = false
            self.readyFile = url
            self.needsDisplay = true
            self.tryStartDrag()
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
        controller?.endEditing()
    }

    private func drawActionHint() {
        guard frames != 0, let pointer, bounds.contains(pointer) else { return }
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

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        let panel = NSBezierPath(roundedRect: bounds, xRadius: 6, yRadius: 6)
        NSColor(calibratedRed: 0.07, green: 0.09, blue: 0.12, alpha: 1).setFill()
        panel.fill()
        NSGraphicsContext.saveGraphicsState()
        panel.addClip()
        let middle = bounds.midY
        let height = bounds.height * 0.46
        let preview = selection ?? hover.map { $0...1 }
        if let preview, frames != 0 {
            NSColor.systemTeal.withAlphaComponent(0.23).setFill()
            NSBezierPath.fill(NSRect(
                x: bounds.width * CGFloat(preview.lowerBound), y: 0,
                width: bounds.width * CGFloat(preview.upperBound - preview.lowerBound),
                height: bounds.height
            ))
        }

        NSColor.white.withAlphaComponent(0.17).setStroke()
        NSBezierPath.strokeLine(from: NSPoint(x: 0, y: middle), to: NSPoint(x: bounds.width, y: middle))

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
            NSColor(calibratedRed: 0.32, green: 0.89, blue: 0.82, alpha: 1).setStroke()
            trace.lineWidth = max(1, step * 0.75)
            trace.stroke()
        }
        if let hover {
            NSColor.white.withAlphaComponent(0.4).setStroke()
            let x = CGFloat(hover) * bounds.width
            NSBezierPath.strokeLine(from: NSPoint(x: x, y: 0), to: NSPoint(x: x, y: bounds.height))
        }
        if let preview, frames != 0, sampleRate > 0 {
            let seconds = Double(frames) / sampleRate * (preview.upperBound - preview.lowerBound)
            let label = NSAttributedString(string: String(format: "%.2f 秒", seconds), attributes: [
                .font: NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .medium),
                .foregroundColor: NSColor.white
            ])
            let size = label.size()
            label.draw(at: NSPoint(x: bounds.width - size.width - 7, y: bounds.height - size.height - 4))
        }
        drawActionHint()
        NSGraphicsContext.restoreGraphicsState()
    }
}

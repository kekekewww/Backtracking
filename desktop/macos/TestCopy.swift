import AppKit
import Carbon
import Darwin
import Foundation

struct MessageError: Error, CustomStringConvertible, ExpressibleByStringLiteral, ExpressibleByStringInterpolation {
    let message: String
    init(_ message: String) { self.message = message }
    init(stringLiteral value: String) { self.message = value }
    var description: String { message }
}

enum AppLanguage: Equatable {
    case english
    case zhHant

    static var current: AppLanguage { TestCopyConfig.current.language }
}

final class TestCopyConfig: @unchecked Sendable {
    static let instanceVariable = "BACKTRACKING_TEST_INSTANCE"
    static let dataDirVariable = "BACKTRACKING_TEST_DATA_DIR"
    static let localAppDataVariable = "LOCALAPPDATA"
    static let windowSizeVariable = "BACKTRACKING_TEST_WINDOW_SIZE"
    static let windowPositionVariable = "BACKTRACKING_TEST_WINDOW_POSITION"
    static let languageVariable = "BACKTRACKING_TEST_LANGUAGE"
    static let systemFormatVariable = "BACKTRACKING_TEST_SYSTEM_FORMAT"
    static let screenshotVariable = "BACKTRACKING_TEST_SCREENSHOT"
    static let screenshotDelayVariable = "BACKTRACKING_TEST_SCREENSHOT_DELAY_MS"
    static let viewVariable = "BACKTRACKING_TEST_VIEW"
    static let showOptionsArgument = "--show-options"
    /// Marks a window a test copy opened. Such a launch refuses to start unless it is a test copy
    /// itself, so a lost environment can never open an instance on the user's real data.
    static let testCopyLaunchArgument = "--backtracking-test-copy-window"

    static let current = TestCopyConfig()

    /// The variables a window opened from this process needs to be the same test copy. On Windows
    /// the new process inherits the whole environment; Launch Services does not, so they are passed on.
    static func inheritedTestEnvironment(_ environment: [String: String]) -> [String: String] {
        environment.filter { key, _ in key.hasPrefix("BACKTRACKING_TEST_") || key == localAppDataVariable }
    }

    /// A test copy neither saves nor restores window state. The argument domain outranks the
    /// app's, the global and the registration domains, and like registration it lives only in
    /// memory, so nothing is written for it.
    static func turnOffStateRestoration(_ defaults: UserDefaults) {
        var arguments = defaults.volatileDomain(forName: UserDefaults.argumentDomain)
        arguments["ApplePersistenceIgnoreState"] = true
        arguments["NSQuitAlwaysKeepsWindows"] = false
        defaults.setVolatileDomain(arguments, forName: UserDefaults.argumentDomain)
    }

    static let productionBundleID = "com.backtracking.audio.capture"
    static let testCopyBundlePrefix = "com.backtracking.audio.capture.testcopy."

    /// Why a test copy cannot run from a bundle carrying the installed app's identifier, or nil.
    /// AppKit keeps panel, window and restore state under the bundle identifier, outside what
    /// `PreferencesStore` isolates, so a test copy runs only from a copy made by
    /// `desktop/macos/make-test-copy.sh`, which gives it its own identifier.
    static func productionBundleRefusal(bundleID: String?) -> String? {
        guard bundleID == productionBundleID else { return nil }
        return "it runs from the installed app's bundle (\(productionBundleID)), whose preferences it would share. Make a test copy with desktop/macos/make-test-copy.sh and start that instead."
    }

    /// Why a bundle made by make-test-copy.sh cannot run as the installed app, or nil. Started
    /// without the test variables (from the Finder, say) it would use the user's real data.
    static func unisolatedTestBundleRefusal(bundleID: String?, isTestCopy: Bool) -> String? {
        guard !isTestCopy, let bundleID, bundleID.hasPrefix(testCopyBundlePrefix) else { return nil }
        return "\(bundleID) is a test copy. Start it with \(instanceVariable) and \(dataDirVariable) set; it does not run on the user's own data."
    }

    /// Whether a launch marked as opened by a test copy has to stop because it is not one.
    static func refusesUnisolatedLaunch(arguments: [String], isTestCopy: Bool) -> Bool {
        arguments.contains(testCopyLaunchArgument) && !isTestCopy
    }

    let instanceName: String?
    let invalidInstanceName: Bool
    let dataDirectory: URL?
    let dataDirectoryError: String?
    let showOptionsOnStart: Bool
    let windowSizeOverride: NSSize?
    let windowPositionOverride: NSPoint?
    let language: AppLanguage
    let syntheticSystemFormat: String?
    let screenshotPath: URL?
    let screenshotDelayMs: UInt64
    let viewScript: String?
    private var lockFD: Int32 = -1

    var isTestCopy: Bool { instanceName != nil }

    var preferencesURL: URL? {
        guard let dataDirectory else { return nil }
        let nested = dataDirectory
            .appendingPathComponent("Backtracking", isDirectory: true)
            .appendingPathComponent("preferences.plist")
        let flat = dataDirectory.appendingPathComponent("preferences.plist")
        if !FileManager.default.fileExists(atPath: nested.path),
           FileManager.default.fileExists(atPath: flat.path) {
            return flat
        }
        return nested
    }

    var addonDataRoot: URL {
        if let dataDirectory {
            return dataDirectory.appendingPathComponent("Backtracking", isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/Backtracking", isDirectory: true)
    }

    func defaultExportDirectory(component: String) -> String {
        if let dataDirectory {
            return dataDirectory
                .appendingPathComponent("Backtracking", isDirectory: true)
                .appendingPathComponent(component, isDirectory: true).path
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Backtracking/\(component)", isDirectory: true).path
    }

    func windowTitle(_ base: String) -> String {
        isTestCopy ? "\(base) (test copy)" : base
    }

    static func isValidInstanceName(_ name: String) -> Bool {
        guard (1...40).contains(name.utf8.count) else { return false }
        return name.utf8.allSatisfy { byte in
            (48...57).contains(byte) || (65...90).contains(byte) || (97...122).contains(byte)
                || byte == 45 || byte == 95
        }
    }

    static func realUserHomeDirectory() -> URL {
        if let pw = getpwuid(getuid()), let dir = pw.pointee.pw_dir {
            return URL(fileURLWithPath: String(cString: dir), isDirectory: true).resolvingSymlinksInPath()
        }
        return FileManager.default.homeDirectoryForCurrentUser.resolvingSymlinksInPath()
    }

    static func validateDataDirectory(_ requestedPath: String?, realHome: URL = realUserHomeDirectory()) -> Result<URL, MessageError> {
        let refusal = MessageError("A test copy needs \(dataDirVariable) (or \(localAppDataVariable)) set to a separate, existing folder.")
        guard let raw = requestedPath?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty else {
            return .failure(refusal)
        }
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: raw, isDirectory: &isDirectory), isDirectory.boolValue else {
            return .failure(refusal)
        }
        let resolved = URL(fileURLWithPath: raw, isDirectory: true).resolvingSymlinksInPath().standardizedFileURL
        let home = realHome.resolvingSymlinksInPath().standardizedFileURL
        let forbidden: [URL] = [
            home,
            home.appendingPathComponent("Library", isDirectory: true),
            home.appendingPathComponent("Library/Preferences", isDirectory: true),
            home.appendingPathComponent("Library/Application Support", isDirectory: true),
            home.appendingPathComponent("Library/Application Support/Backtracking", isDirectory: true),
            home.appendingPathComponent("Backtracking", isDirectory: true),
            home.appendingPathComponent("Backtracking/output", isDirectory: true),
            home.appendingPathComponent("Backtracking/saved", isDirectory: true),
        ].map { $0.resolvingSymlinksInPath().standardizedFileURL }

        let resolvedPath = resolved.path
        for bad in forbidden {
            if resolvedPath == bad.path {
                return .failure(refusal)
            }
        }
        // Nor a folder inside the places that hold the user's real settings and recordings.
        let realData = [
            home.appendingPathComponent("Library/Preferences", isDirectory: true),
            home.appendingPathComponent("Library/Application Support/Backtracking", isDirectory: true),
            home.appendingPathComponent("Backtracking", isDirectory: true),
        ].map { $0.resolvingSymlinksInPath().standardizedFileURL.path }
        if realData.contains(where: { resolvedPath.hasPrefix($0.hasSuffix("/") ? $0 : $0 + "/") }) {
            return .failure(refusal)
        }
        // Reject ancestor folders of the user's home (e.g. "/" or "/Users").
        if home.path == resolvedPath || home.path.hasPrefix(resolvedPath.hasSuffix("/") ? resolvedPath : resolvedPath + "/") {
            return .failure(refusal)
        }
        return .success(resolved)
    }

    static func parseWindowSize(_ text: String?) -> NSSize? {
        guard let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else { return nil }
        let parts = text.lowercased().split(separator: "x", maxSplits: 1)
        guard parts.count == 2,
              let width = Double(parts[0].trimmingCharacters(in: .whitespaces)),
              let height = Double(parts[1].trimmingCharacters(in: .whitespaces)),
              width.isFinite, height.isFinite,
              width > 0, height > 0 else { return nil }
        return NSSize(width: min(4096, width), height: min(2160, height))
    }

    static func parseWindowPosition(_ text: String?) -> NSPoint? {
        guard let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else { return nil }
        let parts = text.split(separator: ",", maxSplits: 1)
        guard parts.count == 2,
              let x = Double(parts[0].trimmingCharacters(in: .whitespaces)),
              let y = Double(parts[1].trimmingCharacters(in: .whitespaces)),
              x.isFinite, y.isFinite else { return nil }
        return NSPoint(x: x, y: y)
    }

    init(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        arguments: [String] = CommandLine.arguments
    ) {
        showOptionsOnStart = arguments.contains(Self.showOptionsArgument)
        if let rawInstance = environment[Self.instanceVariable] {
            if Self.isValidInstanceName(rawInstance) {
                instanceName = rawInstance
                invalidInstanceName = false
            } else {
                instanceName = nil
                invalidInstanceName = true
            }
        } else {
            instanceName = nil
            invalidInstanceName = false
        }

        if instanceName != nil {
            let requested = environment[Self.dataDirVariable].flatMap { $0.isEmpty ? nil : $0 }
                ?? environment[Self.localAppDataVariable]
            switch Self.validateDataDirectory(requested) {
            case .success(let url):
                dataDirectory = url
                dataDirectoryError = nil
            case .failure(let error):
                dataDirectory = nil
                dataDirectoryError = error.message
            }
        } else {
            dataDirectory = nil
            dataDirectoryError = nil
        }

        let testCopy = instanceName != nil
        if testCopy || showOptionsOnStart {
            windowSizeOverride = Self.parseWindowSize(environment[Self.windowSizeVariable])
        } else {
            windowSizeOverride = nil
        }
        windowPositionOverride = testCopy ? Self.parseWindowPosition(environment[Self.windowPositionVariable]) : nil
        if testCopy,
           let lang = environment[Self.languageVariable]?.trimmingCharacters(in: .whitespacesAndNewlines),
           lang.caseInsensitiveCompare("zh-Hant") == .orderedSame {
            language = .zhHant
        } else {
            language = .english
        }
        if testCopy, let spec = environment[Self.systemFormatVariable]?.trimmingCharacters(in: .whitespacesAndNewlines),
           !spec.isEmpty {
            syntheticSystemFormat = spec
        } else {
            syntheticSystemFormat = nil
        }
        if testCopy, let shot = environment[Self.screenshotVariable]?.trimmingCharacters(in: .whitespacesAndNewlines),
           !shot.isEmpty {
            screenshotPath = URL(fileURLWithPath: shot)
        } else {
            screenshotPath = nil
        }
        if let delayText = environment[Self.screenshotDelayVariable]?.trimmingCharacters(in: .whitespacesAndNewlines),
           let parsed = UInt64(delayText) {
            screenshotDelayMs = parsed
        } else {
            screenshotDelayMs = 4000
        }
        viewScript = testCopy ? environment[Self.viewVariable] : nil
    }

    /// Validates test copy startup requirements. Exits with code 2 on invalid config, or returns
    /// false when another primary instance with the same test name is already running.
    func validateAndAcquireLock(primary: Bool) -> Bool {
        if invalidInstanceName {
            fputs("Backtracking test copy could not start: \(Self.instanceVariable) takes 1 to 40 letters, digits, - or _.\n", stderr)
            exit(2)
        }
        guard let instanceName else { return true }
        if let dataDirectoryError {
            fputs("Backtracking test copy could not start: \(dataDirectoryError)\n", stderr)
            exit(2)
        }
        guard primary, let dataDirectory else { return true }
        let lockPath = dataDirectory.appendingPathComponent(".backtracking-test-\(instanceName).lock").path
        let fd = open(lockPath, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR)
        if fd < 0 {
            fputs("Backtracking test copy could not start: could not create instance lock at \(lockPath).\n", stderr)
            exit(2)
        }
        if flock(fd, LOCK_EX | LOCK_NB) != 0 {
            close(fd)
            return false
        }
        lockFD = fd
        return true
    }

    func releaseLock() {
        if lockFD >= 0 {
            flock(lockFD, LOCK_UN)
            close(lockFD)
            lockFD = -1
        }
    }
}

/// Abstracts preference persistence so a test copy never touches `UserDefaults.standard`
/// or `~/Library/Preferences/com.backtracking.audio.capture.plist`.
final class PreferencesStore {
    private enum Storage {
        case userDefaults(UserDefaults, cleanupSuite: String?)
        case file(URL)
    }

    private let storage: Storage
    private var entries: [String: Any]

    private init(storage: Storage, entries: [String: Any]) {
        self.storage = storage
        self.entries = entries
    }

    static func standard() -> PreferencesStore {
        PreferencesStore(storage: .userDefaults(.standard, cleanupSuite: nil), entries: [:])
    }

    static func receiver() -> PreferencesStore {
        let name = "com.backtracking.audio.capture.receiver.\(UUID().uuidString)"
        guard let preferences = UserDefaults(suiteName: name) else { return .standard() }
        preferences.register(defaults: UserDefaults.standard.dictionaryRepresentation())
        return PreferencesStore(storage: .userDefaults(preferences, cleanupSuite: name), entries: [:])
    }

    static func file(at url: URL) -> PreferencesStore {
        var loaded: [String: Any] = [:]
        if let data = try? Data(contentsOf: url),
           let plist = try? PropertyListSerialization.propertyList(from: data, options: [], format: nil),
           let dict = plist as? [String: Any] {
            loaded = dict
        }
        return PreferencesStore(storage: .file(url), entries: loaded)
    }

    static func forCurrentProcess(independentWindow: Bool) -> PreferencesStore {
        if let url = TestCopyConfig.current.preferencesURL {
            return .file(at: url)
        }
        return independentWindow ? .receiver() : .standard()
    }

    func object(forKey key: String) -> Any? {
        switch storage {
        case .userDefaults(let ud, _): return ud.object(forKey: key)
        case .file: return entries[key]
        }
    }

    func bool(forKey key: String) -> Bool {
        switch storage {
        case .userDefaults(let ud, _): return ud.bool(forKey: key)
        case .file:
            if let value = entries[key] as? Bool { return value }
            if let number = entries[key] as? NSNumber { return number.boolValue }
            return false
        }
    }

    func string(forKey key: String) -> String? {
        switch storage {
        case .userDefaults(let ud, _): return ud.string(forKey: key)
        case .file: return entries[key] as? String
        }
    }

    func data(forKey key: String) -> Data? {
        switch storage {
        case .userDefaults(let ud, _): return ud.data(forKey: key)
        case .file: return entries[key] as? Data
        }
    }

    func array(forKey key: String) -> [Any]? {
        switch storage {
        case .userDefaults(let ud, _): return ud.array(forKey: key)
        case .file: return entries[key] as? [Any]
        }
    }

    func dictionary(forKey key: String) -> [String: Any]? {
        switch storage {
        case .userDefaults(let ud, _): return ud.dictionary(forKey: key)
        case .file: return entries[key] as? [String: Any]
        }
    }

    func set(_ value: Any?, forKey key: String) {
        switch storage {
        case .userDefaults(let ud, _):
            if let value { ud.set(value, forKey: key) }
            else { ud.removeObject(forKey: key) }
        case .file(let url):
            if let value { entries[key] = value }
            else { entries.removeValue(forKey: key) }
            persist(to: url)
        }
    }

    func removeObject(forKey key: String) {
        set(nil, forKey: key)
    }

    func cleanup() {
        if case .userDefaults(let ud, let suite?) = storage {
            ud.removePersistentDomain(forName: suite)
        }
    }

    private func persist(to url: URL) {
        let directory = url.deletingLastPathComponent()
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                                                 attributes: [.posixPermissions: 0o700])
        if let data = try? PropertyListSerialization.data(fromPropertyList: entries, format: .xml, options: 0) {
            try? data.write(to: url, options: .atomic)
        }
    }
}

// MARK: - Unified Action Layer (TEST-18 / ADD-W26 foundation)

struct TestActionRefusal: Error, Equatable {
    let code: String
    let message: String
}

enum TestWindowAction {
    case startCapture
    case pauseCapture
    case clearHistory
    case selectBack(from: Double, to: Double)
    case selectFrames(Range<UInt64>)
    case clearSelection
    case separate(Bool)
    case chooseLane(Int)
    case chooseRows(firstLane: Int, firstRow: Int, lastLane: Int, lastRow: Int)
    case zoomNewest(Double)
    case view(Range<UInt64>)
    case viewAll
    case save(folder: URL)
    case dragPrepare
    case previewPlan
    case playPreview
    case stopPreview
    case addSource(CaptureSource)
    case removeSource(Int)
}

// MARK: - Test Harness (TEST-07..TEST-15)

enum TestStep: Equatable {
    case zoom(Double)
    case select(Double, Double)
    case selectLane(Int)
    case rows(Int, Int, Int, Int)
    case separate(Bool)
    case save(URL)
    case dragPrepare(URL)
    case exportMode(UInt32)
    case openExportMode
    case previewPlan
    case scroll(Int, Int)
    case group(Int, String)
    case hideSilent(Bool)
    case pair(Bool)
    case options(String, NSSize?)
    case optionsScroll(CGFloat)
    case bind(RecorderAction, [RecorderShortcut])
    case key(RecorderShortcut)
    case openMeters
    case wait(UInt64)
    case look(String?)
    case waveformColor(Bool)
    case gradient(FrequencyGradient?)
    case colorWait(Bool)
    case addSource(String)
    case removeSource(Int)
    case dropPackage(URL)
    case dropFile(URL)
    case installConfirm
    case allowControl(String, Bool)
    case setAddonSetting(String, String, String)
    case runAddonAction(String, String)
    case selectAddonFormat(Int, String)
    case addonTransport(Int, String, Double?)
    case removeAddon(String, Bool)
    case unsupportedAddon(String)

    var waits: Bool {
        switch self {
        case .save, .dragPrepare, .exportMode, .wait, .previewPlan,
             .rows, .selectLane, .key, .addSource, .removeSource, .optionsScroll,
             .dropPackage, .dropFile, .installConfirm, .allowControl,
             .setAddonSetting, .runAddonAction, .selectAddonFormat, .addonTransport, .removeAddon:
            return true
        default:
            return false
        }
    }
}

struct WavHeaderInfo {
    let channels: UInt16
    let sampleRate: UInt32
    let bitsPerSample: UInt16
    let frames: UInt64

    static func parse(_ data: Data) -> WavHeaderInfo? {
        guard data.count >= 44 else { return nil }
        func u16(_ offset: Int) -> UInt16? {
            guard offset + 2 <= data.count else { return nil }
            return UInt16(data[offset]) | (UInt16(data[offset + 1]) << 8)
        }
        func u32(_ offset: Int) -> UInt32? {
            guard offset + 4 <= data.count else { return nil }
            return UInt32(data[offset])
                | (UInt32(data[offset + 1]) << 8)
                | (UInt32(data[offset + 2]) << 16)
                | (UInt32(data[offset + 3]) << 24)
        }
        func u64(_ offset: Int) -> UInt64? {
            guard let lo = u32(offset), let hi = u32(offset + 4) else { return nil }
            return UInt64(lo) | (UInt64(hi) << 32)
        }
        let riff = String(decoding: data[0..<4], as: UTF8.self)
        let wave = String(decoding: data[8..<12], as: UTF8.self)
        guard (riff == "RIFF" || riff == "RF64"), wave == "WAVE" else { return nil }
        var fmt: (UInt16, UInt32, UInt16)?
        var bigData: UInt64?
        var dataBytes: UInt64?
        var at = 12
        while at + 8 <= data.count {
            let id = String(decoding: data[at..<(at + 4)], as: UTF8.self)
            guard let size = u32(at + 4) else { return nil }
            let sizeInt = Int(size)
            switch id {
            case "fmt ":
                if let ch = u16(at + 10), let rate = u32(at + 12), let bits = u16(at + 22) {
                    fmt = (ch, rate, bits)
                }
            case "ds64":
                bigData = u64(at + 16)
            case "data":
                dataBytes = size == UInt32.max ? bigData : UInt64(size)
                at = data.count
                continue
            default:
                break
            }
            at += 8 + sizeInt + (sizeInt % 2)
        }
        guard let (ch, rate, bits) = fmt, let bytes = dataBytes else { return nil }
        let frameBytes = UInt64(ch) * UInt64(max(1, bits / 8))
        return WavHeaderInfo(channels: ch, sampleRate: rate, bitsPerSample: bits, frames: bytes / max(1, frameBytes))
    }
}

@MainActor
final class TestHarness {
    static let warmUpSeconds: TimeInterval = 1.5
    static let settleSeconds: TimeInterval = 0.5

    let output: URL
    let started: TimeInterval
    let delaySeconds: TimeInterval
    var steps: [TestStep]
    var appliedAt: TimeInterval?
    var holdSeconds: TimeInterval = 0
    var frameDurations: [Double] = []
    var firstTimed: TimeInterval?
    var lastTimed: TimeInterval?
    var notes: [String] = []
    var optionsPicture: Bool
    var optionsSize: NSSize?
    var dragCopyFolder: URL?
    var colorWait = true
    var finished = false
    var freezeRequestedAt: TimeInterval?
    var freezeReceivedAt: TimeInterval?

    init?(config: TestCopyConfig = .current) {
        guard let output = config.screenshotPath else { return nil }
        self.output = output
        self.started = ProcessInfo.processInfo.systemUptime
        self.delaySeconds = Double(config.screenshotDelayMs) / 1000.0
        var notes: [String] = []
        let parsed: [TestStep]
        if let script = config.viewScript, !script.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            switch Self.parseSteps(script) {
            case .success(let list): parsed = list
            case .failure(let error):
                notes.append("\(TestCopyConfig.viewVariable) ignored: \(error)")
                parsed = []
            }
        } else {
            parsed = []
        }
        self.steps = parsed
        self.notes = notes
        self.optionsPicture = parsed.contains {
            if case .options = $0 { return true }
            return false
        }
        self.optionsSize = parsed.compactMap { step -> NSSize? in
            if case .options(_, let size) = step { return size }
            return nil
        }.first
    }

    func recordFrameWork(_ duration: TimeInterval, now: TimeInterval) {
        guard now - started >= Self.warmUpSeconds, appliedAt == nil else { return }
        frameDurations.append(duration)
        if firstTimed == nil { firstTimed = now }
        lastTimed = now
    }

    func note(_ text: String) {
        notes.append(text)
    }

    func noteSaved(_ urls: [URL]) {
        for url in urls {
            if let data = try? Data(contentsOf: url), let header = WavHeaderInfo.parse(data) {
                let seconds = Double(header.frames) / Double(max(1, header.sampleRate))
                notes.append(String(
                    format: "saved %@: %u channels, %u Hz, %u bits, %llu frames (%.3f s)",
                    url.path, header.channels, header.sampleRate, header.bitsPerSample, header.frames, seconds
                ))
            } else {
                notes.append("saved \(url.path) (not a WAV file this can read)")
            }
        }
    }

    /// Captures `view` as an 8-bit RGB PNG (alpha composited over black) and returns its pixel size.
    static func captureRGBPNG(of view: NSView, to url: URL) -> Result<(Int, Int), MessageError> {
        view.layoutSubtreeIfNeeded()
        let bounds = view.bounds
        guard bounds.width > 0, bounds.height > 0 else {
            return .failure("window view has zero size")
        }
        guard let rep = view.bitmapImageRepForCachingDisplay(in: bounds) else {
            return .failure("could not create bitmap representation")
        }
        view.cacheDisplay(in: bounds, to: rep)
        guard let cgImage = rep.cgImage else {
            return .failure("could not extract CGImage from view")
        }
        let width = cgImage.width
        let height = cgImage.height
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        guard let context = CGContext(
            data: nil, width: width, height: height,
            bitsPerComponent: 8, bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
        ) else {
            return .failure("could not create RGB bitmap context")
        }
        context.setFillColor(red: 0, green: 0, blue: 0, alpha: 1)
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
        guard let rgbImage = context.makeImage() else {
            return .failure("could not finalize RGB image")
        }
        let outRep = NSBitmapImageRep(cgImage: rgbImage)
        guard let png = outRep.representation(using: .png, properties: [:]) else {
            return .failure("could not encode PNG")
        }
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try png.write(to: url, options: .atomic)
            return .success((width, height))
        } catch {
            return .failure(MessageError(error.localizedDescription))
        }
    }

    func writeLog(picture: Result<(Int, Int), MessageError>) {
        var lines: [String] = []
        switch picture {
        case .success(let (w, h)):
            lines.append("picture \(output.path) (\(w) x \(h) px)")
        case .failure(let error):
            lines.append("picture failed: \(error)")
        }
        let sorted = frameDurations.sorted()
        if let first = firstTimed, let last = lastTimed, !sorted.isEmpty {
            let span = max(0, last - first)
            func at(_ fraction: Double) -> Double {
                let idx = Int((Double(sorted.count - 1) * fraction).rounded())
                return sorted[min(sorted.count - 1, max(0, idx))] * 1000.0
            }
            let mean = (sorted.reduce(0, +) / Double(sorted.count)) * 1000.0
            let fps = span > 0 ? Double(max(0, sorted.count - 1)) / span : 0.0
            lines.append(String(format: "frames %d in %.2f s (%.1f per second)", sorted.count, span, fps))
            lines.append(String(format: "frame work ms: mean %.2f, median %.2f, 95th percentile %.2f, max %.2f",
                                mean, at(0.5), at(0.95), at(1.0)))
        } else {
            lines.append("no frames were timed")
        }
        lines.append(contentsOf: notes)
        let logURL = URL(fileURLWithPath: output.path + ".log")
        try? (lines.joined(separator: "\n") + "\n").write(to: logURL, atomically: true, encoding: .utf8)
    }

    static func parseLane(_ text: String) -> Int? {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        if let index = Int(trimmed), (0..<maximumCaptureLanes).contains(index) {
            return index
        }
        let upper = trimmed.uppercased()
        for i in 0..<maximumCaptureLanes where captureLaneLetter(i) == upper {
            return i
        }
        return nil
    }

    static func parseActionID(_ text: String) -> RecorderAction? {
        let normalized = text.trimmingCharacters(in: .whitespaces).lowercased()
        switch normalized {
        case "global-snapshot": return .globalSnapshot
        case "toggle-capture": return .toggleCapture
        case "clear": return .clear
        case "cancel-selection": return .cancelSelection
        case "separate-lanes", "edit-separately": return .separateLanes
        case "save-snapshot": return .saveSnapshot
        case "waveform-only": return .waveformOnly
        case "keep-on-top": return .keepOnTop
        case "open-options": return .openOptions
        case "quit": return .quit
        default: return nil
        }
    }

    static func parseBinding(_ text: String) -> RecorderShortcut? {
        let trimmed = text.trimmingCharacters(in: .whitespaces).lowercased()
        if trimmed == "right-ctrl-tap" || trimmed == "right-command-tap" || trimmed == "right-cmd-tap" {
            return .rightCommand
        }
        let tokens = trimmed.split(separator: "+").map { String($0).trimmingCharacters(in: .whitespaces) }
        guard let keyToken = tokens.last, !keyToken.isEmpty else { return nil }
        var modifiers: NSEvent.ModifierFlags = []
        for token in tokens.dropLast() {
            switch token {
            case "ctrl", "control": modifiers.insert(.control)
            case "alt", "opt", "option": modifiers.insert(.option)
            case "shift": modifiers.insert(.shift)
            case "cmd", "command", "meta": modifiers.insert(.command)
            case "fn", "globe": modifiers.insert(.function)
            default: return nil
            }
        }
        let mapping: [String: (UInt16, String)] = [
            "space": (UInt16(kVK_Space), "Space"),
            "esc": (UInt16(kVK_Escape), "Esc"),
            "escape": (UInt16(kVK_Escape), "Esc"),
            "backspace": (UInt16(kVK_Delete), "Backspace"),
            "delete": (UInt16(kVK_ForwardDelete), "Forward Delete"),
            "forward-delete": (UInt16(kVK_ForwardDelete), "Forward Delete"),
            "enter": (UInt16(kVK_Return), "Return"),
            "return": (UInt16(kVK_Return), "Return"),
            "tab": (UInt16(kVK_Tab), "Tab"),
            "key-left-of-1": (RecorderShortcut.leftOfOne, "Key left of 1"),
            "left": (UInt16(kVK_LeftArrow), "←"),
            "right": (UInt16(kVK_RightArrow), "→"),
            "up": (UInt16(kVK_UpArrow), "↑"),
            "down": (UInt16(kVK_DownArrow), "↓"),
            "a": (UInt16(kVK_ANSI_A), "A"), "b": (UInt16(kVK_ANSI_B), "B"),
            "c": (UInt16(kVK_ANSI_C), "C"), "d": (UInt16(kVK_ANSI_D), "D"),
            "e": (UInt16(kVK_ANSI_E), "E"), "f": (UInt16(kVK_ANSI_F), "F"),
            "g": (UInt16(kVK_ANSI_G), "G"), "h": (UInt16(kVK_ANSI_H), "H"),
            "i": (UInt16(kVK_ANSI_I), "I"), "j": (UInt16(kVK_ANSI_J), "J"),
            "k": (UInt16(kVK_ANSI_K), "K"), "l": (UInt16(kVK_ANSI_L), "L"),
            "m": (UInt16(kVK_ANSI_M), "M"), "n": (UInt16(kVK_ANSI_N), "N"),
            "o": (UInt16(kVK_ANSI_O), "O"), "p": (UInt16(kVK_ANSI_P), "P"),
            "q": (UInt16(kVK_ANSI_Q), "Q"), "r": (UInt16(kVK_ANSI_R), "R"),
            "s": (UInt16(kVK_ANSI_S), "S"), "t": (UInt16(kVK_ANSI_T), "T"),
            "u": (UInt16(kVK_ANSI_U), "U"), "v": (UInt16(kVK_ANSI_V), "V"),
            "w": (UInt16(kVK_ANSI_W), "W"), "x": (UInt16(kVK_ANSI_X), "X"),
            "y": (UInt16(kVK_ANSI_Y), "Y"), "z": (UInt16(kVK_ANSI_Z), "Z"),
            "0": (UInt16(kVK_ANSI_0), "0"), "1": (UInt16(kVK_ANSI_1), "1"),
            "2": (UInt16(kVK_ANSI_2), "2"), "3": (UInt16(kVK_ANSI_3), "3"),
            "4": (UInt16(kVK_ANSI_4), "4"), "5": (UInt16(kVK_ANSI_5), "5"),
            "6": (UInt16(kVK_ANSI_6), "6"), "7": (UInt16(kVK_ANSI_7), "7"),
            "8": (UInt16(kVK_ANSI_8), "8"), "9": (UInt16(kVK_ANSI_9), "9"),
            "f1": (UInt16(kVK_F1), "F1"), "f2": (UInt16(kVK_F2), "F2"),
            "f3": (UInt16(kVK_F3), "F3"), "f4": (UInt16(kVK_F4), "F4"),
            "f5": (UInt16(kVK_F5), "F5"), "f6": (UInt16(kVK_F6), "F6"),
            "f7": (UInt16(kVK_F7), "F7"), "f8": (UInt16(kVK_F8), "F8"),
            "f9": (UInt16(kVK_F9), "F9"), "f10": (UInt16(kVK_F10), "F10"),
            "f11": (UInt16(kVK_F11), "F11"), "f12": (UInt16(kVK_F12), "F12"),
        ]
        guard let (code, label) = mapping[keyToken] else { return nil }
        return RecorderShortcut(code: code, modifiers: modifiers, key: label)
    }

    static func parseGradient(_ text: String) -> FrequencyGradient? {
        let items = text.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        guard (2...8).contains(items.count) else { return nil }
        var stops: [FrequencyGradient.Stop] = []
        for item in items {
            let parts = item.split(separator: "@", maxSplits: 1)
            guard parts.count == 2 else { return nil }
            let hex = String(parts[0]).trimmingCharacters(in: .whitespaces)
            guard hex.hasPrefix("#"), hex.count == 7,
                  let rgb = UInt32(hex.dropFirst(), radix: 16),
                  let pos = Double(parts[1].trimmingCharacters(in: .whitespaces)),
                  pos.isFinite, (0...1).contains(pos) else { return nil }
            stops.append(FrequencyGradient.Stop(position: pos, rgb: rgb))
        }
        stops.sort { $0.position < $1.position }
        return FrequencyGradient(stops: stops)
    }

    static func parseSteps(_ text: String) -> Result<[TestStep], MessageError> {
        var result: [TestStep] = []
        for raw in text.split(separator: ";") {
            let step = raw.trimmingCharacters(in: .whitespaces)
            if step.isEmpty { continue }
            if step == "install" { result.append(.installConfirm); continue }
            if step == "preview-plan" { result.append(.previewPlan); continue }
            if step == "open-export-mode" { result.append(.openExportMode); continue }
            if step == "meters=open" { result.append(.openMeters); continue }
            guard let eq = step.firstIndex(of: "=") else {
                return .failure("no '=' in \(step)")
            }
            let name = step[..<eq].trimmingCharacters(in: .whitespaces)
            let value = step[step.index(after: eq)...].trimmingCharacters(in: .whitespaces)

            func parseNonNegative(_ s: String) -> Result<Double, MessageError> {
                guard let v = Double(s.trimmingCharacters(in: .whitespaces)), v.isFinite, v >= 0 else {
                    return .failure("\(s) is not a number of seconds")
                }
                return .success(v)
            }
            func splitPair(_ s: String) -> Result<(String, String), MessageError> {
                guard let colon = s.firstIndex(of: ":") else {
                    return .failure("\(step) needs <lane>:<value>")
                }
                return .success((String(s[..<colon]), String(s[s.index(after: colon)...])))
            }

            switch name {
            case "zoom":
                switch parseNonNegative(value) {
                case .success(let v): result.append(.zoom(v))
                case .failure(let e): return .failure(e)
                }
            case "select":
                guard let dash = value.firstIndex(of: "-") else {
                    return .failure("\(step) needs <from>-<to>")
                }
                let a = String(value[..<dash])
                let b = String(value[value.index(after: dash)...])
                switch (parseNonNegative(a), parseNonNegative(b)) {
                case (.success(let from), .success(let to)): result.append(.select(from, to))
                case (.failure(let e), _), (_, .failure(let e)): return .failure(e)
                }
            case "select-lane":
                guard let lane = parseLane(value) else { return .failure("\(value) is not a lane") }
                result.append(.selectLane(lane))
            case "rows":
                let p = splitPair(value)
                guard case .success(let (firstLaneText, rowsText)) = p else {
                    if case .failure(let e) = p { return .failure(e) }
                    return .failure("\(step) needs <lane>:<first>-<last>")
                }
                guard let firstLane = parseLane(firstLaneText) else { return .failure("\(firstLaneText) is not a lane") }
                guard let dash = rowsText.firstIndex(of: "-") else { return .failure("\(step) needs <lane>:<first>-<last>") }
                let firstText = String(rowsText[..<dash]).trimmingCharacters(in: .whitespaces)
                let tail = String(rowsText[rowsText.index(after: dash)...]).trimmingCharacters(in: .whitespaces)
                let lastLane: Int
                let lastText: String
                if let colon = tail.firstIndex(of: ":") {
                    let lt = String(tail[..<colon])
                    guard let parsedLastLane = parseLane(lt) else { return .failure("\(lt) is not a lane") }
                    lastLane = parsedLastLane
                    lastText = String(tail[tail.index(after: colon)...]).trimmingCharacters(in: .whitespaces)
                } else {
                    lastLane = firstLane
                    lastText = tail
                }
                guard let firstRow = Int(firstText), firstRow >= 1 else {
                    return .failure("\(firstText) is not a row counted from 1")
                }
                guard let lastRow = Int(lastText), lastRow >= 1 else {
                    return .failure("\(lastText) is not a row counted from 1")
                }
                result.append(.rows(firstLane, firstRow - 1, lastLane, lastRow - 1))
            case "save":
                result.append(.save(URL(fileURLWithPath: value)))
            case "drag-prepare":
                result.append(.dragPrepare(URL(fileURLWithPath: value)))
            case "export-mode":
                switch value {
                case "binaural-files": result.append(.exportMode(0))
                case "mono-files": result.append(.exportMode(1))
                case "multichannel": result.append(.exportMode(2))
                case "binaural-mix": result.append(.exportMode(3))
                default: return .failure("\(value) is not binaural-files, mono-files, multichannel or binaural-mix")
                }
            case "separate":
                if value == "on" { result.append(.separate(true)) }
                else if value == "off" { result.append(.separate(false)) }
                else { return .failure("\(value) is not on or off") }
            case "hide-silent":
                if value == "on" { result.append(.hideSilent(true)) }
                else if value == "off" { result.append(.hideSilent(false)) }
                else { return .failure("\(value) is not on or off") }
            case "pair":
                if value == "stereo" { result.append(.pair(false)) }
                else if value == "binaural" { result.append(.pair(true)) }
                else { return .failure("\(value) is not stereo or binaural") }
            case "scroll":
                guard case .success(let (laneText, rowText)) = splitPair(value) else {
                    return .failure("\(step) needs <lane>:<value>")
                }
                guard let lane = parseLane(laneText) else { return .failure("\(laneText) is not a lane") }
                guard let row = Int(rowText.trimmingCharacters(in: .whitespaces)), row >= 0 else {
                    return .failure("\(rowText) is not a row")
                }
                result.append(.scroll(lane, row))
            case "group":
                guard case .success(let (laneText, id)) = splitPair(value), let lane = parseLane(laneText) else {
                    return .failure("\(step) needs <lane>:<group>")
                }
                result.append(.group(lane, id.trimmingCharacters(in: .whitespaces)))
            case "options":
                let page: String
                let size: NSSize?
                if let colon = value.firstIndex(of: ":") {
                    page = String(value[..<colon]).trimmingCharacters(in: .whitespaces).lowercased().replacingOccurrences(of: "-", with: "")
                    let sizeText = String(value[value.index(after: colon)...])
                    guard let parsed = TestCopyConfig.parseWindowSize(sizeText),
                          parsed.width >= 200, parsed.height >= 200 else {
                        return .failure("\(sizeText) is not <width>x<height>")
                    }
                    size = parsed
                } else {
                    page = value.lowercased().replacingOccurrences(of: "-", with: "")
                    size = nil
                }
                let validPages = ["actions", "hotkeys", "capture", "sources", "appearance", "export", "addons", "updates"]
                guard validPages.contains(page) else { return .failure("\(value) is not an Options page") }
                result.append(.options(page, size))
            case "options-scroll":
                guard let pts = Double(value), pts.isFinite, pts >= 0 else {
                    return .failure("\(value) is not a number of points")
                }
                result.append(.optionsScroll(CGFloat(pts)))
            case "wait":
                guard let ms = UInt64(value) else { return .failure("\(value) is not a number of milliseconds") }
                result.append(.wait(ms))
            case "bind":
                guard case .success(let (actionText, bindText)) = splitPair(value) else {
                    return .failure("\(step) needs <action id>:<binding>")
                }
                guard let action = parseActionID(actionText) else { return .failure("unknown action \(actionText)") }
                let trimmedBind = bindText.trimmingCharacters(in: .whitespaces)
                if trimmedBind == "none" {
                    result.append(.bind(action, []))
                } else {
                    var list: [RecorderShortcut] = []
                    for part in trimmedBind.split(separator: ",") {
                        guard let binding = parseBinding(String(part)) else {
                            return .failure("unknown binding \(part)")
                        }
                        list.append(binding)
                    }
                    result.append(.bind(action, list))
                }
            case "key":
                guard let binding = parseBinding(value) else { return .failure("unknown binding \(value)") }
                result.append(.key(binding))
            case "look":
                if value.isEmpty { return .failure("\(step) needs an add-on id or none") }
                result.append(.look(value == "none" ? nil : value))
            case "waveform-color":
                if value == "single" { result.append(.waveformColor(false)) }
                else if value == "frequency" { result.append(.waveformColor(true)) }
                else { return .failure("\(value) is not single or frequency") }
            case "gradient":
                if value == "default" {
                    result.append(.gradient(nil))
                } else if let g = parseGradient(value) {
                    result.append(.gradient(g))
                } else {
                    return .failure("\(value) is not 2 to 8 stops like #e8302c@0,#2c68ec@1")
                }
            case "color-wait":
                if value == "on" { result.append(.colorWait(true)) }
                else if value == "off" { result.append(.colorWait(false)) }
                else { return .failure("\(value) is not on or off") }
            case "add-source":
                if value.isEmpty { return .failure("\(step) needs the name of a source") }
                result.append(.addSource(value))
            case "remove-source":
                guard let lane = parseLane(value) else { return .failure("\(value) is not a lane") }
                result.append(.removeSource(lane))
            case "drop", "drop-options":
                let url = URL(fileURLWithPath: value)
                if AddonFormatting.isPackageURL(url) {
                    result.append(.dropPackage(url))
                } else {
                    result.append(.dropFile(url))
                }
            case "allow-control":
                guard case .success(let (id, flag)) = splitPair(value) else {
                    return .failure("\(step) needs <addon-id>:<on|off>")
                }
                result.append(.allowControl(id.trimmingCharacters(in: .whitespaces), flag == "on" || flag == "true"))
            case "set":
                let parts = value.split(separator: ":", maxSplits: 2).map(String.init)
                guard parts.count == 3 else { return .failure("\(step) needs <addon-id>:<key>:<value>") }
                result.append(.setAddonSetting(parts[0], parts[1], parts[2]))
            case "action":
                guard case .success(let (id, actionID)) = splitPair(value) else {
                    return .failure("\(step) needs <addon-id>:<action>")
                }
                result.append(.runAddonAction(id.trimmingCharacters(in: .whitespaces), actionID.trimmingCharacters(in: .whitespaces)))
            case "format":
                guard case .success(let (laneText, fmt)) = splitPair(value), let lane = parseLane(laneText) else {
                    return .failure("\(step) needs <lane>:<format-id>")
                }
                result.append(.selectAddonFormat(lane, fmt.trimmingCharacters(in: .whitespaces)))
            case "transport":
                let parts = value.split(separator: ":", maxSplits: 2).map(String.init)
                guard parts.count >= 2, let lane = parseLane(parts[0]) else {
                    return .failure("\(step) needs <lane>:<command>[:<pos>]")
                }
                let pos = parts.count == 3 ? Double(parts[2]) : nil
                result.append(.addonTransport(lane, parts[1], pos))
            case "remove":
                let parts = value.split(separator: ":", maxSplits: 1).map(String.init)
                let deleteData = parts.count == 2 && (parts[1] == "delete" || parts[1] == "true")
                result.append(.removeAddon(parts[0], deleteData))
            case "open-format", "open":
                result.append(.unsupportedAddon(step))
            default:
                return .failure("unknown step \(name)")
            }
        }
        return .success(result)
    }
}

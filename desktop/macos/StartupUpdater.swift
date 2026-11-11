import AppKit
import CryptoKit
import Darwin
import Foundation

private let releaseEndpoint = URL(string: "https://api.github.com/repos/kekekewww/Backtracking/releases/latest")!
private let releasesPage = URL(string: "https://github.com/kekekewww/Backtracking/releases")!
private let updateBundleID = "com.backtracking.audio.capture"
private let maximumUpdateSize = 128 * 1024 * 1024
private let applyUpdateFlag = "--backtracking-apply-update"
private let updateReceiptFlag = "--backtracking-update-receipt"
private let skipUpdateFlag = "--backtracking-skip-update"

private enum UpdateFailure: LocalizedError {
    case invalidRelease, invalidSignature, invalidPackage, unconfigured, installation

    var errorDescription: String? {
        switch self {
        case .invalidRelease: "GitHub 的更新資料不完整或版本不符。"
        case .invalidSignature: "更新簽章無法驗證。"
        case .invalidPackage: "更新檔案不完整或不適用於這部 Mac。"
        case .unconfigured: "此版本尚未配置更新驗證公鑰。"
        case .installation: "無法安裝新版，請確認 Backtracking 所在資料夾可寫入。"
        }
    }
}

private struct UpdateVersion: Comparable {
    let parts: [UInt64]

    init?(_ text: String) {
        let fields = text.split(separator: ".", omittingEmptySubsequences: false)
        guard fields.count == 3,
              fields.allSatisfy({ !$0.isEmpty && ($0.count == 1 || $0.first != "0") }),
              fields.allSatisfy({ $0.utf8.allSatisfy { 48...57 ~= $0 } }) else { return nil }
        let numbers = fields.compactMap { UInt64($0) }
        guard numbers.count == 3 else { return nil }
        parts = numbers
    }

    static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.parts.lexicographicallyPrecedes(rhs.parts)
    }
}

private struct GitHubUpdate: Decodable {
    struct Asset: Decodable {
        let name: String
        let browser_download_url: URL
        let size: Int
    }

    let tag_name: String
    let body: String?
    let draft: Bool
    let prerelease: Bool
    let assets: [Asset]

    var version: String? {
        guard !draft, !prerelease, tag_name.first == "v" else { return nil }
        let version = String(tag_name.dropFirst())
        return UpdateVersion(version) == nil ? nil : version
    }

    func asset(_ name: String) throws -> Asset {
        let matches = assets.filter { $0.name == name }
        guard let asset = matches.first, matches.count == 1,
              asset.browser_download_url == URL(
                string: "https://github.com/kekekewww/Backtracking/releases/download/\(tag_name)/\(name)"
              ) else { throw UpdateFailure.invalidRelease }
        return asset
    }
}

private struct UpdateManifest: Codable, Sendable {
    let version: String
    let platform: String
    let artifact: String
    let size: Int
    let sha256: String

    static var platform: String {
        #if arch(arm64)
        "macos-arm64"
        #else
        "macos-x64"
        #endif
    }

    static var name: String { "Backtracking-update-\(platform).json" }
    static var artifactName: String { "Backtracking-update-\(platform).zip" }

    static func verify(_ bytes: Data, signature: Data, publicKey: String?) throws -> Self {
        guard let publicKey, let keyBytes = Data(lowercaseHex: publicKey), keyBytes.count == 32 else {
            throw UpdateFailure.unconfigured
        }
        let key = try Curve25519.Signing.PublicKey(rawRepresentation: keyBytes)
        guard signature.count == 64, key.isValidSignature(signature, for: bytes) else {
            throw UpdateFailure.invalidSignature
        }
        let manifest = try JSONDecoder().decode(Self.self, from: bytes)
        guard UpdateVersion(manifest.version) != nil, manifest.platform == platform,
              manifest.artifact == artifactName, (1...maximumUpdateSize).contains(manifest.size),
              Data(lowercaseHex: manifest.sha256)?.count == 32 else {
            throw UpdateFailure.invalidRelease
        }
        return manifest
    }

    func verifyArchive(_ url: URL) throws {
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        guard (attributes[.size] as? NSNumber)?.intValue == size,
              attributes[.type] as? FileAttributeType == .typeRegular else { throw UpdateFailure.invalidPackage }
        let input = try FileHandle(forReadingFrom: url)
        defer { try? input.close() }
        var hash = SHA256()
        while let chunk = try input.read(upToCount: 256 * 1024), !chunk.isEmpty {
            hash.update(data: chunk)
        }
        guard Data(hash.finalize()) == Data(lowercaseHex: sha256) else { throw UpdateFailure.invalidPackage }
    }
}

private extension Data {
    init?(lowercaseHex text: String) {
        let bytes = Array(text.utf8)
        guard bytes.count.isMultiple(of: 2),
              bytes.allSatisfy({ 48...57 ~= $0 || 97...102 ~= $0 }) else { return nil }
        let digits: [UInt8] = bytes.map { $0 <= 57 ? $0 - 48 : $0 - 87 }
        self.init()
        reserveCapacity(digits.count / 2)
        for offset in stride(from: 0, to: digits.count, by: 2) {
            let high: UInt8 = digits[offset] << 4
            append(high | digits[offset + 1])
        }
    }
}

private struct UpdateTransaction: Codable {
    let installedApp: URL
    let previousVersion: String
    let nextVersion: String
    let parentPID: Int32
    let notes: String

    static let name = "transaction.json"
    static let health = "started"

    static func read(_ directory: URL) throws -> Self {
        try JSONDecoder().decode(Self.self, from: Data(contentsOf: directory.appendingPathComponent(name)))
    }
}

@MainActor
final class StartupUpdater: NSObject {
    private enum Preference: String { case ask, automatic, manual }
    private enum Choice { case update, later, never }
    private let defaults = UserDefaults.standard
    private var busy = false
    private weak var automaticButton: NSButton?
    private weak var checkButton: NSButton?
    private weak var status: NSTextField?

    private var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }

    private var publicKey: String? {
        Bundle.main.object(forInfoDictionaryKey: "BacktrackingUpdatePublicKey") as? String
    }

    private var preference: Preference {
        get { Preference(rawValue: defaults.string(forKey: "updatePreference") ?? "") ?? .ask }
        set {
            defaults.set(newValue.rawValue, forKey: "updatePreference")
            automaticButton?.state = newValue == .automatic ? .on : .off
        }
    }

    func makeOptionsTab() -> NSTabViewItem {
        let versionLabel = NSTextField(labelWithString: "目前版本 \(version)")
        let automatic = NSButton(checkboxWithTitle: "啟動時自動安裝更新", target: self, action: #selector(toggleAutomatic(_:)))
        automatic.state = preference == .automatic ? .on : .off
        automaticButton = automatic
        let check = NSButton(title: "檢查更新", target: self, action: #selector(checkManually))
        check.isEnabled = !busy
        checkButton = check
        let open = NSButton(title: "GitHub Releases", target: self, action: #selector(openReleases))
        let notes = NSButton(title: "上次更新內容", target: self, action: #selector(showPreviousNotes))
        let hint = NSTextField(wrappingLabelWithString: "手動啟動和登入啟動都會先檢查，再開始擷取。")
        let status = NSTextField(wrappingLabelWithString: "")
        self.status = status
        let buttons = NSStackView(views: [check, open, notes])
        buttons.spacing = 8
        let stack = NSStackView(views: [versionLabel, automatic, buttons, hint, status])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 16, bottom: 16, right: 16)
        let tab = NSTabViewItem(identifier: "update")
        tab.label = "更新"
        tab.view = stack
        return tab
    }

    func checkAtLaunch(ready: @escaping @MainActor () -> Void) {
        let arguments = CommandLine.arguments
        if arguments.count == 3, arguments[1] == updateReceiptFlag {
            let directory = URL(fileURLWithPath: arguments[2], isDirectory: true)
            let receipt = try? UpdateTransaction.read(directory)
            if let receipt, receipt.nextVersion == version,
               receipt.installedApp.resolvingSymlinksInPath() == Bundle.main.bundleURL.resolvingSymlinksInPath() {
                do {
                    try Data(version.utf8).write(
                        to: directory.appendingPathComponent(UpdateTransaction.health), options: .atomic
                    )
                } catch { ready(); return }
                ready()
                defaults.set(receipt.notes, forKey: "lastUpdateNotes")
                defaults.set(receipt.nextVersion, forKey: "lastUpdateVersion")
                showNotes(title: "已更新至 \(receipt.nextVersion)", notes: receipt.notes, buttons: ["好"])
            } else {
                ready()
            }
            return
        }
        guard !arguments.contains(skipUpdateFlag), preference != .manual, publicKey != nil else {
            ready()
            return
        }
        Task {
            if !(await check(startup: true)) { ready() }
        }
    }

    @objc private func toggleAutomatic(_ button: NSButton) {
        preference = button.state == .on ? .automatic : .manual
        if preference == .automatic { defaults.removeObject(forKey: "skippedUpdateVersion") }
    }

    @objc private func checkManually() {
        Task { _ = await check(startup: false) }
    }

    @objc private func openReleases() {
        NSWorkspace.shared.open(releasesPage)
    }

    @objc private func showPreviousNotes() {
        let version = defaults.string(forKey: "lastUpdateVersion") ?? self.version
        showNotes(title: "更新內容 \(version)", notes: defaults.string(forKey: "lastUpdateNotes") ?? "", buttons: ["好"])
    }

    private func check(startup: Bool) async -> Bool {
        guard !busy else { return false }
        busy = true
        checkButton?.isEnabled = false
        status?.stringValue = "正在檢查更新…"
        defer { busy = false; checkButton?.isEnabled = true }
        do {
            guard let publicKey else { throw UpdateFailure.unconfigured }
            let data = try await updateData(releaseEndpoint, timeout: 4, limit: 1_048_576)
            let release = try JSONDecoder().decode(GitHubUpdate.self, from: data)
            guard let newVersion = release.version, let candidate = UpdateVersion(newVersion),
                  let current = UpdateVersion(version), candidate > current,
                  !(startup && defaults.string(forKey: "skippedUpdateVersion") == newVersion) else {
                status?.stringValue = "目前沒有可安裝的新版。"
                return false
            }
            let manifestAsset = try release.asset(UpdateManifest.name)
            let signatureAsset = try release.asset(UpdateManifest.name + ".sig")
            async let manifestData = updateData(manifestAsset.browser_download_url, timeout: 8, limit: 8_192)
            async let signatureData = updateData(signatureAsset.browser_download_url, timeout: 8, limit: 64)
            let (manifestBytes, signature) = try await (manifestData, signatureData)
            let manifest = try UpdateManifest.verify(manifestBytes, signature: signature, publicKey: publicKey)
            let asset = try release.asset(manifest.artifact)
            guard manifest.version == newVersion, asset.size == manifest.size else { throw UpdateFailure.invalidRelease }
            let notes = String((release.body ?? "").prefix(65_536))
            if !startup || preference == .ask {
                let buttons = startup
                    ? ["更新並啟用自動更新", "下個版本再提示", "不再顯示更新提示"]
                    : ["更新並重新啟動", "稍後"]
                switch showNotes(title: "Backtracking \(newVersion) 已推出", notes: notes, buttons: buttons) {
                case .alertFirstButtonReturn:
                    if startup { preference = .automatic }
                case .alertThirdButtonReturn:
                    preference = .manual
                    status?.stringValue = ""
                    return false
                default:
                    if startup { defaults.set(newVersion, forKey: "skippedUpdateVersion") }
                    status?.stringValue = ""
                    return false
                }
            }
            status?.stringValue = "正在下載及驗證更新…"
            try await prepare(asset: asset, manifest: manifest, manifestBytes: manifestBytes, signature: signature, notes: notes)
            NSApp.terminate(nil)
            return true
        } catch {
            status?.stringValue = error.localizedDescription
            if !startup {
                showNotes(title: "無法更新 Backtracking", notes: error.localizedDescription, buttons: ["好"])
            }
            return false
        }
    }

    private func prepare(
        asset: GitHubUpdate.Asset, manifest: UpdateManifest, manifestBytes: Data, signature: Data, notes: String
    ) async throws {
        let installed = Bundle.main.bundleURL.resolvingSymlinksInPath()
        guard let binary = Bundle.main.executableURL, installed.pathExtension == "app" else {
            throw UpdateFailure.invalidRelease
        }

        let fm = FileManager.default
        let directory = fm.temporaryDirectory.appendingPathComponent("backtracking-update-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        var dispatched = false
        defer { if !dispatched { try? fm.removeItem(at: directory) } }

        let package = directory.appendingPathComponent(manifest.artifact)
        try await downloadUpdate(asset.browser_download_url, to: package, expectedSize: manifest.size)
        try await Task.detached(priority: .utility) { try manifest.verifyArchive(package) }.value
        try manifestBytes.write(to: directory.appendingPathComponent(UpdateManifest.name), options: .atomic)
        try signature.write(to: directory.appendingPathComponent(UpdateManifest.name + ".sig"), options: .atomic)
        let transaction = UpdateTransaction(
            installedApp: installed, previousVersion: version, nextVersion: manifest.version,
            parentPID: getpid(), notes: notes
        )
        try JSONEncoder().encode(transaction).write(to: directory.appendingPathComponent(UpdateTransaction.name), options: .atomic)
        let helper = directory.appendingPathComponent("BacktrackingUpdater")
        try fm.copyItem(at: binary, to: helper)
        _ = try launch(helper, [applyUpdateFlag, directory.path])
        dispatched = true
    }

    @discardableResult
    private func showNotes(title: String, notes: String, buttons: [String]) -> NSApplication.ModalResponse {
        let alert = NSAlert()
        alert.messageText = title
        for title in buttons { alert.addButton(withTitle: title) }
        if !notes.isEmpty {
            let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 480, height: 230))
            scroll.hasVerticalScroller = true
            scroll.borderType = .bezelBorder
            let text = NSTextView(frame: scroll.bounds)
            text.isEditable = false
            text.isRichText = false
            text.isVerticallyResizable = true
            text.autoresizingMask = .width
            text.textContainer?.widthTracksTextView = true
            text.string = notes
            scroll.documentView = text
            alert.accessoryView = scroll
        }
        NSApp.activate(ignoringOtherApps: true)
        return alert.runModal()
    }

    static func runHelperIfRequested() -> Bool {
        let args = CommandLine.arguments
        guard args.count == 3, args[1] == applyUpdateFlag else { return false }
        applyUpdate(at: URL(fileURLWithPath: args[2], isDirectory: true))
        return true
    }
}

private func updateData(_ url: URL, timeout: TimeInterval, limit: Int) async throws -> Data {
    let session = makeUpdateSession(timeout: timeout)
    defer { session.invalidateAndCancel() }
    let (bytes, response) = try await session.bytes(for: updateRequest(url))
    try validate(response)
    var data = Data()
    for try await byte in bytes {
        guard data.count < limit else { throw UpdateFailure.invalidPackage }
        data.append(byte)
    }
    return data
}

private func downloadUpdate(_ url: URL, to output: URL, expectedSize: Int) async throws {
    let session = makeUpdateSession(timeout: 60)
    defer { session.invalidateAndCancel() }
    let (bytes, response) = try await session.bytes(for: updateRequest(url))
    try validate(response)
    guard FileManager.default.createFile(atPath: output.path, contents: nil) else {
        throw UpdateFailure.installation
    }
    let file = try FileHandle(forWritingTo: output)
    defer { try? file.close() }
    var chunk = Data()
    var count = 0
    for try await byte in bytes {
        guard count < expectedSize else { throw UpdateFailure.invalidPackage }
        count += 1
        chunk.append(byte)
        if chunk.count == 64 * 1024 {
            try file.write(contentsOf: chunk)
            chunk.removeAll(keepingCapacity: true)
        }
    }
    guard count == expectedSize else { throw UpdateFailure.invalidPackage }
    try file.write(contentsOf: chunk)
    try file.synchronize()
}

private func makeUpdateSession(timeout: TimeInterval) -> URLSession {
    let config = URLSessionConfiguration.ephemeral
    config.timeoutIntervalForRequest = timeout
    config.timeoutIntervalForResource = timeout
    return URLSession(configuration: config)
}

private func updateRequest(_ url: URL) -> URLRequest {
    var request = URLRequest(url: url)
    request.setValue("Backtracking macOS updater", forHTTPHeaderField: "User-Agent")
    return request
}

private func validate(_ response: URLResponse) throws {
    guard let http = response as? HTTPURLResponse, http.statusCode == 200,
          let url = http.url, url.scheme == "https",
          let host = url.host,
          host == "api.github.com" || host == "github.com" || host == "release-assets.githubusercontent.com" ||
          host == "objects.githubusercontent.com" else { throw UpdateFailure.invalidRelease }
}

private func launch(_ executable: URL, _ arguments: [String]) throws -> Process {
    let process = Process()
    process.executableURL = executable
    process.arguments = arguments
    try process.run()
    return process
}

private func run(_ command: String, _ arguments: [String]) throws {
    let child = try launch(URL(fileURLWithPath: command), arguments)
    child.waitUntilExit()
    guard child.terminationStatus == 0 else { throw UpdateFailure.installation }
}

private func applyUpdate(at directory: URL) {
    let fm = FileManager.default
    guard let transaction = try? UpdateTransaction.read(directory),
          transaction.parentPID > 1,
          transaction.installedApp.pathExtension == "app",
          let original = Bundle(url: transaction.installedApp),
          original.bundleIdentifier == updateBundleID,
          let originalBinary = original.executableURL,
          let previous = UpdateVersion(transaction.previousVersion),
          let candidate = UpdateVersion(transaction.nextVersion), candidate > previous else { return }

    let deadline = Date().addingTimeInterval(20)
    while kill(transaction.parentPID, 0) == 0, Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
    guard kill(transaction.parentPID, 0) != 0, errno == ESRCH else { return }

    let installed = transaction.installedApp
    let lockURL = installed.deletingLastPathComponent().appendingPathComponent(".Backtracking-update.lock")
    let lock = open(lockURL.path, O_CREAT | O_NOFOLLOW | O_RDWR, S_IRUSR | S_IWUSR)
    guard lock >= 0 else { _ = try? launch(originalBinary, [skipUpdateFlag]); return }
    defer { close(lock) }
    guard flock(lock, LOCK_EX | LOCK_NB) == 0 else { return }
    defer { try? fm.removeItem(at: directory) }
    guard original.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String == transaction.previousVersion else { return }

    var replacementDirectory: URL?
    var swapped = false
    var preserveBackup = false
    defer {
        if !preserveBackup, let replacementDirectory { try? fm.removeItem(at: replacementDirectory) }
    }

    do {
        let manifest = try UpdateManifest.verify(
            Data(contentsOf: directory.appendingPathComponent(UpdateManifest.name)),
            signature: Data(contentsOf: directory.appendingPathComponent(UpdateManifest.name + ".sig")),
            publicKey: original.object(forInfoDictionaryKey: "BacktrackingUpdatePublicKey") as? String
        )
        guard manifest.version == transaction.nextVersion else { throw UpdateFailure.invalidRelease }
        let archive = directory.appendingPathComponent(manifest.artifact)
        try manifest.verifyArchive(archive)
        let staging = try fm.url(for: .itemReplacementDirectory, in: .userDomainMask, appropriateFor: installed, create: true)
        replacementDirectory = staging
        try run("/usr/bin/ditto", ["-x", "-k", archive.path, staging.path])
        let app = staging.appendingPathComponent("Backtracking.app")
        guard let bundle = Bundle(url: app), bundle.bundleIdentifier == updateBundleID,
              bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String == manifest.version else {
            throw UpdateFailure.invalidPackage
        }
        try run("/usr/bin/codesign", ["--verify", "--strict", "--deep", app.path])
        try swap(installed, app)
        swapped = true

        let child = try launch(originalBinary, [updateReceiptFlag, directory.path])
        let health = directory.appendingPathComponent(UpdateTransaction.health)
        let readyBy = Date().addingTimeInterval(20)
        while child.isRunning, !fm.fileExists(atPath: health.path), Date() < readyBy {
            Thread.sleep(forTimeInterval: 0.05)
        }
        if child.isRunning, (try? String(contentsOf: health, encoding: .utf8)) == manifest.version { return }
        if child.isRunning {
            child.terminate()
            let stopBy = Date().addingTimeInterval(2)
            while child.isRunning, Date() < stopBy { Thread.sleep(forTimeInterval: 0.05) }
            if child.isRunning { kill(child.processIdentifier, SIGKILL) }
            child.waitUntilExit()
        }
        throw UpdateFailure.installation
    } catch {
        if swapped, let replacementDirectory {
            do { try swap(installed, replacementDirectory.appendingPathComponent("Backtracking.app")) }
            catch { preserveBackup = true; return }
        }
        _ = try? launch(originalBinary, [skipUpdateFlag])
    }
}

private func swap(_ first: URL, _ second: URL) throws {
    let result = first.path.withCString { firstPath in
        second.path.withCString { secondPath in renamex_np(firstPath, secondPath, UInt32(RENAME_SWAP)) }
    }
    guard result == 0 else { throw UpdateFailure.installation }
}

#if BACKTRACKING_UPDATER_TESTS
enum StartupUpdaterVerification {
    static func runTests() throws {
        precondition(UpdateVersion("0.1.5")! < UpdateVersion("0.1.10")!)
        precondition(UpdateVersion("01.1.5") == nil)
        precondition(UpdateVersion("0.1.5-beta") == nil)

        let key = Curve25519.Signing.PrivateKey()
        let publicKey = hex(key.publicKey.rawRepresentation)
        let sample = UpdateManifest(
            version: "0.1.6", platform: UpdateManifest.platform,
            artifact: UpdateManifest.artifactName, size: 3, sha256: hex(Data(SHA256.hash(data: Data("abc".utf8))))
        )
        let manifest = try JSONEncoder().encode(sample)
        let signature = try key.signature(for: manifest)
        let verified = try UpdateManifest.verify(manifest, signature: signature, publicKey: publicKey)
        precondition(verified.sha256 == sample.sha256)
        expectFailure {
            _ = try UpdateManifest.verify(manifest + Data([10]), signature: signature, publicKey: publicKey)
        }
        expectFailure {
            _ = try UpdateManifest.verify(manifest, signature: signature, publicKey: nil)
        }

        try testApplicationReplacement(key: key, succeeds: true)
        try testApplicationReplacement(key: key, succeeds: false)
        print("macOS updater: signature, version, native application replacement and rollback passed")
    }

    static func respondToChildInvocation() throws -> Bool {
        let args = CommandLine.arguments
        if args.contains(skipUpdateFlag) { return true }
        guard args.count == 3, args[1] == updateReceiptFlag else { return false }
        if Bundle.main.object(forInfoDictionaryKey: "FailTestLaunch") as? Bool == true { return true }
        let directory = URL(fileURLWithPath: args[2], isDirectory: true)
        let receipt = try UpdateTransaction.read(directory)
        guard Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String == receipt.nextVersion else {
            throw UpdateFailure.installation
        }
        try Data(receipt.nextVersion.utf8).write(
            to: directory.appendingPathComponent(UpdateTransaction.health), options: .atomic
        )
        Thread.sleep(forTimeInterval: 1)
        return true
    }

    private static func testApplicationReplacement(key: Curve25519.Signing.PrivateKey, succeeds: Bool) throws {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.appendingPathComponent("BacktrackingUpdaterTest-\(UUID().uuidString)")
        try fm.createDirectory(at: root, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        defer { try? fm.removeItem(at: root) }
        let installed = root.appendingPathComponent("Backtracking.app")
        let build = root.appendingPathComponent("build")
        let candidate = build.appendingPathComponent("Backtracking.app")
        let transactionDirectory = root.appendingPathComponent("transaction")
        try fm.createDirectory(at: transactionDirectory, withIntermediateDirectories: true)
        let publicKey = hex(key.publicKey.rawRepresentation)
        try makeTestBundle(installed, version: "0.1.5", publicKey: publicKey, failLaunch: false)
        try makeTestBundle(candidate, version: "0.1.6", publicKey: publicKey, failLaunch: !succeeds)

        let archive = transactionDirectory.appendingPathComponent(UpdateManifest.artifactName)
        try run("/usr/bin/ditto", ["-c", "-k", "--keepParent", candidate.path, archive.path])
        let file = try Data(contentsOf: archive)
        let manifest = UpdateManifest(
            version: "0.1.6", platform: UpdateManifest.platform, artifact: UpdateManifest.artifactName,
            size: file.count, sha256: hex(Data(SHA256.hash(data: file)))
        )
        let bytes = try JSONEncoder().encode(manifest)
        try bytes.write(to: transactionDirectory.appendingPathComponent(UpdateManifest.name))
        try key.signature(for: bytes).write(to: transactionDirectory.appendingPathComponent(UpdateManifest.name + ".sig"))
        let finishedParent = try launch(URL(fileURLWithPath: "/usr/bin/true"), [])
        finishedParent.waitUntilExit()
        let transaction = UpdateTransaction(
            installedApp: installed, previousVersion: "0.1.5", nextVersion: "0.1.6",
            parentPID: finishedParent.processIdentifier, notes: "Test notes"
        )
        try JSONEncoder().encode(transaction).write(to: transactionDirectory.appendingPathComponent(UpdateTransaction.name))
        applyUpdate(at: transactionDirectory)
        let plist = installed.appendingPathComponent("Contents/Info.plist")
        let fields = try PropertyListSerialization.propertyList(from: Data(contentsOf: plist), format: nil) as? [String: Any]
        precondition(fields?["CFBundleShortVersionString"] as? String == (succeeds ? "0.1.6" : "0.1.5"))
    }

    private static func makeTestBundle(_ bundle: URL, version: String, publicKey: String, failLaunch: Bool) throws {
        let fm = FileManager.default
        let contents = bundle.appendingPathComponent("Contents")
        let binary = contents.appendingPathComponent("MacOS")
        try fm.createDirectory(at: binary, withIntermediateDirectories: true)
        try fm.copyItem(at: Bundle.main.executableURL!, to: binary.appendingPathComponent("Backtracking"))
        let fields: [String: Any] = [
            "CFBundleExecutable": "Backtracking", "CFBundleIdentifier": updateBundleID,
            "CFBundlePackageType": "APPL", "CFBundleShortVersionString": version,
            "BacktrackingUpdatePublicKey": publicKey, "FailTestLaunch": failLaunch,
        ]
        try PropertyListSerialization.data(fromPropertyList: fields, format: .xml, options: 0)
            .write(to: contents.appendingPathComponent("Info.plist"))
        try run("/usr/bin/codesign", ["--force", "--sign", "-", bundle.path])
    }

    private static func hex(_ bytes: Data) -> String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }

    private static func expectFailure(_ check: () throws -> Void) {
        do {
            try check()
            preconditionFailure("Expected update verification to fail")
        } catch {}
    }
}
#endif

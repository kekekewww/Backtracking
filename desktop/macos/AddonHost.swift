import AppKit
import Foundation

// MARK: - Dynamic JSON value for add-on settings

enum AddonJSONValue: Equatable, Codable {
    case bool(Bool)
    case int(Int64)
    case double(Double)
    case string(String)
    case null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let b = try? container.decode(Bool.self) {
            self = .bool(b)
        } else if let i = try? container.decode(Int64.self) {
            self = .int(i)
        } else if let d = try? container.decode(Double.self) {
            self = .double(d)
        } else if let s = try? container.decode(String.self) {
            self = .string(s)
        } else {
            self = .null
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .bool(let b): try container.encode(b)
        case .int(let i): try container.encode(i)
        case .double(let d): try container.encode(d)
        case .string(let s): try container.encode(s)
        case .null: try container.encodeNil()
        }
    }

    var boolValue: Bool? {
        if case .bool(let b) = self { return b }
        return nil
    }

    var doubleValue: Double? {
        switch self {
        case .double(let d) where d.isFinite: return d
        case .int(let i): return Double(i)
        default: return nil
        }
    }

    var stringValue: String? {
        if case .string(let s) = self { return s }
        return nil
    }

    var anyObject: Any {
        switch self {
        case .bool(let b): return b
        case .int(let i): return NSNumber(value: i)
        case .double(let d): return d
        case .string(let s): return s
        case .null: return NSNull()
        }
    }

    static func snappedNumber(_ value: Double, min: Double, max: Double, step: Double) -> AddonJSONValue {
        let snapped = snapNumber(value, min: min, max: max, step: step)
        if snapped.truncatingRemainder(dividingBy: 1.0) == 0.0, abs(snapped) < 9.0e15 {
            return .int(Int64(snapped))
        }
        return .double(snapped)
    }

    static func snapNumber(_ value: Double, min: Double, max: Double, step: Double) -> Double {
        guard value.isFinite else { return min }
        var snapped: Double
        if step > 0 {
            snapped = min + ((value - min) / step).rounded() * step
        } else {
            snapped = value
        }
        if snapped > max {
            if step > 0 {
                snapped = min + floor((max - min) / step) * step
            } else {
                snapped = max
            }
        }
        snapped = (snapped * 1e9).rounded() / 1e9
        return Swift.min(Swift.max(snapped, min), max)
    }

    static func stepDecimals(_ step: Double) -> Int {
        for decimals in 0...6 {
            let scaled = step * pow(10.0, Double(decimals))
            if abs(scaled - scaled.rounded()) < 1e-6 {
                return decimals
            }
        }
        return 6
    }
}

// MARK: - Frozen ABI v1 Models

struct AddonOpenFiles: Codable, Equatable {
    let description: String
    let extensions: [String]

    func accepts(url: URL) -> Bool {
        let ext = url.pathExtension.lowercased()
        guard !ext.isEmpty else { return false }
        return extensions.contains { $0.trimmingCharacters(in: CharacterSet(charactersIn: ".")).lowercased() == ext }
    }

    var formattedExtensions: String {
        extensions.map { ".\($0)" }.joined(separator: ", ")
    }
}

struct AddonManifest: Codable, Equatable {
    let contract: UInt32
    let id: String
    let name: String
    let version: String
    let author: String
    let description: String
    let homepage: String?
    let license: String?
    let capabilities: [String]
    let open_files: AddonOpenFiles?

    func hasCapability(_ name: String) -> Bool {
        capabilities.contains(name)
    }
}

struct AddonNotice: Codable, Equatable {
    let level: String // "info" | "warning" | "error"
    let text: String
}

struct AddonFormat: Codable, Equatable {
    let id: String
    let label: String
}

struct AddonChannelGroup: Codable, Equatable {
    let id: String
    let label: String
    let channels: [String]
    let positioned: Bool

    enum CodingKeys: String, CodingKey {
        case id, label, channels, positioned
    }

    init(id: String, label: String, channels: [String], positioned: Bool = false) {
        self.id = id
        self.label = label
        self.channels = channels
        self.positioned = positioned
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        label = try c.decode(String.self, forKey: .label)
        channels = try c.decode([String].self, forKey: .channels)
        positioned = try c.decodeIfPresent(Bool.self, forKey: .positioned) ?? false
    }
}

struct AddonTransport: Codable, Equatable {
    let state: String // "playing" | "paused" | "stopped"
    let position: Double
    let duration: Double?

    var isPlaying: Bool { state == "playing" }
}

struct AddonSource: Codable, Equatable {
    let id: String
    let name: String
    let sender: String
    let formats: [AddonFormat]
    let format: String
    let groups: [AddonChannelGroup]?
    let transport: AddonTransport?

    var allChannelLabels: [String] {
        guard let groups, !groups.isEmpty else { return [] }
        return groups.flatMap(\.channels)
    }

    var hasPositionedGroup: Bool {
        groups?.contains(where: \.positioned) ?? false
    }

    func channelRange(of groupID: String) -> Range<Int>? {
        guard let groups else { return nil }
        var offset = 0
        for group in groups {
            let end = offset + group.channels.count
            if group.id == groupID {
                return offset..<end
            }
            offset = end
        }
        return nil
    }

    var firstNonPositionedGroup: (group: AddonChannelGroup, range: Range<Int>)? {
        guard let groups else { return nil }
        var offset = 0
        for group in groups {
            let end = offset + group.channels.count
            if !group.positioned {
                return (group, offset..<end)
            }
            offset = end
        }
        return nil
    }

    func resolvedShownGroup(preferredID: String?) -> (group: AddonChannelGroup, range: Range<Int>)? {
        guard let groups, !groups.isEmpty else { return nil }
        if let preferredID {
            var offset = 0
            for group in groups {
                let end = offset + group.channels.count
                if group.id == preferredID {
                    return (group, offset..<end)
                }
                offset = end
            }
        }
        return (groups[0], 0..<groups[0].channels.count)
    }
}

struct AddonChoiceOption: Codable, Equatable {
    let value: String
    let label: String
}

struct AddonSettingDeclaration: Codable, Equatable {
    let key: String
    let label: String
    let help: String?
    let kind: String // "toggle" | "choice" | "number" | "text"
    let defaultValue: AddonJSONValue
    let options: [AddonChoiceOption]?
    let min: Double?
    let max: Double?
    let step: Double?

    enum CodingKeys: String, CodingKey {
        case key, label, help, kind
        case defaultValue = "default"
        case options, min, max, step
    }
}

struct AddonSettingEntry: Codable, Equatable {
    let declaration: AddonSettingDeclaration
    let value: AddonJSONValue
}

struct AddonAction: Codable, Equatable {
    let id: String
    let label: String
}

enum AddonStateTone: Equatable {
    case running
    case busy
    case off
    case problem
}

struct AddonRecord: Codable, Equatable {
    let id: String
    let manifest: AddonManifest?
    let enabled: Bool
    let state: String // stopped | starting | running | stopping | not_responding | failed | broken_package
    let reason: String?
    let pid: UInt32?
    let message: AddonNotice?
    let sources: [AddonSource]
    let settings: [AddonSettingEntry]
    let actions: [AddonAction]
    let control_allowed: Bool
    let control_asked: Bool

    var displayName: String {
        manifest?.name ?? id
    }

    var isRunning: Bool { state == "running" }
    var isBroken: Bool { state == "broken_package" }
    var canStartAgain: Bool { state == "failed" || state == "not_responding" }

    var openFilesSpec: AddonOpenFiles? {
        guard let manifest, manifest.hasCapability("open-files") else { return nil }
        return manifest.open_files
    }

    var declaresControl: Bool {
        manifest?.hasCapability("control") ?? false
    }

    var statusPresentation: (tone: AddonStateTone, text: String) {
        switch state {
        case "running":
            return (.running, "Running")
        case "starting":
            return (.busy, "Starting…")
        case "stopping":
            return (.busy, "Stopping…")
        case "stopped":
            return (.off, enabled ? "Stopped" : "Off")
        case "not_responding":
            return (.problem, "Not responding: it did not connect within 10 seconds, so it was stopped.")
        case "failed":
            return (.problem, "Stopped because of a problem: \(reason ?? "unknown error")")
        case "broken_package":
            return (.problem, "Broken package: \(reason ?? "unknown error")")
        default:
            return (.off, state)
        }
    }
}

struct AddonSpectrumStop: Codable, Equatable {
    let color: String
    let position: Double
}

struct AddonAppearance: Codable, Equatable {
    let colors: [String: String]
    let numbers: [String: Double]
    let rows: [String: String]
    let spectrum: [AddonSpectrumStop]

    enum CodingKeys: String, CodingKey {
        case colors, numbers, rows, spectrum
    }

    init(
        colors: [String: String] = [:],
        numbers: [String: Double] = [:],
        rows: [String: String] = [:],
        spectrum: [AddonSpectrumStop] = []
    ) {
        self.colors = colors
        self.numbers = numbers
        self.rows = rows
        self.spectrum = spectrum
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        colors = try c.decodeIfPresent([String: String].self, forKey: .colors) ?? [:]
        numbers = try c.decodeIfPresent([String: Double].self, forKey: .numbers) ?? [:]
        rows = try c.decodeIfPresent([String: String].self, forKey: .rows) ?? [:]
        spectrum = try c.decodeIfPresent([AddonSpectrumStop].self, forKey: .spectrum) ?? []
    }
}

struct AddonSnapshot: Codable, Equatable {
    let schema: Int
    let generation: UInt64
    let ready: Bool
    let error: String?
    let addons: [AddonRecord]
    let appearances: [String: AddonAppearance]
    let completed: [UInt64]

    static let empty = AddonSnapshot(
        schema: 1,
        generation: 0,
        ready: false,
        error: nil,
        addons: [],
        appearances: [:],
        completed: []
    )

    func addon(_ id: String) -> AddonRecord? {
        addons.first { $0.id == id }
    }

    func source(addonID: String, sourceID: String) -> (addon: AddonRecord, source: AddonSource)? {
        guard let addon = addon(addonID), addon.isRunning else { return nil }
        guard let source = addon.sources.first(where: { $0.id == sourceID }) else { return nil }
        return (addon, source)
    }

    /// Sender instance IDs currently claimed by running add-on sources (hidden from raw Sender list).
    var claimedSenderIDs: Set<String> {
        var ids = Set<String>()
        for addon in addons where addon.isRunning {
            for source in addon.sources {
                ids.insert(source.sender)
            }
        }
        return ids
    }

    func addonName(for id: String) -> String {
        addon(id)?.displayName ?? id
    }
}

struct AddonPackageSummary: Codable, Equatable {
    let manifest: AddonManifest
    let program: String
    let file_count: Int
    let entry_count: Int
    let unpacked_size: UInt64
}

struct AddonInspectResult: Codable, Equatable {
    let manifest: AddonManifest
    let program: String
    let file_count: Int
    let entry_count: Int
    let unpacked_size: UInt64
    let review_id: String
}

// MARK: - Control Protocol Models (Pkg 17)

struct AddonRowRef: Codable, Equatable {
    let lane: String
    let row: UInt32
}

struct AddonControlCommand: Codable, Equatable {
    let command: String
    let from_seconds_ago: Double?
    let to_seconds_ago: Double?
    let start_frame: UInt64?
    let end_frame: UInt64?
    let lane: String?
    let first: AddonRowRef?
    let last: AddonRowRef?
    let seconds: Double?
    let folder: String?
    let source: String?

    var isQuiet: Bool {
        command == "get_state" || command == "list_sources" || command == "stop_preview"
    }
}

struct AddonControlCall: Codable, Equatable {
    let addon_id: String
    let addon_name: String
    let run: UInt64
    let request_id: String
    let command: AddonControlCommand
}

struct AddonFrameSpan: Codable, Equatable {
    let start_frame: UInt64
    let end_frame: UInt64
    let seconds: Double
}

struct AddonSelectionState: Codable, Equatable {
    let start_frame: UInt64
    let end_frame: UInt64
    let seconds: Double
    let from_seconds_ago: Double
    let to_seconds_ago: Double
}

struct AddonSourceInfo: Codable, Equatable {
    let id: String
    let kind: String
    let name: String
}

struct AddonLaneState: Codable, Equatable {
    let lane: String
    let source: AddonSourceInfo
    let channels: UInt32?
    let route: String
    let problem: String?
}

struct AddonRowState: Codable, Equatable {
    let lane: String
    let row: UInt32
    let label: String
}

struct AddonSavedFile: Codable, Equatable {
    let path: String
    let kind: String // "wav" | "positions"
    let lane: String?
    let channels: UInt32?
    let sample_rate: UInt32?
    let frames: UInt64?
}

struct AddonControlState: Codable, Equatable {
    let capturing: Bool
    let recording: Bool
    let problem: String?
    let sample_rate: UInt32
    let lanes: [AddonLaneState]
    let history: AddonFrameSpan
    let selection: AddonSelectionState?
    let view: AddonFrameSpan
    let separate_rows: Bool
    let chosen_rows: [AddonRowState]
    let preview_playing: Bool
    let save_folder: String
    let drag_folder: String
    let busy: String?
}

struct AddonControlResult: Codable, Equatable {
    let state: AddonControlState
    let sources: [AddonSourceInfo]?
    let folder: String?
    let files: [AddonSavedFile]?
    let history_restarted: Bool?

    init(
        state: AddonControlState,
        sources: [AddonSourceInfo]? = nil,
        folder: String? = nil,
        files: [AddonSavedFile]? = nil,
        history_restarted: Bool = false
    ) {
        self.state = state
        self.sources = sources
        self.folder = folder
        self.files = files
        self.history_restarted = history_restarted ? true : nil
    }
}

struct AddonControlError: Codable, Equatable {
    let code: String
    let message: String
}

// MARK: - Position Events (Pkg 19)

struct AddonChannelPosition: Codable, Equatable {
    let channel: UInt32
    let x: Double
    let y: Double
    let z: Double
    let size: Double?
    let gain: Double?

    var isFinite: Bool {
        x.isFinite && y.isFinite && z.isFinite
            && (size?.isFinite ?? true)
            && (gain?.isFinite ?? true)
    }
}

struct AddonPositionEvent: Codable, Equatable {
    let sequence: UInt64
    let addon_id: String
    let source_id: String
    let time: UInt64
    let channels: [AddonChannelPosition]
}

struct AddonPositionsBatch: Codable, Equatable {
    let cursor: UInt64
    let positions: [AddonPositionEvent]
}

// MARK: - Envelope Parsing Helpers

private struct AddonTicketEnvelope: Codable {
    let schema: Int
    let ok: Bool
    let ticket: UInt64?
    let code: String?
    let message: String?
}

private struct AddonValueEnvelope<T: Decodable>: Decodable {
    let schema: Int
    let ok: Bool
    let value: T?
    let code: String?
    let message: String?
}

private struct AddonErrorEnvelope: Decodable {
    let schema: Int
    let ok: Bool
    let code: String?
    let message: String?
}

// MARK: - AddonHostClient

final class AddonHostClient {
    private var handle: OpaquePointer?
    private var lastGeneration: UInt64 = UInt64.max
    private(set) var snapshot: AddonSnapshot = .empty
    private var callbacks: [UInt64: (Data) -> Void] = [:]
    private let decoder = JSONDecoder()
    private let encoder = JSONEncoder()
    let dataRoot: String
    let isPrimary: Bool
    #if BACKTRACKING_CAPTURE_VERIFY
    /// Runs once the polled snapshot's completed tickets have been consumed.
    var verifyAfterTakingResults: (() -> Void)?
    #endif

    init?(dataRoot: String, appVersion: String, primary: Bool) {
        self.dataRoot = dataRoot
        self.isPrimary = primary
        let role: UInt32 = primary ? UInt32(BT_ADDON_PRIMARY) : UInt32(BT_ADDON_SECONDARY)
        guard let ptr = dataRoot.withCString({ rootC in
            appVersion.withCString { verC in
                bt_addon_host_new(rootC, verC, role)
            }
        }) else {
            return nil
        }
        self.handle = ptr
        _ = pollSnapshot(force: true)
    }

    deinit {
        if let ptr = handle {
            bt_addon_host_free(ptr)
            handle = nil
        }
    }

    func shutdownAndFree(timeoutMs: UInt32 = 2500) {
        guard let ptr = handle else { return }
        handle = nil
        _ = bt_addon_host_shutdown(ptr, timeoutMs)
        bt_addon_host_free(ptr)
        callbacks.removeAll()
    }

    @discardableResult
    func pollSnapshot(force: Bool = false) -> Bool {
        guard let ptr = handle else { return false }
        let generation = bt_addon_host_generation(ptr)
        if !force && generation == lastGeneration {
            return false
        }
        guard let raw = bt_addon_host_snapshot(ptr) else { return false }
        defer { bt_addon_string_free(raw) }
        let data = Data(String(cString: raw).utf8)
        guard let parsed = try? decoder.decode(AddonSnapshot.self, from: data) else {
            return false
        }
        snapshot = parsed
        lastGeneration = parsed.generation
        if !parsed.completed.isEmpty {
            for ticket in parsed.completed {
                let resultData = rawCommand(["schema": 1, "op": "take_result", "ticket": ticket])
                if let cb = callbacks.removeValue(forKey: ticket), let resultData {
                    cb(resultData)
                }
            }
            #if BACKTRACKING_CAPTURE_VERIFY
            verifyAfterTakingResults?()
            #endif
            // Keep the snapshot's generation: take_result advances it, so the next poll reads the
            // snapshot again and collects any ticket that finished after this one was taken.
        }
        return true
    }

    private func rawCommand(_ payload: [String: Any]) -> Data? {
        guard let ptr = handle,
              let jsonData = try? JSONSerialization.data(withJSONObject: payload),
              let jsonStr = String(data: jsonData, encoding: .utf8) else {
            return nil
        }
        guard let raw = jsonStr.withCString({ bt_addon_host_command(ptr, $0) }) else {
            return nil
        }
        defer { bt_addon_string_free(raw) }
        return Data(String(cString: raw).utf8)
    }

    private func rawCommandData(_ jsonData: Data) -> Data? {
        guard let ptr = handle, let jsonStr = String(data: jsonData, encoding: .utf8) else {
            return nil
        }
        guard let raw = jsonStr.withCString({ bt_addon_host_command(ptr, $0) }) else {
            return nil
        }
        defer { bt_addon_string_free(raw) }
        return Data(String(cString: raw).utf8)
    }

    private func submitTicket(_ payload: [String: Any], onResult: @escaping (Data) -> Void) -> Result<UInt64, MessageError> {
        guard let data = rawCommand(payload),
              let envelope = try? decoder.decode(AddonTicketEnvelope.self, from: data) else {
            return .failure("The add-on manager did not respond.")
        }
        if envelope.ok, let ticket = envelope.ticket {
            callbacks[ticket] = onResult
            return .success(ticket)
        }
        return .failure(MessageError(envelope.message ?? envelope.code ?? "Command failed"))
    }

    private func submitVoid(_ payload: [String: Any], completion: @escaping (Result<Void, MessageError>) -> Void) {
        let admitted = submitTicket(payload) { [weak self] data in
            guard let self else { return }
            if let env = try? self.decoder.decode(AddonErrorEnvelope.self, from: data) {
                if env.ok {
                    completion(.success(()))
                } else {
                    completion(.failure(MessageError(env.message ?? env.code ?? "Command failed")))
                }
            } else {
                completion(.failure("Invalid reply from add-on manager"))
            }
        }
        if case .failure(let message) = admitted {
            completion(.failure(message))
        }
    }

    // MARK: Async Ticket Operations

    func inspect(path: String, completion: @escaping (Result<AddonInspectResult, MessageError>) -> Void) {
        let admitted = submitTicket(["schema": 1, "op": "inspect", "path": path]) { [weak self] data in
            guard let self else { return }
            if let env = try? self.decoder.decode(AddonValueEnvelope<AddonInspectResult>.self, from: data),
               env.ok, let value = env.value {
                completion(.success(value))
            } else if let err = try? self.decoder.decode(AddonErrorEnvelope.self, from: data) {
                completion(.failure(MessageError(err.message ?? err.code ?? "Could not inspect package")))
            } else {
                completion(.failure("Could not read package inspection result"))
            }
        }
        if case .failure(let message) = admitted {
            completion(.failure(message))
        }
    }

    func install(path: String, reviewID: String, completion: @escaping (Result<AddonPackageSummary, MessageError>) -> Void) {
        let admitted = submitTicket([
            "schema": 1,
            "op": "install",
            "path": path,
            "review_id": reviewID,
        ]) { [weak self] data in
            guard let self else { return }
            if let env = try? self.decoder.decode(AddonValueEnvelope<AddonPackageSummary>.self, from: data),
               env.ok, let value = env.value {
                completion(.success(value))
            } else if let err = try? self.decoder.decode(AddonErrorEnvelope.self, from: data) {
                completion(.failure(MessageError(err.message ?? err.code ?? "Could not install add-on")))
            } else {
                completion(.failure("Could not read installation result"))
            }
        }
        if case .failure(let message) = admitted {
            completion(.failure(message))
        }
    }

    func remove(id: String, deleteData: Bool, completion: @escaping (Result<Void, MessageError>) -> Void) {
        submitVoid(["schema": 1, "op": "remove", "id": id, "delete_data": deleteData], completion: completion)
    }

    func setEnabled(id: String, enabled: Bool, completion: @escaping (Result<Void, MessageError>) -> Void) {
        submitVoid(["schema": 1, "op": "set_enabled", "id": id, "enabled": enabled], completion: completion)
    }

    func start(id: String, completion: @escaping (Result<Void, MessageError>) -> Void) {
        submitVoid(["schema": 1, "op": "start", "id": id], completion: completion)
    }

    func stop(id: String, completion: @escaping (Result<Void, MessageError>) -> Void) {
        submitVoid(["schema": 1, "op": "stop", "id": id], completion: completion)
    }

    func setSetting(id: String, key: String, value: AddonJSONValue, completion: @escaping (Result<Void, MessageError>) -> Void) {
        submitVoid([
            "schema": 1,
            "op": "set_setting",
            "id": id,
            "key": key,
            "value": value.anyObject,
        ], completion: completion)
    }

    func runAction(id: String, action: String, completion: @escaping (Result<Void, MessageError>) -> Void) {
        submitVoid(["schema": 1, "op": "run_action", "id": id, "action": action], completion: completion)
    }

    func openFile(id: String, path: String, completion: @escaping (Result<Void, MessageError>) -> Void) {
        submitVoid(["schema": 1, "op": "open_file", "id": id, "path": path], completion: completion)
    }

    func selectFormat(id: String, source: String, format: String, completion: @escaping (Result<Void, MessageError>) -> Void) {
        submitVoid([
            "schema": 1,
            "op": "select_format",
            "id": id,
            "source": source,
            "format": format,
        ], completion: completion)
    }

    func transport(
        id: String,
        source: String,
        command: String,
        position: Double? = nil,
        completion: @escaping (Result<Void, MessageError>) -> Void
    ) {
        var payload: [String: Any] = [
            "schema": 1,
            "op": "transport",
            "id": id,
            "source": source,
            "command": command,
        ]
        if let position {
            payload["position"] = position
        }
        submitVoid(payload, completion: completion)
    }

    func clipExported(files: [String], sampleRate: UInt32, frames: UInt64) {
        guard !files.isEmpty, frames > 0, sampleRate > 0 else { return }
        submitVoid([
            "schema": 1,
            "op": "clip_exported",
            "files": files,
            "sample_rate": sampleRate,
            "frames": frames,
        ]) { _ in }
    }

    func setControlAllowed(id: String, allowed: Bool, completion: @escaping (Result<Void, MessageError>) -> Void) {
        submitVoid([
            "schema": 1,
            "op": "set_control_allowed",
            "id": id,
            "allowed": allowed,
        ], completion: completion)
    }

    // MARK: Synchronous Operations

    func takeControl() -> [AddonControlCall] {
        guard let data = rawCommand(["schema": 1, "op": "take_control"]),
              let env = try? decoder.decode(AddonValueEnvelope<[AddonControlCall]>.self, from: data),
              env.ok, let calls = env.value else {
            return []
        }
        return calls
    }

    private struct ControlReplyPayload: Encodable {
        let schema: Int = 1
        let op: String = "control_reply"
        let addon_id: String
        let run: UInt64
        let request_id: String
        let result: AddonControlResult?
        let error: AddonControlError?
    }

    func controlReply(call: AddonControlCall, result: AddonControlResult) {
        let payload = ControlReplyPayload(
            addon_id: call.addon_id,
            run: call.run,
            request_id: call.request_id,
            result: result,
            error: nil
        )
        if let data = try? encoder.encode(payload) {
            _ = rawCommandData(data)
        }
    }

    func controlReply(call: AddonControlCall, error: AddonControlError) {
        let payload = ControlReplyPayload(
            addon_id: call.addon_id,
            run: call.run,
            request_id: call.request_id,
            result: nil,
            error: error
        )
        if let data = try? encoder.encode(payload) {
            _ = rawCommandData(data)
        }
    }

    func positionsSince(cursor: UInt64) -> AddonPositionsBatch? {
        guard let data = rawCommand(["schema": 1, "op": "positions_since", "cursor": cursor]),
              let env = try? decoder.decode(AddonValueEnvelope<AddonPositionsBatch>.self, from: data),
              env.ok else {
            return nil
        }
        return env.value
    }
}

// MARK: - Package 19: Position Track & Frozen Positions Handles

final class FrozenPositionsHandle: @unchecked Sendable {
    let pointer: OpaquePointer

    init(_ pointer: OpaquePointer) {
        self.pointer = pointer
    }

    deinit {
        bt_frozen_positions_free(pointer)
    }
}

final class LanePositionBuffer {
    private var track: OpaquePointer?
    private var pending: [(timeNs: UInt64, channels: [AddonChannelPosition])] = []
    private static let maxPendingEvents = 4096

    init() {
        track = bt_positions_new()
    }

    deinit {
        if let track {
            bt_positions_free(track)
        }
    }

    func enqueue(timeNs: UInt64, channels: [AddonChannelPosition]) {
        guard !channels.isEmpty else { return }
        if pending.count >= Self.maxPendingEvents {
            pending.removeFirst(pending.count - Self.maxPendingEvents + 1)
        }
        pending.append((timeNs: timeNs, channels: channels))
    }

    /// Resolves pending events against the lane's frame clock.
    /// `lookup(timeNs)` returns: `(1, frame)` Ready(Some), `(0, 0)` Ready(None), `(-1, 0)` Pending.
    func drain(
        evictedBefore: UInt64?,
        retainedRange: Range<UInt64>?,
        lookup: (UInt64) -> (status: Int32, frame: UInt64)
    ) {
        guard let track else { return }
        if let evictedBefore {
            bt_positions_clock_evicted_before(track, evictedBefore)
        }
        if !pending.isEmpty {
            var remaining: [(timeNs: UInt64, channels: [AddonChannelPosition])] = []
            remaining.reserveCapacity(pending.count)
            for event in pending {
                let res = lookup(event.timeNs)
                if res.status == 1 {
                    for ch in event.channels where ch.isFinite && ch.channel < 64 {
                        let key = BtPositionKey(
                            frame: res.frame,
                            x: min(max(ch.x, -1.0), 1.0),
                            y: min(max(ch.y, -1.0), 1.0),
                            z: min(max(ch.z, -1.0), 1.0),
                            size: min(max(ch.size ?? 0.0, 0.0), 1.0),
                            gain: min(max(ch.gain ?? 1.0, 0.0), 4.0)
                        )
                        _ = bt_positions_insert(track, ch.channel, key)
                    }
                } else if res.status < 0 {
                    remaining.append(event)
                }
                // status == 0 means Ready(None) -> before retained audio / dropped
            }
            pending = remaining
        }
        if let retainedRange, retainedRange.lowerBound < retainedRange.upperBound {
            bt_positions_retain(track, retainedRange.lowerBound, retainedRange.upperBound)
        }
    }

    func takeOverflowed() -> Bool {
        guard let track else { return false }
        return bt_positions_take_overflowed(track)
    }

    func clear() {
        pending.removeAll()
        if let track {
            bt_positions_clear(track)
        }
    }

    func freeze() -> FrozenPositionsHandle? {
        guard let track, let frozen = bt_positions_freeze(track) else { return nil }
        return FrozenPositionsHandle(frozen)
    }
}

// MARK: - Package 20: Add-on Look Resolution & Readability (mirrors bui/theme.rs + look.rs)

struct RGBA8: Equatable, Hashable {
    var r: UInt8
    var g: UInt8
    var b: UInt8
    var a: UInt8

    init(_ r: UInt8, _ g: UInt8, _ b: UInt8, _ a: UInt8 = 255) {
        self.r = r
        self.g = g
        self.b = b
        self.a = a
    }

    init(rgb: UInt32, alpha: UInt8 = 255) {
        self.r = UInt8((rgb >> 16) & 0xFF)
        self.g = UInt8((rgb >> 8) & 0xFF)
        self.b = UInt8(rgb & 0xFF)
        self.a = alpha
    }

    static func parseHex(_ hex: String) -> RGBA8? {
        guard hex.hasPrefix("#") else { return nil }
        let digits = String(hex.dropFirst())
        guard digits.count == 6 || digits.count == 8,
              let value = UInt32(digits, radix: 16) else {
            return nil
        }
        if digits.count == 6 {
            return RGBA8(rgb: value, alpha: 255)
        } else {
            return RGBA8(
                UInt8((value >> 24) & 0xFF),
                UInt8((value >> 16) & 0xFF),
                UInt8((value >> 8) & 0xFF),
                UInt8(value & 0xFF)
            )
        }
    }

    var rgbUInt32: UInt32 {
        (UInt32(r) << 16) | (UInt32(g) << 8) | UInt32(b)
    }

    var opaque: RGBA8 { RGBA8(r, g, b, 255) }

    var nsColor: NSColor {
        NSColor(
            srgbRed: CGFloat(r) / 255.0,
            green: CGFloat(g) / 255.0,
            blue: CGFloat(b) / 255.0,
            alpha: CGFloat(a) / 255.0
        )
    }

    func nsColor(alphaMultiplier: CGFloat) -> NSColor {
        NSColor(
            srgbRed: CGFloat(r) / 255.0,
            green: CGFloat(g) / 255.0,
            blue: CGFloat(b) / 255.0,
            alpha: (CGFloat(a) / 255.0) * alphaMultiplier
        )
    }

    func over(_ behind: RGBA8) -> RGBA8 {
        let keep = UInt16(255 - a)
        let alpha = UInt16(a)
        func mix(_ top: UInt8, _ bottom: UInt8) -> UInt8 {
            let v = (UInt16(top) * alpha + UInt16(bottom) * keep + 127) / 255
            return UInt8(min(v, 255))
        }
        return RGBA8(mix(r, behind.r), mix(g, behind.g), mix(b, behind.b), 255)
    }

    func lerpWhite(_ t: Double) -> RGBA8 {
        func mix(_ c: UInt8) -> UInt8 {
            UInt8(min(max((Double(c) + (255.0 - Double(c)) * t).rounded(), 0), 255))
        }
        return RGBA8(mix(r), mix(g), mix(b), a)
    }

    var relativeLuminance: Double {
        func lin(_ c: UInt8) -> Double {
            let v = Double(c) / 255.0
            return v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    static func contrastRatio(_ a: RGBA8, _ b: RGBA8) -> Double {
        let la = a.relativeLuminance
        let lb = b.relativeLuminance
        return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
    }

    static func apart(_ a: RGBA8, _ b: RGBA8, contrast: UInt8) -> Bool {
        abs(Int(a.r) - Int(b.r)) >= Int(contrast)
            || abs(Int(a.g) - Int(b.g)) >= Int(contrast)
            || abs(Int(a.b) - Int(b.b)) >= Int(contrast)
    }

    static func distinguishable(_ mark: RGBA8, _ behind: RGBA8) -> Bool {
        let dist = abs(Int(mark.r) - Int(behind.r))
            + abs(Int(mark.g) - Int(behind.g))
            + abs(Int(mark.b) - Int(behind.b))
        return dist >= 90
    }

    var muted: RGBA8 {
        RGBA8(r / 3, g / 3, b / 3, 255)
    }
}

struct ResolvedLook: Equatable {
    static let defaultPlotBackground = RGBA8(22, 26, 29, 255)
    static let defaultPanelFill = RGBA8(27, 27, 27, 255)
    static let defaultPanelText = RGBA8(210, 210, 210, 255)
    static let defaultCenterLine = RGBA8(47, 60, 64, 255)
    static let defaultBarLine = RGBA8(255, 255, 255, 104)
    static let defaultSecondLine = RGBA8(255, 255, 255, 52)
    static let defaultRowLabel = RGBA8(180, 180, 180, 255)
    static let defaultFlag = RGBA8(255, 201, 109, 255)
    static let defaultPlayhead = RGBA8(245, 245, 245, 255)
    static let defaultChipBackground = RGBA8(0, 0, 0, 168)
    static let defaultChipText = RGBA8(180, 180, 180, 255)

    static let graphicContrast: Double = 2.0
    static let textContrast: Double = 3.0
    static let minAlpha: UInt8 = 102
    static let selectionContrast: UInt8 = 80

    static let statusMarks: [RGBA8] = [
        RGBA8(249, 78, 88),   // Recording lit
        RGBA8(103, 44, 48),   // Recording dim
        RGBA8(244, 198, 92),  // Waiting
        RGBA8(118, 128, 134), // Paused
        RGBA8(217, 102, 102), // Unavailable
    ]

    var waveform: RGBA8? = nil
    var waveformSelected: RGBA8? = nil
    var plotBackground: RGBA8 = ResolvedLook.defaultPlotBackground
    var centerLine: RGBA8 = ResolvedLook.defaultCenterLine
    var border: RGBA8? = nil
    var laneDivider: RGBA8? = nil
    var barLine: RGBA8 = ResolvedLook.defaultBarLine
    var secondLine: RGBA8 = ResolvedLook.defaultSecondLine
    var rowLabel: RGBA8 = ResolvedLook.defaultRowLabel
    var flag: RGBA8 = ResolvedLook.defaultFlag
    var playhead: RGBA8 = ResolvedLook.defaultPlayhead
    var panelBackground: RGBA8? = nil
    var chipBackground: RGBA8 = ResolvedLook.defaultChipBackground
    var chipText: RGBA8 = ResolvedLook.defaultChipText
    var chipCurrent: RGBA8? = nil
    var scrollThumb: RGBA8? = nil
    var previewHighlight: RGBA8? = nil
    var borderWidth: CGFloat? = nil
    var cornerRadius: CGFloat = 6.0
    var waveformStrokeScale: CGFloat = 1.0
    var waveformFillOpacity: CGFloat = 0.4
    var rowColors: [String: RGBA8] = [:]

    static let `default` = ResolvedLook()

    func effectiveWaveform(accent: RGBA8) -> RGBA8 {
        waveform ?? accent
    }

    func effectiveWaveformSelected(accent: RGBA8) -> RGBA8 {
        if let waveformSelected { return waveformSelected }
        let base = effectiveWaveform(accent: accent)
        return RGBA8(rgb: WaveformTheme.selectionRGB(for: base.rgbUInt32))
    }

    func effectiveRowWaveform(label: String?, accent: RGBA8) -> RGBA8 {
        if let label, let custom = rowColors[label] {
            return custom
        }
        return effectiveWaveform(accent: accent)
    }

    func effectiveChipCurrent(accent: RGBA8) -> RGBA8 {
        chipCurrent ?? accent.muted
    }

    func effectivePreviewHighlight(accent: RGBA8) -> RGBA8 {
        previewHighlight ?? accent.muted
    }

    func effectiveScrollThumb(active: Bool) -> RGBA8 {
        guard let scrollThumb else {
            return active ? RGBA8(200, 200, 200) : RGBA8(128, 128, 128)
        }
        return active ? scrollThumb.lerpWhite(0.5) : scrollThumb
    }

    func effectiveLaneDivider(accent: RGBA8) -> RGBA8 {
        laneDivider ?? RGBA8(accent.r, accent.g, accent.b, 178)
    }

    func effectiveBorderColor(accent: RGBA8, userBorderRGB: UInt32?) -> RGBA8 {
        if let userBorderRGB { return RGBA8(rgb: userBorderRGB) }
        return border ?? accent
    }

    func effectiveBorderWidth(userWidth: Int) -> CGFloat {
        borderWidth ?? CGFloat(userWidth)
    }

    static func from(appearance: AddonAppearance, accentRGB: UInt32) -> ResolvedLook {
        var look = ResolvedLook()
        let c = appearance.colors
        if let v = c["waveform"].flatMap(RGBA8.parseHex) { look.waveform = v }
        if let v = c["waveform_selected"].flatMap(RGBA8.parseHex) { look.waveformSelected = v }
        if let v = c["plot_background"].flatMap(RGBA8.parseHex) { look.plotBackground = v.opaque }
        if let v = c["center_line"].flatMap(RGBA8.parseHex) { look.centerLine = v }
        if let v = c["border"].flatMap(RGBA8.parseHex) { look.border = v }
        if let v = c["lane_divider"].flatMap(RGBA8.parseHex) { look.laneDivider = v }
        if let v = c["bar_line"].flatMap(RGBA8.parseHex) { look.barLine = v }
        if let v = c["second_line"].flatMap(RGBA8.parseHex) { look.secondLine = v }
        if let v = c["row_label"].flatMap(RGBA8.parseHex) { look.rowLabel = v }
        if let v = c["flag"].flatMap(RGBA8.parseHex) { look.flag = v }
        if let v = c["playhead"].flatMap(RGBA8.parseHex) { look.playhead = v }
        if let v = c["panel_background"].flatMap(RGBA8.parseHex) { look.panelBackground = v.opaque }
        if let v = c["chip_background"].flatMap(RGBA8.parseHex) { look.chipBackground = v }
        if let v = c["chip_text"].flatMap(RGBA8.parseHex) { look.chipText = v }
        if let v = c["chip_current"].flatMap(RGBA8.parseHex) { look.chipCurrent = v }
        if let v = c["scroll_thumb"].flatMap(RGBA8.parseHex) { look.scrollThumb = v }
        if let v = c["preview_highlight"].flatMap(RGBA8.parseHex) { look.previewHighlight = v }

        let n = appearance.numbers
        if let v = n["border_width"], v.isFinite {
            look.borderWidth = CGFloat(min(max(v, 0.0), 8.0))
        }
        if let v = n["corner_radius"], v.isFinite {
            look.cornerRadius = CGFloat(min(max(v, 0.0), 16.0))
        }
        if let v = n["waveform_stroke_scale"], v.isFinite {
            look.waveformStrokeScale = CGFloat(min(max(v, 0.5), 3.0))
        }
        if let v = n["waveform_fill_opacity"], v.isFinite {
            look.waveformFillOpacity = CGFloat(min(max(v, 0.0), 1.0))
        }

        for (label, hex) in appearance.rows.prefix(64) {
            if let rgba = RGBA8.parseHex(hex) {
                look.rowColors[label] = rgba
            }
        }
        return look.legible(accent: RGBA8(rgb: accentRGB))
    }

    static func spectrum(from appearance: AddonAppearance) -> FrequencyGradient? {
        guard (2...8).contains(appearance.spectrum.count) else { return nil }
        var stops: [FrequencyGradient.Stop] = []
        var prev = -1.0
        for s in appearance.spectrum {
            guard s.position.isFinite, s.position >= 0, s.position <= 1, s.position >= prev,
                  let rgba = RGBA8.parseHex(s.color) else {
                return nil
            }
            prev = s.position
            stops.append(FrequencyGradient.Stop(position: s.position, rgb: rgba.opaque.rgbUInt32))
        }
        return FrequencyGradient(stops: stops)
    }

    func legibleSpectrum(for gradient: FrequencyGradient) -> (gradient: FrequencyGradient, replacedAny: Bool) {
        let plot = plotBackground.opaque
        let panel = (panelBackground ?? ResolvedLook.defaultPanelFill).opaque
        var replacedAny = false
        let stops = gradient.stops.map { stop -> FrequencyGradient.Stop in
            let color = RGBA8(rgb: stop.rgb)
            let ok = RGBA8.contrastRatio(color, plot) >= ResolvedLook.graphicContrast
                && RGBA8.contrastRatio(color, panel) >= ResolvedLook.graphicContrast
            if ok { return stop }
            replacedAny = true
            return FrequencyGradient.Stop(
                position: stop.position,
                rgb: FrequencyGradient.standard.rgbAt(position: stop.position)
            )
        }
        return (FrequencyGradient(stops: stops), replacedAny)
    }

    private enum Token {
        case plotBackground, panelBackground, waveform, waveformSelected
        case row(String)
        case flag, playhead, rowLabel
        case chipText, chipBackground, chipCurrent
        case previewHighlight, laneDivider, scrollThumb
    }

    func legible(accent: RGBA8) -> ResolvedLook {
        var copy = self
        copy.plotBackground = copy.plotBackground.opaque
        copy.panelBackground = copy.panelBackground?.opaque
        var guardIterations = 0
        while let token = copy.firstIllegibleToken(accent: accent), guardIterations < 128 {
            copy.reset(token)
            guardIterations += 1
        }
        return copy
    }

    private mutating func reset(_ token: Token) {
        let own = ResolvedLook.default
        switch token {
        case .plotBackground: plotBackground = own.plotBackground
        case .panelBackground: panelBackground = own.panelBackground
        case .waveform: waveform = own.waveform
        case .waveformSelected: waveformSelected = own.waveformSelected
        case .row(let label): rowColors.removeValue(forKey: label)
        case .flag: flag = own.flag
        case .playhead: playhead = own.playhead
        case .rowLabel: rowLabel = own.rowLabel
        case .chipText: chipText = own.chipText
        case .chipBackground: chipBackground = own.chipBackground
        case .chipCurrent: chipCurrent = own.chipCurrent
        case .previewHighlight: previewHighlight = own.previewHighlight
        case .laneDivider: laneDivider = own.laneDivider
        case .scrollThumb: scrollThumb = own.scrollThumb
        }
    }

    private func firstIllegibleToken(accent: RGBA8) -> Token? {
        let own = ResolvedLook.default
        let plot = plotBackground
        let plotSet = plot != own.plotBackground
        let panel = panelBackground ?? ResolvedLook.defaultPanelFill
        let panelSet = panelBackground != nil
        let backings: [(RGBA8, Bool, Token)] = [
            (plot, plotSet, .plotBackground),
            (panel, panelSet, .panelBackground),
        ]

        func blame(_ candidates: [(Bool, Token)]) -> Token? {
            candidates.first(where: { $0.0 })?.1
        }
        func onBackings(_ reads: (RGBA8) -> Bool, drawn: [(Bool, Token)]) -> Token? {
            for (backing, set, token) in backings {
                if !reads(backing) {
                    if let t = blame(drawn + [(set, token)]) {
                        return t
                    }
                }
            }
            return nil
        }
        func graphic(_ color: RGBA8, _ backing: RGBA8) -> Bool {
            color.a >= ResolvedLook.minAlpha
                && RGBA8.contrastRatio(color.over(backing), backing) >= ResolvedLook.graphicContrast
        }
        func text(_ color: RGBA8, _ behind: RGBA8) -> Bool {
            RGBA8.contrastRatio(color.over(behind), behind) >= ResolvedLook.textContrast
        }
        func distinct(_ a: RGBA8, _ b: RGBA8, _ backing: RGBA8) -> Bool {
            RGBA8.apart(a.over(backing), b.over(backing), contrast: ResolvedLook.selectionContrast)
        }
        func marksLost(_ behind: RGBA8) -> Bool {
            ResolvedLook.statusMarks.contains { !RGBA8.distinguishable($0, behind) }
        }

        if plotSet && marksLost(plot) { return .plotBackground }
        if panelSet && (marksLost(panel) || !text(ResolvedLook.defaultPanelText, panel)) {
            return .panelBackground
        }

        let wf = effectiveWaveform(accent: accent)
        let wfSet = waveform != nil
        if let t = onBackings({ graphic(wf, $0) }, drawn: [(wfSet, .waveform)]) { return t }

        let sel = effectiveWaveformSelected(accent: accent)
        if let t = onBackings(
            { graphic(sel, $0) && distinct(sel, wf, $0) },
            drawn: [(waveformSelected != nil, .waveformSelected), (wfSet, .waveform)]
        ) {
            return t
        }

        for (label, color) in rowColors {
            if backings.contains(where: { !(graphic(color, $0.0) && distinct(color, sel, $0.0)) }) {
                return .row(label)
            }
        }

        if let t = onBackings({ graphic(flag, $0) }, drawn: [(flag != own.flag, .flag)]) { return t }
        if let t = onBackings({ graphic(playhead, $0) }, drawn: [(playhead != own.playhead, .playhead)]) { return t }
        if let t = onBackings({ text(rowLabel, $0) }, drawn: [(rowLabel != own.rowLabel, .rowLabel)]) { return t }

        let chipBgSet = chipBackground != own.chipBackground
        let chipCurSet = chipCurrent != nil
        let chipCur = effectiveChipCurrent(accent: accent)
        if let t = onBackings(
            { text(chipText, chipBackground.over($0)) },
            drawn: [(chipText != own.chipText, .chipText), (chipBgSet, .chipBackground)]
        ) {
            return t
        }
        let white = RGBA8(255, 255, 255)
        for (backing, _, _) in backings {
            let sw = chipBackground.over(backing)
            let cur = chipCur.over(sw)
            if !text(white, sw), let t = blame([(chipBgSet, .chipBackground)]) { return t }
            if !(text(white, cur) && RGBA8.apart(cur, sw, contrast: ResolvedLook.selectionContrast)),
               let t = blame([(chipCurSet, .chipCurrent), (chipBgSet, .chipBackground)]) {
                return t
            }
        }

        if let hl = previewHighlight {
            if hl.a < ResolvedLook.minAlpha || backings.contains(where: { !distinct(hl, wf, $0.0) }) {
                return .previewHighlight
            }
        }
        if let div = laneDivider {
            if backings.contains(where: { !graphic(div, $0.0) }) {
                return .laneDivider
            }
        }
        if scrollThumb != nil {
            if backings.contains(where: {
                !(graphic(effectiveScrollThumb(active: false), $0.0)
                    && graphic(effectiveScrollThumb(active: true), $0.0))
            }) {
                return .scrollThumb
            }
        }
        return nil
    }
}

// MARK: - Formatting & Helpers

enum AddonFormatting {
    static func isPackageURL(_ url: URL) -> Bool {
        url.pathExtension.lowercased() == "btaddon"
    }

    static func linkIsOpenable(_ link: String) -> Bool {
        link.hasPrefix("https://")
            && link.count > "https://".count
            && link.count <= 2048
            && !link.contains(where: { $0.isWhitespace || $0.isNewline || $0 == "\"" })
    }

    static func clock(_ seconds: Double) -> String {
        let total = seconds.isFinite ? UInt64(max(0.0, floor(seconds))) : 0
        let hours = total / 3600
        let minutes = (total / 60) % 60
        let secs = total % 60
        if hours > 0 {
            return String(format: "%llu:%02llu:%02llu", hours, minutes, secs)
        }
        return String(format: "%llu:%02llu", minutes, secs)
    }

    static func byteSize(_ bytes: UInt64) -> String {
        if bytes < 1024 { return "\(bytes) B" }
        let units = ["KB", "MB", "GB", "TB"]
        var value = Double(bytes) / 1024.0
        var unit = 0
        while value >= 1024.0 && unit + 1 < units.count {
            value /= 1024.0
            unit += 1
        }
        if value < 10.0 {
            return String(format: "%.1f %@", value, units[unit])
        }
        return String(format: "%.0f %@", value, units[unit])
    }

    static func sizeInFiles(bytes: UInt64, files: Int) -> String {
        let size = byteSize(bytes)
        return files == 1 ? "\(size) in 1 file" : "\(size) in \(files) files"
    }

    static func capabilityLines(for manifest: AddonManifest) -> [String] {
        if manifest.capabilities.isEmpty {
            return ["Nothing beyond running its program."]
        }
        return manifest.capabilities.map { cap in
            switch cap {
            case "source":
                return "Provide audio sources in the source list"
            case "open-files":
                if let of = manifest.open_files {
                    return "Open files of these types: \(of.description) (\(of.formattedExtensions))"
                }
                return "Open files"
            case "settings":
                return "Have settings, which Backtracking shows and keeps"
            case "actions":
                return "Add buttons to its entry on the Add-ons page"
            case "clips":
                return "Be told about every clip you export, with its file path"
            case "appearance":
                return "Offer colors and line styles for the window, which you can turn on in Options"
            case "control":
                return "Control Backtracking: start and pause recording, select, save files and change sources, once you allow it on the Add-ons page"
            default:
                return cap
            }
        }
    }

    static func addonMessageText(_ notice: AddonNotice) -> String {
        let prefix: String
        switch notice.level {
        case "warning": prefix = "Warning"
        case "error": prefix = "Error"
        default: prefix = "Message"
        }
        return "\(prefix): \(notice.text)"
    }
}

#if BACKTRACKING_CAPTURE_VERIFY
extension AddonHostClient {
    /// Ticket B completes after the poll read its snapshot but before the poll finished.
    /// The host then stays quiet; B's completion must still run, exactly once, like A's.
    static func verifyTicketCompletions() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-tickets-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        guard let client = AddonHostClient(dataRoot: root.path, appVersion: "0.2.1", primary: true) else {
            throw MessageError("AddonHostClient failed to start")
        }
        defer { client.shutdownAndFree() }
        func pump(for seconds: Double, until done: () -> Bool = { false }) {
            let deadline = ProcessInfo.processInfo.systemUptime + seconds
            while !done() && ProcessInfo.processInfo.systemUptime < deadline {
                client.pollSnapshot()
                Thread.sleep(forTimeInterval: 0.01)
            }
        }
        pump(for: 3) { client.snapshot.ready }
        guard client.snapshot.ready, let handle = client.handle else { throw MessageError("Add-on host did not become ready") }
        pump(for: 0.3)
        let quiet = bt_addon_host_generation(handle)
        Thread.sleep(forTimeInterval: 0.2)
        guard bt_addon_host_generation(handle) == quiet else { throw MessageError("Add-on host never went quiet") }

        var calls: [String: Int] = [:]
        var lateCompleted = false
        client.verifyAfterTakingResults = { [unowned client] in
            client.verifyAfterTakingResults = nil
            let before = bt_addon_host_generation(handle)
            client.inspect(path: root.appendingPathComponent("missing-b.btaddon").path) { _ in calls["B", default: 0] += 1 }
            let deadline = ProcessInfo.processInfo.systemUptime + 2
            while bt_addon_host_generation(handle) == before && ProcessInfo.processInfo.systemUptime < deadline {
                Thread.sleep(forTimeInterval: 0.002)
            }
            lateCompleted = bt_addon_host_generation(handle) != before
        }
        client.inspect(path: root.appendingPathComponent("missing-a.btaddon").path) { _ in calls["A", default: 0] += 1 }
        pump(for: 2) { calls["A"] != nil }
        guard calls["A"] == 1, lateCompleted, client.verifyAfterTakingResults == nil else {
            throw MessageError("Ticket A did not complete or ticket B did not finish inside the poll (A \(calls["A"] ?? 0), B finished \(lateCompleted))")
        }
        pump(for: 1)
        guard calls["B"] == 1, calls["A"] == 1 else {
            throw MessageError("Completion callbacks ran A \(calls["A"] ?? 0) and B \(calls["B"] ?? 0) times; each must run exactly once")
        }
        guard client.callbacks.isEmpty else { throw MessageError("A completed ticket kept its callback") }
    }
}
#endif

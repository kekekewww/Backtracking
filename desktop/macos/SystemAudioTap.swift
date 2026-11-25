import AudioToolbox
import AVFoundation
import CoreAudio
import Darwin
import Foundation
import AppKit

struct CaptureError: LocalizedError {
    let operation: String
    let status: OSStatus?

    init(_ operation: String, status: OSStatus? = nil) {
        self.operation = operation
        self.status = status
    }

    var errorDescription: String? {
        if let status {
            return "\(operation) (Core Audio status \(status))."
        }
        return operation
    }
}

private func checkAudio(_ status: OSStatus, _ operation: String) throws {
    if status != noErr {
        throw CaptureError(operation, status: status)
    }
}

/// Comes before a plug-in Sender's name in the source list.
let senderMark = "【Sender】"

/// Latest snapshot published by AddonHostClient, read on the main actor by capture source helpers.
@MainActor var currentAddonSnapshot: AddonSnapshot = .empty

enum CaptureSource: Hashable {
    case system
    case microphone(String)
    case application(String)
    case sender(String)
    case addon(addonID: String, sourceID: String)

    init?(key: String) {
        if key == "system" { self = .system }
        else if key.hasPrefix("mic:") { self = .microphone(String(key.dropFirst(4))) }
        else if key.hasPrefix("app:"), key.count > 4 { self = .application(String(key.dropFirst(4))) }
        else if key.hasPrefix("sender:"), key.count > 7 { self = .sender(String(key.dropFirst(7))) }
        else if key.hasPrefix("addon:") {
            let rest = key.dropFirst(6)
            guard let colon = rest.firstIndex(of: ":") else { return nil }
            let addonID = String(rest[..<colon])
            let sourceID = String(rest[rest.index(after: colon)...])
            guard !addonID.isEmpty, !sourceID.isEmpty else { return nil }
            self = .addon(addonID: addonID, sourceID: sourceID)
        } else { return nil }
    }

    var key: String {
        switch self {
        case .system: "system"
        case .microphone(let uid): "mic:\(uid)"
        case .application(let identifier): "app:\(identifier)"
        case .sender(let id): "sender:\(id)"
        case .addon(let addonID, let sourceID): "addon:\(addonID):\(sourceID)"
        }
    }

    var controlKind: String {
        switch self {
        case .system: "system"
        case .microphone(let uid): uid.isEmpty ? "default-microphone" : "microphone"
        case .application: "application"
        case .sender: "sender"
        case .addon: "addon"
        }
    }

    var label: String {
        switch self {
        case .system: "System audio"
        case .microphone(let uid):
            if uid.isEmpty { "Default microphone" } else { "Microphone · \(uid)" }
        case .application(let identifier): "Application · \(identifier)"
        case .sender: senderMark
        case .addon(_, let sourceID): sourceID
        }
    }

    var isSender: Bool {
        if case .sender = self { return true }
        return false
    }

    var isAddon: Bool {
        if case .addon = self { return true }
        return false
    }

    var isSenderBacked: Bool {
        isSender || isAddon
    }

    var isMicrophone: Bool {
        if case .microphone = self { return true }
        return false
    }
}

enum CaptureRoute: UInt32, CaseIterable, Sendable {
    case merged, left, right, center, mid, side, allChannels

    // UserDefaults stores the Swift raw values. WAV export uses the core route codes.
    var wavRouteCode: UInt32 {
        switch self {
        case .left: 0
        case .right: 1
        case .center: 2
        case .mid: 3
        case .side: 4
        case .merged: 5
        case .allChannels: 7
        }
    }

    var label: String {
        switch self {
        case .merged: "Merged"
        case .allChannels: "All channels"
        case .left: "Left"
        case .right: "Right"
        case .center: "Center"
        case .mid: "Mid"
        case .side: "Side"
        }
    }

    var controlName: String {
        switch self {
        case .merged: "merged"
        case .allChannels: "all"
        case .left: "left"
        case .right: "right"
        case .center: "center"
        case .mid: "mid"
        case .side: "side"
        }
    }
}

let maximumCaptureLanes = 64

func captureLaneLetter(_ index: Int) -> String {
    guard let index = UInt32(exactly: index), let label = bt_lane_letter(index) else { return "?" }
    defer { bt_string_free(label) }
    return String(cString: label)
}

struct CaptureLane: Equatable {
    var source: CaptureSource
    var route: CaptureRoute = .merged
}

struct WaveformEnvelopeColumn: Equatable, Sendable {
    let x: Double
    let minimum: Float
    let maximum: Float
}

struct WaveformSamplePoint: Equatable, Sendable {
    let x: Double
    let value: Float
}

enum WaveformDisplayGeometry: Equatable, Sendable {
    case empty
    case envelope([[WaveformEnvelopeColumn]])
    case samples([[WaveformSamplePoint]])
}

struct WaveformRenderGate {
    private(set) var inFlight = false
    private var refreshPending = false

    mutating func request() -> Bool {
        if inFlight {
            refreshPending = true
            return false
        }
        inFlight = true
        refreshPending = false
        return true
    }

    mutating func complete(stale: Bool) -> Bool {
        guard inFlight else { return false }
        inFlight = false
        let retry = stale || refreshPending
        refreshPending = false
        return retry
    }
}

func waveformDisplayGeometry(
    minimum: [Float], maximum: [Float], visibleFrames: Range<UInt64>
) -> WaveformDisplayGeometry {
    let count = min(minimum.count, maximum.count)
    guard count > 0, count <= 4_096, !visibleFrames.isEmpty else { return .empty }
    var sampleMode = UInt64(count) == visibleFrames.upperBound - visibleFrames.lowerBound
    var spans: [Range<Int>] = []
    var first: Int?
    for index in 0...count {
        let valid = index < count && minimum[index].isFinite && maximum[index].isFinite
        if valid {
            if first == nil { first = index }
            if minimum[index] != maximum[index] { sampleMode = false }
        } else if let start = first {
            spans.append(start..<index)
            first = nil
        }
    }
    guard !spans.isEmpty else { return .empty }
    let scale = Double(count)
    func bounded(_ value: Float) -> Float { max(-1, min(1, value)) }
    if sampleMode {
        return .samples(spans.map { span in
            span.map { WaveformSamplePoint(x: Double($0) / scale, value: bounded(minimum[$0])) }
        })
    }
    func column(_ index: Int, at x: Double) -> WaveformEnvelopeColumn {
        WaveformEnvelopeColumn(x: x, minimum: bounded(min(minimum[index], maximum[index])),
                               maximum: bounded(max(minimum[index], maximum[index])))
    }
    return .envelope(spans.map { span in
        var columns: [WaveformEnvelopeColumn] = []
        columns.reserveCapacity(span.count + 2)
        columns.append(column(span.lowerBound, at: Double(span.lowerBound) / scale))
        for index in span { columns.append(column(index, at: (Double(index) + 0.5) / scale)) }
        columns.append(column(span.upperBound - 1, at: Double(span.upperBound) / scale))
        return columns
    })
}

func minimumWaveformZoomFrames(width: Double, retainedFrames: UInt64) -> UInt64 {
    guard retainedFrames > 0 else { return 0 }
    guard retainedFrames > 1 else { return 1 }
    let desired = width.isFinite && width > 0 ? UInt64((min(width, 16_384) / 4).rounded(.down)) : 2
    return min(retainedFrames, max(2, min(4_096, desired)))
}

func zoomedWaveformRange(
    _ view: Range<UInt64>, in available: Range<UInt64>, width: Double,
    cursorFraction: Double, delta: Double
) -> Range<UInt64> {
    guard !available.isEmpty, !view.isEmpty else { return available }
    let maximum = available.upperBound - available.lowerBound
    let current = min(view.upperBound - view.lowerBound, maximum)
    let first = min(max(view.lowerBound, available.lowerBound), available.upperBound - current)
    guard delta.isFinite else { return first..<(first + current) }
    let fraction = cursorFraction.isFinite ? min(1, max(0, cursorFraction)) : 0.5
    let minimum = min(current, minimumWaveformZoomFrames(width: width, retainedFrames: maximum))
    let proposed = (Double(current) * exp(-delta)).rounded()
    let next: UInt64
    if proposed <= Double(minimum) { next = minimum }
    else if proposed >= Double(maximum) { next = maximum }
    else { next = UInt64(proposed) }
    let pivot = Double(first - available.lowerBound) + Double(current) * fraction
    let proposedOffset = (pivot - Double(next) * fraction).rounded()
    let maximumOffset = maximum - next
    let offset: UInt64
    if proposedOffset <= 0 { offset = 0 }
    else if proposedOffset >= Double(maximumOffset) { offset = maximumOffset }
    else { offset = UInt64(proposedOffset) }
    let start = available.lowerBound + offset
    return start..<(start + next)
}

private struct SenderDescriptor {
    let id: String
    let name: String
    let sampleRate: UInt32
    let channels: UInt32
}

private func senderDescriptors(audioOnly: Bool) -> [SenderDescriptor] {
    guard let list = bt_senders_new(audioOnly ? 1 : 0) else { return [] }
    defer { bt_senders_free(list) }
    return (0..<bt_senders_count(list)).compactMap { index in
        guard let id = bt_senders_string(list, index, 0) else { return nil }
        defer { bt_string_free(id) }
        guard let name = bt_senders_string(list, index, 1) else { return nil }
        defer { bt_string_free(name) }
        var channels: UInt32 = 0
        let sampleRate = bt_senders_format(list, index, &channels)
        return SenderDescriptor(id: String(cString: id), name: String(cString: name),
                                sampleRate: sampleRate, channels: channels)
    }
}

func availableSenders() -> [(id: String, name: String)] {
    senderDescriptors(audioOnly: true).map { ($0.id, $0.name) }
}

func availableSnapshotTargets() -> [(id: String, name: String)] {
    senderDescriptors(audioOnly: false).map { ($0.id, $0.name) }
}

func requestSenderSnapshot(_ id: String) -> Bool {
    id.withCString { bt_sender_request_snapshot($0) != 0 }
}

struct RunningAppSnapshot: Sendable {
    let pid: pid_t
    let bundleIdentifier: String
    let localizedName: String?
    let isRegular: Bool
}

@MainActor
func snapshotRunningApps() -> [RunningAppSnapshot] {
    NSWorkspace.shared.runningApplications.compactMap { app in
        guard app.activationPolicy != .prohibited, let bundle = app.bundleIdentifier else { return nil }
        return RunningAppSnapshot(
            pid: app.processIdentifier, bundleIdentifier: bundle,
            localizedName: app.localizedName, isRegular: app.activationPolicy == .regular
        )
    }
}

@MainActor
func availableCaptureSources(excluding privateDevices: Set<AudioObjectID>) -> [(source: CaptureSource, title: String)] {
    scanCaptureSources(
        excluding: privateDevices,
        runningApps: snapshotRunningApps(),
        addonSnapshot: currentAddonSnapshot
    )
}

func scanCaptureSources(
    excluding privateDevices: Set<AudioObjectID>,
    runningApps apps: [RunningAppSnapshot],
    addonSnapshot: AddonSnapshot = .empty
) -> [(source: CaptureSource, title: String)] {
    var choices: [(CaptureSource, String)] = []
    if TestCopyConfig.current.syntheticSystemFormat != nil
        || defaultDevice(kAudioHardwarePropertyDefaultOutputDevice) != nil {
        choices.append((.system, CaptureSource.system.label))
    }
    let inputDevices = audioObjects(kAudioHardwarePropertyDevices).filter { device in
        !privateDevices.contains(device)
            && (audioUInt32(device, kAudioDevicePropertyDeviceIsAlive) ?? 0) != 0
            && inputChannels(device) > 0
    }
    if let device = defaultDevice(kAudioHardwarePropertyDefaultInputDevice), inputDevices.contains(device) {
        choices.append((.microphone(""), CaptureSource.microphone("").label))
    }
    // Add-on sources and plug-in senders come right after the two fixed entries, as on Windows,
    // so they are not lost below a long list of devices and apps.
    let fixedChoices = choices.count
    for device in inputDevices {
        guard let uid = audioString(device, kAudioDevicePropertyDeviceUID), !uid.isEmpty,
              let name = audioString(device, kAudioObjectPropertyName) else { continue }
        choices.append((.microphone(uid), name))
    }

    let appsByPID = Dictionary(apps.map { ($0.pid, $0) }, uniquingKeysWith: { first, _ in first })
    let ownPID = ProcessInfo.processInfo.processIdentifier
    let ownBundle = Bundle.main.bundleIdentifier
    var namesByBundle: [String: String] = [:]
    for object in audioObjects(kAudioHardwarePropertyProcessObjectList) {
        guard hasPlaybackStream(object), let pid = processID(object), pid != ownPID else { continue }
        let bundle = audioString(object, kAudioProcessPropertyBundleID)
        var owner = appsByPID[pid]
        if owner?.isRegular != true {
            let helperOwner = apps.first { app in
                bundle?.hasPrefix(app.bundleIdentifier + ".helper") == true
            }
            let ancestor = helperOwner ?? processLineage(pid).dropFirst()
                .first(where: { appsByPID[$0]?.isRegular == true }).flatMap { appsByPID[$0] }
            owner = ancestor ?? owner ?? apps.first { $0.bundleIdentifier == bundle }
        }
        guard let identifier = owner?.bundleIdentifier ?? bundle, !identifier.isEmpty,
              identifier != ownBundle else { continue }
        namesByBundle[identifier] = owner?.localizedName ?? namesByBundle[identifier] ?? identifier
    }
    let sortedApps = namesByBundle.sorted {
        let order = $0.value.localizedStandardCompare($1.value)
        return order == .orderedSame ? $0.key < $1.key : order == .orderedAscending
    }
    choices += sortedApps.map { (.application($0.key), $0.value) }

    // ADD-W30: Hide raw Sender entries claimed by running add-on sources, and list add-on sources first.
    let claimedSenders = addonSnapshot.claimedSenderIDs
    var addonEntries: [(CaptureSource, String)] = []
    var seenNames: [String: Int] = [:]
    for addon in addonSnapshot.addons where addon.isRunning {
        for source in addon.sources {
            seenNames[source.name, default: 0] += 1
        }
    }
    for addon in addonSnapshot.addons where addon.isRunning {
        for source in addon.sources {
            let title = (seenNames[source.name] ?? 0) > 1
                ? "\(source.name) (\(addon.displayName))"
                : source.name
            addonEntries.append((.addon(addonID: addon.id, sourceID: source.id), title))
        }
    }
    let rawSenderEntries: [(CaptureSource, String)] = availableSenders()
        .filter { !claimedSenders.contains($0.id) }
        .map { (.sender($0.id), "\(senderMark)\($0.name)") }
    choices.insert(contentsOf: addonEntries + rawSenderEntries, at: fixedChoices)
    return choices
}

/// The name each source was last found under, by key, so its files keep that name after the
/// device, app or Sender goes away.
@MainActor private var knownFileNames: [String: String] = [:]

/// What a source's files are called when the user has not named it: its name in the source
/// list, without the mark before a Sender. A source that is gone keeps the name it was last
/// found under, or else its identifier.
@MainActor
func defaultFileName(for source: CaptureSource) -> String {
    if let name = presentFileName(for: source), !name.isEmpty {
        knownFileNames[source.key] = name
        return name
    }
    if let known = knownFileNames[source.key] { return known }
    switch source {
    case .system:
        return source.label
    case .microphone(let uid):
        return uid
    case .application(let identifier):
        // An app that is not running is named the way the Finder shows it, without ".app".
        return NSWorkspace.shared.urlForApplication(withBundleIdentifier: identifier)?
            .deletingPathExtension().lastPathComponent ?? identifier
    case .sender:
        return "Sender"
    case .addon(_, let sourceID):
        return sourceID
    }
}

/// The name `source` is listed under now, or nil while it is gone.
@MainActor
private func presentFileName(for source: CaptureSource) -> String? {
    switch source {
    case .system:
        return source.label
    case .microphone(let uid):
        guard !uid.isEmpty else { return source.label }
        let device = audioObjects(kAudioHardwarePropertyDevices).first {
            audioString($0, kAudioDevicePropertyDeviceUID) == uid
        }
        return device.flatMap { audioString($0, kAudioObjectPropertyName) }
    case .application(let identifier):
        return NSRunningApplication.runningApplications(withBundleIdentifier: identifier).first?.localizedName
    case .sender(let id):
        return availableSenders().first { $0.id == id }?.name
    case .addon(let addonID, let sourceID):
        return currentAddonSnapshot.source(addonID: addonID, sourceID: sourceID)?.source.name
    }
}

private let systemObject = AudioObjectID(kAudioObjectSystemObject)
private let unknownObject = AudioObjectID(kAudioObjectUnknown)

private func property(
    _ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
}

private func audioObjects(_ selector: AudioObjectPropertySelector) -> [AudioObjectID] {
    var address = property(selector)
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(systemObject, &address, 0, nil, &size) == noErr else { return [] }
    var values = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.stride)
    guard !values.isEmpty else { return [] }
    let status = values.withUnsafeMutableBytes {
        AudioObjectGetPropertyData(systemObject, &address, 0, nil, &size, $0.baseAddress!)
    }
    return status == noErr ? Array(values.prefix(Int(size) / MemoryLayout<AudioObjectID>.stride)) : []
}

private func audioUInt32(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> UInt32? {
    var address = property(selector)
    var value: UInt32 = 0
    var size = UInt32(MemoryLayout.size(ofValue: value))
    guard AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr else { return nil }
    return value
}

private func hasPlaybackStream(_ object: AudioObjectID) -> Bool {
    if (audioUInt32(object, kAudioProcessPropertyIsRunningOutput) ?? 0) != 0 { return true }
    // Include clients that still have an output device attached while their stream is idle.
    var address = property(kAudioProcessPropertyDevices, kAudioObjectPropertyScopeOutput)
    var size: UInt32 = 0
    return AudioObjectGetPropertyDataSize(object, &address, 0, nil, &size) == noErr
        && size >= UInt32(MemoryLayout<AudioObjectID>.stride)
}

private func audioString(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> String? {
    var address = property(selector)
    var value: Unmanaged<CFString>?
    var size = UInt32(MemoryLayout.size(ofValue: value))
    guard AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr else { return nil }
    guard let value else { return nil }
    return value.takeRetainedValue() as String
}

private func defaultDevice(_ selector: AudioObjectPropertySelector) -> AudioObjectID? {
    var address = property(selector)
    var device = unknownObject
    var size = UInt32(MemoryLayout.size(ofValue: device))
    guard AudioObjectGetPropertyData(systemObject, &address, 0, nil, &size, &device) == noErr,
          device != unknownObject else { return nil }
    return device
}

private func inputChannels(_ device: AudioObjectID) -> Int {
    deviceChannelCount(device, scope: kAudioDevicePropertyScopeInput)
}

private func deviceChannelCount(_ device: AudioObjectID, scope: AudioObjectPropertyScope) -> Int {
    var address = property(kAudioDevicePropertyStreamConfiguration, scope)
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(device, &address, 0, nil, &size) == noErr, size > 0 else { return 0 }
    let data = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment)
    defer { data.deallocate() }
    guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, data) == noErr else { return 0 }
    return UnsafeMutableAudioBufferListPointer(data.assumingMemoryBound(to: AudioBufferList.self))
        .reduce(0) { $0 + Int($1.mNumberChannels) }
}

private func layoutLabels(_ layout: UnsafePointer<AudioChannelLayout>) -> [AudioChannelLabel] {
    let value = layout.pointee
    switch value.mChannelLayoutTag {
    case kAudioChannelLayoutTag_UseChannelDescriptions:
        let count = Int(value.mNumberChannelDescriptions)
        guard count > 0, count <= 64 else { return [] }
        // The C ABI places its variable array after three UInt32 header fields.
        let offset = 3 * MemoryLayout<UInt32>.stride
        let descriptions = UnsafeRawPointer(layout).advanced(by: offset).assumingMemoryBound(to: AudioChannelDescription.self)
        return (0..<count).map { descriptions[$0].mChannelLabel }
    default:
        var specifier: UInt32 = value.mChannelLayoutTag == kAudioChannelLayoutTag_UseChannelBitmap
            ? value.mChannelBitmap.rawValue : value.mChannelLayoutTag
        let selector = value.mChannelLayoutTag == kAudioChannelLayoutTag_UseChannelBitmap
            ? kAudioFormatProperty_ChannelLayoutForBitmap : kAudioFormatProperty_ChannelLayoutForTag
        let specifierSize = UInt32(MemoryLayout.size(ofValue: specifier))
        var size: UInt32 = 0
        guard AudioFormatGetPropertyInfo(selector, specifierSize, &specifier, &size) == noErr,
              size >= MemoryLayout<AudioChannelLayout>.size else { return [] }
        let data = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioChannelLayout>.alignment)
        defer { data.deallocate() }
        guard AudioFormatGetProperty(selector, specifierSize, &specifier, &size, data) == noErr else { return [] }
        let expanded = data.assumingMemoryBound(to: AudioChannelLayout.self)
        guard expanded.pointee.mChannelLayoutTag == kAudioChannelLayoutTag_UseChannelDescriptions else { return [] }
        return layoutLabels(expanded)
    }
}

private func deviceLabels(_ device: AudioObjectID, scope: AudioObjectPropertyScope) -> [AudioChannelLabel] {
    var address = property(kAudioDevicePropertyPreferredChannelLayout, scope)
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(device, &address, 0, nil, &size) == noErr,
          size >= MemoryLayout<AudioChannelLayout>.size else { return [] }
    let data = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioChannelLayout>.alignment)
    defer { data.deallocate() }
    guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, data) == noErr else { return [] }
    return layoutLabels(data.assumingMemoryBound(to: AudioChannelLayout.self))
}

private func processID(_ object: AudioObjectID) -> pid_t? {
    var address = property(kAudioProcessPropertyPID)
    var pid: pid_t = 0
    var size = UInt32(MemoryLayout.size(ofValue: pid))
    guard AudioObjectGetPropertyData(object, &address, 0, nil, &size, &pid) == noErr,
          pid > 1 else { return nil }
    return pid
}

private func parentProcess(_ pid: pid_t) -> pid_t? {
    var request: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
    var result = kinfo_proc()
    var size = MemoryLayout.size(ofValue: result)
    let status = request.withUnsafeMutableBufferPointer {
        sysctl($0.baseAddress, UInt32($0.count), &result, &size, nil, 0)
    }
    guard status == 0, size == MemoryLayout.size(ofValue: result) else { return nil }
    let parent = result.kp_eproc.e_ppid
    return parent > 1 && parent != pid ? parent : nil
}

private func processLineage(_ pid: pid_t) -> some Sequence<pid_t> {
    sequence(first: pid, next: parentProcess).prefix(32)
}

@MainActor
private func processObjects(for identifier: String) -> [AudioObjectID] {
    let roots = Set<pid_t>(NSRunningApplication.runningApplications(withBundleIdentifier: identifier).map {
        $0.processIdentifier
    })
    return audioObjects(kAudioHardwarePropertyProcessObjectList).filter { object in
        let bundle = audioString(object, kAudioProcessPropertyBundleID)
        if bundle == identifier { return true }
        guard !roots.isEmpty, let pid = processID(object) else { return false }
        // Include audio-producing children of the selected app, including browser renderers.
        return bundle?.hasPrefix(identifier + ".helper") == true || processLineage(pid).contains(where: roots.contains)
    }.sorted()
}

private func nominalRate(_ device: AudioObjectID) -> Double {
    var address = property(kAudioDevicePropertyNominalSampleRate)
    var rate = 0.0
    var size = UInt32(MemoryLayout.size(ofValue: rate))
    return AudioObjectGetPropertyData(device, &address, 0, nil, &size, &rate) == noErr && rate.isFinite ? rate : 0
}

private enum InputIdentity: Equatable {
    case system(AudioObjectID, Double)
    case syntheticSystem(String)
    case microphone(AudioObjectID, Double)
    case application([AudioObjectID], AudioObjectID, Double)
    case sender(String, UInt32, UInt32, [String]?)
}

@MainActor
private func identity(for source: CaptureSource) -> InputIdentity? {
    switch source {
    case .sender(let id):
        return senderDescriptors(audioOnly: true).first { $0.id == id }
            .map { .sender(id, $0.sampleRate, $0.channels, nil) }
    case .addon(let addonID, let sourceID):
        guard let (_, addonSource) = currentAddonSnapshot.source(addonID: addonID, sourceID: sourceID) else {
            return nil
        }
        let groupLabels = addonSource.allChannelLabels
        return senderDescriptors(audioOnly: true).first { $0.id == addonSource.sender }
            .map {
                let labels = groupLabels.count == Int($0.channels) ? groupLabels : nil
                return .sender(addonSource.sender, $0.sampleRate, $0.channels, labels)
            }
    case .system:
        if let spec = TestCopyConfig.current.syntheticSystemFormat {
            return .syntheticSystem(spec)
        }
        return defaultDevice(kAudioHardwarePropertyDefaultOutputDevice).map { .system($0, nominalRate($0)) }
    case .application(let identifier):
        let processes = processObjects(for: identifier)
        guard !processes.isEmpty else { return nil }
        let output = defaultDevice(kAudioHardwarePropertyDefaultOutputDevice) ?? unknownObject
        return .application(processes, output, nominalRate(output))
    case .microphone(let uid):
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else { return nil }
        let device = uid.isEmpty ? defaultDevice(kAudioHardwarePropertyDefaultInputDevice)
            : audioObjects(kAudioHardwarePropertyDevices).first {
                audioString($0, kAudioDevicePropertyDeviceUID) == uid && inputChannels($0) > 0
            }
        return device.map { .microphone($0, nominalRate($0)) }
    }
}

private func speakerName(_ label: AudioChannelLabel) -> String? {
    switch label {
    case kAudioChannelLabel_Left: "FL"
    case kAudioChannelLabel_Right: "FR"
    case kAudioChannelLabel_Center: "FC"
    case kAudioChannelLabel_LFEScreen: "LFE"
    case kAudioChannelLabel_LeftCenter: "FLc"
    case kAudioChannelLabel_RightCenter: "FRc"
    case kAudioChannelLabel_LeftSurround, kAudioChannelLabel_LeftSurroundDirect: "Ls"
    case kAudioChannelLabel_RightSurround, kAudioChannelLabel_RightSurroundDirect: "Rs"
    case kAudioChannelLabel_RearSurroundLeft: "Lb"
    case kAudioChannelLabel_RearSurroundRight: "Rb"
    case kAudioChannelLabel_CenterSurround: "BC"
    case kAudioChannelLabel_TopCenterSurround: "TpC"
    case kAudioChannelLabel_VerticalHeightLeft: "TFL"
    case kAudioChannelLabel_VerticalHeightCenter: "TpFC"
    case kAudioChannelLabel_VerticalHeightRight: "TFR"
    case kAudioChannelLabel_TopBackLeft: "TBL"
    case kAudioChannelLabel_TopBackCenter: "TpBC"
    case kAudioChannelLabel_TopBackRight: "TBR"
    default: nil
    }
}

func localizedBilingual(_ text: String) -> String {
    guard let range = text.range(of: " / ") else { return text }
    switch AppLanguage.current {
    case .english: return String(text[..<range.lowerBound])
    case .zhHant: return String(text[range.upperBound...])
    }
}

private struct InputFormat {
    let channels: Int
    let sampleRate: Double
    let planar: Bool
    let left: Int?
    let right: Int?
    let center: Int?
    let mix: [(channel: Int, left: Float, right: Float)]
    let mixScale: Float
    let channelNames: [String]

    init(syntheticChannelNames: [String], sampleRate: Double = 48_000) {
        channels = syntheticChannelNames.count
        self.sampleRate = sampleRate
        planar = false
        channelNames = syntheticChannelNames
        left = syntheticChannelNames.firstIndex(of: "FL") ?? 0
        right = syntheticChannelNames.firstIndex(of: "FR") ?? (channels >= 2 ? 1 : nil)
        center = syntheticChannelNames.firstIndex(of: "FC")
        var weights: [(Int, Float, Float)] = []
        if channels > 2 {
            var leftSum: Float = 1.0
            var rightSum: Float = 1.0
            for (index, name) in syntheticChannelNames.enumerated() where index != left && index != right {
                let (wl, wr): (Float, Float) = switch name {
                case "FC": (0.70710677, 0.70710677)
                case "LFE": (0.5, 0.5)
                case "Ls", "Lb", "TFL", "TBL": (0.70710677, 0)
                case "Rs", "Rb", "TFR", "TBR": (0, 0.70710677)
                default: (0.70710677, 0.70710677)
                }
                leftSum += wl
                rightSum += wr
                weights.append((index, wl, wr))
            }
            let norm = max(1.0, max(leftSum, rightSum))
            mixScale = norm
            mix = weights.map { ($0.0, $0.1 / norm, $0.2 / norm) }
        } else {
            mixScale = 1.0
            mix = []
        }
    }

    init(senderID: String, senderChannels: UInt32, sampleRate: UInt32, overrideLabels: [String]? = nil) {
        channels = Int(senderChannels)
        self.sampleRate = Double(sampleRate)
        planar = false
        left = 0
        right = channels >= 2 ? 1 : nil
        center = nil
        mix = []
        mixScale = 1.0
        if let overrideLabels, overrideLabels.count == channels {
            channelNames = overrideLabels
            return
        }
        let names: [String]
        if let labels = senderID.withCString({ bt_sender_channel_labels($0) }) {
            names = String(cString: labels).components(separatedBy: "\t")
            bt_string_free(labels)
        } else {
            names = []
        }
        channelNames = names.count == channels ? names : (1...channels).map { "Channel \($0)" }
    }

    init(_ stream: AudioStreamBasicDescription, labels: [AudioChannelLabel]) throws {
        let count = Int(stream.mChannelsPerFrame)
        let separate = (stream.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0
        let sampleBytes = UInt32(MemoryLayout<Float>.stride)
        guard stream.mFormatID == kAudioFormatLinearPCM,
              (stream.mFormatFlags & kAudioFormatFlagIsFloat) != 0,
              (stream.mFormatFlags & kAudioFormatFlagIsBigEndian) == 0,
              stream.mBitsPerChannel == 32, (1...64).contains(count),
              (1...384_000).contains(stream.mSampleRate),
              stream.mBytesPerFrame == sampleBytes * (separate ? 1 : stream.mChannelsPerFrame) else {
            throw CaptureError("This source's audio format is not supported")
        }
        channels = count
        sampleRate = stream.mSampleRate
        planar = separate
        let exact = labels.count == count ? labels : []
        channelNames = (0..<count).map { index in
            guard exact.indices.contains(index), let label = speakerName(exact[index]) else { return "Channel \(index + 1)" }
            return label
        }
        left = exact.firstIndex(of: kAudioChannelLabel_Left) ?? 0
        right = exact.firstIndex(of: kAudioChannelLabel_Right) ?? (count >= 2 ? 1 : nil)
        center = exact.firstIndex(of: kAudioChannelLabel_Center)
        if count <= 2 {
            mixScale = 1.0
            mix = []
        } else {
            var leftSum: Float = 1.0
            var rightSum: Float = 1.0
            var rawWeights: [(Int, Float, Float)] = []
            for index in 0..<count where index != left && index != right {
                let label = exact.indices.contains(index) ? exact[index] : kAudioChannelLabel_Unknown
                let (wl, wr): (Float, Float) = switch label {
                case kAudioChannelLabel_LeftSurround, kAudioChannelLabel_LeftSurroundDirect,
                     kAudioChannelLabel_RearSurroundLeft, kAudioChannelLabel_LeftCenter,
                     kAudioChannelLabel_VerticalHeightLeft, kAudioChannelLabel_TopBackLeft:
                    (0.70710677, 0)
                case kAudioChannelLabel_RightSurround, kAudioChannelLabel_RightSurroundDirect,
                     kAudioChannelLabel_RearSurroundRight, kAudioChannelLabel_RightCenter,
                     kAudioChannelLabel_VerticalHeightRight, kAudioChannelLabel_TopBackRight:
                    (0, 0.70710677)
                case kAudioChannelLabel_LFEScreen:
                    (0.5, 0.5)
                default:
                    (0.70710677, 0.70710677)
                }
                leftSum += wl
                rightSum += wr
                rawWeights.append((index, wl, wr))
            }
            let scale = max(1.0, max(leftSum, rightSum))
            mixScale = scale
            mix = rawWeights.map { ($0.0, $0.1 / scale, $0.2 / scale) }
        }
    }

    func width(for route: CaptureRoute) -> UInt32 {
        switch route {
        case .merged: UInt32(min(2, channels))
        case .allChannels: UInt32(channels)
        default: 1
        }
    }

    func supports(_ route: CaptureRoute) -> Bool {
        switch route {
        case .merged: true
        case .allChannels: channels > 2
        case .left: left != nil
        case .right: right != nil
        case .center: center != nil
        case .mid, .side: left != nil && right != nil
        }
    }

    func availableFrames(_ buffers: UnsafeMutableAudioBufferListPointer, limit: Int) -> Int {
        guard !buffers.isEmpty else { return 0 }
        let size = MemoryLayout<Float>.stride
        if planar {
            guard buffers.count >= channels else { return 0 }
            var frames = limit
            for index in 0..<channels {
                let buffer = buffers[index]
                guard buffer.mNumberChannels == 1, buffer.mData != nil else { return 0 }
                frames = min(frames, Int(buffer.mDataByteSize) / size)
            }
            return frames
        }
        let buffer = buffers[0]
        guard buffer.mNumberChannels == channels, buffer.mData != nil else { return 0 }
        return min(limit, Int(buffer.mDataByteSize) / (channels * size))
    }

    func sample(_ buffers: UnsafeMutableAudioBufferListPointer, frame: Int, channel: Int) -> Float {
        if planar { return buffers[channel].mData!.assumingMemoryBound(to: Float.self)[frame] }
        return buffers[0].mData!.assumingMemoryBound(to: Float.self)[frame * channels + channel]
    }
}

private final class TimelineHandle: @unchecked Sendable {
    let core: OpaquePointer

    convenience init(history: UInt32, a: UInt32, b: UInt32, retainSilence: Bool = false) throws {
        let widths = b == 0 ? [a] : [a, b]
        try self.init(history: history, widths: widths, retainSilence: retainSilence)
    }

    init(history: UInt32, widths: [UInt32], retainSilence: Bool = false) throws {
        guard let core = widths.withUnsafeBufferPointer({
            bt_tracks_new_lanes(SystemAudioTap.timelineRate, $0.baseAddress, $0.count,
                history, ProcessInfo.processInfo.physicalMemory)
        }) else {
            throw CaptureError("Could not allocate the capture buffer")
        }
        self.core = core
        setRetainSilence(retainSilence)
    }

    func setRetainSilence(_ enabled: Bool) {
        bt_tracks_set_retain_silence(core, enabled ? 1 : 0)
    }

    deinit { bt_tracks_free(core) }
}

private final class LaneProducer {
    private static let chunkFrames = 2048
    private let timeline: TimelineHandle
    private let lane: UInt32
    private let route: CaptureRoute
    private let outputChannels: Int
    private let scratch: UnsafeMutablePointer<Float>

    init(timeline: TimelineHandle, lane: UInt32, route: CaptureRoute, outputChannels: UInt32) {
        self.timeline = timeline
        self.lane = lane
        self.route = route
        self.outputChannels = Int(outputChannels)
        scratch = .allocate(capacity: Self.chunkFrames * Int(outputChannels))
        scratch.initialize(repeating: 0, count: Self.chunkFrames * Int(outputChannels))
    }

    deinit { scratch.deallocate() }

    func attachSender(_ id: String) -> Bool {
        id.withCString { bt_tracks_attach_sender(timeline.core, lane, $0, route.wavRouteCode) != 0 }
    }

    func detachSender() {
        bt_tracks_detach_sender(timeline.core, lane)
    }

    func push(_ buffers: UnsafeMutableAudioBufferListPointer, format: InputFormat, frameLimit: Int, time: UInt64) {
        let frames = format.availableFrames(buffers, limit: frameLimit)
        guard frames > 0 else { return }
        var offset = 0
        while offset < frames {
            let count = min(Self.chunkFrames, frames - offset)
            let packetTime = time &+ UInt64(Double(offset) * 1_000_000_000 / format.sampleRate)
            if (route == .merged || route == .allChannels) && format.channels == outputChannels {
                if !format.planar || outputChannels == 1 {
                    let start = buffers[0].mData!.assumingMemoryBound(to: Float.self).advanced(by: offset * outputChannels)
                    _ = bt_tracks_push_interleaved(timeline.core, lane, start, UInt32(count), packetTime, format.sampleRate)
                    offset += count
                    continue
                }
                if outputChannels == 2 {
                    let left = buffers[0].mData!.assumingMemoryBound(to: Float.self).advanced(by: offset)
                    let right = buffers[1].mData!.assumingMemoryBound(to: Float.self).advanced(by: offset)
                    _ = bt_tracks_push_planar_stereo(timeline.core, lane, left, right, UInt32(count), packetTime, format.sampleRate)
                } else {
                    for frame in 0..<count {
                        for channel in 0..<outputChannels {
                            scratch[frame * outputChannels + channel] =
                                format.sample(buffers, frame: offset + frame, channel: channel)
                        }
                    }
                    _ = bt_tracks_push_interleaved(timeline.core, lane, scratch, UInt32(count), packetTime, format.sampleRate)
                }
                offset += count
                continue
            }

            if route == .allChannels { return }
            for frame in 0..<count {
                let input = offset + frame
                let left = format.left.map { format.sample(buffers, frame: input, channel: $0) } ?? 0
                let right = format.right.map { format.sample(buffers, frame: input, channel: $0) } ?? 0
                switch route {
                case .merged:
                    var mixedLeft = left / format.mixScale
                    var mixedRight = right / format.mixScale
                    for part in format.mix {
                        let value = format.sample(buffers, frame: input, channel: part.channel)
                        mixedLeft += value * part.left
                        mixedRight += value * part.right
                    }
                    scratch[frame * 2] = mixedLeft
                    scratch[frame * 2 + 1] = mixedRight
                case .left: scratch[frame] = left
                case .right: scratch[frame] = right
                case .center:
                    scratch[frame] = format.center.map { format.sample(buffers, frame: input, channel: $0) } ?? 0
                case .mid: scratch[frame] = left * 0.5 + right * 0.5
                case .side: scratch[frame] = left * 0.5 - right * 0.5
                case .allChannels: break
                }
            }
            _ = bt_tracks_push_interleaved(timeline.core, lane, scratch, UInt32(count), packetTime, format.sampleRate)
            offset += count
        }
    }
}

private func packetTime(hostTime: UInt64, frames: Int, sampleRate: Double) -> UInt64 {
    if hostTime != 0 { return AudioConvertHostTimeToNanos(hostTime) }
    let current = AudioConvertHostTimeToNanos(AudioGetCurrentHostTime())
    let duration = UInt64(Double(frames) * 1_000_000_000 / sampleRate)
    return current - min(current, duration)
}

private protocol NativeInput: AnyObject {
    var format: InputFormat { get }
    func bind(_ producers: [LaneProducer]) throws
    func start() throws
    func stop() throws
}

private final class SyntheticSystemInput: NativeInput {
    let format: InputFormat
    private let width: Int
    private let stereoOnly: Bool
    private var producers: [LaneProducer] = []
    private let queue = DispatchQueue(label: "Backtracking.synthetic-system", qos: .userInitiated)
    private var timer: DispatchSourceTimer?
    private var frameIndex: UInt64 = 0
    private var packetTimestampNs: UInt64 = 0
    private var running = false

    static func parse(_ spec: String) -> (labels: [String], stereoOnly: Bool)? {
        let parts = spec.split(separator: ":", maxSplits: 1)
        guard parts.count == 2 else { return nil }
        let activity = String(parts[1])
        guard activity == "all" || activity == "stereo" else { return nil }
        let labels: [String]
        switch parts[0] {
        case "5.1": labels = ["FL", "FR", "FC", "LFE", "Ls", "Rs"]
        case "7.1": labels = ["FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs"]
        case "7.1.4": labels = ["FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "TFL", "TFR", "TBL", "TBR"]
        default: return nil
        }
        return (labels, activity == "stereo")
    }

    init(spec: String) throws {
        guard let parsed = Self.parse(spec) else {
            throw CaptureError("Unknown synthetic format \(spec)")
        }
        width = parsed.labels.count
        stereoOnly = parsed.stereoOnly
        format = InputFormat(syntheticChannelNames: parsed.labels, sampleRate: 48_000)
    }

    deinit {
        try? stop()
    }

    func bind(_ producers: [LaneProducer]) throws {
        self.producers = producers
    }

    func start() throws {
        guard !running else { return }
        running = true
        packetTimestampNs = AudioConvertHostTimeToNanos(AudioGetCurrentHostTime())
        let timer = DispatchSource.makeTimerSource(queue: queue)
        let width = self.width
        let stereoOnly = self.stereoOnly
        let format = self.format
        let producers = self.producers
        let framesPerPacket = 480
        var interleaved = [Float](repeating: 0, count: framesPerPacket * width)
        timer.schedule(deadline: .now(), repeating: .milliseconds(10), leeway: .milliseconds(1))
        timer.setEventHandler { [weak self] in
            guard let self, self.running else { return }
            let startFrame = self.frameIndex
            for f in 0..<framesPerPacket {
                let phaseFrame = Double((startFrame + UInt64(f)) % 48_000)
                for ch in 0..<width {
                    let sample: Float
                    if !stereoOnly || ch < 2 {
                        let freq = Double(220 + 30 * ch)
                        sample = Float(sin(2.0 * Double.pi * phaseFrame * freq / 48_000.0)) * 0.25
                    } else {
                        sample = 0
                    }
                    interleaved[f * width + ch] = sample
                }
            }
            let ts = self.packetTimestampNs
            self.frameIndex += UInt64(framesPerPacket)
            self.packetTimestampNs &+= 10_000_000
            interleaved.withUnsafeMutableBytes { raw in
                var list = AudioBufferList(
                    mNumberBuffers: 1,
                    mBuffers: AudioBuffer(
                        mNumberChannels: UInt32(width),
                        mDataByteSize: UInt32(raw.count),
                        mData: raw.baseAddress
                    )
                )
                withUnsafeMutablePointer(to: &list) { ptr in
                    let buffers = UnsafeMutableAudioBufferListPointer(ptr)
                    for producer in producers {
                        producer.push(buffers, format: format, frameLimit: framesPerPacket, time: ts)
                    }
                }
            }
        }
        self.timer = timer
        timer.resume()
    }

    func stop() throws {
        running = false
        timer?.cancel()
        timer = nil
    }
}

private final class SenderInput: NativeInput {
    let format: InputFormat
    private let id: String
    private var producers: [LaneProducer] = []

    init(id: String, rate: UInt32, channels: UInt32, overrideLabels: [String]? = nil) {
        self.id = id
        format = InputFormat(senderID: id, senderChannels: channels, sampleRate: rate, overrideLabels: overrideLabels)
    }

    func bind(_ producers: [LaneProducer]) throws { self.producers = producers }

    func start() throws {
        for producer in producers {
            guard producer.attachSender(id) else {
                try stop()
                throw CaptureError("Could not connect to the sender")
            }
        }
    }

    func stop() throws {
        for producer in producers { producer.detachSender() }
    }
}

private final class PlaybackTap: NativeInput {
    private var tapID = unknownObject
    private(set) var aggregateID = unknownObject
    private var ioProc: AudioDeviceIOProcID?
    private var running = false
    private var streamFormat: InputFormat?
    var format: InputFormat { streamFormat! }

    init(processes: [AudioObjectID]?, device: AudioObjectID, preferNativeLayout: Bool) throws {
        do {
            try prepare(processes: processes, device: device, preferNativeLayout: preferNativeLayout)
        } catch {
            close()
            throw error
        }
    }

    deinit { close() }

    private func prepare(processes: [AudioObjectID]?, device: AudioObjectID, preferNativeLayout: Bool) throws {
        let tap: CATapDescription
        var labels: [AudioChannelLabel] = []
        if preferNativeLayout, device != unknownObject,
           let uid = audioString(device, kAudioDevicePropertyDeviceUID) {
            labels = deviceLabels(device, scope: kAudioDevicePropertyScopeOutput)
            if deviceChannelCount(device, scope: kAudioDevicePropertyScopeOutput) > 2
                || labels.contains(kAudioChannelLabel_Center) {
                if let processes { tap = CATapDescription(processes: processes, deviceUID: uid, stream: 0) }
                else { tap = CATapDescription(excludingProcesses: [], deviceUID: uid, stream: 0) }
            } else if let processes { tap = CATapDescription(stereoMixdownOfProcesses: processes) }
            else { tap = CATapDescription(stereoGlobalTapButExcludeProcesses: []) }
        } else if let processes { tap = CATapDescription(stereoMixdownOfProcesses: processes) }
        else { tap = CATapDescription(stereoGlobalTapButExcludeProcesses: []) }

        tap.name = "Backtracking Audio"
        tap.uuid = UUID()
        tap.isPrivate = true
        tap.muteBehavior = .unmuted
        try checkAudio(AudioHardwareCreateProcessTap(tap, &tapID), "Could not create the system or application audio capture")
        let aggregate: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Backtracking Capture",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceTapAutoStartKey: false,
            kAudioAggregateDeviceTapListKey: [
                [kAudioSubTapUIDKey: tap.uuid.uuidString, kAudioSubTapDriftCompensationKey: true]
            ]
        ]
        try checkAudio(AudioHardwareCreateAggregateDevice(aggregate as CFDictionary, &aggregateID), "Could not create the audio capture device")
        var address = property(kAudioTapPropertyFormat)
        var stream = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout.size(ofValue: stream))
        try checkAudio(AudioObjectGetPropertyData(tapID, &address, 0, nil, &size, &stream), "Could not read the source format")
        streamFormat = try InputFormat(stream, labels: labels)
    }

    func bind(_ producers: [LaneProducer]) throws {
        let format = self.format
        try checkAudio(
            AudioDeviceCreateIOProcIDWithBlock(&ioProc, aggregateID, nil) { _, input, stamp, _, _ in
                let buffers = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: input))
                let frames = format.availableFrames(buffers, limit: Int.max)
                let timestamp = packetTime(hostTime: stamp.pointee.mHostTime, frames: frames, sampleRate: format.sampleRate)
                for producer in producers { producer.push(buffers, format: format, frameLimit: frames, time: timestamp) }
            },
            "Could not install the audio capture callback"
        )
    }

    func start() throws {
        guard let ioProc, !running else { return }
        try checkAudio(
            AudioDeviceStart(aggregateID, ioProc),
            "Could not start system audio capture. Check the Screen & System Audio Recording permission"
        )
        running = true
    }

    func stop() throws {
        guard let ioProc, running else { return }
        try checkAudio(AudioDeviceStop(aggregateID, ioProc), "Could not pause the audio capture device")
        running = false
    }

    private func close() {
        if let ioProc {
            try? stop()
            AudioDeviceDestroyIOProcID(aggregateID, ioProc)
        }
        if aggregateID != unknownObject { AudioHardwareDestroyAggregateDevice(aggregateID) }
        if tapID != unknownObject { AudioHardwareDestroyProcessTap(tapID) }
        // A surviving HAL callback retains its LaneProducers and therefore the Rust handle.
    }
}

private final class MicrophoneInput: NativeInput {
    private let engine: AVAudioEngine
    private let node: AVAudioInputNode
    private let audioFormat: AVAudioFormat
    let format: InputFormat
    private var installed = false

    init(device: AudioObjectID) throws {
        let engine = AVAudioEngine()
        let node = engine.inputNode
        if device != defaultDevice(kAudioHardwarePropertyDefaultInputDevice) {
            guard let unit = node.audioUnit else { throw CaptureError("Could not open the microphone input") }
            var selected = device
            try checkAudio(
                AudioUnitSetProperty(
                    unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                    &selected, UInt32(MemoryLayout.size(ofValue: selected))
                ), "Could not switch the microphone device"
            )
        }
        let audioFormat = node.outputFormat(forBus: 0)
        let labels = audioFormat.channelLayout.map { layoutLabels($0.layout) }
            ?? deviceLabels(device, scope: kAudioDevicePropertyScopeInput)
        self.format = try InputFormat(audioFormat.streamDescription.pointee, labels: labels)
        self.engine = engine
        self.node = node
        self.audioFormat = audioFormat
    }

    deinit {
        engine.stop()
        if installed { node.removeTap(onBus: 0) }
    }

    func bind(_ producers: [LaneProducer]) throws {
        let format = self.format
        node.installTap(onBus: 0, bufferSize: 1024, format: audioFormat) { buffer, when in
            let buffers = UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList)
            let frames = Int(buffer.frameLength)
            let hostTime = when.isHostTimeValid ? when.hostTime : 0
            let timestamp = packetTime(hostTime: hostTime, frames: frames, sampleRate: format.sampleRate)
            for producer in producers { producer.push(buffers, format: format, frameLimit: frames, time: timestamp) }
        }
        installed = true
    }

    func start() throws {
        if !engine.isRunning { try engine.start() }
    }

    func stop() throws { engine.stop() }
}

/// Owns one producer per distinct physical source and one timestamped core lane per output.
@MainActor
final class SystemAudioTap {
    nonisolated static let timelineRate = 48_000.0

    private struct Slot {
        let source: CaptureSource
        let indexes: [Int]
        var identity: InputIdentity?
        var input: NativeInput? = nil
        var error: String? = nil
        var retryAt = 0.0
    }

    private let timeline: TimelineHandle
    private let lanes: [CaptureLane]
    private let widths: [UInt32]
    private let routes: [CaptureRoute]
    private let laneCapErrors: [String?]
    private var slots: [Slot]
    private var running = false
    private let captureQueue = DispatchQueue(label: "Backtracking.capture")
    private var pollInFlight = false
    private var telemetryVersion: UInt64 = 0
    private var lastActivityPoll = 0.0
    private var activeTrackers: [ChannelActivityTracker]
    private var positionBuffers: [LanePositionBuffer?]
    private(set) var cachedState: UInt32 = 0
    private static let silentMeter = BtMeterReadings(
        peak_dbfs: -Float.infinity, vu: -Float.infinity, momentary_lufs: -Float.infinity)
    private(set) var cachedMeters: [BtMeterReadings]
    var core: OpaquePointer { timeline.core }
    var laneCount: Int { lanes.count }
    /// The channels of the lanes that start; a lane past the limit holds none (Windows `track_channels`).
    var totalChannelCount: UInt32 { Self.channelsInUse(widths: widths, capErrors: laneCapErrors) }

    /// Channels the capture core holds for sources that cannot start: one each (Windows holds none).
    var channelsKeptForRefusedLanes: UInt32 { UInt32(laneCapErrors.filter { $0 != nil }.count) }

    /// The Sources page's count, in English and Traditional Chinese: the channels in use, and the
    /// ones kept for sources that cannot start, which count toward the 64 too.
    nonisolated static func channelsInUseNote(used: UInt32, kept: UInt32) -> String {
        guard kept > 0 else { return "\(used) of 64 channels in use / 已使用 \(used) 個聲道，上限 64 個" }
        return "\(used) of 64 channels in use, and \(kept) kept for sources that cannot start / 已使用 \(used) 個聲道，另有 \(kept) 個保留給無法開始的來源，上限 64 個"
    }

    /// Why source `lane` cannot be added or changed: the sources would hold `total` channels,
    /// `kept` of them one for each source that cannot start.
    nonisolated static func addRefusal(lane: Int, total: UInt32, kept: UInt32) -> String {
        let letter = captureLaneLetter(lane)
        guard kept > 0 else {
            return "Source \(letter) would bring the total to \(total) channels. The limit is 64. / 來源 \(letter) 會使總數變成 \(total) 個聲道，上限為 64。"
        }
        return "Source \(letter) would bring the total to \(total) channels, with one channel kept for each source that cannot start. The limit is 64. / 來源 \(letter) 會使總數變成 \(total) 個聲道（每個無法開始的來源保留 1 個），上限為 64。"
    }

    nonisolated static func channelsInUse(widths: [UInt32], capErrors: [String?]) -> UInt32 {
        zip(widths, capErrors).reduce(0) { $0 + ($1.1 == nil ? $1.0 : 0) }
    }

    /// CAP-16: lanes start in order up to 64 channels in all; one that would go past does not
    /// start, and later ones that still fit do (Windows `controller::lane_fits`). Unlike Windows,
    /// a lane that does not start still holds one channel in the capture core, which takes at
    /// most 64 in all. When those placeholders would push the capture past 64, the latest lanes
    /// that started give way too, so only lanes are refused and the capture itself still opens.
    nonisolated static func acceptLanes(_ requested: [UInt32]) -> (widths: [UInt32], errors: [String?]) {
        var widths: [UInt32] = []
        var errors: [String?] = []
        var total: UInt32 = 0
        for (index, width) in requested.enumerated() {
            if total + width <= 64 {
                total += width
                widths.append(width)
                errors.append(nil)
            } else {
                errors.append(overLimitMessage(lane: index, total: total + width))
                widths.append(1)
            }
        }
        return (widths, errors)
    }

    /// Why the capture cannot open at all, or nil. A lane that does not start still holds one
    /// channel of the capture core here (Windows leaves it out), so the lanes that start and those
    /// placeholders can pass 64 together. Then nothing records, and the window says so.
    nonisolated static func captureOverLimitMessage(_ held: [UInt32]) -> String? {
        let total = held.reduce(0, +)
        guard total > 64 else { return nil }
        return "Capture cannot start: with one channel kept for each source that cannot start, the sources would hold \(total) channels; the limit is 64. Remove a source or choose a narrower route."
    }

    /// Why lane `lane` cannot start after lanes holding `total - width` channels, in English and
    /// Traditional Chinese on one line, worded as Windows `controller::lane_fits`.
    nonisolated static func overLimitMessage(lane: Int, total: UInt32) -> String {
        let letter = captureLaneLetter(lane)
        let english = lane <= 1 ? "lanes A and \(letter)" : "lanes A to \(letter)"
        let chinese = lane <= 1 ? "來源 A 與 \(letter)" : "來源 A 到 \(letter)"
        return "Lane \(letter) cannot start: \(english) would contain \(total) channels; the limit is 64 / 來源 \(letter) 無法開始：\(chinese) 合計會有 \(total) 個聲道，上限為 64。"
    }
    var retainedSeconds: Double { Double(bt_tracks_retention_frames(core)) / Self.timelineRate }
    var captureRoutes: [CaptureRoute] { routes }
    var captureChannelNames: [[String]] {
        lanes.indices.map { index in
            guard widths[index] > 2 else { return [] }
            if case .addon(let addonID, let sourceID) = lanes[index].source,
               let (_, src) = currentAddonSnapshot.source(addonID: addonID, sourceID: sourceID) {
                let labels = src.allChannelLabels
                if labels.count == Int(widths[index]) {
                    return labels
                }
            }
            return slots.first(where: { $0.indexes.contains(index) })?.input?.format.channelNames
                ?? (1...Int(widths[index])).map { "Channel \($0)" }
        }
    }

    func channelGroups(for lane: Int) -> [AddonChannelGroup] {
        guard lanes.indices.contains(lane), widths.indices.contains(lane), widths[lane] > 2 else { return [] }
        guard case .addon(let addonID, let sourceID) = lanes[lane].source,
              let (_, src) = currentAddonSnapshot.source(addonID: addonID, sourceID: sourceID),
              let groups = src.groups, !groups.isEmpty else {
            return []
        }
        let total = groups.reduce(0) { $0 + $1.channels.count }
        guard total == Int(widths[lane]) else { return [] }
        return groups
    }

    func ingestAddonPositions(_ events: [AddonPositionEvent]) -> Bool {
        guard positionBuffers.contains(where: { $0 != nil }) else { return false }
        for event in events {
            for (index, lane) in lanes.enumerated() {
                guard case .addon(let addonID, let sourceID) = lane.source,
                      addonID == event.addon_id, sourceID == event.source_id,
                      let buf = positionBuffers[index] else { continue }
                buf.enqueue(timeNs: event.time, channels: event.channels)
            }
        }
        var overflowed = false
        let timeline = self.timeline
        captureQueue.sync {
            let first = bt_tracks_first_frame(timeline.core)
            let end = bt_tracks_end_frame(timeline.core)
            let retained: Range<UInt64>? = first < end ? first..<end : nil
            for (index, buf) in positionBuffers.enumerated() {
                guard let buf else { continue }
                var evictedFirst: UInt64 = 0
                let evicted: UInt64? = bt_tracks_take_frame_clock_eviction(
                    timeline.core, UInt32(index), &evictedFirst
                ) != 0 ? evictedFirst : nil
                buf.drain(evictedBefore: evicted, retainedRange: retained) { timeNs in
                    var frame: UInt64 = 0
                    let status = bt_tracks_retained_frame_at(timeline.core, UInt32(index), timeNs, &frame)
                    return (status, frame)
                }
                if buf.takeOverflowed() {
                    overflowed = true
                }
            }
        }
        return overflowed
    }

    func clearPositionTracks() {
        for buf in positionBuffers {
            buf?.clear()
        }
    }

    func freezePositions() -> [Int: FrozenPositionsHandle] {
        var result: [Int: FrozenPositionsHandle] = [:]
        for (index, buf) in positionBuffers.enumerated() {
            if let frozen = buf?.freeze() {
                result[index] = frozen
            }
        }
        return result
    }

    func channelCount(for lane: Int) -> UInt32? {
        widths.indices.contains(lane) ? widths[lane] : nil
    }

    /// Only wide (>2 ch) system, microphone, and application sources hide silent channels.
    /// Sender and add-on sources always show every channel.
    func canHideSilentChannels(for lane: Int) -> Bool {
        guard lanes.indices.contains(lane), widths.indices.contains(lane) else { return false }
        return widths[lane] > 2 && !lanes[lane].source.isSenderBacked
    }

    func isWideSender(lane: Int) -> Bool {
        guard lanes.indices.contains(lane), widths.indices.contains(lane) else { return false }
        return lanes[lane].source.isSenderBacked && widths[lane] > 2
    }

    func liveActiveMask(for lane: Int, hideSilent: Bool) -> UInt64? {
        guard hideSilent, canHideSilentChannels(for: lane), activeTrackers.indices.contains(lane) else { return nil }
        return activeTrackers[lane].shown ?? 0
    }

    var privatePlaybackDevices: Set<AudioObjectID> {
        Set(slots.compactMap { ($0.input as? PlaybackTap)?.aggregateID })
    }

    init(history: UInt32, lanes: [CaptureLane], retainSilence: Bool = false) throws {
        guard (1...maximumCaptureLanes).contains(lanes.count) else {
            throw CaptureError("Capture takes 1 to 64 sources")
        }
        cachedMeters = Array(repeating: Self.silentMeter, count: lanes.count)
        activeTrackers = Array(repeating: ChannelActivityTracker(), count: lanes.count)
        positionBuffers = Array(repeating: nil, count: lanes.count)
        self.lanes = lanes
        var slots: [Slot] = []
        for (index, lane) in lanes.enumerated() {
            let current = identity(for: lane.source)
            if let existing = slots.firstIndex(where: { $0.source == lane.source }) {
                let old = slots[existing]
                slots[existing] = Slot(source: old.source, indexes: old.indexes + [index], identity: current)
            } else {
                slots.append(Slot(source: lane.source, indexes: [index], identity: current))
            }
        }
        for index in slots.indices {
            do {
                slots[index].input = try Self.makeInput(
                    slots[index].identity, preferNative: slots[index].indexes.contains { [.center, .allChannels].contains(lanes[$0].route) }
                )
            } catch {
                slots[index].error = error.localizedDescription
                slots[index].retryAt = ProcessInfo.processInfo.systemUptime + 2
            }
        }
        let formats = lanes.indices.map { index in
            slots.first(where: { $0.indexes.contains(index) })?.input?.format
        }
        let routes = lanes.enumerated().map { index, lane -> CaptureRoute in
            // CAP-10: A wide Sender or add-on source (>2 channels) always captures all native channels.
            if lane.source.isSenderBacked, let format = formats[index], format.channels > 2 {
                return .allChannels
            }
            return lane.route == .allChannels && formats[index]?.supports(.allChannels) == false ? .merged : lane.route
        }
        let requestedWidths = lanes.enumerated().map { index, lane -> UInt32 in
            let route = routes[index]
            guard let format = formats[index], format.supports(route) else {
                return route == .merged && !lane.source.isMicrophone ? 2 : 1
            }
            return format.width(for: route)
        }
        let (acceptedWidths, capErrors) = Self.acceptLanes(requestedWidths)
        if let overLimit = Self.captureOverLimitMessage(acceptedWidths) { throw CaptureError(overLimit) }
        self.routes = routes
        self.widths = acceptedWidths
        self.laneCapErrors = capErrors
        self.timeline = try TimelineHandle(history: history, widths: acceptedWidths, retainSilence: retainSilence)
        self.slots = slots
        // Enable frame clock & position tracking for wide add-on lanes (Pkg 19).
        for (index, lane) in lanes.enumerated() where lane.source.isAddon && acceptedWidths[index] > 2 {
            if bt_tracks_enable_frame_clock(timeline.core, UInt32(index)) != 0 {
                self.positionBuffers[index] = LanePositionBuffer()
            }
        }
        // A selection preview from any Backtracking is heard in System playback. Those lanes
        // record silence while it plays, so the preview never enters the history.
        // A test copy neither plays previews nor joins the shared preview gate.
        if !TestCopyConfig.current.isTestCopy {
            for (index, lane) in lanes.enumerated() where lane.source == .system {
                bt_tracks_set_preview_silence(timeline.core, UInt32(index), 1)
            }
        }
        for index in self.slots.indices { bind(index) }
    }

    static var requiresMicrophonePrompt: Bool {
        AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined
    }

    static func requestMicrophonePermission(_ completion: @escaping @Sendable () -> Void) {
        AVCaptureDevice.requestAccess(for: .audio) { _ in completion() }
    }

    static func estimatedWidth(for lane: CaptureLane) -> UInt32 {
        guard lane.route == .merged || lane.route == .allChannels else { return 1 }
        let channels: Int
        switch identity(for: lane.source) {
        case .syntheticSystem(let spec):
            channels = SyntheticSystemInput.parse(spec)?.labels.count ?? 2
        case .sender(_, _, let count, _): channels = Int(count)
        case .microphone(let device, _): channels = inputChannels(device)
        case .system(let device, _), .application(_, let device, _):
            channels = max(2, deviceChannelCount(device, scope: kAudioDevicePropertyScopeOutput))
        case nil: channels = lane.source.isMicrophone ? 1 : 2
        }
        if (lane.route == .allChannels || lane.source.isSenderBacked), channels > 2 { return UInt32(channels) }
        return UInt32(max(1, min(2, channels)))
    }

    private static func makeInput(_ identity: InputIdentity?, preferNative: Bool) throws -> NativeInput? {
        switch identity {
        case .syntheticSystem(let spec): try SyntheticSystemInput(spec: spec)
        case .system(let device, _): try PlaybackTap(processes: nil, device: device, preferNativeLayout: preferNative)
        case .application(let processes, let device, _):
            try PlaybackTap(processes: processes, device: device, preferNativeLayout: preferNative)
        case .microphone(let device, _): try MicrophoneInput(device: device)
        case .sender(let id, let rate, let channels, let labels):
            SenderInput(id: id, rate: rate, channels: channels, overrideLabels: labels)
        case nil: nil
        }
    }

    private func bind(_ index: Int) {
        guard let input = slots[index].input else { return }
        let available = slots[index].indexes.filter { laneCapErrors[$0] == nil && input.format.supports(routes[$0]) }
        let producers = available.map { lane in
            LaneProducer(timeline: timeline, lane: UInt32(lane), route: routes[lane], outputChannels: widths[lane])
        }
        guard !producers.isEmpty else { return }
        do { try input.bind(producers) }
        catch {
            slots[index].error = error.localizedDescription
            slots[index].input = nil
            slots[index].retryAt = ProcessInfo.processInfo.systemUptime + 2
        }
    }

    func supports(_ route: CaptureRoute, lane: Int) -> Bool {
        guard lanes.indices.contains(lane) else { return false }
        if isWideSender(lane: lane) { return route == .merged }
        guard let format = slots.first(where: { $0.indexes.contains(lane) })?.input?.format else {
            return route != .center && route != .allChannels
        }
        if format.supports(route) { return true }
        if (route == .center || route == .allChannels), !lanes[lane].source.isMicrophone, !lanes[lane].source.isSender,
           let device = defaultDevice(kAudioHardwarePropertyDefaultOutputDevice) {
            let labels = deviceLabels(device, scope: kAudioDevicePropertyScopeOutput)
            return route == .center ? labels.contains(kAudioChannelLabel_Center)
                : deviceChannelCount(device, scope: kAudioDevicePropertyScopeOutput) > 2
        }
        return false
    }

    /// The short capture status of an add-on source that is not delivering audio, in Windows'
    /// words (`controller.rs`); the Sources page explains it at length.
    nonisolated static func addonCaptureStatus(_ addon: AddonRecord?, sourceID: String) -> String {
        guard let addon else { return "Add-on is not installed" }
        switch addon.state {
        case "running":
            return addon.sources.contains(where: { $0.id == sourceID }) ? "Waiting for add-on audio" : "Add-on source is not available"
        case "starting": return "Add-on is starting"
        case "stopping": return "Add-on is stopping"
        case "stopped": return "Add-on is stopped"
        case "not_responding": return "Add-on is not responding"
        default: return "Add-on unavailable: \(addon.reason ?? addon.state)"
        }
    }

    func sourceProblem(for lane: Int) -> String? {
        guard lanes.indices.contains(lane) else { return nil }
        if let capError = laneCapErrors[lane] {
            return localizedBilingual(capError)
        }
        guard let slot = slots.first(where: { $0.indexes.contains(lane) }) else { return nil }
        if let error = slot.error { return localizedBilingual(error) }
        if slot.input == nil {
            if case .addon(let addonID, let sourceID) = slot.source {
                return Self.addonCaptureStatus(currentAddonSnapshot.addon(addonID), sourceID: sourceID)
            }
            if slot.source.isSender {
                return "Sender audio is unavailable"
            }
            if slot.source.isMicrophone, AVCaptureDevice.authorizationStatus(for: .audio) != .authorized {
                return "needs microphone permission"
            }
            if case .application(let identifier) = slot.source {
                return localizedBilingual("Waiting for application to start or allow access (\(identifier)) / 等待應用程式啟動或允許存取（\(identifier)）")
            }
            return "waiting for the source"
        }
        if let input = slot.input, !lanes[lane].source.isSenderBacked, !input.format.supports(lanes[lane].route) {
            if lanes[lane].route == .allChannels, routes[lane] == .merged {
                return "capturing in Merged because this source has fewer than three channels"
            }
            return "this source has no \(lanes[lane].route.label) channel"
        }
        return nil
    }

    var sourceMessage: String? {
        let problems: [(Int, String)] = lanes.indices.compactMap { lane in
            sourceProblem(for: lane).map { (lane, $0) }
        }
        guard let first = problems.first else { return nil }
        if lanes.count == 1 {
            return first.1
        }
        let head = "Source \(captureLaneLetter(first.0)): \(first.1)"
        let extra = problems.count - 1
        if extra == 0 { return head }
        if extra == 1 { return "\(head) · 1 more source has a problem" }
        return "\(head) · \(extra) more sources have a problem"
    }

    private func start(_ index: Int) {
        guard let input = slots[index].input,
              slots[index].indexes.contains(where: { laneCapErrors[$0] == nil && input.format.supports(routes[$0]) }) else { return }
        do {
            try input.start()
            slots[index].error = nil
        } catch {
            slots[index].error = error.localizedDescription
            slots[index].retryAt = ProcessInfo.processInfo.systemUptime + 2
        }
    }

    func pollCapture(synchronous: Bool = false) {
        let timeline = self.timeline
        if synchronous {
            captureQueue.sync { bt_tracks_poll(timeline.core) }
            return
        }
        guard !pollInFlight else { return }
        pollInFlight = true
        let version = telemetryVersion
        let count = laneCount
        let now = ProcessInfo.processInfo.systemUptime
        let shouldPollActivity = (now - lastActivityPoll) >= ChannelActivityTracker.pollInterval
        let wideLanes = shouldPollActivity ? lanes.indices.filter { canHideSilentChannels(for: $0) } : []
        if shouldPollActivity { lastActivityPoll = now }
        captureQueue.async { [weak self] in
            bt_tracks_poll(timeline.core)
            let state = bt_tracks_state(timeline.core)
            let meters = (0..<count).map { bt_tracks_meter_readings(timeline.core, UInt32($0)) }
            let activity: [(Int, UInt64)] = wideLanes.map { lane in
                (lane, bt_tracks_active_channels(timeline.core, UInt32(lane)))
            }
            DispatchQueue.main.async { [weak self] in
                if let self, self.telemetryVersion == version {
                    self.cachedState = state
                    self.cachedMeters = meters
                    for (lane, mask) in activity where self.activeTrackers.indices.contains(lane) {
                        self.activeTrackers[lane].recordAnswer(mask)
                    }
                }
                self?.pollInFlight = false
            }
        }
    }

    // UI callers never wait for the capture queue to copy or summarize audio.
    // Keeping the handle alive in the closure also makes late completions safe
    // after a source has been replaced on the main thread.
    func readCapture<T: Sendable>(
        poll: Bool = true, _ read: @escaping @Sendable (OpaquePointer) -> T,
        completion: @escaping @MainActor (T) -> Void
    ) {
        let timeline = self.timeline
        captureQueue.async {
            if poll { bt_tracks_poll(timeline.core) }
            let result = read(timeline.core)
            DispatchQueue.main.async { completion(result) }
        }
    }

    func withCaptureRead<T>(_ read: (OpaquePointer) -> T) -> T {
        let timeline = self.timeline
        return captureQueue.sync { read(timeline.core) }
    }

    func withCaptureSnapshot<T>(_ read: (OpaquePointer) -> T) -> T {
        let timeline = self.timeline
        return captureQueue.sync {
            bt_tracks_poll(timeline.core)
            return read(timeline.core)
        }
    }

    func setHistory(_ seconds: UInt32, completion: @escaping (Bool) -> Void) {
        let timeline = self.timeline
        captureQueue.async {
            let success = bt_tracks_set_history(timeline.core, seconds) == 1
            DispatchQueue.main.async { completion(success) }
        }
    }

    func setRetainSilence(_ enabled: Bool) {
        let timeline = self.timeline
        captureQueue.async { timeline.setRetainSilence(enabled) }
    }

    func resume() throws {
        if running { try pause() }
        telemetryVersion &+= 1
        bt_tracks_resume(core)
        for index in slots.indices { start(index) }
        running = true
        cachedState = 0
    }

    func pause() throws {
        telemetryVersion &+= 1
        bt_tracks_pause(core)
        cachedState = 2
        cachedMeters = Array(repeating: Self.silentMeter, count: laneCount)
        guard running else { return }
        var firstError: Error?
        for input in slots.compactMap(\.input) {
            do { try input.stop() }
            catch { if firstError == nil { firstError = error } }
        }
        if let firstError { throw firstError }
        running = false
    }

    /// A native channel-count change requires a new timeline. Process restarts with the
    /// same format can replace their tap without disturbing the other source or its history.
    func refreshSources() -> Bool {
        guard running else { return false }
        let now = ProcessInfo.processInfo.systemUptime
        let identities = lanes.map { identity(for: $0.source) }
        for index in slots.indices {
            let next = identities[slots[index].indexes[0]]
            let unchanged = next == slots[index].identity
            if unchanged {
                guard next != nil, slots[index].error != nil, now >= slots[index].retryAt else { continue }
                if slots[index].input != nil {
                    start(index)
                    continue
                }
            }
            do { try slots[index].input?.stop() }
            catch {
                slots[index].error = error.localizedDescription
                slots[index].retryAt = now + 2
                continue
            }
            slots[index].input = nil
            slots[index].identity = next
            slots[index].error = nil
            do {
                let input = try Self.makeInput(
                    next, preferNative: slots[index].indexes.contains { [.center, .allChannels].contains(lanes[$0].route) }
                )
                if let input, slots[index].indexes.contains(where: {
                    let wanted = lanes[$0].route
                    let route: CaptureRoute
                    if lanes[$0].source.isSenderBacked, input.format.channels > 2 {
                        route = .allChannels
                    } else {
                        route = wanted == .allChannels && !input.format.supports(wanted) ? .merged : wanted
                    }
                    return route != routes[$0] || (input.format.supports(route) && input.format.width(for: route) != widths[$0])
                }) {
                    return true
                }
                slots[index].input = input
                bind(index)
                start(index)
            } catch {
                slots[index].error = error.localizedDescription
                slots[index].retryAt = now + 2
            }
        }
        return false
    }
}

#if BACKTRACKING_CAPTURE_VERIFY
private func verifyMacWaveformGeometry() throws {
    func expect(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        if !condition() { throw CaptureError(message) }
    }
    var gate = WaveformRenderGate()
    try expect(gate.request(), "The initial waveform request was not started")
    try expect(!gate.request() && !gate.request(), "Concurrent waveform requests were started")
    try expect(gate.complete(stale: true) && !gate.inFlight,
               "A stale paused-view result did not request the latest view")
    try expect(gate.request() && !gate.complete(stale: false),
               "Waveform retries did not coalesce or spun after a valid result")
    try expect(gate.request() && !gate.request() && gate.complete(stale: false),
               "A pending refresh was lost when its request parameters stayed equal")
    try expect(gate.request() && !gate.complete(stale: false), "A paused refresh did not finish")
    try expect(gate.request() && gate.complete(stale: true),
               "A changed view was lost without a timer-driven refresh")
    try expect(gate.request() && !gate.complete(stale: false) && !gate.complete(stale: true),
               "Duplicate completion restarted an idle waveform renderer")
    let samples: [Float] = [0.25, -0.5, 2, -2]
    let expected = WaveformDisplayGeometry.samples([[
        WaveformSamplePoint(x: 0, value: 0.25), WaveformSamplePoint(x: 0.25, value: -0.5),
        WaveformSamplePoint(x: 0.5, value: 1), WaveformSamplePoint(x: 0.75, value: -1)
    ]])
    try expect(waveformDisplayGeometry(minimum: samples, maximum: samples, visibleFrames: 100..<104) == expected,
               "Sample geometry lost the original frame positions")
    try expect(waveformDisplayGeometry(minimum: samples, maximum: samples,
               visibleFrames: (UInt64.max - 4)..<UInt64.max) == expected,
               "Sample geometry changed with a large absolute frame origin")
    let missing: [Float] = [0.25, .nan, -0.5]
    try expect(waveformDisplayGeometry(minimum: missing, maximum: missing, visibleFrames: 0..<3) == .samples([
        [WaveformSamplePoint(x: 0, value: 0.25)], [WaveformSamplePoint(x: 2.0 / 3, value: -0.5)]
    ]), "Sample geometry connected across unknown audio")
    guard case .envelope(let runs) = waveformDisplayGeometry(
        minimum: [0.1, .nan, 0.8], maximum: [0.4, .nan, -0.3], visibleFrames: 10..<110
    ) else { throw CaptureError("Aggregated peaks were rendered as raw samples") }
    try expect(runs.count == 2 && runs[0].count == 3 && runs[1].count == 3,
               "Envelope geometry did not separate finite runs")
    try expect(runs[0].first?.x == 0 && runs[0].last?.x == 1.0 / 3
               && runs[1].first?.x == 2.0 / 3 && runs[1].last?.x == 1,
               "Envelope geometry omitted bin edges or crossed unknown audio")
    try expect(runs[1].allSatisfy { $0.minimum == -0.3 && $0.maximum == 0.8 },
               "Envelope geometry did not sort the supplied extrema")
    try expect(waveformDisplayGeometry(minimum: [-2], maximum: [2], visibleFrames: 0..<100) == .envelope([[
        WaveformEnvelopeColumn(x: 0, minimum: -1, maximum: 1),
        WaveformEnvelopeColumn(x: 0.5, minimum: -1, maximum: 1),
        WaveformEnvelopeColumn(x: 1, minimum: -1, maximum: 1)
    ]]), "A single envelope bin did not fill its time interval")
    try expect(waveformDisplayGeometry(minimum: [0.25], maximum: [0.25], visibleFrames: 0..<1)
               == .samples([[WaveformSamplePoint(x: 0, value: 0.25)]]),
               "A single sample was shifted to a bin center")
    guard case .envelope = waveformDisplayGeometry(
        minimum: [0.25, 0.25], maximum: [0.25, 0.25], visibleFrames: 0..<1
    ) else { throw CaptureError("Repeated bins were misclassified as raw samples") }
    guard case .envelope = waveformDisplayGeometry(
        minimum: [-0.2, 0.1], maximum: [0.3, 0.2], visibleFrames: 0..<2
    ) else { throw CaptureError("Unequal peak pairs were misclassified as raw samples") }
    try expect(waveformDisplayGeometry(minimum: [0, 0], maximum: [0, 0], visibleFrames: 0..<96_000)
               == .envelope([[
                   WaveformEnvelopeColumn(x: 0, minimum: 0, maximum: 0),
                   WaveformEnvelopeColumn(x: 0.25, minimum: 0, maximum: 0),
                   WaveformEnvelopeColumn(x: 0.75, minimum: 0, maximum: 0),
                   WaveformEnvelopeColumn(x: 1, minimum: 0, maximum: 0)
               ]]), "Quiet envelope geometry did not cover the continuous visible interval")
    try expect(waveformDisplayGeometry(minimum: [.nan], maximum: [.infinity], visibleFrames: 0..<10) == .empty,
               "Invalid peak data produced geometry")
    try expect(waveformDisplayGeometry(minimum: [], maximum: [], visibleFrames: 0..<10) == .empty,
               "Empty peak data produced geometry")
    try expect(waveformDisplayGeometry(minimum: [0], maximum: [0], visibleFrames: 3..<3) == .empty,
               "An empty frame range produced geometry")
    let excessive = [Float](repeating: 0, count: 4_097)
    try expect(waveformDisplayGeometry(minimum: excessive, maximum: excessive, visibleFrames: 0..<10_000) == .empty,
               "Geometry exceeded the bounded bin capacity")
    try expect(minimumWaveformZoomFrames(width: 800, retainedFrames: 10_000) == 200
               && minimumWaveformZoomFrames(width: 800, retainedFrames: 100) == 100,
               "Sample zoom did not follow the occupied audio width")
    try expect(minimumWaveformZoomFrames(width: .nan, retainedFrames: 100) == 2
               && minimumWaveformZoomFrames(width: .infinity, retainedFrames: 100) == 2
               && minimumWaveformZoomFrames(width: 1, retainedFrames: 1) == 1
               && minimumWaveformZoomFrames(width: 1, retainedFrames: 0) == 0
               && minimumWaveformZoomFrames(width: .greatestFiniteMagnitude, retainedFrames: 10_000) == 4_096,
               "Invalid or extreme zoom geometry was not bounded")
    try expect(zoomedWaveformRange(1_000..<1_100, in: 0..<10_000, width: 800,
                                  cursorFraction: 0.5, delta: 1) == 1_000..<1_100,
               "Zooming in after a resize made the view wider")
    let zoomed = zoomedWaveformRange(1_000..<2_000, in: 0..<10_000, width: 800,
                                    cursorFraction: 0.25, delta: log(2))
    try expect(zoomed == 1_125..<1_625, "Zoom did not preserve the cursor's sample position")
    let origin = UInt64.max - 10_000
    try expect(zoomedWaveformRange((origin + 1_000)..<(origin + 2_000), in: origin..<UInt64.max,
                                  width: 800, cursorFraction: 0.25, delta: log(2))
               == (origin + 1_125)..<(origin + 1_625), "Zoom lost precision at a large frame origin")
}

private func verifyMacRetainSilenceOption() throws {
    let blockFrames = 4_096
    func push(_ timeline: TimelineHandle, a: Float, b: Float, sequence: UInt64) {
        let time = 1_000_000_000 + sequence * UInt64(blockFrames) * 1_000_000_000 / 48_000
        let left = [Float](repeating: a, count: blockFrames)
        let right = [Float](repeating: b, count: blockFrames)
        left.withUnsafeBufferPointer {
            _ = bt_tracks_push_interleaved(timeline.core, 0, $0.baseAddress, UInt32(blockFrames),
                                           time, SystemAudioTap.timelineRate)
        }
        right.withUnsafeBufferPointer {
            _ = bt_tracks_push_interleaved(timeline.core, 1, $0.baseAddress, UInt32(blockFrames),
                                           time, SystemAudioTap.timelineRate)
        }
    }

    let disabled = try TimelineHandle(history: 10, a: 1, b: 0, retainSilence: true)
    let disabledProducer = LaneProducer(timeline: disabled, lane: 0, route: .merged, outputChannels: 1)
    let disabledFormat = InputFormat(senderID: "", senderChannels: 1, sampleRate: 48_000)
    var disabledBuffers = AudioBufferList(
        mNumberBuffers: 1,
        mBuffers: AudioBuffer(mNumberChannels: 1, mDataByteSize: UInt32(blockFrames * 4), mData: nil)
    )
    withUnsafeMutablePointer(to: &disabledBuffers) {
        disabledProducer.push(UnsafeMutableAudioBufferListPointer($0), format: disabledFormat,
                              frameLimit: blockFrames, time: 1_000_000_000)
    }
    bt_tracks_pause(disabled.core)
    guard bt_tracks_end_frame(disabled.core) == bt_tracks_first_frame(disabled.core) else {
        throw CaptureError("A disabled stream was converted into silent audio")
    }

    let compressed = try TimelineHandle(history: 10, a: 1, b: 1)
    push(compressed, a: 0.00001, b: -0.00001, sequence: 0)
    bt_tracks_pause(compressed.core)
    guard bt_tracks_end_frame(compressed.core) == bt_tracks_first_frame(compressed.core) else {
        throw CaptureError("Quiet sections were retained by default")
    }

    let retained = try TimelineHandle(history: 10, a: 1, b: 1, retainSilence: true)
    push(retained, a: 0, b: 0, sequence: 0)
    retained.setRetainSilence(false)
    guard bt_tracks_end_frame(retained.core) - bt_tracks_first_frame(retained.core) == UInt64(blockFrames),
          bt_tracks_state(retained.core) != 2 else {
        throw CaptureError("Changing quiet retention cleared audio or paused capture")
    }
    push(retained, a: 0.00001, b: -0.00001, sequence: 1)
    retained.setRetainSilence(true)
    guard bt_tracks_end_frame(retained.core) - bt_tracks_first_frame(retained.core) == UInt64(blockFrames),
          bt_tracks_state(retained.core) != 2 else {
        throw CaptureError("Enabling quiet retention restored discarded audio or paused capture")
    }
    push(retained, a: 0.00002, b: -0.00003, sequence: 2)
    bt_tracks_pause(retained.core)
    var sharedFirst: UInt64? = nil
    let expectedLanes: [(UInt32, Float)] = [(0, 0.00002), (1, -0.00003)]
    for (lane, value) in expectedLanes {
        guard let snapshot = bt_tracks_snapshot(retained.core, lane) else {
            throw CaptureError("Quiet-retained track has no snapshot")
        }
        defer { bt_snapshot_free(snapshot) }
        let first = bt_snapshot_first_frame(snapshot)
        guard bt_snapshot_frame_count(snapshot) == UInt64(blockFrames * 2),
              sharedFirst == nil || sharedFirst == first else {
            throw CaptureError("Quiet retention changed the A/B frame boundaries")
        }
        sharedFirst = first
        let sections: [(UInt64, Float)] = [(0, 0), (UInt64(blockFrames), value)]
        for (offset, expected) in sections {
            let count = 64
            var minimum = [Float](repeating: 0, count: count)
            var maximum = [Float](repeating: 0, count: count)
            let start = first + offset
            let frames = minimum.withUnsafeMutableBufferPointer { low in
                maximum.withUnsafeMutableBufferPointer { high in
                    bt_snapshot_waveform_range(snapshot, 0, start, start + UInt64(count),
                                               low.baseAddress, high.baseAddress, UInt32(count))
                }
            }
            guard frames == UInt64(count), minimum.allSatisfy({ abs($0 - expected) < 0.000000001 }),
                  maximum.allSatisfy({ abs($0 - expected) < 0.000000001 }) else {
                throw CaptureError("Quiet retention changed the original sample values")
            }
        }
    }
    retained.setRetainSilence(false)
    guard bt_tracks_state(retained.core) == 2 else {
        throw CaptureError("Quiet retention released a manual pause")
    }
}

private func verifyMacMultiSourceCapture() throws {
    let timeline = try TimelineHandle(history: 10, widths: [1, 2, 6], retainSilence: true)
    let frames = 4_800
    let levels: [Float] = [0.125, 0.25, 0.5]
    for (lane, width) in [1, 2, 6].enumerated() {
        let samples = [Float](repeating: levels[lane], count: frames * width)
        let accepted = samples.withUnsafeBufferPointer {
            bt_tracks_push_interleaved(timeline.core, UInt32(lane), $0.baseAddress,
                UInt32(frames), 1_000_000_000, SystemAudioTap.timelineRate)
        }
        guard accepted == UInt32(frames) else { throw CaptureError("Lane did not accept its full input") }
    }
    bt_tracks_pause(timeline.core)
    let first = bt_tracks_first_frame(timeline.core)
    let end = bt_tracks_end_frame(timeline.core)
    guard end - first == UInt64(frames), let frozen = bt_tracks_freeze(timeline.core, first, end) else {
        throw CaptureError("The three lanes did not retain a shared range")
    }
    defer { bt_frozen_free(frozen) }
    bt_tracks_clear(timeline.core)

    let folder = FileManager.default.temporaryDirectory.appendingPathComponent("Backtracking-verification-\(UUID())")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: false)
    defer { try? FileManager.default.removeItem(at: folder) }
    let selectionStart = first + 200
    let selectionEnd = first + 500
    let fades = BtFades(fade_in: 0, fade_out: 0)
    let lanes = [
        BtFrozenLane(index: 0, route: 5, source_name: nil, channel_labels: nil, view: 0),
        BtFrozenLane(index: 1, route: 5, source_name: nil, channel_labels: nil, view: 0),
        BtFrozenLane(index: 2, route: 7, source_name: nil, channel_labels: nil, view: 0)
    ]
    func export(_ selected: [BtFrozenLane], mode: UInt32 = 2) throws -> [URL] {
        let paths = folder.path.withCString { path in
            selected.withUnsafeBufferPointer {
                bt_frozen_export(frozen, $0.baseAddress, $0.count, selectionStart, selectionEnd,
                    path, 48_000, 32, 0, mode, 0, fades)
            }
        }
        guard let paths else { throw CaptureError("Aligned export failed") }
        defer { bt_export_paths_free(paths) }
        return (0..<bt_export_paths_count(paths)).compactMap { index in
            bt_export_path(paths, index).map { URL(fileURLWithPath: String(cString: $0)) }
        }
    }
    func checkFile(_ url: URL, lane: Int, width: UInt32) throws {
        let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
        let count = AVAudioFrameCount(selectionEnd - selectionStart)
        guard file.processingFormat.channelCount == width, file.length == Int64(count),
              file.processingFormat.sampleRate == 48_000,
              let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: count) else {
            throw CaptureError("The frozen WAV has the wrong channels or boundaries")
        }
        try file.read(into: buffer)
        guard let channels = buffer.floatChannelData,
              url.deletingPathExtension().lastPathComponent.hasSuffix("-\(captureLaneLetter(lane))") else {
            throw CaptureError("The WAV lost its lane label or sample data")
        }
        for channel in 0..<Int(width) {
            guard abs(channels[channel][0] - levels[lane]) < 0.00001 else {
                throw CaptureError("The WAV's lane sample does not match its source")
            }
        }
    }
    let aligned = try export(lanes)
    guard aligned.count == 4 else { throw CaptureError("Expected three aligned WAVs plus the stereo fold-down") }
    for lane in lanes.indices {
        try checkFile(aligned[lane], lane: lane, width: [1, 2, 6][lane])
    }
    let isolated = try export([lanes[2]])
    guard isolated.count == 2 else { throw CaptureError("Expected a multichannel and a stereo WAV for lane C") }
    try checkFile(isolated[0], lane: 2, width: 6)
    for (mode, count, width): (UInt32, Int, UInt32) in [(0, 7, 2), (1, 7, 1), (3, 2, 2)] {
        let urls = try export([lanes[2]], mode: mode)
        guard urls.count == count,
              try AVAudioFile(forReading: urls[0]).fileFormat.channelCount == width,
              urls.last?.lastPathComponent.hasSuffix("-Stereo.wav") == true else {
            throw CaptureError("The multichannel export mode wrote the wrong files")
        }
    }
    var isolatedRow = lanes[2]
    isolatedRow.view = 5
    let rowFiles = try export([isolatedRow], mode: 1)
    guard rowFiles.count == 1, rowFiles[0].lastPathComponent.contains("-Ch03_"),
          try AVAudioFile(forReading: rowFiles[0]).fileFormat.channelCount == 1 else {
        throw CaptureError("Isolated channel export failed")
    }
    isolatedRow.view = 67
    let pairFiles = try export([isolatedRow], mode: 1)
    guard pairFiles.count == 1, pairFiles[0].lastPathComponent.hasSuffix("-Stereo.wav") else {
        throw CaptureError("L/R row selection did not export the pair")
    }
    var low: Float = 0
    var high: Float = 0
    bt_frozen_waveform(frozen, 2, 5, selectionStart, selectionEnd, &low, &high, 1, nil)
    guard abs(low - levels[2]) < 0.00001, abs(high - levels[2]) < 0.00001 else {
        throw CaptureError("The isolated row waveform did not use its channel")
    }
    bt_frozen_waveform(frozen, 2, 67, selectionStart, selectionEnd, &low, &high, 1, nil)
    guard high > 0, low == high else { throw CaptureError("The L/R row is missing its fold-down") }
    guard let slot = bt_preview_slot_new_local() else { throw CaptureError("Could not create a preview slot") }
    defer { bt_preview_slot_free(slot) }
    let playing = lanes.withUnsafeBufferPointer {
        bt_frozen_play(frozen, slot, $0.baseAddress, $0.count, selectionStart, selectionEnd, 0, 0, fades)
    }
    var position: UInt64 = 0
    guard playing == 1, bt_preview_slot_position(slot, &position) == 1, position == selectionStart else {
        throw CaptureError("Preview did not start at the aligned selection")
    }

    let maximum = try TimelineHandle(history: 10, widths: Array(repeating: 1, count: 64))
    guard bt_tracks_channels(maximum.core, 63) == 1, bt_tracks_channels(maximum.core, 64) == 0,
          captureLaneLetter(63) == "BL" else { throw CaptureError("The 64-source boundary is wrong") }
    let invalid = [UInt32](repeating: 1, count: 65)
    guard invalid.withUnsafeBufferPointer({
        bt_tracks_new_lanes(SystemAudioTap.timelineRate, $0.baseAddress, $0.count, 10, 0)
    }) == nil else { throw CaptureError("The capture accepted a 65th source") }
}

private func verifyMacFrequencyCapture() throws {
    let timeline = try TimelineHandle(history: 10, widths: [2], retainSilence: true)
    let frames = 9_600
    let signal = (0..<frames).flatMap { frame -> [Float] in
        let angle = 2 * Double.pi * Double(frame) / 48_000
        return [Float(sin(angle * 60)) * 0.5, Float(sin(angle * 10_000)) * 0.5]
    }
    let accepted = signal.withUnsafeBufferPointer {
        bt_tracks_push_interleaved(timeline.core, 0, $0.baseAddress, UInt32(frames),
            1_000_000_000, SystemAudioTap.timelineRate)
    }
    guard accepted == UInt32(frames) else { throw CaptureError("Frequency test input was lost") }
    bt_tracks_pause(timeline.core)
    let first = bt_tracks_first_frame(timeline.core)
    let end = bt_tracks_end_frame(timeline.core)
    guard let frozen = bt_tracks_freeze(timeline.core, first, end), end - first == UInt64(frames) else {
        throw CaptureError("Could not freeze the two frequency tones")
    }
    defer { bt_frozen_free(frozen) }
    for (view, expected): (UInt32, Int) in [(1, 0), (2, 5)] {
        var bands = [Float](repeating: 0, count: 6)
        bands.withUnsafeMutableBufferPointer {
            bt_frozen_spectrum(frozen, 0, view, first, end, $0.baseAddress, 1, nil)
        }
        guard bands.indices.max(by: { bands[$0] < bands[$1] }) == expected, bands[expected] > 0 else {
            throw CaptureError("Rust frequency bands did not identify the correct tone")
        }
    }
}

func verifyMacCaptureRouting() throws {
    try verifyMacMultiSourceCapture()
    try verifyMacFrequencyCapture()
    try verifyMacWaveformGeometry()
    try verifyMacRetainSilenceOption()
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }

    func check(
        samples: [Float], labels: [AudioChannelLabel], routes: [CaptureRoute],
        captured: [[Float]], routed: [[Float]], mono: [[Float]]
    ) throws {
        let frameCount = 4_096
        let channels = UInt32(samples.count)
        let stream = AudioStreamBasicDescription(
            mSampleRate: SystemAudioTap.timelineRate, mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            mBytesPerPacket: channels * 4, mFramesPerPacket: 1, mBytesPerFrame: channels * 4,
            mChannelsPerFrame: channels, mBitsPerChannel: 32, mReserved: 0
        )
        let format = try InputFormat(stream, labels: labels)
        if channels == 2, format.supports(.center) { throw CaptureError("Stereo falsely reported native Center") }
        let timeline = try TimelineHandle(history: 10, a: UInt32(captured[0].count), b: UInt32(captured[1].count))
        let producers = routes.enumerated().map { index, route in
            LaneProducer(timeline: timeline, lane: UInt32(index), route: route, outputChannels: UInt32(captured[index].count))
        }
        var signal = (0..<frameCount).flatMap { _ in samples }
        signal.withUnsafeMutableBytes { data in
            var list = AudioBufferList(
                mNumberBuffers: 1,
                mBuffers: AudioBuffer(mNumberChannels: channels, mDataByteSize: UInt32(data.count), mData: data.baseAddress)
            )
            withUnsafeMutablePointer(to: &list) { list in
                let buffers = UnsafeMutableAudioBufferListPointer(list)
                for producer in producers {
                    producer.push(buffers, format: format, frameLimit: frameCount, time: 1_000_000_000)
                }
            }
        }
        bt_tracks_pause(timeline.core)
        guard let snapshotA = bt_tracks_snapshot(timeline.core, 0) else { throw CaptureError("Missing snapshot A") }
        defer { bt_snapshot_free(snapshotA) }
        guard let snapshotB = bt_tracks_snapshot(timeline.core, 1) else { throw CaptureError("Missing snapshot B") }
        defer { bt_snapshot_free(snapshotB) }

        let first = bt_snapshot_first_frame(snapshotA)
        guard first == bt_snapshot_first_frame(snapshotB),
              bt_snapshot_frame_count(snapshotA) == UInt64(frameCount),
              bt_snapshot_frame_count(snapshotB) == UInt64(frameCount) else {
            throw CaptureError("The synchronized tracks lost or misaligned frames")
        }
        let start = first + 37
        let end = first + 173

        func verifyFile(_ url: URL, expected: [Float]) throws {
            let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
            let count = AVAudioFrameCount(end - start)
            guard file.processingFormat.channelCount == UInt32(expected.count),
                  file.processingFormat.sampleRate == 48_000, file.length == Int64(count),
                  let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: count) else {
                throw CaptureError("WAV format or selected frame range is incorrect")
            }
            try file.read(into: buffer)
            guard buffer.frameLength == count, let data = buffer.floatChannelData else {
                throw CaptureError("WAV has unreadable sample data")
            }
            for (channel, value) in expected.enumerated() {
                for frame in 0..<Int(count) where abs(data[channel][frame] - value) > 0.00001 {
                    throw CaptureError("Unexpected sample in \(url.lastPathComponent), channel \(channel), frame \(frame)")
                }
            }
        }

        func verifyPair(_ expected: [[Float]], routed: Bool, forceMono: UInt32 = 0) throws {
            var a: UnsafeMutablePointer<CChar>?
            var b: UnsafeMutablePointer<CChar>?
            let written = directory.path.withCString { path in
                if routed {
                    return bt_snapshots_export_pair_routed_frames_with_profile(
                        snapshotA, snapshotB, start, end, path, 48_000, 32,
                        routes[0].wavRouteCode, routes[1].wavRouteCode, forceMono, &a, &b
                    )
                }
                return bt_snapshots_export_pair_frames_with_profile(
                    snapshotA, snapshotB, start, end, path, 48_000, 32, &a, &b
                )
            }
            guard written == 1, let pathA = a, let pathB = b else {
                throw CaptureError("Could not export the A/B take")
            }
            defer {
                bt_string_free(pathA)
                bt_string_free(pathB)
            }
            let urlA = URL(fileURLWithPath: String(cString: pathA))
            let urlB = URL(fileURLWithPath: String(cString: pathB))
            guard urlA.lastPathComponent.hasSuffix("-A.wav"), urlB.lastPathComponent.hasSuffix("-B.wav"),
                  urlA.lastPathComponent.dropLast(6) == urlB.lastPathComponent.dropLast(6) else {
                throw CaptureError("A/B files do not share a take name")
            }
            try verifyFile(urlA, expected: expected[0])
            try verifyFile(urlB, expected: expected[1])
        }

        try verifyPair(captured, routed: false)
        try verifyPair(routed, routed: true)
        try verifyPair(mono, routed: true, forceMono: 1)

        if routes[1] == .side {
            let layouts: [(UInt32, [Float])] = [(0, routed[1]), (1, mono[1])]
            for (forceMono, expected) in layouts {
                let exported = directory.path.withCString {
                    bt_snapshot_export_routed_frames_with_profile(
                        snapshotB, start, end, $0, 48_000, 32, routes[1].wavRouteCode, forceMono
                    )
                }
                guard let exported else { throw CaptureError("Could not export the single Side take") }
                defer { bt_string_free(exported) }
                try verifyFile(URL(fileURLWithPath: String(cString: exported)), expected: expected)
            }
        }
    }

    let stereo = [kAudioChannelLabel_Left, kAudioChannelLabel_Right]
    try check(
        samples: [0.25, -0.5], labels: stereo, routes: [.left, .right],
        captured: [[0.25], [-0.5]], routed: [[0.25, 0], [0, -0.5]], mono: [[0.25], [-0.5]]
    )
    try check(
        samples: [0.25, -0.5], labels: stereo, routes: [.mid, .side],
        captured: [[-0.125], [0.375]], routed: [[-0.125], [0.375, -0.375]], mono: [[-0.125], [0.375]]
    )
    try check(
        samples: [0.25, -0.5], labels: stereo, routes: [.merged, .left],
        captured: [[0.25, -0.5], [0.25]], routed: [[0.25, -0.5], [0.25, 0]], mono: [[-0.125], [0.25]]
    )
    let center: Float = 0.4
    let attenuation: Float = 0.70710677
    let scale: Float = 1 + attenuation
    let left = (0.25 + center * attenuation) / scale
    let right = (-0.5 + center * attenuation) / scale
    try check(
        samples: [0.25, -0.5, center], labels: stereo + [kAudioChannelLabel_Center], routes: [.center, .merged],
        captured: [[center], [left, right]], routed: [[center], [left, right]], mono: [[center], [(left + right) * 0.5]]
    )
}
#endif

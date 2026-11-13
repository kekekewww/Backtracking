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

enum CaptureSource: Hashable {
    case system
    case microphone(String)
    case application(String)
    case sender(String)

    init?(key: String) {
        if key == "system" { self = .system }
        else if key.hasPrefix("mic:") { self = .microphone(String(key.dropFirst(4))) }
        else if key.hasPrefix("app:"), key.count > 4 { self = .application(String(key.dropFirst(4))) }
        else if key.hasPrefix("sender:"), key.count > 7 { self = .sender(String(key.dropFirst(7))) }
        else { return nil }
    }

    var key: String {
        switch self {
        case .system: "system"
        case .microphone(let uid): "mic:\(uid)"
        case .application(let identifier): "app:\(identifier)"
        case .sender(let id): "sender:\(id)"
        }
    }

    var label: String {
        switch self {
        case .system: "系統播放"
        case .microphone(let uid):
            if uid.isEmpty { "預設麥克風" } else { "麥克風 · \(uid)" }
        case .application(let identifier): "應用程式 · \(identifier)"
        case .sender: "【Sender】"
        }
    }

    var isSender: Bool {
        if case .sender = self { return true }
        return false
    }

    var isMicrophone: Bool {
        if case .microphone = self { return true }
        return false
    }
}

enum CaptureRoute: UInt32, CaseIterable {
    case merged, left, right, center, mid, side

    // UserDefaults stores the Swift raw values. WAV export uses the core route codes.
    var wavRouteCode: UInt32 {
        switch self {
        case .left: 0
        case .right: 1
        case .center: 2
        case .mid: 3
        case .side: 4
        case .merged: 5
        }
    }

    var label: String {
        switch self {
        case .merged: "Merged"
        case .left: "Left"
        case .right: "Right"
        case .center: "Center"
        case .mid: "Mid"
        case .side: "Side"
        }
    }
}

struct CaptureLane: Equatable {
    var source: CaptureSource
    var route: CaptureRoute = .merged
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

@MainActor
func availableCaptureSources(excluding privateDevices: Set<AudioObjectID>) -> [(source: CaptureSource, title: String)] {
    var choices: [(CaptureSource, String)] = []
    if defaultDevice(kAudioHardwarePropertyDefaultOutputDevice) != nil {
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
    for device in inputDevices {
        guard let uid = audioString(device, kAudioDevicePropertyDeviceUID), !uid.isEmpty,
              let name = audioString(device, kAudioObjectPropertyName) else { continue }
        choices.append((.microphone(uid), "麥克風 · \(name)"))
    }

    // Workspace metadata names HAL playback clients and associates helper processes with their app.
    let apps = NSWorkspace.shared.runningApplications.filter {
        $0.activationPolicy != .prohibited && $0.bundleIdentifier != nil
    }
    let appsByPID = Dictionary(apps.map { ($0.processIdentifier, $0) }, uniquingKeysWith: { first, _ in first })
    let ownPID = ProcessInfo.processInfo.processIdentifier
    let ownBundle = Bundle.main.bundleIdentifier
    var namesByBundle: [String: String] = [:]
    for object in audioObjects(kAudioHardwarePropertyProcessObjectList) {
        guard hasPlaybackStream(object), let pid = processID(object), pid != ownPID else { continue }
        let bundle = audioString(object, kAudioProcessPropertyBundleID)
        var owner = appsByPID[pid]
        if owner?.activationPolicy != .regular {
            let helperOwner = apps.first { app in
                guard let identifier = app.bundleIdentifier else { return false }
                return bundle?.hasPrefix(identifier + ".helper") == true
            }
            let ancestor = helperOwner ?? processLineage(pid).dropFirst()
                .first(where: { appsByPID[$0]?.activationPolicy == .regular }).flatMap { appsByPID[$0] }
            owner = ancestor ?? owner ?? apps.first { $0.bundleIdentifier == bundle }
        }
        guard let identifier = owner?.bundleIdentifier ?? bundle, !identifier.isEmpty,
              identifier != ownBundle else { continue }
        namesByBundle[identifier] = owner?.localizedName ?? NSRunningApplication(processIdentifier: pid)?.localizedName
            ?? namesByBundle[identifier] ?? identifier
    }
    let sortedApps = namesByBundle.sorted {
        let order = $0.value.localizedStandardCompare($1.value)
        return order == .orderedSame ? $0.key < $1.key : order == .orderedAscending
    }
    choices += sortedApps.map { (.application($0.key), "應用程式 · \($0.value)") }
    choices += availableSenders().map { (.sender($0.id), "【Sender】\($0.name)") }
    return choices
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
    var address = property(kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyScopeInput)
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
    case microphone(AudioObjectID, Double)
    case application([AudioObjectID], AudioObjectID, Double)
    case sender(String, UInt32, UInt32)
}

@MainActor
private func identity(for source: CaptureSource) -> InputIdentity? {
    switch source {
    case .sender(let id):
        return senderDescriptors(audioOnly: true).first { $0.id == id }
            .map { .sender(id, $0.sampleRate, $0.channels) }
    case .system:
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

private struct InputFormat {
    let channels: Int
    let sampleRate: Double
    let planar: Bool
    let left: Int?
    let right: Int?
    let center: Int?
    let mix: [(channel: Int, left: Float, right: Float)]

    init(senderChannels: UInt32, sampleRate: UInt32) {
        channels = Int(senderChannels)
        self.sampleRate = Double(sampleRate)
        planar = false
        left = 0
        right = channels == 2 ? 1 : nil
        center = nil
        mix = []
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
            throw CaptureError("此來源的音訊格式不受支援")
        }
        channels = count
        sampleRate = stream.mSampleRate
        planar = separate
        let exact = labels.count == count ? labels : []
        left = exact.firstIndex(of: kAudioChannelLabel_Left) ?? 0
        right = exact.firstIndex(of: kAudioChannelLabel_Right) ?? (count >= 2 ? 1 : nil)
        center = exact.firstIndex(of: kAudioChannelLabel_Center)
        mix = count <= 2 ? [] : exact.enumerated().compactMap { index, label in
            switch label {
            case kAudioChannelLabel_LeftSurround, kAudioChannelLabel_LeftSurroundDirect,
                 kAudioChannelLabel_RearSurroundLeft: (index, 0.70710677, 0)
            case kAudioChannelLabel_RightSurround, kAudioChannelLabel_RightSurroundDirect,
                 kAudioChannelLabel_RearSurroundRight: (index, 0, 0.70710677)
            case kAudioChannelLabel_Center, kAudioChannelLabel_CenterSurround: (index, 0.70710677, 0.70710677)
            default: nil
            }
        }
    }

    var mergedChannels: UInt32 { channels == 1 ? 1 : 2 }

    func supports(_ route: CaptureRoute) -> Bool {
        switch route {
        case .merged: true
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

private final class TimelineHandle {
    let core: OpaquePointer

    init(history: UInt32, a: UInt32, b: UInt32) throws {
        guard let core = bt_tracks_new(SystemAudioTap.timelineRate, a, b, history) else {
            throw CaptureError("無法配置錄音緩衝區")
        }
        self.core = core
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
            // Preserve native samples in the ordinary mono and stereo cases.
            if route == .merged && format.channels == outputChannels {
                if !format.planar || outputChannels == 1 {
                    let start = buffers[0].mData!.assumingMemoryBound(to: Float.self).advanced(by: offset * outputChannels)
                    _ = bt_tracks_push_interleaved(timeline.core, lane, start, UInt32(count), packetTime, format.sampleRate)
                    offset += count
                    continue
                }
                let left = buffers[0].mData!.assumingMemoryBound(to: Float.self).advanced(by: offset)
                let right = buffers[1].mData!.assumingMemoryBound(to: Float.self).advanced(by: offset)
                _ = bt_tracks_push_planar_stereo(timeline.core, lane, left, right, UInt32(count), packetTime, format.sampleRate)
                offset += count
                continue
            }

            for frame in 0..<count {
                let input = offset + frame
                let left = format.left.map { format.sample(buffers, frame: input, channel: $0) } ?? 0
                let right = format.right.map { format.sample(buffers, frame: input, channel: $0) } ?? 0
                switch route {
                case .merged:
                    var mixedLeft = left
                    var mixedRight = right
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

private final class SenderInput: NativeInput {
    let format: InputFormat
    private let id: String
    private var producers: [LaneProducer] = []

    init(id: String, rate: UInt32, channels: UInt32) {
        self.id = id
        format = InputFormat(senderChannels: channels, sampleRate: rate)
    }

    func bind(_ producers: [LaneProducer]) throws { self.producers = producers }

    func start() throws {
        for producer in producers {
            guard producer.attachSender(id) else {
                try stop()
                throw CaptureError("Sender 無法連線")
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
            if labels.contains(kAudioChannelLabel_Center) {
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
        try checkAudio(AudioHardwareCreateProcessTap(tap, &tapID), "無法建立系統或應用程式音訊擷取")
        let aggregate: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Backtracking Capture",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceTapAutoStartKey: false,
            kAudioAggregateDeviceTapListKey: [
                [kAudioSubTapUIDKey: tap.uuid.uuidString, kAudioSubTapDriftCompensationKey: true]
            ]
        ]
        try checkAudio(AudioHardwareCreateAggregateDevice(aggregate as CFDictionary, &aggregateID), "無法建立音訊擷取裝置")
        var address = property(kAudioTapPropertyFormat)
        var stream = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout.size(ofValue: stream))
        try checkAudio(AudioObjectGetPropertyData(tapID, &address, 0, nil, &size, &stream), "無法讀取來源格式")
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
            "無法安裝音訊擷取回呼"
        )
    }

    func start() throws {
        guard let ioProc, !running else { return }
        try checkAudio(
            AudioDeviceStart(aggregateID, ioProc),
            "無法開始系統音訊擷取，請檢查螢幕與系統音訊錄製授權"
        )
        running = true
    }

    func stop() throws {
        guard let ioProc, running else { return }
        try checkAudio(AudioDeviceStop(aggregateID, ioProc), "無法暫停音訊擷取裝置")
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
            guard let unit = node.audioUnit else { throw CaptureError("無法取得麥克風輸入裝置") }
            var selected = device
            try checkAudio(
                AudioUnitSetProperty(
                    unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                    &selected, UInt32(MemoryLayout.size(ofValue: selected))
                ), "無法切換麥克風裝置"
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
    private var slots: [Slot]
    private var running = false
    private let captureQueue = DispatchQueue(label: "Backtracking.capture")
    private var pollInFlight = false
    var core: OpaquePointer { timeline.core }
    var laneCount: Int { lanes.count }
    var captureRoutes: [CaptureRoute] { lanes.map(\.route) }
    var privatePlaybackDevices: Set<AudioObjectID> {
        Set(slots.compactMap { ($0.input as? PlaybackTap)?.aggregateID })
    }

    init(history: UInt32, lanes: [CaptureLane]) throws {
        guard (1...2).contains(lanes.count) else { throw CaptureError("可擷取一或兩路來源") }
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
                    slots[index].identity, preferNative: slots[index].indexes.contains { lanes[$0].route == .center }
                )
            } catch {
                slots[index].error = error.localizedDescription
                slots[index].retryAt = ProcessInfo.processInfo.systemUptime + 5
            }
        }
        let widths = lanes.enumerated().map { index, lane -> UInt32 in
            guard lane.route == .merged else { return 1 }
            return slots.first { $0.indexes.contains(index) }?.input?.format.mergedChannels
                ?? (lane.source.isMicrophone ? 1 : 2)
        }
        self.widths = widths
        self.timeline = try TimelineHandle(history: history, a: widths[0], b: widths.count == 2 ? widths[1] : 0)
        self.slots = slots
        for index in self.slots.indices { bind(index) }
    }

    static var requiresMicrophonePrompt: Bool {
        AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined
    }

    static func requestMicrophonePermission(_ completion: @escaping @Sendable () -> Void) {
        AVCaptureDevice.requestAccess(for: .audio) { _ in completion() }
    }

    private static func makeInput(_ identity: InputIdentity?, preferNative: Bool) throws -> NativeInput? {
        switch identity {
        case .system(let device, _): try PlaybackTap(processes: nil, device: device, preferNativeLayout: preferNative)
        case .application(let processes, let device, _):
            try PlaybackTap(processes: processes, device: device, preferNativeLayout: preferNative)
        case .microphone(let device, _): try MicrophoneInput(device: device)
        case .sender(let id, let rate, let channels): SenderInput(id: id, rate: rate, channels: channels)
        case nil: nil
        }
    }

    private func bind(_ index: Int) {
        guard let input = slots[index].input else { return }
        let available = slots[index].indexes.filter { input.format.supports(lanes[$0].route) }
        let producers = available.map { lane in
            LaneProducer(timeline: timeline, lane: UInt32(lane), route: lanes[lane].route, outputChannels: widths[lane])
        }
        guard !producers.isEmpty else { return }
        do { try input.bind(producers) }
        catch {
            slots[index].error = error.localizedDescription
            slots[index].input = nil
            slots[index].retryAt = ProcessInfo.processInfo.systemUptime + 5
        }
    }

    func supports(_ route: CaptureRoute, lane: Int) -> Bool {
        guard let format = slots.first(where: { $0.indexes.contains(lane) })?.input?.format else {
            return route != .center
        }
        if route == .center, format.center == nil, !lanes[lane].source.isMicrophone, !lanes[lane].source.isSender,
           let device = defaultDevice(kAudioHardwarePropertyDefaultOutputDevice) {
            // A normal stereo tap can be recreated for an explicitly selected native Center.
            return deviceLabels(device, scope: kAudioDevicePropertyScopeOutput).contains(kAudioChannelLabel_Center)
        }
        return format.supports(route)
    }

    var sourceMessage: String? {
        for slot in slots {
            if let error = slot.error { return "\(slot.source.label) · \(error)" }
            if slot.input == nil {
                if slot.source.isMicrophone, AVCaptureDevice.authorizationStatus(for: .audio) != .authorized {
                    return "\(slot.source.label) · 需要麥克風授權"
                }
                return "\(slot.source.label) · 等待來源"
            }
            if let input = slot.input, let index = slot.indexes.first(where: { !input.format.supports(lanes[$0].route) }) {
                return "\(index == 0 ? "A" : "B") · 此來源未提供 \(lanes[index].route.label)"
            }
        }
        return nil
    }

    private func start(_ index: Int) {
        guard let input = slots[index].input,
              slots[index].indexes.contains(where: { input.format.supports(lanes[$0].route) }) else { return }
        do {
            try input.start()
            slots[index].error = nil
        } catch {
            slots[index].error = error.localizedDescription
            slots[index].retryAt = ProcessInfo.processInfo.systemUptime + 5
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
        captureQueue.async { [weak self] in
            bt_tracks_poll(timeline.core)
            DispatchQueue.main.async { [weak self] in self?.pollInFlight = false }
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

    func resume() throws {
        if running { try pause() }
        bt_tracks_resume(core)
        for index in slots.indices { start(index) }
        running = true
    }

    func pause() throws {
        bt_tracks_pause(core)
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
        if identities.count == 2, lanes[0].source != lanes[1].source {
            // A default device can converge with, or diverge from, an explicitly selected one.
            let shareProducer = identities[0] != nil && identities[0] == identities[1]
            if shareProducer != (slots.count == 1) { return true }
        }
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
                slots[index].retryAt = now + 5
                continue
            }
            slots[index].input = nil
            slots[index].identity = next
            slots[index].error = nil
            do {
                let input = try Self.makeInput(
                    next, preferNative: slots[index].indexes.contains { lanes[$0].route == .center }
                )
                if let input, slots[index].indexes.contains(where: {
                    lanes[$0].route == .merged && input.format.mergedChannels != widths[$0]
                }) {
                    return true
                }
                slots[index].input = input
                bind(index)
                start(index)
            } catch {
                slots[index].error = error.localizedDescription
                slots[index].retryAt = now + 5
            }
        }
        return false
    }
}

#if BACKTRACKING_CAPTURE_VERIFY
func verifyMacCaptureRouting() throws {
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
    let left = 0.25 + center * attenuation
    let right = -0.5 + center * attenuation
    try check(
        samples: [0.25, -0.5, center], labels: stereo + [kAudioChannelLabel_Center], routes: [.center, .merged],
        captured: [[center], [left, right]], routed: [[center], [left, right]], mono: [[center], [(left + right) * 0.5]]
    )
}
#endif

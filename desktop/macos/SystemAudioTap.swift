import AudioToolbox
import CoreAudio
import Foundation

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

final class SystemAudioTap {
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var ioProc: AudioDeviceIOProcID?
    private var running = false
    private(set) var core: OpaquePointer?
    private(set) var sampleRate = 0.0

    init(history: UInt32) throws {
        do {
            try prepare(history: history)
        } catch {
            close()
            throw error
        }
    }

    deinit {
        close()
    }

    private func prepare(history: UInt32) throws {
        let tap = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
        tap.name = "Backtracking System Audio"
        tap.uuid = UUID()
        tap.isPrivate = true
        tap.muteBehavior = .unmuted
        try checkAudio(
            AudioHardwareCreateProcessTap(tap, &tapID),
            "Could not create a system-audio tap"
        )

        let aggregate: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Backtracking Capture",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceIsPrivateKey: true,
            // The app starts the device explicitly and lets the Rust silence gate decide when to append.
            kAudioAggregateDeviceTapAutoStartKey: false,
            kAudioAggregateDeviceTapListKey: [
                [kAudioSubTapUIDKey: tap.uuid.uuidString, kAudioSubTapDriftCompensationKey: true]
            ]
        ]
        try checkAudio(
            AudioHardwareCreateAggregateDevice(aggregate as CFDictionary, &aggregateID),
            "Could not create the capture device"
        )

        var address = AudioObjectPropertyAddress(
            mSelector: kAudioTapPropertyFormat,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var format = AudioStreamBasicDescription()
        var formatSize = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        try checkAudio(
            AudioObjectGetPropertyData(tapID, &address, 0, nil, &formatSize, &format),
            "Could not read the tap's stream format"
        )

        let planar = (format.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0
        let channels = format.mChannelsPerFrame
        let bytesPerSample = UInt32(MemoryLayout<Float>.stride)
        guard format.mFormatID == kAudioFormatLinearPCM,
              (format.mFormatFlags & kAudioFormatFlagIsFloat) != 0,
              (format.mFormatFlags & kAudioFormatFlagIsBigEndian) == 0,
              format.mBitsPerChannel == 32,
              format.mBytesPerFrame == bytesPerSample * (planar ? 1 : channels),
              channels == 1 || channels == 2,
              let recorder = bt_new(format.mSampleRate, channels, history) else {
            throw CaptureError("The system audio tap did not supply supported 32-bit float PCM.")
        }
        core = recorder
        sampleRate = format.mSampleRate

        // The producer callback reads HAL buffers directly. The preallocated Rust queue copies
        // their contents before Core Audio reuses them. No window or file APIs are called here.
        try checkAudio(
            AudioDeviceCreateIOProcIDWithBlock(&ioProc, aggregateID, nil) { _, input, _, _, _ in
                let buffers = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: input))
                if channels == 2 && planar {
                    guard buffers.count >= 2,
                          buffers[0].mNumberChannels == 1, buffers[1].mNumberChannels == 1,
                          let left = buffers[0].mData, let right = buffers[1].mData else { return }
                    let frames = min(buffers[0].mDataByteSize, buffers[1].mDataByteSize) / bytesPerSample
                    _ = bt_push_planar_stereo(
                        recorder, left.assumingMemoryBound(to: Float.self),
                        right.assumingMemoryBound(to: Float.self), frames
                    )
                } else {
                    guard !buffers.isEmpty, buffers[0].mNumberChannels == channels,
                          let data = buffers[0].mData else { return }
                    let frames = buffers[0].mDataByteSize / (channels * bytesPerSample)
                    _ = bt_push_interleaved(recorder, data.assumingMemoryBound(to: Float.self), frames)
                }
            },
            "Could not install the capture callback"
        )
    }

    func resume() throws {
        guard let ioProc, let core else { return }
        // Retry a failed stop before reopening the Rust producer gate.
        if running { try pause() }
        bt_resume(core)
        let status = AudioDeviceStart(aggregateID, ioProc)
        if status != noErr {
            bt_pause(core)
            throw CaptureError("Could not start system-audio capture. Check System Settings → Privacy & Security → Screen & System Audio Recording", status: status)
        }
        running = true
    }

    func pause() throws {
        if let ioProc, running {
            let status = AudioDeviceStop(aggregateID, ioProc)
            if let core { bt_pause(core) }
            try checkAudio(status, "Could not pause the capture device")
            running = false
        } else if let core {
            bt_pause(core)
        }
    }

    private func close() {
        var callbackRemoved = true
        if let ioProc {
            if running {
                AudioDeviceStop(aggregateID, ioProc)
            }
            callbackRemoved = AudioDeviceDestroyIOProcID(aggregateID, ioProc) == noErr
            self.ioProc = nil
        }
        if aggregateID != kAudioObjectUnknown {
            let status = AudioHardwareDestroyAggregateDevice(aggregateID)
            callbackRemoved = callbackRemoved || status == noErr
            aggregateID = kAudioObjectUnknown
        }
        if tapID != kAudioObjectUnknown {
            AudioHardwareDestroyProcessTap(tapID)
            tapID = AudioObjectID(kAudioObjectUnknown)
        }
        if let core {
            // A surviving HAL callback still owns a raw pointer into this core.
            if callbackRemoved { bt_free(core) }
            self.core = nil
        }
        running = false
    }
}

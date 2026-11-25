#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  printf '%s\n' 'This capture verification uses the native macOS audio frameworks.' >&2
  exit 1
fi

source_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "$source_dir/../.." && pwd)"
cd "$repo_dir"
case "$(uname -m)" in
  arm64) rust_target=aarch64-apple-darwin; swift_target=arm64-apple-macos14.2 ;;
  x86_64) rust_target=x86_64-apple-darwin; swift_target=x86_64-apple-macos14.2 ;;
  *) printf '%s\n' 'Unsupported macOS architecture.' >&2; exit 1 ;;
esac

target_dir="${CARGO_TARGET_DIR:-$repo_dir/target}"
CARGO_TARGET_DIR="$target_dir" cargo build --locked --release --target "$rust_target" -p backtracking-addon-host
scratch="$(mktemp -d "${TMPDIR:-/tmp}/bt-verify.XXXXXX")"
trap 'rm -rf -- "$scratch"' EXIT
cat > "$scratch/Verify.swift" <<'SWIFT'
import Foundation

@main
enum Verify {
    @MainActor
    static func main() {
        // A child run of one check that may crash: a crash exits with status 11, without a report.
        if CommandLine.arguments.count == 3, CommandLine.arguments[1] == "--options-rebuild" {
            exitOnCrash()
            do {
                try verifyMacOptionsRebuild(step: CommandLine.arguments[2])
                exit(0)
            } catch {
                fputs("Verification failed: \(error)\n", stderr)
                exit(1)
            }
        }
        // A failed check exits with status 1 and its message, never a crash report.
        do {
            try checks()
        } catch let error as VerificationError {
            fputs("Verification failed: \(error.message)\n", stderr)
            exit(1)
        } catch {
            fputs("Verification failed: \(error)\n", stderr)
            exit(1)
        }
    }

    @MainActor
    static func checks() throws {
        try verifyMacCaptureRouting()
        try verifyMacOptions()
        try verifyMacTestCopyIsolation()
        try verifyMacFlagDragKeepsRows()
        try verifyMacChipToolTips()
        try verifyMacCrossSourceEdgeScroll()
        try verifyMacChannelLimit()
        try verifyMacChannelCountsAgree()
        try verifyMacChannelLimitCaptureRefused()
        try verifyMacLookGradient()
        try verifyMacControlPauseHolds()
        try verifyMacLookUpdatesKeepControls()
        try verifyMacSnapshotRebuilds()
        try verifyMacHiddenPagesCatchUp()
        try verifyMacFrequencyRows()
        try verifyMacAddonSettingControls()
        try verifyMacAddonCaptureStatus()
        try verifyMacSavedFileLanes()
        try verifyMacRowsAcrossGroups()
        try verifyMacPreviewPlan()
        try verifyMacEmptyGroupKeepsBed()
        try verifyMacTerminationSaves()
        try verifyFrequencyColors()
        try verifyRecorderHotkeys()
        try verifyRowStackAndActivity()
        try verifyThemeAndReadability()
        try verifyStageBAddonsAndLook()
        try AddonHostClient.verifyTicketCompletions()
        try AddonHostClient.verifyControlGate()
        try verifyMacControlBatchSnapshotSync()
        try verifyMacClipExportedAtExportRate()
        try verifyMacSelectWaitsForFreeze()
        try verifyMacControlSaveReply()
        try verifyMacSavedFilesReadHeadersOnly()
        print("macOS geometry, routes, quiet retention, aligned multi-source WAVs, frequency colors, Options, test-copy isolation, flag drags keeping rows, chip tooltips, cross-source edge scroll, 64-channel limit, look gradients, add-on pauses holding, look updates keeping controls, snapshot rebuilds, hidden pages catching up, frequency rows by identity, add-on setting controls, add-on capture status, saved file lanes, rows across groups, preview plan, exit saves, hotkeys, row layout, silent channel tracking, theme, add-on host, ticket completions, control grant checks, control batch window sync, clip_exported rates, select waiting for its freeze, control save reply, look readability and positions passed.")
    }

    static func verifyFrequencyColors() throws {
        let low: [Float] = [1, 0, 0, 0, 0, 0]
        let high: [Float] = [0, 0, 0, 0, 0, 1]
        let silence = [Float](repeating: 0, count: 6)
        let spectrum = FrequencyWaveform(range: 0..<300, rows: [low + silence + high])
        let gradient = FrequencyGradient.standard
        let edited = gradient.addingStop()
        guard gradient.colors(for: low + high) == [0xE8302C, 0x2C68EC],
              edited.stops.count == 3, FrequencyGradient(settings: edited.settings) == edited,
              spectrum.colors(for: 0, in: 0..<400, bins: 4, gradient: gradient, liveFrames: 100)
                  == [0xE8302C, 0x2C68EC, 0x2C68EC, 0x2C68EC] else {
            throw VerificationError("The gradient or live-edge colors changed")
        }
        // Package 9: dark stop fails readability (< 2.0 contrast vs plot bg) and falls back to default at that stop position.
        let dark = FrequencyGradient(stops: [
            FrequencyGradient.Stop(position: 0, rgb: 0x161A1D),
            FrequencyGradient.Stop(position: 1, rgb: 0x2C68EC)
        ])
        let readable = dark.readableForDrawing()
        guard dark.hasUnreadableStops, !readable.hasUnreadableStops,
              readable.stops[0].rgb == FrequencyGradient.standard.rgbAt(position: 0),
              readable.stops[1].rgb == 0x2C68EC else {
            throw VerificationError("Gradient readability fallback did not replace unreadable stop")
        }
    }

    static func verifyRecorderHotkeys() throws {
        var hotkeys = RecorderHotkeys()
        let key = RecorderAction.toggleCapture.presets[0]
        let second = RecorderShortcut(code: 0, modifiers: [.control, .shift], key: "A")
        let shiftBackspace = RecorderShortcut(code: 51, modifiers: [.shift], key: "Backspace")
        let bareBackspace = RecorderShortcut(code: 51, modifiers: [], key: "Backspace")
        guard hotkeys.windowAction(for: key) == .toggleCapture,
              hotkeys.set([key], for: .saveSnapshot) != nil,
              hotkeys.set([key], for: .globalSnapshot) != nil,
              // Package 11: bare cancel key (Backspace) conflicts with Shift+Backspace in window actions.
              hotkeys.set([shiftBackspace], for: .saveSnapshot) != nil,
              // Package 5 / HK-05: bare cancel binding matches with modifiers and on key repeat when canCancel is true.
              hotkeys.windowAction(for: shiftBackspace, isRepeat: true, canCancel: true) == .cancelSelection,
              hotkeys.windowAction(for: shiftBackspace, isRepeat: true, canCancel: false) == nil,
              hotkeys.windowAction(for: bareBackspace, isRepeat: false, canCancel: true) == .cancelSelection,
              // Package 11: Right Command tap rejected for non-separateLanes.
              hotkeys.set([.rightCommand], for: .saveSnapshot) == "A tap of Right Command works only for Edit separately.",
              hotkeys.set([], for: .toggleCapture) == nil,
              hotkeys.set([key, second], for: .saveSnapshot) == nil,
              hotkeys.set([key, key], for: .saveSnapshot) != nil,
              let data = hotkeys.data else { throw VerificationError("Shortcut validation failed") }
        let restored = RecorderHotkeys(data: data)
        guard restored.windowAction(for: second) == .saveSnapshot, restored[.toggleCapture].isEmpty,
              restored.windowAction(for: .rightCommand) == .separateLanes else {
            throw VerificationError("Shortcut settings did not survive a reload")
        }
        // Package 11 / HK-22: partial-bad JSON keeps valid actions and explicit [] while resetting only the bad action.
        let partialJSON = """
        {"toggleCapture":[],"saveSnapshot":[{"code":0,"flags":131072,"key":"A"}],"keepOnTop":[{"code":9999,"flags":0,"key":""}]}
        """.data(using: .utf8)!
        let partial = RecorderHotkeys(data: partialJSON)
        guard partial[.toggleCapture].isEmpty,
              partial[.saveSnapshot].count == 1,
              partial[.keepOnTop] == RecorderAction.keepOnTop.presets else {
            throw VerificationError("Partial-bad hotkeys JSON did not preserve valid overrides")
        }
    }

    static func verifyRowStackAndActivity() throws {
        // Package 2: ChannelActivityTracker hysteresis (immediate appear, 2 quiet polls to hide, FL/FR-only -> empty).
        var tracker = ChannelActivityTracker()
        guard tracker.shown == nil else { throw VerificationError("Initial mask should be nil") }
        let labels51 = ["FL", "FR", "FC", "LFE", "Ls", "Rs"]
        guard ChannelActivityTracker.shownChannels(width: 6, labels: labels51, activeMask: 0b000011).isEmpty else {
            throw VerificationError("FL/FR-only activity should hide channel rows below L/R pair")
        }
        tracker.recordAnswer(0b000111) // FL, FR, FC active
        guard tracker.shown == 0b000111,
              ChannelActivityTracker.shownChannels(width: 6, labels: labels51, activeMask: tracker.shown) == [0, 1, 2] else {
            throw VerificationError("Active FC channel did not appear immediately")
        }
        tracker.recordAnswer(0b000011) // 1st quiet poll for FC: stays visible
        guard ((tracker.shown ?? 0) & 0b000100) != 0 else {
            throw VerificationError("Channel hid after only 1 quiet poll")
        }
        tracker.recordAnswer(0b000011) // 2nd quiet poll for FC: hides
        guard ((tracker.shown ?? 0) & 0b000100) == 0 else {
            throw VerificationError("Channel did not hide after 2 quiet polls")
        }

        // Package 4: RowStackLayout 1-2 lanes & >2 lanes single-column scroll + shortLabel + labelGutterWidth.
        guard RowStackLayout.shortLabel("Object 12") == "12",
              RowStackLayout.shortLabel("TpFC") == "TpFC",
              RowStackLayout.shortLabel("Center") == "Cent",
              RowStackLayout.labelGutterWidth(laneCount: 1, hasStacked: false) == 0,
              RowStackLayout.labelGutterWidth(laneCount: 1, hasStacked: true) == 28,
              RowStackLayout.labelGutterWidth(laneCount: 3, hasStacked: false) == 16,
              RowStackLayout.labelGutterWidth(laneCount: 3, hasStacked: true) == 36 else {
            throw VerificationError("RowStackLayout label/gutter rules mismatch")
        }
        let twoBoxes = RowStackLayout.layOut(height: 100, lanes: [LaneLayoutInput(rows: 1, stacked: false), LaneLayoutInput(rows: 14, stacked: true)], first: [0, 0])
        guard twoBoxes.count == 2, twoBoxes[0].shown == 1, !twoBoxes[0].scrolls, twoBoxes[1].scrolls,
              twoBoxes[1].rowHeight >= RowStackLayout.minRowHeight else {
            throw VerificationError("Two-lane stacked layout failed")
        }
        let (colBoxes, col) = RowStackLayout.layOutColumn(height: 100, lanes: [
            LaneLayoutInput(rows: 1, stacked: false),
            LaneLayoutInput(rows: 1, stacked: false),
            LaneLayoutInput(rows: 14, stacked: true)
        ], first: 3)
        guard colBoxes.count == 3, col.scrolls, col.first == 3, col.rowHeight >= RowStackLayout.minRowHeight else {
            throw VerificationError("Multi-lane column scroll layout failed")
        }

        // Package 1: L/R pair atomic selection & rowsLabel formatting.
        let specs: [WaveformRowSpec] = [
            WaveformRowSpec(identity: .pair(lane: 0, side: 0), lane: 0, view: 67, shortLabel: "L", fullLabel: "L", laneChannelCount: 6, channelNumberOneBased: nil),
            WaveformRowSpec(identity: .pair(lane: 0, side: 1), lane: 0, view: 68, shortLabel: "R", fullLabel: "R", laneChannelCount: 6, channelNumberOneBased: nil),
            WaveformRowSpec(identity: .channel(lane: 0, index: 0), lane: 0, view: 3, shortLabel: "FL", fullLabel: "FL", laneChannelCount: 6, channelNumberOneBased: 1),
            WaveformRowSpec(identity: .channel(lane: 0, index: 1), lane: 0, view: 4, shortLabel: "FR", fullLabel: "FR", laneChannelCount: 6, channelNumberOneBased: 2),
        ]
        let span = RowStackLayout.rowsBetween(allRows: specs, anchor: .pair(lane: 0, side: 1), cursor: .channel(lane: 0, index: 0))
        guard span == [.pair(lane: 0, side: 0), .pair(lane: 0, side: 1), .channel(lane: 0, index: 0)],
              RowStackLayout.rowsLabel(span, dual: false, channelLabels: [labels51]) == "L/R–FL",
              RowStackLayout.rowsLabel(span, dual: true, channelLabels: [labels51]) == "A L/R–FL",
              RowStackLayout.rowsLabel([.pair(lane: 0, side: 0), .whole(lane: 1)], dual: true, channelLabels: [labels51]) == "A L/R–B" else {
            throw VerificationError("Cross-row span or label mismatch")
        }
    }

    static func verifyThemeAndReadability() throws {
        // Package 8: complement selection color vs 80-delta white fallback.
        guard WaveformTheme.defaultAccentRGB == 0x67E7B5,
              WaveformTheme.selectionRGB(for: 0x67E7B5) == (0x67E7B5 ^ 0xFFFFFF),
              WaveformTheme.selectionRGB(for: 0x808080) == 0xFFFFFF,
              WaveformTheme.isReadableGradientStop(0xE8302C),
              !WaveformTheme.isReadableGradientStop(0x161A1D) else {
            throw VerificationError("WaveformTheme color/contrast checks failed")
        }
    }

    static func verifyStageBAddonsAndLook() throws {
        // Pkg 20: ResolvedLook readability rejects illegible dark waveform on dark plot and clamps numbers.
        let badOffer = AddonAppearance(
            colors: ["waveform": "#161a1d", "flag": "#ffaa44"],
            numbers: ["border_width": 12.0, "corner_radius": 24.0, "waveform_stroke_scale": 0.1, "waveform_fill_opacity": 2.0],
            rows: ["LFE": "#ff4444", "BadDark": "#161a1d"],
            spectrum: [
                AddonSpectrumStop(color: "#161a1d", position: 0.0),
                AddonSpectrumStop(color: "#00ff88", position: 1.0),
            ]
        )
        let resolved = ResolvedLook.from(appearance: badOffer, accentRGB: WaveformTheme.defaultAccentRGB)
        guard resolved.waveform == nil,
              resolved.flag == RGBA8(0xff, 0xaa, 0x44),
              resolved.borderWidth == 8.0,
              resolved.cornerRadius == 16.0,
              resolved.waveformStrokeScale == 0.5,
              resolved.waveformFillOpacity == 1.0,
              resolved.rowColors["LFE"] == RGBA8(0xff, 0x44, 0x44),
              resolved.rowColors["BadDark"] == nil else {
            throw VerificationError("ResolvedLook readability or clamping failed")
        }
        guard let lookSpectrum = ResolvedLook.spectrum(from: badOffer) else {
            throw VerificationError("ResolvedLook.spectrum failed to parse valid stops")
        }
        let legibleSpec = resolved.legibleSpectrum(for: lookSpectrum)
        guard legibleSpec.replacedAny,
              legibleSpec.gradient.stops[0].rgb == FrequencyGradient.standard.rgbAt(position: 0),
              legibleSpec.gradient.stops[1].rgb == 0x00ff88 else {
            throw VerificationError("ResolvedLook.legibleSpectrum did not replace unreadable stop")
        }

        // Pkg 16: AddonJSONValue snapping & link guard.
        guard AddonJSONValue.snapNumber(0.30000000000000004, min: 0.0, max: 1.0, step: 0.1) == 0.3,
              AddonFormatting.linkIsOpenable("https://example.com/addon"),
              !AddonFormatting.linkIsOpenable("http://insecure.example.com"),
              !AddonFormatting.linkIsOpenable("https://bad.com/a b"),
              CaptureSource(key: "addon:test.addon:main") == .addon(addonID: "test.addon", sourceID: "main") else {
            throw VerificationError("Add-on helper validation failed")
        }

        // Pkg 19: LanePositionBuffer freeze & overflow tracking.
        let posBuf = LanePositionBuffer()
        posBuf.enqueue(timeNs: 1_000_000_000, channels: [
            AddonChannelPosition(channel: 3, x: 0.25, y: 0.5, z: 0.0, size: 0.1, gain: 1.0)
        ])
        posBuf.drain(evictedBefore: nil, retainedRange: 0..<1000) { _ in (1, 100) }
        guard let frozen = posBuf.freeze() else {
            throw VerificationError("LanePositionBuffer.freeze returned nil")
        }
        var outKey = BtPositionKey(frame: 0, x: 0, y: 0, z: 0, size: 0, gain: 0)
        let count = bt_frozen_positions_read(frozen.pointer, 3, 0, 1000, &outKey, 1)
        guard count == 1, outKey.frame == 100, abs(outKey.x - 0.25) < 1e-6 else {
            throw VerificationError("Frozen positions readback mismatch")
        }

        // Pkg 16: AddonHostClient isolated root startup & clean shutdown.
        let tmpRoot = FileManager.default.temporaryDirectory.appendingPathComponent("bt-verify-addon-host-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmpRoot, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmpRoot) }
        guard let client = AddonHostClient(dataRoot: tmpRoot.path, appVersion: "0.2.1", primary: true) else {
            throw VerificationError("AddonHostClient failed to start")
        }
        let deadline = ProcessInfo.processInfo.systemUptime + 3.0
        while !client.snapshot.ready && ProcessInfo.processInfo.systemUptime < deadline {
            _ = client.pollSnapshot()
            Thread.sleep(forTimeInterval: 0.02)
        }
        guard client.snapshot.ready, client.snapshot.error == nil, client.snapshot.addons.isEmpty else {
            client.shutdownAndFree()
            throw VerificationError("AddonHostClient did not reach ready state: \(client.snapshot.error ?? "not ready")")
        }
        client.shutdownAndFree()
    }

    /// Takes this process's crash exceptions itself and exits with status 11, so a crash in a
    /// child check leaves no crash report behind.
    static func exitOnCrash() {
        var port = mach_port_t()
        guard mach_port_allocate(mach_task_self_, MACH_PORT_RIGHT_RECEIVE, &port) == KERN_SUCCESS,
              mach_port_insert_right(mach_task_self_, port, port, mach_msg_type_name_t(MACH_MSG_TYPE_MAKE_SEND)) == KERN_SUCCESS else { return }
        let mask = exception_mask_t(EXC_MASK_BAD_ACCESS | EXC_MASK_BAD_INSTRUCTION | EXC_MASK_ARITHMETIC | EXC_MASK_BREAKPOINT | EXC_MASK_SOFTWARE)
        _ = task_set_exception_ports(mach_task_self_, mask, port, EXCEPTION_DEFAULT, THREAD_STATE_NONE)
        Thread.detachNewThread {
            let size = 4_096
            let buffer = UnsafeMutableRawPointer.allocate(byteCount: size, alignment: 8)
            let header = buffer.assumingMemoryBound(to: mach_msg_header_t.self)
            header.pointee.msgh_size = mach_msg_size_t(size)
            header.pointee.msgh_local_port = port
            _ = mach_msg(header, MACH_RCV_MSG, 0, mach_msg_size_t(size), port, MACH_MSG_TIMEOUT_NONE, mach_port_t(MACH_PORT_NULL))
            fputs("Verification crashed\n", stderr)
            _exit(11)
        }
    }

    struct VerificationError: Error {
        let message: String
        init(_ message: String) { self.message = message }
    }
}
SWIFT
xcrun --sdk macosx swiftc -swift-version 5 -parse-as-library -D BACKTRACKING_CAPTURE_VERIFY \
  -target "$swift_target" -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -import-objc-header "$source_dir/BacktrackingBridging.h" \
  "$source_dir/BacktrackingApp.swift" "$source_dir/SystemAudioTap.swift" "$source_dir/StartupUpdater.swift" \
  "$source_dir/FrequencyWaveform.swift" "$source_dir/RecorderHotkeys.swift" "$source_dir/TestCopy.swift" \
  "$source_dir/WaveformLayout.swift" "$source_dir/AddonHost.swift" \
  "$scratch/Verify.swift" "$target_dir/$rust_target/release/libbacktracking_addon_host.a" \
  -framework AppKit -framework AudioToolbox -framework CoreAudio -framework AVFoundation \
  -framework ServiceManagement -framework Carbon -framework CoreGraphics -o "$scratch/verify"
"$scratch/verify"
# N1: rebuilding an Options page under a field being typed into, then changing category, in a
# child that exits 11 on a crash instead of leaving a crash report.
for step in rebuild close-reopen end-editing typed-values deferred sources-name open-list key-loop switch-while-typing switch-unchanged not-a-number; do
  status=0
  "$scratch/verify" --options-rebuild "$step" || status=$?
  if [[ $status -ne 0 ]]; then
    printf 'Options rebuild check (%s) ended with status %s\n' "$step" "$status" >&2
    exit 1
  fi
done
printf 'Options rebuild checks passed.\n'
bash "$source_dir/tests/make-test-copy-tests.sh"

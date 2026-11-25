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
CARGO_TARGET_DIR="$target_dir" cargo build --locked --release --target "$rust_target" -p backtracking-core
scratch="$(mktemp -d)"
trap 'rm -rf -- "$scratch"' EXIT
cat > "$scratch/Verify.swift" <<'SWIFT'
@main
enum Verify {
    @MainActor
    static func main() throws {
        try verifyMacCaptureRouting()
        try verifyMacOptions()
        try verifyFrequencyColors()
        try verifyRecorderHotkeys()
        print("macOS geometry, routes, quiet retention, aligned multi-source WAVs, frequency colors, Options and hotkeys passed.")
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
    }

    static func verifyRecorderHotkeys() throws {
        var hotkeys = RecorderHotkeys()
        let key = RecorderAction.toggleCapture.presets[0]
        let second = RecorderShortcut(code: 0, modifiers: [.control, .shift], key: "A")
        guard hotkeys.windowAction(for: key) == .toggleCapture,
              hotkeys.set([key], for: .saveSnapshot) != nil,
              hotkeys.set([key], for: .globalSnapshot) != nil,
              hotkeys.set([], for: .toggleCapture) == nil,
              hotkeys.set([key, second], for: .saveSnapshot) == nil,
              hotkeys.set([key, key], for: .saveSnapshot) != nil,
              let data = hotkeys.data else { throw VerificationError("Shortcut validation failed") }
        let restored = RecorderHotkeys(data: data)
        guard restored.windowAction(for: second) == .saveSnapshot, restored[.toggleCapture].isEmpty,
              restored.windowAction(for: .rightCommand) == .separateLanes else {
            throw VerificationError("Shortcut settings did not survive a reload")
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
  -import-objc-header "$repo_dir/crates/backtracking-core/include/backtracking.h" \
  "$source_dir/BacktrackingApp.swift" "$source_dir/SystemAudioTap.swift" "$source_dir/StartupUpdater.swift" \
  "$source_dir/FrequencyWaveform.swift" "$source_dir/RecorderHotkeys.swift" "$source_dir/TestCopy.swift" \
  "$scratch/Verify.swift" "$target_dir/$rust_target/release/libbacktracking_core.a" \
  -framework AppKit -framework AudioToolbox -framework CoreAudio -framework AVFoundation \
  -framework ServiceManagement -framework Carbon -framework CoreGraphics -o "$scratch/verify"
"$scratch/verify"

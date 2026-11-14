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
    static func main() throws {
        try verifyMacCaptureRouting()
        print("macOS Swift waveform geometry, capture routing, quiet retention, alignment and A/B WAV export passed.")
    }
}
SWIFT
xcrun --sdk macosx swiftc -swift-version 5 -parse-as-library -D BACKTRACKING_CAPTURE_VERIFY \
  -target "$swift_target" -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -import-objc-header "$repo_dir/crates/backtracking-core/include/backtracking.h" \
  "$source_dir/SystemAudioTap.swift" "$scratch/Verify.swift" "$target_dir/$rust_target/release/libbacktracking_core.a" \
  -framework AppKit -framework AudioToolbox -framework CoreAudio -framework AVFoundation -o "$scratch/verify"
"$scratch/verify"

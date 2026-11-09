#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  printf '%s\n' 'Backtracking desktop requires macOS with Xcode Command Line Tools.' >&2
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
CARGO_TARGET_DIR="$target_dir" cargo build --release --target "$rust_target" -p backtracking-core
app_dir="$target_dir/bundled/Backtracking.app"
mkdir -p "$app_dir/Contents/MacOS"
cp "$source_dir/Info.plist" "$app_dir/Contents/Info.plist"
xcrun --sdk macosx swiftc -swift-version 5 -parse-as-library -O \
  -target "$swift_target" -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -import-objc-header "$repo_dir/crates/backtracking-core/include/backtracking.h" \
  "$source_dir/BacktrackingApp.swift" "$source_dir/SystemAudioTap.swift" \
  "$target_dir/$rust_target/release/libbacktracking_core.a" \
  -framework AppKit -framework CoreAudio -framework AudioToolbox \
  -o "$app_dir/Contents/MacOS/Backtracking"
codesign --force --sign - "$app_dir"
printf 'Built %s\n' "$app_dir"

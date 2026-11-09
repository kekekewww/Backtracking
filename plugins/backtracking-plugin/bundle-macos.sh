#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  printf '%s\n' 'Backtracking macOS plug-ins require macOS with Xcode Command Line Tools.' >&2
  exit 1
fi

source_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "$source_dir/../.." && pwd)"
cd "$repo_dir"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_dir/target}"
cargo nice-plug bundle backtracking-plugin --release --locked \
  --manifest-path "$source_dir/Cargo.toml" "$@"

for extension in vst3 clap; do
  bundle="$CARGO_TARGET_DIR/bundled/Backtracking.$extension"
  plist="$bundle/Contents/Info.plist"
  mkdir -p "$bundle/Contents/Resources"
  cp "$repo_dir/assets/Backtracking.icns" "$bundle/Contents/Resources/Backtracking.icns"
  plutil -replace CFBundleIconFile -string Backtracking.icns "$plist"
  codesign --force --sign - "$bundle"
done

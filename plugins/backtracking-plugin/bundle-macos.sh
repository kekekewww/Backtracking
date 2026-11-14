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

# Finder shows the bundle's own version, so it carries the plug-in's instead of the bundler's default.
version="$(tr -d '\r' < "$source_dir/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1)"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf '%s\n' 'Could not read the plug-in version from Cargo.toml.' >&2
  exit 1
fi

for extension in vst3 clap; do
  bundle="$CARGO_TARGET_DIR/bundled/Backtracking.$extension"
  plist="$bundle/Contents/Info.plist"
  mkdir -p "$bundle/Contents/Resources"
  cp "$repo_dir/assets/Backtracking.icns" "$bundle/Contents/Resources/Backtracking.icns"
  plutil -replace CFBundleIconFile -string Backtracking.icns "$plist"
  plutil -replace CFBundleShortVersionString -string "$version" "$plist"
  plutil -replace CFBundleVersion -string "$version" "$plist"
  codesign --force --sign - "$bundle"
done

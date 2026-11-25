#!/usr/bin/env bash
# Checks desktop/macos/make-test-copy.sh without starting anything it makes.
#   make-test-copy-tests.sh [source.app]
# Without a source app it builds a stub bundle from the real Info.plist. Copies are made under
# target/ in the repository and removed afterwards.
set -euo pipefail
source_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
repo_dir="$(cd -- "$source_dir/../.." && pwd)"
script="$source_dir/make-test-copy.sh"
work="$repo_dir/target/make-test-copy-tests-$$"
trap 'rm -rf -- "$work"' EXIT
mkdir -p "$work"
fail() { printf 'make-test-copy test failed: %s\n' "$*" >&2; exit 1; }
[[ -x "$script" ]] || fail "$script is missing"

source_app="${1:-}"
if [[ -z "$source_app" ]]; then
  source_app="$work/source/Backtracking.app"
  mkdir -p "$source_app/Contents/MacOS"
  cp "$source_dir/Info.plist" "$source_app/Contents/Info.plist"
  cp /usr/bin/true "$source_app/Contents/MacOS/Backtracking"
  cp /usr/bin/true "$source_app/Contents/MacOS/bt-addon-guard"
  codesign --force --sign - "$source_app/Contents/MacOS/bt-addon-guard" 2>/dev/null
  codesign --force --sign - "$source_app" 2>/dev/null
fi
original_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$source_app/Contents/Info.plist")"

output="$("$script" "$source_app" "$work/copies" "Verify copy/1" 2>/dev/null)"
copy="$work/copies/Backtracking-Verify-copy-1.app"
[[ "$output" == *"Test copy: $copy"* && "$output" == *"Bundle identifier: com.backtracking.audio.capture.testcopy.Verify-copy-1"* ]] \
  || fail "unexpected output: $output"
id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$copy/Contents/Info.plist")"
[[ "$id" == com.backtracking.audio.capture.testcopy.Verify-copy-1 ]] || fail "the copy is $id"
codesign --verify --strict --deep "$copy" || fail "the copy's signature does not verify"
codesign --verify --strict "$copy/Contents/MacOS/bt-addon-guard" || fail "bt-addon-guard is not signed"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$source_app/Contents/Info.plist")" == "$original_id" ]] \
  || fail "the source bundle changed"

real_home="$(eval printf '%s' "~$(id -un)")"
for inside in "Library/Preferences" "Library/Application Support/Backtracking" "Backtracking/output"; do
  target="$real_home/$inside/bt-test-copy-refused-$$"
  if "$script" "$source_app" "$target" refused >/dev/null 2>&1; then fail "a copy was made inside $inside"; fi
  [[ ! -e "$target" ]] || fail "$target was created"
done
if "$script" "$source_app" "$work/copies" "---" >/dev/null 2>&1; then fail "a name without letters was taken"; fi
for id in com.backtracking.audio.capture com.backtracking.audio.capture.testcopy. other.testcopy.x "com.backtracking.audio.capture.testcopy.a b"; do
  if "$script" --clean "$id" >/dev/null 2>&1; then fail "--clean accepted $id"; fi
done
"$script" --clean "com.backtracking.audio.capture.testcopy.never-made-$$" >/dev/null || fail "--clean of an unused test id failed"
printf 'make-test-copy: copy, identifier, signature, refused folders and clean checks passed.\n'

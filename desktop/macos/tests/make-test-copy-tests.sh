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

# A .. in the part that does not exist yet would land in ~/Backtracking after mkdir.
dotted="$real_home/bt-test-copy-missing-$$/../Backtracking"
if "$script" "$source_app" "$dotted" dotted >/dev/null 2>&1; then fail "a destination with .. was taken"; fi
[[ ! -e "$real_home/bt-test-copy-missing-$$" && ! -e "$real_home/Backtracking/Backtracking-dotted.app" ]] || fail "the .. destination made something"
if "$script" "$source_app" "$work/./dot" dot >/dev/null 2>&1; then fail "a destination with . was taken"; fi
[[ ! -e "$work/dot" ]] || fail "the . destination made a folder"

# The disk does not tell case apart: the real locations in another case are refused too,
# before anything is made there.
for inside in "library/preferences" "LIBRARY/Application Support/backtracking" "backtracking/output"; do
  target="$real_home/$inside/bt-test-copy-case-$$"
  if "$script" "$source_app" "$target" cased >/dev/null 2>&1; then fail "a copy was made inside $inside"; fi
  [[ ! -e "$target" ]] || fail "$target was created"
done
# An empty destination is refused before anything is made.
if (cd "$work" && "$script" "$source_app" "" empty >/dev/null 2>&1); then fail "an empty destination was taken"; fi
[[ ! -e "$work/Backtracking-empty.app" ]] || fail "the empty destination made a copy"

# The copy may not be, hold or lie in the source; the source stays whole.
same_dir="$work/same"
mkdir -p "$same_dir"
ditto "$source_app" "$same_dir/Backtracking-same.app"
source_sum="$(find "$same_dir/Backtracking-same.app" -type f -exec shasum {} + | sort | shasum)"
if "$script" "$same_dir/Backtracking-same.app" "$same_dir" same >/dev/null 2>&1; then fail "a copy over its own source was taken"; fi
if "$script" "$same_dir/Backtracking-same.app" "$same_dir/Backtracking-same.app/Contents" inside >/dev/null 2>&1; then fail "a copy inside its source was taken"; fi
# The same source named in another case.
if "$script" "$same_dir/backtracking-same.app" "$same_dir" same >/dev/null 2>&1; then fail "a copy over its own source in another case was taken"; fi
[[ "$(find "$same_dir/Backtracking-same.app" -type f -exec shasum {} + | sort | shasum)" == "$source_sum" ]] || fail "the source changed"
[[ ! -e "$same_dir/Backtracking-same.app/Contents/Backtracking-inside.app" ]] || fail "a copy was made inside the source"

# Folder names with letters outside ASCII, in another case or another Unicode form, are still the
# same folder: the copy over its own source is refused, the source stays whole.
nfc="$(printf 'Caf\xc3\xa9')"; nfd="$(printf 'Cafe\xcc\x81')"; upper="$(printf 'CAF\xc3\x89')"
mkdir -p "$work/$nfc"
ditto "$source_app" "$work/$nfc/Backtracking-same.app"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier com.backtracking.audio.capture.testcopy.same" "$work/$nfc/Backtracking-same.app/Contents/Info.plist"
unicode_sum="$(find "$work/$nfc/Backtracking-same.app" -type f -exec shasum {} + | sort | shasum)"
for spelled in "$nfd" "$upper"; do
  if "$script" "$work/$spelled/Backtracking-same.app" "$work/$nfc" same >/dev/null 2>&1; then fail "a copy over its source spelled $spelled was taken"; fi
  if "$script" "$work/$nfc/Backtracking-same.app" "$work/$spelled" same >/dev/null 2>&1; then fail "a copy over its source in $spelled was taken"; fi
done
[[ "$(find "$work/$nfc/Backtracking-same.app" -type f -exec shasum {} + | sort | shasum)" == "$unicode_sum" ]] || fail "the source spelled outside ASCII changed"

# A real folder that is a symlink, as when recordings are kept on another disk, is refused by
# where it leads, however the destination is spelled, and nothing is made there.
mkdir -p "$work/elsewhere"
ln -s "$work/elsewhere" "$work/linked"
for target in "$work/elsewhere" "$work/elsewhere/sub" "$work/linked" "$work/linked/sub"; do
  status=0
  BACKTRACKING_TEST_COPY_ALSO_REFUSE="$work/linked" "$script" "$source_app" "$target" linked >/dev/null 2>&1 || status=$?
  [[ $status -eq 2 ]] || fail "a copy into $target, inside a refused folder that is a symlink, ended with status $status"
done
[[ -z "$(ls -A "$work/elsewhere")" ]] || fail "something was made inside the refused folder that is a symlink"
# The extra refused folder refuses nothing outside it.
BACKTRACKING_TEST_COPY_ALSO_REFUSE="$work/linked" "$script" "$source_app" "$work/beside" beside >/dev/null 2>&1 \
  || fail "a copy beside the refused folder was not made"

# Something at the copy's path that is not a test copy is left alone.
mkdir -p "$work/taken"
ditto "$source_app" "$work/taken/Backtracking-taken.app"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier com.backtracking.audio.capture" "$work/taken/Backtracking-taken.app/Contents/Info.plist"
if "$script" "$source_app" "$work/taken" taken >/dev/null 2>&1; then fail "an app that is not a test copy was replaced"; fi
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$work/taken/Backtracking-taken.app/Contents/Info.plist")" == com.backtracking.audio.capture ]] \
  || fail "the app at the copy's path changed"
# A test copy made earlier is replaced.
"$script" "$source_app" "$work/copies" "Verify copy/1" >/dev/null 2>&1 || fail "an earlier test copy was not replaced"
for id in com.backtracking.audio.capture com.backtracking.audio.capture.testcopy. other.testcopy.x "com.backtracking.audio.capture.testcopy.a b"; do
  if "$script" --clean "$id" >/dev/null 2>&1; then fail "--clean accepted $id"; fi
done
"$script" --clean "com.backtracking.audio.capture.testcopy.never-made-$$" >/dev/null || fail "--clean of an unused test id failed"
[[ ! -e "$real_home/Library/Preferences/com.backtracking.audio.capture.testcopy.never-made-$$.plist" ]] || fail "--clean left a preferences file"
printf 'make-test-copy: copy, identifier, signature, refused folders and clean checks passed.\n'

#!/usr/bin/env bash
# Makes a test copy of Backtracking.app with its own bundle identifier, so AppKit's own state
# (open and save panels, the colour panel, window frames, saved application state) and macOS
# privacy grants stay apart from the installed app's `com.backtracking.audio.capture`.
#
#   make-test-copy.sh <source.app> <destination folder> <name>
#       Copies the bundle to <destination>/Backtracking-<name>.app, sets CFBundleIdentifier to
#       com.backtracking.audio.capture.testcopy.<name>, signs bt-addon-guard and then the app
#       ad hoc, and prints the copy's path and identifier.
#   make-test-copy.sh --clean <bundle identifier>
#       Deletes that test identifier's preferences domain and saved application state. Only
#       identifiers starting with com.backtracking.audio.capture.testcopy. are accepted.
#
# Start the copy with BACKTRACKING_TEST_INSTANCE and BACKTRACKING_TEST_DATA_DIR set; the app
# refuses to run as a test copy from a bundle that still has the installed app's identifier.
#
# BACKTRACKING_TEST_COPY_ALSO_REFUSE names one more folder no test copy may be made in, besides
# the user's real settings and recordings folders. It can only add a refusal.
set -euo pipefail

prefix=com.backtracking.audio.capture.testcopy.
fail() { printf '%s\n' "$*" >&2; exit 2; }

# The user's own home from the directory service, whatever HOME says.
real_home="$(eval printf '%s' "~$(id -un)")"
real_home="$(cd -- "$real_home" && pwd -P)"

if [[ "${1:-}" == --clean ]]; then
  [[ $# -eq 2 ]] || fail "Usage: make-test-copy.sh --clean <bundle identifier>"
  id="$2"
  [[ "$id" == "$prefix"* && "$id" =~ ^[A-Za-z0-9.-]+$ && "${#id}" -gt "${#prefix}" ]] \
    || fail "Only test copy identifiers starting with $prefix can be cleaned."
  # A domain that was never written is not an error. defaults leaves an empty file behind, so
  # the file goes too, once cfprefsd has let go of the domain.
  defaults delete "$id" >/dev/null 2>&1 || true
  rm -f -- "$real_home/Library/Preferences/$id.plist"
  rm -rf -- "$real_home/Library/Saved Application State/$id.savedState"
  printf 'Cleaned %s\n' "$id"
  exit 0
fi

[[ $# -eq 3 ]] || fail "Usage: make-test-copy.sh <source.app> <destination folder> <name>"
source_app="$1"
destination="$2"
raw_name="$3"
[[ -n "$destination" ]] || fail "Give a destination folder."

[[ -d "$source_app" && -f "$source_app/Contents/Info.plist" ]] || fail "$source_app is not an app bundle."
[[ -f "$source_app/Contents/MacOS/bt-addon-guard" ]] || fail "$source_app has no Contents/MacOS/bt-addon-guard."
source_app="$(cd -- "$source_app" && pwd -P)"

# Letters, digits and - only, as a bundle identifier part and a folder name take them.
name="$(printf '%s' "$raw_name" | LC_ALL=C tr -c 'A-Za-z0-9-' '-' | sed -e 's/--*/-/g' -e 's/^-//' -e 's/-$//' | cut -c1-40)"
name="${name%-}"
[[ -n "$name" ]] || fail "The name \"$raw_name\" leaves nothing for a bundle identifier."

# A . or .. in the part of the path that does not exist yet would only be resolved by mkdir,
# after the checks below, so such a destination is refused outright.
IFS=/ read -r -a parts <<< "$destination"
for part in "${parts[@]}"; do
  [[ "$part" == . || "$part" == .. ]] && fail "Give the destination without . or .. in it: $destination"
done

# Resolve the destination before anything is made there: the nearest folder that exists,
# with symlinks resolved, and the rest of the path as given.
existing="$destination"
rest=""
while [[ ! -d "$existing" ]]; do
  rest="/$(basename -- "$existing")$rest"
  existing="$(dirname -- "$existing")"
done
destination="$(cd -- "$existing" && pwd -P)$rest"
# The disk does not tell case apart and stores names in one Unicode form, while pwd -P keeps the
# spelling it was given; tr leaves non-ASCII letters alone and == does not normalize. So folders
# that exist are compared by device and inode; only the part of a path that does not exist yet is
# compared as text, in lower case. stat -L: a folder that is a symlink, such as recordings kept on
# another disk, is the folder it leads to.
lower() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }
# The device and inode of $1 and of every folder above it, from the nearest one that exists.
ancestor_ids() {
  local path="$1"
  while [[ ! -e "$path" ]]; do path="$(dirname -- "$path")"; done
  while :; do
    stat -L -f '%d:%i' -- "$path"
    [[ "$path" == / ]] && break
    path="$(dirname -- "$path")"
  done
}
# Whether $1 is $2 or lies inside it.
within() {
  if [[ -e "$2" ]]; then
    local id
    id="$(stat -L -f '%d:%i' -- "$2")"
    # Not a pipe: grep -q stops reading at a match, which under pipefail could fail the pipe.
    grep -qx -- "$id" < <(ancestor_ids "$1")
  else
    local a b
    a="$(lower "$1")"; b="$(lower "$2")"
    [[ "$a" == "$b" || "$a" == "$b/"* ]]
  fi
}
refuse_real() {
  local real
  for real in "$real_home/Library/Preferences" "$real_home/Library/Application Support/Backtracking" "$real_home/Backtracking" \
      ${BACKTRACKING_TEST_COPY_ALSO_REFUSE:+"$BACKTRACKING_TEST_COPY_ALSO_REFUSE"}; do
    if within "$1" "$real"; then
      fail "A test copy cannot be made inside $real, which holds the user's real settings or recordings."
    fi
  done
}
refuse_real "$destination"

id="$prefix$name"
copy="$destination/Backtracking-$name.app"
# The copy may not be the source, hold it, or lie inside it.
if within "$copy" "$source_app" || within "$source_app" "$copy"; then
  fail "The test copy $copy would overlap the source $source_app."
fi
# Only a test copy this script made is replaced.
if [[ -e "$copy" || -L "$copy" ]]; then
  existing_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$copy/Contents/Info.plist" 2>/dev/null || true)"
  [[ "$existing_id" == "$prefix"* ]] || fail "$copy exists and is not a test copy; it is left alone."
fi

mkdir -p -- "$destination"
# Check where the folder really is now that it exists.
real_destination="$(cd -- "$destination" && pwd -P)"
refuse_real "$real_destination"
[[ "$(lower "$real_destination")" == "$(lower "$destination")" ]] || fail "The destination resolved to $real_destination; give that path instead."
rm -rf -- "$copy"
ditto "$source_app" "$copy"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $id" "$copy/Contents/Info.plist"
codesign --force --sign - "$copy/Contents/MacOS/bt-addon-guard"
codesign --force --sign - "$copy"
printf 'Test copy: %s\nBundle identifier: %s\n' "$copy" "$id"

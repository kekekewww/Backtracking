# macOS test copy

A test copy runs beside the installed Backtracking without reading or writing the user's
settings, recordings or add-ons. It matches the Windows `BACKTRACKING_TEST_INSTANCE` copy,
with one extra step: on macOS it runs from its own copy of the app bundle.

## Making and starting one

```sh
bash desktop/macos/make-test-copy.sh target/bundled/Backtracking.app <folder> <name>
```

The script copies the bundle to `<folder>/Backtracking-<name>.app`. It sets `CFBundleIdentifier`
to `com.backtracking.audio.capture.testcopy.<name>`, signs `bt-addon-guard` and then the app ad
hoc, and prints the copy's path and identifier. `<name>` keeps letters, digits and `-`. The
script refuses a folder inside `~/Library/Preferences`, `~/Library/Application Support/Backtracking`
or `~/Backtracking`. One of these that is a symlink, such as recordings kept on another disk, is
refused by the folder it leads to. `BACKTRACKING_TEST_COPY_ALSO_REFUSE=<folder>` adds one more
folder to refuse.

Start the copy's executable with the usual test variables:

- `BACKTRACKING_TEST_INSTANCE=<name>`: 1 to 40 letters, digits, `-` or `_`.
- `BACKTRACKING_TEST_DATA_DIR=<folder>` or `LOCALAPPDATA=<folder>`: an existing folder that is not
  one of the user's real data folders, and not inside one.
- The optional `BACKTRACKING_TEST_*` variables for window size, position, language, synthetic
  System format, screenshot and view script.

A test copy started from a bundle whose identifier is still `com.backtracking.audio.capture` stops
with exit code 2 and says to make a test copy. A copy started without `BACKTRACKING_TEST_INSTANCE`
(from the Finder, say) also stops with exit code 2, so it never runs on the user's own data.

The copy is signed ad hoc without the hardened runtime or the app's entitlements. That is enough
to run it on this Mac, not to distribute it.

To remove what a test copy left in the system:

```sh
bash desktop/macos/make-test-copy.sh --clean com.backtracking.audio.capture.testcopy.<name>
```

This runs `defaults delete` for that identifier and removes its
`~/Library/Saved Application State/<id>.savedState`. Only identifiers starting with
`com.backtracking.audio.capture.testcopy.` are accepted.

## What stays apart

- **The app's own settings** go to `<data folder>/Backtracking/preferences.plist`, never
  `UserDefaults.standard`.
- **AppKit's own state** goes to the test identifier's domain, not the installed app's. This
  covers the open and save panels, the colour panel, window frames and saved application state.
- **Add-ons and their data** live under `<data folder>/Backtracking`. **Exports** go to
  `<data folder>/Backtracking/output` and `saved`.
- **Not done at all in a test copy:** global shortcut, Input Monitoring request, status item,
  login item, update check, preview playback.
- **New capture windows** open from the same copy, so they carry the same identifier. They also
  get the test variables. A window that was opened by a test copy but lacks them refuses to start.

## Differences from Windows

- **Bundle copy:** Windows starts the installed executable with `BACKTRACKING_TEST_INSTANCE`.
  macOS needs a bundle copy with its own identifier, because AppKit keeps panel, window and
  restore state under the bundle identifier, and `CFFIXED_USER_HOME` does not redirect
  preferences there.
- **Privacy grants:** the copy's identifier is new to macOS privacy controls, so microphone and
  system audio access are asked for and granted for it separately from the installed app.
- **Data folder check:** the test data folder may not lie inside the user's real data folders,
  which is stricter than Windows. Windows only refuses the same folder.
- **Sender folder:** on Windows the Sender folder follows `LOCALAPPDATA`, so the test data folder
  keeps it apart. On macOS it follows `HOME` (`$HOME/Library/Application Support/Backtracking/senders-v1`),
  so a test copy that should not see the user's Senders also needs `HOME` set for its process.

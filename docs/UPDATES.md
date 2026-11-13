# Backtracking desktop updates

## 使用方式

桌面版可在 **Options → 更新** 查看目前版本、開啟自動更新、手動檢查，以及重看上次更新內容。第一次遇到已簽署的新版會顯示 GitHub Release 說明。**下個版本再提示**只跳過當前版本，**不再顯示更新提示**則改為手動檢查。啟用後，手動開啟和登入啟動都會先處理更新，再開始錄音。離線、檢查逾時或更新失敗時會啟動目前版本。

第一次使用需手動安裝已內建發行公鑰的版本。後續可由更新器從 GitHub Release 下載並驗證對應平台的更新檔。

## Update behavior

The desktop updater uses the latest published stable GitHub Release. Windows and macOS check before starting capture on a manual start or at sign-in. The first signed update prompts for consent. Users can skip that version, switch to manual checks, or enable automatic installation. Options contains the installed version, update mode, a manual check, an explicit restart action, and release notes. The updater waits for the running application to exit, keeps a backup, and restores it if the new application cannot report a healthy launch.

## Prepare the release key

Generate a dedicated Ed25519 key and store the private key outside the repository and release assets. Distribute only its public key in the compiled application.

```sh
openssl genpkey -algorithm ED25519 -out /private/path/backtracking-update-key.pem
python3 tools/sign_backtracking_update.py public-key /private/path/backtracking-update-key.pem
```

Backtracking's release public key is stored in [`desktop/update-public-key.hex`](../desktop/update-public-key.hex). CI in the official repository supplies it to both desktop release builds. For a local release build, set `BACKTRACKING_UPDATE_PUBLIC_KEY` to the contents of that file before running the Windows or macOS build command. Keep the matching private key in the maintainer's signing environment. Windows builds without an embedded key show the GitHub download link in Options. On macOS, the manual check reports that the key is unconfigured.

正式版公鑰已收錄在 [`desktop/update-public-key.hex`](../desktop/update-public-key.hex)，官方 CI 會將它提供給 Windows 與 macOS 桌面版建置。本機建置正式版前，請將檔案中的公鑰設為 `BACKTRACKING_UPDATE_PUBLIC_KEY`。每次發布仍須使用對應的私鑰簽署各平台更新檔。

## Build and sign

Set the application version before building. For Windows it comes from `desktop/windows/Cargo.toml`. For macOS it comes from the app bundle's `CFBundleShortVersionString`. Publish under a stable GitHub tag of the form `vX.Y.Z` with the same version in the signed manifests.

Use these exact artifact names. Each platform adds a matching `.json` and `.json.sig` beside its payload.

| Platform | Signed payload |
| --- | --- |
| Windows x64 | `Backtracking-update-windows-x64.exe` |
| macOS Apple silicon | `Backtracking-update-macos-arm64.zip` |
| macOS Intel | `Backtracking-update-macos-x64.zip` |

For example, sign a Windows release payload using its compiled version.

```sh
python3 tools/sign_backtracking_update.py sign windows-x64 0.1.5 \
  Backtracking-update-windows-x64.exe /private/path/backtracking-update-key.pem
```

On macOS, build on the intended architecture. Set `BACKTRACKING_MAC_CODESIGN_IDENTITY` to the Developer ID identity when preparing a public build. Archive the signed, notarized app as the top-level `Backtracking.app` and sign the archive using the same release key.

```sh
BACKTRACKING_UPDATE_PUBLIC_KEY="$UPDATE_PUBLIC_KEY" bash desktop/macos/build.sh
ditto -c -k --keepParent target/bundled/Backtracking.app Backtracking-update-macos-arm64.zip
python3 tools/sign_backtracking_update.py sign macos-arm64 0.1.5 \
  Backtracking-update-macos-arm64.zip /private/path/backtracking-update-key.pem
```

Upload the signed payload, manifest, and detached signature together before publishing the release. The manifest contains the platform, version, file name, byte count, and SHA-256. The detached signature is the 64-byte Ed25519 signature of the exact manifest bytes. The user sees the GitHub Release description when offered an update or after a successful install.

The standard release ZIP may continue to contain the desktop application, VST3 and CLAP. Distribute the first updater-enabled build through that manual download. Subsequent desktop updates use the per-platform payload. Plugin files are updated separately while the DAW is closed.

# Windows Setup

Build the desktop application, VST3 and CLAP, then create an interactive Setup:

```powershell
powershell -NoProfile -File tools/build_windows_setup.ps1
```

The output is `target/setup/Backtracking-<version>-Windows-x64-Setup.exe`, with a
SHA-256 file beside it. Open the EXE to test the installation wizard. The desktop
is required; VST3, CLAP and a desktop shortcut can be selected in Components.
VST3 and CLAP are independent choices. Each selected format shows its own scan
folder page after the desktop folder page; unselected formats skip that page.

Setup installs for the current Windows user without requesting administrator
access. Its default locations are:

| Component | Location |
| --- | --- |
| Desktop | `%LOCALAPPDATA%\Programs\Backtracking\` |
| VST3 | `%LOCALAPPDATA%\Programs\Common\VST3\Backtracking.vst3\` |
| CLAP | `%LOCALAPPDATA%\Programs\Common\CLAP\Backtracking\` |

On a plugin folder page, choose the **scan folder**, for example `D:\Audio\VST3`
or `D:\Audio\CLAP`. Setup appends `Backtracking.vst3` for VST3 and `Backtracking`
for CLAP. Keeping each plugin's runtime DLLs inside its own subfolder avoids
overwriting DLLs in a shared scan folder. Both formats can use the same scan
folder, or different folders. If a DAW does not scan the chosen folder
automatically, add it to that format's plugin scan paths.

Setup remembers installed plugin folders and selects those components on the
next run. Changing one format's folder moves its installer-owned files to the
new location; it leaves unrelated files and the scan folder itself in place.
Deselecting an already installed format leaves that installation available;
it does not uninstall the format. The registered uninstaller removes both
formats previously installed by Setup, including a pending interrupted move.
Close Backtracking and the DAW before updating installed binaries.

Custom folders must be absolute, writable by the current Windows account, and
below a drive or network share root. Setup rejects system folders, ambiguous
names, links/junctions, and overlapping desktop/plugin destinations. Long paths
are limited to leave room for the VST3 bundle files under Windows' normal path
limit. System-wide folders requiring administrator access are unavailable in
this per-user Setup. A folder containing an unowned Backtracking plugin is not
overwritten; remove that older manual installation or choose another scan root.

The Start Menu and Windows Installed Apps provide an uninstaller. Uninstall
removes installer-owned application/plugin files and shortcuts; it preserves
recordings, application settings and other files added by the user. A login
entry is removed only when it points to the application being uninstalled.
Unknown files inside a plugin bundle or application folder are preserved. A
custom plugin folder must have Setup's `.backtracking-setup.ini` ownership marker
before its payload can be removed. Existing installations made by the previous
Setup at its exact default locations are migrated without requiring that marker.

For unattended installation, `/S` uses the same validation as the wizard:

```powershell
.\Backtracking-<version>-Windows-x64-Setup.exe /S /VST3=1 /CLAP=1 /VST3DIR="D:\Audio\VST3" /CLAPDIR="D:\Audio\CLAP" /D=D:\Apps\Backtracking
```

`/VST3=0` or `/CLAP=0` skips installing that format. Folder arguments alone do
not select a component. On a new installation both plugins are unselected by
default; on upgrades their previous selections and folders are used. `/D=` is
the desktop folder and must be the final argument, without quotes even when its
path contains spaces (NSIS convention). Validation failure returns exit code 87;
canceling a locked-file retry returns 2. If per-user installation registration
cannot be written, Setup stops before copying payload and returns 5. Silent
Setup never starts Backtracking.

For a desktop-only Setup, pass `-DesktopOnly`. To package already-built Release
binaries, pass `-SkipBuild`. `-NsisPath` can specify an existing `makensis.exe`.
Without one, the build downloads the pinned NSIS 3.13 portable compiler into
`target/installer-tools/` and checks the official ZIP's SHA-256. A mirror package
can supply that same ZIP; no mirror installation scripts are run.

Visual Studio C++ Build Tools must include its x64 VC runtime redistributable
directory. The build copies the unmodified `vcruntime140.dll` and
`vcruntime140_1.dll` beside each installed binary. Use `-RuntimeDirectory` to
provide that directory explicitly. No system-wide runtime installer is run.

Local builds are unsigned. This script does not publish or install the result.

Run `powershell -NoProfile -File tools/test_windows_setup.ps1` for the isolated
NSIS integration tests. They compile tiny text fixtures and exercise silent
install/upgrade/uninstall under `target/setup-tests/<random-id>/`, with separate
test registry keys and shortcuts. They do not start or install the real app.
`-Case Compile` checks full and desktop-only builds without running installers;
`-Case Smoke` also checks custom folders and file preservation. Test artifacts
are retained for inspection.

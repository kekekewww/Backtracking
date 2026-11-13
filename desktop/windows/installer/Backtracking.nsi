# -*- coding: utf-8 -*-
; Build with NSIS 3 and the defines supplied by tools/build_windows_setup.ps1.
; File operations deliberately name installer-owned files. Never recursively
; remove an application/plugin folder: recordings and user-added files survive.

Unicode true

!ifndef APP_VERSION
  !error "APP_VERSION is required (a three-part version, for example 0.1.5)."
!endif
!ifndef PAYLOAD_DIR
  !error "PAYLOAD_DIR is required (the absolute staged payload directory)."
!endif
!ifndef OUTPUT_FILE
  !error "OUTPUT_FILE is required (the absolute Setup executable path)."
!endif
!ifndef ASSETS_DIR
  !error "ASSETS_DIR is required (the absolute repository assets directory)."
!endif

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "Sections.nsh"
!include "WinVer.nsh"
!include "x64.nsh"
!include "FileFunc.nsh"
!include "InstallerPaths.nsh"

!define APP_NAME "Backtracking"
; Overrides let the integration tests use disposable registry/shortcut names.
!ifndef INSTALLER_KEY
  !define INSTALLER_KEY "Software\Backtracking\Installer"
!endif
!ifndef UNINSTALL_KEY
  !define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Backtracking"
!endif
!ifndef DESKTOP_MUTEX
  !define DESKTOP_MUTEX "Local\Backtracking.Desktop.7c1311ad-1d75-4b4b-846a-ff7d0ef2dc79"
!endif
!ifndef DEFAULT_INSTALL_DIR
  !define DEFAULT_INSTALL_DIR "$LOCALAPPDATA\Programs\Backtracking"
!endif
!ifndef DEFAULT_VST3_ROOT
  !define DEFAULT_VST3_ROOT "$LOCALAPPDATA\Programs\Common\VST3"
!endif
!ifndef DEFAULT_CLAP_ROOT
  !define DEFAULT_CLAP_ROOT "$LOCALAPPDATA\Programs\Common\CLAP"
!endif
!ifndef STARTMENU_DIR
  !define STARTMENU_DIR "$SMPROGRAMS\Backtracking"
!endif
!ifndef DESKTOP_SHORTCUT
  !define DESKTOP_SHORTCUT "$DESKTOP\Backtracking.lnk"
!endif
!ifndef RUN_KEY
  !define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!endif
!ifndef RUN_VALUE
  !define RUN_VALUE "Backtracking"
!endif
!define OWNERSHIP_FILE ".backtracking-setup.ini"
!ifdef INCLUDE_PLUGINS
  Var VST3Root
  Var CLAPRoot
!endif

Name "${APP_NAME} ${APP_VERSION}"
Caption "${APP_NAME} Setup"
OutFile "${OUTPUT_FILE}"
InstallDir "${DEFAULT_INSTALL_DIR}"
InstallDirRegKey HKCU "${INSTALLER_KEY}" "InstallDir"
RequestExecutionLevel user
ManifestSupportedOS Win10
ManifestDPIAware true
SetCompressor /SOLID lzma
SetCompressorDictSize 32
SetOverwrite on
AllowSkipFiles off
ShowInstDetails show
ShowUninstDetails show
BrandingText "${APP_NAME}"

VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "FileDescription" "${APP_NAME} per-user Setup (64-bit)"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Backtracking contributors"

!define MUI_ICON "${ASSETS_DIR}\backtracking.ico"
!define MUI_UNICON "${ASSETS_DIR}\backtracking.ico"
!define MUI_ABORTWARNING
!define MUI_UNABORTWARNING

!define MUI_WELCOMEPAGE_TEXT "This wizard installs Backtracking for your Windows account.$\r$\n$\r$\nClose Backtracking, including its system-tray icon, before continuing. Close your DAW before installing or updating plugins.$\r$\n$\r$\nRecordings and settings are preserved when you uninstall."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${PAYLOAD_DIR}\LICENSE.txt"
!ifdef INCLUDE_PLUGINS
  !define MUI_COMPONENTSPAGE_TEXT_TOP "Choose components for your account. Each selected plugin has its own folder page."
!else
  !define MUI_COMPONENTSPAGE_TEXT_TOP "Choose components to install for your Windows account."
!endif
!insertmacro MUI_PAGE_COMPONENTS
!define MUI_DIRECTORYPAGE_TEXT_TOP "Choose the Backtracking application folder. This is a per-user installation and does not request administrator access."
!insertmacro MUI_PAGE_DIRECTORY
!ifdef INCLUDE_PLUGINS
  !define MUI_PAGE_CUSTOMFUNCTION_PRE VST3PagePre
  !define MUI_PAGE_CUSTOMFUNCTION_LEAVE VST3PageLeave
  !define MUI_PAGE_HEADER_TEXT "$(VST3FolderTitle)"
  !define MUI_PAGE_HEADER_SUBTEXT "$(VST3FolderSubtitle)"
  !define MUI_DIRECTORYPAGE_TEXT_TOP "$(VST3FolderText)"
  !define MUI_DIRECTORYPAGE_VARIABLE $VST3Root
  !insertmacro MUI_PAGE_DIRECTORY
  !define MUI_PAGE_CUSTOMFUNCTION_PRE CLAPPagePre
  !define MUI_PAGE_CUSTOMFUNCTION_LEAVE CLAPPageLeave
  !define MUI_PAGE_HEADER_TEXT "$(CLAPFolderTitle)"
  !define MUI_PAGE_HEADER_SUBTEXT "$(CLAPFolderSubtitle)"
  !define MUI_DIRECTORYPAGE_TEXT_TOP "$(CLAPFolderText)"
  !define MUI_DIRECTORYPAGE_VARIABLE $CLAPRoot
  !insertmacro MUI_PAGE_DIRECTORY
!endif
!define MUI_PAGE_CUSTOMFUNCTION_PRE EnsureDesktopClosed
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\Backtracking.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Backtracking"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchBacktracking
!insertmacro MUI_PAGE_FINISH

!define MUI_UNWELCOMEPAGE_TEXT "This wizard removes Backtracking and the plugins installed by this Setup for your account.$\r$\n$\r$\nClose Backtracking and your DAW before continuing.$\r$\n$\r$\nYour recordings, settings, and other user-created files are preserved."
!insertmacro MUI_UNPAGE_WELCOME
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

; NSIS automatically chooses a supported language from the user's locale.
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "TradChinese"

Var RegisteredInstall
Var PreviousInstallDir
Var VST3Dir
Var CLAPDir
Var PluginKind
Var PluginDir
Var PluginOwned
Var PluginValue
Var PreviousPluginValue
!ifdef INCLUDE_PLUGINS
  Var OldPluginDir
  Var CommandLine
!endif
Var OtherPath
Var PathsOverlap

LangString InvalidFolder ${LANG_ENGLISH} "Choose a writable absolute folder below a drive or share root. Windows/system folders, ambiguous paths, links/junctions and paths that overlap application/plugin folders are not supported. Setup does not request administrator access."
LangString InvalidFolder ${LANG_TRADCHINESE} "請選擇磁碟或共用根目錄下可寫入的完整資料夾路徑。不支援 Windows／系統資料夾、含糊路徑、連結／接合點，或與程式／插件互相重疊的資料夾。安裝程式不會要求系統管理員權限。"
LangString UnownedPlugin ${LANG_ENGLISH} "This plugin folder contains a Backtracking binary that this Setup does not own. Choose another scan folder or remove the old plugin manually."
LangString UnownedPlugin ${LANG_TRADCHINESE} "此插件資料夾含有不屬於本安裝程式的 Backtracking 檔案。請選擇其他掃描資料夾，或手動移除舊插件。"
!ifdef INCLUDE_PLUGINS
LangString VST3FolderTitle ${LANG_ENGLISH} "VST3 scan folder"
LangString VST3FolderTitle ${LANG_TRADCHINESE} "VST3 掃描資料夾"
LangString VST3FolderSubtitle ${LANG_ENGLISH} "Choose the folder your DAW scans for VST3 plugins."
LangString VST3FolderSubtitle ${LANG_TRADCHINESE} "選擇 DAW 用來掃描 VST3 插件的資料夾。"
LangString VST3FolderText ${LANG_ENGLISH} "Setup installs Backtracking.vst3 inside this folder. Add the selected folder to your DAW's VST3 scan paths if needed. Choose a folder writable by your Windows account."
LangString VST3FolderText ${LANG_TRADCHINESE} "安裝程式會在此資料夾內建立 Backtracking.vst3。必要時，請將此處選擇的資料夾加入 DAW 的 VST3 掃描路徑。請選擇目前 Windows 帳戶可寫入的資料夾。"
LangString CLAPFolderTitle ${LANG_ENGLISH} "CLAP scan folder"
LangString CLAPFolderTitle ${LANG_TRADCHINESE} "CLAP 掃描資料夾"
LangString CLAPFolderSubtitle ${LANG_ENGLISH} "Choose the folder your DAW scans for CLAP plugins."
LangString CLAPFolderSubtitle ${LANG_TRADCHINESE} "選擇 DAW 用來掃描 CLAP 插件的資料夾。"
LangString CLAPFolderText ${LANG_ENGLISH} "Setup installs Backtracking.clap and its runtime DLLs in a Backtracking subfolder. Add the selected folder to your DAW's CLAP scan paths if needed. Choose a folder writable by your Windows account."
LangString CLAPFolderText ${LANG_TRADCHINESE} "安裝程式會在 Backtracking 子資料夾內安裝 Backtracking.clap 與執行階段 DLL。必要時，請將此處選擇的資料夾加入 DAW 的 CLAP 掃描路徑。請選擇目前 Windows 帳戶可寫入的資料夾。"
!endif

!insertmacro DefinePathFunctions ""
!insertmacro DefinePathFunctions "un."

!macro ReadComponentOption NAME SECTION
  ClearErrors
  ${GetOptions} "$CommandLine" "/${NAME}=" $0
  ${IfNot} ${Errors}
    ${If} $0 == "1"
      !insertmacro SelectSection ${SECTION}
    ${ElseIf} $0 == "0"
      !insertmacro UnselectSection ${SECTION}
    ${Else}
      MessageBox MB_OK|MB_ICONSTOP "Use /${NAME}=1 or /${NAME}=0." /SD IDOK
      SetErrorLevel 87
      Quit
    ${EndIf}
  ${EndIf}
!macroend

!macro ValidatePayloadFile FILE
  StrCpy $PathToCheck "${FILE}"
  Call CheckPayloadFile
  ${If} $PathValid != 1
    Call InvalidFolder
  ${EndIf}
!macroend

!macro ValidateRecordedPlugin KIND VALUE
  StrCpy $PluginKind "${KIND}"
  ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "${VALUE}"
  Call IsOwnedPluginDir
  ${If} $PluginOwned == 1
    Call ValidatePluginOverlap
  ${EndIf}
!macroend

; These are the only app-local runtime DLLs shipped by this installer. If a
; future payload adds another DLL, add its exact name to uninstall as well.
!macro InstallRuntime SOURCE
  !if /FileExists "${SOURCE}\vcruntime140.dll"
    File "${SOURCE}\vcruntime140.dll"
  !endif
  !if /FileExists "${SOURCE}\vcruntime140_1.dll"
    File "${SOURCE}\vcruntime140_1.dll"
  !endif
!macroend

Section "Backtracking desktop (required)" SEC_DESKTOP
  SectionIn RO
  Call EnsureDesktopClosed
  ; Pages do not run in silent mode. Validate every selected destination before
  ; creating application files, shortcuts, or registration in either mode.
  Call ValidateInstallPaths
  ; Fail before copying payload if registration is unavailable (for example a
  ; restricted account). NSIS otherwise silently continues after WriteRegStr.
  ClearErrors
  WriteRegStr HKCU "${INSTALLER_KEY}" "SetupWriteProbe" "$INSTDIR"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONSTOP "Setup could not write its per-user installation registration." /SD IDOK
    SetErrorLevel 5
    Abort
  ${EndIf}
  DeleteRegValue HKCU "${INSTALLER_KEY}" "SetupWriteProbe"
  SetShellVarContext current
  SetRegView 64
  SetOutPath "$INSTDIR"
  File "${PAYLOAD_DIR}\desktop\Backtracking.exe"
  !insertmacro InstallRuntime "${PAYLOAD_DIR}\desktop"
  File "${PAYLOAD_DIR}\LICENSE.txt"
  File "${PAYLOAD_DIR}\FONT-LICENSE.txt"
  File "${PAYLOAD_DIR}\USER_GUIDE.md"
  File "${PAYLOAD_DIR}\BUILD-INFO.txt"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "${STARTMENU_DIR}"
  CreateShortcut "${STARTMENU_DIR}\Backtracking.lnk" "$INSTDIR\Backtracking.exe"
  CreateShortcut "${STARTMENU_DIR}\Uninstall Backtracking.lnk" "$INSTDIR\Uninstall.exe"

  WriteRegStr HKCU "${INSTALLER_KEY}" "InstallDir" "$INSTDIR"
  WriteRegDWORD HKCU "${INSTALLER_KEY}" "InstallerLanguage" $LANGUAGE
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Backtracking contributors"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$\"$INSTDIR\Backtracking.exe$\",0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" "$\"$INSTDIR\Uninstall.exe$\" /S"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/kekekewww/Backtracking"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

!ifdef INCLUDE_PLUGINS
Section /o "VST3 plugin (current user)" SEC_VST3
  StrCpy $PluginKind "VST3"
  StrCpy $PluginDir $VST3Dir
  Call PreparePluginInstall
  SetOutPath "$VST3Dir\Contents\x86_64-win"
  File "${PAYLOAD_DIR}\plugins\Backtracking.vst3\Contents\x86_64-win\Backtracking.vst3"
  !insertmacro InstallRuntime "${PAYLOAD_DIR}\plugins\Backtracking.vst3\Contents\x86_64-win"
  Call FinishPluginInstall
SectionEnd

Section /o "CLAP plugin (current user)" SEC_CLAP
  ; CLAP hosts scan subfolders. The owned subfolder keeps runtime DLLs out of
  ; the shared Common\CLAP root, where another product could own the same name.
  StrCpy $PluginKind "CLAP"
  StrCpy $PluginDir $CLAPDir
  Call PreparePluginInstall
  SetOutPath "$CLAPDir"
  File "${PAYLOAD_DIR}\plugins\Backtracking.clap"
  !insertmacro InstallRuntime "${PAYLOAD_DIR}\plugins"
  Call FinishPluginInstall
SectionEnd
!endif

Section /o "Desktop shortcut" SEC_SHORTCUT
  CreateShortcut "${DESKTOP_SHORTCUT}" "$INSTDIR\Backtracking.exe"
  WriteRegDWORD HKCU "${INSTALLER_KEY}" "DesktopShortcut" 1
SectionEnd

Section -FinishRegistration
  SectionGetSize ${SEC_DESKTOP} $0
!ifdef INCLUDE_PLUGINS
  ${If} ${SectionIsSelected} ${SEC_VST3}
    SectionGetSize ${SEC_VST3} $1
    IntOp $0 $0 + $1
  ${EndIf}
  ${If} ${SectionIsSelected} ${SEC_CLAP}
    SectionGetSize ${SEC_CLAP} $1
    IntOp $0 $0 + $1
  ${EndIf}
!endif
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0
  SetOutPath "$INSTDIR"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} "Backtracking desktop, Start Menu shortcuts, and uninstaller. Your recordings and settings are kept when uninstalling."
!ifdef INCLUDE_PLUGINS
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_VST3} "Install VST3 in a Backtracking.vst3 bundle. Choose its scan folder on the next pages."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_CLAP} "Install CLAP in a Backtracking subfolder. Choose its scan folder independently on the next pages."
!endif
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_SHORTCUT} "Create a Backtracking shortcut on your desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "Backtracking requires a 64-bit version of Windows 10 or later." /SD IDOK
    SetErrorLevel 1633
    Quit
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "Backtracking requires Windows 10 or later." /SD IDOK
    SetErrorLevel 1633
    Quit
  ${EndIf}
  SetShellVarContext current
  SetRegView 64
  ; Preserve selections for components already owned by an earlier Setup.
!ifdef INCLUDE_PLUGINS
  StrCpy $VST3Root "${DEFAULT_VST3_ROOT}"
  StrCpy $CLAPRoot "${DEFAULT_CLAP_ROOT}"
  StrCpy $PluginKind "VST3"
  ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "VST3Dir"
  Call IsOwnedPluginDir
  ${If} $PluginOwned == 1
    ${GetParent} "$PluginDir" $VST3Root
    !insertmacro SelectSection ${SEC_VST3}
  ${EndIf}
  StrCpy $PluginKind "CLAP"
  ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "CLAPDir"
  Call IsOwnedPluginDir
  ${If} $PluginOwned == 1
    ${GetParent} "$PluginDir" $CLAPRoot
    !insertmacro SelectSection ${SEC_CLAP}
  ${EndIf}
  ${GetParameters} $CommandLine
  !insertmacro ReadComponentOption "VST3" ${SEC_VST3}
  !insertmacro ReadComponentOption "CLAP" ${SEC_CLAP}
  ${GetOptions} "$CommandLine" "/VST3DIR=" $0
  ${IfNot} ${Errors}
    StrCpy $VST3Root $0
  ${EndIf}
  ${GetOptions} "$CommandLine" "/CLAPDIR=" $0
  ${IfNot} ${Errors}
    StrCpy $CLAPRoot $0
  ${EndIf}
!endif
  ReadRegDWORD $0 HKCU "${INSTALLER_KEY}" "DesktopShortcut"
  ${If} $0 == 1
    !insertmacro SelectSection ${SEC_SHORTCUT}
  ${EndIf}
FunctionEnd

Function InvalidFolder
  MessageBox MB_OK|MB_ICONSTOP "$(InvalidFolder)$\r$\n$\r$\n$PathToCheck" /SD IDOK
  SetErrorLevel 87
  Abort
FunctionEnd

; A selected scan root may contain both formats, but their owned destinations
; must never contain each other or the desktop installation.
Function CheckPathOverlap
  Push $0
  Push $1
  StrCpy $PathsOverlap 0
  StrLen $0 $PathToCheck
  StrCpy $1 $OtherPath $0
  ${If} $1 == $PathToCheck
    StrCpy $1 $OtherPath 1 $0
    ${If} $1 == ""
    ${OrIf} $1 == "\"
      StrCpy $PathsOverlap 1
    ${EndIf}
  ${EndIf}
  StrLen $0 $OtherPath
  StrCpy $1 $PathToCheck $0
  ${If} $1 == $OtherPath
    StrCpy $1 $PathToCheck 1 $0
    ${If} $1 == ""
    ${OrIf} $1 == "\"
      StrCpy $PathsOverlap 1
    ${EndIf}
  ${EndIf}
  Pop $1
  Pop $0
FunctionEnd

Function ValidatePluginOverlap
  StrCpy $PathToCheck $PluginDir
  StrCpy $OtherPath $INSTDIR
  Call CheckPathOverlap
  ${If} $PathsOverlap == 1
    Call InvalidFolder
  ${EndIf}
  ; A stale uninstaller still owns its old app-local runtime DLLs. Neither new
  ; nor relocated plugins may share those folders with the old desktop copy.
  ${If} $PreviousInstallDir != ""
    StrCpy $OtherPath $PreviousInstallDir
    Call CheckPathOverlap
    ${If} $PathsOverlap == 1
      Call InvalidFolder
    ${EndIf}
  ${EndIf}
  ${If} $VST3Dir != ""
    ${If} $PluginKind != "VST3"
    ${OrIf} $PluginDir != $VST3Dir
      StrCpy $OtherPath $VST3Dir
      Call CheckPathOverlap
      ${If} $PathsOverlap == 1
        Call InvalidFolder
      ${EndIf}
    ${EndIf}
  ${EndIf}
  ${If} $CLAPDir != ""
    ${If} $PluginKind != "CLAP"
    ${OrIf} $PluginDir != $CLAPDir
      StrCpy $OtherPath $CLAPDir
      Call CheckPathOverlap
      ${If} $PathsOverlap == 1
        Call InvalidFolder
      ${EndIf}
    ${EndIf}
  ${EndIf}
FunctionEnd

!ifdef INCLUDE_PLUGINS
Function VST3PagePre
  ${IfNot} ${SectionIsSelected} ${SEC_VST3}
    Abort
  ${EndIf}
FunctionEnd

Function CLAPPagePre
  ${IfNot} ${SectionIsSelected} ${SEC_CLAP}
    Abort
  ${EndIf}
FunctionEnd

Function VST3PageLeave
  StrCpy $PluginKind "VST3"
  Call ValidatePluginRoot
FunctionEnd

Function CLAPPageLeave
  StrCpy $PluginKind "CLAP"
  Call ValidatePluginRoot
FunctionEnd

Function ValidatePluginRoot
  ${If} $PluginKind == "VST3"
    StrCpy $PathToCheck $VST3Root
  ${Else}
    StrCpy $PathToCheck $CLAPRoot
  ${EndIf}
  Call NormalizeFolder
  ${If} $PathValid != 1
    Call InvalidFolder
  ${EndIf}
  ${If} $PluginKind == "VST3"
    StrCpy $VST3Root $PathToCheck
    StrCpy $VST3Dir "$VST3Root\Backtracking.vst3"
    StrCpy $PluginDir $VST3Dir
    StrCpy $OtherPath "$VST3Dir\Contents\x86_64-win\Backtracking.vst3"
  ${Else}
    StrCpy $CLAPRoot $PathToCheck
    StrCpy $CLAPDir "$CLAPRoot\Backtracking"
    StrCpy $PluginDir $CLAPDir
    StrCpy $OtherPath "$CLAPDir\Backtracking.clap"
  ${EndIf}
  ; Do not adopt or overwrite an unrelated, manually installed binary. A new
  ; empty folder or a folder with only unrelated user files is safe to use.
  IfFileExists "$OtherPath" 0 plugin_target_available
  Call IsOwnedPluginDir
  ${If} $PluginOwned != 1
    MessageBox MB_OK|MB_ICONSTOP "$(UnownedPlugin)$\r$\n$\r$\n$PluginDir" /SD IDOK
    SetErrorLevel 87
    Abort
  ${EndIf}
plugin_target_available:
  StrCpy $PathToCheck $PluginDir
  Call NormalizeFolder
  ${If} $PathValid != 1
    Call InvalidFolder
  ${EndIf}
  ${If} $PluginKind == "VST3"
    StrCpy $PathToCheck "$PluginDir\Contents\x86_64-win"
  ${EndIf}
  ; Copies older than the last registered desktop may still have uninstallers.
  ; Their runtime DLLs cannot be shared with a newly installed plugin.
  ${If} ${FileExists} "$PathToCheck\Backtracking.exe"
  ${OrIf} ${FileExists} "$PathToCheck\Uninstall.exe"
    Call InvalidFolder
  ${EndIf}
  Call ProbeWritableFolder
  ${If} $PathValid != 1
    Call InvalidFolder
  ${EndIf}
  !insertmacro ValidatePayloadFile "$PluginDir\${OWNERSHIP_FILE}"
  ${If} $PluginKind == "VST3"
    !insertmacro ValidatePayloadFile "$PluginDir\Contents\x86_64-win\Backtracking.vst3"
    !insertmacro ValidatePayloadFile "$PluginDir\Contents\x86_64-win\vcruntime140.dll"
    !insertmacro ValidatePayloadFile "$PluginDir\Contents\x86_64-win\vcruntime140_1.dll"
  ${Else}
    !insertmacro ValidatePayloadFile "$PluginDir\Backtracking.clap"
    !insertmacro ValidatePayloadFile "$PluginDir\vcruntime140.dll"
    !insertmacro ValidatePayloadFile "$PluginDir\vcruntime140_1.dll"
  ${EndIf}
FunctionEnd
!endif

Function ValidateInstallPaths
  StrCpy $PathToCheck $INSTDIR
  Call NormalizeFolder
  ${If} $PathValid != 1
    Call InvalidFolder
  ${EndIf}
  StrCpy $INSTDIR $PathToCheck
  ReadRegStr $PathToCheck HKCU "${INSTALLER_KEY}" "InstallDir"
  Call NormalizeFolder
  StrCpy $PreviousInstallDir ""
  ${If} $PathValid == 1
    StrCpy $PreviousInstallDir $PathToCheck
  ${EndIf}
  StrCpy $VST3Dir ""
  StrCpy $CLAPDir ""
!ifdef INCLUDE_PLUGINS
  ${If} ${SectionIsSelected} ${SEC_VST3}
    StrCpy $PluginKind "VST3"
    Call ValidatePluginRoot
  ${Else}
!endif
    StrCpy $PluginKind "VST3"
    ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "VST3Dir"
    Call IsOwnedPluginDir
    ${If} $PluginOwned == 1
      StrCpy $VST3Dir $PluginDir
    ${EndIf}
!ifdef INCLUDE_PLUGINS
  ${EndIf}
  ${If} ${SectionIsSelected} ${SEC_CLAP}
    StrCpy $PluginKind "CLAP"
    Call ValidatePluginRoot
  ${Else}
!endif
    StrCpy $PluginKind "CLAP"
    ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "CLAPDir"
    Call IsOwnedPluginDir
    ${If} $PluginOwned == 1
      StrCpy $CLAPDir $PluginDir
    ${EndIf}
!ifdef INCLUDE_PLUGINS
  ${EndIf}
!endif
  ${If} $VST3Dir != ""
    StrCpy $PluginKind "VST3"
    StrCpy $PluginDir $VST3Dir
    Call ValidatePluginOverlap
  ${EndIf}
  ${If} $CLAPDir != ""
    StrCpy $PluginKind "CLAP"
    StrCpy $PluginDir $CLAPDir
    Call ValidatePluginOverlap
  ${EndIf}
  ; Previous destinations may still be removed after extraction. Check them
  ; too, before any new desktop files could be mistaken for an old runtime.
  !insertmacro ValidateRecordedPlugin "VST3" "VST3Dir"
  !insertmacro ValidateRecordedPlugin "VST3" "VST3PreviousDir"
  !insertmacro ValidateRecordedPlugin "CLAP" "CLAPDir"
  !insertmacro ValidateRecordedPlugin "CLAP" "CLAPPreviousDir"
  StrCpy $PathToCheck $INSTDIR
  Call ProbeWritableFolder
  ${If} $PathValid != 1
    Call InvalidFolder
  ${EndIf}
  !insertmacro ValidatePayloadFile "$INSTDIR\Backtracking.exe"
  !insertmacro ValidatePayloadFile "$INSTDIR\vcruntime140.dll"
  !insertmacro ValidatePayloadFile "$INSTDIR\vcruntime140_1.dll"
  !insertmacro ValidatePayloadFile "$INSTDIR\LICENSE.txt"
  !insertmacro ValidatePayloadFile "$INSTDIR\FONT-LICENSE.txt"
  !insertmacro ValidatePayloadFile "$INSTDIR\USER_GUIDE.md"
  !insertmacro ValidatePayloadFile "$INSTDIR\BUILD-INFO.txt"
  !insertmacro ValidatePayloadFile "$INSTDIR\Uninstall.exe"
FunctionEnd

; Opening the desktop's mutex detects both its window and its tray-only state.
; Retry gives the user time to exit normally; Setup never terminates processes.
!macro DesktopGuard PREFIX
Function ${PREFIX}EnsureDesktopClosed
  Push $0
guard_retry:
  System::Call 'kernel32::OpenMutexW(i 0x00100000, i 0, w "${DESKTOP_MUTEX}") p .r0'
  ${If} $0 != 0
    System::Call 'kernel32::CloseHandle(p r0)'
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Backtracking is running. Exit Backtracking from its window or system-tray menu, then click Retry.$\r$\n$\r$\nNo processes will be closed automatically." /SD IDCANCEL IDRETRY guard_retry
    Pop $0
    SetErrorLevel 2
    Quit
  ${EndIf}
  Pop $0
FunctionEnd
!macroend
!insertmacro DesktopGuard ""
!insertmacro DesktopGuard "un."

Function LaunchBacktracking
  ; This callback is invoked only by the finish-page checkbox. Keep an explicit
  ; silent guard so /S can never start the desktop application.
  IfSilent launch_done
  SetOutPath "$INSTDIR"
  Exec '$\"$INSTDIR\Backtracking.exe$\"'
launch_done:
FunctionEnd

Function un.onInit
  SetShellVarContext current
  SetRegView 64
  StrCpy $PathToCheck $INSTDIR
  Call un.NormalizeFolder
  ${If} $PathValid != 1
    MessageBox MB_OK|MB_ICONSTOP "$(InvalidFolder)$\r$\n$\r$\n$INSTDIR" /SD IDOK
    SetErrorLevel 87
    Quit
  ${EndIf}
  StrCpy $INSTDIR $PathToCheck
  ReadRegDWORD $0 HKCU "${INSTALLER_KEY}" "InstallerLanguage"
  ${If} $0 == ${LANG_ENGLISH}
  ${OrIf} $0 == ${LANG_TRADCHINESE}
    StrCpy $LANGUAGE $0
  ${EndIf}
  Call un.EnsureDesktopClosed
  ; An old uninstaller left in a different folder must not unregister a newer
  ; installation or remove the newer installation's shortcuts and plugins.
  StrCpy $RegisteredInstall 0
  ReadRegStr $0 HKCU "${INSTALLER_KEY}" "InstallDir"
  ${If} $0 == $INSTDIR
    StrCpy $RegisteredInstall 1
  ${EndIf}
FunctionEnd

; Delete retries for locked files instead of silently leaving a registered
; component partially removed. Labels are unique within the uninstall section.
!macro DeleteOwnedFile FILE LABEL
  IfFileExists "${FILE}" 0 ${LABEL}_done
${LABEL}_retry:
  ClearErrors
  Delete "${FILE}"
  ${If} ${Errors}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Could not remove:$\r$\n${FILE}$\r$\n$\r$\nClose Backtracking, your DAW, or any application using this file, then click Retry." /SD IDCANCEL IDRETRY ${LABEL}_retry
    SetErrorLevel 2
    Abort
  ${EndIf}
${LABEL}_done:
!macroend

; Custom plugin folders need a product marker and the expected fixed leaf.
; Markerless legacy ownership is accepted only at the former exact defaults
; and only when that path is still recorded by this installer.
!macro PluginOwnershipFunctions PREFIX
Function ${PREFIX}SetPluginRegistryValues
  ${If} $PluginKind == "VST3"
    StrCpy $PluginValue "VST3Dir"
    StrCpy $PreviousPluginValue "VST3PreviousDir"
  ${Else}
    StrCpy $PluginValue "CLAPDir"
    StrCpy $PreviousPluginValue "CLAPPreviousDir"
  ${EndIf}
FunctionEnd

Function ${PREFIX}IsOwnedPluginDir
  Push $0
  Push $1
  StrCpy $PluginOwned 0
  Call ${PREFIX}SetPluginRegistryValues
  StrCpy $PathToCheck $PluginDir
  Call ${PREFIX}NormalizeFolder
  ${If} $PathValid != 1
    Goto ownership_done
  ${EndIf}
  StrCpy $PluginDir $PathToCheck
  ${GetFileName} "$PluginDir" $0
  ${If} $PluginKind == "VST3"
    ${If} $0 != "Backtracking.vst3"
      Goto ownership_done
    ${EndIf}
    StrCpy $1 "${DEFAULT_VST3_ROOT}\Backtracking.vst3"
    ; The runtime directory can itself be a junction even when the bundle is
    ; ordinary. Validate it before reading ownership or deleting payload files.
    StrCpy $PathToCheck "$PluginDir\Contents\x86_64-win"
    Call ${PREFIX}NormalizeFolder
    ${If} $PathValid != 1
      Goto ownership_done
    ${EndIf}
  ${ElseIf} $PluginKind == "CLAP"
    ${If} $0 != "Backtracking"
      Goto ownership_done
    ${EndIf}
    StrCpy $1 "${DEFAULT_CLAP_ROOT}\Backtracking"
  ${Else}
    Goto ownership_done
  ${EndIf}
  StrCpy $PathToCheck "$PluginDir\${OWNERSHIP_FILE}"
  Call ${PREFIX}CheckPayloadFile
  ${If} $PathValid != 1
    Goto ownership_done
  ${EndIf}
  ReadINIStr $0 "$PluginDir\${OWNERSHIP_FILE}" "Ownership" "Product"
  ${If} $0 == "${INSTALLER_KEY}"
    StrCpy $PluginOwned 1
  ${ElseIf} $PluginDir == $1
    ReadRegStr $0 HKCU "${INSTALLER_KEY}" "$PluginValue"
    ${If} $0 == $PluginDir
      StrCpy $PluginOwned 1
    ${Else}
      ReadRegStr $0 HKCU "${INSTALLER_KEY}" "$PreviousPluginValue"
      ${If} $0 == $PluginDir
        StrCpy $PluginOwned 1
      ${EndIf}
    ${EndIf}
  ${EndIf}
ownership_done:
  Pop $1
  Pop $0
FunctionEnd
!macroend
!insertmacro PluginOwnershipFunctions ""
!insertmacro PluginOwnershipFunctions "un."

!macro RemovePluginFunction PREFIX
Function ${PREFIX}RemovePlugin
  Call ${PREFIX}IsOwnedPluginDir
  ${If} $PluginOwned != 1
    DetailPrint "Skipping unowned or invalid $PluginKind path: $PluginDir"
    Return
  ${EndIf}
  ${If} $PluginKind == "VST3"
    !insertmacro DeleteOwnedFile "$PluginDir\Contents\x86_64-win\Backtracking.vst3" remove_vst3
    !insertmacro DeleteOwnedFile "$PluginDir\Contents\x86_64-win\vcruntime140.dll" remove_vst3_runtime
    !insertmacro DeleteOwnedFile "$PluginDir\Contents\x86_64-win\vcruntime140_1.dll" remove_vst3_runtime1
    RMDir "$PluginDir\Contents\x86_64-win"
    RMDir "$PluginDir\Contents"
  ${Else}
    !insertmacro DeleteOwnedFile "$PluginDir\Backtracking.clap" remove_clap
    !insertmacro DeleteOwnedFile "$PluginDir\vcruntime140.dll" remove_clap_runtime
    !insertmacro DeleteOwnedFile "$PluginDir\vcruntime140_1.dll" remove_clap_runtime1
  ${EndIf}
  !insertmacro DeleteOwnedFile "$PluginDir\${OWNERSHIP_FILE}" remove_marker
  RMDir "$PluginDir"
FunctionEnd
!macroend
!insertmacro RemovePluginFunction "un."

!ifdef INCLUDE_PLUGINS
!insertmacro RemovePluginFunction ""
Function PreparePluginInstall
  Call SetPluginRegistryValues
  ; Recover an interrupted earlier relocation before starting another one.
  Push $PluginDir
  ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "$PreviousPluginValue"
  ${If} $PluginDir != ""
    Call RemovePlugin
    DeleteRegValue HKCU "${INSTALLER_KEY}" "$PreviousPluginValue"
  ${EndIf}
  Pop $PluginDir
  ReadRegStr $OldPluginDir HKCU "${INSTALLER_KEY}" "$PluginValue"
  ${If} $OldPluginDir != ""
  ${AndIf} $OldPluginDir != $PluginDir
    WriteRegStr HKCU "${INSTALLER_KEY}" "$PreviousPluginValue" "$OldPluginDir"
  ${EndIf}
  ; Journal both paths before extraction. Even an interrupted copy can then
  ; be removed by the next Setup/uninstall without orphaning the old copy.
  WriteRegStr HKCU "${INSTALLER_KEY}" "$PluginValue" "$PluginDir"
  CreateDirectory "$PluginDir"
  ClearErrors
  WriteINIStr "$PluginDir\${OWNERSHIP_FILE}" "Ownership" "Product" "${INSTALLER_KEY}"
  ${If} ${Errors}
    StrCpy $PathToCheck $PluginDir
    Call InvalidFolder
  ${EndIf}
FunctionEnd

Function FinishPluginInstall
  Call SetPluginRegistryValues
  ; SetOutPath away from the old folder so empty-folder removal can succeed.
  SetOutPath "$INSTDIR"
  ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "$PreviousPluginValue"
  ${If} $PluginDir != ""
    Call RemovePlugin
    DeleteRegValue HKCU "${INSTALLER_KEY}" "$PreviousPluginValue"
  ${EndIf}
FunctionEnd
!endif

Section "Uninstall"
  Call un.EnsureDesktopClosed
  ${If} $RegisteredInstall == 1
    StrCpy $PluginKind "VST3"
    ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "VST3Dir"
    Call un.RemovePlugin
    ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "VST3PreviousDir"
    Call un.RemovePlugin
    StrCpy $PluginKind "CLAP"
    ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "CLAPDir"
    Call un.RemovePlugin
    ReadRegStr $PluginDir HKCU "${INSTALLER_KEY}" "CLAPPreviousDir"
    Call un.RemovePlugin
  ${EndIf}

  !insertmacro DeleteOwnedFile "$INSTDIR\Backtracking.exe" un_desktop
  !insertmacro DeleteOwnedFile "$INSTDIR\vcruntime140.dll" un_desktop_runtime
  !insertmacro DeleteOwnedFile "$INSTDIR\vcruntime140_1.dll" un_desktop_runtime1
  !insertmacro DeleteOwnedFile "$INSTDIR\LICENSE.txt" un_license
  !insertmacro DeleteOwnedFile "$INSTDIR\FONT-LICENSE.txt" un_font_license
  !insertmacro DeleteOwnedFile "$INSTDIR\USER_GUIDE.md" un_guide
  !insertmacro DeleteOwnedFile "$INSTDIR\BUILD-INFO.txt" un_buildinfo

  ; An old copy may still own the login entry even after another installation
  ; becomes current. Remove only an exact reference to the copy being removed.
  ReadRegStr $0 HKCU "${RUN_KEY}" "${RUN_VALUE}"
  ${If} $0 == '$\"$INSTDIR\Backtracking.exe$\"'
    DeleteRegValue HKCU "${RUN_KEY}" "${RUN_VALUE}"
  ${EndIf}

  ${If} $RegisteredInstall == 1
    !insertmacro DeleteOwnedFile "${STARTMENU_DIR}\Backtracking.lnk" un_menu_app
    !insertmacro DeleteOwnedFile "${STARTMENU_DIR}\Uninstall Backtracking.lnk" un_menu_uninstall
    RMDir "${STARTMENU_DIR}"
    ReadRegDWORD $0 HKCU "${INSTALLER_KEY}" "DesktopShortcut"
    ${If} $0 == 1
      !insertmacro DeleteOwnedFile "${DESKTOP_SHORTCUT}" un_shortcut
    ${EndIf}
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    ; Only this installer subkey is removed. Application preferences may live
    ; elsewhere under Software\Backtracking and must remain untouched.
    DeleteRegKey HKCU "${INSTALLER_KEY}"
  ${EndIf}
  !insertmacro DeleteOwnedFile "$INSTDIR\Uninstall.exe" un_self
  RMDir "$INSTDIR"
SectionEnd

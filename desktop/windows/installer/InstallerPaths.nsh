; Shared path checks for Setup and its uninstaller. A selected scan root is
; never deleted; callers append their fixed, product-owned subdirectories.
!ifndef BACKTRACKING_INSTALLER_PATHS_INCLUDED
!define BACKTRACKING_INSTALLER_PATHS_INCLUDED
!include "FileFunc.nsh"

Var PathToCheck
Var PathValid

!macro RejectSystemTree FOLDER
  StrCmp $PathToCheck "${FOLDER}" path_done
  StrLen $0 "${FOLDER}\"
  StrCpy $1 $PathToCheck $0
  StrCmp $1 "${FOLDER}\" path_done
!macroend

!macro DefinePathFunctions PREFIX
Function ${PREFIX}NormalizeFolder
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  StrCpy $PathValid 0
  StrCmp $PathToCheck "" path_done

  ; Accept only drive-absolute paths or UNC paths, never shell/device paths.
  StrCpy $0 $PathToCheck 4
  StrCmp $0 "\\.\" path_done
  StrCmp $0 "\\?\" path_done
  StrCpy $0 $PathToCheck 2
  StrCpy $5 0
  StrCmp $0 "\\" path_characters
  System::Call 'shlwapi::PathGetDriveNumberW(w "$PathToCheck") i .r0'
  IntCmp $0 -1 path_done
  StrCpy $0 $PathToCheck 1 2
  StrCmp $0 "\" 0 path_done
  StrCpy $5 1

path_characters:
  StrCpy $0 0
  StrCpy $3 ""
path_character_loop:
  StrCpy $2 $PathToCheck 1 $0
  StrCmp $2 "" path_canonicalize
  StrCmp $2 "*" path_done
  StrCmp $2 "?" path_done
  StrCmp $2 '$\"' path_done
  StrCmp $2 "<" path_done
  StrCmp $2 ">" path_done
  StrCmp $2 "|" path_done
  StrCmp $2 "/" path_done
  StrCmp $2 "$\r" path_done
  StrCmp $2 "$\n" path_done
  StrCmp $2 "$\t" path_done
  StrCmp $2 ":" 0 path_separator
  StrCmp $5 1 0 path_done
  StrCmp $0 1 0 path_done
path_separator:
  ; Win32 silently aliases trailing dots/spaces. Reject those ambiguous names
  ; rather than later comparing a different path from the one Windows opens.
  StrCmp $2 "\" 0 path_next_character
  StrCmp $3 "." path_done
  StrCmp $3 " " path_done
path_next_character:
  StrCpy $3 $2
  IntOp $0 $0 + 1
  Goto path_character_loop

path_canonicalize:
  StrCmp $3 "." path_done
  StrCmp $3 " " path_done
  System::Call 'shlwapi::PathIsRelativeW(w "$PathToCheck") i .r0'
  StrCmp $0 0 0 path_done
  ; NSIS GetFullPathName requires an existing path. The Win32 API also handles
  ; the new directories a user normally selects during a fresh installation.
  System::Call 'kernel32::GetFullPathNameW(w "$PathToCheck", i ${NSIS_MAX_STRLEN}, w .r4, p 0) i .r0'
  StrCmp $0 0 path_done
  IntCmp $0 ${NSIS_MAX_STRLEN} path_done 0 path_done
  StrCpy $PathToCheck $4
path_trim_separator:
  StrCpy $0 $PathToCheck 1 -1
  StrCmp $0 "\" 0 path_check_root
  System::Call 'shlwapi::PathIsRootW(w "$PathToCheck") i .r0'
  StrCmp $0 0 0 path_done
  StrCpy $PathToCheck $PathToCheck -1
  Goto path_trim_separator
path_check_root:
  System::Call 'shlwapi::PathIsRootW(w "$PathToCheck") i .r0'
  StrCmp $0 0 0 path_done
  StrLen $0 $PathToCheck
  ; Fixed VST3 payload suffixes and ownership markers must fit MAX_PATH.
  IntCmp $0 200 0 0 path_done
  !insertmacro RejectSystemTree "$WINDIR"
  !insertmacro RejectSystemTree "$SYSDIR"
  StrCmp $PathToCheck "$PROFILE" path_done
  StrCmp $PathToCheck "$APPDATA" path_done
  StrCmp $PathToCheck "$LOCALAPPDATA" path_done
  StrCmp $PathToCheck "$PROGRAMFILES32" path_done
  StrCmp $PathToCheck "$PROGRAMFILES64" path_done
  StrCmp $PathToCheck "$COMMONFILES32" path_done
  StrCmp $PathToCheck "$COMMONFILES64" path_done

  ; Do not follow existing junctions/symlinks during either copy or cleanup.
  ; Walk all ancestors, including those above a not-yet-created destination.
  StrCpy $0 $PathToCheck
path_ancestor_loop:
  System::Call 'kernel32::GetFileAttributesW(w r0) i .r1'
  StrCmp $1 -1 path_parent
  IntOp $2 $1 & 0x400
  StrCmp $2 0 0 path_done
  IntOp $2 $1 & 0x10
  StrCmp $2 0 path_done
path_parent:
  System::Call 'shlwapi::PathIsRootW(w r0) i .r1'
  StrCmp $1 0 0 path_valid
  ${GetParent} "$0" $1
  StrCmp $1 "" path_done
  StrCmp $1 $0 path_done
  StrCpy $0 $1
  StrCpy $1 $0 1 -1
  StrCmp $1 ":" 0 path_ancestor_loop
  StrCpy $0 "$0\"
  Goto path_ancestor_loop
path_valid:
  StrCpy $PathValid 1
path_done:
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

Function ${PREFIX}CheckPayloadFile
  Push $0
  Push $1
  StrCpy $PathValid 0
  System::Call 'kernel32::GetFileAttributesW(w "$PathToCheck") i .r0 ?e'
  Pop $1
  StrCmp $0 -1 payload_absent
  IntOp $1 $0 & 0x410
  StrCmp $1 0 payload_valid payload_done
payload_absent:
  ; A permissions or I/O error must not be treated as permission to overwrite.
  StrCmp $1 2 payload_valid
  StrCmp $1 3 0 payload_done
payload_valid:
  StrCpy $PathValid 1
payload_done:
  Pop $1
  Pop $0
FunctionEnd

; Uninstall never creates destinations or writes a probe.
!if "${PREFIX}" == ""
Function ${PREFIX}ProbeWritableFolder
  Push $0
  Push $1
  Call ${PREFIX}NormalizeFolder
  StrCmp $PathValid 1 0 probe_done
  StrCpy $PathValid 0
  ClearErrors
  CreateDirectory "$PathToCheck"
  IfErrors probe_done
  ; Recheck after creation in case any new component is a reparse point.
  Call ${PREFIX}NormalizeFolder
  StrCmp $PathValid 1 0 probe_done
  StrCpy $PathValid 0
  ClearErrors
  GetTempFileName $0 "$PathToCheck"
  IfErrors probe_done
  ClearErrors
  FileOpen $1 "$0" w
  IfErrors probe_delete
  FileWrite $1 "Backtracking Setup write check$\r$\n"
  FileClose $1
  IfErrors probe_delete
  ClearErrors
  Delete "$0"
  IfErrors probe_done
  StrCpy $PathValid 1
  Goto probe_done
probe_delete:
  Delete "$0"
probe_done:
  Pop $1
  Pop $0
FunctionEnd
!endif
!macroend
!endif

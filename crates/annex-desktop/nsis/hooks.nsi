; Annex NSIS installer hooks. Never recursively follow a redirected data path.
!include "FileFunc.nsh"

Var AnnexDataError
Var AnnexDeleteData

; Validate every existing ancestor, including the requested path. An absent
; path is already clean, but inaccessible paths must never be treated as absent.
Function un.AnnexCheckAncestors
  Exch $0
  Push $1
  Push $2
  Push $3
  AncestorNext:
    StrCmp $AnnexDataError "" 0 AncestorDone
    StrCmp $0 "" AncestorDone
    System::Call 'kernel32::GetFileAttributesW(w r0) i .r1 ?e'
    Pop $2
    StrCmp $1 -1 AncestorMissing
    IntOp $3 $1 & 0x400 ; FILE_ATTRIBUTE_REPARSE_POINT
    StrCmp $3 0 AncestorParent
    StrCpy $AnnexDataError "Refusing to follow a redirected folder or file: $0"
    Goto AncestorDone
  AncestorMissing:
    StrCmp $2 2 AncestorParent ; ERROR_FILE_NOT_FOUND
    StrCmp $2 3 AncestorParent ; ERROR_PATH_NOT_FOUND
    StrCpy $AnnexDataError "Cannot inspect $0 (Windows error $2)."
    Goto AncestorDone
  AncestorParent:
    ; GetParent(C:\) returns C:, so stop at a drive root explicitly.
    StrLen $3 $0
    IntCmp $3 3 AncestorDone AncestorDone
    ${un.GetParent} "$0" $1
    StrCmp $0 $1 AncestorDone
    StrCpy $0 $1
    Goto AncestorNext
  AncestorDone:
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; First call with AnnexDeleteData=0 for all three trees before any mutation.
; The second pass checks again and deletes exact files/empty directories only.
; No recursive directory removal, wildcard Delete, or delayed deletion is used.
Function un.AnnexDataTree
  Exch $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  StrCmp $AnnexDataError "" 0 TreeDone
  Push $0
  Call un.AnnexCheckAncestors
  StrCmp $AnnexDataError "" 0 TreeDone
  System::Call 'kernel32::GetFileAttributesW(w r0) i .r1 ?e'
  Pop $2
  StrCmp $1 -1 TreeMissing
  IntOp $3 $1 & 0x10 ; FILE_ATTRIBUTE_DIRECTORY
  StrCmp $3 0 TreeFile
  ; Use Win32 directly so enumeration errors remain distinguishable from its
  ; normal end. NSIS FindNext does not preserve GetLastError for plugins.
  System::Alloc 592 ; sizeof(WIN32_FIND_DATAW)
  Pop $5
  StrCmp $5 0 TreeAllocationError
  System::Call 'kernel32::FindFirstFileW(w "$0\*", p r5) p .r3 ?e'
  Pop $2
  StrCmp $3 -1 TreeFirstError
  TreeNext:
    IntOp $2 $5 + 44 ; WIN32_FIND_DATAW.cFileName
    System::Call '*$2(&w260 .r4)'
    StrCmp $4 "." TreeAdvance
    StrCmp $4 ".." TreeAdvance
    Push "$0\$4"
    Call un.AnnexDataTree
    StrCmp $AnnexDataError "" 0 TreeClose
  TreeAdvance:
    System::Call 'kernel32::FindNextFileW(p r3, p r5) i .r1 ?e'
    Pop $2
    StrCmp $1 0 0 TreeNext
    StrCmp $2 18 TreeClose ; ERROR_NO_MORE_FILES
    StrCpy $AnnexDataError "Cannot finish inspecting $0 (Windows error $2)."
  TreeClose:
    System::Call 'kernel32::FindClose(p r3)'
    System::Free $5
    StrCmp $AnnexDataError "" 0 TreeDone
    Goto TreeDirectory
  TreeFirstError:
    System::Free $5
    StrCmp $2 2 TreeDirectory ; empty directory
    StrCpy $AnnexDataError "Cannot inspect the contents of $0 (Windows error $2)."
    Goto TreeDone
  TreeAllocationError:
    StrCpy $AnnexDataError "Not enough memory to inspect $0."
    Goto TreeDone
  TreeDirectory:
    StrCmp $AnnexDeleteData 1 0 TreeDone
    ; Only succeeds once the directory is empty; newly-created content fails.
    System::Call 'kernel32::RemoveDirectoryW(w r0) i .r1 ?e'
    Pop $2
    StrCmp $1 0 TreeDeleteError TreeGone
  TreeFile:
    StrCmp $AnnexDeleteData 1 0 TreeDone
    System::Call 'kernel32::DeleteFileW(w r0) i .r1 ?e'
    Pop $2
    StrCmp $1 0 TreeDeleteError TreeGone
  TreeGone:
    System::Call 'kernel32::GetFileAttributesW(w r0) i .r1 ?e'
    Pop $2
    StrCmp $1 -1 TreeMissing
    StrCpy $AnnexDataError "Data remains at $0. Close Annex and retry."
    Goto TreeDone
  TreeDeleteError:
    StrCpy $AnnexDataError "Could not remove $0 (Windows error $2). Close Annex and retry."
    Goto TreeDone
  TreeMissing:
    StrCmp $2 2 TreeDone
    StrCmp $2 3 TreeDone
    StrCpy $AnnexDataError "Cannot inspect $0 (Windows error $2)."
  TreeDone:
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

!macro AnnexDefaultDataTrees
  Push "$APPDATA\Annex"
  Call un.AnnexDataTree
  Push "$LOCALAPPDATA\com.annex.desktop"
  Call un.AnnexDataTree
  Push "$LOCALAPPDATA\Annex"
  Call un.AnnexDataTree
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Preserve data and credentials by default, including silent updates.
  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Remove this Windows account's default Annex data and saved credentials (database, config, uploads, cached data)?$\nIsolated profiles are preserved. This cannot be undone." /SD IDNO IDNO SkipDataClean

    ; A machine-wide installer uses 'all', but data belongs to this account.
    SetShellVarContext current
    StrCpy $AnnexDataError ""
    StrCpy $AnnexDeleteData 0
    !insertmacro AnnexDefaultDataTrees
    StrCmp $AnnexDataError "" 0 DataCleanFailed

    Push $0
    Push $1
    ; keyring 3.x names a Windows generic credential as user.service. Never
    ; enumerate credentials or match a prefix: isolated namespaces are separate.
    System::Call 'advapi32::CredDeleteW(w "webrtc-api-secret.com.annex.desktop", i 1, i 0) i .r0 ?e'
    Pop $1
    StrCmp $0 0 0 CredentialRemoved
    StrCmp $1 1168 CredentialRemoved ; ERROR_NOT_FOUND is already clean
    StrCpy $AnnexDataError "Could not remove the saved credential (Windows error $1). No Annex data was deleted."
    Pop $1
    Pop $0
    Goto DataCleanFailed
  CredentialRemoved:
    Pop $1
    Pop $0
    StrCpy $AnnexDeleteData 1
    !insertmacro AnnexDefaultDataTrees
    StrCmp $AnnexDataError "" 0 DataCleanFailed
    StrCmp $MultiUser.InstallMode "AllUsers" 0 SkipDataClean
    SetShellVarContext all
    Goto SkipDataClean
  DataCleanFailed:
    StrCmp $MultiUser.InstallMode "AllUsers" 0 +2
    SetShellVarContext all
    MessageBox MB_OK|MB_ICONSTOP "Full data removal did not complete.$\n$AnnexDataError$\nUninstall has stopped. Existing isolated profiles were not selected for removal." /SD IDOK
    Abort "Full data removal did not complete."
  SkipDataClean:
!macroend

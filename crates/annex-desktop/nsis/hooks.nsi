; Annex NSIS installer hooks.
; Tauri injects these macros into the generated installer.nsi via
; the bundle.windows.nsis.installerHooks config field.

!macro NSIS_HOOK_PREUNINSTALL
  ; Ask the user whether to delete their server data (database, config,
  ; uploads, cached binaries). Defaults to "No" on silent uninstall.
  MessageBox MB_YESNO "Remove Annex server data (database, config, uploads)?$\nThis cannot be undone." /SD IDNO IDNO SkipDataClean
    RMDir /r "$APPDATA\Annex"
  SkipDataClean:
!macroend

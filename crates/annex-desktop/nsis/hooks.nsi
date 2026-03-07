; Annex NSIS installer hooks.
; Tauri injects these macros into the generated installer.nsi via
; the bundle.windows.nsis.installerHooks config field.

!macro NSIS_HOOK_PREUNINSTALL
  ; Ask the user whether to delete their data.
  ; Defaults to "No" on silent uninstall (/SD IDNO).
  MessageBox MB_YESNO "Remove all Annex data (database, config, uploads, cached data)?$\nThis cannot be undone." /SD IDNO IDNO SkipDataClean

    ; Server data: database, config.toml, uploads, signing keys, LiveKit binaries
    RMDir /r "$APPDATA\Annex"

    ; WebView2 user data: cache, cookies, IndexedDB, localStorage, service workers.
    ; Tauri stores this under %LOCALAPPDATA%\<bundle-identifier>.
    RMDir /r "$LOCALAPPDATA\com.annex.desktop"

    ; Tauri may also create a directory under the product name for logs/crash data.
    RMDir /r "$LOCALAPPDATA\Annex"

  SkipDataClean:
!macroend

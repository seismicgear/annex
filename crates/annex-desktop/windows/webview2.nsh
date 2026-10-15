; A stale nonempty EdgeUpdate registration bypasses Tauri's WebView2 section.
; Probe the actual SDK, then use the same embedded Evergreen installer to repair.
!define ANNEX_WEBVIEW2_PROBE "${__FILEDIR__}\..\..\..\assets\webview2\AnnexWebView2Probe.dll"
!define ANNEX_WEBVIEW2_VERIFY "${__FILEDIR__}\..\..\..\scripts\verify-webview2-bundle.ps1"

!macro NSIS_HOOK_PREINSTALL
  !if "${INSTALLWEBVIEW2MODE}" != "offlineInstaller"
    !error "Annex Windows setup requires the embedded offline WebView2 installer."
  !endif
  ; Tauri obtains the current Evergreen standalone installer from Microsoft.
  ; Authenticate those exact input bytes before NSIS embeds them.
  !system 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "${ANNEX_WEBVIEW2_VERIFY}" -InstallerPath "${WEBVIEW2INSTALLERPATH}"' = 0
  InitPluginsDir
  File "/oname=$PLUGINSDIR\AnnexWebView2Probe.dll" "${ANNEX_WEBVIEW2_PROBE}"
  ; System leaves the output untouched if loading/export resolution fails.
  ; Never let a previous success value stand in for a probe result.
  StrCpy $0 3
  System::Call '"$PLUGINSDIR\AnnexWebView2Probe.dll"::AnnexWebView2Probe() i .r0'
  ${If} $0 == 2
    DetailPrint "Repairing the Microsoft WebView2 Runtime..."
    File "/oname=$PLUGINSDIR\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"
    ClearErrors
    ExecWait '"$PLUGINSDIR\MicrosoftEdgeWebView2RuntimeInstaller.exe" /silent /install' $1
    ${If} ${Errors}
    ${OrIf} $1 != 0
      SetErrorLevel 1
      Abort "Microsoft WebView2 Runtime installation failed. Annex was not installed."
    ${EndIf}
    StrCpy $0 3
    System::Call '"$PLUGINSDIR\AnnexWebView2Probe.dll"::AnnexWebView2Probe() i .r0'
  ${EndIf}
  ${If} $0 != 0
    SetErrorLevel 1
    Abort "The Microsoft WebView2 Runtime is unavailable. Annex was not installed. Repair WebView2 and retry setup."
  ${EndIf}
!macroend

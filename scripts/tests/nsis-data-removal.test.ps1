param(
  [Parameter(Mandatory=$true)][string]$ScratchDir,
  [string]$NsisCompiler = "$env:LOCALAPPDATA\tauri\NSIS\makensis.exe"
)
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath($ScratchDir)
if (-not [IO.Directory]::Exists($taskRoot)) { throw 'ScratchDir must already exist.' }
$fixtureRoot = Join-Path $taskRoot ('annex-uninstall-check-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixtureRoot) | Out-Null
# Every executable and selected removal target is a newly created descendant
# of this explicit scratch root. The default-profile macro is never invoked.
$selected = @('normal','junction','locked','ancestor-target')
foreach ($name in $selected) { [IO.Directory]::CreateDirectory((Join-Path $fixtureRoot $name)) | Out-Null }
[IO.Directory]::CreateDirectory((Join-Path $fixtureRoot 'normal\nested\empty')) | Out-Null
[IO.File]::WriteAllText((Join-Path $fixtureRoot 'normal\nested\plain.txt'), 'fixture')
[IO.File]::WriteAllText((Join-Path $fixtureRoot 'normal\name with spaces.txt'), 'fixture')
[IO.Directory]::CreateDirectory((Join-Path $fixtureRoot 'external')) | Out-Null
[IO.File]::WriteAllText((Join-Path $fixtureRoot 'external\sentinel.txt'), 'must remain')
[IO.File]::WriteAllText((Join-Path $fixtureRoot 'ancestor-target\sentinel.txt'), 'must remain')
New-Item -ItemType Junction -Path (Join-Path $fixtureRoot 'junction\redirect') -Target (Join-Path $fixtureRoot 'external') | Out-Null
New-Item -ItemType Junction -Path (Join-Path $fixtureRoot 'ancestor-alias') -Target (Join-Path $fixtureRoot 'ancestor-target') | Out-Null
$lockPath = Join-Path $fixtureRoot 'locked\busy.db'
[IO.File]::WriteAllText($lockPath, 'locked fixture')
$lock = [IO.File]::Open($lockPath, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
function ConvertTo-NsisLiteral([string]$Text) { return $Text.Replace('$','$$').Replace('"','$\"') }
$fixtureLiteral = ConvertTo-NsisLiteral $fixtureRoot
$hook = ConvertTo-NsisLiteral ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\crates\annex-desktop\nsis\hooks.nsi')))
$template = @'
Unicode true
Name "Annex scratch-only data removal test"
OutFile "fixture.exe"
RequestExecutionLevel user
SilentInstall silent
SilentUnInstall silent
Var MultiUser.InstallMode
!include "@@HOOK@@"
; Tauri declares these after including hooks, before expanding PREUNINSTALL.
Var DeleteAppDataCheckboxState
Var UpdateMode
Var PassiveMode
!define FIXTURE "@@ROOT@@"
Section
  WriteUninstaller "${FIXTURE}\fixture-uninstall.exe"
SectionEnd
!macro CheckTree NAME PATH DELETE
  StrCpy $AnnexDataError ""
  StrCpy $AnnexDeleteData 0
  Push "${PATH}"
  Call un.AnnexDataTree
  StrCmp $AnnexDataError "" 0 +4
  StrCpy $AnnexDeleteData ${DELETE}
  Push "${PATH}"
  Call un.AnnexDataTree
  FileWrite $9 "${NAME}=$AnnexDataError$\r$\n"
!macroend
!macro CheckChoice NAME CHECKED UPDATE PASSIVE
  StrCpy $DeleteAppDataCheckboxState ${CHECKED}
  StrCpy $UpdateMode ${UPDATE}
  StrCpy $PassiveMode ${PASSIVE}
  !insertmacro AnnexReadDataRemovalChoice
  FileWrite $9 "${NAME}=$AnnexDeleteData,$DeleteAppDataCheckboxState$\r$\n"
!macroend
Section "Uninstall"
  FileOpen $9 "${FIXTURE}\results.txt" w
  !insertmacro CheckTree "normal" "${FIXTURE}\normal" 1
  !insertmacro CheckTree "absent" "${FIXTURE}\absent" 1
  !insertmacro CheckTree "junction" "${FIXTURE}\junction" 1
  !insertmacro CheckTree "ancestor" "${FIXTURE}\ancestor-alias\sentinel.txt" 1
  !insertmacro CheckTree "locked" "${FIXTURE}\locked" 1
  SetSilent normal
  !insertmacro CheckChoice "unchecked-preserves" 0 0 0
  !insertmacro CheckChoice "checked-offers-guarded-removal" 1 0 0
  !insertmacro CheckChoice "updating-preserves" 1 1 0
  !insertmacro CheckChoice "passive-preserves" 1 0 1
  SetSilent silent
  !insertmacro CheckChoice "silent-preserves" 1 0 0
  FileClose $9
SectionEnd
'@
$template = $template.Replace('@@HOOK@@', $hook).Replace('@@ROOT@@', $fixtureLiteral)
[IO.File]::WriteAllText((Join-Path $fixtureRoot 'fixture.nsi'), $template)
try {
  Push-Location $fixtureRoot
  try { & $NsisCompiler /V2 fixture.nsi; if ($LASTEXITCODE -ne 0) { throw 'NSIS harness compilation failed.' } }
  finally { Pop-Location }
  $process = Start-Process -FilePath (Join-Path $fixtureRoot 'fixture.exe') -ArgumentList '/S' -WindowStyle Hidden -PassThru -Wait
  if ($process.ExitCode -ne 0) { throw "Fixture generator failed: $($process.ExitCode)" }
  $process = Start-Process -FilePath (Join-Path $fixtureRoot 'fixture-uninstall.exe') -ArgumentList @('/S', ('_?=' + $fixtureRoot)) -WindowStyle Hidden -PassThru -Wait
  if ($process.ExitCode -ne 0) { throw "Removal harness failed: $($process.ExitCode)" }
  $results = Get-Content -LiteralPath (Join-Path $fixtureRoot 'results.txt')
  if ($results.Count -ne 10) { throw "Expected 10 results; received $($results.Count)." }
  if ($results[0] -ne 'normal=' -or (Test-Path -LiteralPath (Join-Path $fixtureRoot 'normal'))) { throw "Ordinary removal failed: $($results[0])" }
  if ($results[1] -ne 'absent=') { throw "Absent target failed: $($results[1])" }
  if ($results[2] -notlike 'junction=Refusing to follow*') { throw "Junction was not refused: $($results[2])" }
  if ($results[3] -notlike 'ancestor=Refusing to follow*') { throw "Redirected ancestor was not refused: $($results[3])" }
  if ($results[4] -notlike 'locked=Could not remove*') { throw "Locked file failure was not reported: $($results[4])" }
  foreach ($sentinel in @('external\sentinel.txt', 'ancestor-target\sentinel.txt', 'locked\busy.db')) {
    if (-not (Test-Path -LiteralPath (Join-Path $fixtureRoot $sentinel))) { throw "Protected fixture changed: $sentinel" }
  }
  $choices=@('unchecked-preserves=0,0','checked-offers-guarded-removal=1,0','updating-preserves=0,0','passive-preserves=0,0','silent-preserves=0,0')
  for($i=0;$i -lt $choices.Count;$i++) { if($results[$i+5] -ne $choices[$i]) { throw "Data removal consent failed: $($results[$i+5])" } }
  Write-Output "PASS: 10 real NSIS cases; consent, update preservation, redirected targets and locked data. Evidence: $fixtureRoot"
} finally { $lock.Dispose() }

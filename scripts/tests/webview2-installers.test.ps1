param(
  [ValidateSet('Build', 'Run')][string]$Mode = 'Build',
  [string]$ScratchDir,
  [string]$PackageDirectory,
  [string]$ResultFile,
  [string]$NsisCompiler = "$env:LOCALAPPDATA\tauri\NSIS\makensis.exe",
  [string]$WixDirectory = "$env:LOCALAPPDATA\tauri\WixTools314"
)
$ErrorActionPreference = 'Stop'
$fixtureRoot = 'C:\AnnexWebViewFixture'
function Write-Utf8([string]$Path, [string]$Text) {
  [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}
function File-Hash([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

if ($Mode -eq 'Run') {
  # MSI registers fixture products. Never run this branch on a developer's host.
  $hostedRunner = $env:GITHUB_ACTIONS -eq 'true' -and $env:RUNNER_ENVIRONMENT -eq 'github-hosted' -and $env:RUNNER_OS -eq 'Windows'
  if ($env:USERNAME -ne 'WDAGUtilityAccount' -and -not $hostedRunner) { throw 'Run requires a fresh Windows Sandbox WDAGUtilityAccount or disposable GitHub-hosted Windows runner.' }
  $evidenceRoot = Join-Path $fixtureRoot 'evidence'
  if ([IO.File]::Exists($fixtureRoot)) { throw 'The fixture scratch root must be a fresh directory.' }
  if ([IO.Directory]::Exists($fixtureRoot)) {
    $existing = @(Get-ChildItem -LiteralPath $fixtureRoot -Force)
    if ($existing.Count -gt 1 -or ($existing.Count -eq 1 -and ($existing[0].FullName -ne $evidenceRoot -or -not $existing[0].PSIsContainer))) { throw 'Only an empty mapped evidence directory may already exist in the scratch root.' }
    if ([IO.Directory]::Exists($evidenceRoot) -and @(Get-ChildItem -LiteralPath $evidenceRoot -Force).Count -gt 0) { throw 'Evidence directory must be empty.' }
  }
  $package = [IO.Path]::GetFullPath($PackageDirectory)
  $manifest = Get-Content -LiteralPath (Join-Path $package 'manifest.json') -Raw | ConvertFrom-Json
  foreach ($file in $manifest.files) {
    $candidate = [IO.Path]::GetFullPath((Join-Path $package $file.path))
    if (-not $candidate.StartsWith($package.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Package path escaped its root.' }
    if ((File-Hash $candidate) -ne $file.sha256) { throw "Package hash mismatch: $($file.path)" }
  }
  [IO.Directory]::CreateDirectory($fixtureRoot) | Out-Null
  [IO.Directory]::CreateDirectory($evidenceRoot) | Out-Null
  # Windows Installer performs software-restriction checks on the input MSI.
  # Give it local NTFS files, rather than a Sandbox redirected host share.
  $localPackage = Join-Path $fixtureRoot 'packages'
  foreach ($entry in $manifest.installers) {
    $destination = [IO.Path]::GetFullPath((Join-Path $localPackage $entry.path))
    if (-not $destination.StartsWith($localPackage + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Installer path escaped the local package directory.' }
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination)) | Out-Null
    [IO.File]::Copy((Join-Path $package $entry.path),$destination,$false)
    $expected = @($manifest.files | Where-Object path -EQ $entry.path)
    if ($expected.Count -ne 1 -or (File-Hash $destination) -ne $expected[0].sha256) { throw 'Local fixture copy differs from its manifest.' }
  }
  $results = [Collections.Generic.List[object]]::new()
  $installer = New-Object -ComObject WindowsInstaller.Installer
  function Product-State([string]$Code) {
    $installer.GetType().InvokeMember('ProductState', [Reflection.BindingFlags]::GetProperty, $null, $installer, @($Code))
  }
  function Run-Process([string]$Executable, [string]$Arguments, [int]$TimeoutSeconds = 120) {
    $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    $retainedHandle = $process.Handle
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) { throw "Fixture process timed out after ${TimeoutSeconds}s: $Executable" }
    $exitCode = $process.ExitCode
    if ($retainedHandle -eq [IntPtr]::Zero -or $null -eq $exitCode -or $exitCode -isnot [int]) { throw 'Fixture process did not yield a concrete exit code.' }
    return $exitCode
  }
  $cases = @(
    @{name='valid'; state='valid'; behavior='repair'; success=$true; calls=0; variant='normal'},
    @{name='missing'; state='missing'; behavior='repair'; success=$true; calls=1; variant='normal'},
    @{name='stale-registration'; state='missing'; behavior='repair'; success=$true; calls=1; variant='normal'; stale=$true},
    @{name='installer-failure'; state='missing'; behavior='fail'; success=$false; calls=1; variant='normal'},
    @{name='false-success'; state='missing'; behavior='false-success'; success=$false; calls=1; variant='normal'},
    @{name='unexpected-sdk-error'; state='error'; behavior='repair'; success=$false; calls=0; variant='normal'},
    @{name='preview-channel'; state='preview'; behavior='repair'; success=$true; calls=1; variant='normal'},
    @{name='malformed-dll'; state='valid'; behavior='repair'; success=$false; calls=0; variant='malformed'},
    @{name='missing-export'; state='valid'; behavior='repair'; success=$false; calls=0; variant='missing-export'}
  )
  $failure = $null
  try {
    foreach ($format in @('nsis', 'msi')) {
      foreach ($case in $cases) {
        $name = "$format-$($case.name)"
        $sentinel = Join-Path $fixtureRoot $(if ($format -eq 'msi') { 'installed-msi\payload.txt' } else { 'installed-nsis.txt' })
        if (Test-Path -LiteralPath $sentinel) { throw "Unexpected sentinel before $name" }
        Write-Utf8 (Join-Path $fixtureRoot 'state.txt') $case.state
        Write-Utf8 (Join-Path $fixtureRoot 'behavior.txt') $case.behavior
        Write-Utf8 (Join-Path $fixtureRoot 'invocations.txt') ''
        $entry = @($manifest.installers | Where-Object { $_.format -eq $format -and $_.variant -eq $case.variant })
        if ($entry.Count -ne 1) { throw "Missing unique fixture installer: $name" }
        $entry = $entry[0]
        $exitCode = $null; $cleanupExit = $null; $afterCleanup = $null
        $caseTimer = [Diagnostics.Stopwatch]::StartNew()
        try {
          if ($format -eq 'msi') {
            if ((Product-State $entry.productCode) -ne -1) { throw 'Fixture product is already registered; refusing to touch it.' }
            $stale = if ($case.stale) { ' INSTALLED_WEBVIEW2_VERSION="999.0.0.0"' } else { '' }
            # In clean Sandbox, Windows' software-restriction check itself took
            # 120s before the first MSI action. Preserve that policy and allow a
            # bounded 300s engine startup; do not change behavior assertions.
            $exitCode = Run-Process "$env:WINDIR\System32\msiexec.exe" ('/i "' + (Join-Path $localPackage $entry.path) + '" /qn /norestart /l*vx! "' + (Join-Path $evidenceRoot "$name.log") + '"' + $stale) 300
          } else {
            $exitCode = Run-Process (Join-Path $localPackage $entry.path) '/S'
          }
          $calls = ([IO.File]::ReadAllText((Join-Path $fixtureRoot 'invocations.txt'))).Length
          $installed = Test-Path -LiteralPath $sentinel
          $passed = (($exitCode -eq 0) -eq $case.success) -and ($installed -eq $case.success) -and ($calls -eq $case.calls)
          $results.Add([ordered]@{ name=$name; elapsedSeconds=$caseTimer.Elapsed.TotalSeconds; exitCode=$exitCode; expectedSuccess=$case.success; sentinel=$installed; installerCalls=$calls; expectedCalls=$case.calls; stateAfter=[IO.File]::ReadAllText((Join-Path $fixtureRoot 'state.txt')); passed=$passed })
          if (-not $passed) { throw "Installer case failed: $name (exit=$exitCode, sentinel=$installed, calls=$calls)" }
        } finally {
          if ($format -eq 'msi') {
            # Only this package's freshly generated product code can be selected.
            if ((Product-State $entry.productCode) -ne -1) {
              $cleanupExit = Run-Process "$env:WINDIR\System32\msiexec.exe" ('/x ' + $entry.productCode + ' /qn /norestart /l*vx! "' + (Join-Path $evidenceRoot "$name-cleanup.log") + '"') 300
            }
            $afterCleanup = Product-State $entry.productCode
            if ($afterCleanup -ne -1 -or (Test-Path -LiteralPath $sentinel)) { throw "Fixture MSI cleanup failed: $name ($cleanupExit, state=$afterCleanup)" }
          } elseif (Test-Path -LiteralPath $sentinel) {
            Remove-Item -LiteralPath $sentinel
          }
          if ($results.Count -gt 0 -and $results[$results.Count - 1].name -eq $name) {
            $results[$results.Count - 1].cleanupExitCode = $cleanupExit
            $results[$results.Count - 1].productStateAfterCleanup = $afterCleanup
          }
          Write-Utf8 (Join-Path $evidenceRoot 'progress.json') ($results | ConvertTo-Json -Depth 10)
        }
      }
    }
  } catch { $failure = $_.ToString() }
  $report = [ordered]@{ completedAt=[DateTime]::UtcNow.ToString('o'); user=$env:USERNAME; manifestSha256=File-Hash (Join-Path $package 'manifest.json'); passed=($null -eq $failure -and $results.Count -eq 18); tests=$results; error=$failure; boundary='Real NSIS/MSI engines and production hooks/probe logic with a controlled SDK stub and fake runtime installer. No Microsoft runtime or its registry entries are modified.' }
  Write-Utf8 (Join-Path $evidenceRoot 'results.json') ($report | ConvertTo-Json -Depth 12)
  Write-Utf8 (Join-Path $evidenceRoot 'completed.txt') $(if ($report.passed) { 'PASS' } else { 'FAIL' })
  if (-not $report.passed) { throw "Installer regression failed: $failure" }
  Write-Output "PASS: 18 real installer cases. Evidence: $evidenceRoot"
  exit 0
}

if (-not $ScratchDir -or -not [IO.Directory]::Exists($ScratchDir)) { throw 'Build requires an existing ScratchDir.' }
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$output = Join-Path ([IO.Path]::GetFullPath($ScratchDir)) ('webview2-installer-tests-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($output) | Out-Null
$source = Join-Path $output 'source'
[IO.Directory]::CreateDirectory($source) | Out-Null
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual C++ x86 toolchain is required.' }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars32.bat'
$stub = @'
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <objbase.h>
#include <cstring>
static void read(const wchar_t* path, char* out, DWORD size) {
  HANDLE f=CreateFileW(path,GENERIC_READ,FILE_SHARE_READ|FILE_SHARE_WRITE,nullptr,OPEN_EXISTING,FILE_ATTRIBUTE_NORMAL,nullptr);
  if(f==INVALID_HANDLE_VALUE) { out[0]=0; return; }
  DWORD n=0; ReadFile(f,out,size-1,&n,nullptr); out[n]=0; CloseHandle(f);
}
extern "C" HRESULT STDAPICALLTYPE GetAvailableCoreWebView2BrowserVersionString(PCWSTR, PWSTR* version) {
  char state[64]; read(L"C:\\AnnexWebViewFixture\\state.txt",state,sizeof(state)); *version=nullptr;
  if(!strcmp(state,"missing")) return HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND);
  if(!strcmp(state,"error")) return E_FAIL;
  const wchar_t* value=!strcmp(state,"preview")?L"140.0.0.0 dev":L"140.0.3485.81";
  const SIZE_T bytes=(lstrlenW(value)+1)*sizeof(wchar_t); *version=(PWSTR)CoTaskMemAlloc(bytes);
  if(!*version) return E_OUTOFMEMORY; CopyMemory(*version,value,bytes); return S_OK;
}
'@
$fake = @'
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstring>
int WINAPI WinMain(HINSTANCE,HINSTANCE,LPSTR,int) {
  HANDLE log=CreateFileW(L"C:\\AnnexWebViewFixture\\invocations.txt",FILE_APPEND_DATA,FILE_SHARE_READ,nullptr,OPEN_EXISTING,FILE_ATTRIBUTE_NORMAL,nullptr);
  if(log==INVALID_HANDLE_VALUE) return 18; DWORD n; WriteFile(log,"1",1,&n,nullptr); CloseHandle(log);
  HANDLE mode=CreateFileW(L"C:\\AnnexWebViewFixture\\behavior.txt",GENERIC_READ,FILE_SHARE_READ,nullptr,OPEN_EXISTING,FILE_ATTRIBUTE_NORMAL,nullptr);
  if(mode==INVALID_HANDLE_VALUE) return 19; char value[64]={0}; ReadFile(mode,value,63,&n,nullptr); CloseHandle(mode);
  if(!strcmp(value,"fail")) return 17;
  if(!strcmp(value,"false-success")) return 0;
  HANDLE state=CreateFileW(L"C:\\AnnexWebViewFixture\\state.txt",GENERIC_WRITE,FILE_SHARE_READ|FILE_SHARE_WRITE,nullptr,TRUNCATE_EXISTING,FILE_ATTRIBUTE_NORMAL,nullptr);
  if(state==INVALID_HANDLE_VALUE) return 20; WriteFile(state,"valid",5,&n,nullptr); CloseHandle(state); return 0;
}
'@
Write-Utf8 (Join-Path $source 'sdk-stub.cpp') $stub
Write-Utf8 (Join-Path $source 'fake-installer.cpp') $fake
Write-Utf8 (Join-Path $source 'missing-export.cpp') 'extern "C" __declspec(dllexport) int unrelated() { return 0; }'
Copy-Item -LiteralPath (Join-Path $repo 'crates\annex-desktop\windows\webview2-probe.cpp') -Destination $source
Copy-Item -LiteralPath (Join-Path $repo 'crates\annex-desktop\windows\exports.def') -Destination $source
$compile = @"
@echo off
call "$vcvars" >nul
if errorlevel 1 exit /b 1
cd /d "$source"
cl /nologo /MT /O2 /EHsc /LD webview2-probe.cpp sdk-stub.cpp /link /DEF:exports.def /OUT:probe.dll ole32.lib msi.lib
if errorlevel 1 exit /b 1
cl /nologo /MT /O2 /EHsc fake-installer.cpp /link /SUBSYSTEM:WINDOWS /OUT:fake-installer.exe
if errorlevel 1 exit /b 1
cl /nologo /MT /O2 /EHsc /LD missing-export.cpp /link /OUT:missing-export.dll
if errorlevel 1 exit /b 1
"@
Write-Utf8 (Join-Path $source 'compile.cmd') $compile
& $env:ComSpec /d /c (Join-Path $source 'compile.cmd') *> (Join-Path $output 'compile.log')
if ($LASTEXITCODE -ne 0) { throw "Fixture native compile failed. See $output\compile.log" }
Write-Utf8 (Join-Path $source 'malformed.dll') 'Deliberately not a PE image.'
Write-Utf8 (Join-Path $source 'payload.txt') 'This is a disposable installer prerequisite sentinel.'
$entries = @()
foreach ($variant in @('normal','malformed','missing-export')) {
  $variantRoot = Join-Path $output $variant
  $windows = Join-Path $variantRoot 'crates\annex-desktop\windows'
  $nsis = Join-Path $variantRoot 'crates\annex-desktop\nsis'
  $assets = Join-Path $variantRoot 'assets\webview2'
  $fixtureScripts = Join-Path $variantRoot 'scripts'
  foreach ($dir in @($windows,$nsis,$assets,$fixtureScripts)) { [IO.Directory]::CreateDirectory($dir) | Out-Null }
  Copy-Item -LiteralPath (Join-Path $repo 'crates\annex-desktop\nsis\hooks.nsi') -Destination $nsis
  Copy-Item -LiteralPath (Join-Path $repo 'crates\annex-desktop\windows\webview2.nsh') -Destination $windows
  # Only the fixture's compiler-side verification provider is replaced. The
  # runtime hook remains byte-identical; the real bundle verifier authenticates
  # Microsoft bytes, whereas this package embeds this exact controlled EXE.
  $fixtureVerifier = @'
param([Parameter(Mandatory=$true)][string]$InstallerPath)
$ErrorActionPreference = 'Stop'
$digest = [Security.Cryptography.SHA256]::Create()
try { $actual = [BitConverter]::ToString($digest.ComputeHash([IO.File]::ReadAllBytes($InstallerPath))).Replace('-','').ToLowerInvariant() }
finally { $digest.Dispose() }
if ($actual -ne '@@HASH@@') { throw 'Fixture installer hash mismatch.' }
'@
  Write-Utf8 (Join-Path $fixtureScripts 'verify-webview2-bundle.ps1') ($fixtureVerifier.Replace('@@HASH@@', (File-Hash (Join-Path $source 'fake-installer.exe'))))
  $dll = if ($variant -eq 'normal') { 'probe.dll' } elseif ($variant -eq 'malformed') { 'malformed.dll' } else { 'missing-export.dll' }
  Copy-Item -LiteralPath (Join-Path $source $dll) -Destination (Join-Path $assets 'AnnexWebView2Probe.dll')
  $nsi = @'
Unicode true
Name "Annex disposable WebView2 prerequisite fixture"
OutFile "fixture.exe"
RequestExecutionLevel user
SilentInstall silent
!include "LogicLib.nsh"
Var MultiUser.InstallMode
; Match Tauri: hooks are included before bundle constants are defined.
!include "crates\annex-desktop\nsis\hooks.nsi"
Var DeleteAppDataCheckboxState
Var UpdateMode
Var PassiveMode
!define INSTALLWEBVIEW2MODE "offlineInstaller"
!define WEBVIEW2INSTALLERPATH "@@INSTALLER@@"
Section
  !insertmacro NSIS_HOOK_PREINSTALL
  FileOpen $9 "C:\AnnexWebViewFixture\installed-nsis.txt" w
  FileWrite $9 "installed"
  FileClose $9
SectionEnd
'@
  Write-Utf8 (Join-Path $variantRoot 'fixture.nsi') ($nsi.Replace('@@INSTALLER@@', (Join-Path $source 'fake-installer.exe')))
  Push-Location $variantRoot
  try { & $NsisCompiler /V3 fixture.nsi *> compile-nsis.log; if ($LASTEXITCODE -ne 0) { throw "NSIS compile failed: $variantRoot" } }
  finally { Pop-Location }
  $productCode = '{' + [Guid]::NewGuid().ToString().ToUpperInvariant() + '}'
  $upgradeCode = '{' + [Guid]::NewGuid().ToString().ToUpperInvariant() + '}'
  # Compile the unmodified fragment from another working directory, matching
  # Tauri's source-relative binary lookup instead of rewriting it for the test.
  Copy-Item -LiteralPath (Join-Path $repo 'crates\annex-desktop\windows\webview2.wxs') -Destination $windows
  $wxs = @'
<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
  <Product Id="@@PRODUCT@@" Name="Annex disposable WebView2 fixture @@VARIANT@@" Language="1033" Version="1.0.0" Manufacturer="Annex regression fixture" UpgradeCode="@@UPGRADE@@">
    <Package InstallerVersion="500" Compressed="yes" InstallScope="perUser" InstallPrivileges="limited" />
    <MediaTemplate EmbedCab="yes" />
    <Property Id="INSTALLFOLDER" Value="C:\AnnexWebViewFixture\installed-msi" />
    <Directory Id="TARGETDIR" Name="SourceDir"><Directory Id="INSTALLFOLDER" Name="installed-msi"><Component Id="Payload" Guid="@@COMPONENT@@" Win64="yes"><File Id="PayloadFile" Source="@@PAYLOAD@@" KeyPath="yes" /></Component></Directory></Directory>
    <Feature Id="Complete" Level="1"><ComponentRef Id="Payload" /><ComponentGroupRef Id="AnnexWebView2Prerequisite" /></Feature>
    <Binary Id="MicrosoftEdgeWebView2RuntimeInstaller.exe" SourceFile="@@INSTALLER@@" />
    <!-- Exact Tauri offline-installer action and condition; production fragment adds actual detection/verification. -->
    <CustomAction Id="InvokeStandalone" BinaryKey="MicrosoftEdgeWebView2RuntimeInstaller.exe" Execute="deferred" ExeCommand="/silent /install" Return="check" />
    <InstallExecuteSequence><Custom Action="InvokeStandalone" Before="InstallFinalize"><![CDATA[NOT(REMOVE OR INSTALLED_WEBVIEW2_VERSION)]]></Custom></InstallExecuteSequence>
  </Product>
</Wix>
'@
  $wxs = $wxs.Replace('@@PRODUCT@@',$productCode).Replace('@@UPGRADE@@',$upgradeCode).Replace('@@COMPONENT@@',[Guid]::NewGuid().ToString()).Replace('@@VARIANT@@',$variant).Replace('@@PAYLOAD@@',[Security.SecurityElement]::Escape((Join-Path $source 'payload.txt'))).Replace('@@INSTALLER@@',[Security.SecurityElement]::Escape((Join-Path $source 'fake-installer.exe')))
  Write-Utf8 (Join-Path $variantRoot 'fixture.wxs') $wxs
  Push-Location $variantRoot
  try {
    # Match the released x64 MSI; its DLL custom action remains x86, matching
    # the actual Microsoft SDK probe and the NSIS process architecture.
    & (Join-Path $WixDirectory 'candle.exe') -nologo -arch x64 fixture.wxs crates\annex-desktop\windows\webview2.wxs *> compile-wix.log
    if ($LASTEXITCODE -ne 0) { throw "WiX compile failed: $variantRoot" }
    & (Join-Path $WixDirectory 'light.exe') -nologo -sval -out fixture.msi fixture.wixobj webview2.wixobj *>> compile-wix.log
    if ($LASTEXITCODE -ne 0) { throw "WiX link failed: $variantRoot" }
  } finally { Pop-Location }
  $entries += @{format='nsis'; variant=$variant; path="$variant\fixture.exe"}
  $entries += @{format='msi'; variant=$variant; path="$variant\fixture.msi"; productCode=$productCode}
}
Copy-Item -LiteralPath $PSCommandPath -Destination (Join-Path $output 'run-tests.ps1')
$files = @(Get-ChildItem -LiteralPath $output -File -Recurse | ForEach-Object { @{ path=$_.FullName.Substring($output.Length+1); sha256=File-Hash $_.FullName } })
$production = @('crates/annex-desktop/windows/webview2-probe.cpp','crates/annex-desktop/windows/exports.def','crates/annex-desktop/windows/webview2.nsh','crates/annex-desktop/windows/webview2.wxs','crates/annex-desktop/nsis/hooks.nsi') | ForEach-Object { @{path=$_; sha256=File-Hash (Join-Path $repo $_)} }
Write-Utf8 (Join-Path $output 'manifest.json') (@{ createdAt=[DateTime]::UtcNow.ToString('o'); installers=$entries; productionSources=$production; files=$files } | ConvertTo-Json -Depth 10)
if ($ResultFile) { Write-Utf8 ([IO.Path]::GetFullPath($ResultFile)) ($output + [Environment]::NewLine) }
Write-Output "Built disposable fixture package: $output"
Write-Output 'Run only in a fresh Windows Sandbox: powershell -NoProfile -ExecutionPolicy Bypass -File <package>\run-tests.ps1 -Mode Run -PackageDirectory <package>'

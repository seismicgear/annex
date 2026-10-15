param([string]$AssetsDir,[string]$WorkDir)
$ErrorActionPreference='Stop'
$vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$installation=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if(-not $installation){throw 'A native Visual C++ x64 toolchain is required.'}
$developerCommand=Join-Path $installation 'VC\Auxiliary\Build\vcvars64.bat'
$compilerEnvironment=& cmd.exe /d /c ('"'+$developerCommand+'" >nul && set')
if($LASTEXITCODE -ne 0){throw 'Could not initialize the Visual C++ toolchain.'}
foreach($line in $compilerEnvironment){
    if($line -match '^([^=]+)=(.*)$'){[Environment]::SetEnvironmentVariable($matches[1],$matches[2],'Process')}
}
$env:CMAKE_GENERATOR='Ninja'
& (Join-Path $PSScriptRoot 'prepare-windows-runtime.ps1') -AssetsDir $AssetsDir
$probeArguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',(Join-Path $PSScriptRoot 'prepare-webview2-probe.ps1'))
if($AssetsDir){$probeArguments+=@('-AssetsDir',$AssetsDir)}
if($WorkDir){$probeArguments+=@('-WorkDir',$WorkDir)}
& powershell.exe @probeArguments
if($LASTEXITCODE -ne 0){throw 'Could not prepare the WebView2 prerequisite probe.'}
$arguments=@((Join-Path $PSScriptRoot 'prepare-desktop-assets.mjs'),'--target','x86_64-pc-windows-msvc')
if($AssetsDir){$arguments+=@('--assets-dir',$AssetsDir)}
if($WorkDir){$arguments+=@('--work-dir',$WorkDir)}
& node @arguments
exit $LASTEXITCODE

param([string]$AssetsDir, [string]$WorkDir, [string]$SdkArchive, [switch]$VerifyOnly)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# A Windows PowerShell child can inherit PowerShell 7's module search path.
# Use this interpreter's inbox modules for hashing, HTTP, and file inspection.
foreach ($module in @('Microsoft.PowerShell.Utility','Microsoft.PowerShell.Management')) {
    Import-Module (Join-Path $PSHOME ('Modules\'+$module+'\'+$module+'.psd1')) -ErrorAction Stop
}
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'WebView2 prerequisite preparation requires Windows.' }
$repository = Split-Path -Parent $PSScriptRoot
if (-not $AssetsDir) { $AssetsDir = Join-Path $repository 'assets' }
if (-not $WorkDir) { $WorkDir = Join-Path $repository 'target\desktop-assets' }
$destination = [IO.Path]::GetFullPath((Join-Path $AssetsDir 'webview2'))
$receiptPath = Join-Path $destination 'provenance.json'
$sdkVersion = '1.0.3800.47'
$sdkUrl = 'https://api.nuget.org/v3-flatcontainer/microsoft.web.webview2/1.0.3800.47/microsoft.web.webview2.1.0.3800.47.nupkg'
$sdkHash = '56c9f26bdd07916a2d1949fb58a5c7e434dfa1173577dca879206050c4e718db'
$libraryHash = '0513f7e61304321f2d91e1851475be07e091042aa3640597e660608f72521e6c'
$licenseHash = '0af8f1b807512aae39c2ac1aa4d0cae65cabecb6fd554b8439a5162a0d6eca55'
$sourcePaths = @('crates/annex-desktop/windows/webview2-probe.cpp','crates/annex-desktop/windows/exports.def','scripts/prepare-webview2-probe.ps1')
$expectedImports = @('advapi32.dll','kernel32.dll','msi.dll','ole32.dll')
$expectedExports = @('AnnexWebView2MsiDetect','AnnexWebView2MsiVerify','AnnexWebView2Probe')
function Hash([string]$File) { return (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant() }
function SourceInventory {
    return @($sourcePaths | ForEach-Object { @{path=$_;sha256=(Hash (Join-Path $repository $_))} })
}
function Assert-Probe([string]$File) {
    $bytes = [IO.File]::ReadAllBytes($File)
    if ($bytes.Length -lt 256 -or [BitConverter]::ToUInt16($bytes,0) -ne 0x5a4d) { throw 'Invalid probe PE header.' }
    $pe = [BitConverter]::ToInt32($bytes,0x3c)
    if ($pe -lt 64 -or $pe -gt ($bytes.Length-96) -or [BitConverter]::ToUInt32($bytes,$pe) -ne 0x4550 -or [BitConverter]::ToUInt16($bytes,$pe+4) -ne 0x14c -or [BitConverter]::ToUInt16($bytes,$pe+24) -ne 0x10b -or ([BitConverter]::ToUInt16($bytes,$pe+22) -band 0x2000) -eq 0) { throw 'Probe must be an x86 PE32 DLL.' }
    if (([BitConverter]::ToUInt16($bytes,$pe+94) -band 0x140) -ne 0x140) { throw 'Probe must retain ASLR and NX compatibility.' }
    $dependencies = @(& $script:dumpbin /nologo /dependents $File)
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect probe dependencies.' }
    $imports = @($dependencies | ForEach-Object { if ($_ -match '^\s+([A-Za-z0-9_.-]+\.dll)\s*$') { $matches[1].ToLowerInvariant() } } | Sort-Object -Unique)
    if (@(Compare-Object $expectedImports $imports).Count) { throw ('Unexpected probe dependencies: '+($imports -join ', ')) }
    $exportDump = @(& $script:dumpbin /nologo /exports $File)
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect probe exports.' }
    $exports = @($exportDump | ForEach-Object { if ($_ -match '^\s+\d+\s+[0-9A-F]+\s+[0-9A-F]+\s+([A-Za-z0-9_]+)\s*$') { $matches[1] } } | Sort-Object)
    if (@(Compare-Object $expectedExports $exports).Count) { throw 'Unexpected probe exports.' }
    return @{architecture='x86';imports=$imports;exports=$exports;bytes=$bytes.Length;sha256=(Hash $File)}
}

# The installer is an x86 process even when it installs the x64 application.
# Invoke this script in a child PowerShell so this toolchain cannot replace the
# caller's x64 environment used for the speech sidecars and Rust build.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$installation = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $installation) { throw 'Visual Studio C++ x86/x64 build tools are required.' }
$developerCommand = Join-Path $installation 'VC\Auxiliary\Build\vcvars32.bat'
$compilerEnvironment = & cmd.exe /d /c ('"'+$developerCommand+'" >nul && set')
if ($LASTEXITCODE -ne 0) { throw 'Could not initialize the x86 Visual C++ toolchain.' }
foreach ($line in $compilerEnvironment) {
    if ($line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1],$matches[2],'Process') }
}
$compiler = (Get-Command cl.exe -CommandType Application).Source
$script:dumpbin = (Get-Command dumpbin.exe -CommandType Application).Source

if (-not $VerifyOnly) {
    $cache = [IO.Path]::GetFullPath((Join-Path $WorkDir 'webview2-probe'))
    New-Item -ItemType Directory -Path $cache -Force | Out-Null
    if (-not $SdkArchive) { $SdkArchive = Join-Path $cache ('microsoft.web.webview2.'+$sdkVersion+'.nupkg') }
    $SdkArchive = [IO.Path]::GetFullPath($SdkArchive)
    if (-not (Test-Path -LiteralPath $SdkArchive)) {
        $download = Join-Path $cache ('sdk-'+[Guid]::NewGuid().ToString('N')+'.partial')
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $sdkUrl -UseBasicParsing -OutFile $download
        if ((Hash $download) -ne $sdkHash) { throw 'Downloaded WebView2 SDK differs from its pinned digest.' }
        [IO.File]::Move($download,$SdkArchive)
    }
    if ((Hash $SdkArchive) -ne $sdkHash -or (Get-Item -LiteralPath $SdkArchive).Length -ne 8973762) { throw 'WebView2 SDK archive differs from its pinned digest or size.' }
    $build = Join-Path $cache ('build-'+[Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $build | Out-Null
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($SdkArchive)
    try {
        foreach ($entryName in @('build/native/x86/WebView2LoaderStatic.lib','LICENSE.txt')) {
            $entries = @($archive.Entries | Where-Object FullName -EQ $entryName)
            if ($entries.Count -ne 1) { throw "Pinned SDK is missing an unambiguous $entryName" }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entries[0],(Join-Path $build ([IO.Path]::GetFileName($entryName))),$false)
        }
    } finally { $archive.Dispose() }
    $library = Join-Path $build 'WebView2LoaderStatic.lib'
    $license = Join-Path $build 'LICENSE.txt'
    if ((Hash $library) -ne $libraryHash -or (Hash $license) -ne $licenseHash) { throw 'Extracted SDK library or license differs from its pinned content.' }
    $sourceBefore = @(SourceInventory)
    $output = Join-Path $build 'AnnexWebView2Probe.dll'
    $arguments = @('/nologo','/MT','/O2','/W4','/WX','/EHsc','/LD',('/Fo'+(Join-Path $build 'probe.obj')),('/Fe'+$output),(Join-Path $repository $sourcePaths[0]),$library,'ole32.lib','msi.lib','advapi32.lib','/link','/MACHINE:X86',('/DEF:'+(Join-Path $repository $sourcePaths[1])),('/IMPLIB:'+(Join-Path $build 'probe.lib')),'/DYNAMICBASE','/NXCOMPAT','/Brepro')
    & $compiler @arguments
    if ($LASTEXITCODE -ne 0) { throw 'WebView2 probe compilation failed.' }
    $image = Assert-Probe $output
    if (($sourceBefore | ConvertTo-Json -Compress) -ne ((SourceInventory) | ConvertTo-Json -Compress)) { throw 'Probe source changed during compilation.' }
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Copy-Item -LiteralPath $output -Destination (Join-Path $destination 'AnnexWebView2Probe.dll') -Force
    Copy-Item -LiteralPath $license -Destination (Join-Path $destination 'LICENSE.txt') -Force
    $receipt = @{version=1;prepared_utc=[DateTime]::UtcNow.ToString('o');sdk=@{version=$sdkVersion;url=$sdkUrl;sha256=$sdkHash;bytes=8973762;library_path='build/native/x86/WebView2LoaderStatic.lib';library_sha256=$libraryHash;license_sha256=$licenseHash};sources=$sourceBefore;compiler=@{path=$compiler;sha256=(Hash $compiler);version=(Get-Item -LiteralPath $compiler).VersionInfo.FileVersion;arguments=$arguments};probe=$image;files=@(@{path='AnnexWebView2Probe.dll';sha256=$image.sha256;bytes=$image.bytes},@{path='LICENSE.txt';sha256=$licenseHash;bytes=(Get-Item -LiteralPath $license).Length})}
    [IO.File]::WriteAllText($receiptPath,($receipt | ConvertTo-Json -Depth 8),(New-Object Text.UTF8Encoding($false)))
}

if (-not (Test-Path -LiteralPath $receiptPath)) { throw 'WebView2 probe provenance is missing. Run scripts/prepare-webview2-probe.ps1 before packaging.' }
$receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
if ($receipt.version -ne 1 -or $receipt.sdk.version -ne $sdkVersion -or $receipt.sdk.url -ne $sdkUrl -or $receipt.sdk.sha256 -ne $sdkHash -or $receipt.sdk.bytes -ne 8973762 -or $receipt.sdk.library_path -ne 'build/native/x86/WebView2LoaderStatic.lib' -or $receipt.sdk.library_sha256 -ne $libraryHash -or $receipt.sdk.license_sha256 -ne $licenseHash) { throw 'WebView2 SDK provenance differs from its pin.' }
if ($receipt.sources.Count -ne $sourcePaths.Count) { throw 'Incomplete WebView2 probe source inventory.' }
foreach ($relative in $sourcePaths) {
    $entries = @($receipt.sources | Where-Object path -EQ $relative)
    if ($entries.Count -ne 1 -or $entries[0].sha256 -ne (Hash (Join-Path $repository $relative))) { throw "WebView2 probe was built from different source: $relative" }
}
$expectedFiles = @('AnnexWebView2Probe.dll','LICENSE.txt')
if ($receipt.files.Count -ne $expectedFiles.Count) { throw 'Unexpected WebView2 probe inventory.' }
foreach ($filename in $expectedFiles) {
    $entries = @($receipt.files | Where-Object path -EQ $filename)
    $file = Join-Path $destination $filename
    if ($entries.Count -ne 1 -or (Hash $file) -ne $entries[0].sha256 -or (Get-Item -LiteralPath $file).Length -ne $entries[0].bytes) { throw "WebView2 probe inventory changed: $filename" }
}
if ((Hash (Join-Path $destination 'LICENSE.txt')) -ne $licenseHash) { throw 'Microsoft SDK license changed.' }
$actualNames = @(Get-ChildItem -LiteralPath $destination -Force | Select-Object -ExpandProperty Name | Sort-Object)
if (@(Compare-Object @('AnnexWebView2Probe.dll','LICENSE.txt','provenance.json') $actualNames).Count) { throw 'Unexpected files in the WebView2 probe directory.' }
$actualProbe = Assert-Probe (Join-Path $destination 'AnnexWebView2Probe.dll')
if ($receipt.probe.architecture -ne 'x86' -or $receipt.probe.sha256 -ne $actualProbe.sha256 -or $receipt.probe.bytes -ne $actualProbe.bytes -or @(Compare-Object $expectedImports @($receipt.probe.imports)).Count -or @(Compare-Object $expectedExports @($receipt.probe.exports)).Count) { throw 'Probe binary does not match its dependency/export provenance.' }
Write-Host "[webview2-probe] verified x86 static-CRT probe, pinned Microsoft SDK $sdkVersion, source hashes, license, and inbox-only imports"

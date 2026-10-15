param([string]$AssetsDir, [switch]$VerifyOnly)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT -or -not [Environment]::Is64BitProcess) { throw 'Windows x64 runtime preparation requires a native 64-bit Windows process.' }
$repository = Split-Path -Parent $PSScriptRoot
if (-not $AssetsDir) { $AssetsDir = Join-Path $repository 'assets' }
$destination = [IO.Path]::GetFullPath((Join-Path $AssetsDir 'windows-runtime'))
$receiptPath = Join-Path $destination 'provenance.json'

function Assert-MicrosoftRuntime([string]$File) {
    $signature = Get-AuthenticodeSignature -LiteralPath $File
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(?:^|, )O=Microsoft Corporation(?:,|$)') { throw "Runtime does not have a valid Microsoft Authenticode signature: $File" }
    $bytes = [IO.File]::ReadAllBytes($File)
    if ($bytes.Length -lt 64 -or [BitConverter]::ToUInt16($bytes,0) -ne 0x5a4d) { throw "Invalid runtime PE image: $File" }
    $header = [BitConverter]::ToInt32($bytes,0x3c)
    if ($header -lt 64 -or $header -gt ($bytes.Length-6) -or [BitConverter]::ToUInt32($bytes,$header) -ne 0x4550 -or [BitConverter]::ToUInt16($bytes,$header+4) -ne 0x8664) { throw "Runtime is not an x64 PE image: $File" }
    return $signature.SignerCertificate.Thumbprint
}

if (-not (Test-Path -LiteralPath $receiptPath)) {
    if ($VerifyOnly) { throw 'Windows runtime provenance is missing. Run scripts/prepare-windows-runtime.ps1 before packaging.' }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $installation = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $installation) { throw 'A licensed Visual Studio C++ x64 toolchain with its redistributable files is required.' }
    $redistRoot = Join-Path $installation 'VC\Redist\MSVC'
    $versions = @(Get-ChildItem -LiteralPath $redistRoot -Directory | Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' } | Sort-Object { [Version]$_.Name } -Descending)
    $runtime = $null
    foreach ($version in $versions) {
        $candidate = Join-Path $version.FullName 'x64\Microsoft.VC143.CRT'
        if (Test-Path -LiteralPath $candidate) { $runtime = $candidate; break }
    }
    if (-not $runtime) { throw 'Visual Studio 2022 x64 Microsoft.VC143.CRT redistribution directory is missing.' }
    $libraries = @(Get-ChildItem -LiteralPath $runtime -File -Filter '*.dll' | Sort-Object Name)
    foreach ($required in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')) {
        if ($libraries.Name -notcontains $required) { throw "Official runtime is incomplete: $required" }
    }
    $notice = @'
Microsoft Visual C++ 2022 Runtime Libraries (x64)
Copyright Microsoft Corporation. All rights reserved.

These unmodified, Microsoft-authenticated redistributable files are supplied
from the licensed Visual Studio build toolchain's Microsoft.VC143.CRT folder.
They remain subject to the applicable Microsoft Software License Terms and
Visual Studio Distributable Code conditions; the Annex source license does
not replace those terms.

https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files
https://aka.ms/vs/17/redist.txt

Annex uses application-local deployment in its NSIS and MSI installers.
Annex releases are responsible for updating these copies;
the updater signature and release checksums cover the complete installers.
'@
    $files = @()
    foreach ($subdirectory in @('app','piper','whisper')) {
        $directory = Join-Path $destination $subdirectory
        New-Item -ItemType Directory -Path $directory -Force | Out-Null
        foreach ($library in $libraries) {
            $thumbprint = Assert-MicrosoftRuntime $library.FullName
            $target = Join-Path $directory $library.Name
            $hash = (Get-FileHash -LiteralPath $library.FullName -Algorithm SHA256).Hash
            if (Test-Path -LiteralPath $target) {
                if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $hash) { throw "Refusing to mix changed runtime files at $target" }
            } else { Copy-Item -LiteralPath $library.FullName -Destination $target }
            $files += @{ path=($subdirectory+'/'+$library.Name); sha256=$hash.ToLowerInvariant(); bytes=$library.Length; version=$library.VersionInfo.FileVersion; microsoft_signer_thumbprint=$thumbprint }
        }
    }
    $noticePath = Join-Path $destination 'app\MICROSOFT-VC-RUNTIME-NOTICE.txt'
    [IO.File]::WriteAllText($noticePath,$notice,(New-Object Text.UTF8Encoding($false)))
    $files += @{path='app/MICROSOFT-VC-RUNTIME-NOTICE.txt';sha256=(Get-FileHash -LiteralPath $noticePath).Hash.ToLowerInvariant();bytes=(Get-Item -LiteralPath $noticePath).Length}
    $receipt = @{version=1;architecture='x86_64';source='Visual Studio 2022 Microsoft.VC143.CRT';source_directory=$runtime;prepared_utc=[DateTime]::UtcNow.ToString('o');files=$files}
    [IO.File]::WriteAllText($receiptPath,($receipt|ConvertTo-Json -Depth 6),(New-Object Text.UTF8Encoding($false)))
}

$receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
if ($receipt.version -ne 1 -or $receipt.architecture -ne 'x86_64' -or $receipt.source -ne 'Visual Studio 2022 Microsoft.VC143.CRT') { throw 'Unexpected Windows runtime provenance.' }
$expected = @()
foreach ($entry in $receipt.files) {
    if ($entry.path -notmatch '^(app|piper|whisper)/[A-Za-z0-9_.-]+$') { throw 'Unsafe runtime inventory path.' }
    $file = Join-Path $destination $entry.path
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry.sha256 -or (Get-Item -LiteralPath $file).Length -ne $entry.bytes) { throw "Windows runtime inventory changed: $($entry.path)" }
    if ($entry.path -match '\.dll$') {
        $thumbprint = Assert-MicrosoftRuntime $file
        if ($thumbprint -ne $entry.microsoft_signer_thumbprint) { throw "Runtime signer changed: $($entry.path)" }
    }
    $expected += $entry.path
}
foreach ($subdirectory in @('app','piper','whisper')) {
    foreach ($required in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')) {
        if ($expected -notcontains ($subdirectory+'/'+$required)) { throw "Missing required runtime $subdirectory/$required" }
    }
}
$actual = @(Get-ChildItem -LiteralPath $destination -File -Recurse | Where-Object FullName -NE $receiptPath | ForEach-Object { $_.FullName.Substring($destination.Length+1).Replace('\','/') })
if (@(Compare-Object @($expected|Sort-Object) @($actual|Sort-Object)).Count) { throw 'Unexpected files in Windows runtime directory.' }
Write-Host "[windows-runtime] verified $($receipt.files.Count) files from $($receipt.source_directory)"

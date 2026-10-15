param([string]$InstallerPath,[string]$BuildDir,[switch]$RequireMsi)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# NSIS is a 32-bit process and can inherit a PowerShell 7 module search path.
# Load the inbox modules belonging to the actual interpreter, not that path.
Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Utility\Microsoft.PowerShell.Utility.psd1') -ErrorAction Stop

function Read-AuthenticatedInstaller([string]$Path) {
    $file = Get-Item -LiteralPath $Path
    if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'WebView2 installer must be an ordinary file.' }
    $signature = Get-AuthenticodeSignature -LiteralPath $file.FullName
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(?:^|, )O=Microsoft Corporation(?:,|$)') { throw "WebView2 installer is not authentically signed by Microsoft: $Path" }
    if ($file.VersionInfo.OriginalFilename -ne 'MicrosoftEdgeUpdateSetup.exe' -or $file.VersionInfo.ProductName -ne 'Microsoft Edge Update') { throw 'The authenticated payload is not the Microsoft Edge runtime installer.' }
    return [ordered]@{sha256=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant();bytes=$file.Length;version=$file.VersionInfo.FileVersion;microsoft_signer_thumbprint=$signature.SignerCertificate.Thumbprint}
}

if ($InstallerPath) {
    if ($BuildDir -or $RequireMsi) { throw 'InstallerPath cannot be combined with bundle verification.' }
    Read-AuthenticatedInstaller $InstallerPath | ConvertTo-Json -Compress
    exit 0
}
if (-not $BuildDir) { throw 'Supply -InstallerPath for NSIS compile-time verification, or -BuildDir for final bundle verification.' }
$directory = [IO.Path]::GetFullPath($BuildDir)
$template = Get-Content -LiteralPath (Join-Path $directory 'nsis\x64\installer.nsi') -Raw
$match = [regex]::Match($template,'(?m)^!define WEBVIEW2INSTALLERPATH "([^"\r\n]+)"\s*$')
if (-not $match.Success) { throw 'Generated NSIS bundle has no embedded WebView2 installer.' }
$installer = $match.Groups[1].Value
$payload = Read-AuthenticatedInstaller $installer
$msis = @(Get-ChildItem -LiteralPath (Join-Path $directory 'bundle\msi') -Filter '*.msi' -File -ErrorAction SilentlyContinue)
if ($RequireMsi -and $msis.Count -ne 1) { throw 'The release requires exactly one MSI with verified embedded prerequisites.' }
if ($msis.Count -gt 1) { throw 'Ambiguous MSI bundle inventory.' }

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class AnnexMsiBinaryReader {
  [DllImport("msi.dll",CharSet=CharSet.Unicode,ExactSpelling=true)] public static extern uint MsiOpenDatabaseW(string path,IntPtr persistence,out uint database);
  [DllImport("msi.dll",CharSet=CharSet.Unicode,ExactSpelling=true)] public static extern uint MsiDatabaseOpenViewW(uint database,string query,out uint view);
  [DllImport("msi.dll",ExactSpelling=true)] public static extern uint MsiViewExecute(uint view,uint record);
  [DllImport("msi.dll",ExactSpelling=true)] public static extern uint MsiViewFetch(uint view,out uint record);
  [DllImport("msi.dll",ExactSpelling=true)] public static extern uint MsiRecordReadStream(uint record,uint field,byte[] buffer,ref uint size);
  [DllImport("msi.dll",ExactSpelling=true)] public static extern uint MsiCloseHandle(uint handle);
}
'@
function Assert-MsiResult([uint32]$Result) { if ($Result -ne 0) { throw "Cannot read MSI prerequisite payload (Windows Installer error $Result)." } }
function Export-MsiBinary([string]$Msi,[string]$Name,[string]$Destination) {
    [uint32]$database=0; [uint32]$view=0; [uint32]$record=0
    try {
        Assert-MsiResult ([AnnexMsiBinaryReader]::MsiOpenDatabaseW($Msi,[IntPtr]::Zero,[ref]$database))
        # Both callers below use fixed identifiers, never untrusted SQL.
        Assert-MsiResult ([AnnexMsiBinaryReader]::MsiDatabaseOpenViewW($database,"SELECT ``Data`` FROM ``Binary`` WHERE ``Name``='$Name'",[ref]$view))
        Assert-MsiResult ([AnnexMsiBinaryReader]::MsiViewExecute($view,0))
        Assert-MsiResult ([AnnexMsiBinaryReader]::MsiViewFetch($view,[ref]$record))
        $stream=[IO.File]::Open($Destination,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
        try {
            $buffer=New-Object byte[] 65536
            do {
                [uint32]$size=$buffer.Length
                Assert-MsiResult ([AnnexMsiBinaryReader]::MsiRecordReadStream($record,1,$buffer,[ref]$size))
                if ($size) { $stream.Write($buffer,0,$size) }
            } while ($size)
        } finally {$stream.Dispose()}
    } finally {
        foreach ($handle in @($record,$view,$database)) { if ($handle) { [void][AnnexMsiBinaryReader]::MsiCloseHandle($handle) } }
    }
}
$verified = @()
foreach ($msi in $msis) {
    $temporary = Join-Path $directory ('webview2-verify-' + [Guid]::NewGuid().ToString('N') + '.exe')
    try {
        Export-MsiBinary $msi.FullName 'MicrosoftEdgeWebView2RuntimeInstaller.exe' $temporary
        $embedded = Read-AuthenticatedInstaller $temporary
        if ($embedded.sha256 -ne $payload.sha256) { throw 'NSIS and MSI contain different WebView2 runtime installers.' }
        $verified += [ordered]@{file=$msi.Name;sha256=(Get-FileHash -LiteralPath $msi.FullName).Hash.ToLowerInvariant();embedded_installer=$embedded}
    } finally { if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force } }
    $temporary = Join-Path $directory ('webview2-probe-verify-' + [Guid]::NewGuid().ToString('N') + '.dll')
    try {
        Export-MsiBinary $msi.FullName 'AnnexWebView2Probe' $temporary
        $expected = Get-Content -LiteralPath (Join-Path $directory 'webview2\provenance.json') -Raw | ConvertFrom-Json
        $probeHash = (Get-FileHash -LiteralPath $temporary).Hash.ToLowerInvariant()
        if ($probeHash -ne $expected.probe.sha256) { throw 'MSI prerequisite probe differs from its verified build provenance.' }
    } finally { if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force } }
}
[ordered]@{version=1;checked_utc=[DateTime]::UtcNow.ToString('o');nsis_input_microsoft_signature_verified=$true;evergreen_installer=$payload;msi=$verified} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $directory 'windows-webview2-closure.json') -Encoding UTF8
Write-Host '[webview2] authenticated Evergreen payload and embedded MSI prerequisites verified'

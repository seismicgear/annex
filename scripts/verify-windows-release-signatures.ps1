param([Parameter(Mandatory)][string]$ReleaseDirectory, [Parameter(Mandatory)][string]$Output)
$ErrorActionPreference = 'Stop'
if ($env:ANNEX_NATIVE_SIGNING -ne 'true') { throw 'Native publisher verification requires signing mode.' }
$annexThumbprint = $env:WINDOWS_CERTIFICATE_THUMBPRINT
if ($annexThumbprint -notmatch '^[A-Fa-f0-9]{40}$') { throw 'Invalid publisher thumbprint.' }
$annexRoot = (Resolve-Path -LiteralPath $ReleaseDirectory).Path
$annexFiles = @(
    Get-Item -LiteralPath (Join-Path $annexRoot 'annex-desktop.exe')
    Get-ChildItem -LiteralPath (Join-Path $annexRoot 'bundle/nsis') -File -Filter '*.exe'
    Get-ChildItem -LiteralPath (Join-Path $annexRoot 'bundle/msi') -File -Filter '*.msi'
)
if ($annexFiles.Count -ne 3) { throw 'Expected one executable, one NSIS installer and one MSI installer.' }
$annexReceipts = foreach ($annexFile in $annexFiles) {
    $annexSignature = Get-AuthenticodeSignature -LiteralPath $annexFile.FullName
    if ($annexSignature.Status -ne 'Valid' -or $annexSignature.SignerCertificate.Thumbprint -ne $annexThumbprint -or
        $null -eq $annexSignature.TimeStamperCertificate) {
        throw "Publisher trust or timestamp verification failed: $($annexFile.Name)"
    }
    [ordered]@{ file = $annexFile.Name; sha256 = (Get-FileHash -LiteralPath $annexFile.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        status = 'Valid'; publisherThumbprint = $annexSignature.SignerCertificate.Thumbprint
        timestampThumbprint = $annexSignature.TimeStamperCertificate.Thumbprint }
}
@{ platform = 'win32'; verified = $true; files = @($annexReceipts) } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $Output

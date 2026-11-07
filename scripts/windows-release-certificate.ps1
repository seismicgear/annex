param([Parameter(Mandatory)][ValidateSet('Import', 'Cleanup')][string]$Mode)
$ErrorActionPreference = 'Stop'
if ($env:RUNNER_OS -ne 'Windows' -or -not $env:RUNNER_TEMP) {
    throw 'This certificate lifecycle is restricted to the Windows release runner.'
}
$annexTempRoot = (Resolve-Path -LiteralPath $env:RUNNER_TEMP).Path
$annexState = Join-Path $annexTempRoot 'annex-imported-release-certificate.json'
if ($Mode -eq 'Cleanup') {
    if (-not (Test-Path -LiteralPath $annexState)) { return }
    $annexRecord = Get-Content -LiteralPath $annexState -Raw | ConvertFrom-Json
    if ($annexRecord.thumbprint -notmatch '^[A-F0-9]{40}$') { throw 'Invalid imported certificate receipt.' }
    $annexCertPath = 'Cert:\CurrentUser\My\' + $annexRecord.thumbprint
    if (Test-Path -LiteralPath $annexCertPath) { Remove-Item -LiteralPath $annexCertPath -DeleteKey }
    Remove-Item -LiteralPath $annexState
    return
}
if ($env:ANNEX_NATIVE_SIGNING -ne 'true') { throw 'Native signing was not requested.' }
$annexThumbprint = $env:WINDOWS_CERTIFICATE_THUMBPRINT
if ($annexThumbprint -notmatch '^[A-Fa-f0-9]{40}$') { throw 'Invalid Windows publisher thumbprint.' }
$annexThumbprint = $annexThumbprint.ToUpperInvariant()
if (Test-Path -LiteralPath ('Cert:\CurrentUser\My\' + $annexThumbprint)) {
    throw 'Refusing to replace an existing certificate.'
}
if (Test-Path -LiteralPath $annexState) { throw 'Certificate lifecycle receipt already exists.' }
$annexPfx = [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE)
$annexPassword = ConvertTo-SecureString $env:WINDOWS_CERTIFICATE_PASSWORD -AsPlainText -Force
$annexProbe = [System.Security.Cryptography.X509Certificates.X509Certificate2Collection]::new()
$annexPfxPath = Join-Path $annexTempRoot ('annex-signing-' + [guid]::NewGuid() + '.pfx')
try {
    $annexProbe.Import($annexPfx, $env:WINDOWS_CERTIFICATE_PASSWORD,
        ([System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet -bor
         [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::Exportable))
    $annexLeaves = @($annexProbe | Where-Object HasPrivateKey)
    if ($annexLeaves.Count -ne 1 -or $annexLeaves[0].Thumbprint -ne $annexThumbprint) {
        throw 'PFX does not contain exactly the approved publisher private key.'
    }
    $annexLeaf = $annexLeaves[0]
    $annexCodeSigning = @($annexLeaf.EnhancedKeyUsageList | Where-Object ObjectId -eq '1.3.6.1.5.5.7.3.3')
    if ($annexCodeSigning.Count -ne 1 -or $annexLeaf.NotBefore -gt (Get-Date) -or $annexLeaf.NotAfter -le (Get-Date)) {
        throw 'Publisher certificate is not currently valid for code signing.'
    }
    $annexChain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
    try {
        $annexChain.ChainPolicy.ExtraStore.AddRange($annexProbe)
        if (-not $annexChain.Build($annexLeaf)) { throw 'Publisher certificate chain is not trusted.' }
    } finally { $annexChain.Dispose() }
    # Only this leaf/key is imported. Chain certificates are never added to trust.
    $annexSinglePfx = $annexLeaf.Export(
        [System.Security.Cryptography.X509Certificates.X509ContentType]::Pfx,
        $env:WINDOWS_CERTIFICATE_PASSWORD)
    try { [IO.File]::WriteAllBytes($annexPfxPath, $annexSinglePfx) }
    finally { [Array]::Clear($annexSinglePfx, 0, $annexSinglePfx.Length) }
    # Save cleanup ownership before import so a partial failure is cleaned too.
    @{ thumbprint = $annexThumbprint } | ConvertTo-Json | Set-Content -LiteralPath $annexState
    $annexImported = @(Import-PfxCertificate -FilePath $annexPfxPath -CertStoreLocation Cert:\CurrentUser\My -Password $annexPassword)
    if ($annexImported.Count -ne 1 -or $annexImported[0].Thumbprint -ne $annexThumbprint -or -not $annexImported[0].HasPrivateKey) {
        throw 'Imported publisher key did not match the approved certificate.'
    }
    Write-Host 'Approved Windows publisher key imported for this build.'
} finally {
    if (Test-Path -LiteralPath $annexPfxPath) { Remove-Item -LiteralPath $annexPfxPath }
    [Array]::Clear($annexPfx, 0, $annexPfx.Length)
    foreach ($annexItem in $annexProbe) { $annexItem.Dispose() }
}

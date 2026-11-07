$ErrorActionPreference = 'Stop'
$annexTestRoot = Join-Path ([IO.Path]::GetTempPath()) ('annex-certificate-test-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $annexTestRoot | Out-Null
$annexNames = @('RUNNER_OS','RUNNER_TEMP','ANNEX_NATIVE_SIGNING','WINDOWS_CERTIFICATE',
    'WINDOWS_CERTIFICATE_PASSWORD','WINDOWS_CERTIFICATE_THUMBPRINT')
$annexOld = @{}
foreach ($annexName in $annexNames) { $annexOld[$annexName] = [Environment]::GetEnvironmentVariable($annexName) }
$annexRsa = [Security.Cryptography.RSA]::Create(2048)
$annexRequest = [Security.Cryptography.X509Certificates.CertificateRequest]::new(
    'CN=Annex ephemeral untrusted test', $annexRsa, [Security.Cryptography.HashAlgorithmName]::SHA256,
    [Security.Cryptography.RSASignaturePadding]::Pkcs1)
$annexEku = [Security.Cryptography.OidCollection]::new()
[void]$annexEku.Add([Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.3'))
$annexRequest.CertificateExtensions.Add(
    [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($annexEku, $true))
$annexCertificate = $annexRequest.CreateSelfSigned([DateTimeOffset]::UtcNow.AddMinutes(-1), [DateTimeOffset]::UtcNow.AddHours(1))
try {
    $env:RUNNER_OS = 'Windows'
    $env:RUNNER_TEMP = $annexTestRoot
    $env:ANNEX_NATIVE_SIGNING = 'true'
    $env:WINDOWS_CERTIFICATE_PASSWORD = 'disposable-negative-test'
    $env:WINDOWS_CERTIFICATE_THUMBPRINT = $annexCertificate.Thumbprint
    $env:WINDOWS_CERTIFICATE = [Convert]::ToBase64String($annexCertificate.Export(
        [Security.Cryptography.X509Certificates.X509ContentType]::Pfx, $env:WINDOWS_CERTIFICATE_PASSWORD))
    $annexFailure = $null
    try { & (Join-Path $PSScriptRoot '../windows-release-certificate.ps1') -Mode Import }
    catch { $annexFailure = $_.Exception.Message }
    if ($annexFailure -ne 'Publisher certificate chain is not trusted.') {
        throw "Untrusted certificate did not reach the expected refusal: $annexFailure"
    }
    if (Test-Path -LiteralPath ('Cert:\CurrentUser\My\' + $annexCertificate.Thumbprint)) {
        throw 'Untrusted certificate was imported.'
    }
    if (@(Get-ChildItem -LiteralPath $annexTestRoot -Force).Count -ne 0) {
        throw 'Negative certificate validation left private material or an ownership receipt.'
    }
    Write-Host 'PASS: a genuine ephemeral code-signing key with an untrusted chain is refused without importing it.'
} finally {
    $annexCertificate.Dispose()
    $annexRsa.Dispose()
    foreach ($annexName in $annexNames) { [Environment]::SetEnvironmentVariable($annexName, $annexOld[$annexName]) }
    # Nonrecursive: fail rather than erase unexpected files.
    [IO.Directory]::Delete($annexTestRoot)
}

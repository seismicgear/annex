param([switch]$Cleanup)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'This ephemeral trust fixture is restricted to disposable Windows Actions runners.'
}
$directory = Join-Path $env:RUNNER_TEMP 'annex-updater-tls'
$ownership = Join-Path $directory 'owned-root-thumbprint.txt'
if ($Cleanup) {
    if (Test-Path -LiteralPath $ownership) {
        $thumbprint = (Get-Content -LiteralPath $ownership -Raw).Trim()
        if ($thumbprint -notmatch '^[A-F0-9]{40}$') { throw 'Invalid fixture ownership receipt' }
        $certificatePath = "Cert:\LocalMachine\Root\$thumbprint"
        if (Test-Path -LiteralPath $certificatePath) { Remove-Item -LiteralPath $certificatePath }
        Remove-Item -LiteralPath $ownership
    }
    exit 0
}
if (Test-Path -LiteralPath $directory) { throw 'TLS fixture requires a fresh runner directory' }
New-Item -ItemType Directory -Path $directory | Out-Null
$openssl = (Get-Command openssl -ErrorAction SilentlyContinue).Source
if (-not $openssl) { $openssl = 'C:\Program Files\Git\usr\bin\openssl.exe' }
if (-not (Test-Path -LiteralPath $openssl)) { throw 'OpenSSL is required for test TLS' }
& $openssl req -x509 -newkey rsa:2048 -sha256 -days 1 -nodes -keyout "$directory/root.key" -out "$directory/root.pem" -subj '/CN=Annex Native Updater Fixture' -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign,cRLSign'
if ($LASTEXITCODE) { throw 'Test root generation failed' }
& $openssl req -newkey rsa:2048 -sha256 -nodes -keyout "$directory/server.key" -out "$directory/server.csr" -subj '/CN=localhost'
if ($LASTEXITCODE) { throw 'Test TLS key generation failed' }
@'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:localhost,IP:127.0.0.1
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid:always
'@ | Set-Content -LiteralPath "$directory/server.ext" -Encoding utf8NoBOM
& $openssl x509 -req -sha256 -days 1 -in "$directory/server.csr" -CA "$directory/root.pem" -CAkey "$directory/root.key" -CAcreateserial -extfile "$directory/server.ext" -out "$directory/server.pem"
if ($LASTEXITCODE) { throw 'Test TLS certificate generation failed' }
& $openssl verify -CAfile "$directory/root.pem" "$directory/server.pem"
if ($LASTEXITCODE) { throw 'Test TLS chain verification failed' }
$certificate = [Security.Cryptography.X509Certificates.X509Certificate2]::new("$directory/root.pem")
if (Test-Path -LiteralPath "Cert:\LocalMachine\Root\$($certificate.Thumbprint)") { throw 'Refusing to claim existing trust' }
$certificate.Thumbprint | Set-Content -LiteralPath $ownership
# The disposable runner is elevated. CurrentUser root insertion displays an
# interactive Windows trust prompt; machine-store insertion uses the runner's
# existing authority without a dialog. Cleanup removes this exact owned leaf.
Import-Certificate -FilePath "$directory/root.pem" -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
@{ fixtureRootThumbprint=$certificate.Thumbprint; ephemeralRunnerTrust=$true; productionOriginTested=$false; insecureTlsBypass=$false } |
    ConvertTo-Json | Set-Content -LiteralPath 'evidence/tls-boundary.json'

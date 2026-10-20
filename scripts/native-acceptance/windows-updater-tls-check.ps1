$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'TLS qualification requires a disposable Windows runner'
}
$tls = Join-Path $env:RUNNER_TEMP 'annex-updater-tls'
$serverScript = Join-Path $env:RUNNER_TEMP 'annex-updater-health.py'
@'
import http.server, pathlib, ssl, sys
root = pathlib.Path(sys.argv[1])
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/health":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", "5")
        self.end_headers()
        self.wfile.write(b"ready")
server = http.server.ThreadingHTTPServer(("127.0.0.1", 34567), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.minimum_version = ssl.TLSVersion.TLSv1_2
context.load_cert_chain(root / "server.pem", root / "server.key")
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
'@ | Set-Content -LiteralPath $serverScript -Encoding utf8NoBOM
$server = Start-Process -FilePath (Get-Command python).Source -ArgumentList @(('"' + $serverScript + '"'),('"' + $tls + '"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput "$PWD/evidence/health-server.log" -RedirectStandardError "$PWD/evidence/health-server-error.log"
try {
    # Public certificates only. Private fixture keys never enter artifacts.
    Copy-Item -LiteralPath "$tls/root.pem","$tls/server.pem" -Destination evidence
    $certificate = [Security.Cryptography.X509Certificates.X509Certificate2]::new("$tls/server.pem")
    $chain = [Security.Cryptography.X509Certificates.X509Chain]::new()
    $trusted = $chain.Build($certificate)
    @{ trusted=$trusted; status=@($chain.ChainStatus | ForEach-Object { @{ status=$_.Status.ToString(); info=$_.StatusInformation } }); subject=$certificate.Subject; issuer=$certificate.Issuer } |
        ConvertTo-Json -Depth 5 | Set-Content evidence/windows-chain.json
    $ready = $false
    for ($attempt = 0; $attempt -lt 6; $attempt++) {
        if ($server.HasExited) { throw 'TLS health server exited' }
        try {
            $response = Invoke-WebRequest 'https://localhost:34567/health' -NoProxy -UseBasicParsing -TimeoutSec 10
            if ($response.StatusCode -ne 200 -or $response.Content -ne 'ready') { throw 'Unexpected health response' }
            $ready = $true
            break
        } catch {
            $record = @{ attempt=$attempt; message=$_.Exception.Message; inner=$_.Exception.InnerException.Message }
            $record | ConvertTo-Json -Compress | Tee-Object -FilePath evidence/tls-errors.jsonl -Append
        }
        Start-Sleep -Seconds 1
    }
    if (-not $ready) { throw 'Actual Windows HTTPS verification failed' }
    @{ passed=$true; osTrustUsed=$true; certificateVerificationBypassed=$false; productionOriginTested=$false } |
        ConvertTo-Json | Set-Content evidence/result.json
} finally {
    if (-not $server.HasExited) { Stop-Process -Id $server.Id -ErrorAction SilentlyContinue }
}

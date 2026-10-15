#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Deploy an Annex server instance.

.DESCRIPTION
    Sets up and runs an Annex server from source or Docker.
    Handles Rust build, database initialization, ZK key verification,
    and server startup with sane defaults.

.PARAMETER Mode
    Deployment mode: "docker" (default) or "source".

.PARAMETER BindAddress
    Bind address. Default: 0.0.0.0

.PARAMETER Port
    Bind port. Default: 3000

.PARAMETER DataDir
    Directory for persistent data (database, keys). Default: ./data

.PARAMETER ServerLabel
    Display name for this server instance. Default: "Annex Server"

.PARAMETER ServerSlug
    URL-safe identifier for this server. Default: "default"

.PARAMETER PublicUrl
    Public URL for federation. Default: auto-detected from incoming requests

.PARAMETER SigningKey
    Ed25519 signing key (64-char hex). If omitted, loads or persists a local key.

.PARAMETER LogLevel
    Log level: trace, debug, info, warn, error. Default: info

.PARAMETER LogJson
    Output structured JSON logs. Default for docker mode.

.PARAMETER SkipBuild
    Skip cargo build (use existing binary).

.PARAMETER BinaryPath
    Explicit existing server binary when using -SkipBuild.

.PARAMETER Check
    Validate the source installation with the real server --check entry point, then exit.

.PARAMETER WebRtcUrl
    Built-in WebRTC signaling URL (optional). LiveKitUrl is a legacy parameter alias.

.PARAMETER WebRtcApiKey
    WebRTC API key (optional). LiveKitApiKey remains an alias.

.PARAMETER WebRtcApiSecret
    WebRTC API secret (optional). LiveKitApiSecret remains an alias.

.EXAMPLE
    # Docker (easiest)
    ./deploy.ps1

.EXAMPLE
    # From source
    ./deploy.ps1 -Mode source -BindAddress 0.0.0.0 -Port 3000

.EXAMPLE
    # Production with signing key and federation
    ./deploy.ps1 -Mode source -PublicUrl https://annex.example.com -SigningKey <hex>
#>

param(
    [ValidateSet("docker", "source")]
    [string]$Mode = "docker",

    [string]$BindAddress = "0.0.0.0",
    [int]$Port = 3000,
    [string]$DataDir = "./data",
    [string]$ServerLabel = "Annex Server",
    [string]$ServerSlug = "default",
    [string]$PublicUrl = "",
    [string]$SigningKey = "",
    [string]$LogLevel = "info",
    [switch]$LogJson,
    [switch]$SkipBuild,
    [string]$BinaryPath = "",
    [switch]$Check,
    [Alias('LiveKitUrl')][string]$WebRtcUrl = "",
    [Alias('LiveKitApiKey')][string]$WebRtcApiKey = "",
    [Alias('LiveKitApiSecret')][string]$WebRtcApiSecret = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
if (-not (Test-Path variable:IsWindows)) { $IsWindows = $true }
if ($Check -and $Mode -ne 'source') { throw '-Check requires -Mode source.' }
if ($BinaryPath -and -not $SkipBuild) { throw '-BinaryPath requires -SkipBuild.' }

# ── Helpers ──

function Write-Step { param([string]$msg) Write-Host "`n:: $msg" -ForegroundColor Cyan }
function Write-Ok   { param([string]$msg) Write-Host "   OK: $msg" -ForegroundColor Green }
function Write-Warn { param([string]$msg) Write-Host "   WARN: $msg" -ForegroundColor Yellow }
function Write-Fail { param([string]$msg) Write-Host "   FAIL: $msg" -ForegroundColor Red; exit 1 }

function Test-Command { param([string]$cmd) return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }
function ConvertTo-TomlString { param([string]$Value) ConvertTo-Json -InputObject $Value -Compress }

# ── Resolve defaults ──

# PS 5.1-compatible directory resolution (no ?. operator)
$resolvedDir = Resolve-Path -Path $DataDir -ErrorAction SilentlyContinue
if ($resolvedDir) {
    $DataDir = $resolvedDir.Path
} else {
    $DataDir = (New-Item -ItemType Directory -Path $DataDir -Force).FullName
}

$DbPath = Join-Path $DataDir "annex.db"
$ProjectRoot = $PSScriptRoot

Write-Host ""
Write-Host "======================================" -ForegroundColor White
Write-Host "         Annex Server Deploy          " -ForegroundColor White
Write-Host "======================================" -ForegroundColor White
Write-Host ""
Write-Host "  Mode:       $Mode"
Write-Host "  Bind:       ${BindAddress}:${Port}"
Write-Host "  Data dir:   $DataDir"
Write-Host "  Public URL: $PublicUrl"
Write-Host "  Log level:  $LogLevel"
Write-Host ""

# ── Docker mode ──

if ($Mode -eq "docker") {
    Write-Step "Checking Docker"
    if (-not (Test-Command "docker")) { Write-Fail "Docker not found. Install from https://docker.com" }

    # Detect compose: prefer v2 plugin, fall back to standalone v1
    $useComposePlugin = $false
    try {
        $null = docker compose version 2>&1
        if ($LASTEXITCODE -eq 0) { $useComposePlugin = $true }
    } catch {}

    if (-not $useComposePlugin -and -not (Test-Command "docker-compose")) {
        Write-Fail "Neither 'docker compose' (v2) nor 'docker-compose' (v1) found"
    }
    Write-Ok "Docker found"

    Write-Step "Building and starting containers"
    $env:ANNEX_HOST = $BindAddress
    $env:ANNEX_PORT = $Port
    $env:ANNEX_LOG_LEVEL = $LogLevel

    $composeFile = Join-Path $ProjectRoot "docker-compose.yml"
    if ($useComposePlugin) {
        docker compose -f $composeFile up -d --build
    } else {
        docker-compose -f $composeFile up -d --build
    }
    if ($LASTEXITCODE -ne 0) { Write-Fail "Docker Compose failed" }

    $composeLabel = if ($useComposePlugin) { "docker compose" } else { "docker-compose" }
    Write-Ok "Containers started"
    Write-Step "Annex is running at http://localhost:$Port"
    Write-Host ""
    Write-Host "  Logs:    $composeLabel logs -f annex"
    Write-Host "  Stop:    $composeLabel down"
    Write-Host "  Restart: $composeLabel restart annex"
    Write-Host ""

    # Auto-open browser on the host (the server inside Docker cannot do this).
    $openEnv = if ($env:ANNEX_OPEN_BROWSER) { $env:ANNEX_OPEN_BROWSER.ToLower() } else { "true" }
    if ($openEnv -notin @("false", "0", "no")) {
        Start-Process "http://localhost:$Port"
    }

    exit 0
}

# ── Source mode ──

Write-Step "Checking prerequisites"

# Rust
if (-not (Test-Command "cargo")) { Write-Fail "Rust not found. Install from https://rustup.rs" }
$rustVersion = (rustc --version) -replace 'rustc (\d+\.\d+).*', '$1'
Write-Ok "Rust $rustVersion"

# Install only the repository's verified, pinned proof artifacts. A deployment
# must not silently create an unrelated development ceremony when files are absent.
if (-not (Test-Command 'node')) { Write-Fail 'Node.js is required to verify the pinned proof artifacts.' }
$vkeyPath = Join-Path $ProjectRoot "zk" "keys" "membership_vkey.json"
Write-Step 'Verifying and installing pinned ZK artifacts'
node (Join-Path $ProjectRoot 'zk/scripts/verify-artifacts.js') --all --profile production
if ($LASTEXITCODE -ne 0) { Write-Fail 'Pinned proof artifact verification failed.' }
node (Join-Path $ProjectRoot 'zk/scripts/install-ceremony.js')
if ($LASTEXITCODE -ne 0) { Write-Fail 'Pinned proof artifact installation failed.' }
Write-Ok 'All pinned proof artifacts verified and installed'

# Piper TTS -- bootstrap if missing
$piperBinary = Join-Path $ProjectRoot "assets" "piper" "piper"
if ($IsWindows -or ($env:OS -eq "Windows_NT")) { $piperBinary += ".exe" }
$voiceModel = Join-Path $ProjectRoot "assets" "voices" "en_US-lessac-medium.onnx"

if (-not (Test-Path $piperBinary) -or -not (Test-Path $voiceModel)) {
    Write-Step "Bootstrapping Piper TTS voice model"
    $setupScript = Join-Path $ProjectRoot "scripts" "setup-piper.ps1"
    if (Test-Path $setupScript) {
        & $setupScript
        if ($LASTEXITCODE -ne 0) { Write-Warn "Piper setup failed (voice features will be unavailable)" }
    } else {
        Write-Warn "scripts/setup-piper.ps1 not found -- run it manually for voice features"
    }
} else {
    Write-Ok "Piper TTS binary and voice model present"
}

if ($IsWindows) {
    Write-Step 'Verifying native speech recognition'
    & (Join-Path $ProjectRoot 'scripts/setup-stt.ps1')
    if ($LASTEXITCODE -ne 0) { Write-Fail 'Speech recognition setup failed.' }
}

# ── Build ──

# On Windows cargo produces .exe; on Linux/macOS no extension
$exeSuffix = if ($IsWindows -or ($env:OS -eq "Windows_NT")) { ".exe" } else { "" }
$binaryName = "annex-server$exeSuffix"

if ($SkipBuild) {
    Write-Step "Skipping build (-SkipBuild)"
    $binary = if ($BinaryPath) { (Resolve-Path -LiteralPath $BinaryPath).Path } else { Join-Path $ProjectRoot "target" "release" $binaryName }
    if (-not (Test-Path $binary)) { Write-Fail "Binary not found at $binary. Remove -SkipBuild to build." }
} else {
    Write-Step "Building annex-server (release)"
    Push-Location $ProjectRoot
    try {
        cargo build --locked --release --bin annex-server
        if ($LASTEXITCODE -ne 0) { Write-Fail "Build failed" }
    } finally {
        Pop-Location
    }
    $binary = Join-Path $ProjectRoot "target" "release" $binaryName
    Write-Ok "Built $binary"
}

# The real startup path runs migrations and seeds its complete current policy.
# Running the server as a supposed migration failure used to hang indefinitely
# before configuration/key setup. No external SQLite CLI or partial policy is needed.
$clientDir = Join-Path $ProjectRoot 'client/dist'
if (-not $SkipBuild) {
    Write-Step 'Building the browser client'
    $npmCommand = if ($IsWindows) { 'npm.cmd' } else { 'npm' }
    if (-not (Test-Command $npmCommand)) { Write-Fail 'npm is required to build the client.' }
    Push-Location (Join-Path $ProjectRoot 'client')
    try {
        & $npmCommand ci
        if ($LASTEXITCODE -ne 0) { Write-Fail 'Client dependency installation failed.' }
        & $npmCommand run build
        if ($LASTEXITCODE -ne 0) { Write-Fail 'Client build failed.' }
    } finally { Pop-Location }
}
if (-not (Test-Path -LiteralPath (Join-Path $clientDir 'index.html'))) {
    Write-Fail 'Built client is missing; remove -SkipBuild to build it.'
}

# ── Signing key persistence ──

$keyFile = Join-Path $DataDir "signing.key"

if (-not $SigningKey) {
    if (Test-Path $keyFile) {
        $SigningKey = (Get-Content -Path $keyFile -Raw).Trim()
        Write-Ok "Loaded signing key from $keyFile"
    } elseif (Test-Command "openssl") {
        $SigningKey = (openssl rand -hex 32).Trim()
        Set-Content -Path $keyFile -Value $SigningKey -NoNewline -Encoding ASCII
        Write-Ok "Generated and persisted signing key at $keyFile"
    } else {
        # Generate using .NET as fallback
        $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
        $bytes = New-Object byte[] 32
        $rng.GetBytes($bytes)
        $SigningKey = -join ($bytes | ForEach-Object { $_.ToString("x2") })
        Set-Content -Path $keyFile -Value $SigningKey -NoNewline -Encoding ASCII
        Write-Ok "Generated and persisted signing key at $keyFile"
    }
}

# ── Write runtime config ──

# Convert backslash paths to forward slashes for TOML compatibility (Windows)
$tomlDbPath = $DbPath -replace '\\', '/'

$configPath = Join-Path $DataDir "config.toml"
$configContent = @"
# Generated by deploy.ps1 on $(Get-Date -Format "yyyy-MM-dd HH:mm:ss")

[server]
host = $(ConvertTo-TomlString $BindAddress)
port = $Port
public_url = $(ConvertTo-TomlString $PublicUrl)
server_slug = $(ConvertTo-TomlString $ServerSlug)

[database]
path = $(ConvertTo-TomlString $tomlDbPath)
busy_timeout_ms = 5000
pool_max_size = 8

[logging]
level = $(ConvertTo-TomlString $LogLevel)
json = $(if ($LogJson) { "true" } else { "false" })
"@

if ($WebRtcUrl) {
    $configContent += @"

[webrtc]
url = $(ConvertTo-TomlString $WebRtcUrl)
api_key = $(ConvertTo-TomlString $WebRtcApiKey)
api_secret = $(ConvertTo-TomlString $WebRtcApiSecret)
"@
}

Set-Content -Path $configPath -Value $configContent -Encoding UTF8
Write-Ok "Config written to $configPath"

# ── Start server ──

if ($Check) { Write-Step 'Validating Annex installation' }
else { Write-Step 'Starting Annex server' }

$clientDir = Join-Path $ProjectRoot "client" "dist"

$envVars = @{
    ANNEX_CONFIG_PATH  = $configPath
    ANNEX_ZK_KEY_PATH  = $vkeyPath
    ANNEX_DB_PATH      = $DbPath
    ANNEX_HOST         = $BindAddress
    ANNEX_PORT         = $Port
    ANNEX_LOG_LEVEL    = $LogLevel
    ANNEX_PUBLIC_URL   = $PublicUrl
    ANNEX_CLIENT_DIR   = $clientDir
    ANNEX_UPLOAD_DIR   = (Join-Path $DataDir 'uploads')
    ANNEX_SERVER_SLUG  = $ServerSlug
    ANNEX_SERVER_LABEL = $ServerLabel
    ANNEX_EMBEDDING_MODEL_DIR = (Join-Path $ProjectRoot 'assets/embedding')
}

if ($SigningKey) { $envVars["ANNEX_SIGNING_KEY"] = $SigningKey }
if ($LogJson)    { $envVars["ANNEX_LOG_JSON"] = "true" }

# TTS/STT paths (if assets are present)
$piperPath = Join-Path $ProjectRoot "assets" "piper" "piper"
if ($IsWindows -or ($env:OS -eq "Windows_NT")) { $piperPath += ".exe" }
$voicesDir = Join-Path $ProjectRoot "assets" "voices"
if (Test-Path $piperPath) { $envVars["ANNEX_TTS_BINARY_PATH"] = $piperPath }
if (Test-Path $voicesDir) { $envVars["ANNEX_TTS_VOICES_DIR"] = $voicesDir }
$sttBinary = Join-Path $ProjectRoot "assets" "whisper" "whisper-cli$exeSuffix"
$sttModel = Join-Path $ProjectRoot "assets" "models" "ggml-base.en.bin"
if (Test-Path $sttBinary) { $envVars["ANNEX_STT_BINARY_PATH"] = $sttBinary }
if (Test-Path $sttModel) { $envVars["ANNEX_STT_MODEL_PATH"] = $sttModel }

# Auto-open browser for source deployments (unless explicitly suppressed)
if (-not $env:ANNEX_OPEN_BROWSER) { $envVars["ANNEX_OPEN_BROWSER"] = "true" }
else { $envVars["ANNEX_OPEN_BROWSER"] = $env:ANNEX_OPEN_BROWSER }

foreach ($kv in $envVars.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($kv.Key, $kv.Value, "Process")
}

Write-Host ""
if ($Check) { Write-Host '  Checking configuration, database, proof keys and runtime assets' -ForegroundColor Green }
else { Write-Host "  Server starting at http://${BindAddress}:${Port}" -ForegroundColor Green }
Write-Host "  Public URL: $PublicUrl" -ForegroundColor Green
Write-Host "  Data: $DataDir" -ForegroundColor Green
Write-Host "  Logs: $LogLevel" -ForegroundColor Green
Write-Host ""
if (-not $Check) { Write-Host "  Press Ctrl+C to stop." -ForegroundColor DarkGray }
Write-Host ""

Push-Location $ProjectRoot
try {
    if ($Check) { & $binary $configPath --check }
    else { & $binary $configPath }
    if ($LASTEXITCODE -ne 0) { Write-Fail "Annex server exited with code $LASTEXITCODE." }
} finally { Pop-Location }

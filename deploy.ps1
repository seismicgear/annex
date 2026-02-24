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
    Ed25519 signing key (64-char hex). If omitted, generates ephemeral key.

.PARAMETER LogLevel
    Log level: trace, debug, info, warn, error. Default: info

.PARAMETER LogJson
    Output structured JSON logs. Default for docker mode.

.PARAMETER SkipBuild
    Skip cargo build (use existing binary).

.PARAMETER LiveKitUrl
    LiveKit server URL (optional, for voice features).

.PARAMETER LiveKitApiKey
    LiveKit API key (optional).

.PARAMETER LiveKitApiSecret
    LiveKit API secret (optional).

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
    [string]$LiveKitUrl = "",
    [string]$LiveKitApiKey = "",
    [string]$LiveKitApiSecret = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# Default server policy matching ServerPolicy::default() in Rust
$DefaultPolicy = '{"agent_min_alignment_score":0.8,"agent_required_capabilities":[],"federation_enabled":true,"default_retention_days":30,"voice_enabled":true,"max_members":1000}'

# ── Helpers ──

function Write-Step { param([string]$msg) Write-Host "`n:: $msg" -ForegroundColor Cyan }
function Write-Ok   { param([string]$msg) Write-Host "   OK: $msg" -ForegroundColor Green }
function Write-Warn { param([string]$msg) Write-Host "   WARN: $msg" -ForegroundColor Yellow }
function Write-Fail { param([string]$msg) Write-Host "   FAIL: $msg" -ForegroundColor Red; exit 1 }

function Test-Command { param([string]$cmd) return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

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
    exit 0
}

# ── Source mode ──

Write-Step "Checking prerequisites"

# Rust
if (-not (Test-Command "cargo")) { Write-Fail "Rust not found. Install from https://rustup.rs" }
$rustVersion = (rustc --version) -replace 'rustc (\d+\.\d+).*', '$1'
Write-Ok "Rust $rustVersion"

# SQLite CLI (optional, for seeding)
$hasSqlite = Test-Command "sqlite3"
if ($hasSqlite) { Write-Ok "sqlite3 found" }
else { Write-Warn "sqlite3 not found (will guide manual seeding)" }

# ZK verification keys -- bootstrap if missing
$vkeyPath = Join-Path $ProjectRoot "zk" "keys" "membership_vkey.json"
if (-not (Test-Path $vkeyPath)) {
    Write-Step "Bootstrapping ZK circuits and keys"
    if (-not (Test-Command "npm")) { Write-Fail "Node.js (npm) is required to build ZK circuits. Install from https://nodejs.org" }

    Push-Location (Join-Path $ProjectRoot "zk")
    try {
        npm ci
        if ($LASTEXITCODE -ne 0) { Write-Fail "npm ci failed in zk/" }
        node scripts/build-circuits.js
        if ($LASTEXITCODE -ne 0) { Write-Fail "ZK circuit compilation failed" }
        node scripts/setup-groth16.js
        if ($LASTEXITCODE -ne 0) { Write-Fail "ZK Groth16 setup failed" }
    } finally {
        Pop-Location
    }
}
Write-Ok "ZK verification keys verified"

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

# ── Build ──

# On Windows cargo produces .exe; on Linux/macOS no extension
$exeSuffix = if ($IsWindows -or ($env:OS -eq "Windows_NT")) { ".exe" } else { "" }
$binaryName = "annex-server$exeSuffix"

if ($SkipBuild) {
    Write-Step "Skipping build (-SkipBuild)"
    $binary = Join-Path $ProjectRoot "target" "release" $binaryName
    if (-not (Test-Path $binary)) { Write-Fail "Binary not found at $binary. Remove -SkipBuild to build." }
} else {
    Write-Step "Building annex-server (release)"
    Push-Location $ProjectRoot
    try {
        cargo build --release --bin annex-server
        if ($LASTEXITCODE -ne 0) { Write-Fail "Build failed" }
    } finally {
        Pop-Location
    }
    $binary = Join-Path $ProjectRoot "target" "release" $binaryName
    Write-Ok "Built $binary"
}

# ── Initialize database ──

Write-Step "Initializing database at $DbPath"

# Migrations run automatically on startup, but we need the servers row.
# Do a quick startup check: if the DB exists and has a servers row, skip seeding.
$needsSeed = $true
if ((Test-Path $DbPath) -and $hasSqlite) {
    try {
        $count = sqlite3 $DbPath "SELECT COUNT(*) FROM servers;" 2>$null
        if ($count -and [int]$count -gt 0) {
            Write-Ok "Database already seeded ($count server[s])"
            $needsSeed = $false
        }
    } catch {
        # Table might not exist yet -- that is fine, we need to seed
    }
}

if ($needsSeed) {
    if (-not (Test-Path $DbPath)) {
        Write-Host "   Database will be created on first startup"
    }

    # Run the server briefly to trigger migrations, then seed.
    # The server will exit with NoServerConfigured -- that is expected.
    Write-Host "   Running migrations..."
    $env:ANNEX_DB_PATH = $DbPath
    $env:ANNEX_HOST = "127.0.0.1"
    $env:ANNEX_PORT = "0"
    $env:ANNEX_ZK_KEY_PATH = $vkeyPath
    $env:ANNEX_LOG_LEVEL = "warn"

    # Use try/catch because $ErrorActionPreference=Stop would terminate
    # the script when the server writes to stderr during its expected failure.
    $savedEAP = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        & $binary 2>$null
    } catch {
        # Expected -- server exits with error because no server row exists
    } finally {
        $ErrorActionPreference = $savedEAP
    }

    if ($hasSqlite) {
        # Escape single quotes for SQL (double them)
        $safeSlug = $ServerSlug -replace "'", "''"
        $safeLabel = $ServerLabel -replace "'", "''"
        sqlite3 $DbPath "INSERT OR IGNORE INTO servers (slug, label, policy_json) VALUES ('$safeSlug', '$safeLabel', '$DefaultPolicy');"
        if ($LASTEXITCODE -ne 0) { Write-Fail "Failed to seed server row" }
        # Fix any previously seeded rows with empty policy_json
        sqlite3 $DbPath "UPDATE servers SET policy_json = '$DefaultPolicy' WHERE policy_json = '{}';" 2>$null
        Write-Ok "Database seeded: slug='$ServerSlug', label='$ServerLabel'"
    } else {
        Write-Warn "Cannot seed database without sqlite3 CLI."
        Write-Host "   Run this manually:"
        Write-Host "   sqlite3 `"$DbPath`" `"INSERT INTO servers (slug, label, policy_json) VALUES ('<slug>', '<label>', '$DefaultPolicy');`""
        Write-Host ""
        Write-Host "   Or install sqlite3:"
        Write-Host "     Debian/Ubuntu: sudo apt install sqlite3"
        Write-Host "     macOS:         brew install sqlite"
        Write-Host "     Windows:       winget install SQLite.SQLite"
    }
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
host = "$BindAddress"
port = $Port
public_url = "$PublicUrl"

[database]
path = "$tomlDbPath"
busy_timeout_ms = 5000
pool_max_size = 8

[logging]
level = "$LogLevel"
json = $(if ($LogJson) { "true" } else { "false" })
"@

if ($LiveKitUrl) {
    $configContent += @"

[livekit]
url = "$LiveKitUrl"
api_key = "$LiveKitApiKey"
api_secret = "$LiveKitApiSecret"
"@
}

Set-Content -Path $configPath -Value $configContent -Encoding UTF8
Write-Ok "Config written to $configPath"

# ── Start server ──

Write-Step "Starting Annex server"

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
}

if ($SigningKey) { $envVars["ANNEX_SIGNING_KEY"] = $SigningKey }
if ($LogJson)    { $envVars["ANNEX_LOG_JSON"] = "true" }

# TTS/STT paths (if assets are present)
$piperPath = Join-Path $ProjectRoot "assets" "piper" "piper"
$voicesDir = Join-Path $ProjectRoot "assets" "voices"
if (Test-Path $piperPath) { $envVars["ANNEX_TTS_BINARY_PATH"] = $piperPath }
if (Test-Path $voicesDir) { $envVars["ANNEX_TTS_VOICES_DIR"] = $voicesDir }

foreach ($kv in $envVars.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($kv.Key, $kv.Value, "Process")
}

Write-Host ""
Write-Host "  Server starting at http://${BindAddress}:${Port}" -ForegroundColor Green
Write-Host "  Public URL: $PublicUrl" -ForegroundColor Green
Write-Host "  Data: $DataDir" -ForegroundColor Green
Write-Host "  Logs: $LogLevel" -ForegroundColor Green
Write-Host ""
Write-Host "  Press Ctrl+C to stop." -ForegroundColor DarkGray
Write-Host ""

& $binary

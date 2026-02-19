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

.PARAMETER Host
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
    Public URL for federation. Default: http://localhost:<Port>

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
    ./deploy.ps1 -Mode source -Host 0.0.0.0 -Port 3000

.EXAMPLE
    # Production with signing key and federation
    ./deploy.ps1 -Mode source -PublicUrl https://annex.example.com -SigningKey <hex>
#>

param(
    [ValidateSet("docker", "source")]
    [string]$Mode = "docker",

    [string]$Host = "0.0.0.0",
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

# ── Helpers ──

function Write-Step { param([string]$msg) Write-Host "`n:: $msg" -ForegroundColor Cyan }
function Write-Ok   { param([string]$msg) Write-Host "   OK: $msg" -ForegroundColor Green }
function Write-Warn { param([string]$msg) Write-Host "   WARN: $msg" -ForegroundColor Yellow }
function Write-Fail { param([string]$msg) Write-Host "   FAIL: $msg" -ForegroundColor Red; exit 1 }

function Test-Command { param([string]$cmd) return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

# ── Resolve defaults ──

if (-not $PublicUrl) { $PublicUrl = "http://localhost:$Port" }
$DataDir = (Resolve-Path -Path $DataDir -ErrorAction SilentlyContinue)?.Path ?? (New-Item -ItemType Directory -Path $DataDir -Force).FullName
$DbPath = Join-Path $DataDir "annex.db"
$ProjectRoot = $PSScriptRoot

Write-Host ""
Write-Host "╔══════════════════════════════════════╗" -ForegroundColor White
Write-Host "║         Annex Server Deploy          ║" -ForegroundColor White
Write-Host "╚══════════════════════════════════════╝" -ForegroundColor White
Write-Host ""
Write-Host "  Mode:       $Mode"
Write-Host "  Bind:       ${Host}:${Port}"
Write-Host "  Data dir:   $DataDir"
Write-Host "  Public URL: $PublicUrl"
Write-Host "  Log level:  $LogLevel"
Write-Host ""

# ── Docker mode ──

if ($Mode -eq "docker") {
    Write-Step "Checking Docker"
    if (-not (Test-Command "docker")) { Write-Fail "Docker not found. Install from https://docker.com" }

    $composeCmd = if (Test-Command "docker-compose") { "docker-compose" } else { "docker compose" }
    Write-Ok "Docker found"

    Write-Step "Building and starting containers"
    $env:ANNEX_HOST = $Host
    $env:ANNEX_PORT = $Port
    $env:ANNEX_LOG_LEVEL = $LogLevel

    & $composeCmd -f (Join-Path $ProjectRoot "docker-compose.yml") up -d --build
    if ($LASTEXITCODE -ne 0) { Write-Fail "Docker Compose failed" }

    Write-Ok "Containers started"
    Write-Step "Annex is running at http://localhost:$Port"
    Write-Host ""
    Write-Host "  Logs:    $composeCmd logs -f annex"
    Write-Host "  Stop:    $composeCmd down"
    Write-Host "  Restart: $composeCmd restart annex"
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
else { Write-Warn "sqlite3 not found (will seed via server startup)" }

# ZK verification key
$vkeyPath = Join-Path $ProjectRoot "zk/keys/membership_vkey.json"
if (-not (Test-Path $vkeyPath)) { Write-Fail "ZK verification key not found at $vkeyPath" }
Write-Ok "ZK verification key found"

# ── Build ──

if ($SkipBuild) {
    Write-Step "Skipping build (-SkipBuild)"
    $binary = Join-Path $ProjectRoot "target/release/annex-server"
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
    $binary = Join-Path $ProjectRoot "target/release/annex-server"
    Write-Ok "Built $binary"
}

# ── Initialize database ──

Write-Step "Initializing database at $DbPath"

# Migrations run automatically on startup, but we need the servers row.
# Do a quick startup check: if the DB exists and has a servers row, skip seeding.
$needsSeed = $true
if ((Test-Path $DbPath) -and $hasSqlite) {
    $count = sqlite3 $DbPath "SELECT COUNT(*) FROM servers;" 2>$null
    if ($count -gt 0) {
        Write-Ok "Database already seeded ($count server(s))"
        $needsSeed = $false
    }
}

if ($needsSeed) {
    if (-not (Test-Path $DbPath)) {
        Write-Host "   Database will be created on first startup"
    }

    # Run the server briefly to trigger migrations, then seed
    Write-Host "   Running migrations..."
    $env:ANNEX_DB_PATH = $DbPath
    $env:ANNEX_HOST = "127.0.0.1"
    $env:ANNEX_PORT = "0"
    $env:ANNEX_ZK_KEY_PATH = $vkeyPath
    $env:ANNEX_LOG_LEVEL = "warn"

    # Start server briefly to run migrations (it will fail with NoServerConfigured, which is expected)
    $migrationOutput = & $binary 2>&1
    # The server exits with an error because no server row exists — that's expected.

    if ($hasSqlite) {
        sqlite3 $DbPath "INSERT OR IGNORE INTO servers (slug, label, policy_json) VALUES ('$ServerSlug', '$ServerLabel', '{}');"
        if ($LASTEXITCODE -ne 0) { Write-Fail "Failed to seed server row" }
        Write-Ok "Database seeded: slug='$ServerSlug', label='$ServerLabel'"
    } else {
        Write-Warn "Cannot seed database without sqlite3 CLI."
        Write-Host "   Run this manually:"
        Write-Host "   sqlite3 $DbPath `"INSERT INTO servers (slug, label, policy_json) VALUES ('$ServerSlug', '$ServerLabel', '{}');`""
        Write-Host ""
        Write-Host "   Or install sqlite3:"
        Write-Host "     Debian/Ubuntu: sudo apt install sqlite3"
        Write-Host "     macOS:         brew install sqlite"
        Write-Host "     Windows:       winget install SQLite.SQLite"
    }
}

# ── Generate signing key if needed ──

if (-not $SigningKey) {
    Write-Warn "No -SigningKey provided. Server will use an ephemeral key."
    Write-Host "   This is fine for development but NOT for production."
    Write-Host "   Generate a permanent key:"
    Write-Host "   openssl rand -hex 32"
}

# ── Write runtime config ──

$configPath = Join-Path $DataDir "config.toml"
$configContent = @"
# Generated by deploy.ps1 on $(Get-Date -Format "yyyy-MM-dd HH:mm:ss")

[server]
host = "$Host"
port = $Port
public_url = "$PublicUrl"

[database]
path = "$DbPath"
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

$envVars = @{
    ANNEX_CONFIG_PATH  = $configPath
    ANNEX_ZK_KEY_PATH  = $vkeyPath
    ANNEX_DB_PATH      = $DbPath
    ANNEX_HOST         = $Host
    ANNEX_PORT         = $Port
    ANNEX_LOG_LEVEL    = $LogLevel
    ANNEX_PUBLIC_URL   = $PublicUrl
}

if ($SigningKey) { $envVars["ANNEX_SIGNING_KEY"] = $SigningKey }
if ($LogJson)    { $envVars["ANNEX_LOG_JSON"] = "true" }

foreach ($kv in $envVars.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($kv.Key, $kv.Value, "Process")
}

Write-Host ""
Write-Host "  Server starting at http://${Host}:${Port}" -ForegroundColor Green
Write-Host "  Public URL: $PublicUrl" -ForegroundColor Green
Write-Host "  Data: $DataDir" -ForegroundColor Green
Write-Host "  Logs: $LogLevel" -ForegroundColor Green
Write-Host ""
Write-Host "  Press Ctrl+C to stop." -ForegroundColor DarkGray
Write-Host ""

& $binary

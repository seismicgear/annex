#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Production smoke test for the Annex server identity flow on Windows.

.DESCRIPTION
    Boots `annex-server` with `enforce_zk_proofs=true` against a temporary
    data directory, drives it through a real registration → Merkle path →
    Groth16 proof → verify-membership → authenticated channel-create round
    trip, and shuts the server down cleanly. The actual API flow lives in
    `scripts/smoke-server-flow.mjs` so this script and the .sh stay thin.

    Required artifacts (none of these are dev-only):
      - zk/keys/membership_vkey.json
      - zk/build/membership_js/membership.wasm
      - zk/keys/membership_final.zkey

.PARAMETER Port
    Port to bind (default: 7321; must be free).

.PARAMETER ServerHost
    Bind address (default: 127.0.0.1).

.EXAMPLE
    pwsh ./scripts/smoke-server.ps1
#>

param(
    [int]   $Port = 7321,
    [string]$ServerHost = '127.0.0.1'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $RepoRoot

$Url = "http://${ServerHost}:${Port}"
$DataDir = $null
$ServerProcess = $null
$LogFile = $null

function Write-Step {
    param([string]$Message)
    Write-Host "[smoke-server] $Message"
}

function Stop-Server {
    if ($null -ne $ServerProcess) {
        try {
            if (-not $ServerProcess.HasExited) {
                Write-Step "stopping server (PID $($ServerProcess.Id))"
                # Send Ctrl+C to allow graceful shutdown.
                Stop-Process -Id $ServerProcess.Id -ErrorAction SilentlyContinue
                $null = $ServerProcess.WaitForExit(10000)
                if (-not $ServerProcess.HasExited) {
                    Stop-Process -Id $ServerProcess.Id -Force -ErrorAction SilentlyContinue
                }
            }
        } catch {
            # Best effort: the process may already be gone.
        }
    }
}

function Show-LogTail {
    if ($LogFile -and (Test-Path $LogFile)) {
        Write-Host '[smoke-server] -- server log (last 80 lines) --'
        Get-Content -Path $LogFile -Tail 80 | ForEach-Object { Write-Host $_ }
        Write-Host '[smoke-server] --------------------------------'
    }
}

try {
    # -- 1. Verify ZK artifacts -------------------------------------------
    Write-Step 'verifying ZK artifacts'
    $required = @(
        'zk/keys/membership_vkey.json',
        'zk/build/membership_js/membership.wasm',
        'zk/keys/membership_final.zkey'
    )
    foreach ($artifact in $required) {
        $full = Join-Path $RepoRoot $artifact
        if (-not (Test-Path $full) -or (Get-Item $full).Length -le 0) {
            throw "missing ZK artifact: $artifact. Run: (cd zk; npm ci; node scripts/build-circuits.js; node scripts/setup-groth16.js)"
        }
    }

    foreach ($pkg in @('snarkjs', 'circomlibjs')) {
        $modulePath = Join-Path $RepoRoot "zk/node_modules/$pkg"
        if (-not (Test-Path $modulePath)) {
            throw "zk/node_modules missing $pkg (run: npm --prefix zk ci)"
        }
    }

    # -- 2. Build the server (debug; relies on Cargo cache) ---------------
    # We build then exec the binary directly rather than `cargo run` so
    # the captured PID is the server itself — `cargo run` keeps the
    # spawned binary alive even when the cargo wrapper is killed, which
    # leaks state across smoke invocations.
    Write-Step 'building annex-server'
    $build = Start-Process -FilePath 'cargo' -ArgumentList @('build', '-p', 'annex-server', '--quiet') `
        -NoNewWindow -PassThru -Wait
    if ($build.ExitCode -ne 0) {
        throw "cargo build failed with exit code $($build.ExitCode)"
    }
    $ServerBinary = Join-Path $RepoRoot 'target/debug/annex-server.exe'
    if (-not (Test-Path $ServerBinary)) {
        throw "built binary missing: $ServerBinary"
    }

    # -- 3. Allocate temp data dir ----------------------------------------
    $DataDir = Join-Path ([System.IO.Path]::GetTempPath()) ("annex-smoke-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
    $LogFile = Join-Path $DataDir 'server.log'
    Write-Step "data dir: $DataDir"

    # -- 4. Start server with enforce_zk_proofs=true ----------------------
    Write-Step "starting server on $Url (enforce_zk_proofs=true)"

    $env:ANNEX_HOST = $ServerHost
    $env:ANNEX_PORT = "$Port"
    $env:ANNEX_DB_PATH = (Join-Path $DataDir 'annex.db')
    $env:ANNEX_ENFORCE_ZK_PROOFS = 'true'
    $env:ANNEX_OPEN_BROWSER = 'false'
    if (-not $env:RUST_LOG) {
        $env:RUST_LOG = 'warn,annex_server=info'
    }

    $ServerProcess = Start-Process -FilePath $ServerBinary `
        -ArgumentList @('NUL') `
        -NoNewWindow -PassThru `
        -RedirectStandardOutput $LogFile `
        -RedirectStandardError $LogFile

    # -- 5. Wait for /health ---------------------------------------------
    Write-Step 'waiting for /health'
    $ready = $false
    for ($attempt = 1; $attempt -le 90; $attempt++) {
        if ($ServerProcess.HasExited) {
            throw "server exited (code $($ServerProcess.ExitCode)) before becoming ready"
        }
        try {
            $null = Invoke-WebRequest -UseBasicParsing -Uri "$Url/health" -TimeoutSec 2 -ErrorAction Stop
            $ready = $true
            Write-Step "/health up after ${attempt}s"
            break
        } catch {
            Start-Sleep -Seconds 1
        }
    }
    if (-not $ready) {
        throw "/health never became ready"
    }

    # -- 6. Drive the identity + verify-membership + channel-create flow --
    $flow = Start-Process -FilePath 'node' `
        -ArgumentList @('scripts/smoke-server-flow.mjs', '--url', $Url) `
        -WorkingDirectory $RepoRoot -NoNewWindow -PassThru -Wait
    if ($flow.ExitCode -ne 0) {
        throw "smoke-server-flow.mjs failed with exit code $($flow.ExitCode)"
    }

    # -- 7. Stop cleanly --------------------------------------------------
    Write-Step 'flow complete; shutting down server'
    Stop-Server
    if ($null -ne $ServerProcess -and -not $ServerProcess.HasExited) {
        throw 'server did not stop after Stop-Process'
    }
    $ServerProcess = $null
    Write-Step 'OK'
}
catch {
    Show-LogTail
    Write-Error $_
    exit 1
}
finally {
    Stop-Server
    if ($DataDir -and (Test-Path $DataDir)) {
        Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue
    }
}

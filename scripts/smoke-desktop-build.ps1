#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Production smoke test for the Windows desktop build.

.DESCRIPTION
    Pinned to the release-critical surface only:

      1. Verify ZK artifacts (real ones — no dev fallback).
      2. Build the client bundle (tsc + vite).
      3. Build the desktop binary (`cargo build -p annex-desktop --release`).
      4. Confirm the resources Tauri will bundle exist on disk.

    This is intentionally narrower than the full Tauri bundle build that
    `release-desktop.yml` runs — that lane validates the `.exe`
    packaging end-to-end and is too slow for every smoke pass. Use this
    script when you want a fast pass/fail on the link + resource wiring.

.PARAMETER SkipClientBuild
    DEV-ONLY: skip the client build step. Requires client\dist\index.html
    to already exist. Not for release or CI.

.EXAMPLE
    pwsh ./scripts/smoke-desktop-build.ps1

.EXAMPLE
    pwsh ./scripts/smoke-desktop-build.ps1 -SkipClientBuild
#>

param(
    [switch]$SkipClientBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $RepoRoot
Write-Host "[smoke-desktop] repo root: $RepoRoot"

function Invoke-Required {
    param(
        [Parameter(Mandatory)][string]   $File,
        [Parameter(Mandatory)][string[]] $Arguments,
        [string]$WorkingDirectory = $RepoRoot
    )
    $proc = Start-Process -FilePath $File -ArgumentList $Arguments `
        -WorkingDirectory $WorkingDirectory -NoNewWindow -PassThru -Wait
    if ($proc.ExitCode -ne 0) {
        throw "$File $($Arguments -join ' ') exited with code $($proc.ExitCode)"
    }
}

# -- 1. Verify ZK artifacts ---------------------------------------------------
#
# tauri.conf.json declares `../../zk/keys/membership_vkey.json` as a
# bundle resource. The client also needs `membership.wasm` and
# `membership_final.zkey` to generate proofs at runtime — these are
# copied into client/public/zk/ by build-desktop.js. All three must
# exist as non-empty files before we attempt to build.

Write-Host '[smoke-desktop] verifying ZK artifacts'
$RequiredZk = @(
    'zk/keys/membership_vkey.json',
    'zk/build/membership_js/membership.wasm',
    'zk/keys/membership_final.zkey'
)
foreach ($artifact in $RequiredZk) {
    $full = Join-Path $RepoRoot $artifact
    if (-not (Test-Path $full) -or (Get-Item $full).Length -le 0) {
        throw "missing ZK artifact: $artifact. Run: (cd zk; npm ci; node scripts/build-circuits.js; node scripts/setup-groth16.js)"
    }
}

# Ensure the vkey at least parses as JSON so we don't ship a corrupt
# resource into the bundle.
$vkeyPath = Join-Path $RepoRoot 'zk/keys/membership_vkey.json'
try {
    Get-Content -Path $vkeyPath -Raw | ConvertFrom-Json | Out-Null
} catch {
    throw "zk/keys/membership_vkey.json is not parseable JSON: $_"
}

# -- 2. Build the client ------------------------------------------------------

# We run `node scripts/build-desktop.js` rather than `npm run build`
# directly because it is the same script Tauri's `beforeBuildCommand`
# invokes. It (a) copies `membership.wasm` and `membership_final.zkey`
# into `client/public/zk/` so the proof worker can serve them, then
# (b) runs the same `tsc -b && vite build` we'd otherwise invoke. Using
# the real entry point keeps the smoke aligned with the bundle path.
#
# `SKIP_PIPER=1` keeps the smoke fast — Piper TTS assets are validated
# separately by the release-desktop workflow's setup-piper step. They
# are not on the Rust build path that this smoke is gating.

if ($SkipClientBuild) {
    Write-Host '[smoke-desktop] DEV-ONLY: -SkipClientBuild set, skipping client build'
    $indexHtml = Join-Path $RepoRoot 'client/dist/index.html'
    if (-not (Test-Path $indexHtml)) {
        throw "-SkipClientBuild set but client/dist/index.html is missing"
    }
} else {
    Write-Host '[smoke-desktop] installing client deps'
    $clientNm = Join-Path $RepoRoot 'client/node_modules'
    if (-not (Test-Path $clientNm)) {
        Invoke-Required -File 'npm' -Arguments @('--prefix', 'client', 'ci')
    }

    Write-Host '[smoke-desktop] running build-desktop.js (ZK copy + client build)'
    $env:SKIP_PIPER = '1'
    Invoke-Required -File 'node' -Arguments @('scripts/build-desktop.js')
}

# -- 3. Build the desktop binary ---------------------------------------------

Write-Host '[smoke-desktop] cargo build -p annex-desktop --release'
Invoke-Required -File 'cargo' -Arguments @('build', '-p', 'annex-desktop', '--release')

# -- 4. Confirm packaged resources exist -------------------------------------

Write-Host '[smoke-desktop] verifying packaged resources'

$ReleaseOutputs = @(
    'client/dist/index.html',
    'client/public/zk/membership.wasm',
    'client/public/zk/membership_final.zkey',
    'zk/keys/membership_vkey.json'
)

if (-not $SkipClientBuild) {
    $assetsDir = Join-Path $RepoRoot 'client/dist/assets'
    if (-not (Test-Path $assetsDir)) {
        throw 'client/dist/assets missing after build'
    }
}

foreach ($resource in $ReleaseOutputs) {
    $full = Join-Path $RepoRoot $resource
    if (-not (Test-Path $full) -or (Get-Item $full).Length -le 0) {
        throw "expected resource missing: $resource"
    }
}

# Windows release binary path. `target\release\annex-desktop.exe` exists
# once the cargo build above succeeds.
$ReleaseBinary = Join-Path $RepoRoot 'target/release/annex-desktop.exe'
if (-not (Test-Path $ReleaseBinary) -or (Get-Item $ReleaseBinary).Length -le 0) {
    throw "release binary missing: $ReleaseBinary"
}

$binarySize = (Get-Item $ReleaseBinary).Length
Write-Host "[smoke-desktop] release binary: $ReleaseBinary ($binarySize bytes)"

Write-Host '[smoke-desktop] OK'

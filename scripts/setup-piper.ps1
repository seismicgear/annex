#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Downloads the Piper TTS binary and the en_US-lessac-medium voice model.

.DESCRIPTION
    Sets up:
      assets/piper/piper.exe     — the Piper binary (Windows)
      assets/voices/*.onnx       — voice model
      assets/voices/*.onnx.json  — voice model config

.EXAMPLE
    ./scripts/setup-piper.ps1
#>

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$PiperVersion = "2023.11.14-2"
$VoiceModel = "en_US-lessac-medium"
$VoiceBaseUrl = "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$PiperDir = Join-Path $ProjectRoot "assets" "piper"
$VoicesDir = Join-Path $ProjectRoot "assets" "voices"

# ── Helpers ──

function Write-Step { param([string]$msg) Write-Host ":: $msg" -ForegroundColor Cyan }
function Write-Ok   { param([string]$msg) Write-Host "   OK: $msg" -ForegroundColor Green }
function Write-Fail { param([string]$msg) Write-Host "   FAIL: $msg" -ForegroundColor Red; exit 1 }

# ── Detect platform ──

function Get-PiperArchive {
    if ($IsWindows -or ($env:OS -eq "Windows_NT")) {
        return "piper_windows_amd64.zip"
    }
    if ($IsMacOS) {
        $arch = uname -m
        if ($arch -eq "arm64") { return "piper_macos_aarch64.tar.gz" }
        return "piper_macos_x86_64.tar.gz"
    }
    # Linux
    $arch = uname -m
    if ($arch -eq "aarch64") { return "piper_linux_aarch64.tar.gz" }
    return "piper_linux_x86_64.tar.gz"
}

# ── Download Piper binary ──

function Setup-PiperBinary {
    $exeSuffix = if ($IsWindows -or ($env:OS -eq "Windows_NT")) { ".exe" } else { "" }
    $piperBin = Join-Path $PiperDir "piper$exeSuffix"

    if (Test-Path $piperBin) {
        Write-Ok "Piper binary already exists at $piperBin"
        return
    }

    Write-Step "Downloading Piper binary..."
    New-Item -ItemType Directory -Path $PiperDir -Force | Out-Null

    $archive = Get-PiperArchive
    $url = "https://github.com/rhasspy/piper/releases/download/$PiperVersion/$archive"

    $tmpFile = Join-Path ([System.IO.Path]::GetTempPath()) $archive

    try {
        Invoke-WebRequest -Uri $url -OutFile $tmpFile -UseBasicParsing
    } catch {
        Write-Fail "Failed to download Piper from $url : $_"
    }

    if ($archive -like "*.zip") {
        Expand-Archive -Path $tmpFile -DestinationPath $PiperDir -Force
    } else {
        # tar.gz — use tar
        tar -xzf $tmpFile -C $PiperDir --strip-components=1
    }

    Remove-Item $tmpFile -ErrorAction SilentlyContinue

    if (-not ($IsWindows -or ($env:OS -eq "Windows_NT"))) {
        chmod +x $piperBin
    }

    Write-Ok "Piper binary installed to $piperBin"
}

# ── Download voice model ──

function Setup-VoiceModel {
    $onnxFile = Join-Path $VoicesDir "$VoiceModel.onnx"
    $jsonFile = Join-Path $VoicesDir "$VoiceModel.onnx.json"

    if ((Test-Path $onnxFile) -and (Test-Path $jsonFile)) {
        Write-Ok "Voice model $VoiceModel already exists"
        return
    }

    Write-Step "Downloading voice model: $VoiceModel..."
    New-Item -ItemType Directory -Path $VoicesDir -Force | Out-Null

    if (-not (Test-Path $onnxFile)) {
        try {
            Invoke-WebRequest -Uri "$VoiceBaseUrl/$VoiceModel.onnx" -OutFile $onnxFile -UseBasicParsing
        } catch {
            Write-Fail "Failed to download $VoiceModel.onnx : $_"
        }
    }

    if (-not (Test-Path $jsonFile)) {
        try {
            Invoke-WebRequest -Uri "$VoiceBaseUrl/$VoiceModel.onnx.json" -OutFile $jsonFile -UseBasicParsing
        } catch {
            Write-Fail "Failed to download $VoiceModel.onnx.json : $_"
        }
    }

    Write-Ok "Voice model installed to $VoicesDir"
}

# ── Main ──

Write-Step "Setting up Piper TTS for Annex"
Write-Host ""
Setup-PiperBinary
Setup-VoiceModel
Write-Host ""
Write-Ok "Piper TTS setup complete"
Write-Host "   Model:  $(Join-Path $VoicesDir "$VoiceModel.onnx")"
Write-Host "   Config: $(Join-Path $VoicesDir "$VoiceModel.onnx.json")"

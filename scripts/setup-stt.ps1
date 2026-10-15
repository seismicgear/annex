#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Install and verify the native Windows whisper.cpp runtime and a pinned GGML model.
.DESCRIPTION
    Downloads are checked before installation. Existing model files are always verified.
    -Verify performs no downloads. Paths match the Windows defaults used by Annex.
#>
param(
    [ValidateSet('tiny.en', 'base.en')][string]$Model = 'base.en',
    [switch]$Verify
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$AssetsDir = Join-Path $ProjectRoot 'assets'
$ModelsDir = Join-Path $AssetsDir 'models'
$WhisperDir = Join-Path $AssetsDir 'whisper'
$Revision = '5359861c739e955e79d9a303bcbc70fb988958b1'
$Models = @{
    'tiny.en' = @{ Sha = '921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f'; Bytes = 77704715 }
    'base.en' = @{ Sha = 'a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002'; Bytes = 147964211 }
}
$ArchiveSha = '49dcc16de826f20bd53d44f947a1ae49dfa81f86cad67a64d80820cb192d674a'
$ArchiveUrl = 'https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.2/whisper-bin-x64.zip'

function Assert-Digest([string]$Path, [string]$Expected, [long]$Bytes = 0) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Missing file: $Path" }
    if ($Bytes -gt 0 -and (Get-Item -LiteralPath $Path).Length -ne $Bytes) { throw "Size mismatch: $Path" }
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Expected) {
        throw "SHA-256 mismatch: $Path. Refusing to use an unverified speech asset."
    }
}

$ModelPath = Join-Path $ModelsDir "ggml-$Model.bin"
if (-not (Test-Path -LiteralPath $ModelPath)) {
    if ($Verify) { throw "Model missing: $ModelPath" }
    New-Item -ItemType Directory -Path $ModelsDir -Force | Out-Null
    $Partial = "$ModelPath.$([guid]::NewGuid().ToString('N')).partial"
    try {
        Invoke-WebRequest -Uri "https://huggingface.co/ggerganov/whisper.cpp/resolve/$Revision/ggml-$Model.bin" -OutFile $Partial -UseBasicParsing
        Assert-Digest $Partial $Models[$Model].Sha $Models[$Model].Bytes
        Move-Item -LiteralPath $Partial -Destination $ModelPath
    } finally {
        if (Test-Path -LiteralPath $Partial) { Remove-Item -LiteralPath $Partial }
    }
}
Assert-Digest $ModelPath $Models[$Model].Sha $Models[$Model].Bytes

$Binary = Join-Path $WhisperDir 'whisper-cli.exe'
$RuntimeManifest = Join-Path $WhisperDir 'runtime-sha256.json'
if (-not (Test-Path -LiteralPath $RuntimeManifest)) {
    if ($Verify) { throw "Verified runtime missing: $WhisperDir" }
    New-Item -ItemType Directory -Path $WhisperDir -Force | Out-Null
    $Archive = Join-Path $WhisperDir "runtime-$([guid]::NewGuid().ToString('N')).zip"
    $Staging = Join-Path $WhisperDir "stage-$([guid]::NewGuid().ToString('N'))"
    try {
        Invoke-WebRequest -Uri $ArchiveUrl -OutFile $Archive -UseBasicParsing
        Assert-Digest $Archive $ArchiveSha 8194445
        Expand-Archive -LiteralPath $Archive -DestinationPath $Staging
        $Cli = @(Get-ChildItem -LiteralPath $Staging -Filter 'whisper-cli.exe' -Recurse)
        if ($Cli.Count -ne 1) { throw 'Pinned archive must contain exactly one whisper-cli.exe.' }
        $Hashes = @{}
        foreach ($File in (Get-ChildItem -LiteralPath $Cli[0].DirectoryName -File)) {
            if ($File.Extension -in @('.exe', '.dll')) {
                Copy-Item -LiteralPath $File.FullName -Destination $WhisperDir -Force
                $Hashes[$File.Name] = (Get-FileHash -LiteralPath $File.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        }
        $Hashes | ConvertTo-Json | Set-Content -LiteralPath $RuntimeManifest -Encoding utf8
    } finally {
        if (Test-Path -LiteralPath $Archive) { Remove-Item -LiteralPath $Archive }
        if (Test-Path -LiteralPath $Staging) {
            $Resolved = (Resolve-Path -LiteralPath $Staging).Path
            $Allowed = (Resolve-Path -LiteralPath $WhisperDir).Path + [IO.Path]::DirectorySeparatorChar
            if (-not $Resolved.StartsWith($Allowed, [StringComparison]::OrdinalIgnoreCase)) { throw 'Staging cleanup escaped the speech directory.' }
            Remove-Item -LiteralPath $Resolved -Recurse -Force
        }
    }
}
$Hashes = Get-Content -LiteralPath $RuntimeManifest -Raw | ConvertFrom-Json
foreach ($Entry in $Hashes.PSObject.Properties) {
    if ([IO.Path]::GetFileName($Entry.Name) -ne $Entry.Name) { throw 'Invalid speech runtime manifest path.' }
    Assert-Digest (Join-Path $WhisperDir $Entry.Name) $Entry.Value
}
if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) { throw "Speech binary missing: $Binary" }
& $Binary --help *> $null
if ($LASTEXITCODE -ne 0) { throw "whisper-cli could not execute (exit $LASTEXITCODE)." }
Write-Host "[setup-stt] Verified Windows runtime: $Binary"
Write-Host "[setup-stt] Verified model: $ModelPath"
Write-Host "ANNEX_STT_BINARY_PATH=$Binary"
Write-Host "ANNEX_STT_MODEL_PATH=$ModelPath"

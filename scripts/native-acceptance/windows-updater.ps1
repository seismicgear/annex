$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'Actual installation qualification requires a disposable Windows Actions runner.'
}
$evidence = Join-Path $PWD 'evidence'
$roots = @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*')
function Get-AnnexRecord {
    @(Get-ItemProperty -Path $roots -ErrorAction SilentlyContinue | Where-Object DisplayName -eq Annex)
}
if ((Get-AnnexRecord).Count) { throw 'Updater test requires a fresh Annex installation' }
$baseline = @(Get-ChildItem -LiteralPath 'target/x86_64-pc-windows-msvc/release/bundle/nsis' -Filter '*_x64-setup.exe')
if ($baseline.Count -ne 1) { throw 'Expected exactly one test baseline installer' }
$installer = Start-Process -FilePath $baseline[0].FullName -ArgumentList '/S' -WindowStyle Hidden -PassThru -Wait
if ($installer.ExitCode -ne 0) { throw "Baseline install failed: $($installer.ExitCode)" }
$records = @(Get-AnnexRecord)
if ($records.Count -ne 1) { throw 'Expected one installed Annex record' }
$directory = $records[0].InstallLocation.Trim()
if ($directory.StartsWith('"') -or $directory.EndsWith('"')) {
    if ($directory -notmatch '^"[^"]+"$') { throw 'Invalid quoted installation path' }
    $directory = $directory.Substring(1, $directory.Length - 2)
}
if ($directory -match '["\x00-\x1f]' -or -not [IO.Path]::IsPathFullyQualified($directory)) { throw 'Invalid installation path' }
$executable = Join-Path $directory 'annex-desktop.exe'
$before = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLower()
$inventory = Get-Content candidate/array-installation.json -Raw | ConvertFrom-Json
$expected = ($inventory.artifacts | Where-Object kind -eq nsis).executableSha256
if ((Get-Item -LiteralPath $executable).VersionInfo.ProductVersion -ne '0.1.2' -or $before -eq $expected) {
    throw 'Baseline must be the explicit older-version test build'
}
$policy = 'HKLM:\Software\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments'
$policyName = 'annex-desktop.exe'
if (Get-ItemProperty -LiteralPath $policy -Name $policyName -ErrorAction SilentlyContinue) {
    throw 'Refusing to overwrite an existing Annex inspection policy'
}
Set-Content -LiteralPath (Join-Path $env:RUNNER_TEMP 'annex-inspection-policy-owned') -Value $policyName
New-Item -Path $policy -Force | Out-Null
New-ItemProperty -LiteralPath $policy -Name $policyName -Value '--remote-debugging-port=31429 --remote-debugging-address=127.0.0.1' -PropertyType String | Out-Null
$env:ANNEX_NATIVE_CDP = 'http://127.0.0.1:31429'
$env:ANNEX_NATIVE_EXECUTABLE = $executable
$env:ANNEX_UPDATER_MODE_FILE = Join-Path $env:RUNNER_TEMP 'annex-updater-mode.txt'
Set-Content -LiteralPath $env:ANNEX_UPDATER_MODE_FILE -Value 'stall' -Encoding utf8NoBOM
$tls = Join-Path $env:RUNNER_TEMP 'annex-updater-tls'
$serverArguments = @('scripts/native-acceptance/updater-fixture.py', '--candidate', 'candidate', '--tls', ('"' + $tls + '"'),
    '--mode', ('"' + $env:ANNEX_UPDATER_MODE_FILE + '"'), '--events', 'evidence/updater-transport.jsonl')
$server = Start-Process -FilePath (Get-Command python).Source -ArgumentList $serverArguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "$evidence/fixture-stdout.log" -RedirectStandardError "$evidence/fixture-stderr.log"
$app = $null
function Wait-NativeWebView {
    for ($attempt = 0; $attempt -lt 90; $attempt++) {
        try { $null = Invoke-RestMethod "$env:ANNEX_NATIVE_CDP/json/version" -TimeoutSec 1; return } catch {}
        Start-Sleep -Seconds 1
    }
    throw 'Native WebView did not become ready'
}
try {
    $ready = $false
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        if ($server.HasExited) { throw 'Updater HTTPS fixture exited' }
        try { $null = Invoke-WebRequest 'https://localhost:34567/health' -TimeoutSec 1; $ready = $true; break } catch {}
        Start-Sleep -Seconds 1
    }
    if (-not $ready) { throw 'OS-trusted HTTPS fixture did not become ready' }
    # Use the ordinary installed profile. Isolated profiles deliberately disable
    # executable replacement and cannot qualify the updater.
    $app = Start-Process -FilePath $executable -WindowStyle Hidden -PassThru -RedirectStandardOutput "$evidence/native-stdout.log" -RedirectStandardError "$evidence/native-stderr.log"
    Wait-NativeWebView
    $env:ANNEX_NATIVE_PHASE = 'baseline'
    $env:ANNEX_NATIVE_EVIDENCE = Join-Path $evidence 'baseline'
    New-Item -ItemType Directory -Path $env:ANNEX_NATIVE_EVIDENCE | Out-Null
    node client/node_modules/playwright/cli.js test --config client/e2e/native/windows.playwright.config.ts
    if ($LASTEXITCODE) { throw 'Could not create the actual founder and saved message' }
    $env:ANNEX_NATIVE_EVIDENCE = Join-Path $evidence 'update'
    New-Item -ItemType Directory -Path $env:ANNEX_NATIVE_EVIDENCE | Out-Null
    node client/node_modules/playwright/cli.js test --config client/e2e/native/windows-updater.playwright.config.ts
    if ($LASTEXITCODE) { throw 'Native updater failure/retry/install journey failed' }
    $replaced = $false
    for ($attempt = 0; $attempt -lt 180; $attempt++) {
        try {
            if ((Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLower() -eq $expected) { $replaced = $true; break }
        } catch {}
        Start-Sleep -Seconds 2
    }
    if (-not $replaced) { throw 'Actual updater installer never installed the inventoried executable' }
    if (-not $app.WaitForExit(30000)) { throw 'Old application process remained after update' }
    Wait-NativeWebView
    $restarted = @(Get-Process -Name annex-desktop -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $executable })
    if ($restarted.Count -ne 1 -or $restarted[0].Id -eq $app.Id) { throw 'Updater did not restart the installed candidate exactly once' }
    $env:ANNEX_NATIVE_PHASE = 'recovery'
    $env:ANNEX_NATIVE_EVIDENCE = Join-Path $evidence 'recovery'
    New-Item -ItemType Directory -Path $env:ANNEX_NATIVE_EVIDENCE | Out-Null
    node client/node_modules/playwright/cli.js test --config client/e2e/native/windows.playwright.config.ts
    if ($LASTEXITCODE) { throw 'Actual updated app failed to recover its founder and saved message' }
    $version = (Get-Item -LiteralPath $executable).VersionInfo.ProductVersion
    if ($version -ne $inventory.version) { throw 'Updated executable has the wrong product version' }
    @{
        candidateSource=$inventory.sourceCommit; candidateVersion=$version; installedSha256=$expected
        baselineSource=$env:SUBJECT_COMMIT; baselineVersion='0.1.2'; baselineBinarySha256=$before
        baselineIsPublished012=$false; baselineBuildOverrides=@('version','updater HTTPS fixture endpoint')
        actualInAppUpdater=$true; actualNsisReplacement=$true; automaticRestart=$true
        ordinaryProfile=$true; founderAndMessagePreserved=$true; corruptSignatureRejected=$true
        stalledDownloadRecovered=$true; productionOriginTested=$false; windowsPublisherSignatureTested=$false
    } | ConvertTo-Json | Set-Content -LiteralPath "$evidence/result.json"
} finally {
    # Stop only this runner's fresh installation and fixture, then remove only
    # the inspection setting this script established.
    Get-Process -Name annex-desktop -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $executable } | Stop-Process -Force
    if (-not $server.HasExited) { Stop-Process -Id $server.Id -Force }
    Remove-ItemProperty -LiteralPath $policy -Name $policyName -ErrorAction SilentlyContinue
}

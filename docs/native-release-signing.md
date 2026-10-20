# Native publisher signing

Tauri updater signatures, Windows Authenticode, Apple Developer ID/notarization,
and the Array Ed25519 release envelope are independent trust layers. The
release workflow must complete each applicable layer before production release.
An updater-signed draft is not an OS-trusted commercial installer.

The native path uses the pinned official Tauri CLI's
[Windows signing](https://v2.tauri.app/distribute/sign/windows/) and
[macOS signing/notarization](https://v2.tauri.app/distribute/sign/macos/)
interfaces. It does not replace or relax Array's publisher checks.

## Build modes

`Release Desktop App` accepts `native_signing=true` for a fully signed draft
rehearsal. Tag builds require native signing automatically. Preflight refuses
missing credentials before building. Only a branch dispatch with `dry_run=true`
may use `native_signing=false`; those packages retain the existing unsigned
Windows / ad-hoc macOS status. `unsigned_dry_run` refers separately to updater
signatures and cannot be combined with native production signing.

## Required organization-owned configuration

| Platform | Repository secrets | Repository variables |
| --- | --- | --- |
| Windows | `WINDOWS_CERTIFICATE` (base64 PFX), `WINDOWS_CERTIFICATE_PASSWORD` | `WINDOWS_CERTIFICATE_THUMBPRINT` (40 hex digits), `WINDOWS_TIMESTAMP_URL` (issuer-approved HTTPS RFC3161 endpoint) |
| macOS | `APPLE_CERTIFICATE` (base64 P12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD` (app-specific password) | `APPLE_SIGNING_IDENTITY` (`Developer ID Application: ... (TEAMID)`), `APPLE_TEAM_ID` |
| Updater | Existing `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `TAURI_SIGNING_PUBLIC_KEY` | Existing optional `ANNEX_UPDATER_ENDPOINT` |

The certificate owner must provide valid signing authority. No certificate,
production key, Apple identity, timestamp host or Array publisher pin is
generated or inferred by these scripts. This Windows route requires an
exportable organization-approved PFX; hardware/cloud-only signing authority
requires its actual provider integration.

The Windows job validates the PFX leaf's exact thumbprint, private key,
code-signing purpose, validity and trusted chain before importing only that leaf.
It uses Tauri's SHA256/RFC3161 signer. A cleanup step removes only the key owned
by this build, including on failure; no CA is added to Windows trust. The job
then verifies Authenticode validity, publisher identity and timestamps on the
executable, NSIS installer and MSI installer.

The macOS job overrides the draft ad-hoc identity with the approved Developer ID,
keeps the existing entitlements, enables hardened runtime, and gives Tauri the
real notarization credentials. Its gate verifies the sealed application and
embedded Mach-O code, the main publisher's team, hardened runtime, a stapled
notarization ticket and Gatekeeper acceptance. A successful command invocation
alone does not satisfy these checks.

The Array inventory is captured **after** final packaging and signing, because
those operations change executable and installer bytes. Keep the generated
native-signature receipts with the exact build and repeat native installation,
upgrade, recovery and Array acceptance using the signed packages.

## Qualification boundary

The draft path and negative credential/configuration gates are testable without
private keys. No production certificate or notarization credential was available
in the October 20 qualification session. The credentialed path is implemented
but has not produced an OS-trusted candidate; its real signing, timestamp,
notarization and signed-install acceptance remain blocking external checks.

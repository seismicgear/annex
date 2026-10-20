# Install Annex v0.1.2

Use the [download table](../README.md#download-and-install) or
[release assets](https://github.com/seismicgear/annex/releases/tag/v0.1.2).
Windows and Linux packages are x86-64; macOS has separate Apple Silicon and
Intel DMGs. Native Linux ARM and Windows ARM packages are not published.

v0.1.3 is a candidate under qualification. Use its source-bound package and
acceptance records in [release readiness](RELEASE_READINESS.md) when evaluating
that candidate; older release receipts do not qualify it.

Download `SHA256SUMS` from the same release and compare the line matching your
installer's exact filename. The file records SHA-256 digests; a hash verifies
that you received the published bytes, not that a publisher was certified by an OS.

## Windows

Run `annex-desktop-windows-x86_64.exe` or `.msi`, complete the installer, and launch
Annex. The package includes an offline WebView2 runtime. In PowerShell:

```powershell
Get-FileHash .\annex-desktop-windows-x86_64.exe -Algorithm SHA256
```

Windows packages lack Authenticode signatures. Review the release origin and
checksum before approving a trust prompt; respect managed-device policy. If
WebView2 cannot initialize, repair/install Microsoft's WebView2 Runtime and
restart. A release-era Sandbox attempt was blocked at this step. A later
[user-performed v0.1.0 public-invite launch and admission](acceptance/windows-0.1.0-public-invite.json)
succeeded in an isolated standard Windows account. That receipt does not qualify
message exchange or a different installer version.

## macOS

Choose the DMG matching **Apple menu → About This Mac**. Drag Annex into
Applications, then open that copy. The bundle declares macOS 11 as its minimum;
that declaration is not a compatibility test on every macOS release.

```bash
shasum -a 256 annex-desktop-macos-aarch64.dmg
# For Intel, substitute annex-desktop-macos-x86_64.dmg.
```

v0.1.2 retains the corrected bundle sealing introduced in v0.1.1. Published
Apple Silicon and Intel DMGs completed Safari → installed Annex → explicit join
→ independently received message, plus warm invitation delivery in
[Apple Silicon acceptance](https://github.com/seismicgear/annex/actions/runs/39098657653)
and [Intel acceptance](https://github.com/seismicgear/annex/actions/runs/39099967852).

Apple Developer ID signing and notarization remain unavailable. Those native
receipts used verified release downloads and do not establish the ordinary
browser-download/quarantine/Gatekeeper journey. Respect macOS trust prompts and
managed-device policy; do not remove quarantine or disable Gatekeeper. If macOS
blocks the app, report the exact message. The original v0.1.0 installers remain
unchanged and retain their packaging defect.

## Linux

The published Linux installers target x86-64. The installed v0.1.2 DEB completed
hosting, independent direct summary access and persisted-state reopening;
[release readiness](RELEASE_READINESS.md) records the boundary. Compatibility on
other distributions and every supported desktop environment still needs testing.

For Debian/Ubuntu, from the download directory:

```bash
sha256sum annex-desktop-linux-x86_64.deb
sudo apt install ./annex-desktop-linux-x86_64.deb
```

The package installs required desktop dependencies and registers `annex://`.
Use a distribution with the required WebKitGTK 4.1 packages (the DEB declares
WebKitGTK ≥2.40). Launch from the application menu.

For AppImage, first move the file to the location where you will keep it:

```bash
sha256sum annex-desktop-linux-x86_64.AppImage
chmod +x annex-desktop-linux-x86_64.AppImage
./annex-desktop-linux-x86_64.AppImage
```

FUSE 2 compatibility is required (`libfuse2t64` on Ubuntu 24.04; often `libfuse2`
on other distributions). The AppImage registers its current path on first
launch, including paths containing spaces. Launch it again after moving it.
Use the DEB on a supported distribution if AppImage integration is unavailable.

## Open an invitation

Open Annex once, then follow the invitation page's **Open in Annex** link and
approve the browser's external-app prompt. Create/import an identity, review the
server, and explicitly select **Join**. See [Getting started](getting-started.md).

If the handler does not launch:

- Windows: check **Settings → Apps → Default apps** for the ANNEX protocol, or
  reinstall the package.
- macOS: open the installed copy in Applications, then retry the invitation.
- Linux: inspect `xdg-mime query default x-scheme-handler/annex`; launch the
  installed app or AppImage at its permanent path to register it again.

If the app opens but cannot join, ask the inviter to confirm the server is
online and the invitation is valid. Generated invitations use Monolith for peer
introduction and direct authenticated connections for application traffic.
Networks that prevent direct connectivity fail explicitly. A manually configured
server URL requires reachable HTTPS.

## Updates, removal, and server packages

v0.1.0 includes a configured, signed Tauri updater and a published updater feed.
A real in-place upgrade from an installed earlier production version remains
unverified. Do not describe a signature check as an installed update test.
Back up your identity and server data before changing versions. Removing the
application is separate from deleting its data/profile; do not delete data to
troubleshoot an installer without a backup.

The Linux server tarball and container are for operators. The tarball contains
proof keys and the alignment model, **but no browser client or speech runtime**.
Use [Deployment](deployment.md) for those paths.

The original v0.1.0 binaries remain unchanged. Their bundled old license is
supplemented by the KI-owned-code grant in [LICENSE.md §10](../LICENSE.md#10-existing-v010-distribution).

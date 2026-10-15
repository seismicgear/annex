# Install Annex v0.1.0

Use the [download table](../README.md#download-and-install) or
[release assets](https://github.com/seismicgear/annex/releases/tag/v0.1.0).
Windows and Linux packages are x86-64; macOS has separate Apple Silicon and
Intel DMGs. Native Linux ARM and Windows ARM packages are not published.

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
restart. A release-era Windows Sandbox attempt was blocked at this step; the
full native invitation-to-message workflow has not yet been accepted on Windows.

## macOS

Choose the DMG matching **Apple menu → About This Mac**. Drag Annex into
Applications, then open that copy. The bundle declares macOS 11 as its minimum;
that declaration is not a compatibility test on every macOS release.

```bash
shasum -a 256 annex-desktop-macos-aarch64.dmg
# For Intel, substitute annex-desktop-macos-x86_64.dmg.
```

**Known v0.1.0 packaging problem:** the published apps lack complete bundle
signatures and macOS may reject them. Wait for a corrected, qualified release;
do not remove quarantine or disable Gatekeeper to work around this defect.
[#322](https://github.com/seismicgear/annex/pull/322) adds explicit ad-hoc app
signing, signed embedded Whisper code, and strict checks of the completed bundle.
That correction is prepared as v0.1.1 and is separate from the immutable v0.1.0
assets. Developer ID signing and notarization are still unavailable.

The original native builds passed their then-existing release gates, which did
not establish complete bundle sealing. Full Mac invitation/join/message
acceptance and normal browser-download/Gatekeeper acceptance remain separate
requirements; the new native workflow is not proof that either has passed.

## Linux

The published Linux installers target x86-64. The release's native acceptance
used Ubuntu; compatibility on other distributions still needs testing.

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
online and the invitation is valid. A public invitation requires reachable
HTTPS. It does not create a tunnel through the host's network.

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

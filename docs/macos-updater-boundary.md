# macOS installation boundary

The desktop remains sandboxed with its existing entitlements. Its Updates
dialog, transport, progress, pinned public key, signed version metadata and
download verification remain unchanged. macOS replacement is delegated to the
private `com.annex.desktop.installer` XPC service sealed into `Annex.app`.
Windows and Linux retain their existing platform installers.

The service accepts exactly three values: archive bytes, the detached signature
and the announced version. It accepts no destination, executable command,
environment, network URL or verification key. Both peers authenticate the
kernel-bound XPC message using the designated code requirement of the installed
peer. The service independently verifies the archive against its compiled public
key before extraction. All paths stay inside one bounded app archive; links,
devices, duplicate names, traversal and multiple roots are rejected.

The staged app must have Annex's bundle identifier and executable, exactly the
announced version, a version newer than the installed app, and a valid strict
nested code seal. A Developer ID installation also requires the same signing
team. The destination is derived from the service's canonical executable
location. A same-volume atomic exchange preserves a complete app if the service
or computer stops during replacement. No elevated shell, root daemon, quarantine
removal, library-validation bypass or writable-path entitlement is introduced.
The service has no renderer, listener or network client.

After the exchange, the service retains the old bundle in its private staging
directory until the requesting XPC connection closes. The desktop authenticates
the reply before closing that connection. Removing the old running code sooner
invalidates reply authentication even though replacement succeeded. Cleanup
accepts no path or additional client command; it releases only the directory
owned by that connection.

Administrator-managed, read-only or non-atomic-exchange volumes fail closed and
retain the installed app. Their owner must use the normal DMG installer to
replace Annex with appropriate filesystem permission. The helper does not
elevate itself or silently alter the installation's ownership. Application data
and identity remain in the existing profile and are outside its write surface.

`scripts/tests/macos-installer-native.py` exercises the real XPC boundary using
two differently signed native executables, an invalid payload, and a signed
component bundle exchanged through the real desktop adapter. It verifies that
reply authentication succeeds after the exchange. Rust tests
cover signature tampering, archive handling, identity/version
rejection and the actual macOS atomic exchange. These boundary checks are
separate from `NativeUpdaterAcceptance.swift`, which must download the signed
real candidate, reject a stalled/corrupt transfer, replace the installed app,
restart it automatically and recover its founder and message.

The packaging gate inspects signed entitlements and verifies both the nested
service and enclosing app. Production requires Developer ID signing of both,
notarization, a stapled ticket and Gatekeeper acceptance. Ad-hoc qualification
does not establish that publisher trust.

Platform references: [Apple XPC service structure and privilege separation](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingXPCServices.html),
[message-bound code identity](https://developer.apple.com/forums/thread/681053).

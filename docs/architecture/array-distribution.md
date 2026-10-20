# Annex distribution through Array and the KI Store

Annex uses the existing Array native adapter (`annex`, native ID
`com.annex.desktop`) and the Store's signed public-release stream. It remains
free for personal and business use under Annex Community License 2.0. A KI
membership, payment, activation code, or commercial license server is not
required to run Annex. Paid hosting and resale require a separate written
agreement; this integration does not create one or change those terms.

## Release and installation contract

The native release jobs run `scripts/array-release.mjs --mode capture` after
packaging. Each produces `array-installation.json` containing the exact source
commit, version, native identity, installer sizes/hashes and installed executable
hashes. The final assembler requires all four target inventories, checks them
against the actual staged packages and includes them in `SHA256SUMS`.

| Target | Default Array installer | Additional direct download | Installed identity |
| --- | --- | --- | --- |
| Windows x64 | NSIS EXE | MSI | `annex-desktop.exe` |
| Linux x64 | AppImage | DEB | AppImage bytes; DEB uses `/usr/bin/annex-desktop` |
| macOS arm64 / x64 | DMG | Tauri updater archive | `Annex.app/Contents/MacOS/annex-desktop` |

Array uses one release stream per product/platform/architecture/channel, not per
installer kind. Sign and register **one** format per stream and sequence. The
default is NSIS, AppImage or DMG. `--kind msi` or `--kind deb` explicitly selects
an alternative. Signing two different payloads at the same sequence constitutes
equivocation, which Array rejects.

The Array envelope is separate from the Tauri updater signature. Its Ed25519
signature covers the exact serialized payload bytes. Array's unchanged release
verifier is pinned under `scripts/vendor/array/` to Array commit
`01985a14d48614b43f51f4847a499f5872b4bd70`; `UPSTREAM.json` records file hashes.
Tests exercise that verifier directly, including corruption, expiry, downgrade,
sequence replay, product scope and native identity rejection.

## Connect an approved public release

1. Build and qualify the release with the existing signed native workflow.
   Draft URLs cannot be signed as stable Array releases. Preserve the original
   installers, inventory, checksums and updater signatures.
2. Provision a separate Ed25519 Array release signing key. The approved public
   policy must scope its key ID exclusively to product `annex`, channel
   `stable`. Supply real artifact hosts and `productPublishers.annex`:
   `thumbprint` for Windows and `teamId` for macOS. Array requires a valid
   Authenticode signature or Developer ID/Gatekeeper verification respectively.
   Ad-hoc macOS signing and Tauri updater signing do not satisfy those checks.
3. Export the private PEM as `ANNEX_ARRAY_RELEASE_PRIVATE_KEY` and its policy key
   ID as `ANNEX_ARRAY_RELEASE_KEY_ID` in the signing environment. Do not put them
   in repository files, process arguments or the distributed app.
4. Run the signer for each target, providing the actual approved publication
   times, strictly increasing sequence and public release base URL:

   ```text
   node scripts/array-release.mjs --mode sign
     --inventory <release-directory>/array-installation-windows-x86_64.json
     --release-directory <release-directory>
     --base-url <approved-HTTPS-release-directory>
     --sequence <next-sequence>
     --published-at <ISO-UTC-publication-time>
     --expires-at <ISO-UTC-expiration-time>
     --policy <approved-Array-public-policy.json>
     --out <new-empty-envelope-directory>
   ```

   The command hashes the actual installer again, checks key/policy agreement,
   signs the payload, and verifies it with the real Array consumer before
   writing an envelope. It never generates production keys or publisher pins.
   Windows minimum version is NT `10.0.0`; macOS 11 corresponds to Darwin
   `20.0.0`, because Array compares kernel versions.
5. In the Store deployment, register each envelope using its existing
   `services/store/publish-release.mjs <envelope> <approved-policy> public`
   command and authorized database connection. Install the same approved public
   policy in Array. The artifact host must be allowed by both Array's policy
   and Store `STORE_RELEASE_HOSTS`; ticketed downloads additionally need the
   Store hostname in Array's allowed hosts.
6. Verify Store public download eligibility, Array install, launch, upgrade,
   offline launch and repair using those exact installer bytes. A contract test
   does not establish production Store deployment or native acceptance.

No paid offer, entitlement grant or issuer is necessary for this community
distribution. Catalog availability must continue to come from real,
nonwithdrawn Store release records.

## Identity recovery integration

The [credential wallet protocol](array-credential-wallet.md) is independent of
release trust and payment. Array launches a verified Annex executable and
binds its private pipes to the current wallet account and pairing. Annex
retains its own identity/proof authority.

`scripts/array-wallet-acceptance.mjs` runs the production Array broker against a
native process compiling Annex's production bridge. It requires Array's
Electron runtime, native `safeStorage`, an actual Array checkout and the
release build of `scripts/array-wallet-regressions`. Run it in a new empty
output directory with `--array-root`, `--native-client` and `--output`.
It verifies save, same-process restore, standalone offline behavior, restore
after reopening the encrypted wallet, and acknowledged disconnect without
deleting the recovery record. Account identity and recovery material are
explicit synthetic test inputs. The harness is not packaged in Annex and does
not establish cloud synchronization or installed UI acceptance.

## External production requirements

The repository's updater signing credentials do not supply Authenticode,
Developer ID/notarization or Array envelope signing authority. Array's inspected
public policy has no trusted release keys or artifact hosts. Production Store
publication, approved Array policy, native publisher credentials and installed
acceptance therefore remain distinct requirements until actual evidence closes
them. No trust check is disabled to make an unconfigured integration appear
connected.

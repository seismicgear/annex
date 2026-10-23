# Annex distribution through Array and the KI Store

Annex uses the existing Array native adapter (`annex`, native ID
`com.annex.desktop`) and the Store's signed public-release stream. It remains
free for personal and business use under Annex Community License 2.1. A KI
membership, payment, activation code, or commercial license server is not
required to run Annex. Paid hosting and resale require a separate written
agreement; this integration does not create one or change those terms.

The authoritative customer Store and commerce API belong to
[`seismicgear/kuykendall-industries`](https://github.com/seismicgear/kuykendall-industries),
in its existing Vercel website. Array is the desktop client. Its retained
`services/store` implementation is legacy/reference code and is not the
production publication target.

## Release and installation contract

The native release jobs run `scripts/array-release.mjs --mode capture` after
packaging. Each produces `array-installation.json` containing the exact source
commit, version, native identity, installer sizes/hashes and installed executable
hashes. The final assembler requires all four target inventories, checks them
against the actual staged packages and includes them in `SHA256SUMS`.
The executable is read from the final NSIS, MSI, DEB or read-only mounted DMG,
without running it. AppImage is itself the installed executable. Never hash the
shared compiler output: Tauri stamps a different bundle type into each format,
and native signing can change the bytes again. Inventory capture fails if its
native package reader or exact executable member is unavailable.

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

Annex's own updater preserves the installed format independently of Array's
default installer selection. Its feed includes `windows-x86_64-msi`,
`windows-x86_64-nsis`, `linux-x86_64-deb`, `linux-x86_64-appimage` and the two
`darwin-*-app` entries, plus compatible generic defaults. Every selected payload
requires a valid Tauri signature. The pinned Tauri CLI does not automatically
sign the DEB; the release workflow signs that exact inventoried file through
`scripts/sign-deb-updater.mjs` and verifies the result before staging the feed.
Signing metadata must never replace or rebuild an already inventoried payload.

The Array envelope is separate from the Tauri updater signature. Its Ed25519
signature covers the exact serialized payload bytes. Array's unchanged release
verifier is pinned under `scripts/vendor/array/` to Array commit
`01985a14d48614b43f51f4847a499f5872b4bd70`; `UPSTREAM.json` records file hashes.
Tests exercise that verifier directly, including corruption, expiry, downgrade,
sequence replay, product scope and native identity rejection.

## Connect an approved public release

1. Build and qualify the release with the
   [credential-gated native signing workflow](../native-release-signing.md).
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
5. In the KI website repository, use `scripts/store-publish-release.mjs` with
   `--manifest`, `--policy`, `--product annex`, `--platform`, `--arch`, and
   `--visibility public`. The default validates without a database connection.
   After approval and native acceptance, `--apply --project-ref <verified-ref>
   --actor <operator-id>` registers the release using the existing scoped
   `SUPABASE_DB_URL`; `--preview` selects the separate preview schema. Install the same approved public
   policy in Array. The artifact host must be allowed by both Array's policy
   and Store `STORE_RELEASE_HOSTS`; ticketed downloads additionally need the
   Store hostname in Array's allowed hosts.
6. Verify Store public download eligibility, Array install, launch, upgrade,
   offline launch and repair using those exact installer bytes. A contract test
   does not establish production Store deployment or native acceptance.

No paid offer, entitlement grant or issuer is necessary for this community
distribution. Catalog availability must continue to come from real,
nonwithdrawn Store release records.

The [website intake PR #370](https://github.com/seismicgear/kuykendall-industries/pull/370)
was closed without merging on October 22, 2026. Current Store main `6e027a08`
does not contain its intake command or implementation. The instructions above
describe that tested integration branch; acceptance and deployment into the
actual Store are still required. The retained implementation uses the pinned
Array verifier, stream-level transactional
locking, rollback/equivocation checks and audit attribution. Exact replay is
idempotent; withdrawn releases retain their sequence history. Existing
`/v1/releases`, catalog and one-use download routes serve the result. Download
redemption also checks current withdrawal and storage policy. Local tests use
the actual fifteen website migrations, reduced database role and HTTP handler;
ephemeral test keys do not qualify a production publisher or deployment.

[Array PR #10](https://github.com/seismicgear/Array/pull/10) fixes native Windows
lookup for the quoted `InstallLocation` written by Tauri. It preserves executable
hash and publisher verification. The change requires incorporation into the
distributed Array client; an Annex installer cannot update Array's code.

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

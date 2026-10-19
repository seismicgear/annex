# Array credential wallet integration

Annex can use the credential wallet in Array's existing Settings. When a paired
desktop instance is opened from Array, the **Export** (backup) action saves its
portable identity to Array and reports success only after Array returns a save receipt. The identity
settings dialog shows **Managed by Array**, its account label when available,
and a disconnect action. Identity setup can list and restore saved Annex
identities from the same wallet.

## Backup and recovery behavior

The payload is the identity recovery material used to generate proofs, not a
cached zero-knowledge proof or a live login session. Its format is
`annex.identity.v1` and its allowlisted fields are:

| Field | Meaning |
| --- | --- |
| `sk` | Nonzero BN254 scalar, encoded as 1–64 hexadecimal characters |
| `roleCode` | Role code from 1 through 255 |
| `nodeId` | Nonnegative JavaScript safe integer; existing identities with zero remain valid |
| `commitmentHex` | 64-character hexadecimal identity commitment |
| `pseudonymId` | Optional context pseudonym, up to 256 UTF-16 units |
| `serverSlug` | Server context, up to 512 UTF-8 bytes |
| `createdAt` | Identity creation date |

Local database IDs, session tokens, cached membership proofs and registration
state are excluded at both the JavaScript and native command boundaries. Before
importing a restored identity, Annex recomputes its Poseidon commitment and
compares it with the backup. It then uses the existing import and proof flow to
establish a fresh session. An in-flight restore cannot import keys after its
component unmounts or observes pairing revocation.

Backup labels include the server context and a short commitment prefix. The
restore selector also displays a record ID suffix, so multiple saved identities
can be distinguished. Array owns record authorization and identity deduplication.

An instance that has never used Array keeps the existing backup-file behavior.
An Array-managed instance whose wallet is unavailable shows a persistent error
with **Retry Array backup** and **Export backup file instead**. A failed or
malformed save response cannot produce a saved-to-Array success message. The
explicit file export preserves the existing file format and does not revoke an
Array pairing. Browser Annex continues using file import and export.

## Native channel and ownership

Array launches a verified installed Annex executable with inherited stdin/stdout
pipes and the marker `KI_ARRAY_WALLET_PROTOCOL=stdio-v1`. The marker contains no
credential. Annex does not open a listener or discover another process, and no
credential is transferred in arguments, URLs or environment variables. The
native primary-process setup starts the hello request eagerly, after the
single-instance plugin has selected the surviving process. Opening a second
instance cannot attach the new pipe to the already-running instance; users must
close Annex and reopen it from Array.

Each frame is `KI_ARRAY_WALLET_V1 ` followed by JSON and a newline. Requests have
`{id, method, params}`; responses have the same `id` and exactly one of `result`
or `error`. IDs are unique and requests are serialized. Requests are capped at
64 KiB; responses at 256 KiB to support all 250 permitted backup metadata rows.
Each request times out after ten seconds. EOF, correlation errors, malformed
frames and timeouts invalidate that launch's channel. Complete frame writes hold
stdout's lock; Array ignores ordinary log lines without the reserved prefix.
Remote diagnostic text and submitted key material are never returned as UI
errors or logged by this bridge.

| Method | Annex use |
| --- | --- |
| `hello` | Negotiate protocol 1 and confirm Array's Annex app scope |
| `status` | Read pairing, availability and account label |
| `recovery.put` | Save allowlisted identity material and require a credential ID and save time |
| `recovery.list` | Read scoped backup labels and record IDs |
| `recovery.get` | Read one scoped identity backup and verify its format |
| `disconnect` | Revoke pairing and require `paired: false` plus `managedBy: null` |

The app never selects a trusted `appId`; Array binds the private channel to its
verified launch and current account. The marker is only a launch hint, and a
successful hello is only protocol negotiation: the parent process and the local
operating-system account remain the trust boundary.

Annex stores an empty, non-secret `array-wallet-managed` marker in its existing
profile data directory **before the first hello**. This is a backup destination
preference, not evidence of authentication or a current remote pairing. It keeps
subsequent standalone or wallet-locked launches in a managed-but-unavailable
state, including when the initial handshake failed. Marker I/O failures remain
visible and do not silently select file export. The marker is cleared only after
an acknowledged disconnect. If clearing it fails after Array has acknowledged,
Annex retains that acknowledgement in memory so a retry can finish locally
without reusing the closed channel. If the pipe is unavailable, the user can
explicitly export a file and reopen Annex from Array to change pairing.

## Verification and boundaries

The lightweight regression crate compiles the actual production native bridge
without Tauri or GTK. It covers framing, correlation, timeouts, metadata bounds,
backup/restore/disconnect receipts, error redaction, portable field validation,
profile persistence and interrupted startup. It is also run in the existing
server CI job. Tauri command wrappers remain covered by the repository's existing
Linux and Windows desktop build jobs.

```sh
cargo test --locked --manifest-path scripts/array-wallet-regressions/Cargo.toml
cargo clippy --locked --manifest-path scripts/array-wallet-regressions/Cargo.toml --all-targets -- -D warnings
cd client
npx vitest run src/lib/array-wallet.test.ts src/components/StatusBar.wallet.test.tsx src/components/ArrayWallet.test.tsx src/components/StatusBar.test.tsx src/components/IdentitySetup.recovery.test.tsx src/components/IdentitySettings.visibility.test.tsx
npx tsc -b
npx tsc -p tsconfig.test.json
```

This adapter provides Annex identity backup and recovery through Array. It does
not add an Annex provider API key settings flow, KI account authentication, cloud
synchronization, or proof of cross-device account login. Those capabilities need
their corresponding Array/KI services and app integrations. A desktop build and
the tests above do not establish packaged app acceptance on user devices.

# ADR 0012 — Migration integrity (checksums + duplicate-ordinal detection)

Status: Accepted (2026-05-12)
Context tag: `hardening-pass`

## Context

`_annex_migrations` previously recorded `(name, applied_at)` only. The runner skipped any migration whose name was present, so:

1. **Silent edits.** An edit to a committed migration's SQL after it had been applied somewhere was undetectable. Invariant I-DB-1 in `docs/refactor/invariants.md` says "no tooling enforcement yet — humans must hold the line." The reviewer surfaced this as exactly the kind of long-term invariant decay the project does not want.
2. **Duplicate ordinals.** Two contributors could both create a `037_*.sql` on their branches. Whichever merged second would get its file in the embedded list, but only one of the two could ever apply (UNIQUE on name would not catch the ordinal collision; only one would be at position 037 in lexicographic order, and the other would just *exist* without enforcement).

## Decision

1. **Schema** (migration `039_migration_checksums.sql`) — `ALTER TABLE _annex_migrations ADD COLUMN sha256_hex TEXT; ALTER TABLE _annex_migrations ADD COLUMN ordinal INTEGER;`.
2. **Boot-time duplicate scan** — the runner walks the embedded `MIGRATIONS` slice before any DB work, parses the leading `NNN` of each name, and errors with `MigrationError::DuplicateOrdinal` if two share an ordinal.
3. **Boot-time integrity check** — for every embedded migration:
   - If the row exists and has a recorded `sha256_hex`, compare against SHA-256 of the embedded SQL. Mismatch → `MigrationError::ChecksumMismatch`.
   - If the row exists with `NULL sha256_hex`, backfill the recorded value with the embedded SHA-256 (treating embedded source as the trusted baseline). After backfill, subsequent edits trip the gate.
4. **Apply path** — when applying a new migration, record `(name, sha256_hex, ordinal)` together in the same transaction as the migration's effects.

## Consequences

- Editing a committed migration after deploy is detected on next boot rather than silently accepted.
- Two contributors landing the same ordinal is caught at compile/test time, not in production.
- Existing deployments self-upgrade on first boot under migration 039: their rows get their `sha256_hex` backfilled, then any further edit is caught.

## Out of scope (deferred)

- **Pre-commit hook** to refuse committing edits to applied migrations. Useful, but it lives in tooling rather than runtime and is a separate change.
- **Cross-deployment hash agreement.** Each deployment computes its own baseline at backfill time. If two deployments computed different baselines because someone edited the migration *between* deploys, both would think their own SQL is canonical. The protection is "an *edit after this server started recording* is detected" — which is the property the invariant doc was claiming all along.

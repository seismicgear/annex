# Agent Playbook for Annex

This is a contract for any AI agent (and any human) running an Annex coding
task. It exists because the project has cryptographic invariants and a
release matrix that are easy to break with well-intentioned drive-by edits.

## Core rules

1. **Read** `architecture-map.md` and `invariants.md` before touching code that crosses crate boundaries.
2. **Plan in writing** before editing. State the minimum file set you intend to touch and why. If you need more files, update the plan.
3. **Make the smallest coherent change.** Resist the temptation to clean up surrounding code, rename, refactor, or "modernize". One task = one diff.
4. **Run the gates** for the surface you touched (see `release-gates.md`). Don't ship without them.
5. **Never weaken security to make tests pass.** If a test fails because of an invariant, the invariant wins; either provide a real fixture (e.g. a real proof) or revise the task scope.
6. **Never disable hooks, signing, or signature checks** to get unblocked.

## Strict task template

Every task — large or small — is described using this exact structure. Copy
it verbatim into your work plan; fill every field. If a field doesn't apply,
write "n/a" — don't omit it.

```
# Task: <short imperative title>

## Goal
<one paragraph: what state of the world must exist when this is done?>

## Allowed files
<absolute or workspace-relative globs; nothing outside this list will be edited>
- crates/<crate>/src/<file>.rs
- client/src/<area>/<file>.tsx
- ...

## Forbidden files
<explicit list of files that look related but must not be changed>
- crates/annex-db/src/migrations/**     # never edit a published migration
- crates/annex-server/src/api_ws.rs     # WS protocol shape stable; out of scope
- ...

## Behavior allowed to change
<bullets — observable behavior that is in scope to change>
- Add a new optional `audience` field to the invite POST body.
- Surface a "code expired" error in the UI.

## Behavior forbidden to change
<bullets — observable behavior that is NOT in scope and must remain identical>
- WS frame shapes.
- Membership-proof public-signal layout `[root, commitment]`.
- Migration sequencing.
- Default value of `enforce_zk_proofs`.
- Anything in `invariants.md` not explicitly waived for this task.

## Tests required
<bullets; these must pass before the task is "done">
- `cargo fmt --all --check`
- `cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings`
- `cargo test --workspace --exclude annex-desktop --no-fail-fast`
- `npm --prefix client run lint`
- `npm --prefix client test -- --run`
- `npm --prefix client run build`
- (when desktop is touched) `cargo build -p annex-desktop --release`
- (when ZK is touched) `node zk/scripts/test-proofs.js`

## Done definition
<unambiguous, checkable list — no "looks good", no "should work">
- All tests in "Tests required" exit 0.
- New behavior is exercised by at least one test.
- No public type or wire shape changed unless this task lists it under "Behavior allowed to change".
- A short PR description summarizing user-visible effects.
```

## Annex-specific guardrails

These apply to **every** task, regardless of what's in the template above.

- **Never** introduce dummy cryptographic artifacts in a production code path. `generate_dummy_vkey()` is a dev fallback only — see I-ZK-2 in `invariants.md`.
- **Never** bypass `enforce_zk_proofs`. If a test requires it disabled, the test must construct an `AppState` with the flag explicitly false and label itself as such.
- **Never** edit a committed SQL migration in `crates/annex-db/src/migrations/`. Add a new file with the next number — see I-DB-1.
- **Never** delete macOS bundle assets, Entitlements, Info.plist, or the macos matrix entries — see I-DESKTOP-2.
- **Never** add `--no-verify`, `--no-gpg-sign`, or any other signing/hook bypass to git commands without an explicit, written authorization in the task.
- **Never** rotate the Merkle root, regenerate ZK keys, or reset state in a production codepath without a documented epoch model — see `zk-merkle-production.md`.
- **Never** rename or remove a WS frame field consumed by `client/src/lib/ws.ts` without a paired client+server change in the same diff and a parser-shim entry.
- **Don't** add new files at the workspace root. Source goes under `crates/`, `client/`, or `zk/`. Docs go under `docs/`. Tests go alongside the code they exercise.

## Workflow

The expected loop for any task:

1. **Read** `CLAUDE.md`, then `architecture-map.md`, then the relevant section of `invariants.md`.
2. **Locate** the smallest set of files that satisfy the goal. Use `Explore`/grep — do not read entire crates speculatively.
3. **Write the task plan** using the template above. Pin it in the PR description or as a sticky note in the chat session.
4. **Edit.** Keep diffs tight. Avoid touching files outside "Allowed files".
5. **Run targeted tests first** (`cargo test -p <crate>`, `npx vitest run <file>`) for fast feedback.
6. **Run the full gate set** for the surface (see `release-gates.md`).
7. **Self-review the diff** against the "Behavior forbidden to change" list. If the diff touches one of those lines for any reason, stop and revise.
8. **Open a PR** with the task plan in the description. Link to the relevant invariant IDs (e.g. `I-ZK-3`).

## Asking for help vs. acting

These actions are reversible and can be done without confirmation:
- Editing files inside the "Allowed files" list.
- Running tests, fmt, clippy, lint, npm scripts, ZK scripts.
- Creating new files inside `Allowed files`.

These actions are NOT reversible and require explicit user confirmation:
- `git push`, `git push --force`, deleting branches.
- Creating, merging, or closing GitHub PRs.
- Rotating ZK keys (`setup-groth16.js` over an existing key).
- Editing `.github/workflows/*` to disable a job.
- Editing files outside the "Allowed files" list.

## Handing off / pausing

If you have to stop mid-task — context budget, a question, a blocker — leave
behind:

- A summary of what changed and what didn't, in plain prose.
- The exact commands you ran and their exit codes.
- The next step you would have taken.
- Any new failing tests with the failure snippet.

Do not pretend a task is done if any required test failed. Mark it
explicitly "blocked on <X>" and stop.

## Definition of "done"

A task is done when, **simultaneously**:

- Every gate in "Tests required" exits 0.
- The diff is contained inside "Allowed files".
- No "Behavior forbidden to change" item is observably different.
- A reviewer reading only the PR title + description + diff can tell what changed and why.
- No invariant from `invariants.md` is weakened.

If any of those is "almost true," the task is **not** done.

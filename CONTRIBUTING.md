# Contributing to Annex

Contributions are welcome: reproducible bug reports, clearer documentation,
accessibility improvements, operator feedback, integrations, and code. Start
with [the product guide](README.md), [architecture](docs/architecture.md), and
[the roadmap](ROADMAP.md) so your change fits what is implemented and what is planned.

## Before you change code

- Search existing issues and PRs. Describe the user-visible problem and a small,
  reviewable solution. Discuss changes to identity, trust, wire formats, storage,
  or licensing before undertaking a broad redesign.
- Read `AGENTS.md` and `CLAUDE.md`, including the applicable repository workflow.
  `FOUNDATIONS.md` records the architectural principles; preserve their meaning.
- Use a branch from current `main`. When reproducing a published binary problem,
  use its exact tag (`v0.1.0` or `v0.1.1`) and say which revision you tested.
- Follow [development setup](docs/development.md). Install from lockfiles with
  `npm ci`; prepare the pinned ZK artifacts and alignment model before running
  real proof, backend, or browser tests.

## Good bug reports

Include the version/commit, OS and architecture, installation method, exact
steps, expected and actual behavior, and a small sanitized log or screenshot.
For networking issues, describe whether the server is local or remote and
whether HTTPS/ICE/TURN are configured. Do not include private messages, identity
secrets, invite codes, recovery material, access tokens, or unredacted database
files. Report vulnerabilities privately through [SECURITY.md](SECURITY.md).

## Implementation expectations

Keep changes focused. Preserve authorization checks at the point of action,
cryptographic domain separation, input bounds, durable retry behavior, and
truthful failure states. Do not bypass a startup gate or weaken a proof/signature
check to make a test pass. Version protocol/storage changes deliberately, and
keep older-data behavior explicit.

Obtain explicit maintainer approval before changing visual design or user flows;
release-readiness work does not authorize a redesign. For UI work, use the
existing modal and focus primitives, accessible names,
keyboard navigation, and responsive layouts. Update the audit surface manifest
when adding a reachable surface. Review snapshots visually; record intentional
baseline changes separately rather than accepting every diff automatically.

For docs, check claims against implementation and reproducible evidence. Distinguish:

1. Implemented behavior with a cited source or test.
2. Verified behavior with the platform, version, and test context stated.
3. Known limitations or incomplete acceptance.
4. Future architectural goals and their remaining criteria.

Keep `ROADMAP.md`'s status summary, phase status, criteria, and dated status
changes consistent. Preserve criteria that are still needed; do not erase
unfinished architecture to make the release look complete.

## Verification and pull requests

Run the relevant checks from [development](docs/development.md). For a backend
change, exercise both the intended path and the permission/failure boundary it
affects. For a protocol change, use real proof/signature/persistence integration
tests, not only mocked success. For a website or docs change, check links,
commands, keyboard access, and the user journey it describes.

A PR should state:

- The problem, resulting behavior, and scope.
- The implementation or document evidence supporting its claims.
- Commands/tests run, their results, and any skips or environmental limitations.
- Compatibility, migrations, deployment changes, or remaining acceptance work.

Passing compilation is not runtime acceptance. A browser test is not an installed
desktop test. Leave the existing CI gates enabled and fix genuine failures. Do
not attach sensitive credentials or private user content to artifacts. Maintainers
review and merge contributions; opening a PR does not authorize a release or
rewrite of published artifacts.

## Contribution ownership and assignment to KI

**Original contributions accepted into Annex must be assigned to Kuykendall
Industries LLC.** Read and sign the
[KI Contributor Assignment Agreement, version 1.0](CONTRIBUTOR_ASSIGNMENT.md)
for your contribution before merge.

Under that agreement, you assign your rights in the original contribution to
KI. KI may use, modify, combine, distribute, sell, host, sublicense, and
relicense it in Annex or any other current or future project, including
proprietary products and reseller offerings, without further permission or
compensation. Contributing does not give you ownership or licensing control
over KI's other code or projects.

Complete the assignment declaration in
[the PR template](.github/PULL_REQUEST_TEMPLATE.md). Each rightsholder, or an
authorized representative, must sign from their own account and identify the
agreement revision and covered work. Work already owned by KI or covered by an
existing written assignment can identify that basis instead of signing again.
A generic commit sign-off does not replace this assignment.

Identify any third-party code, dependencies, copied or generated material, and
applicable terms. Do not submit employer/client material without authority.
Material that would restrict KI's commercial reuse or impose licensing or
source-disclosure obligations on other KI code requires KI's express written
approval before acceptance.

Maintainers must confirm assignment coverage for every contributing
rightsholder and review disclosed third-party terms before merging. Reject or
replace contributions that do not satisfy these requirements.

Annex is distributed under the [Annex Community License 2.1](LICENSE.md), which
allows commercial use and modifications while requiring KI's separate
agreement for resale or paid hosted access. These distribution conditions do
not limit KI's rights in assigned contributions. See
[the license guide](docs/licensing.md).

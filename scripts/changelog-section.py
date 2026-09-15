#!/usr/bin/env python3
"""Extract one version's section from a Keep-a-Changelog file.

The release workflow passed `generate_release_notes: true` and nothing else, so
every release body was a list of commit subjects — while a hand-written
`## [0.1.0]` section sat in CHANGELOG.md, unused. The changelog is the place the
reasoning lives; a commit list is what you read when there isn't one.

Usage: changelog-section.py CHANGELOG.md 0.1.0 [> RELEASE_NOTES.md]

Exit 1 with a message on stderr when the version has no section, so a release
cannot ship notes for a version nobody wrote about. `version-sync.test.sh`
already asserts the section exists; this is the same fact enforced at the point
of use.
"""
import re
import sys


def extract(text: str, version: str) -> str | None:
    # `## [0.1.0]` or `## [0.1.0] — 2026-09-15`, and nothing that merely starts
    # with the same digits: `[0.1.0]` must not match `[0.1.01]`.
    start = re.compile(r"^##\s+\[" + re.escape(version) + r"\](\s|$)")
    nxt = re.compile(r"^##\s+\[")
    lines = text.splitlines()
    out: list[str] = []
    collecting = False
    for line in lines:
        if collecting and nxt.match(line):
            break
        if collecting:
            out.append(line)
            continue
        if start.match(line):
            collecting = True
    if not collecting:
        return None
    # Trim leading and trailing blank lines; keep the interior verbatim.
    while out and not out[0].strip():
        out.pop(0)
    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out)


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    path, version = argv[1], argv[2]
    with open(path, encoding="utf-8") as fh:
        text = fh.read()
    section = extract(text, version)
    if section is None:
        print(
            f"error: {path} has no '## [{version}]' section, so this release has "
            f"no written notes. Add one before tagging.",
            file=sys.stderr,
        )
        return 1
    if not section.strip():
        print(
            f"error: the '## [{version}]' section in {path} is empty.",
            file=sys.stderr,
        )
        return 1
    print(section)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

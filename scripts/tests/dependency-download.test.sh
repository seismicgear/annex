#!/usr/bin/env bash
set -euo pipefail

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
node --test "$repo_root/scripts/tests/dependency-download.test.mjs"

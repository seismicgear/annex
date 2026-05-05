#!/usr/bin/env node
// setup-groth16.js — Compatibility shim.
//
// The historical entry point for the local Groth16 trusted setup. The
// production / dev split now lives in two scripts:
//
//   - zk/scripts/dev-setup-groth16.js   (random-entropy, dev fixtures only)
//   - zk/scripts/verify-artifacts.js    (verifies pinned artifacts in prod)
//
// Production builds MUST NOT invoke this script. It is preserved only so
// that existing dev workflows (CI lanes, scripts/claude-setup.sh,
// crates/annex-identity/tests/common.rs) that already run setup-groth16.js
// keep working — but they now pass through dev-setup-groth16.js, which:
//
//   - prints an explicit DEV-ONLY banner
//   - refuses to run when ANNEX_BUILD_PROFILE=production|release
//
// See docs/refactor/zk-merkle-production.md.

"use strict";

const path = require("path");

process.stdout.write(
  "[setup-groth16] NOTE: this is a compatibility shim. " +
    "Delegating to dev-setup-groth16.js (dev-only).\n"
);

require(path.resolve(__dirname, "dev-setup-groth16.js"));

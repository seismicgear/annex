# ADR 0006: ZKP Circuit Audit

**Status**: Accepted
**Date**: 2026-02-18
**Phase**: 12 (Hardening & Audit)

## Context

Phase 12.1 requires an audit of all Circom circuits for soundness, invalid witness rejection, trusted setup reproducibility, and documentation of assumptions.

## Circuits Audited

### identity.circom

Computes `commitment = Poseidon(sk, roleCode, nodeId)`.

- **Inputs**: sk (private), roleCode (private), nodeId (private)
- **Outputs**: commitment (public)
- **Assessment**: Sound. Uses standard circomlib Poseidon(3). All signals properly constrained. No underconstraining.

### membership.circom

Proves membership in a Merkle tree of depth 20 (1M leaves max).

- **Inputs**: sk, roleCode, nodeId, leafIndex, pathElements[20], pathIndexBits[20] (all private)
- **Outputs**: root, commitment (both public)
- **Assessment**: Sound. Recomputes identity commitment, verifies Merkle path with correct left/right ordering, constrains leafIndex bits via Num2Bits to match pathIndexBits.

## Soundness Verification

### Hash ordering consistency

The circuit's Merkle path computation uses a mathematical selection trick:
- `pathIndexBits[i] == 0`: hash(currentHash, sibling) — leaf is left child
- `pathIndexBits[i] == 1`: hash(sibling, currentHash) — leaf is right child

This is consistent with the Rust-side Merkle tree in `annex-identity/src/merkle.rs`, which uses `current_idx & 1` with the same semantics. Both implementations produce identical roots for identical trees.

### leafIndex constraint

The circuit constrains `Num2Bits(leafIndex)` to equal `pathIndexBits` at every level. This prevents an attacker from claiming a different leaf position than the path describes. The `Num2Bits` decomposition is little-endian (LSB at index 0), matching the bottom-to-top path ordering.

## Invalid Witness Testing

16 tests pass in `zk/scripts/test-proofs.js`:

| Test | Result |
|------|--------|
| Valid identity proof verifies | PASS |
| Commitment matches expected Poseidon output | PASS |
| Corrupted identity proof is rejected | PASS |
| Tampered public signal is rejected | PASS |
| Different sk produces different commitment | PASS |
| Different roleCode produces different commitment | PASS |
| Different nodeId produces different commitment | PASS |
| Valid membership proof (index 0) verifies | PASS |
| Root matches expected value | PASS |
| Commitment matches in membership proof | PASS |
| Valid membership proof (index 1) verifies | PASS |
| Root matches for index 1 | PASS |
| Corrupted membership proof is rejected | PASS |
| Tampered root is rejected | PASS |
| Tampered commitment is rejected | PASS |
| Mismatched leafIndex/pathIndexBits fails witness gen | PASS |

## Trusted Setup

The Groth16 trusted setup (`zk/scripts/setup-groth16.js`) is **deterministic for contributions** (fixed entropy strings) but **not fully reproducible** because the initial Powers of Tau generation uses OS randomness.

### Limitations

1. **Entropy**: Phase 1 and Phase 2 contributions use hardcoded strings (`"random text"`, `"more entropy"`). This is deterministic but not cryptographically strong. For production deployment, a multi-party ceremony with proper randomness should be conducted.
2. **Reproducibility**: The initial `powersoftau new` command uses internal OS randomness. Subsequent runs produce different ceremony files. The existing keys should be treated as the canonical keys and not regenerated.
3. **Single-party**: The current setup is single-party. A multi-party ceremony would provide stronger security guarantees (toxic waste is only compromised if all participants collude).

## Assumptions and Limitations

1. **BN254 security**: The system relies on the discrete log assumption over BN254. This is currently considered secure but has a lower security margin than BLS12-381.
2. **Poseidon parameters**: Uses circomlib's default Poseidon parameters. These are the same parameters used by Tornado Cash, Semaphore, and other production systems.
3. **Depth 20**: The Merkle tree supports 2^20 = 1,048,576 leaves. Exceeding this capacity requires a circuit recompile with increased depth.
4. **No nullifier in circuit**: Nullifier derivation (SHA-256 based) happens off-circuit in Rust. This is a design choice — it allows flexible nullifier schemes without circuit changes, but means nullifier correctness relies on the server implementation rather than ZK constraints.
5. **Client-side proof generation**: Proofs are generated on the client using snarkjs WASM. The ~5MB zkey file must be transferred to the client. This is a one-time download.

## Consequences

- All identified test gaps have been filled with negative tests
- Setup limitations are documented for production planning
- No soundness bugs were found in circuit logic
- Hash ordering between circuit and Rust implementations is verified consistent

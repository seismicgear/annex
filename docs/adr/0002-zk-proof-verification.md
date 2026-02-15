# ADR 0002: ZK Proof Verification Architecture

## Status

Accepted

## Context

The Annex identity plane relies on Zero-Knowledge Proofs (ZKPs) for membership verification and anonymous identity management. The ZK circuits are implemented in Circom and compiled to R1CS and WASM. We need a way to generate and verify these proofs within the Rust-based `annex-server`.

There are several options for ZK proof handling in Rust:
1.  **Native Rust Prover/Verifier**: Use `ark-groth16` and `ark-circom` to generate witnesses and proofs entirely in Rust.
2.  **Hybrid Approach**: Use `snarkjs` (via CLI or node runtime) to generate proofs (client-side or dev/test) and `ark-groth16` to verify proofs (server-side).
3.  **FFI / Bindings**: Use `snarkjs` via `wasm-bindgen` or other FFI mechanisms.

The `annex-identity` crate currently uses `ark-bn254` (v0.5.0) and `light-poseidon` (v0.4.0) for cryptographic primitives.

## Decision

We will use a **Hybrid Approach**:

1.  **Verification**: We will use `ark-groth16` (v0.5.0) in the `annex-identity` crate to verify Groth16 proofs. This provides:
    *   **Performance**: Native Rust verification is significantly faster than spawning a node process or using WASM in a non-native runtime.
    *   **Type Safety**: Leveraging `arkworks` types ensures correct curve operations and field arithmetic.
    *   **Security**: Minimal external dependencies at runtime compared to invoking a shell command.

2.  **Proof Generation**: For testing and development purposes (and potentially for future CLI tools), we will use the `snarkjs` CLI tool wrapped via `std::process::Command`.
    *   **Simplicity**: Avoids the complexity of integrating `ark-circom` witness generation which can be fragile with circuit updates and dependency versions.
    *   **Consistency**: Ensures that proofs generated are identical to those produced by the standard Circom/SnarkJS workflow used in the JS/WASM client (Phase 11).

## Consequences

### Positive
*   Server verification is fast and robust.
*   We avoid complex Rust-WASM bindings for witness generation in the server core.
*   We maintain compatibility with the extensive `snarkjs` ecosystem.

### Negative
*   Development environment requires `node` and `snarkjs` to be installed for running tests that involve proof generation.
*   We must implement JSON parsing logic to convert `snarkjs` output formats (proof, public signals, verification key) into `ark-groth16` compatible structures.

### Compliance
*   This approach satisfies the ROADMAP requirement to "Implement proof verification ... in Rust".
*   It satisfies the requirement to "Implement proof generation ... via snarkjs ... or native Rust".

## Technical Details

*   **Dependencies**: `ark-groth16`, `ark-serialize`, `ark-bn254`, `serde_json`.
*   **Data Flow**:
    1.  Circom compiles circuits to `.r1cs` and `.wasm`.
    2.  `snarkjs` performs trusted setup and exports `.zkey` and `verification_key.json`.
    3.  `annex-identity` loads `verification_key.json` at startup (or compile time).
    4.  Client (or test harness) generates proof using `snarkjs` (or JS client).
    5.  Server receives proof JSON, parses it into `ark_groth16::Proof`, and verifies it against the loaded key.

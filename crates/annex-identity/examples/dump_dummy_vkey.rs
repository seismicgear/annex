//! Emit a dummy Groth16 membership verifying key in the snarkjs JSON shape that
//! `annex_server` loads at startup (`zk/keys/membership_vkey.json` or the path in
//! `ANNEX_ZK_KEY_PATH`).
//!
//! The dummy key is built from BN254 generator points: it is mathematically
//! valid (on-curve, correct subgroup) so the strict `parse_verification_key`
//! loader accepts it, but it is useless for real proof verification. This lets a
//! developer (or a federation e2e harness) boot a real `annex-server` for
//! API-level testing — e.g. the VRP agent handshake, which does not verify ZK
//! proofs — without running the full circom/snarkjs trusted-setup pipeline.
//!
//! Usage:
//!   cargo run -p annex-identity --example dump_dummy_vkey > /path/to/membership_vkey.json
//!
//! Never use this on a production path: `annex-server` refuses to start in
//! enforced-ZK mode when the loaded key is the dummy (see `is_dummy_vkey`).

fn main() {
    let vk = annex_identity::zk::generate_dummy_vkey();
    let json = annex_identity::zk::serialize_vkey_to_snarkjs_json(&vk);
    print!("{json}");
}

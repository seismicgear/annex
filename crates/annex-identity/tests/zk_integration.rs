use annex_identity::zk::{parse_proof, parse_public_signals, parse_verification_key, verify_proof};
use ark_bn254::Fr;
use ark_ff::Field;
use serde_json::json;

// This module declaration will look for `tests/common.rs` or `tests/common/mod.rs`.
// Since we have `tests/common.rs`, it works, but `common.rs` is also compiled as a test binary.
// To avoid that warnings/duplicate compilation, usually `tests/common/mod.rs` is preferred and NOT having `tests/common.rs`.
// But for now this is fine.
mod common;
use common::{generate_proof, get_verification_key};

#[test]
fn test_identity_commitment_proof_verification() {
    let sk = "123456789";
    let role_code = "1";
    let node_id = "42";

    let input = json!({
        "sk": sk,
        "roleCode": role_code,
        "nodeId": node_id
    });

    println!("Generating proof...");
    let (proof_json, public_json) = generate_proof("identity", &input);
    println!("Proof generated.");

    // Parse proof and public signals
    let proof = parse_proof(&proof_json.to_string()).expect("failed to parse proof");
    let public_signals =
        parse_public_signals(&public_json.to_string()).expect("failed to parse public signals");

    // Load verification key
    let vkey_json = get_verification_key("identity");
    let vkey = parse_verification_key(&vkey_json).expect("failed to parse verification key");

    // Verify valid proof
    let result = verify_proof(&vkey, &proof, &public_signals);
    assert!(
        result.is_ok(),
        "verification failed with error: {:?}",
        result.err()
    );
    assert!(result.unwrap(), "proof verification returned false");

    // Tamper with public input
    // public_signals[0] is the commitment.
    let mut tampered_signals = public_signals.clone();
    tampered_signals[0] += Fr::ONE; // Add 1 to commitment

    // Verify invalid proof
    let result = verify_proof(&vkey, &proof, &tampered_signals);
    // Verification should return Ok(false) or Err(VerificationFailed)?
    // Arkworks verify returns Ok(false) if proof is invalid but well-formed.
    // If malformed, Err.
    // My wrapper returns Result<bool, ZkError>.
    if let Ok(valid) = result {
        assert!(!valid, "tampered proof verified successfully!");
    } else {
        // If it returns error, that's also fine (e.g. malformed inputs), but usually it returns Ok(false).
        println!(
            "Tampered proof verification returned error: {:?}",
            result.err()
        );
    }
}

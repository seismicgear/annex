//! Independent identity-to-device ownership. The actual membership_v2 proof
//! commits to the certificate digest through its constrained challenge signal.
//! This static proof is never an authentication challenge or group admission.

use annex_identity::zk::{self, Bn254, Fr, VerifyingKey};
use ed25519_dalek::{Signature, VerifyingKey as DeviceVerifyingKey};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    services::e2e_service::{self, E2eError},
    AppState,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceCertificate {
    pub version: u8,
    pub server_identity: String,
    pub topic: String,
    pub pseudonym_id: String,
    pub device_id: String,
    pub x25519_pub_hex: String,
    pub ed25519_pub_hex: String,
    pub root_hex: String,
    pub commitment_hex: String,
    pub nullifier_hex: String,
    pub generation: u64,
    pub previous_certificate_sha256: Option<String>,
    pub proof: Value,
    pub public_signals: Vec<String>,
    pub signature_hex: String,
}

fn invalid() -> E2eError {
    E2eError::BadRequest("device certificate context or proof is invalid".into())
}

fn canonical_proof(proof: &Value) -> bool {
    const FQ: &str =
        "21888242871839275222246405745257275088696311157297823662689037894645226208583";
    let coordinate = |value: &Value| {
        value.as_str().is_some_and(|value| {
            !value.is_empty()
                && value.len() <= FQ.len()
                && value.bytes().all(|b| b.is_ascii_digit())
                && (value == "0" || !value.starts_with('0'))
                && (value.len() < FQ.len() || value < FQ)
        })
    };
    let g1 = |value: &Value| {
        value
            .as_array()
            .is_some_and(|v| v.len() == 3 && coordinate(&v[0]) && coordinate(&v[1]) && v[2] == "1")
    };
    let pair = |value: &Value| {
        value
            .as_array()
            .is_some_and(|v| v.len() == 2 && coordinate(&v[0]) && coordinate(&v[1]))
    };
    proof["protocol"] == "groth16"
        && proof["curve"] == "bn128"
        && g1(&proof["pi_a"])
        && g1(&proof["pi_c"])
        && proof["pi_b"].as_array().is_some_and(|v| {
            v.len() == 3 && pair(&v[0]) && pair(&v[1]) && v[2] == json!(["1", "0"])
        })
}

pub fn server_identity(state: &AppState) -> String {
    format!(
        "annex-server:{}",
        hex::encode(Sha256::digest(state.signing_key.verifying_key().to_bytes()))
    )
}

pub fn canonical_body(certificate: &DeviceCertificate) -> Vec<u8> {
    // Values are JSON scalars/arrays only; serialization cannot fail.
    json!([
        "annex-device-certificate-v1",
        certificate.server_identity,
        certificate.topic,
        certificate.pseudonym_id,
        certificate.device_id,
        certificate.x25519_pub_hex,
        certificate.ed25519_pub_hex,
        certificate.root_hex,
        certificate.commitment_hex,
        certificate.nullifier_hex,
        certificate.generation,
        certificate.previous_certificate_sha256,
    ])
    .to_string()
    .into_bytes()
}

pub fn certificate_digest(certificate: &DeviceCertificate) -> String {
    hex::encode(Sha256::digest(canonical_body(certificate)))
}

pub fn certificate_challenge(certificate: &DeviceCertificate) -> Fr {
    // Field arithmetic implements big-endian SHA256 reduction without adding
    // an alternate integer parser or accepting noncanonical wire encodings.
    Sha256::digest(canonical_body(certificate))
        .iter()
        .fold(Fr::from(0u64), |value, byte| {
            value * Fr::from(256u64) + Fr::from(u64::from(*byte))
        })
}

pub fn possession_payload(certificate: &DeviceCertificate) -> Vec<u8> {
    json!([
        "annex-device-possession-v1",
        certificate_digest(certificate)
    ])
    .to_string()
    .into_bytes()
}

/// Fingerprint the actual loaded key, not a potentially changed file on disk.
/// JavaScript computes the same canonical point arrays from its shipped vkey.
pub fn verifier_fingerprint(key: &VerifyingKey<Bn254>) -> String {
    let g1 = |point: &zk::G1Affine| json!([point.x.to_string(), point.y.to_string()]);
    let g2 = |point: &zk::G2Affine| {
        json!([
            [point.x.c0.to_string(), point.x.c1.to_string()],
            [point.y.c0.to_string(), point.y.c1.to_string()],
        ])
    };
    let body = json!([
        "annex-membership-v2-verifier-v1",
        g1(&key.alpha_g1),
        g2(&key.beta_g2),
        g2(&key.gamma_g2),
        g2(&key.delta_g2),
        key.gamma_abc_g1.iter().map(g1).collect::<Vec<_>>(),
    ]);
    hex::encode(Sha256::digest(body.to_string().as_bytes()))
}

pub fn verify_certificate(
    certificate: &DeviceCertificate,
    expected_server_identity: &str,
    expected_pseudonym: &str,
    vkey: &VerifyingKey<Bn254>,
) -> Result<String, E2eError> {
    if certificate.version != 1
        || certificate.server_identity != expected_server_identity
        || certificate.pseudonym_id != expected_pseudonym
        || certificate.topic.is_empty()
        || certificate.topic.len() > 2048
        || certificate.generation == 0
        || certificate.generation > 9_007_199_254_740_991
        || (certificate.generation == 1) != certificate.previous_certificate_sha256.is_none()
        || certificate
            .previous_certificate_sha256
            .as_ref()
            .is_some_and(|digest| !e2e_service::lower_hex(digest))
        || certificate.public_signals.len() != 5
        || vkey.gamma_abc_g1.len() != 6
        || !canonical_proof(&certificate.proof)
        || serde_json::to_vec(certificate)
            .map_err(|_| invalid())?
            .len()
            > 16 * 1024
    {
        return Err(invalid());
    }
    if e2e_service::device_id(&certificate.x25519_pub_hex)? != certificate.device_id
        || annex_identity::derive_pseudonym_id(&certificate.topic, &certificate.nullifier_hex)
            .map_err(|_| invalid())?
            != certificate.pseudonym_id
        || !e2e_service::lower_hex(&certificate.ed25519_pub_hex)
        || certificate.signature_hex.len() != 128
        || certificate
            .signature_hex
            .bytes()
            .any(|byte| !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(invalid());
    }
    let signals: Vec<Fr> = certificate
        .public_signals
        .iter()
        .map(|value| zk::parse_canonical_fr_hex(value).map_err(|_| invalid()))
        .collect::<Result<_, _>>()?;
    let expected = [
        zk::parse_canonical_fr_hex(&certificate.root_hex).map_err(|_| invalid())?,
        zk::parse_canonical_fr_hex(&certificate.commitment_hex).map_err(|_| invalid())?,
        zk::parse_canonical_fr_hex(&certificate.nullifier_hex).map_err(|_| invalid())?,
        zk::topic_hash_for_v2(&certificate.topic).map_err(|_| invalid())?,
        certificate_challenge(certificate),
    ];
    if signals.as_slice() != expected {
        return Err(invalid());
    }
    let public_bytes: [u8; 32] = hex::decode(&certificate.ed25519_pub_hex)
        .map_err(|_| invalid())?
        .try_into()
        .map_err(|_| invalid())?;
    let key = DeviceVerifyingKey::from_bytes(&public_bytes).map_err(|_| invalid())?;
    if key.is_weak() {
        return Err(invalid());
    }
    let signature =
        Signature::from_slice(&hex::decode(&certificate.signature_hex).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    key.verify_strict(&possession_payload(certificate), &signature)
        .map_err(|_| invalid())?;
    let proof = zk::parse_proof(&certificate.proof.to_string()).map_err(|_| invalid())?;
    if !zk::verify_proof(vkey, &proof, &signals).map_err(|_| invalid())? {
        return Err(invalid());
    }
    Ok(certificate_digest(certificate))
}

/// Call within the key publication's authenticated IMMEDIATE transaction.
/// Registered identity/root/lineage checks are admission policy, separate from
/// the pure verifier that independently trusted clients also implement.
pub fn validate_publication(
    conn: &Connection,
    state: &AppState,
    pseudonym: &str,
    certificate: &DeviceCertificate,
) -> Result<String, E2eError> {
    let vkey = state.membership_vkey_v2.as_ref().ok_or_else(|| {
        E2eError::Forbidden("device certificates require the real membership_v2 verifier".into())
    })?;
    let digest = verify_certificate(certificate, &server_identity(state), pseudonym, vkey)?;
    let registered: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM zk_nullifiers WHERE topic=?1 AND nullifier_hex=?2 AND pseudonym_id=?3 AND commitment_hex=?4)",
        params![certificate.topic, certificate.nullifier_hex, pseudonym, certificate.commitment_hex], |row| row.get(0),
    )?;
    let known_root: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM vrp_roots WHERE root_hex=?1)",
        [&certificate.root_hex],
        |row| row.get(0),
    )?;
    if !registered || !known_root {
        return Err(E2eError::Forbidden(
            "certificate is not bound to this registered identity and registry".into(),
        ));
    }
    if let Some(previous) = &certificate.previous_certificate_sha256 {
        let prior: Option<String> = conn.query_row(
            "SELECT device_certificate FROM member_device_keys WHERE server_id=?1 AND pseudonym_id=?2 AND certificate_sha256=?3",
            params![state.server_id, pseudonym, previous], |row| row.get(0),
        ).optional()?;
        let prior: DeviceCertificate =
            serde_json::from_str(&prior.ok_or_else(invalid)?).map_err(|_| invalid())?;
        if prior.generation.checked_add(1) != Some(certificate.generation) {
            return Err(invalid());
        }
    }
    Ok(digest)
}

pub fn context(conn: &Connection, state: &AppState, pseudonym: &str) -> Result<Value, E2eError> {
    let vkey = state
        .membership_vkey_v2
        .as_ref()
        .ok_or_else(|| E2eError::Forbidden("membership_v2 verifier unavailable".into()))?;
    let registration: Option<(String, String, String)> = conn.query_row(
        "SELECT topic,commitment_hex,nullifier_hex FROM zk_nullifiers WHERE pseudonym_id=?1 AND commitment_hex IS NOT NULL",
        [pseudonym], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional()?;
    let (topic, commitment, nullifier) = registration
        .ok_or_else(|| E2eError::Forbidden("a fresh membership_v2 sign-in is required".into()))?;
    Ok(json!({"version":1,"server_identity":server_identity(state),
        "server_ed25519_pub_hex":hex::encode(state.signing_key.verifying_key().to_bytes()),
        "topic":topic,"commitment_hex":commitment,"nullifier_hex":nullifier,
        "verifier_sha256":verifier_fingerprint(vkey)}))
}

/// Publish only verified ownership. Caller holds the authenticated IMMEDIATE
/// transaction; proof admission and immutable-row checks share the same state.
/// Legacy null-certificate rows remain stored until their owner upgrades them.
pub fn publish(
    conn: &Connection,
    state: &AppState,
    pseudonym: &str,
    certificate: &DeviceCertificate,
) -> Result<bool, E2eError> {
    let digest = validate_publication(conn, state, pseudonym, certificate)?;
    let prior: Option<(Option<String>, Option<String>, String)> = conn.query_row(
        "SELECT revoked_at,certificate_sha256,x25519_pub_hex FROM member_device_keys WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3",
        params![state.server_id,pseudonym,certificate.device_id],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional()?;
    if let Some((revoked, existing_digest, public_key)) = prior {
        if revoked.is_some() {
            return Err(E2eError::Forbidden(
                "revoked device cannot be restored; register a fresh device key".into(),
            ));
        }
        if public_key != certificate.x25519_pub_hex {
            return Err(invalid());
        }
        if let Some(existing) = existing_digest {
            if existing != digest {
                return Err(E2eError::Conflict { message: "certified device is immutable; use an explicitly admitted replacement device".into(), state: None });
            }
            // Valid randomized Groth16 proofs for the identical body are
            // idempotent; preserve the already admitted proof bytes.
            return Ok(false);
        }
        conn.execute(
            "UPDATE member_device_keys SET device_certificate=?4,certificate_sha256=?5 WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3 AND device_certificate IS NULL AND revoked_at IS NULL",
            params![state.server_id,pseudonym,certificate.device_id,serde_json::to_string(certificate).map_err(|_| invalid())?,digest],
        )?;
        return Ok(false);
    }
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM member_device_keys WHERE server_id=?1 AND pseudonym_id=?2 AND revoked_at IS NULL",
        params![state.server_id,pseudonym], |row| row.get(0),
    )?;
    if count >= 64 {
        return Err(E2eError::BadRequest(
            "identity has reached the 64 active device limit".into(),
        ));
    }
    conn.execute(
        "INSERT INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex,device_certificate,certificate_sha256) VALUES(?1,?2,?3,?4,?5,?6)",
        params![state.server_id,pseudonym,certificate.device_id,certificate.x25519_pub_hex,serde_json::to_string(certificate).map_err(|_| invalid())?,digest],
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (DeviceCertificate, VerifyingKey<Bn254>, Value) {
        // Public result of a real live registration and actual membership_v2
        // proof. No identity/device private material is in this fixture.
        let value: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-certificate-public.json"
        ))
        .unwrap();
        (
            serde_json::from_value(value["certificate"].clone()).unwrap(),
            zk::parse_verification_key(&value["vkey"].to_string()).unwrap(),
            value["context"].clone(),
        )
    }

    #[test]
    fn actual_membership_proof_and_possession_verify_with_independent_key() {
        let (certificate, key, context) = fixture();
        assert_eq!(verifier_fingerprint(&key), context["verifier_sha256"]);
        assert_eq!(
            verify_certificate(
                &certificate,
                &certificate.server_identity,
                &certificate.pseudonym_id,
                &key
            )
            .unwrap(),
            certificate_digest(&certificate)
        );
        assert_eq!(
            zk::fr_to_canonical_hex(certificate_challenge(&certificate)),
            certificate.public_signals[4]
        );
    }

    #[test]
    fn each_bound_body_field_refuses_substitution_even_with_matching_expected_context() {
        let (certificate, key, _) = fixture();
        let base = serde_json::to_value(&certificate).unwrap();
        for field in [
            "server_identity",
            "topic",
            "pseudonym_id",
            "device_id",
            "x25519_pub_hex",
            "ed25519_pub_hex",
            "root_hex",
            "commitment_hex",
            "nullifier_hex",
            "generation",
            "previous_certificate_sha256",
        ] {
            let mut value = base.clone();
            value[field] = match field {
                "server_identity" => json!(format!("annex-server:{}", "b".repeat(64))),
                "topic" => json!("annex:different-context"),
                "generation" => json!(2),
                "previous_certificate_sha256" => json!("b".repeat(64)),
                _ => json!("0".repeat(64)),
            };
            let changed: DeviceCertificate = serde_json::from_value(value).unwrap();
            assert!(
                verify_certificate(
                    &changed,
                    &changed.server_identity,
                    &changed.pseudonym_id,
                    &key
                )
                .is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn invalid_proof_signature_verifier_or_login_challenge_is_rejected() {
        let (certificate, key, _) = fixture();
        let mut changed = certificate.clone();
        changed.proof["pi_a"][0] = json!("1");
        assert!(verify_certificate(
            &changed,
            &changed.server_identity,
            &changed.pseudonym_id,
            &key
        )
        .is_err());
        let mut changed = certificate.clone();
        changed.signature_hex = "0".repeat(128);
        assert!(verify_certificate(
            &changed,
            &changed.server_identity,
            &changed.pseudonym_id,
            &key
        )
        .is_err());
        let mut changed = certificate.clone();
        changed.public_signals[4] = "0".repeat(64);
        assert!(verify_certificate(
            &changed,
            &changed.server_identity,
            &changed.pseudonym_id,
            &key
        )
        .is_err());
        let mut wrong_key = key.clone();
        wrong_key.gamma_abc_g1[0] = wrong_key.alpha_g1;
        assert!(verify_certificate(
            &certificate,
            &certificate.server_identity,
            &certificate.pseudonym_id,
            &wrong_key
        )
        .is_err());
    }

    #[test]
    fn proof_wire_coordinates_and_signal_fields_are_canonical() {
        let (certificate, key, _) = fixture();
        for value in [
            json!("01"),
            json!("-1"),
            json!(1),
            json!("21888242871839275222246405745257275088696311157297823662689037894645226208583"),
        ] {
            let mut changed = certificate.clone();
            changed.proof["pi_a"][0] = value;
            assert!(verify_certificate(
                &changed,
                &changed.server_identity,
                &changed.pseudonym_id,
                &key
            )
            .is_err());
        }
        let mut changed = certificate.clone();
        changed.proof["pi_b"][2] = json!(["0", "0"]);
        assert!(verify_certificate(
            &changed,
            &changed.server_identity,
            &changed.pseudonym_id,
            &key
        )
        .is_err());
        let mut changed = certificate.clone();
        changed.public_signals.push("0".repeat(64));
        assert!(verify_certificate(
            &changed,
            &changed.server_identity,
            &changed.pseudonym_id,
            &key
        )
        .is_err());
        let mut changed = certificate.clone();
        changed.public_signals[0] = changed.public_signals[0].to_uppercase();
        assert!(verify_certificate(
            &changed,
            &changed.server_identity,
            &changed.pseudonym_id,
            &key
        )
        .is_err());
    }
}

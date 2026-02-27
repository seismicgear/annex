pub use ark_bn254::Bn254;
pub use ark_bn254::Fr;
use ark_bn254::{Fq, Fq2};
pub use ark_bn254::{G1Affine, G2Affine};
use ark_ec::AffineRepr;
use ark_ff::PrimeField;
use ark_groth16::Groth16;
pub use ark_groth16::{Proof, VerifyingKey};
use ark_snark::SNARK;
use serde::Deserialize;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZkError {
    #[error("json parse error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("field element parse error")]
    FieldElementError,
    #[error("point parse error")]
    PointError,
    #[error("verification failed")]
    VerificationFailed,
    #[error("arkworks error: {0}")]
    ArkError(#[from] ark_serialize::SerializationError),
    #[error("snark error: {0}")]
    SnarkError(String),
}

#[derive(Deserialize)]
struct SnarkJsProof {
    pi_a: Vec<String>,
    pi_b: Vec<Vec<String>>,
    pi_c: Vec<String>,
}

#[derive(Deserialize)]
struct SnarkJsVKey {
    vk_alpha_1: Vec<String>,
    vk_beta_2: Vec<Vec<String>>,
    vk_gamma_2: Vec<Vec<String>>,
    vk_delta_2: Vec<Vec<String>>,
    #[serde(rename = "IC")]
    ic: Vec<Vec<String>>,
}

pub fn parse_fr(s: &str) -> Result<Fr, ZkError> {
    Fr::from_str(s).map_err(|_| ZkError::FieldElementError)
}

pub fn parse_fr_from_hex(hex: &str) -> Result<Fr, ZkError> {
    let bytes = hex::decode(hex).map_err(|_| ZkError::FieldElementError)?;
    Ok(Fr::from_be_bytes_mod_order(&bytes))
}

pub fn parse_fq(s: &str) -> Result<Fq, ZkError> {
    Fq::from_str(s).map_err(|_| ZkError::FieldElementError)
}

/// Validates that a G1 affine point lies on the BN254 curve and belongs
/// to the correct prime-order subgroup. Rejecting off-curve or
/// wrong-subgroup points prevents invalid-curve attacks on Groth16.
fn validate_g1(point: &G1Affine) -> Result<(), ZkError> {
    if point.is_zero() {
        // The identity (point at infinity) is a valid group element.
        return Ok(());
    }
    if !point.is_on_curve() {
        return Err(ZkError::PointError);
    }
    if !point.is_in_correct_subgroup_assuming_on_curve() {
        return Err(ZkError::PointError);
    }
    Ok(())
}

/// Validates that a G2 affine point lies on the BN254 twist curve and
/// belongs to the correct prime-order subgroup.
fn validate_g2(point: &G2Affine) -> Result<(), ZkError> {
    if point.is_zero() {
        return Ok(());
    }
    if !point.is_on_curve() {
        return Err(ZkError::PointError);
    }
    if !point.is_in_correct_subgroup_assuming_on_curve() {
        return Err(ZkError::PointError);
    }
    Ok(())
}

fn parse_g1(v: &[String]) -> Result<G1Affine, ZkError> {
    if v.len() < 2 {
        return Err(ZkError::PointError);
    }
    let x = parse_fq(&v[0])?;
    let y = parse_fq(&v[1])?;
    let point = G1Affine::new_unchecked(x, y);
    validate_g1(&point)?;
    Ok(point)
}

fn parse_g2(v: &[Vec<String>]) -> Result<G2Affine, ZkError> {
    if v.len() < 2 {
        return Err(ZkError::PointError);
    }
    if v[0].len() < 2 || v[1].len() < 2 {
        return Err(ZkError::PointError);
    }
    // G2 in SnarkJS is [ [x_c0, x_c1], [y_c0, y_c1], ... ]
    // arkworks Fq2 is c0 + c1*u

    let x_c0 = parse_fq(&v[0][0])?;
    let x_c1 = parse_fq(&v[0][1])?;
    let x = Fq2::new(x_c0, x_c1);

    let y_c0 = parse_fq(&v[1][0])?;
    let y_c1 = parse_fq(&v[1][1])?;
    let y = Fq2::new(y_c0, y_c1);

    let point = G2Affine::new_unchecked(x, y);
    validate_g2(&point)?;
    Ok(point)
}

pub fn parse_proof(json: &str) -> Result<Proof<Bn254>, ZkError> {
    let raw: SnarkJsProof = serde_json::from_str(json)?;

    let a = parse_g1(&raw.pi_a)?;
    let b = parse_g2(&raw.pi_b)?;
    let c = parse_g1(&raw.pi_c)?;

    Ok(Proof { a, b, c })
}

pub fn parse_verification_key(json: &str) -> Result<VerifyingKey<Bn254>, ZkError> {
    let raw: SnarkJsVKey = serde_json::from_str(json)?;

    let alpha_g1 = parse_g1(&raw.vk_alpha_1)?;
    let beta_g2 = parse_g2(&raw.vk_beta_2)?;
    let gamma_g2 = parse_g2(&raw.vk_gamma_2)?;
    let delta_g2 = parse_g2(&raw.vk_delta_2)?;

    let mut gamma_abc_g1 = Vec::with_capacity(raw.ic.len());
    for p in raw.ic {
        gamma_abc_g1.push(parse_g1(&p)?);
    }

    Ok(VerifyingKey {
        alpha_g1,
        beta_g2,
        gamma_g2,
        delta_g2,
        gamma_abc_g1,
    })
}

pub fn parse_public_signals(json: &str) -> Result<Vec<Fr>, ZkError> {
    let raw: Vec<String> = serde_json::from_str(json)?;
    let mut out = Vec::with_capacity(raw.len());
    for s in raw {
        out.push(parse_fr(&s)?);
    }
    Ok(out)
}

pub fn verify_proof(
    vk: &VerifyingKey<Bn254>,
    proof: &Proof<Bn254>,
    public_inputs: &[Fr],
) -> Result<bool, ZkError> {
    Groth16::<Bn254>::verify(vk, public_inputs, proof)
        .map_err(|e| ZkError::SnarkError(e.to_string()))
}

/// Generates a dummy verifying key for testing purposes.
/// This key is mathematically valid (points on curve) but useless for verification.
/// It corresponds to an empty circuit.
pub fn generate_dummy_vkey() -> VerifyingKey<Bn254> {
    // Use generator points which are guaranteed to be on the curve
    let g1 = G1Affine::generator();
    let g2 = G2Affine::generator();

    VerifyingKey {
        alpha_g1: g1,
        beta_g2: g2,
        gamma_g2: g2,
        delta_g2: g2,
        gamma_abc_g1: vec![g1; 2], // 2 public inputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_g1_accepts_generator() {
        let g1 = G1Affine::generator();
        assert!(validate_g1(&g1).is_ok());
    }

    #[test]
    fn validate_g1_accepts_identity() {
        let zero = G1Affine::zero();
        assert!(validate_g1(&zero).is_ok());
    }

    #[test]
    fn validate_g1_rejects_off_curve_point() {
        // Construct a point with arbitrary coordinates not on the curve.
        let x = Fq::from(1u64);
        let y = Fq::from(1u64);
        let bad = G1Affine::new_unchecked(x, y);
        assert!(validate_g1(&bad).is_err());
    }

    #[test]
    fn validate_g2_accepts_generator() {
        let g2 = G2Affine::generator();
        assert!(validate_g2(&g2).is_ok());
    }

    #[test]
    fn validate_g2_accepts_identity() {
        let zero = G2Affine::zero();
        assert!(validate_g2(&zero).is_ok());
    }

    #[test]
    fn validate_g2_rejects_off_curve_point() {
        let x = Fq2::new(Fq::from(1u64), Fq::from(1u64));
        let y = Fq2::new(Fq::from(1u64), Fq::from(1u64));
        let bad = G2Affine::new_unchecked(x, y);
        assert!(validate_g2(&bad).is_err());
    }

    #[test]
    fn parse_proof_rejects_off_curve_pi_a() {
        // Valid JSON structure but invalid curve point
        let json =
            r#"{"pi_a":["1","1","1"],"pi_b":[["1","0"],["0","1"],["1","0"]],"pi_c":["1","1","1"]}"#;
        let result = parse_proof(json);
        assert!(result.is_err(), "off-curve pi_a should be rejected");
    }

    #[test]
    fn parse_g2_rejects_short_inner_arrays() {
        let v: Vec<Vec<String>> = vec![vec!["1".to_string()], vec!["1".to_string()]];
        assert!(parse_g2(&v).is_err());
    }

    #[test]
    fn parse_g1_rejects_too_few_elements() {
        let v: Vec<String> = vec!["1".to_string()];
        assert!(parse_g1(&v).is_err());
    }

    #[test]
    fn parse_g2_rejects_too_few_elements() {
        let v: Vec<Vec<String>> = vec![vec!["1".to_string(), "0".to_string()]];
        assert!(parse_g2(&v).is_err());
    }
}

//! Signed, monotonic RTX restrictions across scope stripping and relays.
use crate::{BundleProvenance, ReflectionSummaryBundle};
use annex_vrp::NegotiatedRtxContract;

pub fn origin_attestation_payload_v3(
    bundle: &ReflectionSummaryBundle,
    origin_server: &str,
    commitment: &str,
    max_hops: u8,
    contract: &NegotiatedRtxContract,
) -> String {
    let mut payload =
        crate::origin_attestation_payload_v2(bundle, origin_server, commitment, max_hops).replacen(
            "annex/rtx/origin-attestation/v2",
            "annex/rtx/origin-attestation/v3",
            1,
        );
    append_contract(&mut payload, contract);
    payload
}

fn append_contract(payload: &mut String, contract: &NegotiatedRtxContract) {
    use std::fmt::Write;
    // This struct contains only strings, bounded unsigned integers and options;
    // serialization is infallible. Field order is the declared protocol order.
    let serialized =
        serde_json::to_string(contract).expect("RTX contract contains no non-JSON values");
    let _ = writeln!(payload, "{}:{}", serialized.len(), serialized);
}

#[allow(clippy::too_many_arguments)]
pub fn relay_hop_payload_with_contract(
    bundle_id: &str,
    origin_server: &str,
    origin_signature: &str,
    hop_index: usize,
    hop_server: &str,
    next_peer: &str,
    content_hash: &str,
    previous_digest: &str,
    contract: Option<&NegotiatedRtxContract>,
) -> String {
    let mut payload = crate::relay_hop_payload(
        bundle_id,
        origin_server,
        origin_signature,
        hop_index,
        hop_server,
        next_peer,
        content_hash,
        previous_digest,
    );
    if let Some(contract) = contract {
        payload = payload.replacen("annex/rtx/hop/v1", "annex/rtx/hop/v2", 1);
        append_contract(&mut payload, contract);
    }
    payload
}

/// Each signed hop must preserve every upstream restriction. Missing a v3
/// restriction is a downgrade, never an invitation to use default permissions.
pub fn provenance_contract(provenance: &BundleProvenance) -> Result<NegotiatedRtxContract, String> {
    let Some(origin) = &provenance.origin else {
        if provenance.hops.iter().any(|h| h.contract.is_some()) {
            return Err("RTX contract has no signed origin".into());
        }
        return Ok(NegotiatedRtxContract::default());
    };
    let mut current = match (origin.version, &origin.contract) {
        (3, Some(contract)) => {
            contract.validate()?;
            contract.clone()
        }
        (1 | 2, None) => NegotiatedRtxContract::default(),
        _ => {
            return Err(
                "RTX origin contract is missing or incompatible with its attestation version"
                    .into(),
            )
        }
    };
    for hop in &provenance.hops {
        match &hop.contract {
            Some(contract) => {
                contract.validate()?;
                if current.intersect(contract) != *contract {
                    return Err("RTX relay widened an upstream contract".into());
                }
                current = contract.clone();
            }
            None if origin.version == 3 || !current.is_legacy_compatible() => {
                return Err("RTX relay discarded authenticated restrictions".into())
            }
            None => {}
        }
    }
    Ok(current)
}

/// Applies the size limit before any scope stripping. A large private reasoning
/// chain cannot be hidden behind a smaller delivered summary to evade admission.
pub fn check_bundle_contract(
    bundle: &ReflectionSummaryBundle,
    contract: &NegotiatedRtxContract,
    now_ms: u64,
) -> Result<Option<u64>, String> {
    contract.validate()?;
    if !contract.allows_domains(&bundle.domain_tags) {
        return Err("RTX domains exceed the negotiated allowlist".into());
    }
    let bytes = serde_json::to_vec(bundle).map_err(|e| e.to_string())?.len() as u64;
    if bytes > contract.max_exchange_size {
        return Err(format!(
            "RTX bundle is {bytes} bytes; negotiated maximum is {}",
            contract.max_exchange_size
        ));
    }
    let created = u64::try_from(bundle.created_at)
        .map_err(|_| "RTX author timestamp exceeds the supported range")?;
    if created == 0 || created > i64::MAX as u64 {
        return Err("RTX author timestamp is invalid".into());
    }
    match contract.retention_seconds {
        None => Ok(None),
        Some(seconds) => {
            if created > now_ms.saturating_add(90_000) {
                return Err("RTX retained bundle has a future author timestamp".into());
            }
            let expires = created
                .checked_add(seconds.checked_mul(1000).ok_or("RTX retention overflow")?)
                .ok_or("RTX retention overflow")?;
            if expires <= now_ms {
                return Err("RTX retention deadline has expired".into());
            }
            Ok(Some(expires))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bundle() -> ReflectionSummaryBundle {
        ReflectionSummaryBundle {
            bundle_id: "limit".into(),
            source_pseudonym: "agent".into(),
            source_server: "https://origin.example".into(),
            domain_tags: vec!["rust".into()],
            summary: "🦀 genuine utf8".into(),
            reasoning_chain: Some("private rationale".into()),
            caveats: vec![],
            created_at: 100_000,
            signature: "aa".repeat(64),
            vrp_handshake_ref: "vrp:1:1".into(),
        }
    }
    #[test]
    fn byte_limit_counts_actual_serialized_utf8_and_exact_boundary() {
        let b = bundle();
        let mut c = NegotiatedRtxContract {
            max_exchange_size: serde_json::to_vec(&b).unwrap().len() as u64,
            ..NegotiatedRtxContract::default()
        };
        assert!(check_bundle_contract(&b, &c, 100_000).is_ok());
        c.max_exchange_size -= 1;
        assert!(check_bundle_contract(&b, &c, 100_000).is_err());
    }
    #[test]
    fn expiry_uses_author_time_and_cannot_be_rebased_by_retry() {
        let b = bundle();
        let c = NegotiatedRtxContract {
            retention_seconds: Some(30),
            ..NegotiatedRtxContract::default()
        };
        assert_eq!(
            check_bundle_contract(&b, &c, 129_999).unwrap(),
            Some(130_000)
        );
        assert!(check_bundle_contract(&b, &c, 130_000).is_err());
        let mut future = b;
        future.created_at = 220_001;
        assert!(check_bundle_contract(&future, &c, 130_000).is_err());
    }
    #[test]
    fn signed_payload_binds_each_restriction() {
        let b = bundle();
        let c = NegotiatedRtxContract::default();
        let original = origin_attestation_payload_v3(&b, &b.source_server, "commitment", 3, &c);
        for modified in [
            NegotiatedRtxContract {
                retention_seconds: Some(10),
                ..c.clone()
            },
            NegotiatedRtxContract {
                knowledge_domains_allowed: Some(vec!["rust".into()]),
                ..c.clone()
            },
            NegotiatedRtxContract {
                max_exchange_size: 1000,
                ..c.clone()
            },
        ] {
            assert_ne!(
                original,
                origin_attestation_payload_v3(&b, &b.source_server, "commitment", 3, &modified)
            );
            assert_ne!(
                relay_hop_payload_with_contract("b", "a", "sig", 0, "a", "b", "hash", "", Some(&c)),
                relay_hop_payload_with_contract(
                    "b",
                    "a",
                    "sig",
                    0,
                    "a",
                    "b",
                    "hash",
                    "",
                    Some(&modified)
                )
            );
        }
    }
}

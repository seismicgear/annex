//! Versioned, deterministic RTX contract negotiation. Legacy peers are accepted
//! only when the newer participant does not demand a restriction they cannot
//! acknowledge. An old JSON field that was previously ignored is never consent.
use crate::types::{NegotiatedRtxContract, VrpCapabilitySharingContract, VrpRetentionPolicy};

pub const MAX_EXCHANGE_BYTES: u64 = 1_048_576;
pub const MAX_RETENTION_SECONDS: u64 = 3_155_760_000; // 100 years

impl Default for NegotiatedRtxContract {
    fn default() -> Self {
        Self {
            knowledge_domains_allowed: None,
            retention_seconds: None,
            max_exchange_size: MAX_EXCHANGE_BYTES,
        }
    }
}

impl VrpCapabilitySharingContract {
    pub fn from_policy(policy: &annex_types::ServerPolicy) -> Self {
        Self {
            version: 2,
            knowledge_domains_allowed: policy.knowledge_domains_allowed.clone(),
            retention_policy: Some(match policy.rtx_retention_seconds {
                Some(seconds) => VrpRetentionPolicy::MaxAge { seconds },
                None => VrpRetentionPolicy::Unlimited,
            }),
            max_exchange_size: Some(policy.max_exchange_size),
            ..Self::default()
        }
    }

    pub fn rtx_restrictions(&self) -> Result<NegotiatedRtxContract, String> {
        match self.version {
            1 => {
                if self.knowledge_domains_allowed.is_some()
                    || self.retention_policy.is_some()
                    || self.max_exchange_size.is_some()
                {
                    return Err("legacy VRP contract contains unacknowledged restrictions; re-handshake with contract version 2".into());
                }
                Ok(NegotiatedRtxContract::default())
            }
            2 => {
                let retention_seconds = match self
                    .retention_policy
                    .as_ref()
                    .ok_or("version 2 requires retention_policy")?
                {
                    VrpRetentionPolicy::Unlimited => None,
                    VrpRetentionPolicy::MaxAge { seconds } => Some(*seconds),
                };
                let value = NegotiatedRtxContract {
                    knowledge_domains_allowed: self.knowledge_domains_allowed.clone(),
                    retention_seconds,
                    max_exchange_size: self
                        .max_exchange_size
                        .ok_or("version 2 requires max_exchange_size")?,
                };
                value.validate()?;
                Ok(value.canonicalized())
            }
            _ => Err("unsupported VRP capability contract version".into()),
        }
    }
}

impl NegotiatedRtxContract {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_exchange_size == 0 || self.max_exchange_size > MAX_EXCHANGE_BYTES {
            return Err(format!(
                "max_exchange_size must be between 1 and {MAX_EXCHANGE_BYTES} bytes"
            ));
        }
        if self
            .retention_seconds
            .is_some_and(|s| s == 0 || s > MAX_RETENTION_SECONDS)
        {
            return Err(format!(
                "retention seconds must be between 1 and {MAX_RETENTION_SECONDS}"
            ));
        }
        if let Some(tags) = &self.knowledge_domains_allowed {
            if tags.len() > 256
                || tags.iter().any(|t| {
                    t.is_empty() || t.len() > 64 || t.trim() != t || t.chars().any(char::is_control)
                })
            {
                return Err("knowledge_domains_allowed requires at most 256 nonempty trimmed domains of at most 64 bytes".into());
            }
        }
        Ok(())
    }

    fn canonicalized(mut self) -> Self {
        if let Some(tags) = &mut self.knowledge_domains_allowed {
            for tag in tags.iter_mut() {
                *tag = tag.to_lowercase();
            }
            tags.sort();
            tags.dedup();
        }
        self
    }

    /// Combining policies is monotonic: no operation can widen a permission.
    pub fn intersect(&self, other: &Self) -> Self {
        let left = self.clone().canonicalized();
        let right = other.clone().canonicalized();
        let knowledge_domains_allowed = match (
            left.knowledge_domains_allowed,
            right.knowledge_domains_allowed,
        ) {
            (None, b) => b,
            (a, None) => a,
            (Some(a), Some(b)) => Some(a.into_iter().filter(|x| b.contains(x)).collect()),
        };
        Self {
            knowledge_domains_allowed,
            retention_seconds: match (self.retention_seconds, other.retention_seconds) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, None) => a,
                (None, b) => b,
            },
            max_exchange_size: self.max_exchange_size.min(other.max_exchange_size),
        }
    }

    pub fn is_legacy_compatible(&self) -> bool {
        *self == Self::default()
    }

    pub fn allows_domains(&self, domains: &[String]) -> bool {
        match &self.knowledge_domains_allowed {
            None => true,
            Some(allowed) => {
                !domains.is_empty()
                    && domains
                        .iter()
                        .all(|d| allowed.iter().any(|a| a.to_lowercase() == d.to_lowercase()))
            }
        }
    }
}

pub fn negotiate_rtx_contract(
    local: &VrpCapabilitySharingContract,
    remote: &VrpCapabilitySharingContract,
) -> Result<NegotiatedRtxContract, String> {
    let ours = local.rtx_restrictions()?;
    let theirs = remote.rtx_restrictions()?;
    if (local.version == 1 && !theirs.is_legacy_compatible())
        || (remote.version == 1 && !ours.is_legacy_compatible())
    {
        return Err("restricted RTX requires both participants to acknowledge capability contract version 2; re-handshake required".into());
    }
    Ok(ours.intersect(&theirs))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contract() -> VrpCapabilitySharingContract {
        VrpCapabilitySharingContract::from_policy(&annex_types::ServerPolicy::default())
    }
    #[test]
    fn legacy_only_when_equivalent_and_unknown_fields_are_rejected() {
        let legacy = VrpCapabilitySharingContract::default();
        assert!(negotiate_rtx_contract(&contract(), &legacy).is_ok());
        let mut restricted = contract();
        restricted.max_exchange_size = Some(999);
        assert!(negotiate_rtx_contract(&restricted, &legacy).is_err());
        assert!(negotiate_rtx_contract(&legacy, &restricted).is_err());
        let mut mislabeled = legacy;
        mislabeled.retention_policy = Some(VrpRetentionPolicy::MaxAge { seconds: 30 });
        assert!(mislabeled.rtx_restrictions().is_err());
        assert!(serde_json::from_str::<VrpCapabilitySharingContract>(
            r#"{"required_capabilities":[],"offered_capabilities":[],"max_exchange_siz":50}"#
        )
        .is_err());
    }
    #[test]
    fn restrictions_intersect_and_never_widen() {
        let mut a = contract();
        let mut b = contract();
        a.knowledge_domains_allowed = Some(vec!["RUST".into(), "security".into()]);
        b.knowledge_domains_allowed = Some(vec!["rust".into(), "biology".into()]);
        a.retention_policy = Some(VrpRetentionPolicy::MaxAge { seconds: 60 });
        b.retention_policy = Some(VrpRetentionPolicy::MaxAge { seconds: 120 });
        a.max_exchange_size = Some(1000);
        b.max_exchange_size = Some(2000);
        let result = negotiate_rtx_contract(&a, &b).unwrap();
        assert_eq!(result.knowledge_domains_allowed, Some(vec!["rust".into()]));
        assert_eq!(result.retention_seconds, Some(60));
        assert_eq!(result.max_exchange_size, 1000);
        assert!(result.allows_domains(&["RuSt".into()]));
        assert!(!result.allows_domains(&[]));
        assert!(!result.allows_domains(&["rust".into(), "secret".into()]));
        assert_eq!(result.intersect(&NegotiatedRtxContract::default()), result);
    }
    #[test]
    fn strict_version_two_declarations_and_boundary_limits() {
        let mut value = contract();
        value.retention_policy = None;
        assert!(value.rtx_restrictions().is_err());
        value = contract();
        value.max_exchange_size = Some(0);
        assert!(value.rtx_restrictions().is_err());
        value.max_exchange_size = Some(MAX_EXCHANGE_BYTES + 1);
        assert!(value.rtx_restrictions().is_err());
        value = contract();
        value.retention_policy = Some(VrpRetentionPolicy::MaxAge { seconds: 0 });
        assert!(value.rtx_restrictions().is_err());
        value = contract();
        value.knowledge_domains_allowed = Some(vec![]);
        assert!(!value
            .rtx_restrictions()
            .unwrap()
            .allows_domains(&["rust".into()]));
    }
}

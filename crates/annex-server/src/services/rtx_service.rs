//! RTX (Reflection Transfer Exchange) orchestration.
//!
//! Each public method is the orchestration the matching `api_rtx`
//! handler used to do inline:
//!
//!   * `publish_bundle` — validate the bundle structure, enforce
//!     identity binding (sender == bundle.source_pseudonym, server URL
//!     matches), gate by the sender's active agent registration's
//!     transfer scope, enforce redacted topics from the sender's
//!     capability contract, scope-strip the bundle, persist the row +
//!     transfer log atomically, fan out to matching local subscribers
//!     (per-subscriber scope re-application + delivery audit log),
//!     spawn the federation relay.
//!   * `subscribe` / `unsubscribe` / `get_subscription` — gate by
//!     active agent registration with `>= ReflectionSummariesOnly`
//!     scope; UPSERT / DELETE / SELECT on `rtx_subscriptions`.
//!   * `governance_transfers` / `governance_summary` — moderator-only
//!     reads against `rtx_transfer_log`.
//!
//! [`relay_rtx_bundles`] is the background fan-out spawned from
//! `publish_bundle` after the local commit; it is also exposed as a
//! free function so it can be `tokio::spawn`-ed without holding the
//! service borrow.
//!
//! Behaviour preservation:
//!   * No SQL change. Each statement reads the same columns / params as
//!     the previous inline code.
//!   * No redaction weakening. Capability-contract `redacted_topics` is
//!     enforced before publish; corrupt JSON fails closed.
//!     Scope-stripping (`reasoning_chain` for ReflectionSummariesOnly)
//!     is preserved. Subscriber `domain_filters_json` parse errors skip
//!     the subscriber entirely (refuse to default to accept-all).
//!   * No federation behaviour change. Cycle detection in
//!     `relay_rtx_bundles` still uses the local public URL + the
//!     bundle's source server; the signed payload format
//!     (`rtx_relay_signing_payload`) is identical.
//!   * No JSON shape change. `PublishResponse`, `SubscribeResponse`,
//!     `SubscriptionInfo`, `TransferLogResponse`, `TransferLogEntry`,
//!     `GovernanceSummaryResponse`, `ScopeBreakdown` all kept here and
//!     re-exported by `api_rtx.rs` so external imports
//!     (`annex_server::api_rtx::Foo`) keep resolving.

use std::sync::Arc;

use annex_federation::FederatedRtxEnvelope;
use annex_rtx::{
    check_redacted_topics, enforce_transfer_scope, validate_bundle_structure, BundleProvenance,
    OriginAttestation, ReflectionSummaryBundle, RelayHop,
};
use annex_vrp::VrpTransferScope;
use ed25519_dalek::Signer;
use serde::{Deserialize, Serialize};

use crate::api::ApiError;
use crate::middleware::IdentityContext;
use crate::parse_transfer_scope;
use crate::services::rtx_repository::{self as repo, TransferLogFilter};
use crate::AppState;

// ── Wire types ─────────────────────────────────────────────────────────

/// Response returned after a successful bundle publish.
#[derive(Debug, Serialize)]
pub struct PublishResponse {
    pub ok: bool,
    #[serde(rename = "bundleId")]
    pub bundle_id: String,
    pub delivered_to: usize,
}

/// Request body for `POST /api/rtx/subscribe`.
#[derive(Debug, Deserialize)]
pub struct SubscribeRequest {
    /// Domain tags to filter incoming bundles (empty = accept all).
    #[serde(default)]
    pub domain_filters: Vec<String>,
    /// Whether to accept bundles relayed from federated servers.
    #[serde(default)]
    pub accept_federated: bool,
}

/// Response from subscribe / unsubscribe / get-subscription operations.
#[derive(Debug, Serialize)]
pub struct SubscribeResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription: Option<SubscriptionInfo>,
}

/// Serialised representation of an RTX subscription.
#[derive(Debug, Serialize)]
pub struct SubscriptionInfo {
    pub subscriber_pseudonym: String,
    pub domain_filters: Vec<String>,
    pub accept_federated: bool,
    pub created_at: String,
}

/// Query parameters for `GET /api/rtx/governance/transfers`.
#[derive(Debug, Deserialize)]
pub struct TransferLogQuery {
    /// Filter by bundle_id.
    pub bundle_id: Option<String>,
    /// Filter by source pseudonym.
    pub source: Option<String>,
    /// Filter by destination pseudonym.
    pub destination: Option<String>,
    /// Filter transfers at or after this ISO 8601 timestamp.
    pub since: Option<String>,
    /// Filter transfers at or before this ISO 8601 timestamp.
    pub until: Option<String>,
    /// Maximum number of results to return (default 50, max 500).
    pub limit: Option<u32>,
    /// Number of results to skip (for pagination).
    pub offset: Option<u32>,
}

/// A single entry from the RTX transfer log.
#[derive(Debug, Serialize)]
pub struct TransferLogEntry {
    pub id: i64,
    pub bundle_id: String,
    pub source_pseudonym: String,
    pub destination_pseudonym: Option<String>,
    pub transfer_scope_applied: String,
    pub redactions_applied: Option<String>,
    pub transferred_at: String,
}

/// Response for `GET /api/rtx/governance/transfers`.
#[derive(Debug, Serialize)]
pub struct TransferLogResponse {
    pub transfers: Vec<TransferLogEntry>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// Breakdown of transfers by scope.
#[derive(Debug, Serialize)]
pub struct ScopeBreakdown {
    pub scope: String,
    pub count: i64,
}

/// Response for `GET /api/rtx/governance/summary`.
#[derive(Debug, Serialize)]
pub struct GovernanceSummaryResponse {
    /// Total number of transfer log entries on this server.
    pub total_transfers: i64,
    /// Count of distinct bundle IDs.
    pub unique_bundles: i64,
    /// Count of distinct source pseudonyms.
    pub unique_sources: i64,
    /// Count of distinct destination pseudonyms (excluding NULL for publishes).
    pub unique_destinations: i64,
    /// Count of transfers where redactions were applied.
    pub redacted_transfers: i64,
    /// Breakdown by transfer scope.
    pub by_scope: Vec<ScopeBreakdown>,
}

// ── Constants / helpers shared with the federation receive path ─────────

/// Computes a stable content hash over a bundle's semantically-meaningful
/// fields.
///
/// Binding this hash into the relay signing payload
/// ([`rtx_relay_signing_payload`]) means a relaying or man-in-the-middle peer
/// cannot alter a bundle's content, tags, author, timestamp, author signature,
/// or VRP/provenance handshake reference without invalidating the origin
/// server's relay signature — the receiver recomputes the hash from the bundle
/// it actually received and verification fails on any mismatch. Fields are
/// length-prefixed (u64 LE) so the encoding is unambiguous across field
/// boundaries.
pub fn rtx_bundle_content_hash(bundle: &ReflectionSummaryBundle) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let absorb = |bytes: &[u8], h: &mut Sha256| {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    absorb(bundle.bundle_id.as_bytes(), &mut h);
    absorb(bundle.source_pseudonym.as_bytes(), &mut h);
    absorb(bundle.source_server.as_bytes(), &mut h);
    absorb(bundle.summary.as_bytes(), &mut h);
    absorb(
        bundle.reasoning_chain.as_deref().unwrap_or("").as_bytes(),
        &mut h,
    );
    h.update((bundle.domain_tags.len() as u64).to_le_bytes());
    for t in &bundle.domain_tags {
        absorb(t.as_bytes(), &mut h);
    }
    h.update((bundle.caveats.len() as u64).to_le_bytes());
    for c in &bundle.caveats {
        absorb(c.as_bytes(), &mut h);
    }
    absorb(bundle.created_at.to_string().as_bytes(), &mut h);
    absorb(bundle.signature.as_bytes(), &mut h);
    // Bind the VRP/provenance handshake reference too: the receive path stores
    // this value, so a relaying peer must not be able to rewrite it without
    // invalidating the relay signature.
    absorb(bundle.vrp_handshake_ref.as_bytes(), &mut h);
    hex::encode(h.finalize())
}

/// Verifies a bundle's per-agent **author** signature against the producing
/// agent's Ed25519 public key (the `signing_pubkey` captured at VRP handshake).
///
/// The agent signs `SHA-256(author_signing_payload(bundle))` — a payload that
/// binds every content field (see [`annex_rtx::author_signing_payload`]) — so a
/// valid signature proves the bundle was authored by the holder of that key and
/// that no content field was altered. This closes the per-agent
/// author-authenticity half of AUDIT P4-FED-1.
///
/// `pubkey_hex` is 64-char hex (32-byte Ed25519 public key); `bundle.signature`
/// is 128-char hex (64-byte Ed25519 signature). Any decode/length/verify
/// failure returns `Err`.
pub fn verify_bundle_author_signature(
    bundle: &ReflectionSummaryBundle,
    pubkey_hex: &str,
) -> Result<(), ApiError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use sha2::{Digest, Sha256};

    let pk_bytes = hex::decode(pubkey_hex.trim())
        .map_err(|_| ApiError::Unauthorized("agent signing_pubkey is not valid hex".to_string()))?;
    let pk_arr: [u8; 32] = pk_bytes
        .as_slice()
        .try_into()
        .map_err(|_| ApiError::Unauthorized("agent signing_pubkey must be 32 bytes".to_string()))?;
    let verifying_key = VerifyingKey::from_bytes(&pk_arr).map_err(|_| {
        ApiError::Unauthorized("agent signing_pubkey is not a valid Ed25519 key".to_string())
    })?;

    let sig_bytes = hex::decode(bundle.signature.trim())
        .map_err(|_| ApiError::Unauthorized("bundle signature is not valid hex".to_string()))?;
    let sig_arr: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| ApiError::Unauthorized("bundle signature must be 64 bytes".to_string()))?;
    let signature = Signature::from_bytes(&sig_arr);

    let digest = Sha256::digest(annex_rtx::author_signing_payload(bundle).as_bytes());
    verifying_key.verify(&digest, &signature).map_err(|_| {
        ApiError::Unauthorized("bundle author signature verification failed".to_string())
    })
}

/// Produces the hex Ed25519 author signature for a bundle given the agent's
/// 32-byte signing key. The counterpart to [`verify_bundle_author_signature`] —
/// exposed so an agent client (and the tests) can sign bundles correctly.
pub fn sign_bundle_author(
    bundle: &ReflectionSummaryBundle,
    signing_key_bytes: &[u8; 32],
) -> String {
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    let sk = SigningKey::from_bytes(signing_key_bytes);
    let digest = Sha256::digest(annex_rtx::author_signing_payload(bundle).as_bytes());
    hex::encode(sk.sign(&digest).to_bytes())
}

/// Constructs the deterministic signing payload for an RTX relay envelope.
///
/// The signed payload uses newline delimiters between fields to prevent
/// ambiguity where field boundaries overlap (e.g., `"ab" + "c"` vs
/// `"a" + "bc"`). Relay path entries are joined with `|` separators
/// within their field. The trailing `content_hash` (see
/// [`rtx_bundle_content_hash`]) binds the bundle's content to the signature so
/// it cannot be tampered with in transit.
pub fn rtx_relay_signing_payload(
    bundle_id: &str,
    relaying_server: &str,
    origin_server: &str,
    relay_path: &[String],
    content_hash: &str,
) -> String {
    let relay_path_joined = relay_path.join("|");
    format!("{bundle_id}\n{relaying_server}\n{origin_server}\n{relay_path_joined}\n{content_hash}")
}

/// Sign the origin attestation for a bundle this server is publishing.
///
/// Must be called on the UNSCOPED bundle, before `enforce_transfer_scope` runs
/// for any peer: `reasoning_commitment` has to be over the chain as published,
/// and scope enforcement strips it. Minting it once per publish rather than once
/// per peer is not an optimisation — a per-peer attestation over a
/// scope-stripped bundle would commit to the absence of the chain, and the next
/// hop could then never tell a stripped bundle from one that never had a chain.
pub fn sign_origin_attestation(
    bundle: &ReflectionSummaryBundle,
    origin_server: &str,
    signing_key: &ed25519_dalek::SigningKey,
    max_hops: u8,
) -> OriginAttestation {
    use sha2::{Digest, Sha256};
    let commitment = annex_rtx::reasoning_commitment(bundle.reasoning_chain.as_deref());
    let payload = annex_rtx::origin_attestation_payload(
        &bundle.bundle_id,
        origin_server,
        &bundle.source_pseudonym,
        bundle.created_at,
        &bundle.vrp_handshake_ref,
        &annex_rtx::scope_invariant_content_digest(bundle),
        &commitment,
        max_hops,
    );
    let digest = Sha256::digest(payload.as_bytes());
    OriginAttestation {
        signature: hex::encode(signing_key.sign(&digest).to_bytes()),
        reasoning_commitment: commitment,
        max_hops,
    }
}

/// Extracts redacted topics from a capability contract JSON string.
///
/// The `redacted_topics` field may or may not be present in the stored
/// JSON (backward compatibility with contracts created before this field
/// existed). If the JSON is entirely unparseable (data corruption),
/// returns an error to fail closed — preventing unauthorised knowledge
/// transfer through topics that should be redacted.
fn extract_redacted_topics(contract_json: &str) -> Result<Vec<String>, String> {
    serde_json::from_str::<annex_vrp::VrpCapabilitySharingContract>(contract_json)
        .map(|c| c.redacted_topics)
        .map_err(|e| {
            tracing::warn!(
                "corrupted capability contract JSON, rejecting publish: {}",
                e
            );
            format!("corrupted capability contract: {e}")
        })
}

/// RTX orchestration. Holds an `Arc<AppState>` so it can be constructed
/// cheaply per-request from a handler's `Extension<Arc<AppState>>`.
pub struct RtxService {
    state: Arc<AppState>,
}

impl RtxService {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// `POST /api/rtx/publish` orchestration. Returns the populated
    /// `PublishResponse` and (out of band) spawns the federation relay
    /// after the local writes commit.
    pub async fn publish_bundle(
        &self,
        identity: &IdentityContext,
        bundle: ReflectionSummaryBundle,
    ) -> Result<PublishResponse, ApiError> {
        // 1. Validate bundle structure
        validate_bundle_structure(&bundle).map_err(|e| ApiError::BadRequest(e.to_string()))?;

        // 2. Verify sender matches bundle source_pseudonym
        let IdentityContext(ref auth) = *identity;
        if auth.pseudonym_id != bundle.source_pseudonym {
            return Err(ApiError::Forbidden(
                "bundle source_pseudonym does not match authenticated identity".to_string(),
            ));
        }

        // 3. Verify source_server matches this server
        let pub_url = self.state.get_public_url();
        if bundle.source_server != pub_url {
            return Err(ApiError::BadRequest(format!(
                "source_server '{}' does not match this server '{}'",
                bundle.source_server, pub_url,
            )));
        }

        let bundle_id = bundle.bundle_id.clone();
        let state = self.state.clone();

        let (delivered_count, deliveries) = tokio::task::spawn_blocking({
            let state = state.clone();
            let bundle = bundle.clone();
            move || -> Result<(usize, Vec<(String, String)>), ApiError> {
                let mut conn = state.pool.get().map_err(|e| {
                    ApiError::InternalServerError(format!("db connection failed: {e}"))
                })?;

                // 4. Check sender has an active agent registration with sufficient transfer scope
                let agent = repo::agent_publish_context(
                    &conn,
                    state.server_id,
                    &bundle.source_pseudonym,
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("db query failed: {e}"))
                })?
                .ok_or_else(|| {
                    ApiError::Forbidden(format!(
                        "sender '{}' does not have an active agent registration",
                        bundle.source_pseudonym
                    ))
                })?;

                // 4b. Per-agent author signature (AUDIT P4-FED-1). When the
                //     agent advertised an Ed25519 signing key at VRP handshake,
                //     the bundle's `signature` MUST be a valid author signature
                //     over every content field — proving authorship and that no
                //     field was altered. Legacy agents with no key on file fall
                //     back to the structural-only check (the signature is still
                //     length-validated by `validate_bundle_structure`), so this
                //     does not break agents that pre-date the handshake field.
                if let Some(pubkey) = agent.signing_pubkey.as_deref() {
                    verify_bundle_author_signature(&bundle, pubkey)?;
                } else {
                    tracing::warn!(
                        pseudonym = %bundle.source_pseudonym,
                        "RTX publish from an agent with no signing_pubkey on file — \
                         author signature not cryptographically verified (legacy agent)"
                    );
                }

                // 5. Parse and validate transfer scope
                let sender_scope =
                    parse_transfer_scope(&agent.transfer_scope_str).ok_or_else(|| {
                        ApiError::Forbidden(
                            "sender's transfer scope does not permit RTX publishing".to_string(),
                        )
                    })?;

                if sender_scope < VrpTransferScope::ReflectionSummariesOnly {
                    return Err(ApiError::Forbidden(
                        "sender's transfer scope does not permit RTX publishing".to_string(),
                    ));
                }

                // 6. Extract redacted topics from capability contract and enforce
                let redacted_topics = extract_redacted_topics(&agent.capability_contract_json)
                    .map_err(ApiError::InternalServerError)?;
                check_redacted_topics(&bundle, &redacted_topics)
                    .map_err(|e| ApiError::Forbidden(e.to_string()))?;

                // 7. Apply sender's transfer scope (strips reasoning_chain if scope is ReflectionSummariesOnly)
                let stored_bundle = enforce_transfer_scope(&bundle, sender_scope)
                    .map_err(|e| ApiError::Forbidden(e.to_string()))?;

                // 8-9. Store bundle + log initial transfer atomically.
                let domain_tags_json = serde_json::to_string(&stored_bundle.domain_tags)
                    .map_err(|e| {
                        ApiError::InternalServerError(format!("json serialization failed: {e}"))
                    })?;
                let caveats_json = serde_json::to_string(&stored_bundle.caveats).map_err(|e| {
                    ApiError::InternalServerError(format!("json serialization failed: {e}"))
                })?;

                let redactions = if stored_bundle.reasoning_chain.is_none()
                    && bundle.reasoning_chain.is_some()
                {
                    Some("reasoning_chain_stripped")
                } else {
                    None
                };

                {
                    // IMMEDIATE — read-then-write, as in the send path.
                    let tx = conn
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .map_err(|e| {
                        ApiError::InternalServerError(format!("failed to begin transaction: {e}"))
                    })?;

                    repo::insert_local_rtx_bundle(
                        &tx,
                        state.server_id,
                        &stored_bundle.bundle_id,
                        &stored_bundle.source_pseudonym,
                        &stored_bundle.source_server,
                        &domain_tags_json,
                        &stored_bundle.summary,
                        stored_bundle.reasoning_chain.as_deref(),
                        &caveats_json,
                        stored_bundle.created_at as i64,
                        &stored_bundle.signature,
                        &stored_bundle.vrp_handshake_ref,
                    )
                    .map_err(|e| {
                        if let rusqlite::Error::SqliteFailure(ref err, _) = e {
                            if err.code == rusqlite::ErrorCode::ConstraintViolation {
                                return ApiError::Conflict(format!(
                                    "bundle '{}' already published",
                                    stored_bundle.bundle_id
                                ));
                            }
                        }
                        ApiError::InternalServerError(format!("failed to store bundle: {e}"))
                    })?;

                    repo::log_rtx_transfer(
                        &tx,
                        state.server_id,
                        &stored_bundle.bundle_id,
                        &stored_bundle.source_pseudonym,
                        None,
                        &sender_scope.to_string(),
                        redactions,
                    )
                    .map_err(|e| {
                        ApiError::InternalServerError(format!("failed to log transfer: {e}"))
                    })?;

                    tx.commit().map_err(|e| {
                        ApiError::InternalServerError(format!(
                            "failed to commit transaction: {e}"
                        ))
                    })?;
                }

                // 10. Find matching subscribers and prepare deliveries
                let subscribers = repo::list_local_rtx_subscribers(
                    &conn,
                    state.server_id,
                    &bundle.source_pseudonym,
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("db query failed: {e}"))
                })?;

                let mut deliveries: Vec<(String, String)> = Vec::new();

                for sub in subscribers {
                    // Parse domain filters. Corrupted JSON → skip this subscriber
                    // entirely (reject) rather than defaulting to accept-all,
                    // which could cause unauthorised knowledge transfer.
                    let domain_filters: Vec<String> =
                        match serde_json::from_str(&sub.domain_filters_json) {
                            Ok(f) => f,
                            Err(e) => {
                                tracing::error!(
                                    subscriber = %sub.pseudonym,
                                    bundle_id = %stored_bundle.bundle_id,
                                    raw_json = %sub.domain_filters_json,
                                    "corrupted domain_filters_json in rtx subscription; skipping delivery to protect against unauthorized transfer: {}",
                                    e
                                );
                                continue;
                            }
                        };

                    // Check domain tag match (empty filters = accept all)
                    let matches = domain_filters.is_empty()
                        || stored_bundle
                            .domain_tags
                            .iter()
                            .any(|tag| domain_filters.contains(tag));

                    if !matches {
                        continue;
                    }

                    // Parse receiver's transfer scope
                    let receiver_scope = match parse_transfer_scope(&sub.transfer_scope_str) {
                        Some(s) if s >= VrpTransferScope::ReflectionSummariesOnly => s,
                        _ => {
                            tracing::warn!(
                                subscriber = %sub.pseudonym,
                                bundle_id = %stored_bundle.bundle_id,
                                scope = %sub.transfer_scope_str,
                                "skipping RTX delivery: transfer scope is NoTransfer or unparseable"
                            );
                            continue;
                        }
                    };

                    // Apply receiver's transfer scope enforcement
                    let scoped = match enforce_transfer_scope(&stored_bundle, receiver_scope) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!(
                                subscriber = %sub.pseudonym,
                                bundle_id = %stored_bundle.bundle_id,
                                scope = %receiver_scope.to_string(),
                                "transfer scope enforcement failed; skipping delivery: {}",
                                e
                            );
                            continue;
                        }
                    };

                    let payload = serde_json::json!({
                        "type": "rtx_bundle",
                        "bundle": scoped,
                    });

                    match serde_json::to_string(&payload) {
                        Ok(json) => {
                            // Log delivery (best-effort; failure to write the
                            // audit row is logged but does not stop delivery)
                            let delivery_redactions = if receiver_scope
                                == VrpTransferScope::ReflectionSummariesOnly
                                && stored_bundle.reasoning_chain.is_some()
                            {
                                Some("reasoning_chain_stripped")
                            } else {
                                None
                            };

                            if let Err(e) = conn.execute(
                                "INSERT INTO rtx_transfer_log (
                                    server_id, bundle_id, source_pseudonym, destination_pseudonym,
                                    transfer_scope_applied, redactions_applied
                                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                                rusqlite::params![
                                    state.server_id,
                                    stored_bundle.bundle_id,
                                    stored_bundle.source_pseudonym,
                                    sub.pseudonym,
                                    receiver_scope.to_string(),
                                    delivery_redactions,
                                ],
                            ) {
                                tracing::warn!(
                                    bundle_id = %stored_bundle.bundle_id,
                                    destination = %sub.pseudonym,
                                    "failed to write rtx transfer log: {}",
                                    e
                                );
                            }

                            deliveries.push((sub.pseudonym, json));
                        }
                        Err(e) => {
                            tracing::error!(
                                bundle_id = %stored_bundle.bundle_id,
                                destination = %sub.pseudonym,
                                "failed to serialize rtx bundle for delivery: {}", e
                            );
                        }
                    }
                }

                let count = deliveries.len();
                Ok((count, deliveries))
            }
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

        // 11. Deliver via WebSocket (async, outside spawn_blocking)
        for (pseudonym, json) in &deliveries {
            self.state
                .connection_manager
                .send(pseudonym, json.clone())
                .await;
        }

        // 12. Relay to federated peers (background task — does not block response)
        tokio::spawn(relay_rtx_bundles(state.clone(), bundle));

        Ok(PublishResponse {
            ok: true,
            bundle_id,
            delivered_to: delivered_count,
        })
    }

    /// `POST /api/rtx/subscribe` orchestration. Returns the persisted
    /// `SubscriptionInfo` for the response body.
    pub async fn subscribe(
        &self,
        identity: &IdentityContext,
        req: SubscribeRequest,
    ) -> Result<SubscriptionInfo, ApiError> {
        let IdentityContext(ref auth) = *identity;
        let pseudonym = auth.pseudonym_id.clone();
        let state = self.state.clone();

        tokio::task::spawn_blocking({
            let state = state.clone();
            let pseudonym = pseudonym.clone();
            let domain_filters = req.domain_filters.clone();
            let accept_federated = req.accept_federated;
            move || -> Result<SubscriptionInfo, ApiError> {
                let conn = state.pool.get().map_err(|e| {
                    ApiError::InternalServerError(format!("db connection failed: {e}"))
                })?;

                // 1. Verify agent has active registration with sufficient scope
                let scope_str =
                    repo::agent_active_transfer_scope(&conn, state.server_id, &pseudonym)
                        .map_err(|e| {
                            ApiError::InternalServerError(format!("db query failed: {e}"))
                        })?
                        .ok_or_else(|| {
                            ApiError::Forbidden(format!(
                                "agent '{pseudonym}' does not have an active registration"
                            ))
                        })?;

                let scope = parse_transfer_scope(&scope_str).ok_or_else(|| {
                    ApiError::Forbidden(
                        "agent's transfer scope does not permit RTX subscriptions".into(),
                    )
                })?;

                if scope < VrpTransferScope::ReflectionSummariesOnly {
                    return Err(ApiError::Forbidden(
                        "agent's transfer scope does not permit RTX subscriptions".to_string(),
                    ));
                }

                // 2. UPSERT subscription
                let filters_json = serde_json::to_string(&domain_filters).map_err(|e| {
                    ApiError::InternalServerError(format!("json serialization failed: {e}"))
                })?;

                repo::upsert_subscription(
                    &conn,
                    state.server_id,
                    &pseudonym,
                    &filters_json,
                    accept_federated,
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("failed to create subscription: {e}"))
                })?;

                // 3. Read back for response
                let row = repo::read_subscription(&conn, state.server_id, &pseudonym)
                    .map_err(|e| {
                        ApiError::InternalServerError(format!("failed to read subscription: {e}"))
                    })?
                    .ok_or_else(|| {
                        ApiError::InternalServerError(
                            "subscription disappeared between upsert and read".to_string(),
                        )
                    })?;

                let parsed_filters: Vec<String> = serde_json::from_str(&row.domain_filters_json)
                    .map_err(|e| {
                        tracing::error!(
                            subscriber = %pseudonym,
                            raw_json = %row.domain_filters_json,
                            "corrupted domain_filters_json in subscription read-back: {}",
                            e
                        );
                        ApiError::InternalServerError(
                            "corrupted domain filter data in subscription".to_string(),
                        )
                    })?;

                Ok(SubscriptionInfo {
                    subscriber_pseudonym: pseudonym,
                    domain_filters: parsed_filters,
                    accept_federated: row.accept_federated,
                    created_at: row.created_at,
                })
            }
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))?
    }

    /// `DELETE /api/rtx/subscribe` orchestration. Returns `Err(NotFound)`
    /// if the agent had no active subscription to remove.
    pub async fn unsubscribe(&self, identity: &IdentityContext) -> Result<(), ApiError> {
        let IdentityContext(ref auth) = *identity;
        let pseudonym = auth.pseudonym_id.clone();
        let state = self.state.clone();

        tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
            let conn = state
                .pool
                .get()
                .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

            let deleted =
                repo::delete_subscription(&conn, state.server_id, &pseudonym).map_err(|e| {
                    ApiError::InternalServerError(format!("failed to delete subscription: {e}"))
                })?;

            if deleted == 0 {
                return Err(ApiError::NotFound("no active RTX subscription".to_string()));
            }
            Ok(())
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))?
    }

    /// `GET /api/rtx/subscriptions` orchestration. `Ok(None)` when the
    /// agent has no subscription row.
    pub async fn get_subscription(
        &self,
        identity: &IdentityContext,
    ) -> Result<Option<SubscriptionInfo>, ApiError> {
        let IdentityContext(ref auth) = *identity;
        let pseudonym = auth.pseudonym_id.clone();
        let state = self.state.clone();

        tokio::task::spawn_blocking(move || -> Result<Option<SubscriptionInfo>, ApiError> {
            let conn = state
                .pool
                .get()
                .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

            let row = repo::read_subscription(&conn, state.server_id, &pseudonym)
                .map_err(|e| ApiError::InternalServerError(format!("db query failed: {e}")))?;

            match row {
                Some(row) => {
                    let domain_filters: Vec<String> =
                        serde_json::from_str(&row.domain_filters_json).map_err(|e| {
                            tracing::error!(
                                subscriber = %pseudonym,
                                raw_json = %row.domain_filters_json,
                                "corrupted domain_filters_json in subscription query: {}",
                                e
                            );
                            ApiError::InternalServerError(
                                "corrupted domain filter data in subscription".to_string(),
                            )
                        })?;
                    Ok(Some(SubscriptionInfo {
                        subscriber_pseudonym: pseudonym,
                        domain_filters,
                        accept_federated: row.accept_federated,
                        created_at: row.created_at,
                    }))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))?
    }

    /// `GET /api/rtx/governance/transfers` orchestration. Moderator-only
    /// (gated at the handler boundary; this method does not re-check).
    pub async fn governance_transfers(
        &self,
        query: TransferLogQuery,
    ) -> Result<TransferLogResponse, ApiError> {
        let limit = query.limit.unwrap_or(50).min(500);
        let offset = query.offset.unwrap_or(0);
        let state = self.state.clone();

        let filter = TransferLogFilter {
            bundle_id: query.bundle_id,
            source: query.source,
            destination: query.destination,
            since: query.since,
            until: query.until,
            limit,
            offset,
        };

        tokio::task::spawn_blocking(move || -> Result<TransferLogResponse, ApiError> {
            let conn = state
                .pool
                .get()
                .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

            let total = repo::count_filtered_transfers(&conn, state.server_id, &filter)
                .map_err(|e| ApiError::InternalServerError(format!("count query failed: {e}")))?;

            let rows =
                repo::list_filtered_transfers(&conn, state.server_id, &filter).map_err(|e| {
                    ApiError::InternalServerError(format!("transfer log query failed: {e}"))
                })?;

            let transfers: Vec<TransferLogEntry> = rows
                .into_iter()
                .map(|r| TransferLogEntry {
                    id: r.id,
                    bundle_id: r.bundle_id,
                    source_pseudonym: r.source_pseudonym,
                    destination_pseudonym: r.destination_pseudonym,
                    transfer_scope_applied: r.transfer_scope_applied,
                    redactions_applied: r.redactions_applied,
                    transferred_at: r.transferred_at,
                })
                .collect();

            Ok(TransferLogResponse {
                transfers,
                total,
                limit: filter.limit,
                offset: filter.offset,
            })
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))?
    }

    /// `GET /api/rtx/governance/summary` orchestration. Moderator-only
    /// (gated at the handler boundary).
    pub async fn governance_summary(&self) -> Result<GovernanceSummaryResponse, ApiError> {
        let state = self.state.clone();

        tokio::task::spawn_blocking(move || -> Result<GovernanceSummaryResponse, ApiError> {
            let conn = state
                .pool
                .get()
                .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

            let counts = repo::governance_counts(&conn, state.server_id).map_err(|e| {
                ApiError::InternalServerError(format!("governance counts failed: {e}"))
            })?;

            let scopes = repo::governance_scope_breakdown(&conn, state.server_id).map_err(|e| {
                ApiError::InternalServerError(format!("scope breakdown failed: {e}"))
            })?;

            Ok(GovernanceSummaryResponse {
                total_transfers: counts.total_transfers,
                unique_bundles: counts.unique_bundles,
                unique_sources: counts.unique_sources,
                unique_destinations: counts.unique_destinations,
                redacted_transfers: counts.redacted_transfers,
                by_scope: scopes
                    .into_iter()
                    .map(|s| ScopeBreakdown {
                        scope: s.scope,
                        count: s.count,
                    })
                    .collect(),
            })
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))?
    }
}

/// SSRF gate for the RTX relay outbound path. Returns `true` when the peer's
/// `base_url` must NOT be contacted.
///
/// Honours `federation.allow_private_peer_addresses`, and did not until
/// 2026-09-15: it called `is_url_private_or_reserved` unconditionally while the
/// message relay called `is_peer_url_disallowed(url, allow_private)`, so an
/// operator who set that flag — documented as what makes two servers on a LAN,
/// two containers addressing each other by Compose service name, and peers
/// across a VPN possible — got messages relayed and RTX bundles silently
/// dropped at the same peer. The comment beside the call site claimed to mirror
/// the message path, which is how the divergence survived.
///
/// Malformed and non-http(s) URLs stay refused whatever the flag says; the flag
/// lifts only the private-address half.
pub(crate) fn rtx_peer_url_is_private_or_reserved(base_url: &str, allow_private: bool) -> bool {
    crate::api_link_preview::is_peer_url_disallowed(base_url, allow_private)
}

/// Background relay: post a freshly published RTX bundle to every active
/// federation peer's `/api/federation/rtx` endpoint. Spawned from
/// `RtxService::publish_bundle` and re-exported through `api_rtx` so the
/// existing `tokio::spawn(relay_rtx_bundles(state, bundle))` call sites
/// continue to work.
///
/// Behaviour preserved verbatim:
///   * Skip peers whose `transfer_scope == "NO_TRANSFER"` or unparseable.
///   * Skip peers whose `base_url` already appears in the relay path or
///     equals the bundle's `source_server` (cycle prevention).
///   * Skip peers whose `base_url` is private/loopback/link-local
///     (SSRF defence-in-depth — mirrors `federation_service::relay_message`).
///   * Apply each peer's transfer scope to the bundle (may strip
///     `reasoning_chain` when the scope is `ReflectionSummariesOnly`).
///   * Sign the relay envelope with the canonical
///     `rtx_relay_signing_payload`.
///   * Build provenance with `origin_server = bundle.source_server`,
///     `relay_path = [local_public_url]`.
///   * Per-peer POST is fire-and-forget under `tokio::spawn`; non-success
///     responses and network errors are logged but not retried.
pub async fn relay_rtx_bundles(state: Arc<AppState>, bundle: ReflectionSummaryBundle) {
    // A locally-published bundle: this server IS the origin, so it mints the
    // attestation and starts an empty chain.
    let origin_server = state.get_public_url();
    let max_hops = state.federation_config.rtx_max_hops;
    let attestation =
        sign_origin_attestation(&bundle, &origin_server, &state.signing_key, max_hops);
    let provenance = BundleProvenance {
        origin_server,
        relay_path: Vec::new(),
        bundle_id: bundle.bundle_id.clone(),
        hops: Vec::new(),
        origin: Some(attestation),
    };
    relay_rtx_bundle_onwards(state, bundle, provenance).await
}

/// Forward a bundle to every eligible peer, extending the provenance chain by
/// one hop.
///
/// Called with an empty chain by [`relay_rtx_bundles`] for a local publish, and
/// with the received chain by `federation_service::receive_federated_rtx` — which
/// is what makes a second hop possible at all. The previous code reset
/// `relay_path` to `vec![local_public_url]` on every send, so the list could
/// never contain more than one entry, the cycle check could only ever see this
/// server, and nothing re-relayed: `receive_federated_rtx` stored and fanned out
/// locally and stopped. "Single-hop only" understated it — multi-hop did not
/// exist, and the two pieces the ROADMAP recorded as done (the circular-relay
/// check and the origin check) were guarding a path no bundle could take.
pub async fn relay_rtx_bundle_onwards(
    state: Arc<AppState>,
    bundle: ReflectionSummaryBundle,
    provenance: BundleProvenance,
) {
    let peers = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        move || -> Result<Vec<repo::FederationPeerRelayTarget>, String> {
            let conn = pool.get().map_err(|e| e.to_string())?;
            repo::list_active_federation_peers(&conn, server_id).map_err(|e| e.to_string())
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));

    let peers = match peers {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Failed to fetch federation peers for RTX relay: {}", e);
            return;
        }
    };

    if peers.is_empty() {
        return;
    }

    let client = match crate::api_federation::federation_http_client() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                "failed to build federation HTTP client for RTX relay: {}",
                e
            );
            return;
        }
    };

    let local_url = state.get_public_url();

    // A server that cannot name itself cannot prove it is not already in the
    // chain, and every loop check below is a comparison against this value.
    // Refusing here is the difference between "no cycle detection" and "cycle
    // detection that silently passes".
    if local_url.is_empty() {
        tracing::warn!(
            bundle_id = %bundle.bundle_id,
            "not relaying: this server has no public URL, so it cannot add an attributable hop"
        );
        return;
    }

    let origin_signature = provenance
        .origin
        .as_ref()
        .map(|o| o.signature.clone())
        .unwrap_or_default();

    // An unsigned chain is not something to extend. An envelope from a peer on
    // an older build arrives with no attestation; it is accepted and delivered
    // locally (see `receive_federated_rtx`) and stops there.
    let Some(attestation) = provenance.origin.clone() else {
        tracing::debug!(
            bundle_id = %bundle.bundle_id,
            "not re-relaying a bundle that carries no origin attestation"
        );
        return;
    };

    // The budget: what the publisher asked for, bounded by what this operator
    // will carry. Neither alone is right — an origin limiting its own blast
    // radius and an operator limiting relay depth are different questions.
    let budget = (attestation.max_hops as usize).min(annex_rtx::RTX_HOP_CEILING);
    if provenance.hops.len() >= budget {
        tracing::debug!(
            bundle_id = %bundle.bundle_id,
            hops = provenance.hops.len(),
            budget,
            "not re-relaying: hop budget exhausted"
        );
        return;
    }

    // The payload the last hop signed, which our hop chains onto. Computed with
    // the shared helper rather than rebuilt here, because a second
    // implementation of this loop is a second chance to disagree with the
    // receiver about what was signed.
    let prev_chain_digest = annex_rtx::hop_payloads(&provenance, &origin_signature, &local_url)
        .last()
        .map(|p| annex_rtx::chain_digest(p))
        .unwrap_or_default();
    let hop_index = provenance.hops.len();

    for peer in peers {
        // Skip peers already in the chain (prevent cycles), the origin, and
        // ourselves. The first of those is a real list now: it used to be a
        // one-element vector containing this server, so it could only detect a
        // cycle back to us.
        if provenance.hops.iter().any(|h| h.server == peer.base_url)
            || provenance.origin_server == peer.base_url
            || local_url == peer.base_url
            || bundle.source_server == peer.base_url
        {
            tracing::debug!(
                peer = %peer.base_url,
                bundle_id = %bundle.bundle_id,
                "skipping RTX relay to peer: already in relay path or is origin"
            );
            continue;
        }

        // SSRF defence-in-depth: skip peers whose base_url resolves to a
        // private/loopback/link-local host. The instances table is
        // operator-controlled but a misconfigured row would otherwise
        // turn this background relay into a continuous probe of internal
        // services (and re-emit signed RTX envelopes to internal hosts).
        // Mirrors the guard in `federation_service::relay_message`.
        if rtx_peer_url_is_private_or_reserved(
            &peer.base_url,
            state.federation_config.allow_private_peer_addresses,
        ) {
            tracing::warn!(
                peer = %peer.base_url,
                bundle_id = %bundle.bundle_id,
                "skipping RTX relay: peer base_url resolves to a private or reserved host"
            );
            continue;
        }

        let scope = match parse_transfer_scope(&peer.transfer_scope) {
            Some(s) if s >= VrpTransferScope::ReflectionSummariesOnly => s,
            _ => {
                tracing::warn!(
                    peer = %peer.base_url,
                    bundle_id = %bundle.bundle_id,
                    scope = %peer.transfer_scope,
                    "skipping RTX federation relay: transfer scope is NoTransfer or unparseable"
                );
                continue;
            }
        };

        // Apply federation transfer scope to the bundle
        let scoped_bundle = match enforce_transfer_scope(&bundle, scope) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(
                    peer = %peer.base_url,
                    bundle_id = %bundle.bundle_id,
                    scope = %scope.to_string(),
                    "federation RTX scope enforcement failed; skipping relay: {}",
                    e
                );
                continue;
            }
        };

        // Our hop, signed over the content we are actually sending (post-scope),
        // the peer we are sending it to, and the digest of the hop before us.
        //
        // The content hash is per-hop for a reason: scope enforcement rewrites
        // the bundle in flight, so the bytes leaving here legitimately differ
        // from the bytes that arrived. An origin-level signature over the full
        // content would fail at every downstream hop, for the correct behaviour
        // of the system — which is why the origin signs a scope-invariant digest
        // plus a commitment to the reasoning chain instead.
        let content_hash = rtx_bundle_content_hash(&scoped_bundle);
        let hop_payload = annex_rtx::relay_hop_payload(
            &bundle.bundle_id,
            &provenance.origin_server,
            &origin_signature,
            hop_index,
            &local_url,
            &peer.base_url,
            &content_hash,
            &prev_chain_digest,
        );
        let hop_signature = {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(hop_payload.as_bytes());
            hex::encode(state.signing_key.sign(&digest).to_bytes())
        };

        let mut hops = provenance.hops.clone();
        hops.push(RelayHop {
            server: local_url.clone(),
            content_hash: content_hash.clone(),
            signature: hop_signature,
        });
        let outbound_provenance = BundleProvenance {
            origin_server: provenance.origin_server.clone(),
            // Mirrored from `hops` so a peer on an older build still parses the
            // envelope. Nothing signs it; nothing should read it for a trust
            // decision.
            relay_path: hops.iter().map(|h| h.server.clone()).collect(),
            bundle_id: bundle.bundle_id.clone(),
            hops,
            origin: Some(attestation.clone()),
        };

        // The legacy envelope signature, kept populated for one release so a
        // peer that only knows how to verify THIS still accepts the envelope.
        // It covers the mirrored `relay_path`, which is why it cannot be the
        // thing a multi-hop receiver trusts.
        let legacy_payload = rtx_relay_signing_payload(
            &bundle.bundle_id,
            &local_url,
            &outbound_provenance.origin_server,
            &outbound_provenance.relay_path,
            &content_hash,
        );
        let signature_hex =
            hex::encode(state.signing_key.sign(legacy_payload.as_bytes()).to_bytes());

        let envelope = FederatedRtxEnvelope {
            bundle: scoped_bundle,
            provenance: outbound_provenance,
            relaying_server: local_url.clone(),
            signature: signature_hex,
        };

        let url = format!("{}/api/federation/rtx", peer.base_url);
        let client_clone = client.clone();

        tokio::spawn(async move {
            match client_clone.post(&url).json(&envelope).send().await {
                Ok(resp) if !resp.status().is_success() => {
                    tracing::warn!("RTX relay to {} returned status {}", url, resp.status());
                }
                Err(e) => {
                    tracing::warn!("Failed to relay RTX bundle to {}: {}", url, e);
                }
                Ok(_) => {
                    tracing::debug!(
                        "RTX bundle {} relayed to {}",
                        envelope.bundle.bundle_id,
                        url
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn author_test_bundle() -> ReflectionSummaryBundle {
        ReflectionSummaryBundle {
            bundle_id: "bundle-1".into(),
            source_pseudonym: "agent-1".into(),
            source_server: "http://localhost:3000".into(),
            domain_tags: vec!["rust".into(), "security".into()],
            summary: "a distilled reflection".into(),
            reasoning_chain: Some("step 1; step 2".into()),
            caveats: vec!["low confidence".into()],
            created_at: 1_700_000_000_000,
            signature: String::new(),
            vrp_handshake_ref: "1:2:3".into(),
        }
    }

    #[test]
    fn author_signature_round_trips_and_rejects_tampering() {
        let sk_bytes = [7u8; 32];
        let pubkey_hex = {
            use ed25519_dalek::SigningKey;
            hex::encode(SigningKey::from_bytes(&sk_bytes).verifying_key().to_bytes())
        };

        let mut bundle = author_test_bundle();
        bundle.signature = sign_bundle_author(&bundle, &sk_bytes);

        // Correct signature verifies.
        assert!(verify_bundle_author_signature(&bundle, &pubkey_hex).is_ok());

        // Tamper with a content field → signature no longer valid.
        let mut tampered = bundle.clone();
        tampered.summary = "a MALICIOUSLY altered reflection".into();
        assert!(
            verify_bundle_author_signature(&tampered, &pubkey_hex).is_err(),
            "altering the summary must invalidate the author signature"
        );

        // Tamper with reasoning_chain → rejected.
        let mut tampered2 = bundle.clone();
        tampered2.reasoning_chain = Some("step 1; step 2; exfiltrate".into());
        assert!(verify_bundle_author_signature(&tampered2, &pubkey_hex).is_err());

        // Wrong key → rejected.
        let other_pubkey = {
            use ed25519_dalek::SigningKey;
            hex::encode(
                SigningKey::from_bytes(&[9u8; 32])
                    .verifying_key()
                    .to_bytes(),
            )
        };
        assert!(verify_bundle_author_signature(&bundle, &other_pubkey).is_err());

        // Malformed key / signature → rejected, not panicking.
        assert!(verify_bundle_author_signature(&bundle, "nothex").is_err());
        let mut bad_sig = bundle.clone();
        bad_sig.signature = "00".into();
        assert!(verify_bundle_author_signature(&bad_sig, &pubkey_hex).is_err());
    }

    #[test]
    fn test_parse_transfer_scope() {
        assert_eq!(
            parse_transfer_scope("FULL_KNOWLEDGE_BUNDLE"),
            Some(VrpTransferScope::FullKnowledgeBundle)
        );
        assert_eq!(
            parse_transfer_scope("REFLECTION_SUMMARIES_ONLY"),
            Some(VrpTransferScope::ReflectionSummariesOnly)
        );
        assert_eq!(
            parse_transfer_scope("NO_TRANSFER"),
            Some(VrpTransferScope::NoTransfer)
        );
        assert_eq!(parse_transfer_scope("UNKNOWN"), None);
    }

    #[test]
    fn test_extract_redacted_topics_with_field() {
        let json = r#"{"required_capabilities":[],"offered_capabilities":[],"redacted_topics":["politics","finance"]}"#;
        let topics = extract_redacted_topics(json).unwrap();
        assert_eq!(topics, vec!["politics", "finance"]);
    }

    #[test]
    fn test_extract_redacted_topics_without_field() {
        let json = r#"{"required_capabilities":[],"offered_capabilities":[]}"#;
        let topics = extract_redacted_topics(json).unwrap();
        assert!(topics.is_empty());
    }

    #[test]
    fn test_extract_redacted_topics_invalid_json() {
        // Corrupted JSON now fails closed (returns Err) to prevent bypass.
        assert!(extract_redacted_topics("not json").is_err());
    }

    #[test]
    fn test_extract_redacted_topics_truncated_json() {
        // Simulates data corruption: truncated JSON string.
        assert!(extract_redacted_topics(r#"{"required_capabilities":["#).is_err());
    }

    #[test]
    fn test_extract_redacted_topics_wrong_type() {
        // JSON is valid but wrong shape — field is a string, not array.
        assert!(extract_redacted_topics(r#"{"redacted_topics": "not_an_array"}"#).is_err());
    }

    #[test]
    fn test_rtx_relay_signing_payload_deterministic() {
        let p1 = rtx_relay_signing_payload("b1", "relay", "origin", &["hop1".into()], "deadbeef");
        let p2 = rtx_relay_signing_payload("b1", "relay", "origin", &["hop1".into()], "deadbeef");
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_rtx_relay_signing_payload_binds_content_hash() {
        let base = rtx_relay_signing_payload("b1", "relay", "origin", &["hop1".into()], "aaaa");
        let tampered = rtx_relay_signing_payload("b1", "relay", "origin", &["hop1".into()], "bbbb");
        assert_ne!(
            base, tampered,
            "a different content hash must produce a different signing payload"
        );
    }

    #[test]
    fn test_rtx_relay_signing_payload_multi_hop() {
        let payload = rtx_relay_signing_payload(
            "bundle-123",
            "http://relay.com",
            "http://origin.com",
            &["http://hop1.com".into(), "http://hop2.com".into()],
            "abc123",
        );
        assert_eq!(
            payload,
            "bundle-123\nhttp://relay.com\nhttp://origin.com\nhttp://hop1.com|http://hop2.com\nabc123"
        );
    }

    #[test]
    fn test_rtx_bundle_content_hash_detects_tampering() {
        use annex_rtx::ReflectionSummaryBundle;
        let mk = |summary: &str| ReflectionSummaryBundle {
            bundle_id: "b1".into(),
            source_pseudonym: "p1".into(),
            source_server: "http://origin".into(),
            domain_tags: vec!["rust".into()],
            summary: summary.into(),
            reasoning_chain: Some("chain".into()),
            caveats: vec!["c1".into()],
            created_at: 1,
            signature: "sig".into(),
            vrp_handshake_ref: "r".into(),
        };
        let a = rtx_bundle_content_hash(&mk("hello"));
        let b = rtx_bundle_content_hash(&mk("hello"));
        let c = rtx_bundle_content_hash(&mk("HELLO"));
        assert_eq!(a, b, "identical content must hash identically");
        assert_ne!(a, c, "altered content must change the hash");
    }

    #[test]
    fn rtx_peer_url_is_private_or_reserved_blocks_loopback() {
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://127.0.0.1:9000",
            false
        ));
        assert!(rtx_peer_url_is_private_or_reserved(
            "https://localhost/api/federation/rtx",
            false
        ));
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://[::1]:9000",
            false
        ));
    }

    #[test]
    fn the_flag_lifts_the_private_check_and_nothing_else() {
        // The divergence this pair pins: the message relay honoured
        // `allow_private_peer_addresses` and this gate did not, so an operator
        // who set it got messages relayed and RTX bundles dropped at the same
        // peer, with only a warn line to say so.
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://10.0.0.5",
            false
        ));
        assert!(
            !rtx_peer_url_is_private_or_reserved("http://10.0.0.5", true),
            "the flag must lift the private-address half",
        );
        assert!(
            rtx_peer_url_is_private_or_reserved("file:///etc/hostname", true),
            "the flag must NOT lift the scheme check",
        );
        assert!(
            rtx_peer_url_is_private_or_reserved("not a url", true),
            "the flag must NOT lift the parse check",
        );
    }

    #[test]
    fn rtx_peer_url_is_private_or_reserved_blocks_private_ranges() {
        // RFC1918
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://10.0.0.5",
            false
        ));
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://172.16.0.1",
            false
        ));
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://192.168.1.1",
            false
        ));
        // Link-local (cloud metadata)
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://169.254.169.254",
            false
        ));
        // CGNAT
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://100.64.0.1",
            false
        ));
        // IPv4-mapped IPv6
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://[::ffff:10.0.0.1]",
            false
        ));
        // Reserved hostnames
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://server.local/api/federation/rtx",
            false
        ));
        assert!(rtx_peer_url_is_private_or_reserved(
            "https://service.internal",
            false
        ));
        assert!(rtx_peer_url_is_private_or_reserved(
            "http://metadata.google.internal/",
            false
        ));
    }

    #[test]
    fn rtx_peer_url_is_private_or_reserved_blocks_unparseable_or_non_http() {
        // Unparseable URLs — fail closed.
        assert!(rtx_peer_url_is_private_or_reserved("not a url", false));
        assert!(rtx_peer_url_is_private_or_reserved("", false));
        // Non-http(s) schemes — fail closed (no relay over file://, ftp://, etc.).
        assert!(rtx_peer_url_is_private_or_reserved(
            "file:///etc/hostname",
            false
        ));
        assert!(rtx_peer_url_is_private_or_reserved(
            "ftp://example.com/api/federation/rtx",
            false
        ));
    }

    #[test]
    fn rtx_peer_url_is_private_or_reserved_allows_public_hosts() {
        assert!(!rtx_peer_url_is_private_or_reserved(
            "https://annex-peer.example.com",
            false
        ));
        assert!(!rtx_peer_url_is_private_or_reserved(
            "http://203.0.113.42:8080/api/federation/rtx",
            false
        ));
        // Public IPv6 (2001:db8::/32 is documentation-reserved per RFC3849
        // but is not in our private/loopback set, so it's allowed at this
        // layer; the wire shape is what matters here).
        assert!(!rtx_peer_url_is_private_or_reserved(
            "http://[2001:db8::1]/api/federation/rtx",
            false
        ));
    }
}

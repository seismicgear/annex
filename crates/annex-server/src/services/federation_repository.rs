//! Federation persistence: pure SQL helpers for the `instances`,
//! `federation_agreements`, `federated_identities`, and `rtx_*` tables
//! plus the `find_commitment_for_pseudonym` lookup that resolves a local
//! pseudonym back to its identity commitment.
//!
//! No policy, no crypto, no networking lives here. Each function takes
//! an `&rusqlite::Connection` (or `&rusqlite::Transaction`) and returns
//! parsed rows. Behavioural quirks of the existing inline code — such as
//! the `'unknown'` attestation-ref fallback when no commitment is
//! registered, or the `ON CONFLICT … DO UPDATE` upserts on
//! `federated_identities` and `platform_identities` — are preserved
//! verbatim by [`crate::services::federation_service`] consuming these
//! helpers.
//!
//! Shape:
//!   * `find_*` / `list_*` / `*_exists` are reads.
//!   * `upsert_*` / `insert_*` / `log_*` are writes (each takes an
//!     `&rusqlite::Transaction`).

use annex_vrp::VrpFederationHandshake;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

/// One row of `instances` selected by `base_url`.
#[derive(Debug, Clone)]
pub(crate) struct RemoteInstance {
    pub id: i64,
    pub public_key_hex: String,
    pub status: String,
}

/// One row of `federated_identities` selected by `(remote_instance_id, commitment_hex)`.
#[derive(Debug, Clone)]
pub(crate) struct FederatedIdentityRow {
    pub pseudonym_id: String,
    /// Empty string when no root was recorded at attestation time.
    pub root_hex_at_verification: String,
}

/// One row of `rtx_subscriptions` joined with `agent_registrations`.
#[derive(Debug, Clone)]
pub(crate) struct RtxFederatedSubscriber {
    pub pseudonym: String,
    pub domain_filters_json: String,
    pub transfer_scope_str: String,
}

/// One row used by `relay_message` to enumerate active federation peers.
/// `id` is the `instances.id` primary key — the outbox keys on this
/// rather than `base_url` so a peer whose URL changes still has a
/// stable delivery target.
#[derive(Debug, Clone)]
pub(crate) struct ActiveFederationPeer {
    pub id: i64,
    pub base_url: String,
    pub transfer_scope: String,
}

/// Look up `(id, public_key, status)` for an instance keyed by `base_url`.
pub(crate) fn find_instance_by_base_url(
    conn: &Connection,
    base_url: &str,
) -> Result<Option<RemoteInstance>, rusqlite::Error> {
    conn.query_row(
        "SELECT id, public_key, status FROM instances WHERE base_url = ?1",
        params![base_url],
        |row| {
            Ok(RemoteInstance {
                id: row.get(0)?,
                public_key_hex: row.get(1)?,
                status: row.get(2)?,
            })
        },
    )
    .optional()
}

/// Look up `(id, public_key)` for an instance keyed by `base_url`.
/// Used by handshake and attestation paths that don't need `status`.
pub(crate) fn find_instance_id_and_key(
    conn: &Connection,
    base_url: &str,
) -> Result<Option<(i64, String)>, rusqlite::Error> {
    conn.query_row(
        "SELECT id, public_key FROM instances WHERE base_url = ?1",
        params![base_url],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    )
    .optional()
}

/// Returns true iff the named base_url is a known instance.
pub(crate) fn instance_known(conn: &Connection, base_url: &str) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM instances WHERE base_url = ?1)",
        params![base_url],
        |row| row.get(0),
    )
}

/// Returns true iff there is an active federation_agreements row between
/// `local_server_id` and `remote_instance_id`.
///
/// `instances` rows are deployment-global, but agreements are per local
/// server — every lookup must scope on `local_server_id` or a co-hosted
/// server's agreement with the same remote would leak across tenants.
pub(crate) fn has_active_agreement(
    conn: &Connection,
    local_server_id: i64,
    remote_instance_id: i64,
) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM federation_agreements WHERE local_server_id = ?1 AND remote_instance_id = ?2 AND active = 1)",
        params![local_server_id, remote_instance_id],
        |row| row.get(0),
    )
}

/// Returns the `transfer_scope` string of the active agreement between
/// `local_server_id` and `remote_instance_id`, or `None` when there is no
/// active agreement.
pub(crate) fn active_agreement_transfer_scope(
    conn: &Connection,
    local_server_id: i64,
    remote_instance_id: i64,
) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row(
        "SELECT transfer_scope FROM federation_agreements
         WHERE local_server_id = ?1 AND remote_instance_id = ?2 AND active = 1",
        params![local_server_id, remote_instance_id],
        |row| row.get(0),
    )
    .optional()
}

/// Returns the redacted_topics list declared in the remote peer's
/// capability_contract for the active agreement between `local_server_id`
/// and `remote_instance_id`. On any read / parse failure returns an empty
/// vector — matching the existing inline behaviour where corrupt or
/// missing handshake JSON is treated as "no redactions to enforce".
///
/// That is a FAIL-OPEN, and the caller
/// (`federation_service`'s inbound relay path) skips `check_redacted_topics`
/// entirely on an empty list — so a handshake row that will not parse means
/// the peer's declared redactions are not enforced against anything it sends
/// us. Until this session the three causes were indistinguishable from a peer
/// that simply declared none, and produced no signal at all. Behaviour is
/// unchanged — rejecting instead would cut off a peer whose stored handshake
/// predates a schema change, which is a deployment decision, not a bug fix —
/// but the operator now gets a line naming the peer, which is the difference
/// between a policy that is off and a policy that is off silently.
pub(crate) fn active_agreement_redacted_topics(
    conn: &Connection,
    local_server_id: i64,
    remote_instance_id: i64,
) -> Vec<String> {
    let stored = match conn.query_row(
        "SELECT remote_handshake_json FROM federation_agreements
         WHERE local_server_id = ?1 AND remote_instance_id = ?2 AND active = 1",
        params![local_server_id, remote_instance_id],
        |row| row.get::<_, Option<String>>(0),
    ) {
        Ok(Some(json)) => json,
        // No active agreement row, or the column is NULL. The caller has
        // already resolved an agreement to get here, so a missing row is
        // worth saying out loud.
        Ok(None) => return Vec::new(),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            tracing::warn!(
                local_server_id,
                remote_instance_id,
                "no active federation agreement while resolving redacted topics — \
                 declared redactions will not be enforced"
            );
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(
                local_server_id,
                remote_instance_id,
                error = %e,
                "could not read the remote handshake while resolving redacted topics — \
                 declared redactions will not be enforced"
            );
            return Vec::new();
        }
    };

    match serde_json::from_str::<VrpFederationHandshake>(&stored) {
        Ok(h) => h.capability_contract.redacted_topics,
        Err(e) => {
            tracing::warn!(
                local_server_id,
                remote_instance_id,
                error = %e,
                "stored remote handshake does not parse — declared redactions will \
                 not be enforced for this peer"
            );
            Vec::new()
        }
    }
}

/// Look up the federated identity row for a `(remote_instance_id,
/// commitment_hex)` pair. Returns `None` when the identity has not been
/// attested.
pub(crate) fn find_federated_identity_by_commitment(
    conn: &Connection,
    remote_instance_id: i64,
    commitment_hex: &str,
) -> Result<Option<FederatedIdentityRow>, rusqlite::Error> {
    conn.query_row(
        "SELECT pseudonym_id, COALESCE(root_hex_at_verification, '') FROM federated_identities
         WHERE remote_instance_id = ?1 AND commitment_hex = ?2",
        params![remote_instance_id, commitment_hex],
        |row| {
            Ok(FederatedIdentityRow {
                pseudonym_id: row.get(0)?,
                root_hex_at_verification: row.get(1)?,
            })
        },
    )
    .optional()
}

/// Returns true iff `(remote_instance_id, pseudonym_id)` exists in
/// `federated_identities`.
pub(crate) fn federated_identity_exists(
    conn: &Connection,
    remote_instance_id: i64,
    pseudonym_id: &str,
) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM federated_identities WHERE remote_instance_id = ?1 AND pseudonym_id = ?2)",
        params![remote_instance_id, pseudonym_id],
        |row| row.get(0),
    )
}

/// `INSERT OR UPDATE` into `federated_identities` for an attestation.
/// Mirrors the existing inline statement byte-for-byte.
pub(crate) fn upsert_federated_identity(
    tx: &Transaction<'_>,
    server_id: i64,
    remote_instance_id: i64,
    commitment_hex: &str,
    pseudonym_id: &str,
    vrp_topic: &str,
    root_hex_at_verification: &str,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO federated_identities (
            server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic, attested_at,
            root_hex_at_verification, last_verified_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), ?6, datetime('now'))
        ON CONFLICT(server_id, remote_instance_id, pseudonym_id) DO UPDATE SET
            attested_at = datetime('now'),
            commitment_hex = excluded.commitment_hex,
            vrp_topic = excluded.vrp_topic,
            root_hex_at_verification = excluded.root_hex_at_verification,
            last_verified_at = datetime('now')
        ",
        params![
            server_id,
            remote_instance_id,
            commitment_hex,
            pseudonym_id,
            vrp_topic,
            root_hex_at_verification
        ],
    )?;
    Ok(())
}

/// Ensure a `platform_identities` row exists / is active for this pseudonym,
/// promoting `participant_type` if it changed. Mirrors the existing inline
/// `INSERT … ON CONFLICT … DO UPDATE` byte-for-byte.
pub(crate) fn upsert_platform_identity(
    tx: &Transaction<'_>,
    server_id: i64,
    pseudonym_id: &str,
    participant_type: &str,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO platform_identities (
            server_id, pseudonym_id, participant_type, active
        ) VALUES (?1, ?2, ?3, 1)
        ON CONFLICT(server_id, pseudonym_id) DO UPDATE SET
            active = 1,
            participant_type = excluded.participant_type
        ",
        params![server_id, pseudonym_id, participant_type],
    )?;
    Ok(())
}

/// Resolves the `(commitment_hex, topic)` pair associated with a
/// pseudonym. Two-tier lookup: indexed fast-path on the denormalised
/// columns introduced in migration 024, with a slow legacy fallback for
/// rows that predate the migration. Behaviour preserved from the
/// previous inline implementation.
pub(crate) fn find_commitment_for_pseudonym(
    conn: &Connection,
    pseudonym: &str,
) -> Result<Option<(String, String)>, rusqlite::Error> {
    // Fast path: indexed lookup on denormalized columns (O(1)).
    let fast_result: Option<(String, String)> = conn
        .query_row(
            "SELECT commitment_hex, topic FROM zk_nullifiers \
             WHERE pseudonym_id = ?1 AND commitment_hex IS NOT NULL \
             LIMIT 1",
            [pseudonym],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    if fast_result.is_some() {
        return Ok(fast_result);
    }

    // Slow fallback: only scan legacy rows that lack the denormalized columns.
    // Once all rows are backfilled this path becomes a no-op.
    let mut stmt =
        conn.prepare("SELECT topic, nullifier_hex FROM zk_nullifiers WHERE pseudonym_id IS NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut candidate_nullifiers = Vec::new();

    for row in rows {
        let (topic, nullifier_hex) = row?;
        match annex_identity::derive_pseudonym_id(&topic, &nullifier_hex) {
            Ok(p) => {
                if p == pseudonym {
                    candidate_nullifiers.push((topic, nullifier_hex));
                }
            }
            Err(e) => {
                tracing::warn!(
                    topic = %topic,
                    "failed to derive pseudonym_id from legacy nullifier row: {}", e
                );
            }
        }
    }

    if candidate_nullifiers.is_empty() {
        return Ok(None);
    }

    let mut id_stmt = conn.prepare("SELECT commitment_hex FROM vrp_identities")?;
    let commitments: Vec<String> = id_stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for (topic, nullifier) in candidate_nullifiers {
        for commitment in &commitments {
            match annex_identity::derive_nullifier_hex(commitment, &topic) {
                Ok(n) => {
                    if n == nullifier {
                        return Ok(Some((commitment.clone(), topic)));
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        commitment = %commitment,
                        topic = %topic,
                        "failed to derive nullifier hex in legacy commitment scan: {}", e
                    );
                }
            }
        }
    }

    Ok(None)
}

/// List the active federation peers (base_url + transfer_scope) the local
/// server has agreements with. Used by `relay_message` to fan out a local
/// channel message to subscribed instances.
pub(crate) fn list_active_peers(
    conn: &Connection,
    server_id: i64,
) -> Result<Vec<ActiveFederationPeer>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT i.id, i.base_url, fa.transfer_scope
         FROM federation_agreements fa
         JOIN instances i ON fa.remote_instance_id = i.id
         WHERE fa.local_server_id = ?1 AND fa.active = 1 AND i.status = 'ACTIVE'",
    )?;

    let rows = stmt.query_map(params![server_id], |row| {
        Ok(ActiveFederationPeer {
            id: row.get::<_, i64>(0)?,
            base_url: row.get::<_, String>(1)?,
            transfer_scope: row.get::<_, String>(2)?,
        })
    })?;

    let mut peers = Vec::new();
    for row in rows {
        peers.push(row?);
    }
    Ok(peers)
}

/// Insert a freshly received RTX bundle. Returns
/// `Ok(true)` when the row was inserted, `Ok(false)` on the
/// idempotent-duplicate path (UNIQUE violation on `bundle_id`). Other
/// errors propagate.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_rtx_bundle(
    tx: &Transaction<'_>,
    server_id: i64,
    bundle_id: &str,
    source_pseudonym: &str,
    source_server: &str,
    domain_tags_json: &str,
    summary: &str,
    reasoning_chain: Option<&str>,
    caveats_json: &str,
    created_at_ms: i64,
    signature: &str,
    vrp_handshake_ref: &str,
    provenance_json: &str,
) -> Result<bool, rusqlite::Error> {
    let result = tx.execute(
        "INSERT INTO rtx_bundles (
            server_id, bundle_id, source_pseudonym, source_server,
            domain_tags_json, summary, reasoning_chain, caveats_json,
            created_at_ms, signature, vrp_handshake_ref, provenance_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            server_id,
            bundle_id,
            source_pseudonym,
            source_server,
            domain_tags_json,
            summary,
            reasoning_chain,
            caveats_json,
            created_at_ms,
            signature,
            vrp_handshake_ref,
            provenance_json,
        ],
    );

    match result {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::SqliteFailure(ref err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            // Duplicate bundle (idempotent) — already received.
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Records one entry in `rtx_transfer_log`. `destination_pseudonym = NULL`
/// for the receive-side audit row; per-subscriber deliveries fill it in.
pub(crate) fn log_rtx_transfer(
    tx: &Transaction<'_>,
    server_id: i64,
    bundle_id: &str,
    source_pseudonym: &str,
    destination_pseudonym: Option<&str>,
    transfer_scope_applied: &str,
    redactions_applied: Option<&str>,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO rtx_transfer_log (
            server_id, bundle_id, source_pseudonym, destination_pseudonym,
            transfer_scope_applied, redactions_applied
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            server_id,
            bundle_id,
            source_pseudonym,
            destination_pseudonym,
            transfer_scope_applied,
            redactions_applied,
        ],
    )?;
    Ok(())
}

/// List subscribers eligible for federated RTX delivery: those who have
/// an active agent registration on this server with `accept_federated = 1`.
/// `domain_filters_json` and `transfer_scope` come straight from the row;
/// the service layer is responsible for parsing them and applying the
/// matching / scope-enforcement rules.
pub(crate) fn list_federated_rtx_subscribers(
    tx: &Transaction<'_>,
    server_id: i64,
) -> Result<Vec<RtxFederatedSubscriber>, rusqlite::Error> {
    let mut stmt = tx.prepare(
        "SELECT s.subscriber_pseudonym, s.domain_filters_json, a.transfer_scope
         FROM rtx_subscriptions s
         JOIN agent_registrations a
           ON a.server_id = s.server_id AND a.pseudonym_id = s.subscriber_pseudonym
         WHERE s.server_id = ?1 AND s.accept_federated = 1 AND a.active = 1",
    )?;

    let rows = stmt.query_map(params![server_id], |row| {
        Ok(RtxFederatedSubscriber {
            pseudonym: row.get::<_, String>(0)?,
            domain_filters_json: row.get::<_, String>(1)?,
            transfer_scope_str: row.get::<_, String>(2)?,
        })
    })?;

    let mut collected = Vec::new();
    for row in rows {
        collected.push(row?);
    }
    Ok(collected)
}

/// Returns `Some(updated_at)` for a known root, `None` for a root the
/// server has never persisted under `vrp_roots`. Used by the
/// `current_vrp_root` handler.
pub(crate) fn root_updated_at(
    conn: &Connection,
    root_hex: &str,
) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row(
        "SELECT created_at FROM vrp_roots WHERE root_hex = ?1",
        [root_hex],
        |row| row.get(0),
    )
    .optional()
}

#[cfg(test)]
mod tests {
    use super::*;
    use annex_db::run_migrations;
    use annex_types::ServerPolicy;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        run_migrations(&conn).expect("migrations");

        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).expect("policy json");
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local', ?1)",
            [policy_json],
        )
        .expect("seed local server");
        conn
    }

    /// `relay_message` constructs the outgoing envelope's `attestation_ref`
    /// by calling `find_commitment_for_pseudonym` and formatting
    /// `"{topic}:{commitment}"`. This test pins the fast-path lookup
    /// (denormalised columns) end-to-end so the relay's provenance string
    /// can never silently drift from the row that backs it.
    #[test]
    fn find_commitment_for_pseudonym_returns_topic_and_commitment_pair() {
        let conn = setup_db();

        // Insert a denormalised zk_nullifiers row that mimics what a
        // verify-membership flow would persist.
        conn.execute(
            "INSERT INTO zk_nullifiers (
                topic, nullifier_hex, pseudonym_id, commitment_hex
            ) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["annex:server:v1", "n-0123", "user-pseudo-1", "c-deadbeef"],
        )
        .expect("insert zk_nullifier row");

        let result =
            find_commitment_for_pseudonym(&conn, "user-pseudo-1").expect("lookup should not error");
        let (commitment, topic) = result.expect("known pseudonym should resolve");
        assert_eq!(
            commitment, "c-deadbeef",
            "commitment must match the row used by the relay's attestation_ref"
        );
        assert_eq!(
            topic, "annex:server:v1",
            "topic must match the row used by the relay's attestation_ref"
        );
    }

    /// Agreements are scoped per local server even though `instances`
    /// rows are deployment-global. Two co-hosted servers federating with
    /// the same remote must each see only their own agreement: scope,
    /// redactions, and existence checks must never leak across tenants.
    #[test]
    fn agreement_lookups_are_scoped_to_the_local_server() {
        let conn = setup_db();

        // Second local server sharing the database.
        let policy_json = serde_json::to_string(&ServerPolicy::default()).expect("policy json");
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('other', 'Other', ?1)",
            [policy_json],
        )
        .expect("seed second server");

        // One shared remote instance.
        conn.execute(
            "INSERT INTO instances (base_url, public_key, label) VALUES ('https://remote.example', 'aa', 'Remote')",
            [],
        )
        .expect("seed instance");
        let instance_id = conn.last_insert_rowid();

        // Server 1 has an active FullKnowledge agreement; server 2 has none.
        conn.execute(
            "INSERT INTO federation_agreements (
                local_server_id, remote_instance_id, alignment_status,
                transfer_scope, agreement_json, active
            ) VALUES (1, ?1, 'ALIGNED', 'FullKnowledge', '{}', 1)",
            params![instance_id],
        )
        .expect("seed agreement");

        assert!(has_active_agreement(&conn, 1, instance_id).expect("lookup"));
        assert!(
            !has_active_agreement(&conn, 2, instance_id).expect("lookup"),
            "server 2 must not inherit server 1's agreement"
        );

        assert_eq!(
            active_agreement_transfer_scope(&conn, 1, instance_id)
                .expect("lookup")
                .as_deref(),
            Some("FullKnowledge")
        );
        assert_eq!(
            active_agreement_transfer_scope(&conn, 2, instance_id).expect("lookup"),
            None,
            "server 2 must not see server 1's transfer scope"
        );

        assert!(active_agreement_redacted_topics(&conn, 2, instance_id).is_empty());
    }

    /// Pseudonyms with no nullifier row resolve to `None`. The relay path
    /// then falls back to the `"annex:server:v1:unknown"` provenance
    /// string — preserving the existing behaviour even when a sender has
    /// no registered commitment yet.
    #[test]
    fn find_commitment_for_pseudonym_returns_none_for_unknown_pseudonym() {
        let conn = setup_db();
        let result = find_commitment_for_pseudonym(&conn, "no-such-pseudonym")
            .expect("lookup should not error");
        assert!(
            result.is_none(),
            "unknown pseudonym must resolve to None, not panic or error"
        );
    }
}

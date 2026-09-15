//! Single-use authentication challenges for membership proofs.
//!
//! # Why this exists
//!
//! A `POST /api/zk/verify-membership` body used to be composed entirely of
//! values that are stable for a given member and topic — the Merkle root, the
//! commitment, the nullifier, the topic hash, and a Groth16 proof over exactly
//! those. Nothing in it said *when* it was produced or *who* was producing it
//! right now. The whole body was a bearer credential.
//!
//! The consequence was not theoretical. Capture one successful request (a proxy
//! log, a shared machine, a browser extension, a crash dump) and you can
//! re-submit it verbatim. The server verifies the proof, finds the nullifier
//! already consumed, reads that as an ordinary re-authentication, and mints a
//! session token **at the identity's current revocation epoch**. So revoking an
//! identity's sessions did not survive its own re-authentication path: the
//! attacker's replay produced a token minted after the bump, which verifies.
//! Holding `sk` was never required.
//!
//! # Why a nonce beside the proof would not have closed it
//!
//! An attacker replaying a captured proof can request a nonce of their own and
//! attach it. Anything the server can hand out on demand is available to
//! whoever is replaying. The freshness has to be *inside* the proof, so that
//! the verification equation itself rejects a proof produced for a different
//! attempt.
//!
//! `zk/circuits/membership_v2.circom` therefore takes `challenge` as a
//! constrained public input (`challengeSquared <== challenge * challenge`, the
//! same device Semaphore uses for its signal hash). Groth16 commits to every
//! public input, so a proof produced for challenge C fails verification when
//! presented with any other value. Replaying the proof means replaying its
//! challenge — and this module lets a challenge be spent exactly once.
//!
//! # What each challenge is bound to
//!
//! - **the attempt**: 253 bits from the OS CSPRNG, recorded once, consumed
//!   inside the same `IMMEDIATE` transaction that mints the session, so two
//!   concurrent presentations cannot both succeed.
//! - **the identity**: issued against one `commitment_hex`. A challenge
//!   observed in flight is useless to anybody else, and a proof for a
//!   different commitment cannot spend it.
//! - **the server**: rows are scoped to `server_id`, and the value never
//!   leaves this server's database, so a challenge from server A is simply
//!   absent at server B.
//! - **the topic**: a challenge issued for `annex:server:x:v2` cannot be spent
//!   on a different topic, which keeps the binding as tight as the pseudonym
//!   derivation it authorises.
//! - **time**: [`CHALLENGE_TTL_SECS`] from issue. Long enough for an in-browser
//!   Groth16 proof on slow hardware, short enough that a captured challenge is
//!   not a standing invitation.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use rand::RngCore;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::{api::ApiError, AppState};

/// How long an issued challenge remains spendable.
///
/// The client has to generate a Groth16 membership proof in between, which on
/// the slowest hardware this ships to (a low-end laptop running snarkjs in a
/// web worker) has been observed at well over a minute for the depth-20 tree.
/// Five minutes leaves room for that plus a retry, and is still short enough
/// that an intercepted challenge expires long before anyone could use it for
/// anything but the attempt it was issued for.
pub const CHALLENGE_TTL_SECS: i64 = 300;

/// How many unspent, unexpired challenges one commitment may hold at once.
///
/// Not a rate limit on requests — a limit on outstanding capability. A client
/// legitimately needs more than one (a retried proof, two tabs), and an
/// attacker gains nothing from a pile of challenges they cannot produce proofs
/// for, but an unbounded issue endpoint is a free write amplifier against the
/// database. When the cap is reached the OLDEST outstanding challenge is
/// dropped rather than the request refused, so a client that abandoned a proof
/// is never wedged out of signing in.
pub const MAX_OUTSTANDING_PER_COMMITMENT: usize = 8;

/// Request body for `POST /api/zk/challenge`.
#[derive(Debug, Deserialize)]
pub struct ChallengeRequest {
    /// The identity commitment the challenge is being requested for.
    pub commitment: String,
    /// The topic the resulting proof will be presented for.
    pub topic: String,
}

/// Response body for `POST /api/zk/challenge`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChallengeResponse {
    /// Canonical 64-character lowercase hex of the BN254 field element to feed
    /// into the circuit as the `challenge` public input.
    pub challenge: String,
    /// Seconds until the challenge stops being spendable.
    #[serde(rename = "expiresInSecs")]
    pub expires_in_secs: i64,
}

/// Reasons a challenge cannot be spent.
///
/// Distinguished rather than collapsed into one string: "I have never seen
/// this" and "this was already used" are different facts about an attempt, and
/// the second is the signature of a replay. Both are reported to the caller as
/// the same 401 — an attacker must not learn which — but they are logged
/// apart, which is the only way an operator sees a replay happening.
#[derive(Debug, PartialEq, Eq)]
pub enum ChallengeRejection {
    /// No such challenge was ever issued by this server.
    Unknown,
    /// Issued, but already spent. This is what a replay looks like.
    AlreadyConsumed,
    /// Issued, never spent, but past its TTL.
    Expired,
    /// Issued to a different commitment than the proof claims.
    WrongCommitment,
    /// Issued for a different topic than the proof claims.
    WrongTopic,
}

impl ChallengeRejection {
    /// A stable, low-cardinality label for logs and metrics.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::AlreadyConsumed => "already_consumed",
            Self::Expired => "expired",
            Self::WrongCommitment => "wrong_commitment",
            Self::WrongTopic => "wrong_topic",
        }
    }
}

/// Draw a fresh challenge value.
///
/// 32 CSPRNG bytes with the top three bits cleared. The BN254 scalar field
/// modulus is just above 2^253, so clearing to 253 bits makes every draw a
/// valid field element without a rejection loop — and, more importantly,
/// without the modular reduction that would make two distinct byte strings
/// collapse to one field element. `parse_canonical_fr_hex` rejects a
/// non-canonical encoding, so a reduced value would be unspendable anyway;
/// this makes the case impossible rather than merely detected.
fn draw_challenge_hex() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes[0] &= 0x1f;
    hex::encode(bytes)
}

/// Issue a single-use challenge. Blocking; call inside `spawn_blocking`.
pub fn issue_challenge(
    conn: &mut rusqlite::Connection,
    server_id: i64,
    commitment_hex: &str,
    topic: &str,
    now: i64,
) -> Result<ChallengeResponse, rusqlite::Error> {
    let challenge_hex = draw_challenge_hex();
    let expires_at = now + CHALLENGE_TTL_SECS;

    // IMMEDIATE: this reads the outstanding count and then writes, which is the
    // snapshot-conflict shape CLAUDE.md describes — a DEFERRED transaction
    // upgrading to a write under WAL fails with SQLITE_BUSY_SNAPSHOT
    // immediately, with the busy handler never invoked.
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    // Retire whatever has expired, for this commitment and in general. Doing it
    // on the issue path rather than in a sweeper task keeps the table bounded
    // without another moving part, and the work is proportional to what has
    // actually expired.
    tx.execute(
        "DELETE FROM zk_auth_challenges WHERE server_id = ?1 AND expires_at < ?2",
        rusqlite::params![server_id, now],
    )?;

    // Bound the outstanding set by dropping the oldest, not by refusing.
    tx.execute(
        "DELETE FROM zk_auth_challenges
         WHERE server_id = ?1 AND commitment_hex = ?2 AND consumed_at IS NULL
           AND rowid NOT IN (
               SELECT rowid FROM zk_auth_challenges
               WHERE server_id = ?1 AND commitment_hex = ?2 AND consumed_at IS NULL
               ORDER BY issued_at DESC, rowid DESC
               LIMIT ?3
           )",
        rusqlite::params![
            server_id,
            commitment_hex,
            (MAX_OUTSTANDING_PER_COMMITMENT - 1) as i64
        ],
    )?;

    tx.execute(
        "INSERT INTO zk_auth_challenges
             (challenge_hex, server_id, commitment_hex, topic, issued_at, expires_at, consumed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        rusqlite::params![
            challenge_hex,
            server_id,
            commitment_hex,
            topic,
            now,
            expires_at
        ],
    )?;
    tx.commit()?;

    Ok(ChallengeResponse {
        challenge: challenge_hex,
        expires_in_secs: CHALLENGE_TTL_SECS,
    })
}

/// Spend a challenge, or say why it cannot be spent.
///
/// Takes a transaction rather than a connection on purpose: the caller runs
/// this inside the same `IMMEDIATE` transaction that mints the session, so
/// "checked unspent" and "marked spent" cannot be separated by a concurrent
/// request. A version of this that took its own connection would leave exactly
/// the window it exists to close.
pub fn consume_challenge(
    tx: &rusqlite::Transaction<'_>,
    server_id: i64,
    challenge_hex: &str,
    commitment_hex: &str,
    topic: &str,
    now: i64,
) -> Result<Result<(), ChallengeRejection>, rusqlite::Error> {
    let row: Option<(String, String, i64, Option<i64>)> = tx
        .query_row(
            "SELECT commitment_hex, topic, expires_at, consumed_at
             FROM zk_auth_challenges
             WHERE server_id = ?1 AND challenge_hex = ?2",
            rusqlite::params![server_id, challenge_hex],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;

    let Some((issued_to, issued_topic, expires_at, consumed_at)) = row else {
        return Ok(Err(ChallengeRejection::Unknown));
    };
    if consumed_at.is_some() {
        return Ok(Err(ChallengeRejection::AlreadyConsumed));
    }
    if !issued_to.eq_ignore_ascii_case(commitment_hex) {
        return Ok(Err(ChallengeRejection::WrongCommitment));
    }
    if issued_topic != topic {
        return Ok(Err(ChallengeRejection::WrongTopic));
    }
    // Expiry is checked AFTER the binding checks so that a mismatched
    // commitment on an expired challenge is still reported as a mismatch in the
    // logs — the more interesting of the two facts.
    if expires_at < now {
        return Ok(Err(ChallengeRejection::Expired));
    }

    // Conditional on `consumed_at IS NULL` as well as the key, so that even if
    // this were ever called outside an IMMEDIATE transaction the second spender
    // updates zero rows rather than silently succeeding.
    let updated = tx.execute(
        "UPDATE zk_auth_challenges SET consumed_at = ?3
         WHERE server_id = ?1 AND challenge_hex = ?2 AND consumed_at IS NULL",
        rusqlite::params![server_id, challenge_hex, now],
    )?;
    if updated == 0 {
        return Ok(Err(ChallengeRejection::AlreadyConsumed));
    }
    Ok(Ok(()))
}

/// Handler for `POST /api/zk/challenge`.
///
/// Public, like `verify-membership` itself: a member who is signing in has no
/// session yet. It discloses nothing — the response is a random number — and
/// the outstanding-set cap above bounds what an unauthenticated caller can make
/// the server store.
pub async fn issue_challenge_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<ChallengeRequest>,
) -> Result<Json<ChallengeResponse>, ApiError> {
    if payload.commitment.trim().is_empty() {
        return Err(ApiError::BadRequest("commitment is required".to_string()));
    }
    if payload.topic.trim().is_empty() {
        return Err(ApiError::BadRequest("topic is required".to_string()));
    }
    // Validated here rather than at spend time so a malformed commitment is a
    // 400 on the cheap request instead of a confusing 401 after the client has
    // spent a minute generating a proof.
    annex_identity::zk::parse_fr_from_hex(&payload.commitment)
        .map_err(|_| ApiError::BadRequest("commitment is not a valid field element".to_string()))?;

    let now = chrono::Utc::now().timestamp();
    let resp = tokio::task::spawn_blocking(move || -> Result<ChallengeResponse, ApiError> {
        let mut conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;
        issue_challenge(
            &mut conn,
            state.server_id,
            &payload.commitment,
            &payload.topic,
            now,
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to issue challenge: {e}")))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(resp))
}

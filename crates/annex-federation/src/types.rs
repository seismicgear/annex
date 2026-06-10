use annex_rtx::{BundleProvenance, ReflectionSummaryBundle};
use annex_vrp::{
    VrpAlignmentStatus, VrpFederationHandshake, VrpTransferScope, VrpValidationReport,
};
use serde::{Deserialize, Serialize};

/// Represents an existing federation agreement between two servers.
#[derive(Debug, Serialize, Deserialize)]
pub struct FederationAgreement {
    pub id: i64,
    pub local_server_id: i64,
    pub remote_instance_id: i64,
    pub alignment_status: VrpAlignmentStatus,
    pub transfer_scope: VrpTransferScope,
    pub agreement_json: VrpValidationReport,
    pub remote_handshake_json: Option<VrpFederationHandshake>,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Request payload for cross-server identity attestation.
///
/// Wire compatibility: `protocol_version`, `public_signals`, `nullifier_hex`
/// and `topic_hash_hex` are all optional. A request that omits them is
/// processed as v1 — the only mode v0.1 peers know how to send. v2 peers
/// MUST send `protocol_version = "v2"` AND a `public_signals` array of
/// length 4 (`[root, commitment, nullifier, topicHash]`); the receiving
/// server cross-checks `public_signals[3]` against
/// `topic_hash_for_v2(topic)` exactly the way the local
/// `verify-membership` endpoint does, so a peer cannot smuggle a proof
/// produced for a different topic.
#[derive(Debug, Serialize, Deserialize)]
pub struct AttestationRequest {
    /// The base URL of the server attesting the identity.
    pub originating_server: String,
    /// The VRP topic used for pseudonym derivation.
    pub topic: String,
    /// The identity commitment (hex).
    pub commitment: String,
    /// The Groth16 proof (JSON object).
    pub proof: serde_json::Value,
    /// The type of participant (e.g., "HUMAN", "AI_AGENT").
    pub participant_type: String,
    /// The signature of the request (hex).
    /// Signed message: `topic\ncommitment\nparticipant_type` for v1 (legacy
    /// wire format) or
    /// `topic\ncommitment\nparticipant_type\nprotocol_version\nnullifier_hex\ntopic_hash_hex`
    /// for v2 (each newline-separated field is the canonical lowercase
    /// 64-char hex value or the literal protocol version).
    pub signature: String,

    /// Membership-circuit version this attestation is for.
    /// `None` or `Some("v1")` selects the legacy v1 verifier
    /// (commitment-derived nullifier). `Some("v2")` selects the
    /// secret-derived nullifier verifier and requires the receiving server
    /// to have v2 enabled in its config; otherwise the attestation is
    /// rejected. The server NEVER silently downgrades or upgrades a peer's
    /// declared protocol version.
    #[serde(
        rename = "protocolVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub protocol_version: Option<String>,

    /// v2-only: the proof's public signals as decimal-encoded scalars in
    /// the order `[root, commitment, nullifier, topicHash]`. Required when
    /// `protocol_version == Some("v2")`. Ignored on v1.
    #[serde(
        rename = "publicSignals",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub public_signals: Option<Vec<String>>,

    /// v2-only: the secret-derived nullifier (hex). When present and
    /// `protocol_version == Some("v2")`, the server checks that this
    /// matches `public_signals[2]` after canonicalisation. Required for v2
    /// so the federated identity row can be inserted with the same
    /// nullifier the originating server bound the proof to.
    #[serde(
        rename = "nullifierHex",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nullifier_hex: Option<String>,

    /// v2-only: the canonical topicHash (BN254 scalar in 64-char hex)
    /// the proof was bound to. Optional — when present it is cross-checked
    /// against both `public_signals[3]` and the server-recomputed
    /// `topic_hash_for_v2(topic)` so a single field mismatch surfaces as
    /// a deterministic 400 instead of a silent verifier failure.
    #[serde(
        rename = "topicHashHex",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub topic_hash_hex: Option<String>,
}

/// Wire-format version constants for the federated message envelope.
///
/// `v1` is the legacy envelope shape: signing input is the unversioned
/// newline-joined field set. `v2` adds `envelope_version` to both the
/// JSON body and the signing input, plus an explicit `created_at` that
/// the receiver freshness-checks against
/// `Config::federation::freshness_window_seconds` /
/// `future_skew_seconds`.
///
/// New deployments default to `v2` (see
/// `Config::federation::default_outbound_envelope_version`). The
/// receive path accepts both — v1 stays in for backwards compatibility
/// with peers that haven't upgraded yet. A v1 envelope on a v2-only
/// server is rejected with a typed error rather than silently
/// downgraded.
pub const FEDERATED_MESSAGE_ENVELOPE_V1: &str = "v1";
pub const FEDERATED_MESSAGE_ENVELOPE_V2: &str = "v2";

/// A message relayed from a federation peer.
///
/// Wire compatibility: `envelope_version` is `Option<String>` because v1
/// peers do not send it. Receivers treat `None` and `Some("v1")` as
/// equivalent. A peer that sends `Some("v2")` opts into the freshness
/// gate and the v2 signing input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedMessageEnvelope {
    /// Envelope wire-format version. `None` or `Some("v1")` selects
    /// the legacy signing input. `Some("v2")` selects the versioned
    /// signing input and enables freshness enforcement on the
    /// receiver.
    #[serde(
        rename = "envelopeVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub envelope_version: Option<String>,
    /// Unique public ID of the message (on the originating server).
    pub message_id: String,
    /// The public channel ID.
    pub channel_id: String,
    /// The message content.
    pub content: String,
    /// The sender's pseudonym on the originating server.
    pub sender_pseudonym: String,
    /// The base URL of the originating server.
    pub originating_server: String,
    /// VRP attestation reference (format: "topic:commitment_hex").
    pub attestation_ref: String,
    /// Ed25519 signature (hex) over the canonical signing input.
    ///
    /// v1 signing input is the newline-joined set:
    ///   `message_id\nchannel_id\ncontent\nsender_pseudonym\noriginating_server\nattestation_ref\ncreated_at`
    ///
    /// v2 prepends an explicit version line:
    ///   `envelope_version\nmessage_id\nchannel_id\ncontent\nsender_pseudonym\noriginating_server\nattestation_ref\ncreated_at`
    ///
    /// Any field shown above is *signed*; changing any of them on the
    /// wire invalidates the signature.
    pub signature: String,
    /// Creation timestamp (ISO 8601, UTC). v2 receivers reject
    /// envelopes outside the configured freshness window unless
    /// delivered through the catch-up endpoint.
    pub created_at: String,
}

/// Wire-format version constant for the federated redaction envelope
/// (ADR-0011 tombstone protocol).
pub const FEDERATED_REDACTION_ENVELOPE_V1: &str = "v1";

/// Domain-separation literal prepended to the redaction signing input.
///
/// Distinct from the message-envelope version lines (`v2\n…` for v2
/// messages, none for v1) so a signature produced for a redaction can
/// never verify as a message envelope or vice versa, regardless of
/// field contents.
pub const REDACTION_SIGNING_DOMAIN_V1: &str = "annex-redaction-v1";

/// Envelope-kind discriminator carried by redaction envelopes so the
/// outbox worker (and any future multiplexed transport) can route the
/// serialized JSON without trial deserialization. Message envelopes
/// have no `envelopeKind` field.
pub const FEDERATED_ENVELOPE_KIND_REDACTION: &str = "redaction";

/// Valid `redaction_reason` values for [`FederatedRedactionEnvelope`].
pub const REDACTION_REASONS: &[&str] = &["deleted", "moderation", "requested"];

/// A signed tombstone propagating a message deletion to federation
/// peers (ADR-0011).
///
/// Sent by the *originating* server of a message after a local soft
/// delete on a federated channel. The receiver verifies the signature
/// against the originating server's published key, checks that it
/// actually received the original message from that same peer (receipt
/// ledger), validates the redactor's authority, then blanks the local
/// copy's content while keeping `message_id` / `created_at` /
/// `sender_pseudonym` for audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedRedactionEnvelope {
    /// Always [`FEDERATED_ENVELOPE_KIND_REDACTION`]. Distinguishes the
    /// serialized form from a message envelope in shared storage
    /// (federation outbox rows).
    #[serde(rename = "envelopeKind")]
    pub envelope_kind: String,
    /// Wire-format version. Currently always
    /// [`FEDERATED_REDACTION_ENVELOPE_V1`].
    #[serde(rename = "envelopeVersion")]
    pub envelope_version: String,
    /// The public ID of the message being redacted (as originally
    /// relayed).
    pub message_id: String,
    /// The public channel ID of the original message.
    pub channel_id: String,
    /// Base URL of the originating server (must match the peer that
    /// delivered the original message).
    pub originating_server: String,
    /// Pseudonym of the redactor. For `reason != "moderation"` this
    /// must equal the original message's `sender_pseudonym`.
    pub redacted_by: String,
    /// One of [`REDACTION_REASONS`]: `deleted` (author delete),
    /// `moderation` (moderator action on the originating server), or
    /// `requested` (subject request honoured by the origin).
    pub redaction_reason: String,
    /// VRP attestation reference of the redactor
    /// (format: `"topic:commitment_hex"`).
    pub attestation_ref: String,
    /// Ed25519 signature (hex) over the canonical signing input:
    ///
    /// ```text
    /// annex-redaction-v1\nmessage_id\nchannel_id\noriginating_server\n
    /// redacted_by\nredaction_reason\nattestation_ref\ncreated_at
    /// ```
    ///
    /// (single newline-joined string; shown wrapped here). Every field
    /// above is signed; changing any of them invalidates the signature.
    pub signature: String,
    /// Creation timestamp (ISO 8601, UTC). Receivers enforce the same
    /// freshness window as v2 message envelopes.
    pub created_at: String,
}

/// An RTX bundle relayed from a federation peer.
///
/// When a bundle is published on one server and relayed to a federated peer,
/// it is wrapped in this envelope. The envelope carries:
/// - The original bundle (with transfer scope already applied by the sending server)
/// - The provenance chain tracking all relay hops
/// - The relaying server's Ed25519 signature proving authenticity
///
/// The receiving server validates the signature against the relaying server's
/// public key, checks the federation agreement's transfer scope, and delivers
/// the bundle to local subscribers with `accept_federated = true`.
#[derive(Debug, Serialize, Deserialize)]
pub struct FederatedRtxEnvelope {
    /// The RTX bundle being relayed.
    pub bundle: ReflectionSummaryBundle,
    /// The provenance chain tracking relay hops from origin to this server.
    pub provenance: BundleProvenance,
    /// The base URL of the server sending this relay (the immediate sender).
    pub relaying_server: String,
    /// Ed25519 signature of the relay payload (hex-encoded).
    ///
    /// Signed payload: `bundle_id + relaying_server + origin_server + relay_path_joined`.
    /// The relay path is joined with `|` separators for deterministic signing.
    pub signature: String,
}

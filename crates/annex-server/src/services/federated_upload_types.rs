//! Versioned, bounded attachment RPC shared by the authority and blob origin.
//! All signing inputs are domain-separated JSON tuples, never concatenated strings.

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const CHUNK_BYTES: u64 = 256 * 1024;
pub const MAX_BLOB_BYTES: u64 = 50 * 1024 * 1024 + 28;
pub const MAX_CLOCK_SKEW_SECONDS: i64 = 90;
pub const PERMIT_SECONDS: i64 = 30;
pub const PERMIT_DOMAIN: &str = "annex-attachment-read-permit-v1";
pub const PLAINTEXT_PERMIT_DOMAIN: &str = "annex-attachment-read-permit-v2";
pub const CHUNK_ENDPOINT: &str = "/api/federation/attachments/chunk";
pub const MAX_CHUNK_RESPONSE_BYTES: usize = 400 * 1024;
const JS_MAX: u64 = 9_007_199_254_740_991;

#[derive(Debug, Error)]
pub enum WireError {
    #[error("invalid attachment protocol: {0}")]
    Invalid(&'static str),
    #[error("attachment authentication failed")]
    Authentication,
    #[error("attachment protocol serialization failed")]
    Serialization,
}

/// Ordinary attachment metadata is public to its channel. It must never be
/// used for an E2E attachment: the latter keeps its filename and MIME inside
/// the encrypted message descriptor and retains the byte-exact v1 protocol.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlainAttachmentMetadata {
    pub filename: String,
    pub content_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttachmentDescriptor {
    pub attachment_id: String,
    pub origin_identity: String,
    pub originating_server: String,
    pub size_bytes: u64,
    pub ciphertext_sha256: String,
    pub category: String,
    pub epoch: u64,
    pub key_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plaintext: Option<PlainAttachmentMetadata>,
}

/// The hash references the enclosing accepted event; it is deliberately not
/// part of AttachmentDescriptor, which the event itself hashes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AcceptedAttachment {
    pub authority_identity: String,
    pub channel_id: String,
    pub message_id: String,
    pub accepted_event_seq: u64,
    pub accepted_event_hash: String,
    pub descriptor: AttachmentDescriptor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadPurpose {
    Chunk,
    Finalize,
    BrowserDownload,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReadIntent {
    pub version: u8,
    pub purpose: ReadPurpose,
    pub requesting_identity: String,
    pub requesting_server: String,
    pub member_pseudonym: String,
    pub device_id: String,
    pub accepted: AcceptedAttachment,
    pub offset: Option<u64>,
    pub length: Option<u64>,
    pub nonce: String,
    /// Unix seconds; integer, with no locale or timestamp-format ambiguity.
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignedReadPermit {
    pub domain: String,
    pub version: u8,
    pub authority_identity: String,
    pub intent_sha256: String,
    pub intent: ReadIntent,
    pub current_revision: u64,
    pub head_seq: u64,
    pub head_hash: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChunkRequest {
    pub version: u8,
    // Kept at top level for transport peer-to-origin binding.
    pub requesting_server: String,
    pub intent: ReadIntent,
    /// Preserved when a distinct blob origin asks the authority on behalf of
    /// this requester; the origin cannot substitute its own attestation.
    pub intent_signature: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChunkOutcome {
    Ready {
        bytes_base64: String,
        chunk_sha256: String,
        count: u64,
    },
    Pending {
        retry_after_seconds: u32,
    },
    Removed {
        reason: String,
        tombstone_seq: u64,
        tombstone_hash: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChunkResponse {
    pub version: u8,
    pub originating_identity: String,
    pub originating_server: String,
    pub request_sha256: String,
    pub intent: ReadIntent,
    /// Ready replies carry the fresh authority permit. A signed tombstone
    /// response is never usable as permission to return bytes.
    pub permit: Option<SignedReadPermit>,
    pub outcome: ChunkOutcome,
    pub created_at: i64,
    pub signature: String,
}

pub fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value)
}
pub fn hex_32(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn stable_identity(public_key: &[u8; 32]) -> String {
    format!("annex-server:{}", hex::encode(Sha256::digest(public_key)))
}
pub fn valid_identity(value: &str) -> bool {
    value.strip_prefix("annex-server:").is_some_and(hex_32)
}
pub fn bounded_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
pub fn canonical_url(value: &str) -> bool {
    let Ok(parsed) = url::Url::parse(value) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.path() == "/"
        && parsed.as_str().trim_end_matches('/') == value
}
fn ensure(value: bool, field: &'static str) -> Result<(), WireError> {
    if value {
        Ok(())
    } else {
        Err(WireError::Invalid(field))
    }
}
pub fn freshness(created_at: i64, now: i64) -> Result<(), WireError> {
    ensure(
        created_at.abs_diff(now) <= MAX_CLOCK_SKEW_SECONDS as u64,
        "timestamp outside freshness window",
    )
}
impl AttachmentDescriptor {
    pub fn is_plaintext(&self) -> bool {
        self.plaintext.is_some()
    }
    pub fn protocol_version(&self) -> u8 {
        if self.is_plaintext() {
            2
        } else {
            1
        }
    }
    pub fn filename(&self) -> &str {
        self.plaintext
            .as_ref()
            .map_or("encrypted.bin", |value| value.filename.as_str())
    }
    pub fn content_type(&self) -> &str {
        self.plaintext
            .as_ref()
            .map_or("application/octet-stream", |value| {
                value.content_type.as_str()
            })
    }
    pub fn public_path(&self) -> String {
        let directory = match self.category.as_str() {
            "image" => "images",
            "video" => "videos",
            _ => "files",
        };
        let extension = if self.is_plaintext() {
            crate::api_upload::ext_from_content_type(self.content_type())
        } else {
            "bin"
        };
        format!("chat/{directory}/{}.{extension}", self.attachment_id)
    }
    pub fn validate(&self) -> Result<(), WireError> {
        ensure(canonical_uuid(&self.attachment_id), "attachment_id")?;
        ensure(valid_identity(&self.origin_identity), "origin_identity")?;
        ensure(
            canonical_url(&self.originating_server),
            "originating_server",
        )?;
        ensure(hex_32(&self.ciphertext_sha256), "content digest")?;
        ensure(
            matches!(self.category.as_str(), "image" | "video" | "file"),
            "category",
        )?;
        if let Some(plain) = &self.plaintext {
            ensure(
                (1..=MAX_BLOB_BYTES - 28).contains(&self.size_bytes),
                "ordinary size_bytes",
            )?;
            ensure(
                self.epoch == 0 && self.key_id.is_empty(),
                "ordinary attachment cannot claim an encryption epoch",
            )?;
            ensure(
                !plain.filename.is_empty()
                    && plain.filename.len() <= 512
                    && !plain.filename.chars().any(char::is_control),
                "ordinary filename",
            )?;
            let category = match plain.content_type.as_str() {
                "image/jpeg" | "image/png" | "image/gif" | "image/webp" => "image",
                "video/mp4" | "video/webm" | "video/quicktime" => "video",
                "application/pdf"
                | "application/zip"
                | "text/plain"
                | "application/octet-stream" => "file",
                _ => return Err(WireError::Invalid("ordinary content type")),
            };
            ensure(self.category == category, "ordinary MIME/category mismatch")
        } else {
            ensure(
                (28..=MAX_BLOB_BYTES).contains(&self.size_bytes),
                "size_bytes",
            )?;
            ensure(hex_32(&self.key_id), "key id")?;
            ensure(self.epoch > 0 && self.epoch <= JS_MAX, "epoch")
        }
    }
}
impl AcceptedAttachment {
    pub fn validate(&self) -> Result<(), WireError> {
        self.descriptor.validate()?;
        ensure(
            valid_identity(&self.authority_identity),
            "authority_identity",
        )?;
        ensure(
            bounded_identifier(&self.channel_id) && bounded_identifier(&self.message_id),
            "channel or message id",
        )?;
        ensure(
            self.accepted_event_seq > 0 && self.accepted_event_seq <= JS_MAX,
            "accepted_event_seq",
        )?;
        ensure(hex_32(&self.accepted_event_hash), "accepted_event_hash")
    }
}
impl ReadIntent {
    pub fn validate(&self, now: i64) -> Result<(), WireError> {
        self.accepted.validate()?;
        ensure(
            self.version == self.accepted.descriptor.protocol_version(),
            "intent version",
        )?;
        ensure(
            valid_identity(&self.requesting_identity) && canonical_url(&self.requesting_server),
            "requesting peer",
        )?;
        ensure(
            !self.member_pseudonym.is_empty()
                && self.member_pseudonym.len() <= 256
                && !self.member_pseudonym.chars().any(char::is_control),
            "member_pseudonym",
        )?;
        ensure(
            hex_32(&self.device_id) && canonical_uuid(&self.nonce),
            "device_id or nonce",
        )?;
        freshness(self.created_at, now)?;
        match self.purpose {
            ReadPurpose::Chunk => {
                let (Some(offset), Some(length)) = (self.offset, self.length) else {
                    return Err(WireError::Invalid("missing chunk range"));
                };
                ensure(
                    offset % CHUNK_BYTES == 0 && offset < self.accepted.descriptor.size_bytes,
                    "chunk offset",
                )?;
                ensure(
                    length == CHUNK_BYTES.min(self.accepted.descriptor.size_bytes - offset),
                    "chunk length",
                )
            }
            _ => ensure(
                self.offset.is_none() && self.length.is_none(),
                "range forbidden for this purpose",
            ),
        }
    }
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, WireError> {
    serde_json::to_vec(value).map_err(|_| WireError::Serialization)
}
pub fn intent_signing_input(intent: &ReadIntent) -> Result<Vec<u8>, WireError> {
    encode(&(
        if intent.version == 2 {
            "annex-attachment-read-intent-v2"
        } else {
            "annex-attachment-read-intent-v1"
        },
        intent,
    ))
}
pub fn permit_domain(version: u8) -> &'static str {
    if version == 2 {
        PLAINTEXT_PERMIT_DOMAIN
    } else {
        PERMIT_DOMAIN
    }
}
pub fn intent_digest(intent: &ReadIntent) -> Result<String, WireError> {
    Ok(hex::encode(Sha256::digest(intent_signing_input(intent)?)))
}
pub fn permit_signing_input(p: &SignedReadPermit) -> Result<Vec<u8>, WireError> {
    encode(&(
        &p.domain,
        p.version,
        &p.authority_identity,
        &p.intent_sha256,
        &p.intent,
        p.current_revision,
        p.head_seq,
        &p.head_hash,
        p.issued_at,
        p.expires_at,
    ))
}
pub fn request_signing_input(r: &ChunkRequest) -> Result<Vec<u8>, WireError> {
    encode(&(
        if r.version == 2 {
            "annex-attachment-chunk-request-v2"
        } else {
            "annex-attachment-chunk-request-v1"
        },
        r.version,
        &r.requesting_server,
        &r.intent,
        &r.intent_signature,
    ))
}
pub fn request_digest(r: &ChunkRequest) -> Result<String, WireError> {
    Ok(hex::encode(Sha256::digest(request_signing_input(r)?)))
}
pub fn response_signing_input(r: &ChunkResponse) -> Result<Vec<u8>, WireError> {
    encode(&(
        if r.version == 2 {
            "annex-attachment-chunk-response-v2"
        } else {
            "annex-attachment-chunk-response-v1"
        },
        r.version,
        &r.originating_identity,
        &r.originating_server,
        &r.request_sha256,
        &r.intent,
        &r.permit,
        &r.outcome,
        r.created_at,
    ))
}
pub fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    hex::encode(key.sign(bytes).to_bytes())
}
pub fn verify(key: &str, signature: &str, bytes: &[u8]) -> Result<(), WireError> {
    if !hex_32(key)
        || signature.len() != 128
        || !signature
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(WireError::Authentication);
    }
    let key: [u8; 32] = hex::decode(key)
        .map_err(|_| WireError::Authentication)?
        .try_into()
        .map_err(|_| WireError::Authentication)?;
    let sig =
        Signature::from_slice(&hex::decode(signature).map_err(|_| WireError::Authentication)?)
            .map_err(|_| WireError::Authentication)?;
    VerifyingKey::from_bytes(&key)
        .map_err(|_| WireError::Authentication)?
        .verify_strict(bytes, &sig)
        .map_err(|_| WireError::Authentication)
}
pub fn verify_permit(
    p: &SignedReadPermit,
    intent: &ReadIntent,
    authority_key: &str,
    now: i64,
) -> Result<(), WireError> {
    intent.validate(now)?;
    let key: [u8; 32] = hex::decode(authority_key)
        .map_err(|_| WireError::Authentication)?
        .try_into()
        .map_err(|_| WireError::Authentication)?;
    ensure(
        p.version == intent.version && p.domain == permit_domain(intent.version),
        "permit version or domain",
    )?;
    ensure(
        &p.intent == intent && p.intent_sha256 == intent_digest(intent)?,
        "permit intent binding",
    )?;
    ensure(
        p.authority_identity == intent.accepted.authority_identity
            && p.authority_identity == stable_identity(&key),
        "permit authority binding",
    )?;
    ensure(
        p.head_seq >= intent.accepted.accepted_event_seq
            && p.head_seq <= JS_MAX
            && p.current_revision <= JS_MAX
            && hex_32(&p.head_hash),
        "permit head",
    )?;
    freshness(p.issued_at, now)?;
    ensure(
        p.expires_at > now
            && p.expires_at > p.issued_at
            && p.expires_at
                .checked_sub(p.issued_at)
                .is_some_and(|lifetime| lifetime <= PERMIT_SECONDS),
        "expired or overlong permit",
    )?;
    verify(authority_key, &p.signature, &permit_signing_input(p)?)
}
pub fn verify_request(r: &ChunkRequest, peer_key: &str, now: i64) -> Result<(), WireError> {
    r.intent.validate(now)?;
    let key: [u8; 32] = hex::decode(peer_key)
        .map_err(|_| WireError::Authentication)?
        .try_into()
        .map_err(|_| WireError::Authentication)?;
    ensure(
        r.version == r.intent.version
            && r.intent.purpose == ReadPurpose::Chunk
            && r.requesting_server == r.intent.requesting_server
            && r.intent.requesting_identity == stable_identity(&key),
        "request peer or purpose",
    )?;
    verify(
        peer_key,
        &r.intent_signature,
        &intent_signing_input(&r.intent)?,
    )?;
    verify(peer_key, &r.signature, &request_signing_input(r)?)
}
/// Authenticating the complete response precedes interpreting its outcome.
/// Decoding is bounded before allocation; no pending/removed response yields bytes.
pub fn verify_response(
    r: &ChunkResponse,
    request: &ChunkRequest,
    origin_key: &str,
    authority_key: &str,
    now: i64,
) -> Result<Option<Vec<u8>>, WireError> {
    request.intent.validate(now)?;
    freshness(r.created_at, now)?;
    let key: [u8; 32] = hex::decode(origin_key)
        .map_err(|_| WireError::Authentication)?
        .try_into()
        .map_err(|_| WireError::Authentication)?;
    ensure(
        r.version == request.version
            && r.intent == request.intent
            && r.request_sha256 == request_digest(request)?,
        "response request binding",
    )?;
    ensure(
        r.originating_identity == request.intent.accepted.descriptor.origin_identity
            && r.originating_identity == stable_identity(&key)
            && r.originating_server == request.intent.accepted.descriptor.originating_server,
        "response origin binding",
    )?;
    verify(origin_key, &r.signature, &response_signing_input(r)?)?;
    match &r.outcome {
        ChunkOutcome::Ready {
            bytes_base64,
            chunk_sha256,
            count,
        } => {
            ensure(
                *count == request.intent.length.unwrap_or(0) && *count <= CHUNK_BYTES,
                "returned chunk length",
            )?;
            ensure(
                bytes_base64.len() <= (CHUNK_BYTES as usize).div_ceil(3) * 4,
                "encoded chunk size",
            )?;
            let bytes = STANDARD
                .decode(bytes_base64)
                .map_err(|_| WireError::Invalid("base64"))?;
            ensure(
                bytes.len() as u64 == *count && STANDARD.encode(&bytes) == *bytes_base64,
                "noncanonical chunk encoding",
            )?;
            ensure(
                hex_32(chunk_sha256) && *chunk_sha256 == hex::encode(Sha256::digest(&bytes)),
                "chunk digest",
            )?;
            let permit = r
                .permit
                .as_ref()
                .ok_or(WireError::Invalid("ready chunk missing permit"))?;
            verify_permit(permit, &request.intent, authority_key, now)?;
            Ok(Some(bytes))
        }
        ChunkOutcome::Pending {
            retry_after_seconds,
        } => {
            ensure(
                r.permit.is_none() && (1..=300).contains(retry_after_seconds),
                "pending response",
            )?;
            Ok(None)
        }
        ChunkOutcome::Removed {
            reason,
            tombstone_seq,
            tombstone_hash,
        } => {
            ensure(
                r.permit.is_none()
                    && matches!(
                        reason.as_str(),
                        "deleted" | "retention" | "detached" | "channel_deleted"
                    )
                    && *tombstone_seq >= request.intent.accepted.accepted_event_seq
                    && *tombstone_seq <= JS_MAX
                    && hex_32(tombstone_hash),
                "removal certificate",
            )?;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: i64 = 1_800_000_000;
    fn fixture() -> (
        SigningKey,
        SigningKey,
        SigningKey,
        ChunkRequest,
        ChunkResponse,
    ) {
        let requester = SigningKey::generate(&mut rand::rngs::OsRng);
        let origin = SigningKey::generate(&mut rand::rngs::OsRng);
        let authority = SigningKey::generate(&mut rand::rngs::OsRng);
        let bytes = vec![0x93; CHUNK_BYTES as usize];
        let intent = ReadIntent {
            version: 1,
            purpose: ReadPurpose::Chunk,
            requesting_identity: stable_identity(&requester.verifying_key().to_bytes()),
            requesting_server: "http://localhost:3002".into(),
            member_pseudonym: "recipient".into(),
            device_id: "a1".repeat(32),
            accepted: AcceptedAttachment {
                authority_identity: stable_identity(&authority.verifying_key().to_bytes()),
                channel_id: "general".into(),
                message_id: "msg-legacy-compatible".into(),
                accepted_event_seq: 7,
                accepted_event_hash: "b2".repeat(32),
                descriptor: AttachmentDescriptor {
                    attachment_id: uuid::Uuid::new_v4().to_string(),
                    origin_identity: stable_identity(&origin.verifying_key().to_bytes()),
                    originating_server: "http://localhost:3000".into(),
                    size_bytes: CHUNK_BYTES + 28,
                    ciphertext_sha256: "c3".repeat(32),
                    category: "file".into(),
                    epoch: 1,
                    key_id: "d4".repeat(32),
                    plaintext: None,
                },
            },
            offset: Some(0),
            length: Some(CHUNK_BYTES),
            nonce: uuid::Uuid::new_v4().to_string(),
            created_at: NOW,
        };
        let mut permit = SignedReadPermit {
            domain: PERMIT_DOMAIN.into(),
            version: 1,
            authority_identity: intent.accepted.authority_identity.clone(),
            intent_sha256: intent_digest(&intent).unwrap(),
            intent: intent.clone(),
            current_revision: 2,
            head_seq: 8,
            head_hash: "e5".repeat(32),
            issued_at: NOW,
            expires_at: NOW + PERMIT_SECONDS,
            signature: String::new(),
        };
        permit.signature = sign(&authority, &permit_signing_input(&permit).unwrap());
        let mut request = ChunkRequest {
            version: 1,
            requesting_server: intent.requesting_server.clone(),
            intent_signature: sign(&requester, &intent_signing_input(&intent).unwrap()),
            intent: intent.clone(),
            signature: String::new(),
        };
        request.signature = sign(&requester, &request_signing_input(&request).unwrap());
        let mut response = ChunkResponse {
            version: 1,
            originating_identity: intent.accepted.descriptor.origin_identity.clone(),
            originating_server: intent.accepted.descriptor.originating_server.clone(),
            request_sha256: request_digest(&request).unwrap(),
            intent,
            permit: Some(permit),
            outcome: ChunkOutcome::Ready {
                count: CHUNK_BYTES,
                bytes_base64: STANDARD.encode(&bytes),
                chunk_sha256: hex::encode(Sha256::digest(&bytes)),
            },
            created_at: NOW,
            signature: String::new(),
        };
        response.signature = sign(&origin, &response_signing_input(&response).unwrap());
        (requester, origin, authority, request, response)
    }
    fn pubhex(k: &SigningKey) -> String {
        hex::encode(k.verifying_key().to_bytes())
    }
    fn ordinary_fixture() -> (
        SigningKey,
        SigningKey,
        SigningKey,
        ChunkRequest,
        ChunkResponse,
    ) {
        let (requester, origin, authority, mut request, mut response) = fixture();
        request.version = 2;
        request.intent.version = 2;
        let descriptor = &mut request.intent.accepted.descriptor;
        descriptor.epoch = 0;
        descriptor.key_id.clear();
        descriptor.size_bytes = 1;
        descriptor.ciphertext_sha256 = hex::encode(Sha256::digest(b"X"));
        descriptor.plaintext = Some(PlainAttachmentMetadata {
            filename: "retained.txt".into(),
            content_type: "text/plain".into(),
        });
        request.intent.length = Some(1);
        request.intent_signature =
            sign(&requester, &intent_signing_input(&request.intent).unwrap());
        request.signature = sign(&requester, &request_signing_input(&request).unwrap());
        response.version = 2;
        response.intent = request.intent.clone();
        response.request_sha256 = request_digest(&request).unwrap();
        let permit = response.permit.as_mut().unwrap();
        permit.domain = permit_domain(2).into();
        permit.version = 2;
        permit.intent = request.intent.clone();
        permit.intent_sha256 = intent_digest(&request.intent).unwrap();
        permit.signature = sign(&authority, &permit_signing_input(permit).unwrap());
        response.outcome = ChunkOutcome::Ready {
            bytes_base64: STANDARD.encode(b"X"),
            chunk_sha256: descriptor_digest(&request),
            count: 1,
        };
        response.signature = sign(&origin, &response_signing_input(&response).unwrap());
        (requester, origin, authority, request, response)
    }
    fn descriptor_digest(request: &ChunkRequest) -> String {
        request.intent.accepted.descriptor.ciphertext_sha256.clone()
    }
    #[test]
    fn ordinary_one_byte_transfer_uses_distinct_domains_and_all_actual_signatures() {
        let (requester, origin, authority, request, response) = ordinary_fixture();
        verify_request(&request, &pubhex(&requester), NOW).unwrap();
        assert_eq!(
            verify_response(
                &response,
                &request,
                &pubhex(&origin),
                &pubhex(&authority),
                NOW
            )
            .unwrap()
            .unwrap(),
            b"X"
        );
        assert_eq!(
            request.intent.accepted.descriptor.public_path(),
            format!(
                "chat/files/{}.txt",
                request.intent.accepted.descriptor.attachment_id
            )
        );
        assert!(
            String::from_utf8(intent_signing_input(&request.intent).unwrap())
                .unwrap()
                .starts_with("[\"annex-attachment-read-intent-v2\"")
        );
        let mut legacy = request.clone();
        legacy.version = 1;
        legacy.intent.version = 1;
        legacy.intent_signature = sign(&requester, &intent_signing_input(&legacy.intent).unwrap());
        legacy.signature = sign(&requester, &request_signing_input(&legacy).unwrap());
        assert!(verify_request(&legacy, &pubhex(&requester), NOW).is_err());
        let mut wrong = response.clone();
        wrong.permit.as_mut().unwrap().domain = PERMIT_DOMAIN.into();
        let permit = wrong.permit.as_mut().unwrap();
        permit.signature = sign(&authority, &permit_signing_input(permit).unwrap());
        wrong.signature = sign(&origin, &response_signing_input(&wrong).unwrap());
        assert!(
            verify_response(&wrong, &request, &pubhex(&origin), &pubhex(&authority), NOW).is_err()
        );
    }
    #[test]
    fn ordinary_metadata_is_signed_and_cannot_claim_encrypted_epochs_or_active_content() {
        let (requester, _, _, request, _) = ordinary_fixture();
        for field in [
            "filename",
            "content_type",
            "epoch",
            "key_id",
            "empty",
            "category",
        ] {
            let mut changed = request.clone();
            let d = &mut changed.intent.accepted.descriptor;
            match field {
                "filename" => d.plaintext.as_mut().unwrap().filename = "other.txt".into(),
                "content_type" => d.plaintext.as_mut().unwrap().content_type = "text/html".into(),
                "epoch" => d.epoch = 1,
                "key_id" => d.key_id = "aa".repeat(32),
                "empty" => d.size_bytes = 0,
                _ => d.category = "image".into(),
            }
            assert!(verify_request(&changed, &pubhex(&requester), NOW).is_err());
        }
        let mut d = request.intent.accepted.descriptor.clone();
        d.plaintext.as_mut().unwrap().filename = "bad\r\nname".into();
        assert!(d.validate().is_err());
    }
    #[test]
    fn encrypted_legacy_descriptor_bytes_and_domain_remain_unchanged() {
        let (_, _, _, request, _) = fixture();
        let descriptor = &request.intent.accepted.descriptor;
        let old = serde_json::json!({"attachment_id":descriptor.attachment_id,"origin_identity":descriptor.origin_identity,
            "originating_server":descriptor.originating_server,"size_bytes":descriptor.size_bytes,"ciphertext_sha256":descriptor.ciphertext_sha256,
            "category":descriptor.category,"epoch":descriptor.epoch,"key_id":descriptor.key_id});
        assert_eq!(serde_json::to_value(descriptor).unwrap(), old);
        assert!(!serde_json::to_string(descriptor)
            .unwrap()
            .contains("plaintext"));
        assert_eq!(
            intent_signing_input(&request.intent).unwrap(),
            encode(&("annex-attachment-read-intent-v1", &request.intent)).unwrap()
        );
        let restored: AttachmentDescriptor = serde_json::from_value(old).unwrap();
        assert_eq!(&restored, descriptor);
    }
    #[test]
    fn actual_distinct_peer_authority_signatures_bind_and_decode_full_bounded_chunk() {
        let (requester, origin, authority, q, r) = fixture();
        verify_request(&q, &pubhex(&requester), NOW).unwrap();
        let bytes = verify_response(&r, &q, &pubhex(&origin), &pubhex(&authority), NOW)
            .unwrap()
            .unwrap();
        assert_eq!(bytes, vec![0x93; CHUNK_BYTES as usize]);
        assert!(serde_json::to_vec(&r).unwrap().len() < MAX_CHUNK_RESPONSE_BYTES);
    }
    #[test]
    fn valid_origin_signature_does_not_replace_authority_or_requester_permission() {
        let (_, origin, authority, q, mut r) = fixture();
        r.permit.as_mut().unwrap().intent.device_id = "f6".repeat(32);
        r.signature = sign(&origin, &response_signing_input(&r).unwrap());
        assert!(verify_response(&r, &q, &pubhex(&origin), &pubhex(&authority), NOW).is_err());
    }
    #[test]
    fn origin_cannot_forge_forwarded_requester_intent() {
        let (requester, origin, _, mut q, _) = fixture();
        q.intent_signature = sign(&origin, &intent_signing_input(&q.intent).unwrap());
        q.signature = sign(&requester, &request_signing_input(&q).unwrap());
        assert!(verify_request(&q, &pubhex(&requester), NOW).is_err());
    }
    #[test]
    fn every_security_relevant_read_field_is_signature_bound() {
        let (requester, _, _, q, _) = fixture();
        let mut mutations = Vec::new();
        let mut v = q.clone();
        v.intent.member_pseudonym = "another".into();
        mutations.push(v);
        let mut v = q.clone();
        v.intent.device_id = "f6".repeat(32);
        mutations.push(v);
        let mut v = q.clone();
        v.intent.accepted.descriptor.category = "image".into();
        mutations.push(v);
        let mut v = q.clone();
        v.intent.accepted.descriptor.ciphertext_sha256 = "f6".repeat(32);
        mutations.push(v);
        let mut v = q.clone();
        v.intent.accepted.descriptor.key_id = "f6".repeat(32);
        mutations.push(v);
        let mut v = q.clone();
        v.intent.accepted.descriptor.epoch = 2;
        mutations.push(v);
        let mut v = q.clone();
        v.intent.accepted.accepted_event_seq += 1;
        mutations.push(v);
        let mut v = q.clone();
        v.intent.accepted.channel_id = "another".into();
        mutations.push(v);
        let mut v = q.clone();
        v.intent.accepted.message_id = "another".into();
        mutations.push(v);
        let mut v = q.clone();
        v.intent.nonce = uuid::Uuid::new_v4().to_string();
        mutations.push(v);
        let mut v = q.clone();
        v.intent.created_at += 1;
        mutations.push(v);
        for mutation in mutations {
            assert!(verify_request(&mutation, &pubhex(&requester), NOW).is_err());
        }
    }
    #[test]
    fn strict_ranges_categories_hashes_urls_and_unknown_fields_refuse() {
        let (_, _, _, q, _) = fixture();
        for (offset, length) in [
            (1, CHUNK_BYTES),
            (0, CHUNK_BYTES + 1),
            (0, 0),
            (u64::MAX, 1),
            (CHUNK_BYTES, CHUNK_BYTES),
        ] {
            let mut i = q.intent.clone();
            i.offset = Some(offset);
            i.length = Some(length);
            assert!(i.validate(NOW).is_err());
        }
        let mut last = q.intent.clone();
        last.offset = Some(CHUNK_BYTES);
        last.length = Some(28);
        last.validate(NOW).unwrap();
        let mut value = serde_json::to_value(&q).unwrap();
        value["intent"]["extra"] = true.into();
        assert!(serde_json::from_value::<ChunkRequest>(value).is_err());
        for url in [
            "file:///tmp/secret",
            "http://user:pass@localhost",
            "http://localhost/path",
            "http://localhost?query=yes",
            "http://localhost/#fragment",
        ] {
            assert!(!canonical_url(url));
        }
        let mut i = q.intent.clone();
        i.accepted.descriptor.category = "executable".into();
        assert!(i.validate(NOW).is_err());
        i = q.intent.clone();
        i.accepted.descriptor.attachment_id = "ABCDEFAB-1234-4321-9876-ABCDEFABCDEF".into();
        assert!(i.validate(NOW).is_err());
        i = q.intent.clone();
        i.accepted.descriptor.epoch = JS_MAX + 1;
        assert!(i.validate(NOW).is_err());
    }
    #[test]
    fn expired_permit_and_resigned_oversized_bytes_do_not_decode() {
        let (_, origin, authority, q, mut r) = fixture();
        assert!(verify_response(
            &r,
            &q,
            &pubhex(&origin),
            &pubhex(&authority),
            NOW + PERMIT_SECONDS + 1
        )
        .is_err());
        if let ChunkOutcome::Ready {
            ref mut bytes_base64,
            ..
        } = r.outcome
        {
            bytes_base64.push_str("AAAA");
        }
        r.signature = sign(&origin, &response_signing_input(&r).unwrap());
        assert!(verify_response(&r, &q, &pubhex(&origin), &pubhex(&authority), NOW).is_err());
    }
    #[test]
    fn permit_clock_skew_preserves_signed_lifetime_without_extending_expiry() {
        let (_, _, authority, q, r) = fixture();
        let key = pubhex(&authority);
        let mut permit = r.permit.unwrap();
        permit.issued_at = NOW + 2;
        permit.expires_at = permit.issued_at + PERMIT_SECONDS;
        permit.signature = sign(&authority, &permit_signing_input(&permit).unwrap());
        verify_permit(&permit, &q.intent, &key, NOW).unwrap();
        assert!(verify_permit(&permit, &q.intent, &key, permit.expires_at).is_err());
        permit.expires_at += 1;
        permit.signature = sign(&authority, &permit_signing_input(&permit).unwrap());
        assert!(verify_permit(&permit, &q.intent, &key, NOW).is_err());
        permit.issued_at = NOW + MAX_CLOCK_SKEW_SECONDS + 1;
        permit.expires_at = permit.issued_at + PERMIT_SECONDS;
        permit.signature = sign(&authority, &permit_signing_input(&permit).unwrap());
        assert!(verify_permit(&permit, &q.intent, &key, NOW).is_err());
        permit.issued_at = NOW;
        permit.expires_at = NOW + PERMIT_SECONDS;
        assert!(verify_permit(&permit, &q.intent, &key, NOW).is_err());
    }
    #[test]
    fn byte_tamper_and_cross_request_replay_fail_even_when_other_fields_match() {
        let (_, origin, authority, mut q, mut r) = fixture();
        if let ChunkOutcome::Ready {
            ref mut bytes_base64,
            ..
        } = r.outcome
        {
            bytes_base64.replace_range(0..1, "A");
        }
        assert!(verify_response(&r, &q, &pubhex(&origin), &pubhex(&authority), NOW).is_err());
        r.signature = sign(&origin, &response_signing_input(&r).unwrap());
        assert!(verify_response(&r, &q, &pubhex(&origin), &pubhex(&authority), NOW).is_err());
        q.intent.nonce = uuid::Uuid::new_v4().to_string();
        assert!(verify_response(&r, &q, &pubhex(&origin), &pubhex(&authority), NOW).is_err());
    }
}

//! Authenticated application receipts. A successful SCTP write is not a receipt.
use super::{verifying_key_from_hex, TransportError};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) const MAX_BODY: usize = super::MAX_APPLICATION_RESPONSE_BYTES;
pub(super) const MAX_REQUEST: usize = super::MAX_APPLICATION_REQUEST_BYTES;
// An application JSON body is encoded once more as Packet.body: quotes and
// backslashes can double its wire size. Header/signature space is bounded too.
pub(super) const MAX_PACKET: usize = 2 * MAX_BODY + 64 * 1024;
pub(super) const CHUNK_BYTES: usize = 8 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Packet {
    pub version: u8,
    pub response: bool,
    pub id: String,
    pub from: String,
    pub to: String,
    pub endpoint: String,
    pub request_hash: String,
    pub success: bool,
    pub body: String,
    pub created_at_ms: i64,
    pub signature: String,
}

impl Packet {
    fn signing_input(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            "annex-federation-rpc-v1",
            self.version,
            self.response,
            &self.id,
            &self.from,
            &self.to,
            &self.endpoint,
            &self.request_hash,
            self.success,
            &self.body,
            self.created_at_ms,
        ))
        .expect("fixed RPC tuple is serializable")
    }

    pub fn digest(&self) -> String {
        BASE64.encode(Sha256::digest(self.signing_input()))
    }

    pub fn sign(&mut self, key: &SigningKey) {
        self.signature = BASE64.encode(key.sign(&self.signing_input()).to_bytes());
    }

    pub fn verify(&self, peer: &str, local: &str) -> Result<(), TransportError> {
        if self.version != 1
            || self.from != peer
            || self.to != local
            || self.id.len() > 64
            || uuid::Uuid::parse_str(&self.id).is_err()
            || self.endpoint.len() > 128
            || self.body.len() > if self.response { MAX_BODY } else { MAX_REQUEST }
            || chrono::Utc::now()
                .timestamp_millis()
                .abs_diff(self.created_at_ms)
                > 90_000
        {
            return Err(TransportError::Protocol(
                "invalid RPC bounds, identity, or timestamp".into(),
            ));
        }
        let signature = BASE64
            .decode(&self.signature)
            .ok()
            .and_then(|b| Signature::from_slice(&b).ok())
            .ok_or_else(|| TransportError::Protocol("invalid RPC signature encoding".into()))?;
        verifying_key_from_hex(peer)?
            .verify_strict(&self.signing_input(), &signature)
            .map_err(|_| TransportError::Protocol("invalid RPC signature".into()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Chunk {
    pub transfer: String,
    pub index: usize,
    pub count: usize,
    pub data: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_signature_binds_peer_nonce_request_hash_result_and_payload() {
        let key = SigningKey::from_bytes(&[27; 32]);
        let peer = key
            .verifying_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut packet = Packet {
            version: 1,
            response: true,
            id: uuid::Uuid::new_v4().to_string(),
            from: peer.clone(),
            to: "local".into(),
            endpoint: "/api/federation/messages".into(),
            request_hash: "committed request hash".into(),
            success: true,
            body: "{}".into(),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            signature: String::new(),
        };
        packet.sign(&key);
        packet.verify(&peer, "local").unwrap();
        for field in ["id", "request_hash", "body", "success"] {
            let mut value = serde_json::to_value(&packet).unwrap();
            value[field] = match field {
                "id" => serde_json::json!(uuid::Uuid::new_v4().to_string()),
                "success" => serde_json::json!(false),
                _ => serde_json::json!("tampered"),
            };
            assert!(serde_json::from_value::<Packet>(value)
                .unwrap()
                .verify(&peer, "local")
                .is_err());
        }
        assert!(packet.verify(&peer, "another receiver").is_err());
    }
}

//! Endpoint-only authenticated encryption for the content-blind public router.
//! Wire specification and cross-language vectors: docs/protocol/public-host.md.

use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Nonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2_11::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

const DOMAIN: &[u8] = b"annex-content-blind-v2";
const CHUNK: usize = 48 * 1024;
const MAX_FRAME: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Packet {
    pub sequence: u64,
    pub data: String,
}

pub struct Cipher {
    outbound: [u8; 32],
    inbound: [u8; 32],
    transcript: [u8; 32],
    sent: u64,
    received: u64,
    fragments: Vec<u8>,
    failed: bool,
}

impl Drop for Cipher {
    fn drop(&mut self) {
        self.outbound.zeroize();
        self.inbound.zeroize();
        self.fragments.zeroize();
    }
}

fn decode(value: &str, length: usize) -> Result<Vec<u8>, String> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| "Invalid tunnel encoding")?;
    if bytes.len() != length || STANDARD.encode(&bytes) != value {
        return Err("Invalid tunnel handshake".into());
    }
    Ok(bytes)
}

fn transcript(identity: &[u8], hello: &[u8], ephemeral: &[u8]) -> Vec<u8> {
    [DOMAIN, identity, hello, ephemeral].concat()
}

impl Cipher {
    fn new(mut shared: [u8; 32], transcript: [u8; 32], host: bool) -> Result<Self, String> {
        if shared == [0; 32] {
            return Err("Invalid tunnel peer key".into());
        }
        let hk = Hkdf::<Sha256>::new(Some(&transcript), &shared);
        let mut c2h = [0u8; 32];
        let mut h2c = [0u8; 32];
        hk.expand(&[DOMAIN, b"/client-to-host"].concat(), &mut c2h)
            .map_err(|_| "Invalid tunnel key")?;
        hk.expand(&[DOMAIN, b"/host-to-client"].concat(), &mut h2c)
            .map_err(|_| "Invalid tunnel key")?;
        shared.zeroize();
        let (outbound, inbound) = if host { (h2c, c2h) } else { (c2h, h2c) };
        c2h.zeroize();
        h2c.zeroize();
        Ok(Self {
            outbound,
            inbound,
            transcript,
            sent: 0,
            received: 0,
            fragments: Vec::new(),
            failed: false,
        })
    }

    pub fn accept(key: &SigningKey, hello: &str) -> Result<(Self, String), String> {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let result = Self::accept_with_seed(key, hello, seed);
        seed.zeroize();
        result
    }

    fn accept_with_seed(
        key: &SigningKey,
        hello: &str,
        seed: [u8; 32],
    ) -> Result<(Self, String), String> {
        let hello = decode(hello, 64)?;
        let ephemeral = StaticSecret::from(seed);
        let public = PublicKey::from(&ephemeral);
        let client =
            PublicKey::from(<[u8; 32]>::try_from(&hello[..32]).map_err(|_| "Invalid peer key")?);
        let text = transcript(key.verifying_key().as_bytes(), &hello, public.as_bytes());
        let cipher = Self::new(
            ephemeral.diffie_hellman(&client).to_bytes(),
            Sha256::digest(&text).into(),
            true,
        )?;
        let signature = key.sign(&text);
        let welcome = [
            key.verifying_key().as_bytes().as_slice(),
            public.as_bytes().as_slice(),
            signature.to_bytes().as_slice(),
        ]
        .concat();
        Ok((cipher, STANDARD.encode(welcome)))
    }

    fn nonce(sequence: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&sequence.to_be_bytes());
        nonce
    }

    pub fn seal(&mut self, value: &serde_json::Value) -> Result<Vec<Packet>, String> {
        if self.failed {
            return Err("Tunnel is closed".into());
        }
        let mut bytes = serde_json::to_vec(value).map_err(|_| "Invalid tunnel JSON")?;
        if bytes.len() > MAX_FRAME {
            return Err("Tunnel frame exceeds its limit".into());
        }
        let mut packets = Vec::new();
        for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
            if self.sent > u32::MAX as u64 {
                self.failed = true;
                return Err("Tunnel key exhausted".into());
            }
            let mut padded = vec![0u8; (chunk.len() + 5).div_ceil(256) * 256];
            padded[0] = u8::from((index + 1) * CHUNK >= bytes.len());
            padded[1..5].copy_from_slice(&(chunk.len() as u32).to_be_bytes());
            padded[5..5 + chunk.len()].copy_from_slice(chunk);
            let sequence = self.sent;
            self.sent += 1;
            let nonce = Self::nonce(sequence);
            let aad = [self.transcript.as_slice(), nonce.as_slice()].concat();
            let sealed = ChaCha20Poly1305::new((&self.outbound).into())
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &padded,
                        aad: &aad,
                    },
                )
                .map_err(|_| "Tunnel encryption failed")?;
            padded.zeroize();
            packets.push(Packet {
                sequence,
                data: STANDARD.encode(sealed),
            });
        }
        bytes.zeroize();
        Ok(packets)
    }

    pub fn open(&mut self, packet: &Packet) -> Result<Option<serde_json::Value>, String> {
        let result = self.open_inner(packet);
        if result.is_err() {
            self.failed = true;
            self.fragments.zeroize();
        }
        result
    }

    fn open_inner(&mut self, packet: &Packet) -> Result<Option<serde_json::Value>, String> {
        if self.failed || packet.sequence != self.received || self.received > u32::MAX as u64 {
            return Err("Tunnel replay or missing packet".into());
        }
        let sealed = STANDARD
            .decode(&packet.data)
            .map_err(|_| "Invalid tunnel encoding")?;
        if sealed.len() < 272
            || sealed.len() > CHUNK + 272
            || STANDARD.encode(&sealed) != packet.data
        {
            return Err("Invalid tunnel packet length".into());
        }
        let nonce = Self::nonce(packet.sequence);
        let aad = [self.transcript.as_slice(), nonce.as_slice()].concat();
        let mut plain = ChaCha20Poly1305::new((&self.inbound).into())
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &sealed,
                    aad: &aad,
                },
            )
            .map_err(|_| "Tunnel authentication failed")?;
        let length = u32::from_be_bytes(
            plain[1..5]
                .try_into()
                .map_err(|_| "Invalid tunnel framing")?,
        ) as usize;
        if plain[0] > 1
            || length > CHUNK
            || length + 5 > plain.len()
            || plain.len() % 256 != 0
            || plain[length + 5..].iter().any(|b| *b != 0)
        {
            return Err("Invalid tunnel framing".into());
        }
        self.received += 1;
        if self.fragments.len() + length > MAX_FRAME {
            return Err("Tunnel reassembly exceeds its limit".into());
        }
        self.fragments.extend_from_slice(&plain[5..5 + length]);
        let last = plain[0] == 1;
        plain.zeroize();
        if !last {
            return Ok(None);
        }
        let value = serde_json::from_slice(&self.fragments).map_err(|_| "Invalid tunnel JSON");
        self.fragments.zeroize();
        self.fragments.clear();
        value.map(Some).map_err(Into::into)
    }
}

pub struct ClientHandshake {
    secret: StaticSecret,
    hello: [u8; 64],
}
impl Default for ClientHandshake {
    fn default() -> Self {
        Self::new()
    }
}
impl ClientHandshake {
    pub fn new() -> Self {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let secret = StaticSecret::from(seed);
        seed.zeroize();
        let mut hello = [0u8; 64];
        hello[..32].copy_from_slice(PublicKey::from(&secret).as_bytes());
        rand::rngs::OsRng.fill_bytes(&mut hello[32..]);
        Self { secret, hello }
    }
    pub fn hello(&self) -> String {
        STANDARD.encode(self.hello)
    }
    pub fn finish(self, host: &str, welcome: &str) -> Result<Cipher, String> {
        let bytes = decode(welcome, 128)?;
        if hex::encode(Sha256::digest(&bytes[..32])) != host {
            return Err("The router returned a different server identity".into());
        }
        let key = VerifyingKey::from_bytes(bytes[..32].try_into().map_err(|_| "Invalid host key")?)
            .map_err(|_| "Invalid host key")?;
        let public =
            PublicKey::from(<[u8; 32]>::try_from(&bytes[32..64]).map_err(|_| "Invalid peer key")?);
        let text = transcript(key.as_bytes(), &self.hello, public.as_bytes());
        key.verify_strict(
            &text,
            &Signature::from_slice(&bytes[64..]).map_err(|_| "Invalid host signature")?,
        )
        .map_err(|_| "The server did not authenticate this connection")?;
        Cipher::new(
            self.secret.diffie_hellman(&public).to_bytes(),
            Sha256::digest(&text).into(),
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vectors() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../docs/protocol/fixtures/public-host-v2.json"
        ))
        .unwrap()
    }
    fn seed(v: &serde_json::Value, name: &str) -> [u8; 32] {
        hex::decode(v[name].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap()
    }
    fn pair() -> (Cipher, Cipher) {
        let v = vectors();
        let key = SigningKey::from_bytes(&seed(&v, "identitySeed"));
        let hello = ClientHandshake {
            secret: StaticSecret::from(seed(&v, "clientSeed")),
            hello: STANDARD
                .decode(v["hello"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap(),
        };
        let (host, welcome) =
            Cipher::accept_with_seed(&key, &hello.hello(), seed(&v, "serverSeed")).unwrap();
        assert_eq!(welcome, v["welcome"].as_str().unwrap());
        let client = hello.finish(v["host"].as_str().unwrap(), &welcome).unwrap();
        (client, host)
    }
    #[test]
    fn matches_independent_openssl_and_typescript_vectors() {
        let v = vectors();
        let (mut client, mut host) = pair();
        let request: Packet = serde_json::from_value(v["requestPacket"].clone()).unwrap();
        let response: Packet = serde_json::from_value(v["responsePacket"].clone()).unwrap();
        assert_eq!(client.seal(&v["request"]).unwrap()[0].data, request.data);
        assert_eq!(host.open(&request).unwrap(), Some(v["request"].clone()));
        assert_eq!(host.seal(&v["response"]).unwrap()[0].data, response.data);
        assert_eq!(client.open(&response).unwrap(), Some(v["response"].clone()));
    }
    #[test]
    fn rejects_replay_tampering_reflection_and_missing_packets() {
        let v = vectors();
        let packet: Packet = serde_json::from_value(v["requestPacket"].clone()).unwrap();
        let (_, mut host) = pair();
        host.open(&packet).unwrap();
        assert!(host.open(&packet).is_err());
        assert!(pair().0.open(&packet).is_err());
        let mut altered = packet.clone();
        altered.sequence = 1;
        assert!(pair().1.open(&altered).is_err());
        let mut bytes = STANDARD.decode(&packet.data).unwrap();
        bytes[12] ^= 1;
        altered.sequence = 0;
        altered.data = STANDARD.encode(bytes);
        assert!(pair().1.open(&altered).is_err());
    }
    #[test]
    fn large_application_frames_are_reassembled_only_after_authentication() {
        let (mut client, mut host) = pair();
        let value = serde_json::json!({"content":"private file bytes ".repeat(60_000)});
        let packets = client.seal(&value).unwrap();
        assert!(packets.len() > 10);
        for packet in &packets[..packets.len() - 1] {
            assert!(host.open(packet).unwrap().is_none());
        }
        assert_eq!(host.open(packets.last().unwrap()).unwrap(), Some(value));
        assert!(packets.iter().all(|packet| packet.data.len() < 100_000));
    }
    #[test]
    fn rejects_wrong_host_identity_and_noncontributory_client_key() {
        let v = vectors();
        let hello = ClientHandshake::new();
        assert!(hello
            .finish(&"0".repeat(64), v["welcome"].as_str().unwrap())
            .is_err());
        let key = SigningKey::from_bytes(&seed(&v, "identitySeed"));
        assert!(
            Cipher::accept_with_seed(&key, &STANDARD.encode([0; 64]), seed(&v, "serverSeed"))
                .is_err()
        );
    }
}

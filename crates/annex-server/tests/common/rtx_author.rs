//! Real handshake and author signatures for bounded RTX API integration tests.
//! Platform authentication is supplied separately by each test fixture.
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::Signer;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use tower::ServiceExt;

pub struct AcceptedAuthor {
    key: ed25519_dalek::SigningKey,
    reference: String,
}

impl AcceptedAuthor {
    pub fn sign(&self, value: &mut Value) {
        value["vrp_handshake_ref"] = Value::String(self.reference.clone());
        let mut bundle: annex_rtx::ReflectionSummaryBundle =
            serde_json::from_value(value.clone()).unwrap();
        let payload = annex_rtx::author_signing_payload(&bundle);
        bundle.signature = hex::encode(
            self.key
                .sign(&Sha256::digest(payload.as_bytes()))
                .to_bytes(),
        );
        *value = serde_json::to_value(bundle).unwrap();
    }
}

pub async fn accept(
    app: &axum::Router,
    pseudonym: &str,
    session_token: Option<&str>,
) -> AcceptedAuthor {
    let key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let handshake = annex_vrp::VrpFederationHandshake {
        anchor_snapshot: annex_vrp::VrpAnchorSnapshot::new(&[], &[]).unwrap(),
        capability_contract: annex_vrp::VrpCapabilitySharingContract {
            offered_capabilities: vec!["TEXT".into(), "VRP".into()],
            ..Default::default()
        },
        scorer: None,
    };
    let mut builder = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json");
    if let Some(token) = session_token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let mut request = builder
        .body(Body::from(
            serde_json::json!({"pseudonymId":pseudonym,"handshake":handshake,
            "signingPubkey":hex::encode(key.verifying_key().to_bytes())})
            .to_string(),
        ))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    let reference = report["handshakeRef"]
        .as_str()
        .expect("accepted handshake reference")
        .to_owned();
    assert!(reference.starts_with("vrp:1:"));
    AcceptedAuthor { key, reference }
}

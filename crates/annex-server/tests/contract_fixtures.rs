//! Contract tests for the shared protocol fixtures under `fixtures/`.
//!
//! Each fixture is the canonical wire representation of one request,
//! response, or WebSocket frame. These tests pin the Rust side of every
//! contract:
//!
//!   • Request fixtures  → deserialize into the corresponding handler
//!     request struct.
//!   • Response fixtures → either deserialize into the response struct
//!     (where one is defined) or are compared structurally to a freshly
//!     built `serde_json::Value`.
//!   • Incoming WS frames → deserialize into [`IncomingMessage`].
//!   • Outgoing WS frames → built in Rust, serialized, and compared
//!     against the fixture (since `OutgoingMessage` is `Serialize`-only).
//!
//! `client/src/contract.test.ts` covers the matching TypeScript side, so
//! the two implementations cannot drift without one of these test
//! suites failing.

use annex_server::api::{
    RegisterRequest, RegisterResponse, VerifyMembershipRequest, VerifyMembershipResponse,
};
use annex_server::api_channels::CreateChannelRequest;
use annex_server::api_ws::{IncomingMessage, OutgoingMessage, WsMessagePayload};
use annex_types::{ChannelType, FederationScope};
use serde_json::Value;

const FX_REGISTER_REQUEST: &str = include_str!("../../../fixtures/api/register.request.json");
const FX_REGISTER_RESPONSE: &str = include_str!("../../../fixtures/api/register.response.json");
const FX_VERIFY_MEMBERSHIP_REQUEST: &str =
    include_str!("../../../fixtures/api/verify-membership.request.json");
const FX_VERIFY_MEMBERSHIP_RESPONSE: &str =
    include_str!("../../../fixtures/api/verify-membership.response.json");
const FX_CREATE_CHANNEL_REQUEST: &str =
    include_str!("../../../fixtures/api/create-channel.request.json");
const FX_CREATE_CHANNEL_RESPONSE: &str =
    include_str!("../../../fixtures/api/create-channel.response.json");
const FX_WS_INCOMING_MESSAGE: &str = include_str!("../../../fixtures/ws/incoming-message.json");
const FX_WS_INCOMING_EDIT: &str = include_str!("../../../fixtures/ws/incoming-edit-message.json");
const FX_WS_INCOMING_DELETE: &str =
    include_str!("../../../fixtures/ws/incoming-delete-message.json");
const FX_WS_INCOMING_TYPING: &str = include_str!("../../../fixtures/ws/incoming-typing.json");
const FX_WS_OUTGOING_MESSAGE: &str = include_str!("../../../fixtures/ws/outgoing-message.json");
const FX_WS_OUTGOING_RESUMED: &str = include_str!("../../../fixtures/ws/outgoing-resumed.json");
const FX_WS_OUTGOING_ERROR: &str = include_str!("../../../fixtures/ws/outgoing-error.json");
const FX_WS_VOICE_OFFER: &str = include_str!("../../../fixtures/ws/voice-offer.json");
const FX_WS_WEBRTC_ICE: &str = include_str!("../../../fixtures/ws/webrtc-ice-candidate.json");

// ── HTTP API: requests ──────────────────────────────────────────────────

#[test]
fn contract_register_request_fixture_matches_struct() {
    let req: RegisterRequest = serde_json::from_str(FX_REGISTER_REQUEST)
        .expect("register.request.json must deserialize into RegisterRequest");
    assert_eq!(
        req.commitment_hex,
        "0000000000000000000000000000000000000000000000000000000000000001"
    );
    assert_eq!(req.role_code, 1);
    assert_eq!(req.node_id, 42);
    assert_eq!(req.invite_code.as_deref(), Some("INVITE-ABC123"));
    assert_eq!(req.server_password.as_deref(), Some("shared-secret"));
}

#[test]
fn contract_register_response_fixture_matches_struct() {
    let resp: RegisterResponse = serde_json::from_str(FX_REGISTER_RESPONSE)
        .expect("register.response.json must deserialize into RegisterResponse");
    assert_eq!(resp.identity_id, 7);
    assert_eq!(resp.leaf_index, 6);
    // Canonical 64-char lowercase hex, no `0x` prefix — matches what
    // `fr_to_canonical_hex` emits across every Rust → JSON boundary,
    // and (critically) what `parse_fr_from_hex` accepts on the round
    // trip back. The pre-[F34] fixture used `0x`-prefixed strings that
    // the server's tolerant hex parser actually rejects (`hex::decode`
    // chokes on the `x` byte), so a client bootstrapped from this
    // fixture would have hit a hard 400 on the very next request.
    assert_eq!(
        resp.root_hex,
        "2ab3a44d96d63f10b5b6f1c7c0a9c4d3e2f1a0908070605040302010f0e0d0c0"
    );
    assert_eq!(
        resp.root_hex.len(),
        64,
        "rootHex must be canonical 64-char hex"
    );
    assert!(
        resp.root_hex
            .chars()
            .all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "rootHex must be lowercase canonical hex with no 0x prefix"
    );
    assert_eq!(resp.path_elements.len(), 3);
    for (i, h) in resp.path_elements.iter().enumerate() {
        assert_eq!(
            h.len(),
            64,
            "pathElements[{i}] must be canonical 64-char hex"
        );
        assert!(
            h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "pathElements[{i}] must be lowercase canonical hex with no 0x prefix"
        );
    }
    assert_eq!(resp.path_indices, vec![0u8, 1, 0]);

    // Round-trip through the wire shape so casing / renames stay stable.
    let round_tripped: Value = serde_json::to_value(&resp).expect("RegisterResponse serializes");
    let original: Value = serde_json::from_str(FX_REGISTER_RESPONSE).unwrap();
    assert_eq!(round_tripped, original);
}

#[test]
fn contract_verify_membership_request_fixture_matches_struct() {
    let req: VerifyMembershipRequest = serde_json::from_str(FX_VERIFY_MEMBERSHIP_REQUEST)
        .expect("verify-membership.request.json must deserialize into VerifyMembershipRequest");
    // Same canonical hex shape as the register.response fixture — see
    // [F34] for the production wire-format alignment rationale.
    assert_eq!(
        req.root,
        "2ab3a44d96d63f10b5b6f1c7c0a9c4d3e2f1a0908070605040302010f0e0d0c0"
    );
    assert_eq!(
        req.commitment,
        "0000000000000000000000000000000000000000000000000000000000000001"
    );
    assert_eq!(req.topic, "annex:identity:v1");
    assert_eq!(req.public_signals.len(), 2);
    for sig in &req.public_signals {
        assert_eq!(sig.len(), 64, "publicSignals must be canonical 64-char hex");
        assert!(
            sig.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "publicSignals must be lowercase canonical hex with no 0x prefix"
        );
    }
    // The fixture targets the v1 verifier, so these v2-only fields stay None.
    assert!(req.protocol_version.is_none());
    assert!(req.nullifier_hex.is_none());
    assert!(req.topic_hash_hex.is_none());
    // The proof itself is opaque to the contract — the wire shape just has
    // to be a JSON object that serde accepts as `serde_json::Value`.
    assert!(req.proof.is_object());
}

#[test]
fn contract_verify_membership_response_fixture_matches_struct() {
    let resp: VerifyMembershipResponse = serde_json::from_str(FX_VERIFY_MEMBERSHIP_RESPONSE)
        .expect("verify-membership.response.json must deserialize into VerifyMembershipResponse");
    assert!(resp.ok);
    assert_eq!(resp.pseudonym_id, "pseudo-7c0a9c4d3e2f1a09");
    assert_eq!(
        resp.session_token,
        "cHNldWRvLTdjMHw5OTk5OTk5OXx2YWxpZHNpZw=="
    );

    let round_tripped: Value = serde_json::to_value(&resp).unwrap();
    let original: Value = serde_json::from_str(FX_VERIFY_MEMBERSHIP_RESPONSE).unwrap();
    assert_eq!(round_tripped, original);
}

#[test]
fn contract_create_channel_request_fixture_matches_struct() {
    let req: CreateChannelRequest = serde_json::from_str(FX_CREATE_CHANNEL_REQUEST)
        .expect("create-channel.request.json must deserialize into CreateChannelRequest");
    assert_eq!(req.channel_id, "general");
    assert_eq!(req.name, "General");
    assert!(matches!(req.channel_type, ChannelType::Text));
    assert_eq!(req.topic.as_deref(), Some("Project-wide announcements"));
    assert!(req.vrp_topic_binding.is_none());
    assert!(req.required_capabilities_json.is_none());
    assert!(req.agent_min_alignment.is_none());
    assert!(req.retention_days.is_none());
    assert!(matches!(req.federation_scope, FederationScope::Local));
}

#[test]
fn contract_create_channel_response_fixture_is_status_object() {
    // The handler returns `Json(json!({"status": "created"}))` — there is
    // no dedicated Rust struct, so we pin the wire shape directly. This
    // also documents the (intentional) difference between the HTTP body
    // and the channel payload broadcast over the WebSocket.
    let resp: Value = serde_json::from_str(FX_CREATE_CHANNEL_RESPONSE)
        .expect("create-channel.response.json must be valid JSON");
    let obj = resp.as_object().expect("response is a JSON object");
    assert_eq!(
        obj.len(),
        1,
        "only the `status` field is part of the contract"
    );
    assert_eq!(obj.get("status").and_then(Value::as_str), Some("created"));
}

// ── WebSocket: client → server ──────────────────────────────────────────

#[test]
fn contract_ws_incoming_message_fixture_deserializes() {
    let frame: IncomingMessage = serde_json::from_str(FX_WS_INCOMING_MESSAGE)
        .expect("incoming-message.json must deserialize into IncomingMessage::Message");
    match frame {
        IncomingMessage::Message {
            channel_id,
            content,
            reply_to,
            client_request_id,
        } => {
            assert_eq!(channel_id, "general");
            assert_eq!(content, "Hello, world!");
            assert!(reply_to.is_none());
            assert_eq!(client_request_id.as_deref(), Some("req-018dSCao"));
        }
        other => panic!("expected IncomingMessage::Message, got {other:?}"),
    }
}

#[test]
fn contract_ws_incoming_edit_message_fixture_deserializes() {
    let frame: IncomingMessage = serde_json::from_str(FX_WS_INCOMING_EDIT)
        .expect("incoming-edit-message.json must deserialize into IncomingMessage::EditMessage");
    match frame {
        IncomingMessage::EditMessage {
            channel_id,
            message_id,
            content,
        } => {
            assert_eq!(channel_id, "general");
            assert_eq!(message_id, "msg-018dSCao");
            assert_eq!(content, "Hello, world! (edited)");
        }
        other => panic!("expected IncomingMessage::EditMessage, got {other:?}"),
    }
}

#[test]
fn contract_ws_incoming_delete_message_fixture_deserializes() {
    let frame: IncomingMessage = serde_json::from_str(FX_WS_INCOMING_DELETE).expect(
        "incoming-delete-message.json must deserialize into IncomingMessage::DeleteMessage",
    );
    match frame {
        IncomingMessage::DeleteMessage {
            channel_id,
            message_id,
        } => {
            assert_eq!(channel_id, "general");
            assert_eq!(message_id, "msg-018dSCao");
        }
        other => panic!("expected IncomingMessage::DeleteMessage, got {other:?}"),
    }
}

#[test]
fn contract_ws_incoming_typing_fixture_deserializes() {
    let frame: IncomingMessage = serde_json::from_str(FX_WS_INCOMING_TYPING)
        .expect("incoming-typing.json must deserialize into IncomingMessage::Typing");
    match frame {
        IncomingMessage::Typing { channel_id } => {
            assert_eq!(channel_id, "general");
        }
        other => panic!("expected IncomingMessage::Typing, got {other:?}"),
    }
}

#[test]
fn contract_ws_voice_offer_fixture_deserializes() {
    let frame: IncomingMessage = serde_json::from_str(FX_WS_VOICE_OFFER)
        .expect("voice-offer.json must deserialize into IncomingMessage::WebRtcOffer");
    match frame {
        IncomingMessage::WebRtcOffer { channel_id, sdp } => {
            assert_eq!(channel_id, "voice-1");
            assert!(sdp.starts_with("v=0"));
            assert!(sdp.contains("m=audio"));
        }
        other => panic!("expected IncomingMessage::WebRtcOffer, got {other:?}"),
    }
}

#[test]
fn contract_ws_webrtc_ice_candidate_fixture_deserializes_incoming() {
    let frame: IncomingMessage = serde_json::from_str(FX_WS_WEBRTC_ICE).expect(
        "webrtc-ice-candidate.json must deserialize into IncomingMessage::WebRtcIceCandidate",
    );
    match frame {
        IncomingMessage::WebRtcIceCandidate {
            channel_id,
            candidate,
            sdp_mid,
            sdp_m_line_index,
            username_fragment,
        } => {
            assert_eq!(channel_id, "voice-1");
            assert!(candidate.starts_with("candidate:"));
            assert_eq!(sdp_mid.as_deref(), Some("0"));
            assert_eq!(sdp_m_line_index, Some(0));
            assert_eq!(username_fragment.as_deref(), Some("abc1"));
        }
        other => panic!("expected IncomingMessage::WebRtcIceCandidate, got {other:?}"),
    }
}

// ── WebSocket: server → client ──────────────────────────────────────────
//
// `OutgoingMessage` is `Serialize`-only, so the contract test builds the
// expected value in Rust, serialises it, and compares the JSON tree to
// the fixture. That keeps the wire shape pinned in both directions.

fn parse_value(s: &str) -> Value {
    serde_json::from_str(s).expect("fixture is valid JSON")
}

#[test]
fn contract_ws_outgoing_message_fixture_matches_serialized_struct() {
    let payload = WsMessagePayload {
        channel_id: "general".to_string(),
        message_id: "msg-018dSCao".to_string(),
        sender_pseudonym: "pseudo-7c0a9c4d3e2f1a09".to_string(),
        content: "Hello, world!".to_string(),
        reply_to_message_id: None,
        created_at: "2026-05-06T12:00:00Z".to_string(),
        edited_at: None,
        deleted_at: None,
        client_request_id: Some("req-018dSCao".to_string()),
    };
    let frame = OutgoingMessage::Message(payload);
    let serialized = serde_json::to_value(&frame).expect("OutgoingMessage::Message serializes");
    assert_eq!(serialized, parse_value(FX_WS_OUTGOING_MESSAGE));
}

#[test]
fn contract_ws_outgoing_resumed_fixture_matches_serialized_struct() {
    let frame = OutgoingMessage::Resumed {
        channel_id: "general".to_string(),
        missed_count: 3,
    };
    let serialized = serde_json::to_value(&frame).expect("OutgoingMessage::Resumed serializes");
    assert_eq!(serialized, parse_value(FX_WS_OUTGOING_RESUMED));
}

#[test]
fn contract_ws_outgoing_error_fixture_matches_serialized_struct() {
    let frame = OutgoingMessage::Error {
        message: "Send rejected: rate limit exceeded".to_string(),
        client_request_id: Some("req-018dSCao".to_string()),
    };
    let serialized = serde_json::to_value(&frame).expect("OutgoingMessage::Error serializes");
    assert_eq!(serialized, parse_value(FX_WS_OUTGOING_ERROR));
}

#[test]
fn contract_ws_webrtc_ice_candidate_fixture_matches_serialized_outgoing() {
    // The same fixture is also the wire shape the server emits when it
    // forwards an ICE candidate to the client. Pin both directions so a
    // future field rename can't pass one side's test while breaking the
    // other.
    let frame = OutgoingMessage::WebRtcIceCandidate {
        channel_id: "voice-1".to_string(),
        candidate: "candidate:842163049 1 udp 1677729535 192.168.1.1 5000 typ srflx raddr 0.0.0.0 \
             rport 0 generation 0 ufrag abc1 network-id 2"
            .to_string(),
        sdp_mid: Some("0".to_string()),
        sdp_m_line_index: Some(0),
        username_fragment: Some("abc1".to_string()),
    };
    let serialized =
        serde_json::to_value(&frame).expect("OutgoingMessage::WebRtcIceCandidate serializes");
    assert_eq!(serialized, parse_value(FX_WS_WEBRTC_ICE));
}

/// [F34] Defence in depth on top of the per-fixture assertions above.
/// This test confirms — directly against the production hex parser —
/// that the fixture's hex shape is exactly what the server accepts on
/// `POST /api/zk/verify-membership`. Pre-[F34] the fixtures had a
/// `0x` prefix; `hex::decode("0x…")` rejects it as an invalid hex
/// character, so a client bootstrapped from the fixture would have
/// hit a hard 400 on every verify-membership call. The test reads
/// the fixture, pulls out every hex field, and pushes each through
/// `parse_fr_from_hex` (the server's actual deserialiser) — anything
/// the server would reject fails the test.
#[test]
fn contract_verify_membership_request_fixture_uses_server_acceptable_hex() {
    use annex_identity::zk::parse_fr_from_hex;

    let req: VerifyMembershipRequest = serde_json::from_str(FX_VERIFY_MEMBERSHIP_REQUEST)
        .expect("verify-membership.request.json must deserialize");

    parse_fr_from_hex(&req.root)
        .expect("fixture rootHex must be acceptable to the server's hex parser");
    parse_fr_from_hex(&req.commitment)
        .expect("fixture commitmentHex must be acceptable to the server's hex parser");
    for (i, sig) in req.public_signals.iter().enumerate() {
        parse_fr_from_hex(sig)
            .unwrap_or_else(|e| panic!("fixture publicSignals[{i}] is malformed: {e}"));
    }
}

/// Same defence applied to the register.response fixture. The server
/// EMITS the values via `fr_to_canonical_hex`, so the round-trip
/// `emit → parse` must round-trip without loss. This catches the case
/// where an editor or generator accidentally re-introduces a `0x`
/// prefix or uppercases the hex in the fixture file.
#[test]
fn contract_register_response_fixture_uses_canonical_emitted_hex() {
    use annex_identity::zk::{fr_to_canonical_hex, parse_fr_from_hex};

    let resp: RegisterResponse = serde_json::from_str(FX_REGISTER_RESPONSE)
        .expect("register.response.json must deserialize");

    let root_fr = parse_fr_from_hex(&resp.root_hex)
        .expect("fixture rootHex must be acceptable to the server's hex parser");
    assert_eq!(
        fr_to_canonical_hex(root_fr),
        resp.root_hex,
        "fixture rootHex must equal its canonical re-encoding (no leading zeros stripped, no \
         uppercase, no 0x prefix)",
    );
    for (i, h) in resp.path_elements.iter().enumerate() {
        let fr = parse_fr_from_hex(h)
            .unwrap_or_else(|e| panic!("fixture pathElements[{i}] is malformed: {e}"));
        assert_eq!(
            fr_to_canonical_hex(fr),
            *h,
            "fixture pathElements[{i}] must equal its canonical re-encoding",
        );
    }
}

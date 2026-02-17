//! Unit tests for the observability event log.

use rusqlite::Connection;

use crate::error::ObserveError;
use crate::event::{EventDomain, EventPayload};
use crate::store::{emit_event, next_seq, query_events, EventFilter};

/// Creates an in-memory SQLite database with migrations applied.
fn test_db() -> Connection {
    let conn = Connection::open_in_memory().expect("should open in-memory db");
    annex_db::run_migrations(&conn).expect("migrations should succeed");
    conn
}

/// Inserts a test server and returns its ID.
fn seed_server(conn: &Connection) -> i64 {
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test Server', '{}')",
        [],
    )
    .expect("should insert server");
    conn.last_insert_rowid()
}

// ── emit_event tests ─────────────────────────────────────────────────

#[test]
fn emit_event_inserts_row() {
    let conn = test_db();
    let sid = seed_server(&conn);

    let payload = EventPayload::IdentityRegistered {
        commitment_hex: "0xabc123".to_string(),
        role_code: 1,
    };

    let id = emit_event(
        &conn,
        sid,
        EventDomain::Identity,
        payload.event_type(),
        payload.entity_type(),
        "0xabc123",
        &payload,
    )
    .expect("emit should succeed");

    assert!(id > 0, "returned row ID should be positive");

    // Verify the row exists with correct values.
    let (domain, event_type, entity_type, entity_id, seq): (String, String, String, String, i64) =
        conn.query_row(
            "SELECT domain, event_type, entity_type, entity_id, seq FROM public_event_log WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .expect("should query inserted row");

    assert_eq!(domain, "IDENTITY");
    assert_eq!(event_type, "IDENTITY_REGISTERED");
    assert_eq!(entity_type, "identity");
    assert_eq!(entity_id, "0xabc123");
    assert_eq!(seq, 1);
}

#[test]
fn emit_event_payload_round_trips_through_json() {
    let conn = test_db();
    let sid = seed_server(&conn);

    let payload = EventPayload::FederationEstablished {
        remote_url: "https://remote.example.com".to_string(),
        alignment_status: "ALIGNED".to_string(),
    };

    let id = emit_event(
        &conn,
        sid,
        EventDomain::Federation,
        payload.event_type(),
        payload.entity_type(),
        "https://remote.example.com",
        &payload,
    )
    .expect("emit should succeed");

    let payload_json: String = conn
        .query_row(
            "SELECT payload_json FROM public_event_log WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .expect("should query payload");

    let restored: EventPayload =
        serde_json::from_str(&payload_json).expect("payload should deserialise");

    match restored {
        EventPayload::FederationEstablished {
            remote_url,
            alignment_status,
        } => {
            assert_eq!(remote_url, "https://remote.example.com");
            assert_eq!(alignment_status, "ALIGNED");
        }
        other => panic!("unexpected payload variant: {other:?}"),
    }
}

// ── Sequence number tests ────────────────────────────────────────────

#[test]
fn next_seq_starts_at_one() {
    let conn = test_db();
    let sid = seed_server(&conn);

    let seq = next_seq(&conn, sid).expect("next_seq should succeed");
    assert_eq!(seq, 1);
}

#[test]
fn next_seq_increments() {
    let conn = test_db();
    let sid = seed_server(&conn);

    let payload = EventPayload::NodeAdded {
        pseudonym_id: "pseudo-1".to_string(),
        node_type: "HUMAN".to_string(),
    };

    emit_event(
        &conn,
        sid,
        EventDomain::Presence,
        payload.event_type(),
        payload.entity_type(),
        "pseudo-1",
        &payload,
    )
    .expect("first emit should succeed");

    let seq = next_seq(&conn, sid).expect("next_seq should succeed");
    assert_eq!(seq, 2);
}

#[test]
fn sequence_numbers_are_server_scoped() {
    let conn = test_db();
    let sid1 = seed_server(&conn);
    // Insert a second server.
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('test2', 'Test Server 2', '{}')",
        [],
    )
    .expect("should insert second server");
    let sid2 = conn.last_insert_rowid();

    let payload = EventPayload::NodePruned {
        pseudonym_id: "p1".to_string(),
    };

    // Emit two events on server 1.
    emit_event(
        &conn,
        sid1,
        EventDomain::Presence,
        payload.event_type(),
        payload.entity_type(),
        "p1",
        &payload,
    )
    .expect("emit on s1");
    emit_event(
        &conn,
        sid1,
        EventDomain::Presence,
        payload.event_type(),
        payload.entity_type(),
        "p1",
        &payload,
    )
    .expect("emit on s1");

    // Server 2 should start at seq 1 independently.
    let seq2 = next_seq(&conn, sid2).expect("next_seq on s2");
    assert_eq!(seq2, 1);
}

// ── query_events tests ───────────────────────────────────────────────

#[test]
fn query_events_returns_all_for_server() {
    let conn = test_db();
    let sid = seed_server(&conn);

    let payloads = [
        EventPayload::IdentityRegistered {
            commitment_hex: "0x1".to_string(),
            role_code: 1,
        },
        EventPayload::NodeAdded {
            pseudonym_id: "p1".to_string(),
            node_type: "HUMAN".to_string(),
        },
        EventPayload::AgentConnected {
            pseudonym_id: "a1".to_string(),
            alignment_status: "ALIGNED".to_string(),
        },
    ];

    for p in &payloads {
        emit_event(
            &conn,
            sid,
            p.domain(),
            p.event_type(),
            p.entity_type(),
            "test",
            p,
        )
        .expect("emit should succeed");
    }

    let events = query_events(&conn, sid, &EventFilter::default()).expect("query should succeed");
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[2].seq, 3);
}

#[test]
fn query_events_filters_by_domain() {
    let conn = test_db();
    let sid = seed_server(&conn);

    let payloads = [
        EventPayload::IdentityRegistered {
            commitment_hex: "0x1".to_string(),
            role_code: 1,
        },
        EventPayload::NodeAdded {
            pseudonym_id: "p1".to_string(),
            node_type: "HUMAN".to_string(),
        },
        EventPayload::IdentityVerified {
            commitment_hex: "0x1".to_string(),
            topic: "annex:server:v1".to_string(),
        },
    ];

    for p in &payloads {
        emit_event(
            &conn,
            sid,
            p.domain(),
            p.event_type(),
            p.entity_type(),
            "test",
            p,
        )
        .expect("emit");
    }

    let filter = EventFilter {
        domain: Some(EventDomain::Identity),
        ..Default::default()
    };
    let events = query_events(&conn, sid, &filter).expect("query");
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| e.domain == "IDENTITY"));
}

#[test]
fn query_events_filters_by_event_type() {
    let conn = test_db();
    let sid = seed_server(&conn);

    let payloads = [
        EventPayload::NodeAdded {
            pseudonym_id: "p1".to_string(),
            node_type: "HUMAN".to_string(),
        },
        EventPayload::NodePruned {
            pseudonym_id: "p1".to_string(),
        },
        EventPayload::NodeReactivated {
            pseudonym_id: "p1".to_string(),
        },
    ];

    for p in &payloads {
        emit_event(
            &conn,
            sid,
            p.domain(),
            p.event_type(),
            p.entity_type(),
            "p1",
            p,
        )
        .expect("emit");
    }

    let filter = EventFilter {
        event_type: Some("NODE_PRUNED".to_string()),
        ..Default::default()
    };
    let events = query_events(&conn, sid, &filter).expect("query");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "NODE_PRUNED");
}

#[test]
fn query_events_filters_by_entity() {
    let conn = test_db();
    let sid = seed_server(&conn);

    let p1 = EventPayload::AgentConnected {
        pseudonym_id: "agent-1".to_string(),
        alignment_status: "ALIGNED".to_string(),
    };
    let p2 = EventPayload::AgentConnected {
        pseudonym_id: "agent-2".to_string(),
        alignment_status: "PARTIAL".to_string(),
    };

    emit_event(
        &conn,
        sid,
        p1.domain(),
        p1.event_type(),
        p1.entity_type(),
        "agent-1",
        &p1,
    )
    .expect("emit");
    emit_event(
        &conn,
        sid,
        p2.domain(),
        p2.event_type(),
        p2.entity_type(),
        "agent-2",
        &p2,
    )
    .expect("emit");

    let filter = EventFilter {
        entity_id: Some("agent-1".to_string()),
        ..Default::default()
    };
    let events = query_events(&conn, sid, &filter).expect("query");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].entity_id, "agent-1");
}

#[test]
fn query_events_respects_limit() {
    let conn = test_db();
    let sid = seed_server(&conn);

    for i in 0..10 {
        let p = EventPayload::NodeAdded {
            pseudonym_id: format!("p{i}"),
            node_type: "HUMAN".to_string(),
        };
        emit_event(
            &conn,
            sid,
            p.domain(),
            p.event_type(),
            p.entity_type(),
            &format!("p{i}"),
            &p,
        )
        .expect("emit");
    }

    let filter = EventFilter {
        limit: Some(3),
        ..Default::default()
    };
    let events = query_events(&conn, sid, &filter).expect("query");
    assert_eq!(events.len(), 3);
    // Should be the first 3 (ordered by seq ASC).
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[2].seq, 3);
}

#[test]
fn query_events_isolates_servers() {
    let conn = test_db();
    let sid1 = seed_server(&conn);
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('s2', 'Server 2', '{}')",
        [],
    )
    .expect("insert server 2");
    let sid2 = conn.last_insert_rowid();

    let p = EventPayload::NodeAdded {
        pseudonym_id: "p1".to_string(),
        node_type: "HUMAN".to_string(),
    };

    emit_event(
        &conn,
        sid1,
        p.domain(),
        p.event_type(),
        p.entity_type(),
        "p1",
        &p,
    )
    .expect("emit on s1");

    let events = query_events(&conn, sid2, &EventFilter::default()).expect("query on s2");
    assert!(events.is_empty(), "server 2 should have no events");
}

// ── EventDomain tests ────────────────────────────────────────────────

#[test]
fn event_domain_round_trip() {
    for domain in [
        EventDomain::Identity,
        EventDomain::Presence,
        EventDomain::Federation,
        EventDomain::Agent,
        EventDomain::Moderation,
    ] {
        let s = domain.as_str();
        let restored: EventDomain = s.parse().expect("should parse domain string");
        assert_eq!(restored, domain);
    }
}

#[test]
fn event_domain_from_invalid() {
    assert!("INVALID".parse::<EventDomain>().is_err());
    assert!("".parse::<EventDomain>().is_err());
}

#[test]
fn event_domain_display() {
    assert_eq!(EventDomain::Identity.to_string(), "IDENTITY");
    assert_eq!(EventDomain::Moderation.to_string(), "MODERATION");
}

// ── EventPayload tests ──────────────────────────────────────────────

#[test]
fn event_payload_type_and_domain_consistency() {
    let payloads: Vec<(EventPayload, EventDomain, &str)> = vec![
        (
            EventPayload::IdentityRegistered {
                commitment_hex: "0x1".to_string(),
                role_code: 1,
            },
            EventDomain::Identity,
            "IDENTITY_REGISTERED",
        ),
        (
            EventPayload::IdentityVerified {
                commitment_hex: "0x1".to_string(),
                topic: "t".to_string(),
            },
            EventDomain::Identity,
            "IDENTITY_VERIFIED",
        ),
        (
            EventPayload::PseudonymDerived {
                pseudonym_id: "p".to_string(),
                topic: "t".to_string(),
            },
            EventDomain::Identity,
            "PSEUDONYM_DERIVED",
        ),
        (
            EventPayload::NodeAdded {
                pseudonym_id: "p".to_string(),
                node_type: "HUMAN".to_string(),
            },
            EventDomain::Presence,
            "NODE_ADDED",
        ),
        (
            EventPayload::NodePruned {
                pseudonym_id: "p".to_string(),
            },
            EventDomain::Presence,
            "NODE_PRUNED",
        ),
        (
            EventPayload::NodeReactivated {
                pseudonym_id: "p".to_string(),
            },
            EventDomain::Presence,
            "NODE_REACTIVATED",
        ),
        (
            EventPayload::FederationEstablished {
                remote_url: "u".to_string(),
                alignment_status: "ALIGNED".to_string(),
            },
            EventDomain::Federation,
            "FEDERATION_ESTABLISHED",
        ),
        (
            EventPayload::FederationRealigned {
                remote_url: "u".to_string(),
                alignment_status: "PARTIAL".to_string(),
                previous_status: "ALIGNED".to_string(),
            },
            EventDomain::Federation,
            "FEDERATION_REALIGNED",
        ),
        (
            EventPayload::FederationSevered {
                remote_url: "u".to_string(),
                reason: "conflict".to_string(),
            },
            EventDomain::Federation,
            "FEDERATION_SEVERED",
        ),
        (
            EventPayload::AgentConnected {
                pseudonym_id: "a".to_string(),
                alignment_status: "ALIGNED".to_string(),
            },
            EventDomain::Agent,
            "AGENT_CONNECTED",
        ),
        (
            EventPayload::AgentRealigned {
                pseudonym_id: "a".to_string(),
                alignment_status: "PARTIAL".to_string(),
                previous_status: "ALIGNED".to_string(),
            },
            EventDomain::Agent,
            "AGENT_REALIGNED",
        ),
        (
            EventPayload::AgentDisconnected {
                pseudonym_id: "a".to_string(),
                reason: "policy".to_string(),
            },
            EventDomain::Agent,
            "AGENT_DISCONNECTED",
        ),
        (
            EventPayload::ModerationAction {
                moderator_pseudonym: "m".to_string(),
                action_type: "ban".to_string(),
                target_pseudonym: Some("t".to_string()),
                description: "test".to_string(),
            },
            EventDomain::Moderation,
            "MODERATION_ACTION",
        ),
    ];

    for (payload, expected_domain, expected_type) in payloads {
        assert_eq!(
            payload.domain(),
            expected_domain,
            "domain mismatch for {expected_type}"
        );
        assert_eq!(
            payload.event_type(),
            expected_type,
            "type mismatch for {expected_type}"
        );
    }
}

#[test]
fn event_payload_serialises_to_tagged_json() {
    let payload = EventPayload::ModerationAction {
        moderator_pseudonym: "mod-1".to_string(),
        action_type: "kick".to_string(),
        target_pseudonym: None,
        description: "disruptive behavior".to_string(),
    };

    let json = serde_json::to_string(&payload).expect("should serialise");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse");

    assert_eq!(parsed["event"], "MODERATION_ACTION");
    assert_eq!(parsed["moderator_pseudonym"], "mod-1");
    assert_eq!(parsed["action_type"], "kick");
    assert!(parsed["target_pseudonym"].is_null());
}

// ── Error handling test ──────────────────────────────────────────────

#[test]
fn emit_event_on_missing_table_returns_database_error() {
    // Use a fresh connection without migrations.
    let conn = Connection::open_in_memory().expect("open db");

    let payload = EventPayload::NodeAdded {
        pseudonym_id: "p".to_string(),
        node_type: "HUMAN".to_string(),
    };

    let result = emit_event(
        &conn,
        1,
        EventDomain::Presence,
        payload.event_type(),
        payload.entity_type(),
        "p",
        &payload,
    );

    assert!(
        matches!(result, Err(ObserveError::Database(_))),
        "should return database error when table is missing"
    );
}

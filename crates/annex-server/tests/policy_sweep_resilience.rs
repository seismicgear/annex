//! One corrupt row must not take out a whole re-evaluation sweep.
//!
//! `recalculate_agent_alignments` walks every active agent and re-scores it
//! against the current server policy. A MISSING anchor snapshot has always
//! been tolerated — logged and skipped — but a MALFORMED one went through
//! `serde_json::from_str(...).map_err(|_| InternalServerError(...))?`, which
//! aborted the entire sweep. So a single unparseable row meant no agent was
//! re-evaluated at all, and the 500 it produced named neither the agent nor
//! what was wrong with it, because serde's message was discarded too.
//!
//! Skipping is what the missing case already does, and it is the better
//! answer: the corrupt agent keeps its previous alignment and every other
//! agent still gets scored.

mod common;

use annex_types::ServerPolicy;
use std::sync::Arc;

fn seed_agent(conn: &rusqlite::Connection, pseudonym: &str, anchor_json: Option<&str>) {
    let contract = r#"{"required_capabilities":["TEXT"],"offered_capabilities":["TEXT"]}"#;
    conn.execute(
        "INSERT INTO agent_registrations (
            server_id, pseudonym_id, alignment_status, transfer_scope,
            capability_contract_json, anchor_snapshot_json, reputation_score,
            last_handshake_at, active
         ) VALUES (1, ?1, 'ALIGNED', 'FULL_KNOWLEDGE_BUNDLE', ?2, ?3, 0.95, datetime('now'), 1)",
        rusqlite::params![pseudonym, contract, anchor_json],
    )
    .unwrap();
}

#[tokio::test]
async fn a_malformed_anchor_skips_its_agent_and_lets_the_sweep_finish() {
    let (_router, pool) = common::setup_test_app().await;
    {
        let conn = pool.get().unwrap();
        // `not json at all` is what a truncated or half-written row looks
        // like. The second agent is well-formed and must still be reached.
        seed_agent(&conn, "agent-corrupt", Some("not json at all"));
        seed_agent(
            &conn,
            "agent-intact",
            Some(r#"{"principles":[],"version":1}"#),
        );
    }

    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        ServerPolicy::default(),
    ));

    // Under the old code this returned Err and nothing after the corrupt row
    // was looked at.
    annex_server::policy::recalculate_agent_alignments(state)
        .await
        .expect("one unparseable row must not abort the sweep");

    // And the corrupt agent is left as it was rather than being half-updated.
    let conn = pool.get().unwrap();
    let status: String = conn
        .query_row(
            "SELECT alignment_status FROM agent_registrations WHERE pseudonym_id = 'agent-corrupt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status, "ALIGNED",
        "the skipped agent keeps its prior standing"
    );
}

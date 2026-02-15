use annex_db::run_migrations;
use annex_vrp::{
    check_reputation_score, record_vrp_outcome, VrpAlignmentStatus, VrpTransferScope,
    VrpValidationReport,
};
use rusqlite::Connection;

fn setup_db() -> Connection {
    let conn = Connection::open_in_memory().expect("should open in-memory db");
    run_migrations(&conn).expect("migrations should succeed");

    // Insert a dummy server for foreign key constraint
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('test-server', 'Test Server', '{}')",
        [],
    )
    .expect("should insert server");

    conn
}

#[test]
fn test_reputation_scoring() {
    let conn = setup_db();
    let server_id = 1;
    let peer_pseudonym = "agent-007";

    // Initial score should be 0.5 (neutral)
    let initial_score = check_reputation_score(&conn, server_id, peer_pseudonym)
        .expect("should check score");
    assert!((initial_score - 0.5).abs() < f32::EPSILON);

    // Record an ALIGNED outcome
    let report_aligned = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Aligned,
        transfer_scope: VrpTransferScope::FullKnowledgeBundle,
        alignment_score: 1.0,
        negotiation_notes: vec![],
    };

    record_vrp_outcome(&conn, server_id, peer_pseudonym, "AGENT", &report_aligned)
        .expect("should record outcome");

    // Score should increase
    // 0.5 + 0.1 * (1.0 - 0.5) = 0.5 + 0.05 = 0.55
    let score_after_aligned = check_reputation_score(&conn, server_id, peer_pseudonym)
        .expect("should check score");
    assert!(score_after_aligned > initial_score);
    assert!((score_after_aligned - 0.55).abs() < 0.001);

    // Record a CONFLICT outcome
    let report_conflict = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Conflict,
        transfer_scope: VrpTransferScope::NoTransfer,
        alignment_score: 0.0,
        negotiation_notes: vec!["Conflict!".to_string()],
    };

    record_vrp_outcome(&conn, server_id, peer_pseudonym, "AGENT", &report_conflict)
        .expect("should record outcome");

    // Score should decrease significantly
    // 0.55 - 0.2 * 0.55 = 0.55 - 0.11 = 0.44
    let score_after_conflict = check_reputation_score(&conn, server_id, peer_pseudonym)
        .expect("should check score");
    assert!(score_after_conflict < score_after_aligned);
    assert!((score_after_conflict - 0.44).abs() < 0.001);

    // Record a PARTIAL outcome
    let report_partial = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Partial,
        transfer_scope: VrpTransferScope::ReflectionSummariesOnly,
        alignment_score: 0.5,
        negotiation_notes: vec![],
    };

    record_vrp_outcome(&conn, server_id, peer_pseudonym, "AGENT", &report_partial)
        .expect("should record outcome");

    // Score should decrease slightly
    // 0.44 - 0.05 * 0.44 = 0.44 - 0.022 = 0.418
    let score_after_partial = check_reputation_score(&conn, server_id, peer_pseudonym)
        .expect("should check score");
    assert!(score_after_partial < score_after_conflict);
    assert!((score_after_partial - 0.418).abs() < 0.001);
}

#[test]
fn test_reputation_persistence() {
    let conn = setup_db();
    let server_id = 1;
    let peer_pseudonym = "agent-persistent";

    let report = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Aligned,
        transfer_scope: VrpTransferScope::FullKnowledgeBundle,
        alignment_score: 1.0,
        negotiation_notes: vec![],
    };

    record_vrp_outcome(&conn, server_id, peer_pseudonym, "AGENT", &report)
        .expect("should record outcome");

    // Query directly to verify persistence
    let count: i32 = conn.query_row(
        "SELECT COUNT(*) FROM vrp_handshake_log WHERE peer_pseudonym = ?1",
        [peer_pseudonym],
        |row| row.get(0)
    ).expect("should query count");

    assert_eq!(count, 1);
}

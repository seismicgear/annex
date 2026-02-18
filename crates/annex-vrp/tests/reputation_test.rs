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
    let initial_score =
        check_reputation_score(&conn, server_id, peer_pseudonym).expect("should check score");
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
    let score_after_aligned =
        check_reputation_score(&conn, server_id, peer_pseudonym).expect("should check score");
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
    let score_after_conflict =
        check_reputation_score(&conn, server_id, peer_pseudonym).expect("should check score");
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
    let score_after_partial =
        check_reputation_score(&conn, server_id, peer_pseudonym).expect("should check score");
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
    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM vrp_handshake_log WHERE peer_pseudonym = ?1",
            [peer_pseudonym],
            |row| row.get(0),
        )
        .expect("should query count");

    assert_eq!(count, 1);
}

#[test]
fn test_reputation_adversarial_oscillation() {
    // Adversarial pattern: alternating ALIGNED and CONFLICT.
    // Score should not accumulate net benefit from this pattern.
    let conn = setup_db();
    let server_id = 1;
    let peer = "adversarial-oscillator";

    let report_aligned = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Aligned,
        transfer_scope: VrpTransferScope::FullKnowledgeBundle,
        alignment_score: 1.0,
        negotiation_notes: vec![],
    };
    let report_conflict = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Conflict,
        transfer_scope: VrpTransferScope::NoTransfer,
        alignment_score: 0.0,
        negotiation_notes: vec![],
    };

    // Record 10 cycles of ALIGNED, CONFLICT
    for _ in 0..10 {
        record_vrp_outcome(&conn, server_id, peer, "AGENT", &report_aligned).unwrap();
        record_vrp_outcome(&conn, server_id, peer, "AGENT", &report_conflict).unwrap();
    }

    let score = check_reputation_score(&conn, server_id, peer).unwrap();

    // After alternating ALIGNED/CONFLICT, score should be below neutral (0.5).
    // Each ALIGNED adds 10% of gap-to-1, each CONFLICT removes 20%.
    // Net effect is negative per cycle — adversarial oscillation degrades reputation.
    assert!(
        score < 0.5,
        "adversarial oscillation should degrade reputation below neutral, got {}",
        score
    );
}

#[test]
fn test_reputation_sustained_conflict_floors() {
    // Sustained CONFLICT should drive score near zero but never exactly zero.
    let conn = setup_db();
    let server_id = 1;
    let peer = "sustained-conflict";

    let report_conflict = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Conflict,
        transfer_scope: VrpTransferScope::NoTransfer,
        alignment_score: 0.0,
        negotiation_notes: vec![],
    };

    for _ in 0..50 {
        record_vrp_outcome(&conn, server_id, peer, "AGENT", &report_conflict).unwrap();
    }

    let score = check_reputation_score(&conn, server_id, peer).unwrap();

    // After 50 CONFLICT events: 0.5 * (0.8^50) ≈ 0.0000072
    assert!(
        score < 0.001,
        "sustained conflict should drive score near zero, got {}",
        score
    );
    assert!(score >= 0.0, "score should never go below zero");
}

#[test]
fn test_reputation_independent_per_pseudonym() {
    // Reputation is scoped per pseudonym — one entity's history doesn't affect another.
    let conn = setup_db();
    let server_id = 1;

    let report_conflict = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Conflict,
        transfer_scope: VrpTransferScope::NoTransfer,
        alignment_score: 0.0,
        negotiation_notes: vec![],
    };
    let report_aligned = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Aligned,
        transfer_scope: VrpTransferScope::FullKnowledgeBundle,
        alignment_score: 1.0,
        negotiation_notes: vec![],
    };

    // Bad actor gets many conflicts
    for _ in 0..10 {
        record_vrp_outcome(&conn, server_id, "bad-actor", "AGENT", &report_conflict).unwrap();
    }

    // Good actor gets many aligned
    for _ in 0..10 {
        record_vrp_outcome(&conn, server_id, "good-actor", "AGENT", &report_aligned).unwrap();
    }

    let bad_score = check_reputation_score(&conn, server_id, "bad-actor").unwrap();
    let good_score = check_reputation_score(&conn, server_id, "good-actor").unwrap();
    let new_score = check_reputation_score(&conn, server_id, "new-actor").unwrap();

    assert!(bad_score < 0.1, "bad actor should have low reputation");
    assert!(good_score > 0.8, "good actor should have high reputation");
    assert!(
        (new_score - 0.5).abs() < f32::EPSILON,
        "new actor should start neutral"
    );
}

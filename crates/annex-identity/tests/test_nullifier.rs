use annex_db::run_migrations;
use annex_identity::{
    backfill_nullifier_owner, check_nullifier_exists, existing_nullifier_owner, insert_nullifier,
    IdentityError,
};
use rusqlite::Connection;

#[test]
fn test_nullifier_tracking_lifecycle() {
    let conn = Connection::open_in_memory().expect("failed to open db");
    run_migrations(&conn).expect("migrations failed");

    let topic = "annex:server:v1";
    let nullifier = "a".repeat(64);

    // 1. Check nullifier does not exist
    let exists = check_nullifier_exists(&conn, topic, &nullifier).expect("check failed");
    assert!(!exists, "nullifier should not exist yet");

    // 2. Insert nullifier (without lookup columns — tests legacy path)
    insert_nullifier(&conn, topic, &nullifier, None, None).expect("insertion failed");

    // 3. Check nullifier exists
    let exists = check_nullifier_exists(&conn, topic, &nullifier).expect("check failed");
    assert!(exists, "nullifier should exist now");

    // 4. Try to insert duplicate
    let err = insert_nullifier(&conn, topic, &nullifier, None, None).unwrap_err();
    assert_eq!(
        err,
        IdentityError::DuplicateNullifier(topic.to_string()),
        "should reject duplicate"
    );

    // 5. Insert same nullifier for different topic
    let other_topic = "annex:channel:v1";
    insert_nullifier(&conn, other_topic, &nullifier, None, None)
        .expect("insertion for other topic failed");

    let exists_other =
        check_nullifier_exists(&conn, other_topic, &nullifier).expect("check failed");
    assert!(exists_other, "nullifier should exist for other topic");
}

#[test]
fn test_nullifier_with_lookup_columns() {
    let conn = Connection::open_in_memory().expect("failed to open db");
    run_migrations(&conn).expect("migrations failed");

    let topic = "annex:server:v1";
    let nullifier = "b".repeat(64);
    let pseudonym = "pseudo_abc123";
    let commitment = "c".repeat(64);

    // Insert with lookup columns
    insert_nullifier(&conn, topic, &nullifier, Some(pseudonym), Some(&commitment))
        .expect("insertion with lookup columns failed");

    // Verify lookup columns are stored
    let (stored_pseudo, stored_commit): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT pseudonym_id, commitment_hex FROM zk_nullifiers WHERE topic = ?1 AND nullifier_hex = ?2",
            [topic, &nullifier],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query failed");

    assert_eq!(stored_pseudo.as_deref(), Some(pseudonym));
    assert_eq!(stored_commit.as_deref(), Some(commitment.as_str()));

    // Verify indexed lookup by pseudonym_id works
    let found: Option<(String, String)> = conn
        .query_row(
            "SELECT commitment_hex, topic FROM zk_nullifiers WHERE pseudonym_id = ?1",
            [pseudonym],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    assert!(found.is_some());
    let (found_commit, found_topic) = found.expect("should find by pseudonym");
    assert_eq!(found_commit, commitment);
    assert_eq!(found_topic, topic);
}

#[test]
fn existing_nullifier_owner_reports_the_binding() {
    let conn = Connection::open_in_memory().expect("failed to open db");
    run_migrations(&conn).expect("migrations failed");

    let topic = "annex:server:demo:v2";
    let nullifier = "b".repeat(64);

    assert_eq!(
        existing_nullifier_owner(&conn, topic, &nullifier).expect("lookup failed"),
        None,
        "an unused nullifier has no owner"
    );

    insert_nullifier(&conn, topic, &nullifier, Some("pseudo-1"), Some("commit-1"))
        .expect("insertion failed");

    let owner = existing_nullifier_owner(&conn, topic, &nullifier)
        .expect("lookup failed")
        .expect("nullifier should have an owner once consumed");
    assert_eq!(owner.pseudonym_id.as_deref(), Some("pseudo-1"));
    assert_eq!(owner.commitment_hex.as_deref(), Some("commit-1"));
}

#[test]
fn backfill_fills_only_missing_owner_columns() {
    let conn = Connection::open_in_memory().expect("failed to open db");
    run_migrations(&conn).expect("migrations failed");

    let topic = "annex:server:demo:v2";

    // A row written before migration 024 added the denormalised columns.
    let legacy = "c".repeat(64);
    insert_nullifier(&conn, topic, &legacy, None, None).expect("insertion failed");
    backfill_nullifier_owner(&conn, topic, &legacy, "pseudo-legacy", "commit-legacy")
        .expect("backfill failed");

    let owner = existing_nullifier_owner(&conn, topic, &legacy)
        .expect("lookup failed")
        .expect("row exists");
    assert_eq!(owner.pseudonym_id.as_deref(), Some("pseudo-legacy"));
    assert_eq!(owner.commitment_hex.as_deref(), Some("commit-legacy"));

    // An existing binding must never be overwritten — re-authentication
    // relies on it to tell the owner apart from a different identity.
    let bound = "d".repeat(64);
    insert_nullifier(
        &conn,
        topic,
        &bound,
        Some("pseudo-real"),
        Some("commit-real"),
    )
    .expect("insertion failed");
    backfill_nullifier_owner(&conn, topic, &bound, "pseudo-other", "commit-other")
        .expect("backfill failed");

    let owner = existing_nullifier_owner(&conn, topic, &bound)
        .expect("lookup failed")
        .expect("row exists");
    assert_eq!(owner.pseudonym_id.as_deref(), Some("pseudo-real"));
    assert_eq!(owner.commitment_hex.as_deref(), Some("commit-real"));
}

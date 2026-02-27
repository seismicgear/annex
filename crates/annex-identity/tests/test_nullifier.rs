use annex_db::run_migrations;
use annex_identity::{check_nullifier_exists, insert_nullifier, IdentityError};
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

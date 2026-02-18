use annex_db::run_migrations;
use annex_identity::{
    check_nullifier_exists, insert_nullifier, insert_nullifier_with_lookup,
    lookup_commitment_by_pseudonym, IdentityError,
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

    // 2. Insert nullifier
    insert_nullifier(&conn, topic, &nullifier).expect("insertion failed");

    // 3. Check nullifier exists
    let exists = check_nullifier_exists(&conn, topic, &nullifier).expect("check failed");
    assert!(exists, "nullifier should exist now");

    // 4. Try to insert duplicate
    let err = insert_nullifier(&conn, topic, &nullifier).unwrap_err();
    assert_eq!(
        err,
        IdentityError::DuplicateNullifier(topic.to_string()),
        "should reject duplicate"
    );

    // 5. Insert same nullifier for different topic
    let other_topic = "annex:channel:v1";
    insert_nullifier(&conn, other_topic, &nullifier).expect("insertion for other topic failed");

    let exists_other =
        check_nullifier_exists(&conn, other_topic, &nullifier).expect("check failed");
    assert!(exists_other, "nullifier should exist for other topic");
}

#[test]
fn test_insert_with_lookup_stores_pseudonym_and_commitment() {
    let conn = Connection::open_in_memory().expect("failed to open db");
    run_migrations(&conn).expect("migrations failed");

    let topic = "annex:server:v1";
    let nullifier = "b".repeat(64);
    let pseudonym_id = "pseudo-abc123";
    let commitment_hex = "c".repeat(64);

    // Insert with lookup columns
    insert_nullifier_with_lookup(
        &conn,
        topic,
        &nullifier,
        Some(pseudonym_id),
        Some(&commitment_hex),
    )
    .expect("insertion with lookup failed");

    // Verify via standard existence check
    assert!(check_nullifier_exists(&conn, topic, &nullifier).expect("check failed"));

    // Verify lookup works
    let result =
        lookup_commitment_by_pseudonym(&conn, pseudonym_id).expect("lookup failed");
    assert!(result.is_some(), "should find commitment by pseudonym");
    let (found_commitment, found_topic) = result.unwrap();
    assert_eq!(found_commitment, commitment_hex);
    assert_eq!(found_topic, topic);
}

#[test]
fn test_lookup_returns_none_for_unknown_pseudonym() {
    let conn = Connection::open_in_memory().expect("failed to open db");
    run_migrations(&conn).expect("migrations failed");

    let result = lookup_commitment_by_pseudonym(&conn, "nonexistent-pseudo")
        .expect("lookup should not error");
    assert!(result.is_none(), "should return None for unknown pseudonym");
}

#[test]
fn test_lookup_returns_none_for_legacy_rows_without_pseudonym() {
    let conn = Connection::open_in_memory().expect("failed to open db");
    run_migrations(&conn).expect("migrations failed");

    // Insert without lookup columns (legacy path)
    let topic = "annex:server:v1";
    let nullifier = "d".repeat(64);
    insert_nullifier(&conn, topic, &nullifier).expect("insertion failed");

    // No pseudonym was stored, so lookup by any pseudonym should return None
    let result = lookup_commitment_by_pseudonym(&conn, "any-pseudo")
        .expect("lookup should not error");
    assert!(result.is_none(), "legacy rows have no pseudonym");
}

#[test]
fn test_insert_with_lookup_duplicate_rejected() {
    let conn = Connection::open_in_memory().expect("failed to open db");
    run_migrations(&conn).expect("migrations failed");

    let topic = "annex:server:v1";
    let nullifier = "e".repeat(64);

    insert_nullifier_with_lookup(
        &conn,
        topic,
        &nullifier,
        Some("pseudo-1"),
        Some(&"f".repeat(64)),
    )
    .expect("first insertion should succeed");

    let err = insert_nullifier_with_lookup(
        &conn,
        topic,
        &nullifier,
        Some("pseudo-2"),
        Some(&"0".repeat(64)),
    )
    .unwrap_err();

    assert_eq!(
        err,
        IdentityError::DuplicateNullifier(topic.to_string()),
        "duplicate should be rejected even with different pseudonym"
    );
}

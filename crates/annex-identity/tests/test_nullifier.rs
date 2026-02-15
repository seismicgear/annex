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

    let exists_other = check_nullifier_exists(&conn, other_topic, &nullifier).expect("check failed");
    assert!(exists_other, "nullifier should exist for other topic");
}

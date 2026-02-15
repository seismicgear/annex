use annex_db::run_migrations;
use annex_identity::{
    create_platform_identity, deactivate_platform_identity, get_platform_identity,
    update_capabilities, Capabilities,
};
use annex_types::RoleCode;
use rusqlite::Connection;

#[test]
fn test_platform_identity_lifecycle() {
    let mut conn = Connection::open_in_memory().expect("failed to open in-memory db");

    // 1. Run migrations
    run_migrations(&mut conn).expect("failed to run migrations");

    // 2. Create Identity
    let server_id = 1;
    let pseudonym_id = "test-pseudonym-123";
    let role = RoleCode::Human;

    let created = create_platform_identity(&conn, server_id, pseudonym_id, role)
        .expect("failed to create identity");

    assert_eq!(created.server_id, server_id);
    assert_eq!(created.pseudonym_id, pseudonym_id);
    assert_eq!(created.participant_type, role);
    assert!(created.active);
    assert!(!created.can_voice); // Default 0

    // 3. Read Identity
    let fetched =
        get_platform_identity(&conn, server_id, pseudonym_id).expect("failed to fetch identity");
    assert_eq!(created, fetched);

    // 4. Update Capabilities
    std::thread::sleep(std::time::Duration::from_secs(1)); // Ensure updated_at changes (SQLite second resolution)

    let new_caps = Capabilities {
        can_voice: true,
        can_moderate: true,
        can_invite: false,
        can_federate: false,
        can_bridge: false,
    };

    update_capabilities(&conn, server_id, pseudonym_id, new_caps)
        .expect("failed to update capabilities");

    let updated = get_platform_identity(&conn, server_id, pseudonym_id)
        .expect("failed to fetch updated identity");

    assert!(updated.can_voice);
    assert!(updated.can_moderate);
    assert!(!updated.can_invite);
    assert!(updated.updated_at > created.updated_at); // Timestamp should update

    // 5. Deactivate Identity
    std::thread::sleep(std::time::Duration::from_secs(1)); // Ensure updated_at changes
    deactivate_platform_identity(&conn, server_id, pseudonym_id)
        .expect("failed to deactivate identity");

    let deactivated = get_platform_identity(&conn, server_id, pseudonym_id)
        .expect("failed to fetch deactivated identity");

    assert!(!deactivated.active);
    assert!(deactivated.updated_at > updated.updated_at);
}

#[test]
fn test_duplicate_pseudonym_per_server() {
    let mut conn = Connection::open_in_memory().expect("failed to open in-memory db");
    run_migrations(&mut conn).expect("failed to run migrations");

    let server_id = 1;
    let pseudonym_id = "duplicate-check";
    let role = RoleCode::AiAgent;

    create_platform_identity(&conn, server_id, pseudonym_id, role)
        .expect("failed to create first identity");

    let err = create_platform_identity(&conn, server_id, pseudonym_id, role);
    assert!(err.is_err()); // Should fail unique constraint
}

#[test]
fn test_same_pseudonym_different_servers() {
    let mut conn = Connection::open_in_memory().expect("failed to open in-memory db");
    run_migrations(&mut conn).expect("failed to run migrations");

    let pseudonym_id = "shared-pseudonym"; // Usually pseudonyms are derived per topic, but let's test unique constraint logic
    let role = RoleCode::Collective;

    let id1 = create_platform_identity(&conn, 1, pseudonym_id, role)
        .expect("failed to create on server 1");
    let id2 = create_platform_identity(&conn, 2, pseudonym_id, role)
        .expect("failed to create on server 2");

    assert_eq!(id1.server_id, 1);
    assert_eq!(id2.server_id, 2);
    assert_ne!(id1.id, id2.id);
}

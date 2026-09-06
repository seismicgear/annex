use annex_db::run_migrations;
use annex_identity::{
    create_platform_identity, deactivate_platform_identity, get_platform_identity,
    update_capabilities, would_remove_last_moderator, Capabilities,
};
use annex_types::RoleCode;
use rusqlite::Connection;

#[test]
fn test_platform_identity_lifecycle() {
    let conn = Connection::open_in_memory().expect("failed to open in-memory db");

    // 1. Run migrations
    run_migrations(&conn).expect("failed to run migrations");

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
    // First identity on a server is the founder and gets full capabilities
    assert!(created.can_voice);
    assert!(created.can_moderate);
    assert!(created.can_invite);
    assert!(created.can_federate);

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
    let conn = Connection::open_in_memory().expect("failed to open in-memory db");
    run_migrations(&conn).expect("failed to run migrations");

    let server_id = 1;
    let pseudonym_id = "duplicate-check";
    let role = RoleCode::AiAgent;

    create_platform_identity(&conn, server_id, pseudonym_id, role)
        .expect("failed to create first identity");

    let err = create_platform_identity(&conn, server_id, pseudonym_id, role);
    assert!(err.is_err()); // Should fail unique constraint
}

#[test]
fn test_second_identity_is_not_founder() {
    let conn = Connection::open_in_memory().expect("failed to open in-memory db");
    run_migrations(&conn).expect("failed to run migrations");

    let server_id = 1;

    // First identity is the founder
    let founder = create_platform_identity(&conn, server_id, "founder-id", RoleCode::Human)
        .expect("failed to create founder");
    assert!(founder.can_voice, "founder should have can_voice");
    assert!(founder.can_moderate, "founder should have can_moderate");
    assert!(founder.can_invite, "founder should have can_invite");
    assert!(founder.can_federate, "founder should have can_federate");

    // Second identity should NOT have founder PRIVILEGES.
    let regular = create_platform_identity(&conn, server_id, "regular-id", RoleCode::Human)
        .expect("failed to create second identity");
    // ...but it can speak. This assertion used to be `!regular.can_voice`,
    // which encoded a defect rather than an intent: it left every member but
    // the founder unable to join any call, on a server whose
    // `ServerPolicy::voice_enabled` defaults to true. See
    // `every_member_can_speak_but_only_the_founder_moderates`.
    assert!(
        regular.can_voice,
        "voice is participation, not privilege — the server-level policy is the operator's control"
    );
    assert!(
        !regular.can_moderate,
        "second identity should NOT have can_moderate"
    );
    assert!(
        !regular.can_invite,
        "second identity should NOT have can_invite"
    );
    assert!(
        !regular.can_federate,
        "second identity should NOT have can_federate"
    );
}

#[test]
fn test_same_pseudonym_different_servers() {
    let conn = Connection::open_in_memory().expect("failed to open in-memory db");
    run_migrations(&conn).expect("failed to run migrations");

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

#[test]
fn test_would_remove_last_moderator_guards_lockout() {
    let conn = Connection::open_in_memory().expect("failed to open in-memory db");
    run_migrations(&conn).expect("failed to run migrations");

    let server_id = 1;

    // Founder is the sole active moderator.
    create_platform_identity(&conn, server_id, "founder", RoleCode::Human)
        .expect("failed to create founder");
    // A second, non-moderator identity.
    create_platform_identity(&conn, server_id, "regular", RoleCode::Human)
        .expect("failed to create regular identity");

    let demote = Capabilities {
        can_voice: true,
        can_moderate: false,
        can_invite: false,
        can_federate: false,
        can_bridge: false,
    };
    let promote = Capabilities {
        can_voice: true,
        can_moderate: true,
        can_invite: true,
        can_federate: true,
        can_bridge: false,
    };

    // Demoting the only moderator must be flagged.
    assert!(
        would_remove_last_moderator(&conn, server_id, "founder", demote)
            .expect("query should succeed"),
        "demoting the sole active moderator must be refused"
    );

    // Demoting a non-moderator never removes a moderator.
    assert!(
        !would_remove_last_moderator(&conn, server_id, "regular", demote)
            .expect("query should succeed"),
        "demoting a non-moderator is not a last-moderator removal"
    );

    // Granting moderation is never a removal.
    assert!(
        !would_remove_last_moderator(&conn, server_id, "founder", promote)
            .expect("query should succeed"),
        "granting/retaining moderation can never be a last-moderator removal"
    );

    // Promote the regular identity to a second moderator.
    update_capabilities(&conn, server_id, "regular", promote)
        .expect("failed to promote second moderator");

    // With two moderators, demoting either is now allowed.
    assert!(
        !would_remove_last_moderator(&conn, server_id, "founder", demote)
            .expect("query should succeed"),
        "with a second moderator present, demoting the founder is allowed"
    );

    // Deactivating the second moderator makes the founder the last one again.
    deactivate_platform_identity(&conn, server_id, "regular")
        .expect("failed to deactivate second moderator");
    assert!(
        would_remove_last_moderator(&conn, server_id, "founder", demote)
            .expect("query should succeed"),
        "an inactive moderator does not count — founder is the last active moderator"
    );
}

/// Every member can speak; only the founder gets the privileges.
///
/// `can_voice` used to be founder-only, alongside moderate/invite/federate.
/// That meant every member except the very first silently could not join any
/// call — the join button rendered disabled reading "Voice is disabled by
/// server policy for your identity", on a server whose
/// `ServerPolicy::voice_enabled` defaults to true. The operator's switch said
/// voice was on and nobody but the owner could use it.
///
/// It also hid every other voice defect behind it: two ordinary members could
/// never get into a call together, so nothing downstream of "more than one
/// person is talking" was reachable at all.
#[test]
fn every_member_can_speak_but_only_the_founder_moderates() {
    let conn = Connection::open_in_memory().expect("open db");
    run_migrations(&conn).expect("migrations");
    let server_id = 1;

    let founder = create_platform_identity(&conn, server_id, "founder-pseudonym", RoleCode::Human)
        .expect("create founder");
    let second = create_platform_identity(&conn, server_id, "second-pseudonym", RoleCode::Human)
        .expect("create second");
    let third = create_platform_identity(&conn, server_id, "third-pseudonym", RoleCode::Human)
        .expect("create third");

    for member in [&founder, &second, &third] {
        assert!(
            member.can_voice,
            "{} cannot speak — voice is participation, not privilege",
            member.pseudonym_id,
        );
    }

    assert!(founder.can_moderate, "the first registrant is the founder");
    for member in [&second, &third] {
        assert!(
            !member.can_moderate,
            "{} must not be a moderator by registering",
            member.pseudonym_id,
        );
        assert!(!member.can_invite);
        assert!(!member.can_federate);
    }
}

/// A moderator can still take voice away from one person.
///
/// The per-identity flag is the revocation mechanism; granting it at
/// registration must not make it un-revokable.
#[test]
fn voice_can_still_be_revoked_from_an_individual() {
    let conn = Connection::open_in_memory().expect("open db");
    run_migrations(&conn).expect("migrations");
    let server_id = 1;

    create_platform_identity(&conn, server_id, "founder-pseudonym", RoleCode::Human)
        .expect("create founder");
    let member = create_platform_identity(&conn, server_id, "member-pseudonym", RoleCode::Human)
        .expect("create member");
    assert!(member.can_voice);

    update_capabilities(
        &conn,
        server_id,
        "member-pseudonym",
        Capabilities {
            can_voice: false,
            can_moderate: false,
            can_invite: false,
            can_federate: false,
            can_bridge: false,
        },
    )
    .expect("update capabilities");

    let after = get_platform_identity(&conn, server_id, "member-pseudonym").expect("reload");
    assert!(!after.can_voice, "a moderator must be able to revoke voice");
}

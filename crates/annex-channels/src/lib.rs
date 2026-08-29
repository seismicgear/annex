//! Channel model and text communication for the Annex platform.
//!
//! Implements channel CRUD, message persistence, WebSocket real-time
//! delivery, message history retrieval, and retention policy enforcement.
//!
//! Channels are the primary communication primitive in Annex. They support
//! multiple types (`Text`, `Voice`, `Hybrid`, `Agent`, `Broadcast`), each
//! with distinct capability requirements and federation scoping.
//!
//! Internal layout: domain types and row mappers live in [`types`]; channel
//! CRUD in [`channels`]; channel membership in [`members`]; message
//! lifecycle (create / read / list / edit / delete / history) plus the
//! edit-window constant in [`messages`]; substring search in [`search`];
//! retention sweep in [`retention`]. The error type lives in [`error`].
//! All public items are re-exported here so external call sites continue
//! to use `annex_channels::Foo` without referencing the new submodules.

mod channels;
mod error;
mod members;
mod messages;
mod retention;
mod search;
mod types;

pub use channels::{
    create_channel, delete_channel, get_channel, list_channels, list_federated_channels,
    update_channel,
};
pub use error::ChannelError;
pub use members::{add_member, is_member, list_members, remove_member};
pub use messages::{
    create_message, delete_message, edit_message, get_edit_history, get_message, list_messages,
    EDIT_WINDOW_SECONDS,
};
pub use retention::{delete_expired_messages, prune_expired_request_ids};
pub use search::{scan_messages, search_messages};
pub use types::{
    Channel, ChannelMember, CreateChannelParams, CreateMessageParams, Message, MessageEdit,
    UpdateChannelParams,
};

#[cfg(test)]
mod tests {
    use super::*;
    use annex_db::run_migrations;
    use annex_types::{AlignmentStatus, ChannelType, FederationScope, ServerPolicy};
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().expect("failed to open in-memory db");
        run_migrations(&conn).expect("failed to run migrations");

        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).expect("failed to serialize policy");

        // We need a server to reference
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test-server', 'Test Server', ?1)",
            [policy_json],
        )
        .expect("failed to create dummy server");
        conn
    }

    #[test]
    fn test_channel_crud() {
        let conn = setup_db();
        let server_id = 1; // From setup_db

        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-123".to_string(),
            name: "General".to_string(),
            channel_type: ChannelType::Text,
            topic: Some("General discussion".to_string()),
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: Some(AlignmentStatus::Aligned),
            retention_days: Some(30),
            federation_scope: FederationScope::Local,
        };

        // Create
        create_channel(&conn, &params).expect("create failed");

        // Get
        let channel = get_channel(&conn, "chan-123").expect("get failed");
        assert_eq!(channel.name, "General");
        assert_eq!(channel.channel_type, ChannelType::Text);
        assert_eq!(channel.agent_min_alignment, Some(AlignmentStatus::Aligned));

        // List
        let channels = list_channels(&conn, server_id).expect("list failed");
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].id, channel.id);

        // Update
        let updates = UpdateChannelParams {
            name: Some("General Chat".to_string()),
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: None,
        };
        update_channel(&conn, "chan-123", &updates).expect("update failed");

        let updated = get_channel(&conn, "chan-123").expect("get updated failed");
        assert_eq!(updated.name, "General Chat");
        assert_eq!(updated.topic, Some("General discussion".to_string())); // Should be preserved

        // Delete
        delete_channel(&conn, "chan-123").expect("delete failed");
        let err = get_channel(&conn, "chan-123").unwrap_err();
        match err {
            ChannelError::NotFound(_) => (),
            _ => panic!("unexpected error type"),
        }
    }

    #[test]
    fn test_message_lifecycle() {
        let conn = setup_db();
        let server_id = 1;

        // Create a channel with specific retention
        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-msg".to_string(),
            name: "Message Test".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: Some(7),
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create channel failed");

        // Create message
        let msg_params = CreateMessageParams {
            channel_id: "chan-msg".to_string(),
            message_id: "msg-1".to_string(),
            sender_pseudonym: "pseudo-1".to_string(),
            content: "Hello World".to_string(),
            reply_to_message_id: None,
        };

        let msg = create_message(&conn, &msg_params).expect("create message failed");
        assert_eq!(msg.content, "Hello World");
        assert!(msg.expires_at.is_some()); // Should have expiration

        // Create reply
        let reply_params = CreateMessageParams {
            channel_id: "chan-msg".to_string(),
            message_id: "msg-2".to_string(),
            sender_pseudonym: "pseudo-2".to_string(),
            content: "Hello back".to_string(),
            reply_to_message_id: Some("msg-1".to_string()),
        };
        let reply = create_message(&conn, &reply_params).expect("create reply failed");
        assert_eq!(reply.reply_to_message_id, Some("msg-1".to_string()));

        // Get message
        let fetched = get_message(&conn, "msg-1").expect("get message failed");
        assert_eq!(fetched.content, "Hello World");

        // List messages
        let messages =
            list_messages(&conn, server_id, "chan-msg", None, None).expect("list messages failed");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].message_id, "msg-2"); // Reverse chronological
        assert_eq!(messages[1].message_id, "msg-1");
    }

    #[test]
    fn test_message_server_retention_fallback() {
        let conn = setup_db();
        let server_id = 1;

        // Channel with NO retention override
        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-default".to_string(),
            name: "Default Retention".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None, // Use server default
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create channel failed");

        let msg_params = CreateMessageParams {
            channel_id: "chan-default".to_string(),
            message_id: "msg-default".to_string(),
            sender_pseudonym: "pseudo-1".to_string(),
            content: "Default retention".to_string(),
            reply_to_message_id: None,
        };

        let msg = create_message(&conn, &msg_params).expect("create message failed");
        assert!(msg.expires_at.is_some());
        // Server default is 30 days (default impl of ServerPolicy)
    }

    #[test]
    fn test_channel_membership() {
        let conn = setup_db();
        let server_id = 1;

        // Create channel
        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-mem".to_string(),
            name: "Members Only".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create channel failed");

        // We need a platform identity to link to, due to FK
        // setup_db only creates the server.
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type) VALUES (1, 'user-1', 'HUMAN')",
            [],
        ).expect("create identity failed");

        // Add member
        add_member(&conn, server_id, "chan-mem", "user-1").expect("add member failed");

        // Check is_member
        assert!(is_member(&conn, server_id, "chan-mem", "user-1").unwrap());
        assert!(!is_member(&conn, server_id, "chan-mem", "user-2").unwrap());

        // List members
        let members = list_members(&conn, "chan-mem").expect("list members failed");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].pseudonym_id, "user-1");

        // Remove member
        remove_member(&conn, server_id, "chan-mem", "user-1").expect("remove member failed");
        assert!(!is_member(&conn, server_id, "chan-mem", "user-1").unwrap());
    }

    #[test]
    fn test_update_channel_nonexistent() {
        let conn = setup_db();

        let updates = UpdateChannelParams {
            name: Some("Ghost".to_string()),
            ..Default::default()
        };
        let err = update_channel(&conn, "does-not-exist", &updates).unwrap_err();
        match err {
            ChannelError::NotFound(id) => assert_eq!(id, "does-not-exist"),
            _ => panic!("expected NotFound, got {err:?}"),
        }
    }

    #[test]
    fn test_update_channel_no_fields() {
        let conn = setup_db();
        let server_id = 1;

        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-noop".to_string(),
            name: "NoOp".to_string(),
            channel_type: ChannelType::Text,
            topic: Some("original".to_string()),
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create failed");

        // Update with all None — should succeed and change nothing
        let updates = UpdateChannelParams::default();
        update_channel(&conn, "chan-noop", &updates).expect("empty update failed");

        let ch = get_channel(&conn, "chan-noop").expect("get failed");
        assert_eq!(ch.name, "NoOp");
        assert_eq!(ch.topic, Some("original".to_string()));
    }

    #[test]
    fn test_update_channel_no_fields_nonexistent() {
        let conn = setup_db();

        let updates = UpdateChannelParams::default();
        let err = update_channel(&conn, "ghost", &updates).unwrap_err();
        match err {
            ChannelError::NotFound(_) => {}
            _ => panic!("expected NotFound, got {err:?}"),
        }
    }

    #[test]
    fn test_update_channel_multiple_fields() {
        let conn = setup_db();
        let server_id = 1;

        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-multi".to_string(),
            name: "Before".to_string(),
            channel_type: ChannelType::Text,
            topic: Some("old topic".to_string()),
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: Some(7),
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create failed");

        let updates = UpdateChannelParams {
            name: Some("After".to_string()),
            topic: Some("new topic".to_string()),
            retention_days: Some(14),
            federation_scope: Some(FederationScope::Federated),
            ..Default::default()
        };
        update_channel(&conn, "chan-multi", &updates).expect("update failed");

        let ch = get_channel(&conn, "chan-multi").expect("get failed");
        assert_eq!(ch.name, "After");
        assert_eq!(ch.topic, Some("new topic".to_string()));
        assert_eq!(ch.retention_days, Some(14));
        assert_eq!(ch.federation_scope, FederationScope::Federated);
        // Untouched fields preserved
        assert_eq!(ch.vrp_topic_binding, None);
        assert_eq!(ch.required_capabilities_json, None);
    }

    #[test]
    fn test_delete_channel_cascades_to_messages_and_members() {
        let conn = setup_db();
        let server_id = 1;

        // Create channel
        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-cascade".to_string(),
            name: "Cascade Test".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create failed");

        // Add a message
        let msg = CreateMessageParams {
            channel_id: "chan-cascade".to_string(),
            message_id: "msg-cascade-1".to_string(),
            sender_pseudonym: "user-1".to_string(),
            content: "will be cascaded".to_string(),
            reply_to_message_id: None,
        };
        create_message(&conn, &msg).expect("create message failed");

        // Add a member (need platform identity for FK)
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type) VALUES (1, 'cascade-user', 'HUMAN')",
            [],
        ).expect("create identity failed");
        add_member(&conn, server_id, "chan-cascade", "cascade-user").expect("add member failed");

        // Verify data exists
        let msg_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE channel_id = 'chan-cascade'",
                [],
                |row| row.get(0),
            )
            .expect("count failed");
        assert_eq!(msg_count, 1);

        let member_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM channel_members WHERE channel_id = 'chan-cascade'",
                [],
                |row| row.get(0),
            )
            .expect("count failed");
        assert_eq!(member_count, 1);

        // Delete channel — should cascade
        delete_channel(&conn, "chan-cascade").expect("delete failed");

        // Verify messages and members are gone
        let msg_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE channel_id = 'chan-cascade'",
                [],
                |row| row.get(0),
            )
            .expect("count failed");
        assert_eq!(msg_count, 0, "messages should be deleted on cascade");

        let member_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM channel_members WHERE channel_id = 'chan-cascade'",
                [],
                |row| row.get(0),
            )
            .expect("count failed");
        assert_eq!(member_count, 0, "members should be deleted on cascade");
    }

    #[test]
    fn test_add_member_idempotent() {
        let conn = setup_db();
        let server_id = 1;

        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-idem".to_string(),
            name: "Idempotent".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create failed");

        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type) VALUES (1, 'idem-user', 'HUMAN')",
            [],
        ).expect("create identity failed");

        // First add succeeds
        add_member(&conn, server_id, "chan-idem", "idem-user").expect("first add failed");
        assert!(is_member(&conn, server_id, "chan-idem", "idem-user").expect("check failed"));

        // Second add is idempotent (no error)
        add_member(&conn, server_id, "chan-idem", "idem-user")
            .expect("idempotent add should succeed");

        // Still exactly one member
        let members = list_members(&conn, "chan-idem").expect("list failed");
        assert_eq!(members.len(), 1);
    }

    #[test]
    fn test_add_member_nonexistent_channel() {
        let conn = setup_db();
        let server_id = 1;

        let err = add_member(&conn, server_id, "nonexistent-channel", "user-1").unwrap_err();
        match err {
            ChannelError::NotFound(_) => {}
            _ => panic!("expected NotFound, got {err:?}"),
        }
    }

    #[test]
    fn test_delete_expired_messages_batched() {
        let conn = setup_db();
        let server_id = 1;

        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-expire".to_string(),
            name: "Expiring".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create failed");

        // Insert 3 messages that are already expired
        for i in 0..3 {
            conn.execute(
                "INSERT INTO messages (server_id, channel_id, message_id, sender_pseudonym, content, expires_at)
                 VALUES (1, 'chan-expire', ?1, 'user-1', 'expired', datetime('now', '-1 day'))",
                [format!("expired-{i}")],
            )
            .expect("insert expired msg failed");
        }

        // Insert 1 message that is NOT expired
        conn.execute(
            "INSERT INTO messages (server_id, channel_id, message_id, sender_pseudonym, content, expires_at)
             VALUES (1, 'chan-expire', 'not-expired', 'user-1', 'still valid', datetime('now', '+1 day'))",
            [],
        )
        .expect("insert valid msg failed");

        let deleted = delete_expired_messages(&conn).expect("delete failed");
        assert_eq!(deleted, 3, "should delete only expired messages");

        // Non-expired message should remain
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE channel_id = 'chan-expire'",
                [],
                |row| row.get(0),
            )
            .expect("count failed");
        assert_eq!(remaining, 1, "non-expired message should remain");
    }

    #[test]
    fn test_prune_expired_request_ids_respects_ttl() {
        let conn = setup_db();

        // Three ledger rows older than the TTL...
        for i in 0..3 {
            conn.execute(
                "INSERT INTO message_request_ids
                     (server_id, channel_id, sender_pseudonym, client_request_id, message_id, created_at)
                 VALUES (1, 'chan-1', 'user-1', ?1, ?2, datetime('now', '-8 days'))",
                [format!("stale-req-{i}"), format!("stale-msg-{i}")],
            )
            .expect("insert stale ledger row failed");
        }

        // ...and one fresh row inside the TTL window.
        conn.execute(
            "INSERT INTO message_request_ids
                 (server_id, channel_id, sender_pseudonym, client_request_id, message_id, created_at)
             VALUES (1, 'chan-1', 'user-1', 'fresh-req', 'fresh-msg', datetime('now', '-1 hour'))",
            [],
        )
        .expect("insert fresh ledger row failed");

        // TTL = 7 days.
        let pruned = prune_expired_request_ids(&conn, 7 * 24 * 3600).expect("prune failed");
        assert_eq!(pruned, 3, "should prune only rows older than the TTL");

        let remaining: String = conn
            .query_row(
                "SELECT client_request_id FROM message_request_ids",
                [],
                |row| row.get(0),
            )
            .expect("exactly one row should remain");
        assert_eq!(remaining, "fresh-req");
    }

    #[test]
    fn test_prune_expired_request_ids_noop_when_all_fresh() {
        let conn = setup_db();

        conn.execute(
            "INSERT INTO message_request_ids
                 (server_id, channel_id, sender_pseudonym, client_request_id, message_id)
             VALUES (1, 'chan-1', 'user-1', 'req-1', 'msg-1')",
            [],
        )
        .expect("insert ledger row failed");

        let pruned = prune_expired_request_ids(&conn, 7 * 24 * 3600).expect("prune failed");
        assert_eq!(pruned, 0, "fresh rows must survive the sweep");
    }

    /// Helper: create a channel and message for edit/delete tests.
    fn setup_editable_message(conn: &Connection) -> Message {
        let server_id = 1;
        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-edit".to_string(),
            name: "Edit Test".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(conn, &params).expect("create channel failed");

        let msg_params = CreateMessageParams {
            channel_id: "chan-edit".to_string(),
            message_id: "msg-edit-1".to_string(),
            sender_pseudonym: "user-a".to_string(),
            content: "Original content".to_string(),
            reply_to_message_id: None,
        };
        create_message(conn, &msg_params).expect("create message failed")
    }

    #[test]
    fn test_edit_message_success() {
        let conn = setup_db();
        let msg = setup_editable_message(&conn);

        let updated = edit_message(&conn, &msg.message_id, "user-a", "Edited content")
            .expect("edit should succeed");
        assert_eq!(updated.content, "Edited content");
        assert!(updated.edited_at.is_some());

        // Check edit history
        let history = get_edit_history(&conn, 1, "chan-edit", &msg.message_id)
            .expect("history should succeed");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].old_content, "Original content");
    }

    #[test]
    fn test_edit_message_wrong_sender() {
        let conn = setup_db();
        let msg = setup_editable_message(&conn);

        let err = edit_message(&conn, &msg.message_id, "user-b", "Hacked")
            .expect_err("edit by wrong sender should fail");
        match err {
            ChannelError::NotFound(_) => (),
            _ => panic!("expected NotFound, got {err:?}"),
        }
    }

    #[test]
    fn test_edit_message_expired_window() {
        let conn = setup_db();
        let msg = setup_editable_message(&conn);

        // Manually backdate the message to 2 minutes ago
        conn.execute(
            "UPDATE messages SET created_at = datetime('now', '-2 minutes') WHERE message_id = ?1",
            [&msg.message_id],
        )
        .expect("backdate failed");

        let err = edit_message(&conn, &msg.message_id, "user-a", "Too late")
            .expect_err("edit after window should fail");
        match err {
            ChannelError::NotFound(s) => {
                assert!(s.contains("expired"), "expected 'expired' in: {s}")
            }
            _ => panic!("expected NotFound, got {err:?}"),
        }
    }

    #[test]
    fn test_delete_message_success() {
        let conn = setup_db();
        let msg = setup_editable_message(&conn);

        let deleted =
            delete_message(&conn, &msg.message_id, "user-a").expect("delete should succeed");
        assert!(deleted.deleted_at.is_some());
        assert_eq!(deleted.content, "");
    }

    #[test]
    fn test_delete_message_wrong_sender() {
        let conn = setup_db();
        let msg = setup_editable_message(&conn);

        let err = delete_message(&conn, &msg.message_id, "user-b")
            .expect_err("delete by wrong sender should fail");
        match err {
            ChannelError::NotFound(_) => (),
            _ => panic!("expected NotFound, got {err:?}"),
        }
    }

    #[test]
    fn test_delete_message_expired_window() {
        let conn = setup_db();
        let msg = setup_editable_message(&conn);

        // Manually backdate the message to 2 minutes ago
        conn.execute(
            "UPDATE messages SET created_at = datetime('now', '-2 minutes') WHERE message_id = ?1",
            [&msg.message_id],
        )
        .expect("backdate failed");

        let err = delete_message(&conn, &msg.message_id, "user-a")
            .expect_err("delete after window should fail");
        match err {
            ChannelError::NotFound(s) => {
                assert!(s.contains("expired"), "expected 'expired' in: {s}")
            }
            _ => panic!("expected NotFound, got {err:?}"),
        }
    }

    #[test]
    fn test_edit_deleted_message_fails() {
        let conn = setup_db();
        let msg = setup_editable_message(&conn);

        delete_message(&conn, &msg.message_id, "user-a").expect("delete should succeed");

        let err = edit_message(&conn, &msg.message_id, "user-a", "Revive")
            .expect_err("editing deleted message should fail");
        match err {
            ChannelError::NotFound(_) => (),
            _ => panic!("expected NotFound, got {err:?}"),
        }
    }

    #[test]
    fn test_multiple_edits_preserve_history() {
        let conn = setup_db();
        let msg = setup_editable_message(&conn);

        edit_message(&conn, &msg.message_id, "user-a", "Edit 1").expect("edit 1 failed");
        edit_message(&conn, &msg.message_id, "user-a", "Edit 2").expect("edit 2 failed");
        edit_message(&conn, &msg.message_id, "user-a", "Edit 3").expect("edit 3 failed");

        let history =
            get_edit_history(&conn, 1, "chan-edit", &msg.message_id).expect("history failed");
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].old_content, "Original content");
        assert_eq!(history[1].old_content, "Edit 1");
        assert_eq!(history[2].old_content, "Edit 2");

        let current = get_message(&conn, &msg.message_id).expect("get msg failed");
        assert_eq!(current.content, "Edit 3");
    }

    /// `message_edits` has no channel column, so the scoping lives entirely
    /// in this query's join. The route that reads it takes the channel and
    /// the message as two independent path segments, and the membership
    /// check upstream can only speak about the first — asking for the
    /// history of a message under the wrong channel has to come back empty
    /// here, or the check upstream is guarding nothing.
    #[test]
    fn edit_history_does_not_cross_channels() {
        let conn = setup_db();
        let msg = setup_editable_message(&conn);
        edit_message(&conn, &msg.message_id, "user-a", "Edited content").expect("edit failed");

        let other = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-other".to_string(),
            name: "Other".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &other).expect("create other channel");

        let leaked = get_edit_history(&conn, 1, "chan-other", &msg.message_id).expect("history");
        assert!(
            leaked.is_empty(),
            "the edit history of a message in chan-edit was returned under \
             chan-other: {leaked:?}",
        );

        // A different server on the same database must not see it either.
        let cross_server =
            get_edit_history(&conn, 2, "chan-edit", &msg.message_id).expect("history");
        assert!(cross_server.is_empty(), "history crossed servers");
    }

    /// Regression test for [F31]: `edit_message` must use
    /// `BEGIN IMMEDIATE`, not `BEGIN DEFERRED`.
    ///
    /// Setup: thread A holds an IMMEDIATE transaction on conn1 that
    /// writes (but does not yet commit) a new content for the
    /// message. Thread B then calls `edit_message` on conn2.
    ///
    /// Under IMMEDIATE (correct): thread B's `BEGIN IMMEDIATE`
    /// blocks until conn1's tx commits (busy_timeout=5s). After
    /// commit, B's BEGIN succeeds, B re-reads the LATEST content
    /// ("from-conn1"), saves it to `message_edits`, and updates
    /// content to "from-thread-B". `message_edits` ends up with one
    /// row: "from-conn1" (the post-A pre-B state).
    ///
    /// Under DEFERRED (the bug being prevented): thread B's
    /// `BEGIN DEFERRED` succeeds immediately. B reads the pre-A
    /// snapshot inside its tx — content="Original". B waits at the
    /// INSERT step for the RESERVED lock. After conn1 commits, B
    /// retries, but SQLite under WAL detects the snapshot conflict
    /// (B's snapshot read a row that conn1 wrote) and returns
    /// `SQLITE_BUSY_SNAPSHOT`, which propagates as an error from
    /// `edit_message`. The test then asserts `res_b.is_ok()` —
    /// which fails under DEFERRED. (Even if SQLite were to silently
    /// continue under DEFERRED, the audit trail would record
    /// "Original" instead of "from-conn1", which the assertion at
    /// the end of the test catches.)
    #[test]
    fn edit_message_uses_immediate_to_serialize_with_external_writer() {
        use std::sync::mpsc;
        use std::thread;

        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("annex-imm-serialize-test.sqlite");

        // Bootstrap.
        {
            let conn = Connection::open(&db_path).expect("open");
            conn.execute_batch("PRAGMA journal_mode = WAL;")
                .expect("enable WAL");
            run_migrations(&conn).expect("migrations");

            let policy_json = serde_json::to_string(&ServerPolicy::default()).expect("policy");
            conn.execute(
                "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
                [policy_json],
            )
            .expect("seed server");

            let server_id: i64 = conn
                .query_row("SELECT id FROM servers WHERE slug='test'", [], |r| r.get(0))
                .expect("server id");

            create_channel(
                &conn,
                &CreateChannelParams {
                    server_id,
                    channel_id: "chan-imm".to_string(),
                    name: "Imm".to_string(),
                    channel_type: ChannelType::Text,
                    topic: None,
                    vrp_topic_binding: None,
                    required_capabilities_json: None,
                    agent_min_alignment: None,
                    retention_days: Some(7),
                    federation_scope: FederationScope::Local,
                },
            )
            .expect("create channel");

            create_message(
                &conn,
                &CreateMessageParams {
                    channel_id: "chan-imm".to_string(),
                    message_id: "msg-imm".to_string(),
                    sender_pseudonym: "user-x".to_string(),
                    content: "Original".to_string(),
                    reply_to_message_id: None,
                },
            )
            .expect("create msg");
        }

        // conn1: external writer that holds an IMMEDIATE tx with a
        // pending UPDATE to the message. A pre-flight write is what
        // causes `BEGIN_DEFERRED` (the bug) on conn2 to read a stale
        // snapshot — that snapshot conflict only manifests when conn1
        // has actually written something B's snapshot would have
        // read.
        let conn1 = Connection::open(&db_path).expect("open conn1");
        conn1
            .busy_timeout(std::time::Duration::from_secs(5))
            .expect("conn1 busy_timeout");
        conn1
            .execute_batch("BEGIN IMMEDIATE")
            .expect("conn1 BEGIN IMMEDIATE");
        conn1
            .execute(
                "UPDATE messages SET content = ?1 WHERE message_id = ?2",
                ["from-conn1", "msg-imm"],
            )
            .expect("conn1 UPDATE");
        // (conn1 has NOT committed yet — its UPDATE is buffered in
        // its tx.)

        // Spawn thread B: try to edit_message on conn2. Under
        // IMMEDIATE, B's BEGIN must wait. Use a channel so the test
        // observes the wait.
        let (tx, rx) = mpsc::channel::<Result<String, String>>();
        let path_b = db_path.clone();
        let h_b = thread::spawn(move || {
            let conn = Connection::open(&path_b).expect("open conn b");
            conn.busy_timeout(std::time::Duration::from_secs(5))
                .expect("conn2 busy_timeout");
            let result = edit_message(&conn, "msg-imm", "user-x", "from-thread-B")
                .map(|m| m.content)
                .map_err(|e| format!("{e:?}"));
            tx.send(result).expect("send result");
        });

        // Confirm thread B is BLOCKED inside edit_message (i.e. it
        // hasn't returned a result within 200ms). If B returned, it
        // either failed with SQLITE_BUSY_SNAPSHOT (DEFERRED bug) or
        // somehow bypassed conn1's lock (unlikely).
        let early = rx.recv_timeout(std::time::Duration::from_millis(200));
        assert!(
            early.is_err(),
            "thread B's edit_message should be blocked while conn1 holds IMMEDIATE; \
             got early result {early:?}"
        );

        // Commit conn1's tx. This releases the RESERVED lock and
        // makes "from-conn1" the visible content.
        conn1.execute_batch("COMMIT").expect("conn1 COMMIT");

        // Now thread B should unblock and complete.
        let res_b = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("thread B should complete after conn1 commits")
            .expect("thread B's edit_message must succeed under IMMEDIATE");
        assert_eq!(res_b, "from-thread-B", "thread B's UPDATE was applied");

        h_b.join().expect("thread B join");

        // Verify the audit trail. Under IMMEDIATE, B's BEGIN waited,
        // so when B's read finally ran it saw "from-conn1" — that's
        // what B saved to message_edits. Under DEFERRED (the bug)
        // B's read happened immediately and saw "Original", so
        // message_edits would have "Original" (and the snapshot
        // conflict at INSERT would either fail or — if the bug
        // happened to slip through SQLite's snapshot check —
        // produce "Original" in the history).
        let final_conn = Connection::open(&db_path).expect("open final conn");
        let history = get_edit_history(&final_conn, 1, "chan-imm", "msg-imm").expect("history");
        assert_eq!(history.len(), 1, "expected exactly 1 edit history row");
        assert_eq!(
            history[0].old_content, "from-conn1",
            "audit row must be the post-conn1 snapshot, not 'Original'; \
             a value of 'Original' here indicates a DEFERRED-tx regression"
        );
    }
}

#[cfg(test)]
mod deletion_tests {
    use super::*;
    use annex_db::run_migrations;
    use annex_types::ServerPolicy;
    use rusqlite::Connection;

    /// Migrations plus `PRAGMA foreign_keys = ON`, which is what the pool
    /// sets in production. Without the pragma neither of these defects is
    /// reachable, which is why they survived: the plain `Connection::open`
    /// used by other tests here does not enforce foreign keys.
    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run_migrations(&conn).unwrap();
        let policy = serde_json::to_string(&ServerPolicy::default()).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('t', 'T', ?1)",
            [policy],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channels (server_id, channel_id, name, channel_type, federation_scope)
             VALUES (1, 'chan', 'Chan', 'Text', 'LOCAL_ONLY')",
            [],
        )
        .unwrap();
        // `channel_members` has a composite FK to platform_identities, so a
        // membership row needs a real identity behind it.
        conn.execute(
            "INSERT INTO platform_identities
               (server_id, pseudonym_id, participant_type, active)
             VALUES (1, 'alice', 'HUMAN', 1)",
            [],
        )
        .unwrap();
        conn
    }

    fn edited_message(conn: &Connection, id: &str, expires_at: Option<&str>) {
        conn.execute(
            "INSERT INTO messages (server_id, channel_id, message_id, sender_pseudonym, content, expires_at)
             VALUES (1, 'chan', ?1, 'alice', 'final', ?2)",
            rusqlite::params![id, expires_at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message_edits (message_id, old_content) VALUES (?1, 'the original')",
            [id],
        )
        .unwrap();
    }

    /// The retention sweep deletes a BATCH in one statement, so a single
    /// expired message with edit history used to abort the whole thing with
    /// a foreign-key violation — and the same row was picked up again next
    /// sweep, so message retention stopped permanently the first time an
    /// edited message aged out. Nothing surfaced it: the sweep is a
    /// background task that logs at warn.
    #[test]
    fn retention_can_delete_a_message_that_was_edited() {
        let conn = setup();
        edited_message(&conn, "expired", Some("2000-01-01 00:00:00"));

        let deleted = delete_expired_messages(&conn).expect("the sweep must not abort");
        assert_eq!(deleted, 1, "the expired message was not deleted");

        let edits: i64 = conn
            .query_row("SELECT COUNT(*) FROM message_edits", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edits, 0, "orphaned edit history survived the message");
    }

    /// And one expired-but-edited message must not block the others.
    #[test]
    fn one_edited_message_does_not_block_the_whole_batch() {
        let conn = setup();
        edited_message(&conn, "expired-edited", Some("2000-01-01 00:00:00"));
        conn.execute(
            "INSERT INTO messages (server_id, channel_id, message_id, sender_pseudonym, content, expires_at)
             VALUES (1, 'chan', 'expired-plain', 'alice', 'x', '2000-01-01 00:00:00')",
            [],
        )
        .unwrap();

        let deleted = delete_expired_messages(&conn).expect("sweep");
        assert_eq!(deleted, 2, "the batch did not delete both expired messages");
    }

    /// Deleting a message has to take its drafts with it.
    ///
    /// Soft delete blanked `content` and left `message_edits` untouched, and
    /// `get_edit_history` did not filter on `deleted_at` — so a deleted
    /// message still served every earlier version. Someone who mistyped
    /// something sensitive, corrected it, then deleted the message had
    /// published the mistake and hidden only the correction.
    #[test]
    fn deleting_a_message_removes_its_earlier_versions() {
        let conn = setup();
        edited_message(&conn, "msg-1", None);

        delete_message(&conn, "msg-1", "alice").expect("delete");

        let history = get_edit_history(&conn, 1, "chan", "msg-1").expect("history");
        assert!(
            history.is_empty(),
            "the deleted message still serves its earlier versions: {history:?}",
        );
    }

    /// An encrypted channel must be deletable.
    ///
    /// `delete_channel` removes messages and members before the channel row,
    /// to satisfy the foreign keys. `channel_key_wraps` — added later, for
    /// E2E key distribution — references `channels(channel_id)` with no
    /// `ON DELETE` and was never added to that list, so deleting an
    /// encrypted channel raised a foreign-key violation and rolled the whole
    /// transaction back. A moderator could not delete an E2E channel at all,
    /// and the reason was invisible: the error surfaces as a generic
    /// database failure.
    ///
    /// Same shape as the retention sweep that `message_edits` blocked. Both
    /// were only reachable with `PRAGMA foreign_keys = ON`, which the pool
    /// sets and the other tests in this file do not.
    #[test]
    fn an_e2e_channel_can_be_deleted() {
        let conn = setup();
        conn.execute(
            "INSERT INTO messages (server_id, channel_id, message_id, sender_pseudonym, content)
             VALUES (1, 'chan', 'm1', 'alice', 'hi')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channel_members (channel_id, pseudonym_id, server_id)
             VALUES ('chan', 'alice', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channel_key_wraps
               (server_id, channel_id, recipient_pseudonym_id, sender_pseudonym_id, wrapped_key_b64)
             VALUES (1, 'chan', 'alice', 'bob', 'd3JhcHBlZA==')",
            [],
        )
        .unwrap();

        delete_channel(&conn, "chan").expect("an encrypted channel must be deletable");

        let wraps: i64 = conn
            .query_row("SELECT COUNT(*) FROM channel_key_wraps", [], |r| r.get(0))
            .unwrap();
        assert_eq!(wraps, 0, "key wraps outlived the channel they belong to");
    }

    /// An ordinary edit still keeps its history — the fix must not delete
    /// the feature along with the leak.
    #[test]
    fn editing_a_message_still_keeps_its_history() {
        let conn = setup();
        edited_message(&conn, "msg-1", None);

        let history = get_edit_history(&conn, 1, "chan", "msg-1").expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].old_content, "the original");
    }
}

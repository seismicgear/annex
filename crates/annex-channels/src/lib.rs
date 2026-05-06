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
pub use retention::delete_expired_messages;
pub use search::search_messages;
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
        let history = get_edit_history(&conn, &msg.message_id).expect("history should succeed");
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

        let history = get_edit_history(&conn, &msg.message_id).expect("history failed");
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].old_content, "Original content");
        assert_eq!(history[1].old_content, "Edit 1");
        assert_eq!(history[2].old_content, "Edit 2");

        let current = get_message(&conn, &msg.message_id).expect("get msg failed");
        assert_eq!(current.content, "Edit 3");
    }
}

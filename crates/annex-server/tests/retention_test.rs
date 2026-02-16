use annex_channels::{
    create_channel, create_message, get_message, ChannelError, CreateChannelParams,
    CreateMessageParams,
};
use annex_db::run_migrations;
use annex_server::retention::start_retention_task;
use annex_types::{AlignmentStatus, ChannelType, FederationScope, ServerPolicy};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_retention_task_deletes_expired_messages() {
    // 1. Setup DB
    let pool = annex_db::create_pool(
        ":memory:",
        annex_db::DbRuntimeSettings {
            busy_timeout_ms: 5000,
            pool_max_size: 1,
        },
    )
    .expect("failed to create pool");

    {
        let conn = pool.get().expect("failed to get connection");
        run_migrations(&conn).expect("failed to run migrations");

        // Create server
        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).expect("failed to serialize policy");
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test Server', ?1)",
            [policy_json],
        )
        .expect("failed to create server");

        // Create channel
        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-retention".to_string(),
            name: "Retention Test".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: Some(AlignmentStatus::Aligned),
            retention_days: Some(30),
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("failed to create channel");

        // Create message
        let msg_params = CreateMessageParams {
            channel_id: "chan-retention".to_string(),
            message_id: "msg-expired".to_string(),
            sender_pseudonym: "user1".to_string(),
            content: "I should be deleted".to_string(),
            reply_to_message_id: None,
        };
        create_message(&conn, &msg_params).expect("failed to create message");

        // Manually expire message (set expires_at to yesterday)
        conn.execute(
            "UPDATE messages SET expires_at = datetime('now', '-1 day') WHERE message_id = 'msg-expired'",
            [],
        )
        .expect("failed to expire message manually");
    }

    // 2. Start retention task in background
    // Interval 1 second
    let pool_clone = pool.clone();
    tokio::spawn(async move {
        start_retention_task(pool_clone, 1).await;
    });

    // 3. Wait for task to run (at least 1 second + buffer)
    sleep(Duration::from_millis(1500)).await;

    // 4. Verify message is gone
    let conn = pool.get().expect("failed to get connection");
    let result = get_message(&conn, "msg-expired");

    match result {
        Err(ChannelError::NotFound(_)) => {
            // Success
        }
        Ok(_) => {
            panic!("message should have been deleted");
        }
        Err(e) => {
            panic!("unexpected error: {}", e);
        }
    }
}

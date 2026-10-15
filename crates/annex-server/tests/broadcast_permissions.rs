//! Real database/service regression for the permission failure found by the
//! independent browser session. Browser acceptance separately checks its UI.
mod common;

use annex_channels::{add_member, create_channel, get_message, CreateChannelParams};
use annex_server::services::{channel_service::ChannelServiceError, ChannelService};
use annex_types::{ChannelType, FederationScope};
use std::sync::Arc;

#[tokio::test]
async fn broadcast_publication_and_edit_follow_current_moderator_capability() {
    let (_router, pool) = common::setup_test_app().await;
    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        Default::default(),
    ));
    let id = state.server_id;
    {
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,active,can_moderate) VALUES(?1,'publisher','HUMAN',1,0)", [id]).unwrap();
        for (channel_id, kind) in [
            ("announcements", ChannelType::Broadcast),
            ("ordinary", ChannelType::Text),
        ] {
            create_channel(
                &conn,
                &CreateChannelParams {
                    server_id: id,
                    channel_id: channel_id.into(),
                    name: channel_id.into(),
                    channel_type: kind,
                    topic: None,
                    vrp_topic_binding: None,
                    required_capabilities_json: None,
                    agent_min_alignment: None,
                    retention_days: None,
                    federation_scope: FederationScope::Local,
                },
            )
            .unwrap();
            add_member(&conn, id, channel_id, "publisher").unwrap();
        }
    }
    let cipher = state.message_cipher();
    let svc = ChannelService::new(state);
    let refused = svc
        .send_message(
            "publisher",
            "announcements",
            "forbidden".into(),
            None,
            Some("refused".into()),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(refused, ChannelServiceError::Forbidden(ref reason) if reason.contains("only moderators"))
    );
    {
        let conn = pool.get().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM message_request_ids", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM federation_outbox", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        conn.execute("UPDATE platform_identities SET can_moderate=1 WHERE server_id=?1 AND pseudonym_id='publisher'", [id]).unwrap();
    }
    let (message, _, _) = svc
        .send_message(
            "publisher",
            "announcements",
            "announcement".into(),
            None,
            Some("refused".into()),
        )
        .await
        .unwrap();
    svc.edit_message("publisher", "announcements", &message.message_id, "revised")
        .await
        .unwrap();
    pool.get().unwrap().execute("UPDATE platform_identities SET can_moderate=0 WHERE server_id=?1 AND pseudonym_id='publisher'", [id]).unwrap();
    assert!(matches!(
        svc.send_message(
            "publisher",
            "announcements",
            "after revocation".into(),
            None,
            None
        )
        .await,
        Err(ChannelServiceError::Forbidden(_))
    ));
    assert!(matches!(
        svc.edit_message(
            "publisher",
            "announcements",
            &message.message_id,
            "after revocation"
        )
        .await,
        Err(ChannelServiceError::Forbidden(_))
    ));
    let mut stored = get_message(&pool.get().unwrap(), &message.message_id).unwrap();
    cipher.decrypt_in_place(&mut stored.content).unwrap();
    assert_eq!(stored.content, "revised");
    // Ordinary chat is still available; revoking announcement publication does
    // not remove membership or deny deleting one's own prior announcement.
    svc.send_message("publisher", "ordinary", "ordinary chat".into(), None, None)
        .await
        .unwrap();
    svc.delete_message("publisher", "announcements", &message.message_id)
        .await
        .unwrap();
    assert!(get_message(&pool.get().unwrap(), &message.message_id)
        .unwrap()
        .deleted_at
        .is_some());
}

//! Bounded repository/filesystem fault tests. Live HTTP/browser evidence is a
//! separate gate: these fixtures do not claim a real participant session.
mod common;

use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{
    services::upload_service::{self, UploadMetadata},
    AppState,
};
use annex_types::ServerPolicy;
use rusqlite::{params, TransactionBehavior};
use std::path::Path;

struct Fixture {
    state: AppState,
    _dir: tempfile::TempDir,
}
fn fixture() -> Fixture {
    fixture_with_database(false)
}
fn fixture_with_database(disk: bool) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let policy = ServerPolicy::default();
    let database = if disk {
        directory
            .path()
            .join("test.db")
            .to_string_lossy()
            .into_owned()
    } else {
        ":memory:".into()
    };
    let pool = create_pool(&database, DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers(slug,label,policy_json) VALUES ('uploads','Uploads',?1)",
            [serde_json::to_string(&policy).unwrap()],
        )
        .unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_voice,can_moderate,can_invite,can_federate,can_bridge,active)
            VALUES (1,'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9','HUMAN',1,1,1,0,0,1),(1,'55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe','HUMAN',1,0,1,0,0,1)", []).unwrap();
        conn.execute("INSERT INTO channels(channel_id,server_id,name,channel_type,federation_scope) VALUES ('room',1,'Room',json_quote('Text'),json_quote('Local'))", []).unwrap();
        conn.execute("INSERT INTO channel_members(server_id,channel_id,pseudonym_id) VALUES (1,'room','a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'),(1,'room','55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe')", []).unwrap();
    }
    let mut state = common::build_app_state(pool, MerkleTree::new(20).unwrap(), policy);
    state.upload_dir = directory.path().to_string_lossy().into_owned();
    state.signing_key = std::sync::Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0x55; 32]));
    state.membership_vkey_v2 = Some(std::sync::Arc::new(
        annex_identity::zk::parse_verification_key(&certified_fixture()["vkey"].to_string())
            .unwrap(),
    ));
    Fixture {
        state,
        _dir: directory,
    }
}

async fn ordinary_prepared_request(f: &Fixture, fields: &str) -> axum::response::Response {
    use axum::{body::Body, extract::ConnectInfo, http::Request};
    use tower::ServiceExt;
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/channels/room/upload")
        .header("X-Annex-Pseudonym", metadata().uploader)
        .header(
            "Content-Type",
            "multipart/form-data; boundary=prepared-upload",
        )
        .body(Body::from(fields.to_owned()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:9130".parse::<std::net::SocketAddr>().unwrap(),
    ));
    annex_server::app(f.state.clone())
        .oneshot(request)
        .await
        .unwrap()
}
fn prepared_id_field(id: &str) -> String {
    format!("--prepared-upload\r\nContent-Disposition: form-data; name=\"attachment_id\"\r\n\r\n{id}\r\n")
}
fn ordinary_file_field() -> &'static str {
    "--prepared-upload\r\nContent-Disposition: form-data; name=\"file\"; filename=\"actual.txt\"\r\nContent-Type: application/octet-stream\r\n\r\nactual ordinary upload bytes\r\n"
}

#[tokio::test]
async fn ordinary_prepared_uuid_roundtrips_and_commit_before_ack_can_be_canceled() {
    use axum::{body::to_bytes, http::StatusCode};
    let f = fixture();
    let id = uuid::Uuid::new_v4().to_string();
    let body = format!(
        "{}{}--prepared-upload--\r\n",
        prepared_id_field(&id),
        ordinary_file_field()
    );
    let response = ordinary_prepared_request(&f, &body).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["upload_id"], id);
    let path: String = f
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT storage_path FROM uploads WHERE upload_id=?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        std::fs::read(Path::new(&f.state.upload_dir).join(&path)).unwrap(),
        b"actual ordinary upload bytes"
    );
    upload_service::abort_upload(&f.state, "room", &metadata().uploader, &id, 0).unwrap();
    assert_eq!(count(&f, "uploads"), 0);
    assert!(!Path::new(&f.state.upload_dir).join(path).exists());
    assert_eq!(
        ordinary_prepared_request(&f, &body).await.status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn ordinary_prepared_uuid_late_post_respects_owner_cancellation_without_writing_bytes() {
    use axum::http::StatusCode;
    let f = fixture();
    let upload = metadata();
    upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0).unwrap();
    let body = format!(
        "{}{}--prepared-upload--\r\n",
        prepared_id_field(&upload.upload_id),
        ordinary_file_field()
    );
    assert_eq!(
        ordinary_prepared_request(&f, &body).await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(count(&f, "uploads"), 0);
    assert_eq!(count(&f, "message_uploads"), 0);
    assert!(!uploaded_path(&f, &upload).exists());
}

#[tokio::test]
async fn ordinary_prepared_uuid_parser_refuses_invalid_duplicate_late_and_file_metadata_fields() {
    use axum::http::StatusCode;
    let f = fixture();
    let id = "abcdef01-1234-4567-89ab-cdef01234567".to_string();
    let idfield = prepared_id_field(&id);
    let file = ordinary_file_field();
    for fields in [
        format!("{}{file}",prepared_id_field("not-a-uuid")),
        format!("{}{file}",prepared_id_field(&id.to_uppercase())),
        format!("{idfield}{idfield}{file}"),
        format!("{file}{idfield}"),
        format!("--prepared-upload\r\nContent-Disposition: form-data; name=\"attachment_id\"; filename=\"id.txt\"\r\n\r\n{id}\r\n{file}"),
        idfield,
    ] {
        assert_eq!(ordinary_prepared_request(&f,&format!("{fields}--prepared-upload--\r\n")).await.status(),StatusCode::BAD_REQUEST);
        assert_eq!(count(&f,"uploads"),0);
    }
    // The longstanding single-file wire shape remains supported.
    assert_eq!(
        ordinary_prepared_request(&f, &format!("{file}--prepared-upload--\r\n"))
            .await
            .status(),
        StatusCode::OK
    );
}
fn metadata() -> UploadMetadata {
    UploadMetadata {
        upload_id: uuid::Uuid::new_v4().to_string(),
        channel_id: "room".into(),
        uploader: "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9".into(),
        token_epoch: 0,
        category: "file".into(),
        original_filename: "ordinary.txt".into(),
        content_type: "application/octet-stream".into(),
        extension: "bin".into(),
        encrypted_epoch: None,
    }
}

#[test]
fn cancellation_before_multipart_persistence_is_owner_bound_idempotent_and_blocks_late_commit() {
    let f = fixture();
    let upload = metadata();
    upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0).unwrap();
    upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0).unwrap();
    assert_eq!(count(&f, "upload_cancellations"), 1);
    assert_eq!(count(&f, "upload_ids_used"), 1);
    assert_eq!(count(&f, "uploads"), 0);
    assert!(matches!(
        upload_service::persist_chat_upload(&f.state, &upload, b"late network body"),
        Err(upload_service::UploadError::Conflict(_))
    ));
    let bob = "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe";
    assert!(matches!(
        upload_service::abort_upload(&f.state, "room", bob, &upload.upload_id, 0),
        Err(upload_service::UploadError::NotFound(_))
    ));
    assert!(!uploaded_path(&f, &upload).exists());
}

#[test]
fn cancellation_after_ready_commit_removes_real_bytes_and_cannot_delete_published_or_foreign_uploads(
) {
    let f = fixture();
    let upload = metadata();
    upload_service::persist_chat_upload(&f.state, &upload, b"remote commit before its HTTP ack")
        .unwrap();
    let bob = "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe";
    assert!(matches!(
        upload_service::abort_upload(&f.state, "room", bob, &upload.upload_id, 0),
        Err(upload_service::UploadError::NotFound(_))
    ));
    assert!(uploaded_path(&f, &upload).exists());
    upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0).unwrap();
    assert!(!uploaded_path(&f, &upload).exists());
    assert_eq!(count(&f, "uploads"), 0);
    assert_eq!(count(&f, "upload_deletion_queue"), 0);
    let published = metadata();
    upload_service::persist_chat_upload(&f.state, &published, b"published must remain").unwrap();
    message(&f, "published", &published.uploader);
    associate(
        &f,
        "published",
        &published.uploader,
        std::slice::from_ref(&published.upload_id),
    )
    .unwrap();
    assert!(matches!(
        upload_service::abort_upload(
            &f.state,
            "room",
            &published.uploader,
            &published.upload_id,
            0
        ),
        Err(upload_service::UploadError::Conflict(_))
    ));
    assert!(uploaded_path(&f, &published).exists());
    assert_eq!(count(&f, "upload_cancellations"), 1);
}

#[test]
fn cancellation_between_staging_and_finalization_prevents_rename_and_drains_files() {
    let f = fixture();
    let upload = metadata();
    // Deterministically enact the competing cancellation transaction's exact
    // committed state at the staging boundary. This is a component fault
    // regression; the browser lane separately races a real HTTP cancellation.
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER cancel_at_stage AFTER INSERT ON uploads BEGIN
        INSERT INTO upload_cancellations(server_id,upload_id,channel_id,uploader_pseudonym,was_unpersisted) VALUES(NEW.server_id,NEW.upload_id,NEW.channel_id,NEW.uploader_pseudonym,0);
        DELETE FROM uploads WHERE upload_id=NEW.upload_id;
        END;").unwrap();
    assert!(matches!(
        upload_service::persist_chat_upload(&f.state, &upload, b"competing canceled body"),
        Err(upload_service::UploadError::Conflict(_))
    ));
    assert_eq!(count(&f, "uploads"), 0);
    assert_eq!(count(&f, "upload_cancellations"), 1);
    assert_eq!(count(&f, "upload_deletion_queue"), 0);
    assert!(!uploaded_path(&f, &upload).exists());
    assert!(!Path::new(&f.state.upload_dir)
        .join(format!(".staging/{}.part", upload.upload_id))
        .exists());
}

#[test]
fn cancellation_reservation_survives_database_reopen_and_never_reuses_uuid() {
    let mut f = fixture_with_database(true);
    let upload = metadata();
    upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0).unwrap();
    let path = f._dir.path().join("test.db");
    let old_pool = std::mem::replace(
        &mut f.state.pool,
        create_pool(":memory:", DbRuntimeSettings::default()).unwrap(),
    );
    drop(old_pool);
    f.state.pool = create_pool(path.to_str().unwrap(), DbRuntimeSettings::default()).unwrap();
    assert_eq!(count(&f, "upload_cancellations"), 1);
    assert!(matches!(
        upload_service::persist_chat_upload(&f.state, &upload, b"after reopen"),
        Err(upload_service::UploadError::Conflict(_))
    ));
    upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0).unwrap();
}

#[test]
fn cancellation_requires_current_session_permission_canonical_id_and_writable_storage() {
    let f = fixture();
    let upload = metadata();
    assert!(matches!(
        upload_service::abort_upload(&f.state, "room", &upload.uploader, "../not-an-id", 0),
        Err(upload_service::UploadError::BadRequest(_))
    ));
    assert!(matches!(
        upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 1),
        Err(upload_service::UploadError::Unauthorized(_))
    ));
    f.state
        .storage_health
        .mark_degraded("controlled component gate");
    assert!(matches!(
        upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0),
        Err(upload_service::UploadError::Unavailable(_))
    ));
    f.state.storage_health.mark_healthy();
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "DELETE FROM channel_members WHERE pseudonym_id=?1",
            [&upload.uploader],
        )
        .unwrap();
    assert!(matches!(
        upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0),
        Err(upload_service::UploadError::Forbidden(_))
    ));
    assert_eq!(count(&f, "upload_cancellations"), 0);
    assert_eq!(count(&f, "upload_ids_used"), 0);
}

#[test]
fn cancellation_budget_bounds_unknown_ids_but_keeps_known_cleanup_and_idempotence_available() {
    let f = fixture();
    let upload = metadata();
    let mut first = String::new();
    for index in 0..upload_service::MAX_UNKNOWN_CANCELLATIONS_PER_MEMBER_DAY {
        let id = uuid::Uuid::new_v4().to_string();
        if index == 0 {
            first = id.clone();
        }
        upload_service::abort_upload(&f.state, "room", &upload.uploader, &id, 0).unwrap();
    }
    assert!(matches!(
        upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0),
        Err(upload_service::UploadError::Unavailable(_))
    ));
    upload_service::abort_upload(&f.state, "room", &upload.uploader, &first, 0).unwrap();
    upload_service::persist_chat_upload(
        &f.state,
        &upload,
        b"known cleanup is exempt from speculative budget",
    )
    .unwrap();
    upload_service::abort_upload(&f.state, "room", &upload.uploader, &upload.upload_id, 0).unwrap();
    assert!(!uploaded_path(&f, &upload).exists());
    assert_eq!(
        count(&f, "upload_cancellations"),
        upload_service::MAX_UNKNOWN_CANCELLATIONS_PER_MEMBER_DAY + 1
    );
}
fn count(f: &Fixture, table: &str) -> i64 {
    f.state
        .pool
        .get()
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}
fn message(f: &Fixture, id: &str, owner: &str) {
    f.state.pool.get().unwrap().execute("INSERT INTO messages(server_id,channel_id,message_id,sender_pseudonym,content) VALUES (1,'room',?1,?2,'body')",
        params![id,owner]).unwrap();
}
fn associate(
    f: &Fixture,
    id: &str,
    owner: &str,
    uploads: &[String],
) -> Result<(), upload_service::UploadError> {
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    upload_service::associate_message_uploads(&tx, &f.state, "room", owner, id, uploads, None)?;
    tx.commit()?;
    Ok(())
}
fn uploaded_path(f: &Fixture, metadata: &UploadMetadata) -> std::path::PathBuf {
    Path::new(&f.state.upload_dir).join(format!("chat/files/{}.bin", metadata.upload_id))
}

#[test]
fn actual_written_bytes_and_metadata_only_become_ready_together() {
    let f = fixture();
    let upload = metadata();
    let bytes = b"actual ordinary attachment bytes";
    let (url, digest) = upload_service::persist_chat_upload(&f.state, &upload, bytes).unwrap();
    assert_eq!(url, format!("/uploads/chat/files/{}.bin", upload.upload_id));
    assert_eq!(std::fs::read(uploaded_path(&f, &upload)).unwrap(), bytes);
    assert_eq!(digest.len(), 64);
    let conn = f.state.pool.get().unwrap();
    let (lifecycle, stage, hash): (String, Option<String>, String) = conn
        .query_row(
            "SELECT lifecycle,staging_path,ciphertext_sha256 FROM uploads WHERE upload_id = ?1",
            [&upload.upload_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(lifecycle, "ready");
    assert!(stage.is_none());
    assert_eq!(hash, digest);
}

#[test]
fn database_failure_after_rename_removes_bytes_and_retains_uuid_tombstone() {
    let f = fixture();
    let upload = metadata();
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER reject_ready BEFORE UPDATE OF lifecycle ON uploads WHEN NEW.lifecycle = 'ready'
        BEGIN SELECT RAISE(ABORT,'injected final database write failure'); END;").unwrap();
    assert!(upload_service::persist_chat_upload(&f.state, &upload, b"must not leak").is_err());
    assert!(!uploaded_path(&f, &upload).exists());
    assert!(!Path::new(&f.state.upload_dir)
        .join(format!(".staging/{}.part", upload.upload_id))
        .exists());
    assert_eq!(count(&f, "uploads"), 0);
    assert_eq!(count(&f, "upload_deletion_queue"), 0);
    assert_eq!(count(&f, "upload_ids_used"), 1);
}

#[test]
fn membership_revocation_between_stage_and_finalize_refuses_and_cleans_file() {
    let f = fixture();
    let upload = metadata();
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER revoke_at_stage AFTER INSERT ON uploads
        BEGIN DELETE FROM channel_members WHERE channel_id = NEW.channel_id AND pseudonym_id = NEW.uploader_pseudonym; END;").unwrap();
    let result = upload_service::persist_chat_upload(&f.state, &upload, b"revoked upload");
    assert!(matches!(
        result,
        Err(upload_service::UploadError::Forbidden(_))
    ));
    assert!(!uploaded_path(&f, &upload).exists());
    assert_eq!(count(&f, "uploads"), 0);
}

#[test]
fn token_revocation_between_stage_and_finalize_refuses_and_cleans_file() {
    let f = fixture();
    let upload = metadata();
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER revoke_at_stage AFTER INSERT ON uploads
        BEGIN UPDATE platform_identities SET token_epoch = token_epoch + 1 WHERE pseudonym_id = NEW.uploader_pseudonym; END;").unwrap();
    let result = upload_service::persist_chat_upload(&f.state, &upload, b"revoked session");
    assert!(matches!(
        result,
        Err(upload_service::UploadError::Unauthorized(_))
    ));
    assert!(!uploaded_path(&f, &upload).exists());
    assert_eq!(count(&f, "uploads"), 0);
}

#[test]
fn policy_change_between_stage_and_finalize_is_read_from_current_database() {
    let f = fixture();
    let upload = metadata();
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER disable_at_stage AFTER INSERT ON uploads
        BEGIN UPDATE servers SET policy_json = json_set(policy_json,'$.files_enabled',json('false')) WHERE id = NEW.server_id; END;").unwrap();
    assert!(upload_service::persist_chat_upload(&f.state, &upload, b"disabled category").is_err());
    assert!(!uploaded_path(&f, &upload).exists());
    assert_eq!(count(&f, "uploads"), 0);
}

#[test]
fn ordinary_broadcast_member_cannot_stage_attachment() {
    let f = fixture();
    let mut upload = metadata();
    upload.uploader = "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe".into();
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE channels SET channel_type = json_quote('Broadcast') WHERE channel_id = 'room'",
            [],
        )
        .unwrap();
    assert!(matches!(
        upload_service::persist_chat_upload(&f.state, &upload, b"announcement"),
        Err(upload_service::UploadError::Forbidden(_))
    ));
    assert_eq!(count(&f, "uploads"), 0);
    assert_eq!(count(&f, "upload_ids_used"), 0);
}

#[test]
fn association_rolls_back_entire_batch_when_one_attachment_is_not_owned() {
    let f = fixture();
    let alice = metadata();
    let mut bob = metadata();
    bob.uploader = "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe".into();
    upload_service::persist_chat_upload(
        &f.state,
        &alice,
        b"a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    )
    .unwrap();
    upload_service::persist_chat_upload(
        &f.state,
        &bob,
        b"55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe",
    )
    .unwrap();
    message(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    assert!(associate(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        &[alice.upload_id.clone(), bob.upload_id]
    )
    .is_err());
    assert_eq!(count(&f, "message_uploads"), 0);
    assert!(uploaded_path(&f, &alice).exists());
}

#[test]
fn soft_delete_queues_physical_cleanup_and_restart_drain_removes_exact_file() {
    let f = fixture();
    let upload = metadata();
    upload_service::persist_chat_upload(&f.state, &upload, b"private attachment").unwrap();
    message(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    associate(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        std::slice::from_ref(&upload.upload_id),
    )
    .unwrap();
    assert!(upload_service::abort_upload(
        &f.state,
        "room",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        &upload.upload_id,
        0
    )
    .is_err());
    f.state.pool.get().unwrap().execute("UPDATE messages SET content = '', deleted_at = datetime('now') WHERE message_id = 'm1'",[]).unwrap();
    assert_eq!(count(&f, "uploads"), 0);
    assert_eq!(count(&f, "message_uploads"), 0);
    assert!(uploaded_path(&f, &upload).exists());
    assert_eq!(count(&f, "upload_deletion_queue"), 1);
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(!uploaded_path(&f, &upload).exists());
    assert_eq!(count(&f, "upload_deletion_queue"), 0);
    assert!(matches!(
        upload_service::persist_chat_upload(&f.state, &upload, b"same old id"),
        Err(upload_service::UploadError::Conflict(_))
    ));
}

#[test]
fn retention_hard_delete_and_channel_delete_have_the_same_cleanup_obligation() {
    for channel_delete in [false, true] {
        let f = fixture();
        let upload = metadata();
        upload_service::persist_chat_upload(&f.state, &upload, b"expires").unwrap();
        message(
            &f,
            "m1",
            "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        );
        associate(
            &f,
            "m1",
            "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
            std::slice::from_ref(&upload.upload_id),
        )
        .unwrap();
        if channel_delete {
            annex_channels::delete_channel(&f.state.pool.get().unwrap(), "room").unwrap();
        } else {
            f.state
                .pool
                .get()
                .unwrap()
                .execute("DELETE FROM messages WHERE message_id = 'm1'", [])
                .unwrap();
        }
        assert_eq!(count(&f, "uploads"), 0);
        assert_eq!(count(&f, "message_uploads"), 0);
        assert_eq!(count(&f, "upload_deletion_queue"), 1);
        upload_service::drain_deletion_queue(&f.state).unwrap();
        assert!(!uploaded_path(&f, &upload).exists());
    }
}

#[test]
fn replacing_attachment_refs_removes_only_no_longer_referenced_uploads() {
    let f = fixture();
    let first = metadata();
    let second = metadata();
    upload_service::persist_chat_upload(&f.state, &first, b"first").unwrap();
    upload_service::persist_chat_upload(&f.state, &second, b"second").unwrap();
    message(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    associate(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        std::slice::from_ref(&first.upload_id),
    )
    .unwrap();
    associate(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        std::slice::from_ref(&second.upload_id),
    )
    .unwrap();
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(!uploaded_path(&f, &first).exists());
    assert!(uploaded_path(&f, &second).exists());
    assert_eq!(count(&f, "message_uploads"), 1);
}

#[test]
fn abandoned_uploads_expire_but_referenced_files_do_not() {
    let f = fixture();
    let abandoned = metadata();
    let used = metadata();
    upload_service::persist_chat_upload(&f.state, &abandoned, b"unused").unwrap();
    upload_service::persist_chat_upload(&f.state, &used, b"used").unwrap();
    message(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    associate(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        std::slice::from_ref(&used.upload_id),
    )
    .unwrap();
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE uploads SET created_at = datetime('now','-2 days')",
            [],
        )
        .unwrap();
    upload_service::expire_abandoned_uploads(&f.state).unwrap();
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(!uploaded_path(&f, &abandoned).exists());
    assert!(uploaded_path(&f, &used).exists());
}

#[test]
fn cleanup_refuses_paths_that_leave_root_and_keeps_a_retryable_failure() {
    let f = fixture();
    f.state.pool.get().unwrap().execute("INSERT INTO upload_deletion_queue(server_id,relative_path) VALUES (1,'../outside.txt')",[]).unwrap();
    assert_eq!(upload_service::drain_deletion_queue(&f.state).unwrap(), 0);
    let conn = f.state.pool.get().unwrap();
    let attempts: i64 = conn
        .query_row("SELECT attempts FROM upload_deletion_queue", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(attempts, 1);
}

#[test]
fn expired_message_bytes_are_unreadable_to_owner_and_member_before_physical_sweep() {
    let f = fixture();
    let upload = metadata();
    let bytes = b"retention-gated real disk bytes";
    upload_service::persist_chat_upload(&f.state, &upload, bytes).unwrap();
    message(&f, "expired-read", &upload.uploader);
    associate(
        &f,
        "expired-read",
        &upload.uploader,
        std::slice::from_ref(&upload.upload_id),
    )
    .unwrap();
    let relative = format!("chat/files/{}.bin", upload.upload_id);
    for reader in [
        upload.uploader.as_str(),
        "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe",
    ] {
        assert_eq!(
            upload_service::read_upload_for_grant(
                &f.state,
                &relative,
                &upload.upload_id,
                reader,
                0
            )
            .unwrap()
            .0,
            bytes
        );
    }
    for expiry in ["2000-01-01 00:00:00", "not-a-valid-expiration"] {
        f.state
            .pool
            .get()
            .unwrap()
            .execute(
                "UPDATE messages SET expires_at=?1 WHERE message_id='expired-read'",
                [expiry],
            )
            .unwrap();
        assert!(uploaded_path(&f, &upload).exists());
        assert_eq!(count(&f, "message_uploads"), 1);
        assert_eq!(count(&f, "upload_deletion_queue"), 0);
        for reader in [
            upload.uploader.as_str(),
            "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe",
        ] {
            assert!(matches!(
                upload_service::read_upload_for_grant(
                    &f.state,
                    &relative,
                    &upload.upload_id,
                    reader,
                    0
                ),
                Err(upload_service::UploadError::NotFound(_))
            ));
        }
    }
}

#[test]
fn legacy_backfill_keeps_expired_association_so_owner_preview_cannot_resurrect_it() {
    let f = fixture();
    let upload = metadata();
    upload_service::persist_chat_upload(&f.state, &upload, b"legacy expired").unwrap();
    let relative = format!("chat/files/{}.bin", upload.upload_id);
    message(&f, "legacy-expired", &upload.uploader);
    let conn = f.state.pool.get().unwrap();
    conn.execute("UPDATE messages SET content=?1,expires_at='2000-01-01 00:00:00' WHERE message_id='legacy-expired'",[format!("/uploads/{relative}")]).unwrap();
    conn.execute(
        "UPDATE uploads SET lifecycle='legacy' WHERE upload_id=?1",
        [&upload.upload_id],
    )
    .unwrap();
    drop(conn);
    upload_service::backfill_legacy_associations(&f.state, |body| Ok(body.to_owned())).unwrap();
    assert_eq!(count(&f, "message_uploads"), 1);
    assert!(uploaded_path(&f, &upload).exists());
    assert!(matches!(
        upload_service::read_upload_for_grant(
            &f.state,
            &relative,
            &upload.upload_id,
            &upload.uploader,
            0
        ),
        Err(upload_service::UploadError::NotFound(_))
    ));
}

#[test]
fn download_requires_exact_path_publication_membership_and_current_token_epoch() {
    let f = fixture();
    let upload = metadata();
    upload_service::persist_chat_upload(&f.state, &upload, b"authorized bytes").unwrap();
    let path = format!("chat/files/{}.bin", upload.upload_id);
    assert_eq!(
        upload_service::read_upload_for_grant(
            &f.state,
            &path,
            &upload.upload_id,
            "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
            0
        )
        .unwrap()
        .0,
        b"authorized bytes"
    );
    assert!(matches!(
        upload_service::read_upload_for_grant(
            &f.state,
            &path,
            &upload.upload_id,
            "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe",
            0
        ),
        Err(upload_service::UploadError::NotFound(_))
    ));
    message(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    associate(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        std::slice::from_ref(&upload.upload_id),
    )
    .unwrap();
    assert_eq!(
        upload_service::read_upload_for_grant(
            &f.state,
            &path,
            &upload.upload_id,
            "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe",
            0
        )
        .unwrap()
        .0,
        b"authorized bytes"
    );
    assert!(matches!(
        upload_service::read_upload_for_grant(
            &f.state,
            &path.replace("files", "images"),
            &upload.upload_id,
            "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe",
            0
        ),
        Err(upload_service::UploadError::NotFound(_))
    ));
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE platform_identities SET token_epoch = 1 WHERE pseudonym_id = '55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe'",
            [],
        )
        .unwrap();
    assert!(matches!(
        upload_service::read_upload_for_grant(
            &f.state,
            &path,
            &upload.upload_id,
            "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe",
            0
        ),
        Err(upload_service::UploadError::Unauthorized(_))
    ));
    f.state
        .pool
        .get()
        .unwrap()
        .execute("DELETE FROM channel_members WHERE pseudonym_id = '55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe'", [])
        .unwrap();
    assert!(matches!(
        upload_service::read_upload_for_grant(
            &f.state,
            &path,
            &upload.upload_id,
            "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe",
            1
        ),
        Err(upload_service::UploadError::NotFound(_))
    ));
}

#[test]
fn disk_corruption_is_never_served_as_a_successful_attachment() {
    let f = fixture();
    let upload = metadata();
    upload_service::persist_chat_upload(&f.state, &upload, b"authentic").unwrap();
    std::fs::write(uploaded_path(&f, &upload), b"corrupted").unwrap();
    let path = format!("chat/files/{}.bin", upload.upload_id);
    assert!(matches!(
        upload_service::read_upload_for_grant(
            &f.state,
            &path,
            &upload.upload_id,
            "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
            0
        ),
        Err(upload_service::UploadError::Internal(_))
    ));
}

#[test]
fn legacy_reference_backfill_preserves_published_bytes_and_makes_deletion_effective() {
    let f = fixture();
    let upload = metadata();
    let (url, _) = upload_service::persist_chat_upload(&f.state, &upload, b"legacy image").unwrap();
    message(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    {
        let conn = f.state.pool.get().unwrap();
        conn.execute("UPDATE uploads SET lifecycle = 'legacy'", [])
            .unwrap();
        conn.execute(
            "UPDATE messages SET content = ?1 WHERE message_id = 'm1'",
            [format!("[old attachment]({url})")],
        )
        .unwrap();
    }
    upload_service::backfill_legacy_associations(&f.state, |body| Ok(body.to_string())).unwrap();
    assert_eq!(count(&f, "message_uploads"), 1);
    assert!(uploaded_path(&f, &upload).exists());
    f.state
        .pool
        .get()
        .unwrap()
        .execute("DELETE FROM messages WHERE message_id = 'm1'", [])
        .unwrap();
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(!uploaded_path(&f, &upload).exists());
}

#[test]
fn legacy_decrypt_failure_rolls_back_partial_backfill_and_never_expires_legacy_bytes() {
    let f = fixture();
    let upload = metadata();
    let (url, _) =
        upload_service::persist_chat_upload(&f.state, &upload, b"legacy attachment").unwrap();
    message(
        &f,
        "m1",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    message(
        &f,
        "m2",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    {
        let conn = f.state.pool.get().unwrap();
        conn.execute(
            "UPDATE uploads SET lifecycle = 'legacy',created_at = datetime('now','-2 days')",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE messages SET content = ?1 WHERE message_id = 'm1'",
            [url],
        )
        .unwrap();
        conn.execute(
            "UPDATE messages SET content = 'injected decrypt refusal' WHERE message_id = 'm2'",
            [],
        )
        .unwrap();
    }
    let result = upload_service::backfill_legacy_associations(&f.state, |body| {
        if body == "injected decrypt refusal" {
            Err(upload_service::UploadError::Internal(
                "decrypt rejected stored ciphertext".into(),
            ))
        } else {
            Ok(body.to_string())
        }
    });
    assert!(result.is_err());
    assert_eq!(count(&f, "message_uploads"), 0);
    upload_service::expire_abandoned_uploads(&f.state).unwrap();
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(uploaded_path(&f, &upload).exists());
    assert_eq!(count(&f, "uploads"), 1);
}

fn certified_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/attachment-certified-public.json")).unwrap()
}

// These bounded storage tests seed verified public admission state. The exact
// certificates and signed room lineage were generated by real registration;
// no identity, device, channel, signature or authority is relabelled here.
fn enable_fixture_epoch(f: &Fixture, epoch: i64, key: &str) {
    use annex_server::{
        device_certificates,
        services::channel_manifest::{self, SignedManifestEvent},
    };
    let conn = f.state.pool.get().unwrap();
    let public = certified_fixture();
    for value in public["certificates"].as_array().unwrap() {
        let cert: device_certificates::DeviceCertificate =
            serde_json::from_value(value.clone()).unwrap();
        device_certificates::verify_certificate(
            &cert,
            &cert.server_identity,
            &cert.pseudonym_id,
            f.state.membership_vkey_v2.as_ref().unwrap(),
        )
        .unwrap();
        conn.execute("INSERT OR IGNORE INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex,device_certificate,certificate_sha256) VALUES(1,?1,?2,?3,?4,?5)",params![cert.pseudonym_id,cert.device_id,cert.x25519_pub_hex,serde_json::to_string(&cert).unwrap(),device_certificates::certificate_digest(&cert)]).unwrap();
    }
    if channel_manifest::projection(&conn, &f.state, "room")
        .unwrap()
        .is_none()
    {
        let mut projection = None;
        for value in public["channels"]["room"].as_array().unwrap() {
            let event: SignedManifestEvent = serde_json::from_value(value.clone()).unwrap();
            let next = channel_manifest::verify_transition(
                projection.as_ref(),
                &event,
                f.state.membership_vkey_v2.as_ref().unwrap(),
            )
            .unwrap();
            channel_manifest::persist(&conn, &f.state, "room", &event, &next).unwrap();
            projection = Some(next);
        }
    }
    let manifest = channel_manifest::projection(&conn, &f.state, "room")
        .unwrap()
        .unwrap();
    assert!(manifest.e2e_enabled);
    let cert = &public["certificates"][0];
    conn.execute(
        "UPDATE channels SET e2e_enabled=1 WHERE channel_id='room'",
        [],
    )
    .unwrap();
    conn.execute("UPDATE channel_key_state SET current_epoch=?1,required_epoch=?1,current_key_id=?2 WHERE channel_id='room'",params![epoch,key]).unwrap();
    // The epoch is controlled component state. Signature admission/CAS is
    // separately exercised through the actual key authorization endpoint.
    conn.execute("INSERT INTO channel_key_epochs(server_id,channel_id,epoch,key_id,committed_revision,committed_by_pseudonym,committed_by_device,manifest_hash,authority_namespace) VALUES(1,'room',?1,?2,?1,?3,?4,?5,?6)",params![epoch,key,cert["pseudonym_id"].as_str().unwrap(),cert["device_id"].as_str().unwrap(),manifest.head_hash,manifest.authority_identity]).unwrap();
}

fn fixture_read_device(f: &Fixture, key: &str) -> String {
    let public = certified_fixture();
    let cert = &public["certificates"][0];
    let device = cert["device_id"].as_str().unwrap().to_string();
    let pid = cert["pseudonym_id"].as_str().unwrap();
    let conn = f.state.pool.get().unwrap();
    conn.execute("INSERT INTO channel_device_key_wraps(server_id,channel_id,recipient_pseudonym_id,recipient_device_id,epoch,key_id,sender_pseudonym_id,sender_device_id,wrapped_key_b64) VALUES(1,'room',?1,?2,1,?3,?1,?2,'controlled-test-wrap')",params![pid,device,key]).unwrap();
    device
}

#[test]
fn actual_aead_ciphertext_round_trip_keeps_filename_and_plaintext_out_of_storage() {
    use chacha20poly1305::{
        aead::{Aead, Payload},
        ChaCha20Poly1305, KeyInit, Nonce,
    };
    let f = fixture();
    let channel_key_id = "ab".repeat(32);
    enable_fixture_epoch(&f, 1, &channel_key_id);
    let device = fixture_read_device(&f, &channel_key_id);
    let mut upload = metadata();
    upload.original_filename = "encrypted.bin".into();
    upload.encrypted_epoch = Some((1, channel_key_id.clone()));
    let filename = "private-plans.txt";
    let plaintext = b"An actual authenticated attachment with confidential content.";
    let key = [0x53; 32];
    let nonce = [0x19; 12];
    let cipher = ChaCha20Poly1305::new((&key).into());
    let aad = serde_json::to_vec(&serde_json::json!([
        "annex-attachment-v1",
        "fixture-authority",
        "room",
        upload.upload_id,
        1,
        channel_key_id
    ]))
    .unwrap();
    let mut ciphertext = nonce.to_vec();
    ciphertext.extend(
        cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .unwrap(),
    );
    upload_service::persist_chat_upload(&f.state, &upload, &ciphertext).unwrap();
    let stored = std::fs::read(uploaded_path(&f, &upload)).unwrap();
    assert_eq!(stored, ciphertext);
    assert!(!stored
        .windows(plaintext.len())
        .any(|window| window == plaintext));
    assert!(!stored
        .windows(filename.len())
        .any(|window| window == filename.as_bytes()));
    let row: (String, String, i64, String) = f
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT original_filename,content_type,epoch,key_id FROM uploads WHERE upload_id = ?1",
            [&upload.upload_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "encrypted.bin".into(),
            "application/octet-stream".into(),
            1,
            channel_key_id
        )
    );
    assert!(upload_service::read_upload_for_grant(
        &f.state,
        &format!("chat/files/{}.bin", upload.upload_id),
        &upload.upload_id,
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        0
    )
    .is_err());
    let received = upload_service::read_upload_for_device_grant(
        &f.state,
        &format!("chat/files/{}.bin", upload.upload_id),
        &upload.upload_id,
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        0,
        Some(&device),
    )
    .unwrap();
    assert_eq!(received.1, "application/octet-stream");
    assert_eq!(
        cipher
            .decrypt(
                Nonce::from_slice(&received.0[..12]),
                Payload {
                    msg: &received.0[12..],
                    aad: &aad
                }
            )
            .unwrap(),
        plaintext
    );
    assert!(cipher
        .decrypt(
            Nonce::from_slice(&received.0[..12]),
            Payload {
                msg: &received.0[12..],
                aad: b"wrong attachment context"
            }
        )
        .is_err());
}

#[test]
fn encrypted_bytes_require_current_owned_device_and_exact_epoch_wrap() {
    for mutation in [
        "UPDATE member_device_keys SET revoked_at=datetime('now') WHERE pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "DELETE FROM channel_device_key_wraps WHERE recipient_pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "UPDATE channel_device_key_wraps SET key_id='wrong-key' WHERE recipient_pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "UPDATE platform_identities SET token_epoch=1 WHERE pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
    ] {
        let f=fixture();
        let key="ab".repeat(32);
        enable_fixture_epoch(&f,1,&key);
        let device=fixture_read_device(&f,&key);
        let mut upload=metadata();
        upload.original_filename="encrypted.bin".into();
        upload.encrypted_epoch=Some((1,key));
        upload_service::persist_chat_upload(&f.state,&upload,&[0x83;64]).unwrap();
        let path=format!("chat/files/{}.bin",upload.upload_id);
        upload_service::read_upload_for_device_grant(&f.state,&path,&upload.upload_id,"a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",0,Some(&device)).unwrap();
        assert!(upload_service::read_upload_for_device_grant(&f.state,&path,&upload.upload_id,"55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe",0,Some(&device)).is_err());
        let conn=f.state.pool.get().unwrap();
        // A different real epoch/key is a valid row yet must not authorize
        // this file. Keep foreign-key validation enabled in the fixture.
        if mutation.contains("wrong-key") {
            conn.execute("INSERT INTO channel_key_epochs(server_id,channel_id,epoch,key_id,committed_revision,committed_by_pseudonym,committed_by_device) VALUES(1,'room',2,'wrong-key',2,'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9',?1)",[&device]).unwrap();
            conn.execute("UPDATE channel_device_key_wraps SET epoch=2,key_id='wrong-key' WHERE recipient_pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",[]).unwrap();
        } else {conn.execute(mutation,[]).unwrap();}
        drop(conn);
        assert!(upload_service::read_upload_for_device_grant(&f.state,&path,&upload.upload_id,"a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",0,Some(&device)).is_err(),"must refuse {mutation}");
    }
}

#[test]
fn encrypted_upload_refuses_plain_metadata_stale_keys_and_mid_finalize_rotation() {
    let f = fixture();
    let key = "ab".repeat(32);
    enable_fixture_epoch(&f, 1, &key);
    let mut upload = metadata();
    upload.encrypted_epoch = Some((1, key.clone()));
    assert!(matches!(
        upload_service::persist_chat_upload(&f.state, &upload, &[5; 40]),
        Err(upload_service::UploadError::BadRequest(_))
    ));
    upload.original_filename = "encrypted.bin".into();
    upload.encrypted_epoch = Some((2, key.clone()));
    assert!(matches!(
        upload_service::persist_chat_upload(&f.state, &upload, &[5; 40]),
        Err(upload_service::UploadError::Conflict(_))
    ));
    upload.encrypted_epoch = Some((1, key));
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER rotate_during_staging AFTER INSERT ON uploads WHEN NEW.lifecycle = 'staging' BEGIN UPDATE channel_key_state SET required_epoch = current_epoch + 1 WHERE channel_id = NEW.channel_id; END;").unwrap();
    assert!(matches!(
        upload_service::persist_chat_upload(&f.state, &upload, &[5; 40]),
        Err(upload_service::UploadError::Conflict(_))
    ));
    assert_eq!(count(&f, "uploads"), 0);
    assert!(!uploaded_path(&f, &upload).exists());
    assert!(!Path::new(&f.state.upload_dir)
        .join(format!(".staging/{}.part", upload.upload_id))
        .exists());
}

#[test]
fn historical_attachment_is_retained_only_by_its_existing_message_after_rotation() {
    let f = fixture();
    let old_key = "ab".repeat(32);
    let new_key = "cd".repeat(32);
    enable_fixture_epoch(&f, 1, &old_key);
    let mut upload = metadata();
    upload.original_filename = "encrypted.bin".into();
    upload.encrypted_epoch = Some((1, old_key.clone()));
    upload_service::persist_chat_upload(&f.state, &upload, &[5; 40]).unwrap();
    message(
        &f,
        "original",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    message(
        &f,
        "other",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    {
        let mut conn = f.state.pool.get().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        upload_service::associate_message_uploads(
            &tx,
            &f.state,
            "room",
            "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
            "original",
            std::slice::from_ref(&upload.upload_id),
            Some((1, &old_key)),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    enable_fixture_epoch(&f, 2, &new_key);
    {
        let mut conn = f.state.pool.get().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        upload_service::associate_message_uploads(
            &tx,
            &f.state,
            "room",
            "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
            "original",
            std::slice::from_ref(&upload.upload_id),
            Some((2, &new_key)),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    {
        let mut conn = f.state.pool.get().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(matches!(
            upload_service::associate_message_uploads(
                &tx,
                &f.state,
                "room",
                "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
                "other",
                std::slice::from_ref(&upload.upload_id),
                Some((2, &new_key))
            ),
            Err(upload_service::UploadError::Conflict(_))
        ));
    }
    assert_eq!(count(&f, "message_uploads"), 1);
    assert!(uploaded_path(&f, &upload).exists());
}

fn branding_metadata() -> UploadMetadata {
    let mut upload = metadata();
    upload.category = "image".into();
    upload.content_type = "image/png".into();
    upload.extension = "png".into();
    upload
}

#[test]
fn replacing_server_branding_retires_old_url_and_durably_removes_old_bytes() {
    let f = fixture();
    let first = branding_metadata();
    let second = branding_metadata();
    let (first_url, _) =
        upload_service::persist_server_image(&f.state, &first, b"first detected image").unwrap();
    assert_eq!(
        upload_service::read_public_server_image(&f.state, first_url.rsplit('/').next().unwrap())
            .unwrap()
            .0,
        b"first detected image"
    );
    let (second_url, _) =
        upload_service::persist_server_image(&f.state, &second, b"second detected image").unwrap();
    let actual: String = f
        .state
        .pool
        .get()
        .unwrap()
        .query_row("SELECT image_url FROM servers WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(actual, second_url);
    assert_eq!(count(&f, "uploads"), 1);
    assert!(matches!(
        upload_service::read_public_server_image(&f.state, first_url.rsplit('/').next().unwrap()),
        Err(upload_service::UploadError::NotFound(_))
    ));
    let old_path = Path::new(&f.state.upload_dir).join(first_url.trim_start_matches("/uploads/"));
    assert!(old_path.exists());
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(!old_path.exists());
    assert_eq!(
        upload_service::read_public_server_image(&f.state, second_url.rsplit('/').next().unwrap())
            .unwrap()
            .0,
        b"second detected image"
    );
}

#[test]
fn failed_server_image_finalization_keeps_previous_url_and_does_not_expose_orphan() {
    let f = fixture();
    let previous = branding_metadata();
    let failing = branding_metadata();
    let (previous_url, _) =
        upload_service::persist_server_image(&f.state, &previous, b"previous detected image")
            .unwrap();
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER fail_branding_commit BEFORE UPDATE OF image_url ON servers BEGIN SELECT RAISE(ABORT,'controlled image URL commit failure'); END;").unwrap();
    assert!(upload_service::persist_server_image(
        &f.state,
        &failing,
        b"uncommitted detected image"
    )
    .is_err());
    let actual: String = f
        .state
        .pool
        .get()
        .unwrap()
        .query_row("SELECT image_url FROM servers WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(actual, previous_url);
    assert_eq!(count(&f, "uploads"), 1);
    assert!(matches!(
        upload_service::read_public_server_image(
            &f.state,
            &format!("server_{}.png", failing.upload_id)
        ),
        Err(upload_service::UploadError::NotFound(_))
    ));
    assert!(!Path::new(&f.state.upload_dir)
        .join(format!("server/server_{}.png", failing.upload_id))
        .exists());
}

#[test]
fn moderator_revocation_during_server_image_staging_aborts_before_publication() {
    let f = fixture();
    let upload = branding_metadata();
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER revoke_moderator_during_upload AFTER INSERT ON uploads WHEN NEW.purpose = 'server_image' BEGIN UPDATE platform_identities SET can_moderate = 0 WHERE pseudonym_id = NEW.uploader_pseudonym; END;").unwrap();
    assert!(matches!(
        upload_service::persist_server_image(&f.state, &upload, b"detected image"),
        Err(upload_service::UploadError::Forbidden(_))
    ));
    assert_eq!(count(&f, "uploads"), 0);
    let actual: Option<String> = f
        .state
        .pool
        .get()
        .unwrap()
        .query_row("SELECT image_url FROM servers WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(actual.is_none());
    assert!(!Path::new(&f.state.upload_dir)
        .join(format!("server/server_{}.png", upload.upload_id))
        .exists());
}

#[cfg(windows)]
#[test]
fn locked_file_cleanup_is_persisted_and_retried_after_the_lock_is_released() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = fixture();
    let upload = metadata();
    upload_service::persist_chat_upload(&f.state, &upload, b"file locked by a reader").unwrap();
    message(
        &f,
        "locked-message",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
    );
    associate(
        &f,
        "locked-message",
        "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
        std::slice::from_ref(&upload.upload_id),
    )
    .unwrap();
    let path = uploaded_path(&f, &upload);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "DELETE FROM messages WHERE message_id = 'locked-message'",
            [],
        )
        .unwrap();
    assert_eq!(upload_service::drain_deletion_queue(&f.state).unwrap(), 0);
    assert_eq!(count(&f, "upload_deletion_queue"), 1);
    assert!(path.exists());
    drop(lock);
    assert_eq!(upload_service::drain_deletion_queue(&f.state).unwrap(), 1);
    assert_eq!(count(&f, "upload_deletion_queue"), 0);
    assert!(!path.exists());
}

/// A real streaming request through the router, held after its first body
/// chunk. This is a controlled backend race test, not a browser/auth proof.
#[tokio::test]
async fn multipart_stream_rechecks_membership_token_and_policy_after_reading() {
    use axum::{
        body::{Body, Bytes},
        extract::ConnectInfo,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    for fault in ["membership", "token", "policy", "storage"] {
        let f = fixture();
        let router = annex_server::app(f.state.clone());
        let (begun_tx, begun_rx) = tokio::sync::oneshot::channel();
        let (body_tx, body_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
        let prefix = b"--annex-live-stream\r\nContent-Disposition: form-data; name=\"file\"; filename=\"ordinary.txt\"\r\nContent-Type: application/octet-stream\r\n\r\nfirst actual chunk".to_vec();
        let stream = futures_util::stream::unfold(
            (Some(prefix), body_rx, Some(begun_tx)),
            |(first, mut receiver, signal)| async move {
                if let Some(bytes) = first {
                    if let Some(signal) = signal {
                        let _ = signal.send(());
                    }
                    Some((
                        Ok::<_, std::convert::Infallible>(Bytes::from(bytes)),
                        (None, receiver, None),
                    ))
                } else {
                    receiver.recv().await.map(|bytes| {
                        (
                            Ok::<_, std::convert::Infallible>(Bytes::from(bytes)),
                            (None, receiver, None),
                        )
                    })
                }
            },
        );
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/channels/room/upload")
            .header(
                "X-Annex-Pseudonym",
                "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9",
            )
            .header(
                "Content-Type",
                "multipart/form-data; boundary=annex-live-stream",
            )
            .body(Body::from_stream(stream))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:9100".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let pending = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
        tokio::time::timeout(std::time::Duration::from_secs(10), begun_rx)
            .await
            .unwrap()
            .unwrap();
        {
            let conn = f.state.pool.get().unwrap();
            match fault {
                "membership" => {
                    conn.execute(
                        "DELETE FROM channel_members WHERE pseudonym_id = 'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
                        [],
                    )
                    .unwrap();
                }
                "token" => {
                    conn.execute("UPDATE platform_identities SET token_epoch = token_epoch + 1 WHERE pseudonym_id = 'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",[]).unwrap();
                }
                "storage" => {
                    f.state
                        .storage_health
                        .mark_degraded("controlled storage failure during streaming");
                }
                _ => {
                    conn.execute("UPDATE servers SET policy_json = json_set(policy_json,'$.files_enabled',json('false')) WHERE id = 1",[]).unwrap();
                }
            }
        }
        body_tx
            .send(b"final actual chunk\r\n--annex-live-stream--\r\n".to_vec())
            .await
            .unwrap();
        drop(body_tx);
        let response = pending.await.unwrap();
        let expected = match fault {
            "token" => StatusCode::UNAUTHORIZED,
            "storage" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::FORBIDDEN,
        };
        assert_eq!(response.status(), expected, "fault={fault}");
        assert_eq!(count(&f, "uploads"), 0, "fault={fault}");
        assert_eq!(count(&f, "message_uploads"), 0, "fault={fault}");
        assert_eq!(
            std::fs::read_dir(&f.state.upload_dir).unwrap().count(),
            0,
            "fault={fault}"
        );
    }
}

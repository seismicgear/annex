//! A member can preflight media without receiving operator secrets.
mod common;
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use tower::ServiceExt;

fn request(path: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(path);
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let mut req = builder.body(Body::empty()).unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:40000".parse::<SocketAddr>().unwrap(),
    ));
    req
}

#[tokio::test]
async fn ordinary_member_receives_exact_media_policy_only_while_authorized() {
    let f = common::revocation::fixture().await;
    {
        let conn = f.pool.get().unwrap();
        conn.execute(
            "UPDATE platform_identities SET can_moderate=0 WHERE pseudonym_id=?1",
            [&f.pseudonym],
        )
        .unwrap();
        conn.execute("INSERT INTO channels(channel_id,server_id,name,channel_type,federation_scope) VALUES ('room',?1,'Room','Text','Local')", [f.server_id]).unwrap();
        conn.execute(
            "INSERT INTO channel_members(server_id,channel_id,pseudonym_id) VALUES (?1,'room',?2)",
            rusqlite::params![f.server_id, f.pseudonym],
        )
        .unwrap();
        let policy = annex_types::ServerPolicy {
            images_enabled: true,
            videos_enabled: false,
            files_enabled: true,
            max_image_size_mb: 7,
            max_video_size_mb: 19,
            max_file_size_mb: 3,
            ..annex_types::ServerPolicy::default()
        };
        conn.execute(
            "UPDATE servers SET policy_json=?2 WHERE id=?1",
            rusqlite::params![f.server_id, serde_json::to_string(&policy).unwrap()],
        )
        .unwrap();
    }
    let path = "/api/channels/room/upload-policy";
    let response = f
        .app
        .clone()
        .oneshot(request(path, Some(&f.token)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"images_enabled":true,"videos_enabled":false,"files_enabled":true,
        "max_image_size_mb":7,"max_video_size_mb":19,"max_file_size_mb":3})
    );
    assert_eq!(
        f.app
            .clone()
            .oneshot(request("/api/admin/policy", Some(&f.token)))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.app
            .clone()
            .oneshot(request(path, None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.app
            .clone()
            .oneshot(request("/api/channels/other/upload-policy", Some(&f.token)))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    f.pool
        .get()
        .unwrap()
        .execute("DELETE FROM channel_members WHERE channel_id='room'", [])
        .unwrap();
    assert_eq!(
        f.app
            .clone()
            .oneshot(request(path, Some(&f.token)))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    common::revocation::revoke(&f.pool, f.server_id, &f.pseudonym);
    assert_eq!(
        f.app
            .clone()
            .oneshot(request(path, Some(&f.token)))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

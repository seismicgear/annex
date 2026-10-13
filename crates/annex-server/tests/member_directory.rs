//! Ordinary members need grant recipients without gaining admin metadata.
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
async fn ordinary_member_sees_only_active_local_pseudonyms_and_types() {
    let f = common::revocation::fixture().await;
    common::revocation::add_member(&f.pool, f.server_id, "other");
    common::revocation::add_member(&f.pool, f.server_id, "inactive");
    {
        let conn = f.pool.get().unwrap();
        conn.execute(
            "UPDATE platform_identities SET can_moderate=0 WHERE pseudonym_id=?1",
            [&f.pseudonym],
        )
        .unwrap();
        conn.execute(
            "UPDATE platform_identities SET active=0 WHERE pseudonym_id='inactive'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO servers (slug,label,policy_json) VALUES ('elsewhere','Elsewhere','{}')",
            [],
        )
        .unwrap();
    }
    common::revocation::add_member(&f.pool, 2, "different-server");
    let response = f
        .app
        .clone()
        .oneshot(request("/api/members", Some(&f.token)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"members": [
            {"pseudonym_id": f.pseudonym, "participant_type": "HUMAN"},
            {"pseudonym_id": "other", "participant_type": "HUMAN"}
        ]})
    );
    assert_eq!(
        f.app
            .clone()
            .oneshot(request("/api/admin/members", Some(&f.token)))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.app
            .clone()
            .oneshot(request("/api/members", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    common::revocation::revoke(&f.pool, f.server_id, &f.pseudonym);
    assert_eq!(
        f.app
            .clone()
            .oneshot(request("/api/members", Some(&f.token)))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

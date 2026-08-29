//! `POST /api/admin/server/image`, `POST /api/channels/{id}/upload` — the
//! upload routes at the HTTP boundary.
//!
//! `api_upload.rs` has a thorough unit-test module, and every one of those
//! tests is about a pure function: does `detect_content_type` recognise a
//! PNG, does `strip_jpeg_metadata` drop APP1. None of them can see the
//! things that actually make an upload route safe or useful, because none
//! of them go through the router:
//!
//!   * whether the handler consults the magic-byte detector at all, or
//!     quietly trusts the client's `Content-Type` — the detector can be
//!     perfect and still never be asked;
//!   * whether the authorization checks are wired to the routes (a
//!     `can_moderate` branch that no request reaches is decoration);
//!   * whether policy toggles and size limits are enforced on the way in;
//!   * whether the metadata stripping is applied to the bytes that are
//!     actually written to disk, rather than to a copy that is discarded;
//!   * whether the URL in the response body serves those bytes back.
//!
//! That last one is the same class of defect as the voice roster that never
//! crossed the wire: the response looked correct, every unit test passed,
//! and the feature was inert. A returned URL that 404s is indistinguishable
//! from a working one until somebody clicks it.

mod common;

use annex_db::{create_pool, run_migrations, DbPool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::app;
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use serde_json::Value;
use std::net::SocketAddr;
use tower::ServiceExt;

// ── Harness ──────────────────────────────────────────────────────────────
//
// The shared `setup_test_app` points `upload_dir` at the process temp dir,
// which is fine for a test that never writes but wrong for these: uploads
// would land in a directory shared with every other test and with the host.
// These tests need a private directory both to assert on the bytes on disk
// and so that `/uploads` is actually mounted — `attach_uploads` only nests
// the static service when the directory already exists.

struct UploadApp {
    router: axum::Router,
    pool: DbPool,
    /// Held for the lifetime of the test: dropping it removes the directory.
    _dir: tempfile::TempDir,
}

async fn setup_upload_app(policy: ServerPolicy) -> UploadApp {
    let dir = tempfile::tempdir().expect("temp upload dir");

    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        let policy_json = serde_json::to_string(&policy).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    let mut state = common::build_app_state(pool.clone(), tree, policy);
    state.upload_dir = dir.path().to_string_lossy().into_owned();

    UploadApp {
        router: app(state),
        pool,
        _dir: dir,
    }
}

fn add_member(pool: &DbPool, pseudonym: &str, can_moderate: bool) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities
           (server_id, pseudonym_id, participant_type, can_voice, can_moderate,
            can_invite, can_federate, can_bridge, active)
         VALUES (1, ?1, 'HUMAN', 1, ?2, 1, 0, 0, 1)",
        rusqlite::params![pseudonym, can_moderate as i64],
    )
    .unwrap();
}

fn add_channel(pool: &DbPool, channel_id: &str, member: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO channels (channel_id, server_id, name, channel_type, federation_scope)
         VALUES (?1, 1, 'uploads', 'Text', 'LOCAL_ONLY')",
        [channel_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO channel_members (channel_id, pseudonym_id, server_id)
         VALUES (?1, ?2, 1)",
        [channel_id, member],
    )
    .unwrap();
}

const BOUNDARY: &str = "annexuploadtestboundary";

/// Builds a `multipart/form-data` body with a single file field.
///
/// `declared_ct` is what the client *claims* the bytes are. Several tests
/// deliberately lie here — that is the point: the handler must decide from
/// the bytes, not from the header.
fn multipart_body(filename: &str, declared_ct: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {declared_ct}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn post_multipart(
    router: &axum::Router,
    uri: &str,
    caller: &str,
    body: Vec<u8>,
) -> (StatusCode, String) {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri(uri)
        .method("POST")
        .header("X-Annex-Pseudonym", caller)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn get(router: &axum::Router, uri: &str, caller: Option<&str>) -> (StatusCode, Vec<u8>) {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut builder = Request::builder().uri(uri).method("GET");
    if let Some(c) = caller {
        builder = builder.header("X-Annex-Pseudonym", c);
    }
    let mut req = builder.body(Body::empty()).unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

// ── Fixtures ─────────────────────────────────────────────────────────────

/// A PNG with a `tEXt` metadata chunk, so stripping has something to remove.
fn png_with_text_chunk() -> Vec<u8> {
    let mut png: Vec<u8> = vec![137, 80, 78, 71, 13, 10, 26, 10];
    // IHDR (kept)
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&[0u8; 13]);
    png.extend_from_slice(&[0, 0, 0, 0]);
    // tEXt (stripped) — the payload is the thing that must not survive
    let text = b"Author\0annex-secret-name";
    png.extend_from_slice(&(text.len() as u32).to_be_bytes());
    png.extend_from_slice(b"tEXt");
    png.extend_from_slice(text);
    png.extend_from_slice(&[0, 0, 0, 0]);
    // IEND (kept)
    png.extend_from_slice(&0u32.to_be_bytes());
    png.extend_from_slice(b"IEND");
    png.extend_from_slice(&[0, 0, 0, 0]);
    png
}

/// A JPEG carrying an APP1/EXIF segment with a recognisable payload.
fn jpeg_with_exif() -> Vec<u8> {
    let mut jpeg: Vec<u8> = vec![0xFF, 0xD8];
    // APP1 / EXIF (stripped). Length covers the two length bytes themselves.
    let exif = b"Exif\0\0annex-gps-location";
    let seg_len = (exif.len() + 2) as u16;
    jpeg.extend_from_slice(&[0xFF, 0xE1]);
    jpeg.extend_from_slice(&seg_len.to_be_bytes());
    jpeg.extend_from_slice(exif);
    // SOS + a scrap of scan data (kept)
    jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x01, 0x02, 0x03]);
    jpeg
}

fn webm() -> Vec<u8> {
    let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
    v.extend_from_slice(b"fake webm payload");
    v
}

fn elf() -> Vec<u8> {
    let mut v = vec![0x7F, 0x45, 0x4C, 0x46];
    v.extend_from_slice(b"fake elf payload");
    v
}

// ── Server image: authorization ──────────────────────────────────────────

#[tokio::test]
async fn a_plain_member_cannot_replace_the_server_image() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "member", false);

    let body = multipart_body("icon.png", "image/png", &png_with_text_chunk());
    let (status, body) = post_multipart(&a.router, "/api/admin/server/image", "member", body).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the server image is server-wide branding; a non-moderator changing it \
         is a defacement, not a preference: {body}",
    );
}

#[tokio::test]
async fn a_moderator_can_replace_the_server_image_and_it_is_readable_afterwards() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "boss", true);

    let body = multipart_body("icon.png", "image/png", &png_with_text_chunk());
    let (status, body) = post_multipart(&a.router, "/api/admin/server/image", "boss", body).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let json: Value = serde_json::from_str(&body).unwrap();
    let url = json["url"].as_str().expect("url in response");
    assert!(url.starts_with("/uploads/server/"), "unexpected url: {url}",);

    // The write has to be visible to the *next* request. An upload that
    // returns a URL but never lands in `servers.image_url` leaves the admin
    // panel showing the old icon with no error anywhere.
    //
    // The read is deliberately unauthenticated: the server icon is branding
    // shown on the join screen, before anyone has an identity. If this were
    // to require auth, the upload would appear to work and the icon would
    // still be missing everywhere it matters.
    let (status, bytes) = get(&a.router, "/api/public/server/image", None).await;
    assert_eq!(status, StatusCode::OK);
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json["image_url"].as_str(),
        Some(url),
        "GET does not reflect the image that was just uploaded",
    );
}

#[tokio::test]
async fn a_pdf_renamed_as_a_png_is_refused() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "boss", true);

    // Declared image/png, filename .png, actually a PDF. Only the bytes are
    // honest, and only the bytes may be believed.
    let body = multipart_body("icon.png", "image/png", b"%PDF-1.4 not an image at all");
    let (status, body) = post_multipart(&a.router, "/api/admin/server/image", "boss", body).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the declared content type was trusted over the magic bytes: {body}",
    );
}

#[tokio::test]
async fn a_server_image_upload_with_no_file_is_a_client_error() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "boss", true);

    // A well-formed multipart envelope with no parts in it.
    let body = format!("--{BOUNDARY}--\r\n").into_bytes();
    let (status, body) = post_multipart(&a.router, "/api/admin/server/image", "boss", body).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a missing file is the caller's mistake, not a server fault: {body}",
    );
}

#[tokio::test]
async fn the_recorded_content_type_is_the_detected_one_not_the_declared_one() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "boss", true);

    // Honest bytes, dishonest header: a real PNG declared as a JPEG. The
    // upload should succeed — and the row should say what it really is,
    // because everything downstream reads the row, not the request.
    let body = multipart_body("icon.png", "image/jpeg", &png_with_text_chunk());
    let (status, body) = post_multipart(&a.router, "/api/admin/server/image", "boss", body).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let conn = a.pool.get().unwrap();
    let ct: String = conn
        .query_row(
            "SELECT content_type FROM uploads WHERE purpose = 'server_image'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        ct, "image/png",
        "the client's declared MIME type was persisted instead of the detected one",
    );
}

// ── Chat uploads: membership ─────────────────────────────────────────────

#[tokio::test]
async fn a_non_member_cannot_upload_to_a_channel() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "alice", false);
    add_member(&a.pool, "stranger", false);
    add_channel(&a.pool, "chan-up", "alice");

    let body = multipart_body("x.png", "image/png", &png_with_text_chunk());
    let (status, body) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "stranger", body).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "uploading into a channel you cannot read is a way to post to it: {body}",
    );
}

#[tokio::test]
async fn a_member_can_upload_an_image_and_fetch_it_back() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "alice", false);
    add_channel(&a.pool, "chan-up", "alice");

    let body = multipart_body("photo.png", "image/png", &png_with_text_chunk());
    let (status, body) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "alice", body).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["category"].as_str(), Some("image"));
    assert_eq!(json["content_type"].as_str(), Some("image/png"));
    let url = json["url"].as_str().expect("url");
    assert!(
        url.starts_with("/uploads/chat/images/"),
        "unexpected url: {url}",
    );

    // The URL in the response is what the client renders. If it does not
    // serve, every uploaded image is a broken thumbnail and the upload
    // response still says "ok".
    let (status, served) = get(&a.router, url, Some("alice")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the returned URL does not serve: {url}"
    );
    assert!(!served.is_empty(), "the served file is empty: {url}");
}

// ── Chat uploads: the privacy promise, end to end ────────────────────────

#[tokio::test]
async fn exif_is_gone_from_the_bytes_that_are_actually_served() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "alice", false);
    add_channel(&a.pool, "chan-up", "alice");

    let original = jpeg_with_exif();
    assert!(
        original
            .windows(b"annex-gps-location".len())
            .any(|w| w == b"annex-gps-location"),
        "fixture is wrong: the EXIF payload is not in the input",
    );

    let body = multipart_body("photo.jpg", "image/jpeg", &original);
    let (status, body) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "alice", body).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    let url = json["url"].as_str().expect("url");

    let (status, served) = get(&a.router, url, Some("alice")).await;
    assert_eq!(status, StatusCode::OK);

    // This is the whole point of stripping: not that a function returns
    // clean bytes, but that the bytes a stranger can download are clean.
    assert!(
        !served
            .windows(b"annex-gps-location".len())
            .any(|w| w == b"annex-gps-location"),
        "the EXIF payload survived to the served file — stripping ran on a \
         copy that was thrown away, or did not run at all",
    );
    assert!(
        json["metadata_stripped_bytes"].as_u64().unwrap_or(0) > 0,
        "the response claims nothing was stripped: {json}",
    );
}

#[tokio::test]
async fn png_text_chunks_do_not_survive_the_round_trip() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "alice", false);
    add_channel(&a.pool, "chan-up", "alice");

    let body = multipart_body("photo.png", "image/png", &png_with_text_chunk());
    let (status, body) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "alice", body).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    let url = json["url"].as_str().expect("url");

    let (_, served) = get(&a.router, url, Some("alice")).await;
    assert!(
        !served.windows(4).any(|w| w == b"tEXt"),
        "a tEXt chunk survived the upload pipeline",
    );
    assert!(
        !served
            .windows(b"annex-secret-name".len())
            .any(|w| w == b"annex-secret-name"),
        "the tEXt payload survived the upload pipeline",
    );
}

// ── Chat uploads: policy is enforced at the edge ─────────────────────────

#[tokio::test]
async fn a_video_is_refused_when_the_policy_disables_videos() {
    let policy = ServerPolicy {
        videos_enabled: false,
        ..Default::default()
    };
    let a = setup_upload_app(policy).await;
    add_member(&a.pool, "alice", false);
    add_channel(&a.pool, "chan-up", "alice");

    let body = multipart_body("clip.webm", "video/webm", &webm());
    let (status, body) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "alice", body).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a policy toggle that the route never consults is a checkbox that \
         does nothing: {body}",
    );
}

/// An upload larger than the GLOBAL body limit still reaches the handler.
///
/// `MAX_REQUEST_BODY_BYTES` is 2 MiB and applies to the whole app; the upload
/// route group overrides it with `DefaultBodyLimit::max(50 MiB)` and lets the
/// per-category policy decide. The default policy allows 5 MB images, so a
/// 3 MiB photo is something the server explicitly permits — and every existing
/// size test stayed under 2 MiB, so nothing established that the override
/// actually wins. If the layer order were ever inverted, axum would answer 413
/// before the handler ran and every upload the policy allows above 2 MiB would
/// fail, with the policy still advertising 5 MB.
#[tokio::test]
async fn an_upload_above_the_global_body_limit_is_still_accepted() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "alice", false);
    add_channel(&a.pool, "chan-up", "alice");

    // 3 MiB: above the 2 MiB global limit, below the 5 MB image policy.
    let mut big = png_with_text_chunk();
    big.extend(std::iter::repeat_n(0u8, 3 * 1024 * 1024));
    assert!(
        big.len() > 2 * 1024 * 1024,
        "the payload must exceed the global body limit for this test to mean anything"
    );

    let body = multipart_body("photo.png", "image/png", &big);
    let (status, body) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "alice", body).await;

    assert_ne!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "the global 2 MiB limit reached the upload route: {body}",
    );
    assert!(
        status.is_success(),
        "an upload the policy allows was refused with {status}: {body}",
    );
}

#[tokio::test]
async fn an_oversized_image_is_refused_at_the_policy_limit() {
    let policy = ServerPolicy {
        max_image_size_mb: 1,
        ..Default::default()
    };
    let a = setup_upload_app(policy).await;
    add_member(&a.pool, "alice", false);
    add_channel(&a.pool, "chan-up", "alice");

    // A real PNG header followed by more than a mebibyte of padding, so the
    // rejection is on size rather than on format.
    let mut big = png_with_text_chunk();
    big.extend(std::iter::repeat_n(0u8, 1024 * 1024 + 1));

    let body = multipart_body("huge.png", "image/png", &big);
    let (status, body) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "alice", body).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the per-category size limit was not applied: {body}",
    );
}

#[tokio::test]
async fn an_executable_is_refused_however_it_is_labelled() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "alice", false);
    add_channel(&a.pool, "chan-up", "alice");

    // Declared as a harmless text file, actually an ELF binary.
    let body = multipart_body("notes.txt", "text/plain", &elf());
    let (status, body) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "alice", body).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an executable was accepted because it claimed to be text: {body}",
    );
}

#[tokio::test]
async fn an_unrecognised_format_is_stored_as_a_generic_file() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "alice", false);
    add_channel(&a.pool, "chan-up", "alice");

    // No magic bytes match. The handler must not fall back to the declared
    // type — that would be the spoofing hole by another route — and must
    // not 500 either.
    let body = multipart_body(
        "thing.png",
        "image/png",
        b"\x00\x01\x02\x03 whatever this is",
    );
    let (status, body) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "alice", body).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json["category"].as_str(),
        Some("file"),
        "an unrecognised payload was categorised from its declared type",
    );
    assert_eq!(
        json["content_type"].as_str(),
        Some("application/octet-stream"),
    );
}

#[tokio::test]
async fn the_upload_is_recorded_against_the_channel_and_the_uploader() {
    let a = setup_upload_app(ServerPolicy::default()).await;
    add_member(&a.pool, "alice", false);
    add_channel(&a.pool, "chan-up", "alice");

    let body = multipart_body("photo.png", "image/png", &png_with_text_chunk());
    let (status, _) =
        post_multipart(&a.router, "/api/channels/chan-up/upload", "alice", body).await;
    assert_eq!(status, StatusCode::OK);

    let conn = a.pool.get().unwrap();
    let (channel, uploader, purpose, category): (String, String, String, String) = conn
        .query_row(
            "SELECT channel_id, uploader_pseudonym, purpose, category FROM uploads",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();

    // Attribution is what makes moderation and retention possible. An
    // upload row with the wrong channel is invisible to both.
    assert_eq!(channel, "chan-up");
    assert_eq!(uploader, "alice");
    assert_eq!(purpose, "chat");
    assert_eq!(category, "image");
}

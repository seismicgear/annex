//! Tests for `resolve_signing_key` behaviour under production vs dev
//! profiles. Locks down the production gate that rejects ephemeral keys
//! and the weak-key sanity check.
//!
//! Each test serialises on the env-var lock — `ANNEX_BUILD_PROFILE`,
//! `ANNEX_SIGNING_KEY`, `ANNEX_ZK_KEY_PATH`, and other prepare-server
//! env vars are process-global, so parallel tests would race.

use annex_server::{config, prepare_server, StartupError};
use std::sync::OnceLock;
use tokio::sync::Mutex;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn clear_env() {
    for k in [
        "ANNEX_BUILD_PROFILE",
        "ANNEX_SIGNING_KEY",
        "ANNEX_ZK_KEY_PATH",
        "ANNEX_ZK_KEY_PATH_V2",
        "ANNEX_UPLOAD_DIR",
        "ANNEX_SERVER_SLUG",
        "ANNEX_SERVER_LABEL",
        "ANNEX_CORS_ORIGINS",
        "ANNEX_TRUSTED_PROXY_DEPTH",
        "ANNEX_DEPLOYMENT_MODE",
        "ANNEX_RATE_LIMIT_BACKEND",
        "ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED",
        "ANNEX_SIGNAL_TRUSTED_PEERS",
    ] {
        std::env::remove_var(k);
    }
}

/// Minimal-viable Config that won't fail validation: in-memory SQLite,
/// ephemeral port, ZK enforcement off (so an absent vkey doesn't
/// block us), and explicit CORS origins so the production CORS gate
/// doesn't preempt the signing-key check.
fn config_for_production_signing_test(db_path: &str) -> config::Config {
    let mut cfg = config::Config::default();
    cfg.database.path = db_path.to_string();
    cfg.server.host = std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1));
    cfg.server.port = 0;
    cfg.security.enforce_zk_proofs = false;
    cfg.cors.allowed_origins = vec!["https://app.example.com".to_string()];
    cfg
}

#[tokio::test]
async fn production_rejects_all_zero_signing_key_env() {
    let _guard = env_lock().lock().await;
    clear_env();

    std::env::set_var("ANNEX_BUILD_PROFILE", "production");
    std::env::set_var(
        "ANNEX_SIGNING_KEY",
        // 64 zero hex chars — 32 zero bytes. The classic placeholder.
        "0000000000000000000000000000000000000000000000000000000000000000",
    );

    let cfg = config_for_production_signing_test(":memory:");
    let err = prepare_server(cfg)
        .await
        .expect_err("production with all-zero signing key must fail to start");
    assert!(
        matches!(err, StartupError::WeakSigningKey { .. }),
        "expected WeakSigningKey, got {err:?}",
    );
}

#[tokio::test]
async fn production_rejects_all_ff_signing_key_env() {
    let _guard = env_lock().lock().await;
    clear_env();

    std::env::set_var("ANNEX_BUILD_PROFILE", "production");
    std::env::set_var(
        "ANNEX_SIGNING_KEY",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    );

    let cfg = config_for_production_signing_test(":memory:");
    let err = prepare_server(cfg)
        .await
        .expect_err("production with all-ff signing key must fail to start");
    assert!(
        matches!(err, StartupError::WeakSigningKey { .. }),
        "expected WeakSigningKey, got {err:?}",
    );
}

#[tokio::test]
async fn production_rejects_single_byte_fill_signing_key_env() {
    let _guard = env_lock().lock().await;
    clear_env();

    std::env::set_var("ANNEX_BUILD_PROFILE", "production");
    // 0xab repeated 32 times — common dev fixture pattern.
    std::env::set_var(
        "ANNEX_SIGNING_KEY",
        "abababababababababababababababababababababababababababababababab",
    );

    let cfg = config_for_production_signing_test(":memory:");
    let err = prepare_server(cfg)
        .await
        .expect_err("single-byte-fill key must be rejected");
    assert!(
        matches!(err, StartupError::WeakSigningKey { .. }),
        "expected WeakSigningKey, got {err:?}",
    );
}

#[tokio::test]
async fn production_accepts_real_signing_key_env() {
    let _guard = env_lock().lock().await;
    clear_env();

    std::env::set_var("ANNEX_BUILD_PROFILE", "production");
    // A real-looking random key (no pattern). Generated once at fixture
    // authoring time via `openssl rand -hex 32`; bytes are independent.
    std::env::set_var(
        "ANNEX_SIGNING_KEY",
        "f2c4a9e1bd7e3856102b94d1c9f6a3c8d4e7b2f5a8c1d9e6b3f0a7c4e1d8b5f2",
    );

    let cfg = config_for_production_signing_test(":memory:");
    let (listener, _router) = prepare_server(cfg)
        .await
        .expect("production accepts a real 32-byte key");
    drop(listener); // free the OS port immediately
}

// Note on coverage gap: the EphemeralSigningKeyInProduction branch
// is reachable only when create_dir_all OR std::fs::write fails on
// the data directory under production profile. DB pool initialisation
// runs FIRST in prepare_server and itself calls create_dir_all on the
// same directory, so any setup that makes the dir unwritable also
// fails the DB init with `DatabaseError(PoolInit(...))`, masking the
// signing-key error path in `prepare_server`-level tests. The
// production guard remains correct (see startup.rs::resolve_signing_key);
// a follow-up refactor that surfaces resolve_signing_key as a unit-
// testable function would let us pin this path directly.

#[tokio::test]
async fn voice_tokens_survive_restart_with_same_persistent_key() {
    let _guard = env_lock().lock().await;
    clear_env();

    // Two starts of prepare_server pointing at the SAME data_dir; the
    // signing key should be persisted to disk on first run and loaded
    // on the second. A voice-join token minted from the first server's
    // derived secret must verify against the second server's
    // independently-derived secret, proving the secret round-trips.
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("annex.db");
    let db_str = db_path.to_string_lossy().into_owned();

    let cfg1 = config_for_production_signing_test(&db_str);
    std::env::set_var("ANNEX_BUILD_PROFILE", "production");
    let (l1, _r1) = prepare_server(cfg1).await.expect("first start ok");
    drop(l1);

    // Capture the secret from disk: rebuild it ourselves using the
    // same derivation that startup runs.
    let key_file = dir.path().join("signing.key");
    let hex_key = std::fs::read_to_string(&key_file).expect("signing key persisted");
    let bytes = hex::decode(hex_key.trim()).expect("hex");
    let signing_key = ed25519_dalek::SigningKey::from_bytes(
        &<[u8; 32]>::try_from(bytes.as_slice()).expect("32 bytes"),
    );
    let secret = annex_voice::derive_voice_token_secret(&signing_key);

    // Mint a token with the derived secret.
    let token =
        annex_voice::generate_join_token("ch-restart", "alice", &secret, 60).expect("token signs");

    // Second start: same db_path → same on-disk key → same derived secret.
    let cfg2 = config_for_production_signing_test(&db_str);
    let (l2, _r2) = prepare_server(cfg2).await.expect("second start ok");
    drop(l2);

    let hex_key2 = std::fs::read_to_string(&key_file).expect("still persisted");
    assert_eq!(
        hex_key.trim(),
        hex_key2.trim(),
        "signing key must be stable"
    );
    let bytes2 = hex::decode(hex_key2.trim()).expect("hex");
    let signing_key2 = ed25519_dalek::SigningKey::from_bytes(
        &<[u8; 32]>::try_from(bytes2.as_slice()).expect("32 bytes"),
    );
    let secret2 = annex_voice::derive_voice_token_secret(&signing_key2);

    // Token minted before "restart" verifies under the
    // freshly-derived secret. This is the invariant operators rely on
    // when running rolling deploys: in-flight tokens survive the
    // restart.
    let claims =
        annex_voice::verify_join_token(&token, &secret2, Some("ch-restart"), Some("alice"))
            .expect("token round-trips through restart");
    assert_eq!(claims.room, "ch-restart");
    assert_eq!(claims.sub, "alice");
}

#[tokio::test]
async fn voice_tokens_become_invalid_when_signing_key_rotates() {
    // If an operator rotates `ANNEX_SIGNING_KEY` deliberately (key
    // compromise drill, regenerate, etc.), every existing voice-join
    // token MUST stop verifying. This documents the only safe rotation
    // path: rotate the signing key and accept that issued tokens are
    // invalidated.
    let signer_a = ed25519_dalek::SigningKey::from_bytes(&[
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        0x00, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0,
        0xf0, 0x01,
    ]);
    let signer_b = ed25519_dalek::SigningKey::from_bytes(&[
        0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
        0x99, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45,
        0x67, 0x89,
    ]);
    let secret_a = annex_voice::derive_voice_token_secret(&signer_a);
    let secret_b = annex_voice::derive_voice_token_secret(&signer_b);
    assert_ne!(secret_a, secret_b);

    let token = annex_voice::generate_join_token("ch", "u", &secret_a, 60).expect("token");
    let err = annex_voice::verify_join_token(&token, &secret_b, None, None)
        .expect_err("rotated secret invalidates old tokens");
    assert_eq!(err, annex_voice::VoiceTokenError::Tampered);
}

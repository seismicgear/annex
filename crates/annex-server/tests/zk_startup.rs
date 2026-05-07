//! Tests for `prepare_server` ZK verification-key loading.
//!
//! These tests cover the documented behaviour of `Config::security::enforce_zk_proofs`:
//!
//! * In enforced mode (the default), the server MUST refuse to start with a
//!   missing or invalid membership verification key.
//! * In unenforced mode (development / test only), a missing key is allowed
//!   and the server falls back to `generate_dummy_vkey()` after logging a
//!   loud warning that identity security is disabled.
//!
//! Tests share a process-wide env-var lock because `ANNEX_ZK_KEY_PATH`,
//! `ANNEX_SIGNING_KEY`, and `ANNEX_UPLOAD_DIR` are read directly by
//! `annex_server::prepare_server`. Without serialisation, parallel tests
//! would race on the global env.

use annex_server::{config, prepare_server, StartupError};
use std::sync::OnceLock;
use tokio::sync::Mutex;

/// Process-wide mutex serialising every test that reads or mutates env vars
/// used by `prepare_server`. Must wrap **every** call into the function.
///
/// `tokio::sync::Mutex` (not `std::sync::Mutex`) so we can hold the guard
/// across the `prepare_server(cfg).await` point without tripping the
/// `clippy::await_holding_lock` defence-in-depth lint.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Removes every env var prepare_server reads. The matching `set_var` calls
/// at the top of each test scope should be preceded by this so leakage from
/// a previous test or the outer environment cannot mask a regression.
fn clear_env() {
    for k in [
        "ANNEX_ZK_KEY_PATH",
        "ANNEX_ZK_KEY_PATH_V2",
        "ANNEX_SIGNING_KEY",
        "ANNEX_UPLOAD_DIR",
        "ANNEX_SERVER_SLUG",
        "ANNEX_SERVER_LABEL",
    ] {
        std::env::remove_var(k);
    }
}

/// Builds a minimum-viable `Config` for `prepare_server`: in-memory SQLite,
/// port 0 (let the OS pick), and the supplied `enforce_zk_proofs` flag.
fn config_for_test(enforce_zk_proofs: bool) -> config::Config {
    let mut cfg = config::Config::default();
    cfg.database.path = ":memory:".to_string();
    cfg.server.host = std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1));
    cfg.server.port = 0;
    cfg.security.enforce_zk_proofs = enforce_zk_proofs;
    cfg
}

/// Picks a unique temp path for ANNEX_ZK_KEY_PATH that is guaranteed not to
/// exist. We never create the file at this path; tests that need a real
/// (or pretend-real) file use `tempfile::NamedTempFile`.
fn unique_missing_path(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join(format!("annex-zk-startup-missing-{label}-{nanos}.json"))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn zk_config_default_enforces_zk() {
    // FINDING-001 invariant restated for this test surface: `Config::default()`
    // must boot with enforcement on. Anything else means a misconfigured
    // server silently allows raw-pseudonym auth and accepts no real proofs.
    let cfg = config::Config::default();
    assert!(
        cfg.security.enforce_zk_proofs,
        "Config::default() must keep enforce_zk_proofs = true"
    );
}

#[tokio::test]
async fn zk_enforced_mode_missing_vkey_returns_startup_error() {
    let _guard = env_lock().lock().await;
    clear_env();

    let path = unique_missing_path("enforced");
    std::env::set_var("ANNEX_ZK_KEY_PATH", &path);
    // Avoid touching disk for the signing key.
    std::env::set_var("ANNEX_SIGNING_KEY", "00".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );

    let cfg = config_for_test(true);

    let result = prepare_server(cfg).await;
    clear_env();

    match result {
        Err(StartupError::MissingVerificationKey {
            path: reported,
            reason: _,
        }) => {
            assert_eq!(reported, path, "error must echo the path we set");
        }
        Err(other) => panic!("expected StartupError::MissingVerificationKey, got: {other:?}"),
        Ok(_) => {
            panic!("prepare_server unexpectedly succeeded with a missing vkey under enforcement")
        }
    }
}

#[tokio::test]
async fn zk_unenforced_mode_missing_vkey_starts_with_dummy() {
    let _guard = env_lock().lock().await;
    clear_env();

    let path = unique_missing_path("unenforced");
    std::env::set_var("ANNEX_ZK_KEY_PATH", &path);
    std::env::set_var("ANNEX_SIGNING_KEY", "01".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );

    let cfg = config_for_test(false);

    let result = prepare_server(cfg).await;
    clear_env();

    let (listener, _router) = result
        .expect("prepare_server must succeed with a missing vkey when enforce_zk_proofs is false");
    // Sanity: the listener should be bound to a valid port — drop it so the
    // OS reclaims the socket before the test exits.
    let local_addr = listener
        .local_addr()
        .expect("test listener must report a bound address");
    assert!(local_addr.port() != 0, "OS should have picked a real port");
    drop(listener);
}

#[test]
fn zk_config_default_only_enables_v1() {
    // The v2 circuit ships alongside v1 but must not be enabled by default
    // — Config::default() carries an explicit ["v1"] so a server running on
    // a stock config never silently accepts v2 payloads (or rejects v1
    // payloads) just because the v2 vkey happens to be on disk.
    let cfg = config::Config::default();
    assert_eq!(
        cfg.security.enabled_zk_versions,
        vec!["v1".to_string()],
        "Config::default().security.enabled_zk_versions must be exactly [\"v1\"]"
    );
}

#[tokio::test]
async fn zk_unknown_version_in_config_is_startup_error() {
    let _guard = env_lock().lock().await;
    clear_env();
    std::env::set_var("ANNEX_SIGNING_KEY", "03".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );

    let mut cfg = config_for_test(false);
    cfg.security.enabled_zk_versions = vec!["v1".to_string(), "v3-future".to_string()];

    let result = prepare_server(cfg).await;
    clear_env();

    match result {
        Err(StartupError::UnknownZkVersion { version }) => {
            assert_eq!(version, "v3-future");
        }
        Err(other) => panic!("expected StartupError::UnknownZkVersion, got: {other:?}"),
        Ok(_) => panic!("prepare_server unexpectedly accepted an unknown version"),
    }
}

#[tokio::test]
async fn zk_v2_enabled_loads_v2_vkey() {
    let _guard = env_lock().lock().await;
    clear_env();
    // Use the dev-fixture v2 vkey produced by `node zk/scripts/dev-setup-groth16.js`
    // — committed at zk/keys/membership_v2_vkey.json. If the file is absent
    // (e.g. fresh checkout that has never run dev-setup), gracefully skip
    // the test rather than fail spuriously: the unenforced path's dummy
    // fallback is exercised by other tests in this file.
    let v2_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_v2_vkey.json");
    if !v2_path.exists() {
        eprintln!("[zk_startup] skipping: {} not found", v2_path.display());
        return;
    }
    let v1_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");

    std::env::set_var("ANNEX_ZK_KEY_PATH", v1_path.to_string_lossy().as_ref());
    std::env::set_var("ANNEX_ZK_KEY_PATH_V2", v2_path.to_string_lossy().as_ref());
    std::env::set_var("ANNEX_SIGNING_KEY", "04".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );

    let mut cfg = config_for_test(true);
    cfg.security.enabled_zk_versions = vec!["v1".to_string(), "v2".to_string()];

    let result = prepare_server(cfg).await;
    clear_env();

    let (listener, _router) =
        result.expect("v1+v2 enabled with both keys present must boot cleanly");
    drop(listener);
}

#[tokio::test]
async fn zk_v2_enforced_missing_vkey_returns_startup_error() {
    let _guard = env_lock().lock().await;
    clear_env();

    // v1 key resolves (use the real one); v2 key explicitly missing.
    let v1_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");
    if !v1_path.exists() {
        eprintln!("[zk_startup] skipping: {} not found", v1_path.display());
        return;
    }
    let missing_v2 = unique_missing_path("v2-enforced");
    std::env::set_var("ANNEX_ZK_KEY_PATH", v1_path.to_string_lossy().as_ref());
    std::env::set_var("ANNEX_ZK_KEY_PATH_V2", &missing_v2);
    std::env::set_var("ANNEX_SIGNING_KEY", "05".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );

    let mut cfg = config_for_test(true);
    cfg.security.enabled_zk_versions = vec!["v1".to_string(), "v2".to_string()];

    let result = prepare_server(cfg).await;
    clear_env();

    match result {
        Err(StartupError::MissingVerificationKey { path, reason }) => {
            assert_eq!(path, missing_v2, "error must echo the missing v2 path");
            assert!(
                reason.contains("membership v2"),
                "v2 missing-key error must label the version: {reason}"
            );
        }
        Err(other) => panic!("expected StartupError::MissingVerificationKey, got: {other:?}"),
        Ok(_) => panic!(
            "prepare_server must refuse to start with v2 enabled + missing v2 vkey under enforcement"
        ),
    }
}

#[tokio::test]
async fn zk_enforced_mode_rejects_on_disk_dummy_vkey() {
    // Defence in depth: even if a dummy vkey somehow ends up on disk at the
    // configured path (e.g. a build pipeline accidentally copies the wrong
    // file, or a developer hand-crafts one), startup MUST refuse rather than
    // silently load it. A dummy vkey accepts every membership proof.
    let _guard = env_lock().lock().await;
    clear_env();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dummy_path = std::env::temp_dir().join(format!("annex-zk-startup-dummy-{nanos}.json"));
    let dummy_json = annex_identity::zk::serialize_vkey_to_snarkjs_json(
        &annex_identity::zk::generate_dummy_vkey(),
    );
    std::fs::write(&dummy_path, dummy_json).expect("must be able to write dummy vkey tempfile");
    let dummy_path_str = dummy_path.to_string_lossy().into_owned();

    std::env::set_var("ANNEX_ZK_KEY_PATH", &dummy_path_str);
    std::env::set_var("ANNEX_SIGNING_KEY", "06".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );

    let cfg = config_for_test(true);

    let result = prepare_server(cfg).await;
    clear_env();
    let _ = std::fs::remove_file(&dummy_path);

    match result {
        Err(StartupError::DummyVerificationKey { path }) => {
            assert_eq!(path, dummy_path_str, "error must echo the dummy path");
        }
        Err(other) => panic!("expected StartupError::DummyVerificationKey, got: {other:?}"),
        Ok(_) => {
            panic!("prepare_server unexpectedly accepted an on-disk dummy vkey under enforcement")
        }
    }
}

#[tokio::test]
async fn zk_unenforced_mode_accepts_on_disk_dummy_vkey() {
    // The same dummy file must load fine in unenforced (dev) mode — the dummy
    // is the explicit fallback for that mode and is accepted by design.
    let _guard = env_lock().lock().await;
    clear_env();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dummy_path = std::env::temp_dir().join(format!("annex-zk-startup-dummy-dev-{nanos}.json"));
    let dummy_json = annex_identity::zk::serialize_vkey_to_snarkjs_json(
        &annex_identity::zk::generate_dummy_vkey(),
    );
    std::fs::write(&dummy_path, dummy_json).expect("must be able to write dummy vkey tempfile");

    std::env::set_var("ANNEX_ZK_KEY_PATH", dummy_path.to_string_lossy().as_ref());
    std::env::set_var("ANNEX_SIGNING_KEY", "07".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );

    let cfg = config_for_test(false);

    let result = prepare_server(cfg).await;
    clear_env();
    let _ = std::fs::remove_file(&dummy_path);

    let (listener, _router) = result
        .expect("dev mode must accept an on-disk dummy vkey (matches the in-memory fallback path)");
    drop(listener);
}

#[tokio::test]
async fn zk_v2_enforced_mode_rejects_on_disk_dummy_vkey() {
    // Same gate must also fire on the v2 vkey path when v2 is enabled.
    let _guard = env_lock().lock().await;
    clear_env();

    let v1_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");
    if !v1_path.exists() {
        eprintln!("[zk_startup] skipping: {} not found", v1_path.display());
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dummy_v2 = std::env::temp_dir().join(format!("annex-zk-startup-dummy-v2-{nanos}.json"));
    let dummy_json = annex_identity::zk::serialize_vkey_to_snarkjs_json(
        &annex_identity::zk::generate_dummy_vkey(),
    );
    std::fs::write(&dummy_v2, dummy_json).expect("must be able to write dummy v2 vkey tempfile");
    let dummy_v2_str = dummy_v2.to_string_lossy().into_owned();

    std::env::set_var("ANNEX_ZK_KEY_PATH", v1_path.to_string_lossy().as_ref());
    std::env::set_var("ANNEX_ZK_KEY_PATH_V2", &dummy_v2_str);
    std::env::set_var("ANNEX_SIGNING_KEY", "08".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );

    let mut cfg = config_for_test(true);
    cfg.security.enabled_zk_versions = vec!["v1".to_string(), "v2".to_string()];

    let result = prepare_server(cfg).await;
    clear_env();
    let _ = std::fs::remove_file(&dummy_v2);

    match result {
        Err(StartupError::DummyVerificationKey { path }) => {
            assert_eq!(path, dummy_v2_str, "error must echo the v2 dummy path");
        }
        Err(other) => panic!("expected StartupError::DummyVerificationKey, got: {other:?}"),
        Ok(_) => panic!(
            "prepare_server unexpectedly accepted an on-disk dummy v2 vkey under enforcement"
        ),
    }
}

#[tokio::test]
async fn zk_enforced_mode_invalid_vkey_returns_startup_error() {
    let _guard = env_lock().lock().await;
    clear_env();

    // Write garbage to a tempfile and point ANNEX_ZK_KEY_PATH at it. The
    // file exists, so the loader proceeds to the parser — which must reject.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let bad_path = std::env::temp_dir().join(format!("annex-zk-startup-invalid-{nanos}.json"));
    std::fs::write(&bad_path, b"this is not a verification key json")
        .expect("must be able to write tempfile");
    let bad_path_str = bad_path.to_string_lossy().into_owned();

    std::env::set_var("ANNEX_ZK_KEY_PATH", &bad_path_str);
    std::env::set_var("ANNEX_SIGNING_KEY", "02".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );

    let cfg = config_for_test(true);

    let result = prepare_server(cfg).await;
    clear_env();
    let _ = std::fs::remove_file(&bad_path);

    match result {
        // The current loader propagates parser failures via `ZkError`. That
        // is the contract this test pins.
        Err(StartupError::ZkError(_)) => {}
        Err(StartupError::MissingVerificationKey { .. }) => panic!(
            "enforced mode + invalid key must surface as ZkError, not MissingVerificationKey \
             — the file existed but was unparseable"
        ),
        Err(other) => panic!("expected StartupError::ZkError(..), got: {other:?}"),
        Ok(_) => {
            panic!("prepare_server unexpectedly succeeded with an invalid vkey under enforcement")
        }
    }
}

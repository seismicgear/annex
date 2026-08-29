//! Startup orchestration.
//!
//! Owns the path from a parsed [`crate::config::Config`] to a bound TCP
//! listener and a fully-wired [`Router`]: opens the database pool, runs
//! migrations, restores the Merkle tree, seeds defaults on first boot,
//! loads the ZK verification keys, resolves the federation signing key,
//! constructs voice/TTS/STT services, builds [`AppState`], spawns the
//! background tasks, and finally binds the listener.
//!
//! HTTP-layer construction (CORS, body limits, security headers) lives
//! under [`crate::http`]; route assembly lives under [`crate::routes`].
//! This module does no I/O beyond what the original inline `prepare_server`
//! did.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};

use annex_identity::MerkleTree;
use annex_types::ServerPolicy;
use axum::Router;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rusqlite::OptionalExtension;
use thiserror::Error;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use crate::api_link_preview;
use crate::api_ws;
use crate::background;
use crate::config;
use crate::middleware::RateLimiter;
use crate::retention;
use crate::routes;
use crate::state::AppState;

/// Errors that can occur during server startup.
#[derive(Debug, Error)]
pub enum StartupError {
    /// The configured logging level filter string was invalid.
    #[error("invalid logging.level '{value}': {reason}")]
    InvalidLoggingLevel { value: String, reason: String },
    /// Failed to load configuration from file or environment.
    #[error("failed to load configuration: {0}")]
    ConfigError(#[from] config::ConfigError),
    /// Failed to initialize the database connection pool.
    #[error("failed to initialize database pool: {0}")]
    DatabaseError(#[from] annex_db::PoolError),
    /// Failed to initialize or restore the Merkle tree.
    #[error("failed to initialize merkle tree: {0}")]
    IdentityError(#[from] annex_identity::IdentityError),
    /// Failed to read a file from disk (e.g. verification key).
    #[error("failed to read verification key: {0}")]
    IoError(#[from] std::io::Error),
    /// Failed to parse the ZK verification key JSON.
    #[error("failed to parse verification key: {0}")]
    ZkError(#[from] annex_identity::zk::ZkError),
    /// Failed to get a database connection from the pool.
    #[error("failed to get database connection from pool: {0}")]
    PoolConnection(#[from] r2d2::Error),
    /// A database migration failed.
    #[error("database migration failed: {0}")]
    Migration(#[from] annex_db::MigrationError),
    /// A database query failed during initialization.
    #[error("database query failed: {0}")]
    DbQuery(#[from] rusqlite::Error),
    /// The `ANNEX_SIGNING_KEY` environment variable was malformed.
    #[error("invalid ANNEX_SIGNING_KEY: {0}")]
    InvalidSigningKey(String),
    /// `enforce_zk_proofs` is enabled but the membership verification key is
    /// missing on disk. Refusing to start with the dummy key fallback.
    #[error(
        "ZK enforcement is enabled but the membership verification key was not found at '{path}': \
         {reason}. Refusing to start with a dummy key. \
         Provide a real key (e.g. via ANNEX_ZK_KEY_PATH or by generating one with \
         `node zk/scripts/dev-setup-groth16.js`), or set security.enforce_zk_proofs = false \
         for development."
    )]
    MissingVerificationKey { path: String, reason: String },
    /// `enforce_zk_proofs` is enabled but the file at the verification-key
    /// path is byte-identical to [`annex_identity::zk::generate_dummy_vkey`].
    /// Refusing to load a dummy verifying key in enforced mode even if it's
    /// on disk — that would silently accept every membership proof.
    #[error(
        "ZK enforcement is enabled but '{path}' contains the dummy verification key. \
         Refusing to start: a dummy vkey would accept any proof. \
         Replace the file with a real key produced by the trusted setup ceremony, \
         or set security.enforce_zk_proofs = false for development."
    )]
    DummyVerificationKey { path: String },
    /// `Config::security.enabled_zk_versions` listed a value other than the
    /// recognised set (`"v1"`, `"v2"`).
    #[error(
        "unknown ZK protocol version '{version}' in security.enabled_zk_versions \
         (recognised values: \"v1\", \"v2\")"
    )]
    UnknownZkVersion { version: String },
    /// Production refused to fall back to an ephemeral signing key after
    /// failing to persist a freshly-generated one to disk. An ephemeral
    /// key would silently invalidate every token this server has ever
    /// issued (WS sessions, voice-join tokens, federation signatures) on
    /// the next restart; under production we hard-fail instead.
    #[error(
        "ANNEX_BUILD_PROFILE=production but the signing key at '{path}' could not be \
         persisted: {reason}. Refusing to run with an ephemeral key. Either ensure the \
         data directory is writable, set ANNEX_SIGNING_KEY explicitly, or run a dev profile."
    )]
    EphemeralSigningKeyInProduction { path: String, reason: String },
    /// Production rejected an obviously-weak signing key (all-zero, all-`0xff`,
    /// or any single-byte fill). These patterns show up in test fixtures and
    /// in mis-pasted env vars; accepting one in production would compromise
    /// every voice-join HMAC and federation signature this server emits.
    #[error(
        "ANNEX_BUILD_PROFILE=production but the signing key from {origin} is a weak \
         placeholder (all-zero / all-0xff / single-byte fill). Refusing to run. Replace \
         it with a real 32-byte secret."
    )]
    WeakSigningKey { origin: String },
}

/// Native WebRTC is embedded in-process; no external sidecar startup is required.
async fn ensure_webrtc_running(
    _config: &annex_voice::WebRtcConfig,
) -> Option<tokio::process::Child> {
    None
}

/// A raw `voice_profiles` row as read from SQLite:
/// `(profile_id, name, model, model_path, config_path, speed, pitch, speaker_id)`.
type VoiceProfileRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    f32,
    f32,
    Option<u32>,
);

/// Loads operator-configured rows from the `voice_profiles` table for
/// `server_id` into the running [`annex_voice::TtsService`]. Returns the count
/// loaded. Best-effort: a malformed row is skipped with a warning rather than
/// failing startup, and a DB error returns 0 (the built-in `"default"` profile
/// still works).
async fn load_voice_profiles(
    pool: &annex_db::DbPool,
    server_id: i64,
    tts: &annex_voice::TtsService,
) -> usize {
    use annex_types::voice::{VoiceModel, VoiceProfile};

    let pool = pool.clone();
    let rows =
        tokio::task::spawn_blocking(move || -> Vec<VoiceProfileRow> {
            let Ok(conn) = pool.get() else {
                return Vec::new();
            };
            let Ok(mut stmt) = conn.prepare(
            "SELECT profile_id, name, model, model_path, config_path, speed, pitch, speaker_id \
             FROM voice_profiles WHERE server_id = ?1",
        ) else { return Vec::new() };
            let mapped = stmt.query_map([server_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, f64>(5)? as f32,
                    row.get::<_, f64>(6)? as f32,
                    row.get::<_, Option<i64>>(7)?.map(|v| v as u32),
                ))
            });
            match mapped {
                Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            }
        })
        .await
        .unwrap_or_default();

    let mut count = 0;
    for (profile_id, name, model_str, model_path, config_path, speed, pitch, speaker_id) in rows {
        let model = match model_str.to_ascii_lowercase().as_str() {
            "piper" => VoiceModel::Piper,
            "bark" => VoiceModel::Bark,
            "system" => VoiceModel::System,
            other => {
                tracing::warn!(profile = %profile_id, model = %other, "skipping voice profile with unknown model");
                continue;
            }
        };
        tts.add_profile(VoiceProfile {
            id: profile_id,
            name,
            model,
            model_path,
            config_path,
            speed,
            pitch,
            speaker_id,
        })
        .await;
        count += 1;
    }
    count
}

/// Loads an **optional** capability-circuit verification key.
///
/// Unlike the membership key (which is the core authentication primitive and is
/// hard-required under `enforce_zk_proofs`), the channel-eligibility,
/// link-pseudonyms, and federation-attestation circuits are opt-in capabilities.
/// A server that doesn't ship their keys should still boot — the matching
/// endpoint just returns `503 Service Unavailable`. So a **missing** key is
/// never a startup error; it disables that one feature.
///
/// Path priority: the named env var, otherwise `default_path`. Behaviour:
///   - file present & parses & (under enforcement) is not the dummy key →
///     `Some(Arc<vkey>)`.
///   - file present but parses to the dummy key under enforcement →
///     `StartupError::DummyVerificationKey` (a dummy verifies any proof, so a
///     committed/misbundled dummy must fail loudly even for an optional circuit).
///   - file missing (with OR without enforcement) → `None` plus a warning; the
///     feature is unavailable and the endpoint returns 503.
///
/// Returning `None` (rather than a dummy) is deliberate: a handler that finds
/// `None` reports "not configured" instead of silently accepting proofs against
/// a dummy key.
fn load_circuit_vkey(
    env_var: &str,
    default_path: &str,
    label: &str,
    enforce_zk_proofs: bool,
) -> Result<Option<Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>>>, StartupError>
{
    let path = std::env::var(env_var).unwrap_or_else(|_| default_path.to_string());
    match std::fs::read_to_string(&path) {
        Ok(vkey_json) => {
            let parsed = annex_identity::zk::parse_verification_key(&vkey_json)
                .map_err(StartupError::ZkError)?;
            if enforce_zk_proofs && annex_identity::zk::is_dummy_vkey(&parsed) {
                return Err(StartupError::DummyVerificationKey { path });
            }
            tracing::info!(circuit = label, path = %path, "loaded ZK circuit verification key");
            Ok(Some(Arc::new(parsed)))
        }
        Err(e) => {
            // Missing optional-circuit key: disable the feature, do NOT fail
            // startup — even under enforcement. (The core membership key IS
            // still hard-required; that check lives above this helper.)
            tracing::warn!(
                circuit = label,
                path = %path,
                error = %e,
                "ZK circuit verification key not found — this capability is disabled \
                 and its endpoint will return 503 until the key is provided (e.g. via \
                 the env override or `node zk/scripts/dev-setup-groth16.js`)."
            );
            Ok(None)
        }
    }
}

/// Initializes the tracing subscriber based on logging configuration.
///
/// Must be called exactly once per process, before any tracing macros are used.
pub fn init_tracing(logging: &config::LoggingConfig) -> Result<(), StartupError> {
    let filter =
        EnvFilter::try_new(&logging.level).map_err(|err| StartupError::InvalidLoggingLevel {
            value: logging.level.clone(),
            reason: err.to_string(),
        })?;

    if logging.json {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    Ok(())
}

/// Resolve the Ed25519 signing key for federation identity.
///
/// Priority:
/// 1. `ANNEX_SIGNING_KEY` environment variable (64-char hex)
/// 2. Persistent key file at `{data_dir}/signing.key`
/// 3. Generate a new key and write it to `{data_dir}/signing.key`
///
/// Under a production profile (`ANNEX_BUILD_PROFILE=production|release`) the
/// key MUST come from one of those three paths AND be persistent: if the
/// generate-and-write step fails to flush a key to disk, this function
/// returns `StartupError::EphemeralSigningKeyInProduction`. Ephemeral keys
/// rotate on every restart, which silently invalidates every WS token,
/// voice-join token, and federation signature this server has ever issued;
/// production never wants that surprise. Dev profiles still tolerate the
/// fallback with a loud warning, matching previous behaviour.
fn resolve_signing_key(db_path: &str) -> Result<SigningKey, StartupError> {
    let is_production = matches!(
        std::env::var("ANNEX_BUILD_PROFILE")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "production" | "release"
    );

    // 1. Check environment variable
    if let Ok(hex_key) = std::env::var("ANNEX_SIGNING_KEY") {
        let bytes = hex::decode(&hex_key)
            .map_err(|e| StartupError::InvalidSigningKey(format!("not valid hex: {e}")))?;
        let byte_array: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
            StartupError::InvalidSigningKey(format!("expected 32 bytes, got {}", v.len()))
        })?;
        // Under production, refuse all-zero / obviously-weak keys.
        if is_production && is_weak_signing_key_bytes(&byte_array) {
            return Err(StartupError::WeakSigningKey {
                origin: "ANNEX_SIGNING_KEY environment variable".to_string(),
            });
        }
        tracing::info!("loaded signing key from ANNEX_SIGNING_KEY environment variable");
        return Ok(SigningKey::from_bytes(&byte_array));
    }

    // 2. Check persistent key file
    let data_dir = std::path::Path::new(db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let key_file = data_dir.join("signing.key");

    if key_file.exists() {
        match std::fs::read_to_string(&key_file) {
            Ok(hex_key) => {
                let hex_key = hex_key.trim();
                match hex::decode(hex_key) {
                    Ok(bytes) if bytes.len() == 32 => {
                        let byte_array: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
                            StartupError::InvalidSigningKey(format!(
                                "expected 32 bytes, got {}",
                                v.len()
                            ))
                        })?;
                        if is_production && is_weak_signing_key_bytes(&byte_array) {
                            return Err(StartupError::WeakSigningKey {
                                origin: format!("signing key file {}", key_file.display()),
                            });
                        }
                        tracing::info!(path = %key_file.display(), "loaded signing key from persistent file");
                        return Ok(SigningKey::from_bytes(&byte_array));
                    }
                    _ => {
                        tracing::warn!(path = %key_file.display(), "signing key file exists but is malformed — generating new key");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(path = %key_file.display(), error = %e, "could not read signing key file — generating new key");
            }
        }
    }

    // 3. Generate a new key and persist it
    let key = SigningKey::generate(&mut OsRng);
    let hex_key = hex::encode(key.to_bytes());

    // Ensure the parent directory exists before writing.
    if let Err(e) = std::fs::create_dir_all(data_dir) {
        tracing::warn!(
            path = %data_dir.display(),
            error = %e,
            "could not create data directory for signing key"
        );
        if is_production {
            return Err(StartupError::EphemeralSigningKeyInProduction {
                path: key_file.display().to_string(),
                reason: format!("could not create data directory: {e}"),
            });
        }
    }

    match std::fs::write(&key_file, &hex_key) {
        Ok(()) => {
            // Set file permissions to owner-only (0600) on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600));
            }
            tracing::info!(path = %key_file.display(), "generated and persisted new signing key");
        }
        Err(e) => {
            if is_production {
                return Err(StartupError::EphemeralSigningKeyInProduction {
                    path: key_file.display().to_string(),
                    reason: e.to_string(),
                });
            }
            tracing::warn!(
                path = %key_file.display(),
                error = %e,
                "could not persist signing key — using ephemeral key (federation identity will change on restart)"
            );
        }
    }

    Ok(key)
}

/// Identify obviously-weak signing keys that must not be accepted in
/// production. The all-zero key has no entropy at all; the all-`0xff` key
/// is the canonical "I forgot to randomise" sentinel. Returns `true` if
/// the bytes look like a placeholder rather than a real key.
fn is_weak_signing_key_bytes(bytes: &[u8; 32]) -> bool {
    let first = bytes[0];
    if bytes.iter().all(|&b| b == 0) {
        return true;
    }
    if bytes.iter().all(|&b| b == 0xff) {
        return true;
    }
    // All-same-byte (e.g. test fixtures full of 0xab) — at most 256 entropy
    // bits but no kept-secret randomness; reject in production.
    bytes.iter().all(|&b| b == first)
}

/// Returns a bound [`TcpListener`] and a fully-configured [`Router`]. The
/// caller is responsible for driving `axum::serve(listener, app)`.
///
/// Tracing must be initialized before calling this function (see [`init_tracing`]).
/// Applies `ANNEX_RATE_LIMIT_*` overrides to a loaded policy, in memory.
///
/// The three rate-limit categories live in `ServerPolicy`, which is stored in
/// the database and edited through `PUT /api/admin/policy`. That endpoint
/// requires an existing moderator, so there is otherwise no way to raise the
/// limits *before* the traffic that needs them — which is exactly what a
/// container deployment, a load test, or an automated UI audit needs to do.
///
/// The value is requests per minute, matching `ServerPolicy::rate_limit`. Note
/// that `0` does not mean "unlimited" — the limiter admits a request when the
/// windowed count is `<= limit`, so `0` rejects everything in that category.
/// To effectively remove a limit, set a large number.
///
/// Unparseable values are logged and ignored rather than failing startup: a
/// typo in an optional tuning knob should not take a server down.
fn apply_rate_limit_env_overrides(policy: &mut ServerPolicy) {
    /// Picks the limit field an override applies to.
    type LimitField = fn(&mut ServerPolicy) -> &mut u32;

    const OVERRIDES: [(&str, LimitField); 3] = [
        ("ANNEX_RATE_LIMIT_REGISTRATION", |p| {
            &mut p.rate_limit.registration_limit
        }),
        ("ANNEX_RATE_LIMIT_VERIFICATION", |p| {
            &mut p.rate_limit.verification_limit
        }),
        ("ANNEX_RATE_LIMIT_DEFAULT", |p| {
            &mut p.rate_limit.default_limit
        }),
    ];

    for (var, field) in OVERRIDES {
        let Ok(raw) = std::env::var(var) else {
            continue;
        };
        match raw.trim().parse::<u32>() {
            Ok(value) => {
                *field(policy) = value;
                tracing::info!(env = var, value, "rate limit overridden from environment");
            }
            Err(e) => {
                tracing::warn!(
                    env = var,
                    value = %raw,
                    error = %e,
                    "ignoring unparseable rate-limit override (expected a non-negative integer)"
                );
            }
        }
    }
}

pub async fn prepare_server(config: config::Config) -> Result<(TcpListener, Router), StartupError> {
    // Initialize database
    let pool = annex_db::create_pool(
        &config.database.path,
        annex_db::DbRuntimeSettings {
            busy_timeout_ms: config.database.busy_timeout_ms,
            pool_max_size: config.database.pool_max_size,
        },
    )?;

    {
        let conn = pool.get()?;
        let applied = annex_db::run_migrations(&conn)?;
        if applied > 0 {
            tracing::info!(count = applied, "applied database migrations");
        }

        // Repair the event-log hash chain for databases upgraded across
        // migration 038 (which added the chain columns with empty
        // defaults for pre-existing rows). Idempotent and a no-op on
        // healthy databases; runs before the server accepts traffic so no
        // new events are emitted against a broken chain.
        match annex_observe::backfill_event_log_chain(&conn) {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                servers = n,
                "rebuilt event-log hash chain for upgraded databases"
            ),
            Err(e) => tracing::error!("event-log hash-chain backfill failed: {}", e),
        }
    }

    // Start background retention task
    let retention_handle = tokio::spawn(retention::start_retention_task(
        pool.clone(),
        config.server.retention_check_interval_seconds,
        config.server.idempotency_ttl_seconds,
    ));
    tokio::spawn(async move {
        if let Err(e) = retention_handle.await {
            tracing::error!("retention background task panicked: {}", e);
        }
    });

    // Initialize Merkle Tree
    let tree = {
        let conn = pool.get()?;
        MerkleTree::restore(&conn, config.server.merkle_tree_depth)?
    };

    // Get Server ID, Policy, and persisted public URL (auto-seed if no server row exists)
    let (server_id, mut policy, db_public_url): (i64, ServerPolicy, String) = {
        let conn = pool.get()?;
        let existing = conn
            .query_row(
                "SELECT id, policy_json, public_url FROM servers LIMIT 1",
                [],
                |row| {
                    let id: i64 = row.get(0)?;
                    let policy_json: String = row.get(1)?;
                    let public_url: String = row.get(2)?;
                    let policy: ServerPolicy = serde_json::from_str(&policy_json).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                    Ok((id, policy, public_url))
                },
            )
            .optional()?;

        match existing {
            Some(row) => row,
            None => {
                tracing::info!("no server configured — seeding default server record");
                let slug = if config.server.server_slug.is_empty() {
                    std::env::var("ANNEX_SERVER_SLUG").unwrap_or_else(|_| "default".to_string())
                } else {
                    config.server.server_slug.clone()
                };
                let label = std::env::var("ANNEX_SERVER_LABEL")
                    .unwrap_or_else(|_| "Annex Server".to_string());
                let default_policy = ServerPolicy::default();
                let policy_json = serde_json::to_string(&default_policy)
                    .expect("ServerPolicy::default() contains only primitive types and cannot fail serialization");
                conn.execute(
                    "INSERT INTO servers (slug, label, policy_json) VALUES (?1, ?2, ?3)",
                    rusqlite::params![slug, label, &policy_json],
                )?;
                let id = conn.last_insert_rowid();

                // Seed a default #general text channel so the first user has
                // somewhere to chat immediately after identity creation.
                let general_id = uuid::Uuid::new_v4().to_string();
                let channel_type_json = serde_json::to_string(&annex_types::ChannelType::Text)
                    .expect("ChannelType::Text serialization cannot fail");
                let scope_json = serde_json::to_string(&annex_types::FederationScope::Local)
                    .expect("FederationScope::Local serialization cannot fail");
                match conn.execute(
                    "INSERT INTO channels (
                        server_id, channel_id, name, channel_type, topic, federation_scope
                    ) VALUES (?1, ?2, 'General', ?3, 'Welcome to Annex!', ?4)",
                    rusqlite::params![id, general_id, channel_type_json, scope_json],
                ) {
                    Ok(_) => {
                        tracing::info!(channel_id = %general_id, "seeded default #General channel")
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to seed default channel (non-fatal)")
                    }
                }

                (id, default_policy, String::new())
            }
        }
    };

    // Rate limits are otherwise only reachable through `PUT /api/admin/policy`,
    // which needs a moderator to already exist — so an automated deployment,
    // load test or browser-driven audit has no way to raise them before the
    // traffic that needs them starts. These env overrides close that gap.
    //
    // In-memory only: they deliberately do NOT rewrite `servers.policy_json`,
    // so an operator's stored policy survives and the override disappears when
    // the variable does.
    apply_rate_limit_env_overrides(&mut policy);

    // Say plainly when the outbox will retry past the point a peer can accept.
    //
    // These two numbers are configured independently and only make sense
    // together: retries beyond the freshness window are not "less likely to
    // succeed", they cannot succeed, because the envelope's signed
    // `created_at` never moves and the live ingest endpoint rejects on age.
    {
        let fed = &config.federation;
        let usable =
            crate::background::attempts_within_freshness_window(fed.freshness_window_seconds);
        if fed.outbox_max_attempts > usable {
            tracing::warn!(
                outbox_max_attempts = fed.outbox_max_attempts,
                freshness_window_seconds = fed.freshness_window_seconds,
                usable_attempts = usable,
                "federation outbox will retry past the receiver's freshness \
                 window: attempts beyond {usable} are rejected as too old, not \
                 merely likely to fail. Lower `outbox_max_attempts` or raise \
                 `freshness_window_seconds`.",
                usable = usable,
            );
        }
    }

    // Say plainly when agent alignment is not being enforced.
    //
    // `agent_min_alignment_score` reads like a working gate, and with no
    // declared principles there is nothing to score against — an agent's
    // anchor is admitted on the strength of its prohibited actions alone.
    // That is the right behaviour for a server that has stated no values
    // (see `compare_peer_anchor_scored`), but it is not what the threshold
    // in the policy suggests is happening, so it should not be silent.
    if policy.principles.is_empty() {
        tracing::warn!(
            min_alignment_score = policy.agent_min_alignment_score,
            "server policy declares no `principles`: agent semantic alignment \
             is NOT enforced and `agent_min_alignment_score` has no effect. \
             Declare principles via PUT /api/admin/policy to turn it on."
        );
    }

    // Load ZK verification key.
    //
    // Priority:
    // 1. ANNEX_ZK_KEY_PATH env var (explicit path)
    // 2. Default path: zk/keys/membership_vkey.json
    //
    // Behaviour when the file is missing or invalid depends on
    // `config.security.enforce_zk_proofs`:
    //   - enforced (default): a missing OR invalid key is `StartupError`.
    //     The dummy verification key is never used in this mode.
    //   - unenforced: a missing key falls back to the dummy verification key
    //     with a loud warning. Invalid (file present but unparseable) is
    //     still `StartupError` because it signals corruption / tampering and
    //     is distinct from "no key configured yet". `generate_dummy_vkey()`
    //     remains exported for tests that explicitly construct an
    //     `AppState` with `enforce_zk_proofs = false`.
    let vkey_path = std::env::var("ANNEX_ZK_KEY_PATH")
        .unwrap_or_else(|_| "zk/keys/membership_vkey.json".to_string());
    let enforce_zk_proofs = config.security.enforce_zk_proofs;
    let membership_vkey = match std::fs::read_to_string(&vkey_path) {
        Ok(vkey_json) => {
            let parsed = annex_identity::zk::parse_verification_key(&vkey_json)
                .map_err(StartupError::ZkError)?;
            // Defence in depth: even if the file parses, refuse to start with a
            // dummy vkey under enforcement. A dummy vkey would silently accept
            // every membership proof.
            if enforce_zk_proofs && annex_identity::zk::is_dummy_vkey(&parsed) {
                return Err(StartupError::DummyVerificationKey {
                    path: vkey_path.clone(),
                });
            }
            parsed
        }
        Err(e) => {
            if enforce_zk_proofs {
                return Err(StartupError::MissingVerificationKey {
                    path: vkey_path.clone(),
                    reason: e.to_string(),
                });
            }
            tracing::warn!(
                path = %vkey_path,
                error = %e,
                "ZK verification key not found — using dummy key. \
                 IDENTITY SECURITY IS DISABLED on this server: \
                 security.enforce_zk_proofs is false, raw pseudonym auth is \
                 permitted, and the dummy key cannot verify any real proof. \
                 This must only happen in development or test runs. \
                 To restore enforcement: set security.enforce_zk_proofs = true \
                 (the default) and provide a real key (e.g. via ANNEX_ZK_KEY_PATH \
                 or `node zk/scripts/dev-setup-groth16.js`)."
            );
            annex_identity::zk::generate_dummy_vkey()
        }
    };

    // Validate and load v2 vkey if v2 is enabled.
    //
    // Recognised versions: "v1" (always implicit; matched the file loaded
    // above) and "v2" (secret-derived nullifier; loaded here only when
    // explicitly enabled by `Config::security.enabled_zk_versions`).
    //
    // Path priority for v2: ANNEX_ZK_KEY_PATH_V2 env var, otherwise
    // `zk/keys/membership_v2_vkey.json`. Same enforcement rules as v1: if
    // `enforce_zk_proofs` is true and v2 is enabled, a missing or invalid
    // v2 vkey is a hard `StartupError`. If v2 is NOT enabled, this block
    // does nothing — the server simply rejects v2 payloads at request
    // time.
    let mut v2_enabled = false;
    for ver in &config.security.enabled_zk_versions {
        match ver.as_str() {
            "v1" => {}
            "v2" => v2_enabled = true,
            other => {
                return Err(StartupError::UnknownZkVersion {
                    version: other.to_string(),
                });
            }
        }
    }
    let membership_vkey_v2 = if v2_enabled {
        let path_v2 = std::env::var("ANNEX_ZK_KEY_PATH_V2")
            .unwrap_or_else(|_| "zk/keys/membership_v2_vkey.json".to_string());
        match std::fs::read_to_string(&path_v2) {
            Ok(vkey_json) => {
                let parsed = annex_identity::zk::parse_verification_key(&vkey_json)
                    .map_err(StartupError::ZkError)?;
                // Same defence-in-depth gate as v1: refuse a dummy v2 vkey under
                // enforcement.
                if enforce_zk_proofs && annex_identity::zk::is_dummy_vkey(&parsed) {
                    return Err(StartupError::DummyVerificationKey { path: path_v2 });
                }
                Some(Arc::new(parsed))
            }
            Err(e) => {
                if enforce_zk_proofs {
                    return Err(StartupError::MissingVerificationKey {
                        path: path_v2,
                        reason: format!("(membership v2) {e}"),
                    });
                }
                tracing::warn!(
                    path = %path_v2,
                    error = %e,
                    "v2 ZK verification key not found — using dummy key for v2. \
                     IDENTITY SECURITY IS DISABLED for v2 proofs on this server. \
                     security.enforce_zk_proofs is false. \
                     To restore enforcement: set security.enforce_zk_proofs = true \
                     and provide the v2 key at ANNEX_ZK_KEY_PATH_V2 (or run \
                     `node zk/scripts/dev-setup-groth16.js`)."
                );
                Some(Arc::new(annex_identity::zk::generate_dummy_vkey()))
            }
        }
    } else {
        None
    };

    // Load the capability/linkage/federation ZK circuit verification keys
    // (AUDIT P4-ID-1). These three circuits prove, in zero knowledge:
    //   - channel-eligibility: role-gated channel access with identity hidden
    //   - link-pseudonyms:      voluntary same-identity linkage across topics
    //   - federation-attestation: a hidden member attesting in a federation ctx
    //
    // They follow the SAME enforcement contract as the membership keys: when
    // `enforce_zk_proofs` is on, a present-but-dummy key is refused; a missing
    // key is a hard `StartupError`. The default key paths sit beside the
    // membership keys and can be overridden per-circuit by env var.
    let channel_eligibility_vkey = load_circuit_vkey(
        "ANNEX_ZK_KEY_PATH_CHANNEL_ELIGIBILITY",
        "zk/keys/channel_eligibility_vkey.json",
        "channel-eligibility",
        enforce_zk_proofs,
    )?;
    let link_pseudonyms_vkey = load_circuit_vkey(
        "ANNEX_ZK_KEY_PATH_LINK_PSEUDONYMS",
        "zk/keys/link_pseudonyms_vkey.json",
        "link-pseudonyms",
        enforce_zk_proofs,
    )?;
    let federation_attestation_vkey = load_circuit_vkey(
        "ANNEX_ZK_KEY_PATH_FEDERATION_ATTESTATION",
        "zk/keys/federation_attestation_vkey.json",
        "federation-attestation",
        enforce_zk_proofs,
    )?;

    // Load or generate Signing Key.
    // Priority: (1) ANNEX_SIGNING_KEY env var, (2) persistent file on disk, (3) generate + persist.
    let signing_key = resolve_signing_key(&config.database.path)?;

    // Create broadcast channels
    let (presence_tx, _) =
        tokio::sync::broadcast::channel(config.server.presence_broadcast_capacity);
    let (observe_tx, _) = tokio::sync::broadcast::channel(256);

    // Ensure WebRTC is running (auto-starts in dev mode if needed).
    // The child handle must be kept alive for the server's lifetime; it is
    // dropped when the server shuts down.
    let _webrtc_child = ensure_webrtc_running(&config.webrtc).await;

    // Initialize Voice / TTS / STT services
    let voice_service = annex_voice::VoiceService::new(config.webrtc);

    // Tell the voice service where this server is actually reachable.
    //
    // The SFU runs in-process and its signalling rides the app's own
    // WebSocket, so the address a remote client needs for voice is simply
    // the address it is already talking to. `WebRtcConfig.url` survives from
    // when the SFU was a separate LiveKit process; nothing dials it now, but
    // `get_public_url()` still gates on it, and it defaults to
    // `ws://localhost:7880` — a loopback address, which that method blanks
    // on purpose because handing a remote client a loopback URL is useless.
    //
    // The effect was that `voice_enabled` defaults to true and voice was
    // nonetheless refused for every client except one on the host machine:
    // `join_voice` sees an empty URL and returns `VoiceNotConfigured`.
    // Setting ANNEX_PUBLIC_URL — the documented way to say where the server
    // lives — did not help, because the resolved value was only ever written
    // into AppState and the sole caller of `set_public_url` was the admin
    // route. An operator had to configure the server correctly AND then make
    // an authenticated PUT before anyone could hold a call.
    let effective_public_url = if config.server.public_url.is_empty() {
        db_public_url.clone()
    } else {
        config.server.public_url.clone()
    };
    if !effective_public_url.is_empty() {
        voice_service.set_public_url(effective_public_url.clone());
    }
    let tts_service = annex_voice::TtsService::new(
        &config.voice.tts_voices_dir,
        &config.voice.tts_binary_path,
        &config.voice.bark_binary_path,
    );
    // Provision the built-in "default" profile and load operator-configured
    // profiles from the `voice_profiles` table into the running TtsService.
    // Without this, the WS voice handler's `"default"` fallback (and any
    // operator profile) resolved to nothing and agent synthesis failed with
    // ProfileNotFound — agent voice could never produce audio (AUDIT P4-VOICE-3).
    let default_voice_model = tts_service.provision_default_profile().await;
    let loaded_profiles = load_voice_profiles(&pool, server_id, &tts_service).await;
    tracing::info!(
        default_backend = ?default_voice_model,
        operator_profiles = loaded_profiles,
        "TTS voice profiles provisioned"
    );
    let stt_service =
        annex_voice::SttService::new(&config.voice.stt_model_path, &config.voice.stt_binary_path);

    // Resolve upload directory
    let upload_dir =
        std::env::var("ANNEX_UPLOAD_DIR").unwrap_or_else(|_| "data/uploads".to_string());
    if let Err(e) = std::fs::create_dir_all(&upload_dir) {
        tracing::warn!(path = %upload_dir, "failed to create upload directory: {}", e);
    } else {
        tracing::info!(path = %upload_dir, "upload directory ready");
    }

    let ws_token_secret = api_ws::derive_ws_token_secret(&signing_key);
    let voice_token_secret = annex_voice::derive_voice_token_secret(&signing_key);

    let storage_health = Arc::new(crate::storage_health::StorageHealth::new());
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(membership_vkey),
        membership_vkey_v2: membership_vkey_v2.clone(),
        channel_eligibility_vkey,
        link_pseudonyms_vkey,
        federation_attestation_vkey,
        server_id,
        signing_key: Arc::new(signing_key),
        // Config/env public_url takes precedence; fall back to DB-persisted
        // value. Resolved above as `effective_public_url` so the voice
        // service is told the same thing — the two drifting apart is what
        // made voice unreachable for remote clients on a correctly
        // configured server.
        public_url: Arc::new(RwLock::new(effective_public_url)),
        policy: Arc::new(RwLock::new(policy)),
        rate_limiter: RateLimiter::new(),
        connection_manager: api_ws::ConnectionManager::new(),
        presence_tx,
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(tts_service),
        stt_service: Arc::new(stt_service),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx,
        upload_dir,
        preview_cache: api_link_preview::PreviewCache::new(),
        ws_token_secret: Arc::new(ws_token_secret),
        voice_token_secret: Arc::new(voice_token_secret),
        cors_origins: config.cors.allowed_origins.clone(),
        enforce_zk_proofs: config.security.enforce_zk_proofs,
        invite_base_url: config.server.invite_base_url.clone(),
        federation_config: config.federation.clone(),
        storage_config: config.storage.clone(),
        storage_health,
        trusted_proxy_depth: config.deployment.trusted_proxy_depth,
    };

    // Start background pruning task
    let pruning_handle = tokio::spawn(background::start_pruning_task(
        Arc::new(state.clone()),
        config.server.inactivity_threshold_seconds,
    ));
    tokio::spawn(async move {
        if let Err(e) = pruning_handle.await {
            tracing::error!("pruning background task panicked: {}", e);
        }
    });

    // Start rate limiter cleanup task
    tokio::spawn(background::start_rate_limit_cleanup_task(
        state.rate_limiter.clone(),
    ));

    // Start federation outbox worker (replaces the pre-hardening
    // fire-and-forget `relay_message` spawn — see migration 037 and
    // ADR-0007 / ADR-0008 for the durability rationale).
    tokio::spawn(background::start_federation_outbox_task(Arc::new(
        state.clone(),
    )));

    // Start SQLite maintenance worker if enabled. The worker is a no-op
    // when `storage.maintenance_enabled = false`; we still spawn it so
    // an operator can flip the flag without restarting.
    tokio::spawn(background::start_db_maintenance_task(Arc::new(
        state.clone(),
    )));

    // Start the proactive storage probe. Returns immediately when
    // `storage.max_db_bytes` is 0 (the default), so it is always safe to
    // spawn.
    tokio::spawn(background::start_storage_probe_task(
        Arc::new(state.clone()),
        std::path::PathBuf::from(&config.database.path),
    ));

    // Auto-expire silent federation agreements past their TTL
    // (`federation.agreement_ttl_days`; 0 disables). Returns immediately when
    // disabled, so it is always safe to spawn.
    tokio::spawn(background::start_federation_agreement_expiry_task(
        Arc::new(state.clone()),
    ));

    // Build application
    let router = routes::app(state);
    let addr = SocketAddr::new(config.server.host, config.server.port);

    tracing::info!(%addr, "starting annex server");

    let listener = TcpListener::bind(addr).await.map_err(|e| {
        tracing::error!(%addr, "failed to bind to address — is another process using this port?");
        StartupError::IoError(e)
    })?;

    Ok((listener, router))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the env-var tests. `std::env::set_var` is process-global, so
    /// two of these running concurrently would see each other's values.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| ((*k).to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        let out = f();
        for (k, v) in saved {
            match v {
                Some(val) => std::env::set_var(&k, val),
                None => std::env::remove_var(&k),
            }
        }
        out
    }

    #[test]
    fn rate_limit_overrides_are_applied_from_env() {
        let policy = with_env(
            &[
                ("ANNEX_RATE_LIMIT_REGISTRATION", Some("55")),
                ("ANNEX_RATE_LIMIT_VERIFICATION", Some("66")),
                ("ANNEX_RATE_LIMIT_DEFAULT", Some("7777")),
            ],
            || {
                let mut p = ServerPolicy::default();
                apply_rate_limit_env_overrides(&mut p);
                p
            },
        );

        assert_eq!(policy.rate_limit.registration_limit, 55);
        assert_eq!(policy.rate_limit.verification_limit, 66);
        assert_eq!(policy.rate_limit.default_limit, 7777);
    }

    #[test]
    fn rate_limit_defaults_survive_when_unset() {
        let defaults = ServerPolicy::default();
        let policy = with_env(
            &[
                ("ANNEX_RATE_LIMIT_REGISTRATION", None),
                ("ANNEX_RATE_LIMIT_VERIFICATION", None),
                ("ANNEX_RATE_LIMIT_DEFAULT", None),
            ],
            || {
                let mut p = ServerPolicy::default();
                apply_rate_limit_env_overrides(&mut p);
                p
            },
        );

        assert_eq!(policy.rate_limit, defaults.rate_limit);
    }

    #[test]
    fn unparseable_rate_limit_override_is_ignored_not_fatal() {
        // A typo in an optional tuning knob must not take the server down, and
        // must not silently reinterpret itself as some other number.
        let defaults = ServerPolicy::default();
        let policy = with_env(
            &[
                ("ANNEX_RATE_LIMIT_DEFAULT", Some("lots")),
                ("ANNEX_RATE_LIMIT_REGISTRATION", Some("-5")),
                ("ANNEX_RATE_LIMIT_VERIFICATION", None),
            ],
            || {
                let mut p = ServerPolicy::default();
                apply_rate_limit_env_overrides(&mut p);
                p
            },
        );

        assert_eq!(
            policy.rate_limit.default_limit,
            defaults.rate_limit.default_limit
        );
        assert_eq!(
            policy.rate_limit.registration_limit,
            defaults.rate_limit.registration_limit
        );
    }
}

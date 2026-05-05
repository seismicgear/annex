//! Annex server library logic.

pub mod api;
pub mod api_admin;
pub mod api_agent;
pub mod api_channels;
pub mod api_federation;
pub mod api_graph;
pub mod api_invite;
pub mod api_link_preview;
pub mod api_observe;
pub mod api_rtx;
pub mod api_sse;
pub mod api_upload;
pub mod api_usernames;
pub mod api_vrp;
pub mod api_ws;
pub mod background;
pub mod config;
pub mod middleware;
pub mod policy;
pub mod retention;
pub mod services;

use annex_db::DbPool;
use annex_identity::zk::{Bn254, VerifyingKey};
use annex_identity::MerkleTree;
use annex_types::ServerPolicy;
use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post, put},
    Extension, Json, Router,
};
use ed25519_dalek::SigningKey;
use middleware::RateLimiter;
use rand::rngs::OsRng;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

/// Application state shared across all request handlers.
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool.
    pub pool: DbPool,
    /// In-memory Merkle tree state.
    pub merkle_tree: Arc<Mutex<MerkleTree>>,
    /// ZK Membership verification key (v1 — commitment-derived nullifier).
    pub membership_vkey: Arc<VerifyingKey<Bn254>>,
    /// ZK Membership verification key (v2 — secret-derived nullifier).
    ///
    /// `Some` only when `"v2"` is in `Config::security.enabled_zk_versions`
    /// at boot. The server uses this to verify proofs whose `protocol_version`
    /// field is `"v2"`; an incoming `"v2"` payload on a server where this is
    /// `None` is rejected with `409 Conflict`. v1 and v2 are NEVER merged
    /// silently — each proof is dispatched to exactly one verifier by its
    /// declared protocol version.
    pub membership_vkey_v2: Option<Arc<VerifyingKey<Bn254>>>,
    /// The local server ID.
    pub server_id: i64,
    /// The local server signing key (Ed25519).
    pub signing_key: Arc<SigningKey>,
    /// The public URL of the server.
    ///
    /// Wrapped in `Arc<RwLock<_>>` so that when no explicit URL is configured,
    /// the server can auto-detect it from the first incoming HTTP request's
    /// `Host` / `X-Forwarded-Host` headers.
    pub public_url: Arc<RwLock<String>>,
    /// Server policy configuration.
    pub policy: Arc<RwLock<ServerPolicy>>,
    /// Rate limiter state.
    pub rate_limiter: RateLimiter,
    /// Connection manager for WebSockets.
    pub connection_manager: api_ws::ConnectionManager,
    /// Broadcast channel for presence events.
    pub presence_tx: broadcast::Sender<annex_types::PresenceEvent>,
    /// Voice service.
    pub voice_service: Arc<annex_voice::VoiceService>,
    /// TTS service.
    pub tts_service: Arc<annex_voice::TtsService>,
    /// STT service.
    pub stt_service: Arc<annex_voice::SttService>,
    /// Active agent voice sessions (pseudonym -> client).
    ///
    /// Uses `std::sync::RwLock` intentionally: all lock acquisitions are brief
    /// HashMap operations (get/insert/remove) that never span `.await` points,
    /// making a synchronous lock safe and more efficient than `tokio::sync::RwLock`.
    pub voice_sessions:
        Arc<RwLock<std::collections::HashMap<String, Arc<annex_voice::AgentVoiceClient>>>>,
    /// Broadcast channel for public observe events (SSE stream).
    pub observe_tx: broadcast::Sender<annex_observe::PublicEvent>,
    /// Directory for uploaded files (images, etc.).
    pub upload_dir: String,
    /// In-memory cache for link preview metadata and proxied images.
    pub preview_cache: api_link_preview::PreviewCache,
    /// HMAC secret for signing WebSocket session tokens. Derived at startup
    /// from the server's Ed25519 key to avoid managing a separate secret.
    pub ws_token_secret: Arc<[u8; 32]>,
    /// Configured CORS allowed origins (empty = same-origin only, ["*"] = permissive).
    pub cors_origins: Vec<String>,
    /// When true, channel access endpoints require ZK membership proof via
    /// the `x-annex-zk-proof` header.
    pub enforce_zk_proofs: bool,
    /// Base URL for generated invite links (e.g. "https://monolithannex.com/invite").
    pub invite_base_url: String,
}

impl AppState {
    /// Returns the current public URL, or an empty string if not yet detected.
    pub fn get_public_url(&self) -> String {
        self.public_url
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// Emits an observe event to the database and broadcasts it to the SSE stream.
///
/// This is a convenience wrapper that calls [`annex_observe::emit_event`] and,
/// on success, sends the resulting [`annex_observe::PublicEvent`] through the
/// broadcast channel. Failures are logged as warnings but never block the
/// caller.
pub fn emit_and_broadcast(
    conn: &rusqlite::Connection,
    server_id: i64,
    entity_id: &str,
    payload: &annex_observe::EventPayload,
    observe_tx: &broadcast::Sender<annex_observe::PublicEvent>,
) {
    let domain = payload.domain();
    match annex_observe::emit_event(
        conn,
        server_id,
        domain,
        payload.event_type(),
        payload.entity_type(),
        entity_id,
        payload,
    ) {
        Ok(event) => {
            if let Err(e) = observe_tx.send(event) {
                tracing::warn!(
                    domain = domain.as_str(),
                    event_type = payload.event_type(),
                    "observe broadcast channel send failed (no receivers or lagged): {}",
                    e
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                domain = domain.as_str(),
                event_type = payload.event_type(),
                "failed to emit observe event: {}",
                e
            );
        }
    }
}

/// Parses a transfer scope string from the database into a [`VrpTransferScope`].
///
/// Returns `None` for unrecognized strings.
pub(crate) fn parse_transfer_scope(s: &str) -> Option<annex_vrp::VrpTransferScope> {
    s.parse().ok()
}

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
    /// `Config::security.enabled_zk_versions` listed a value other than the
    /// recognised set (`"v1"`, `"v2"`).
    #[error(
        "unknown ZK protocol version '{version}' in security.enabled_zk_versions \
         (recognised values: \"v1\", \"v2\")"
    )]
    UnknownZkVersion { version: String },
}

/// Native WebRTC is embedded in-process; no external sidecar startup is required.
async fn ensure_webrtc_running(
    _config: &annex_voice::WebRtcConfig,
) -> Option<tokio::process::Child> {
    None
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

/// Prepares the server: loads database, initializes state, starts background
/// tasks, and binds the TCP listener.
///
/// Resolve the Ed25519 signing key for federation identity.
///
/// Priority:
/// 1. `ANNEX_SIGNING_KEY` environment variable (64-char hex)
/// 2. Persistent key file at `{data_dir}/signing.key`
/// 3. Generate a new key and write it to `{data_dir}/signing.key`
///
/// Falls back to an ephemeral key (with warning) only if the file cannot be written.
fn resolve_signing_key(db_path: &str) -> Result<SigningKey, StartupError> {
    // 1. Check environment variable
    if let Ok(hex_key) = std::env::var("ANNEX_SIGNING_KEY") {
        let bytes = hex::decode(&hex_key)
            .map_err(|e| StartupError::InvalidSigningKey(format!("not valid hex: {e}")))?;
        let byte_array: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
            StartupError::InvalidSigningKey(format!("expected 32 bytes, got {}", v.len()))
        })?;
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
            tracing::warn!(
                path = %key_file.display(),
                error = %e,
                "could not persist signing key — using ephemeral key (federation identity will change on restart)"
            );
        }
    }

    Ok(key)
}

/// Returns a bound [`TcpListener`] and a fully-configured [`Router`]. The
/// caller is responsible for driving `axum::serve(listener, app)`.
///
/// Tracing must be initialized before calling this function (see [`init_tracing`]).
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
    }

    // Start background retention task
    let retention_handle = tokio::spawn(retention::start_retention_task(
        pool.clone(),
        config.server.retention_check_interval_seconds,
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
    let (server_id, policy, db_public_url): (i64, ServerPolicy, String) = {
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
            annex_identity::zk::parse_verification_key(&vkey_json).map_err(StartupError::ZkError)?
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
            Ok(vkey_json) => Some(Arc::new(
                annex_identity::zk::parse_verification_key(&vkey_json)
                    .map_err(StartupError::ZkError)?,
            )),
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
    let tts_service = annex_voice::TtsService::new(
        &config.voice.tts_voices_dir,
        &config.voice.tts_binary_path,
        &config.voice.bark_binary_path,
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

    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(membership_vkey),
        membership_vkey_v2: membership_vkey_v2.clone(),
        server_id,
        signing_key: Arc::new(signing_key),
        // Config/env public_url takes precedence; fall back to DB-persisted value
        public_url: Arc::new(RwLock::new(if config.server.public_url.is_empty() {
            db_public_url
        } else {
            config.server.public_url.clone()
        })),
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
        cors_origins: config.cors.allowed_origins.clone(),
        enforce_zk_proofs: config.security.enforce_zk_proofs,
        invite_base_url: config.server.invite_base_url.clone(),
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

    // Build application
    let router = app(state);
    let addr = SocketAddr::new(config.server.host, config.server.port);

    tracing::info!(%addr, "starting annex server");

    let listener = TcpListener::bind(addr).await.map_err(|e| {
        tracing::error!(%addr, "failed to bind to address — is another process using this port?");
        StartupError::IoError(e)
    })?;

    Ok((listener, router))
}

/// Maximum request body size (2 MiB). Protects against OOM from oversized payloads.
const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Health check handler.
///
/// Reports basic server liveness, version, and whether voice (WebRTC) is configured.
async fn health(Extension(state): Extension<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "voice_enabled": state.voice_service.is_enabled()
    }))
}

/// Voice configuration status (public, no auth required).
///
/// Reports both the server policy voice setting and whether the WebRTC
/// infrastructure is configured, so the client can distinguish between
/// "voice disabled by admin" and "voice enabled but needs WebRTC setup".
async fn voice_config_status(Extension(state): Extension<Arc<AppState>>) -> Json<Value> {
    let infrastructure_ready = state.voice_service.is_enabled();
    // get_public_url() now returns "" for loopback-only URLs, so
    // has_public_url is false when only a loopback endpoint exists.
    let has_public_url = !state.voice_service.get_public_url().is_empty();
    // Also report whether a URL for local clients exists (includes loopback)
    let has_local_url = !state.voice_service.get_url_for_local_client().is_empty();
    let policy_enabled = state
        .policy
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .voice_enabled;

    let setup_hint = if !policy_enabled {
        "Voice is disabled in the server policy. An admin can enable it in Server Policy settings."
    } else if !infrastructure_ready {
        "Voice is enabled by policy but WebRTC is not configured. Set webrtc.url, webrtc.api_key, and webrtc.api_secret in config.toml or use ANNEX_WEBRTC_* environment variables."
    } else if !has_public_url && has_local_url {
        "WebRTC is configured with a loopback-only URL. Voice works for the host but remote users who join via invite will not be able to connect to calls. Set webrtc.public_url in config.toml to a publicly reachable WebSocket address, or set ANNEX_WEBRTC_PUBLIC_URL."
    } else if !has_public_url {
        "WebRTC URL is configured but no public URL is set. Clients may not be able to connect."
    } else {
        "Voice is configured and ready."
    };

    Json(json!({
        "voice_enabled": policy_enabled && infrastructure_ready,
        "policy_enabled": policy_enabled,
        "infrastructure_ready": infrastructure_ready,
        "has_public_url": has_public_url,
        "has_local_url": has_local_url,
        "setup_hint": setup_hint
    }))
}

/// Builds the application router with all routes.
pub fn app(state: AppState) -> Router {
    let protected_routes = Router::new()
        .route(
            "/api/channels",
            post(api_channels::create_channel_handler).get(api_channels::list_channels_handler),
        )
        .route(
            "/api/channels/{channelId}",
            get(api_channels::get_channel_handler).delete(api_channels::delete_channel_handler),
        )
        .route(
            "/api/channels/{channelId}/join",
            post(api_channels::join_channel_handler),
        )
        .route(
            "/api/channels/{channelId}/voice/join",
            post(api_channels::join_voice_channel_handler),
        )
        .route(
            "/api/channels/{channelId}/voice/leave",
            post(api_channels::leave_voice_channel_handler),
        )
        .route(
            "/api/channels/{channelId}/voice/status",
            get(api_channels::voice_status_handler),
        )
        .route(
            "/api/channels/{channelId}/leave",
            post(api_channels::leave_channel_handler),
        )
        .route(
            "/api/messages/search",
            get(api_channels::search_messages_handler),
        )
        .route(
            "/api/channels/{channelId}/messages",
            get(api_channels::get_channel_history_handler),
        )
        .route(
            "/api/channels/{channelId}/messages/{messageId}/edits",
            get(api_channels::get_message_edits_handler),
        )
        .route(
            "/api/agents/{pseudonymId}",
            get(api_agent::get_agent_profile_handler),
        )
        .route(
            "/api/agents/{pseudonymId}/voice-profile",
            put(api_agent::update_agent_voice_profile_handler),
        )
        .route("/api/rtx/publish", post(api_rtx::publish_handler))
        .route(
            "/api/rtx/subscribe",
            post(api_rtx::subscribe_handler).delete(api_rtx::unsubscribe_handler),
        )
        .route(
            "/api/rtx/subscriptions",
            get(api_rtx::get_subscription_handler),
        )
        .route(
            "/api/rtx/governance/transfers",
            get(api_rtx::governance_transfers_handler),
        )
        .route(
            "/api/rtx/governance/summary",
            get(api_rtx::governance_summary_handler),
        )
        .route(
            "/api/admin/policy",
            get(api_admin::get_policy_handler).put(api_admin::update_policy_handler),
        )
        .route(
            "/api/admin/server",
            get(api_admin::get_server_handler).patch(api_admin::rename_server_handler),
        )
        .route(
            "/api/admin/public-url",
            put(api_admin::set_public_url_handler),
        )
        .route(
            "/api/admin/webrtc-public-url",
            put(api_admin::set_webrtc_public_url_handler),
        )
        .route(
            "/api/admin/federation/{id}",
            delete(api_admin::revoke_federation_handler),
        )
        .route("/api/admin/members", get(api_admin::list_members_handler))
        .route(
            "/api/admin/members/{pseudonymId}/capabilities",
            patch(api_admin::update_member_capabilities_handler),
        )
        .route(
            "/api/profile/username",
            put(api_usernames::set_username_handler).delete(api_usernames::delete_username_handler),
        )
        .route(
            "/api/profile/username/grant",
            post(api_usernames::grant_username_handler),
        )
        .route(
            "/api/profile/username/grant/{granteePseudonym}",
            delete(api_usernames::revoke_grant_handler),
        )
        .route(
            "/api/profile/username/grants",
            get(api_usernames::list_grants_handler),
        )
        .route(
            "/api/usernames/visible",
            get(api_usernames::get_visible_usernames_handler),
        )
        .route(
            "/api/link-preview",
            get(api_link_preview::link_preview_handler),
        )
        .route(
            "/api/invites",
            post(api_invite::create_invite_handler).get(api_invite::list_invites_handler),
        )
        .route(
            "/api/invites/{code}",
            delete(api_invite::delete_invite_handler),
        )
        .route("/api/ws/token", post(api_ws::create_ws_token_handler))
        .route(
            "/api/graph/profile/{targetPseudonym}",
            get(api_graph::get_profile_handler),
        )
        .route(
            "/events/presence",
            get(api_sse::get_presence_stream_handler),
        )
        .layer(axum::middleware::from_fn(middleware::auth_middleware));

    // Upload routes need a larger body limit for media uploads.
    // The hard ceiling is 50 MiB; the handler enforces per-category limits from policy.
    let upload_routes = Router::new()
        .route(
            "/api/admin/server/image",
            post(api_upload::upload_server_image_handler),
        )
        .route(
            "/api/channels/{channelId}/upload",
            post(api_upload::upload_chat_handler),
        )
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .layer(axum::middleware::from_fn(middleware::auth_middleware));

    let router = Router::new()
        .route("/health", get(health))
        .route("/api/registry/register", post(api::register_handler))
        .route(
            "/api/registry/path/{commitmentHex}",
            get(api::get_path_handler),
        )
        .route(
            "/api/registry/current-root",
            get(api::get_current_root_handler),
        )
        .route(
            "/api/zk/verify-membership",
            post(api::verify_membership_handler),
        )
        .route(
            "/api/session/refresh",
            post(api_ws::refresh_session_handler),
        )
        .route("/api/registry/topics", get(api::get_topics_handler))
        .route("/api/registry/roles", get(api::get_roles_handler))
        .route(
            "/api/identity/{pseudonymId}",
            get(api::get_identity_handler),
        )
        .route(
            "/api/identity/{pseudonymId}/capabilities",
            get(api::get_identity_capabilities_handler),
        )
        .route(
            "/api/vrp/agent-handshake",
            post(api_vrp::agent_handshake_handler),
        )
        .route(
            "/api/federation/handshake",
            post(api_federation::federation_handshake_handler),
        )
        .route(
            "/api/federation/vrp-root",
            get(api_federation::get_vrp_root_handler),
        )
        .route(
            "/api/federation/attest-membership",
            post(api_federation::attest_membership_handler),
        )
        .route(
            "/api/federation/channels",
            get(api_federation::get_federated_channels_handler),
        )
        .route(
            "/api/federation/channels/{channelId}/join",
            post(api_federation::join_federated_channel_handler),
        )
        .route(
            "/api/federation/messages",
            post(api_federation::receive_federated_message_handler),
        )
        .route(
            "/api/federation/rtx",
            post(api_federation::receive_federated_rtx_handler),
        )
        .route("/api/graph/degrees", get(api_graph::get_degrees_handler))
        .route("/api/public/events", get(api_observe::get_events_handler))
        .route("/events/stream", get(api_observe::get_event_stream_handler))
        .route(
            "/api/public/server/summary",
            get(api_observe::get_server_summary_handler),
        )
        .route(
            "/api/public/federation/peers",
            get(api_observe::get_federation_peers_handler),
        )
        .route("/api/public/agents", get(api_observe::get_agents_handler))
        .route(
            "/api/invites/redeem",
            post(api_invite::redeem_invite_handler),
        )
        .route("/api/voice/config-status", get(voice_config_status))
        .route(
            "/api/public/server/image",
            get(api_upload::get_server_image_handler),
        )
        // Image proxy lives outside auth — browsers load <img src="..."> without
        // custom headers.  The handler already validates URLs (SSRF, DNS rebinding,
        // content-type, size) and only proxies public images.
        .route(
            "/api/link-preview/image",
            get(api_link_preview::image_proxy_handler),
        )
        .merge(protected_routes)
        .merge(upload_routes)
        .route("/ws", get(api_ws::ws_handler));

    // Serve uploaded files (images, etc.) under /uploads/*
    let upload_dir = state.upload_dir.clone();
    let router = if std::path::Path::new(&upload_dir).exists() {
        tracing::info!(path = %upload_dir, "serving uploaded files at /uploads");
        router.nest_service("/uploads", ServeDir::new(&upload_dir))
    } else {
        tracing::info!(path = %upload_dir, "uploads directory not found yet (will be created on first upload)");
        router
    };

    // Serve client static files if the directory exists.
    // Configured via ANNEX_CLIENT_DIR env var; defaults to "client/dist".
    let client_dir =
        std::env::var("ANNEX_CLIENT_DIR").unwrap_or_else(|_| "client/dist".to_string());
    let client_dir = match std::fs::canonicalize(&client_dir) {
        Ok(abs) => {
            let s = abs.to_string_lossy().to_string();
            tracing::info!(original = %client_dir, resolved = %s, "canonicalized client directory path");
            s
        }
        Err(_) => {
            if !std::path::Path::new(&client_dir).is_absolute() {
                tracing::warn!(
                    path = %client_dir,
                    "ANNEX_CLIENT_DIR is relative and could not be canonicalized — \
                     static file serving depends on working directory"
                );
            }
            client_dir
        }
    };
    let router = if std::path::Path::new(&client_dir)
        .join("index.html")
        .exists()
    {
        tracing::info!(path = %client_dir, "serving client static files");
        let index = format!("{client_dir}/index.html");
        router.fallback_service(ServeDir::new(&client_dir).fallback(ServeFile::new(index)))
    } else {
        tracing::info!(path = %client_dir, "client directory not found, skipping static file serving");
        router
    };

    let cors_origins = state.cors_origins.clone();
    let shared_state = Arc::new(state);

    // Build CORS layer from configuration.
    // Default (empty origins): restrictive (same-origin only, no CORS headers).
    // "*": permissive (any origin). Explicit list: only those origins.
    //
    // Debug builds additionally accept any `http(s)://localhost[:port]`,
    // `http(s)://127.0.0.1[:port]`, or `http(s)://[::1][:port]` origin via
    // `is_dev_localhost_origin`. This exists so `cargo tauri dev` (Vite on
    // :5173) and `cargo run -p annex-server` can be hit from the local dev
    // server without hand-configuring `ANNEX_CORS_ORIGINS`. The relaxation is
    // compiled out of release builds — `cfg!(debug_assertions)` is `false`
    // under `cargo build --release`, so production binaries stay strict.
    let cors_layer = {
        let origins = &cors_origins;
        let is_permissive = origins.iter().any(|o| o == "*");
        let base = CorsLayer::new()
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::PUT,
                axum::http::Method::DELETE,
                axum::http::Method::PATCH,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers([
                axum::http::header::CONTENT_TYPE,
                axum::http::header::AUTHORIZATION,
                axum::http::HeaderName::from_static("x-annex-pseudonym"),
                axum::http::HeaderName::from_static("x-annex-zk-proof"),
            ]);

        if is_permissive {
            tracing::info!("CORS: permissive mode (allow all origins)");
            base.allow_origin(Any)
        } else {
            let parsed: Vec<axum::http::HeaderValue> = origins
                .iter()
                .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
                .collect();

            if cfg!(debug_assertions) {
                tracing::info!(
                    origins = ?origins,
                    "CORS: configured origins + any localhost (debug build)"
                );
                let allowed = parsed;
                base.allow_origin(AllowOrigin::predicate(move |origin, _req_parts| {
                    allowed.iter().any(|a| a == origin) || is_dev_localhost_origin(origin)
                }))
            } else if origins.is_empty() {
                tracing::info!("CORS: restrictive mode (same-origin only)");
                // No Access-Control-Allow-Origin header → browsers block cross-origin requests
                base.allow_origin(AllowOrigin::list(
                    std::iter::empty::<axum::http::HeaderValue>(),
                ))
            } else {
                tracing::info!(origins = ?origins, "CORS: restricted to configured origins");
                base.allow_origin(AllowOrigin::list(parsed))
            }
        }
    };

    router
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(axum::middleware::from_fn(
            middleware::security_headers_middleware,
        ))
        .layer(axum::middleware::from_fn(middleware::rate_limit_middleware))
        .layer(cors_layer)
        .layer(Extension(shared_state))
}

/// Returns `true` when the `Origin` header points at any local dev server:
/// `http(s)://localhost[:port]`, `http(s)://127.0.0.1[:port]`, or
/// `http(s)://[::1][:port]`.
///
/// Consulted only from debug builds (see the CORS layer in `app`). Release
/// binaries never trust this — the caller is gated on
/// `cfg!(debug_assertions)`.
pub(crate) fn is_dev_localhost_origin(origin: &axum::http::HeaderValue) -> bool {
    let Ok(raw) = origin.to_str() else {
        return false;
    };
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    // Origin headers are `scheme://host[:port]` — no path, query, or fragment.
    // `url::Url::parse` normalizes `http://localhost` to path `/`, so accept
    // that too.
    if !parsed.path().is_empty() && parsed.path() != "/" {
        return false;
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return false;
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return false;
    }
    matches!(
        parsed.host_str(),
        Some("localhost") | Some("127.0.0.1") | Some("[::1]")
    )
}

#[cfg(test)]
mod tests {
    use super::is_dev_localhost_origin;
    use axum::http::HeaderValue;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    #[test]
    fn accepts_localhost_with_any_port() {
        assert!(is_dev_localhost_origin(&hv("http://localhost:5173")));
        assert!(is_dev_localhost_origin(&hv("http://localhost:3000")));
        assert!(is_dev_localhost_origin(&hv("http://localhost:1")));
        assert!(is_dev_localhost_origin(&hv("http://localhost")));
        assert!(is_dev_localhost_origin(&hv("https://localhost:5173")));
    }

    #[test]
    fn accepts_loopback_ipv4_and_ipv6() {
        assert!(is_dev_localhost_origin(&hv("http://127.0.0.1:5173")));
        assert!(is_dev_localhost_origin(&hv("http://127.0.0.1")));
        assert!(is_dev_localhost_origin(&hv("http://[::1]:5173")));
        assert!(is_dev_localhost_origin(&hv("http://[::1]")));
    }

    #[test]
    fn rejects_non_loopback_hosts() {
        assert!(!is_dev_localhost_origin(&hv("https://example.com")));
        assert!(!is_dev_localhost_origin(&hv("http://evil.localhost")));
        assert!(!is_dev_localhost_origin(&hv("http://localhost.evil.com")));
        assert!(!is_dev_localhost_origin(&hv("http://127.0.0.2")));
        assert!(!is_dev_localhost_origin(&hv("http://10.0.0.1")));
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(!is_dev_localhost_origin(&hv("tauri://localhost")));
        assert!(!is_dev_localhost_origin(&hv("ws://localhost:5173")));
        assert!(!is_dev_localhost_origin(&hv("file://localhost")));
    }

    #[test]
    fn rejects_origins_with_path_or_query() {
        // A well-formed Origin header has no path/query; reject malformed inputs
        // so an attacker can't smuggle `http://localhost/@evil.com` past the check.
        assert!(!is_dev_localhost_origin(&hv("http://localhost:5173/steal")));
        assert!(!is_dev_localhost_origin(&hv("http://localhost:5173/?x=1")));
        assert!(!is_dev_localhost_origin(&hv(
            "http://user:pw@localhost:5173"
        )));
    }

    #[test]
    fn rejects_garbage() {
        assert!(!is_dev_localhost_origin(&hv("")));
        assert!(!is_dev_localhost_origin(&hv("not-a-url")));
        assert!(!is_dev_localhost_origin(&hv("http://")));
    }
}

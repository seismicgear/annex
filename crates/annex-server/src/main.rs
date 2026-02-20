//! Annex server binary — the main entry point for the Annex platform.
//!
//! Starts an axum HTTP server with structured logging, database initialization,
//! and graceful shutdown on SIGTERM/SIGINT.

use annex_identity::MerkleTree;
use annex_server::middleware::RateLimiter;
use annex_server::{app, config, AppState};
use annex_types::ServerPolicy;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rusqlite::OptionalExtension;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use thiserror::Error;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Error)]
enum StartupError {
    #[error("invalid logging.level '{value}': {reason}")]
    InvalidLoggingLevel { value: String, reason: String },
    #[error("failed to load configuration: {0}")]
    ConfigError(#[from] config::ConfigError),
    #[error("failed to initialize database pool: {0}")]
    DatabaseError(#[from] annex_db::PoolError),
    #[error("failed to initialize merkle tree: {0}")]
    IdentityError(#[from] annex_identity::IdentityError),
    #[error("failed to read verification key: {0}")]
    IoError(#[from] std::io::Error),
    #[error("failed to parse verification key: {0}")]
    ZkError(#[from] annex_identity::zk::ZkError),
    #[error("failed to get database connection from pool: {0}")]
    PoolConnection(#[from] r2d2::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] annex_db::MigrationError),
    #[error("database query failed: {0}")]
    DbQuery(#[from] rusqlite::Error),
    #[error("invalid ANNEX_SIGNING_KEY: {0}")]
    InvalidSigningKey(String),
}

fn resolve_config_path() -> (Option<String>, &'static str) {
    if let Some(path) = std::env::args()
        .nth(1)
        .filter(|value| !value.trim().is_empty())
    {
        return (Some(path), "cli-arg");
    }

    if let Ok(path) = std::env::var("ANNEX_CONFIG_PATH") {
        if !path.trim().is_empty() {
            return (Some(path), "env-var");
        }
    }

    (None, "default")
}

fn parse_logging_filter(level: &str) -> Result<EnvFilter, StartupError> {
    EnvFilter::try_new(level).map_err(|err| StartupError::InvalidLoggingLevel {
        value: level.to_string(),
        reason: err.to_string(),
    })
}

#[tokio::main]
async fn main() -> Result<(), StartupError> {
    let (resolved_config_path, config_source) = resolve_config_path();
    let selected_config_path = resolved_config_path.as_deref().or(Some("config.toml"));

    // Load configuration
    let config = config::load_config(selected_config_path).map_err(StartupError::ConfigError)?;

    // Initialize tracing
    let filter = parse_logging_filter(&config.logging.level)?;

    if config.logging.json {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    tracing::info!(
        source = config_source,
        path = selected_config_path.unwrap_or("<none>"),
        "resolved startup configuration path"
    );

    // Warn if the config file path is relative, since it depends on the
    // working directory at startup and may break under process managers.
    if let Some(p) = selected_config_path {
        if !std::path::Path::new(p).is_absolute() {
            tracing::warn!(
                path = p,
                "config file path is relative — behavior depends on working directory; \
                 consider using an absolute path or ANNEX_CONFIG_PATH env var"
            );
        }
    }

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

    // Start background retention task. Monitor for panics to surface
    // them rather than silently swallowing.
    let retention_handle = tokio::spawn(annex_server::retention::start_retention_task(
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

    // Get Server ID and Policy (auto-seed if no server row exists)
    let (server_id, policy): (i64, ServerPolicy) = {
        let conn = pool.get()?;
        let existing = conn.query_row("SELECT id, policy_json FROM servers LIMIT 1", [], |row| {
            let id: i64 = row.get(0)?;
            let policy_json: String = row.get(1)?;
            let policy: ServerPolicy = serde_json::from_str(&policy_json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok((id, policy))
        })
        .optional()?;

        match existing {
            Some(row) => row,
            None => {
                tracing::info!("no server configured — seeding default server record");
                let slug = std::env::var("ANNEX_SERVER_SLUG")
                    .unwrap_or_else(|_| "default".to_string());
                let label = std::env::var("ANNEX_SERVER_LABEL")
                    .unwrap_or_else(|_| "Annex Server".to_string());
                let default_policy = ServerPolicy::default();
                // ServerPolicy contains only primitive types (f64, bool, u32, Vec<String>),
                // so serde_json serialization is infallible for this struct.
                let policy_json = serde_json::to_string(&default_policy)
                    .expect("ServerPolicy::default() contains only primitive types and cannot fail serialization");
                conn.execute(
                    "INSERT INTO servers (slug, label, policy_json) VALUES (?1, ?2, ?3)",
                    rusqlite::params![slug, label, &policy_json],
                )?;
                let id = conn.last_insert_rowid();
                (id, default_policy)
            }
        }
    };

    // Load ZK verification key
    let vkey_path = std::env::var("ANNEX_ZK_KEY_PATH")
        .unwrap_or_else(|_| "zk/keys/membership_vkey.json".to_string());
    let vkey_json = std::fs::read_to_string(&vkey_path).map_err(|e| {
        tracing::error!(path = %vkey_path, "failed to read verification key");
        StartupError::IoError(e)
    })?;
    let membership_vkey =
        annex_identity::zk::parse_verification_key(&vkey_json).map_err(StartupError::ZkError)?;

    // Load or generate Signing Key
    let signing_key = if let Ok(hex_key) = std::env::var("ANNEX_SIGNING_KEY") {
        let bytes = hex::decode(&hex_key)
            .map_err(|e| StartupError::InvalidSigningKey(format!("not valid hex: {}", e)))?;
        let byte_array: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
            StartupError::InvalidSigningKey(format!("expected 32 bytes, got {}", v.len()))
        })?;
        SigningKey::from_bytes(&byte_array)
    } else {
        tracing::warn!("ANNEX_SIGNING_KEY not set. Generating ephemeral key. Federation signatures will change on restart.");
        SigningKey::generate(&mut OsRng)
    };

    // Create presence event broadcast channel
    let (presence_tx, _) = tokio::sync::broadcast::channel(config.server.presence_broadcast_capacity);

    // Create observe event broadcast channel (for SSE /events/stream)
    let (observe_tx, _) = tokio::sync::broadcast::channel(256);

    // Initialize Voice Service
    let voice_service = annex_voice::VoiceService::new(config.livekit);

    // Initialize TTS Service
    let tts_service =
        annex_voice::TtsService::new(&config.voice.tts_voices_dir, &config.voice.tts_binary_path);

    // Initialize STT Service
    let stt_service =
        annex_voice::SttService::new(&config.voice.stt_model_path, &config.voice.stt_binary_path);

    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(membership_vkey),
        server_id,
        signing_key: Arc::new(signing_key),
        public_url: config.server.public_url.clone(),
        policy: Arc::new(RwLock::new(policy)),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx,
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(tts_service),
        stt_service: Arc::new(stt_service),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx,
    };

    // Start background pruning task. Monitor for panics.
    let pruning_handle = tokio::spawn(annex_server::background::start_pruning_task(
        Arc::new(state.clone()),
        config.server.inactivity_threshold_seconds,
    ));
    tokio::spawn(async move {
        if let Err(e) = pruning_handle.await {
            tracing::error!("pruning background task panicked: {}", e);
        }
    });

    // Build application
    let app = app(state);
    let addr = SocketAddr::new(config.server.host, config.server.port);

    tracing::info!(%addr, "starting annex server");

    let listener = TcpListener::bind(addr).await.map_err(|e| {
        tracing::error!(%addr, "failed to bind to address — is another process using this port?");
        StartupError::IoError(e)
    })?;

    // Serve with graceful shutdown
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|e| {
        tracing::error!("server runtime error: {}", e);
        StartupError::IoError(e)
    })?;

    tracing::info!("annex server shut down");

    Ok(())
}

/// Waits for a SIGINT (Ctrl+C) or SIGTERM signal for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            // Signal handler installation is a process-level invariant. If it fails,
            // the OS does not support signals or the runtime is broken — neither can
            // be recovered at this layer.
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            // Same reasoning: if the OS cannot register SIGTERM, no graceful
            // shutdown is possible and the process should abort immediately.
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => { tracing::info!("received SIGINT, initiating graceful shutdown"); }
        () = terminate => { tracing::info!("received SIGTERM, initiating graceful shutdown"); }
    }
}

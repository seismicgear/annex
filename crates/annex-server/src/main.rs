//! Annex server binary — the main entry point for the Annex platform.
//!
//! Starts an axum HTTP server with structured logging, database initialization,
//! and graceful shutdown on SIGTERM/SIGINT.

use annex_identity::MerkleTree;
use annex_server::middleware::RateLimiter;
use annex_server::{app, config, AppState};
use annex_types::ServerPolicy;
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

    // Initialize database
    let pool = annex_db::create_pool(
        &config.database.path,
        annex_db::DbRuntimeSettings {
            busy_timeout_ms: config.database.busy_timeout_ms,
            pool_max_size: config.database.pool_max_size,
        },
    )?;

    {
        let conn = pool
            .get()
            .expect("failed to get database connection for migrations");
        let applied = annex_db::run_migrations(&conn).expect("failed to run database migrations");
        if applied > 0 {
            tracing::info!(count = applied, "applied database migrations");
        }
    }

    // Start background retention task
    tokio::spawn(annex_server::retention::start_retention_task(
        pool.clone(),
        config.server.retention_check_interval_seconds,
    ));

    // Initialize Merkle Tree
    // Get a dedicated connection for tree initialization
    let tree = {
        let conn = pool
            .get()
            .expect("failed to get database connection for merkle tree init");
        MerkleTree::restore(&conn, 20)?
    };

    // Get Server ID and Policy
    let (server_id, policy): (i64, ServerPolicy) = {
        let conn = pool
            .get()
            .expect("failed to get database connection for server id");
        conn.query_row("SELECT id, policy_json FROM servers LIMIT 1", [], |row| {
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
        .optional()
        .expect("failed to query servers table")
        .unwrap_or_else(|| {
            tracing::error!("no server configured in 'servers' table");
            std::process::exit(1);
        })
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

    // Create presence event broadcast channel
    let (presence_tx, _) = tokio::sync::broadcast::channel(100);

    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(membership_vkey),
        server_id,
        policy: Arc::new(RwLock::new(policy)),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx,
    };

    // Start background pruning task
    tokio::spawn(annex_server::background::start_pruning_task(
        Arc::new(state.clone()),
        config.server.inactivity_threshold_seconds,
    ));

    // Build application
    let app = app(state);
    let addr = SocketAddr::new(config.server.host, config.server.port);

    tracing::info!(%addr, "starting annex server");

    let listener = TcpListener::bind(addr)
        .await
        .expect("failed to bind to address — is another process using this port?");

    // Serve with graceful shutdown
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("server error");

    tracing::info!("annex server shut down");

    Ok(())
}

/// Waits for a SIGINT (Ctrl+C) or SIGTERM signal for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
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

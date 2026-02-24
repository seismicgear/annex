//! Annex server binary — the main entry point for the Annex platform.
//!
//! Starts an axum HTTP server with structured logging, database initialization,
//! and graceful shutdown on SIGTERM/SIGINT.

use annex_server::{config, init_tracing, prepare_server, StartupError};
use std::net::SocketAddr;

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

#[tokio::main]
async fn main() -> Result<(), StartupError> {
    let (resolved_config_path, config_source) = resolve_config_path();
    let selected_config_path = resolved_config_path.as_deref().or(Some("config.toml"));

    // Load configuration
    let config = config::load_config(selected_config_path)?;

    // Initialize tracing
    init_tracing(&config.logging)?;

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

    // Prepare and start the server
    let (listener, app) = prepare_server(config).await?;

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

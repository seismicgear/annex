//! Annex server binary — the main entry point for the Annex platform.
//!
//! Starts an axum HTTP server with structured logging, database initialization,
//! and graceful shutdown on SIGTERM/SIGINT.

mod config;

use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// Response structure for the health check endpoint.
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

/// Health check handler.
///
/// Returns `200 OK` with server status and version. Used by load balancers,
/// monitoring, and CI to verify the server is running.
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: "0.0.1",
    })
}

/// Builds the application router with all routes.
fn app() -> Router {
    Router::new().route("/health", get(health))
}

#[tokio::main]
async fn main() {
    // Load configuration
    let config = config::load_config(Some("config.toml"))
        .expect("failed to load configuration — the server cannot start without valid config");

    // Initialize tracing
    let filter =
        EnvFilter::try_new(&config.logging.level).unwrap_or_else(|_| EnvFilter::new("info"));

    if config.logging.json {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    // Initialize database
    let pool = annex_db::create_pool(&config.database.path)
        .expect("failed to create database pool — check database.path in config");

    {
        let conn = pool
            .get()
            .expect("failed to get database connection for migrations");
        let applied = annex_db::run_migrations(&conn).expect("failed to run database migrations");
        if applied > 0 {
            tracing::info!(count = applied, "applied database migrations");
        }
    }

    // Build application
    let app = app();
    let addr = SocketAddr::new(config.server.host, config.server.port);

    tracing::info!(%addr, "starting annex server");

    let listener = TcpListener::bind(addr)
        .await
        .expect("failed to bind to address — is another process using this port?");

    // Serve with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");

    tracing::info!("annex server shut down");
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_check_returns_ok() {
        let app = app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], "0.0.1");
    }
}

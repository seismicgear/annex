//! Annex server library logic.

pub mod api;
pub mod api_vrp;
pub mod config;
pub mod middleware;

use annex_db::DbPool;
use annex_identity::zk::{Bn254, VerifyingKey};
use annex_identity::MerkleTree;
use annex_types::ServerPolicy;
use axum::{
    routing::{get, post},
    Extension, Json, Router,
};
use middleware::RateLimiter;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, RwLock};

/// Application state shared across all request handlers.
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool.
    pub pool: DbPool,
    /// In-memory Merkle tree state.
    pub merkle_tree: Arc<Mutex<MerkleTree>>,
    /// ZK Membership verification key.
    pub membership_vkey: Arc<VerifyingKey<Bn254>>,
    /// The local server ID.
    pub server_id: i64,
    /// Server policy configuration.
    pub policy: Arc<RwLock<ServerPolicy>>,
    /// Rate limiter state.
    pub rate_limiter: RateLimiter,
}

/// Health check handler.
async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": "0.0.1"
    }))
}

/// Builds the application router with all routes.
pub fn app(state: AppState) -> Router {
    Router::new()
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
        .layer(axum::middleware::from_fn(middleware::rate_limit_middleware))
        .layer(Extension(Arc::new(state)))
}

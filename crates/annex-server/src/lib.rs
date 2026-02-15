//! Annex server library logic.

pub mod api;
pub mod config;

use annex_db::DbPool;
use annex_identity::MerkleTree;
use axum::{
    routing::{get, post},
    Extension, Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Application state shared across all request handlers.
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool.
    pub pool: DbPool,
    /// In-memory Merkle tree state.
    pub merkle_tree: Arc<Mutex<MerkleTree>>,
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
        .layer(Extension(Arc::new(state)))
}

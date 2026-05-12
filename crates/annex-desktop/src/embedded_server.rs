//! Lifecycle of the embedded Axum server bundled into the desktop app.
//!
//! In **Host** mode the desktop launches `annex_server::prepare_server` with
//! the on-disk `config.toml` and serves the API/WebSocket endpoints on a
//! loopback port. The frontend then loads the same URL the React webview is
//! pointed at.

use std::net::SocketAddr;

use annex_server::{config, init_tracing, prepare_server};

use crate::app_state::{AppManagedState, ServerState};

/// Start the embedded Axum server. Returns the server URL on success.
/// Idempotent — returns existing URL if already running.
#[tauri::command]
pub(crate) async fn start_embedded_server(
    state: tauri::State<'_, AppManagedState>,
) -> Result<String, String> {
    // Check if server is already running.
    {
        let guard = state.server.lock().map_err(|e| e.to_string())?;
        if let Some(ref srv) = *guard {
            return Ok(srv.url.clone());
        }
    }

    let config_path_str = state.config_path.to_string_lossy().to_string();

    // Load configuration.
    let mut cfg =
        config::load_config(Some(&config_path_str)).map_err(|e| format!("config error: {e}"))?;

    // Apply the runtime WebRTC override (if `start_local_webrtc` ran first).
    // We deliberately do NOT plumb these via `std::env::set_var` because by
    // the time we get here the Tauri runtime is multi-threaded — see
    // `app_state::WebRtcConfigOverride` for the full rationale. Reading
    // from a `Mutex<Option<…>>` and assigning into the loaded `Config`
    // struct is the equivalent operation, minus the UB.
    {
        let guard = state
            .webrtc_config_override
            .lock()
            .map_err(|e| e.to_string())?;
        if let Some(ref ovr) = *guard {
            cfg.webrtc.url = ovr.url.clone();
            cfg.webrtc.public_url = ovr.public_url.clone();
            cfg.webrtc.api_key = ovr.api_key.clone();
            cfg.webrtc.api_secret = ovr.api_secret.clone();
        }
    }

    // Initialize tracing (ignore if already initialized).
    let _ = init_tracing(&cfg.logging);

    // Prepare the server (DB, state, listener).
    let (listener, router) = prepare_server(cfg)
        .await
        .map_err(|e| format!("server startup failed: {e}"))?;

    let addr = listener
        .local_addr()
        .map_err(|e| format!("no local addr: {e}"))?;
    let url = format!("http://127.0.0.1:{}", addr.port());

    tracing::info!(%url, "embedded server ready");

    // Store the server URL.
    {
        let mut guard = state.server.lock().map_err(|e| e.to_string())?;
        *guard = Some(ServerState { url: url.clone() });
    }

    // Spawn the Axum server to run until the process exits.
    tauri::async_runtime::spawn(async move {
        if let Err(e) = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        {
            tracing::error!("server error: {e}");
        }
    });

    // Poll the health endpoint until the server is accepting connections.
    // Without this, the frontend can fire API requests before axum::serve()
    // has polled its first accept(), causing "Failed to fetch" on startup.
    // Budget: 150 attempts × 100ms = 15 seconds — generous for first-run
    // scenarios where database migrations and Merkle tree restoration can
    // be slow on modest hardware or cold disk caches.
    let health_url = format!("{url}/health");
    let client = reqwest::Client::new();
    let mut ready = false;
    for attempt in 0u32..150 {
        match client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                ready = true;
                tracing::debug!(attempt, "embedded server health check passed");
                break;
            }
            _ => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
    if !ready {
        return Err("embedded server failed to become ready within 15 seconds".to_string());
    }

    Ok(url)
}

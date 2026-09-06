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
    //
    // "Running" has to mean "answering", not "we stored a URL once". The URL
    // is recorded below BEFORE the readiness poll, so a start that timed out
    // left this state populated and every retry returned Ok immediately —
    // reporting success about a server that had never served a request. The
    // frontend then went on to fail every call with "Failed to fetch" and no
    // explanation, and the one honest error was the one the user had already
    // dismissed. Re-check before claiming it.
    {
        let existing = {
            let guard = state.server.lock().map_err(|e| e.to_string())?;
            guard.as_ref().map(|srv| srv.url.clone())
        };
        if let Some(url) = existing {
            // A short budget: this server was started earlier, so it is either
            // up by now or it is not coming up.
            if wait_for_health(&url, 30).await {
                return Ok(url);
            }
            return Err(format!(
                "the embedded server at {url} was started but is not responding"
            ));
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

    // Budget: 150 attempts × 100ms = 15 seconds — generous for first-run
    // scenarios where database migrations and Merkle tree restoration can be
    // slow on modest hardware or cold disk caches.
    if !wait_for_health(&url, 150).await {
        return Err("embedded server failed to become ready within 15 seconds".to_string());
    }

    Ok(url)
}

/// Poll `{url}/health` until it answers, or `attempts` × 100ms elapse.
///
/// Without this the frontend can fire API requests before `axum::serve` has
/// polled its first `accept()`, which surfaces as "Failed to fetch" on
/// startup. It is also what the idempotent early return above uses to decide
/// whether an already-started server is actually serving.
async fn wait_for_health(url: &str, attempts: u32) -> bool {
    let health_url = format!("{url}/health");
    let client = reqwest::Client::new();
    for attempt in 0..attempts {
        match client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(attempt, "embedded server health check passed");
                return true;
            }
            _ => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};

    #[tokio::test]
    async fn wait_for_health_sees_a_server_that_is_up() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let app = Router::new().route("/health", get(|| async { "ok" }));
            let _ = axum::serve(listener, app).await;
        });
        assert!(wait_for_health(&format!("http://127.0.0.1:{port}"), 50).await);
    }

    #[tokio::test]
    async fn wait_for_health_gives_up_on_a_port_nothing_is_serving() {
        // Bound and dropped, so the port is almost certainly free and nothing
        // is listening on it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(!wait_for_health(&format!("http://127.0.0.1:{port}"), 3).await);
    }
}

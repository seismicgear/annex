//! Annex router integration: registering the embedded server with a first-party
//! routing layer to obtain a publicly-reachable HTTPS URL (and an optional
//! WebRTC WebSocket URL).
//!
//! Replaces the older cloudflared tunnel approach. The session lifecycle is:
//!   * `acquire_public_endpoint` — POST `/v1/register` with the local port.
//!     The router returns `{ public_url, public_webrtc_url, session_id }`.
//!   * `release_public_endpoint` — POST `/v1/release` so the session can be
//!     reaped immediately. Best-effort; the router also expires sessions on
//!     its own timeout.
//!   * `get_public_endpoint` — surfaces the cached info to the frontend.

use serde::{Deserialize, Serialize};

use crate::app_state::{AppManagedState, RouterSessionState};

/// Default Annex router URL. Override with the `ANNEX_ROUTER_URL` environment
/// variable for custom deployments.
const DEFAULT_ROUTER_URL: &str = "https://router.annex.net";

/// Resolve the Annex router base URL from the environment or use the default.
fn router_base_url() -> String {
    std::env::var("ANNEX_ROUTER_URL").unwrap_or_else(|_| DEFAULT_ROUTER_URL.to_string())
}

/// Response from the Annex router when registering a local server.
#[derive(Debug, Clone, Deserialize)]
struct RouterRegistrationResponse {
    /// Publicly-reachable HTTPS base URL for the Annex server.
    public_url: String,
    /// Optional publicly-reachable WebSocket URL for WebRTC.
    /// `None` if the router does not proxy WebRTC traffic.
    public_webrtc_url: Option<String>,
    /// Session identifier used for heartbeats and release.
    session_id: String,
}

/// Register the local embedded server with the Annex router to acquire a
/// public endpoint. Returns the public URL so the frontend can feed it
/// into the existing `PUT /api/admin/public-url` flow.
#[tauri::command]
pub(crate) async fn acquire_public_endpoint(
    state: tauri::State<'_, AppManagedState>,
) -> Result<String, String> {
    // Return cached session if already registered.
    {
        let guard = state.router_session.lock().map_err(|e| e.to_string())?;
        if let Some(ref session) = *guard {
            return Ok(session.public_url.clone());
        }
    }

    // Get the server port from the running embedded server.
    let port: u16 = {
        let guard = state.server.lock().map_err(|e| e.to_string())?;
        let srv = guard
            .as_ref()
            .ok_or("embedded server is not running — start it first")?;
        srv.url
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| format!("could not parse port from server URL: {}", srv.url))?
    };

    let router_url = router_base_url();
    let register_url = format!("{router_url}/v1/register");

    tracing::info!(%port, %router_url, "registering with Annex router for public endpoint");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let body = serde_json::json!({
        "local_port": port,
        "protocol": "https",
    });

    let resp = client
        .post(&register_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Annex router registration failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!("Annex router returned HTTP {status}: {body_text}"));
    }

    let registration: RouterRegistrationResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse router response: {e}"))?;

    tracing::info!(
        public_url = %registration.public_url,
        public_webrtc_url = ?registration.public_webrtc_url,
        session_id = %registration.session_id,
        "public endpoint acquired from Annex router"
    );

    // If the router returned a public WebRTC URL and WebRTC is running
    // locally, log the availability. The frontend will use this via
    // get_public_endpoint to inform the server.
    {
        let lk_guard = state.webrtc.lock().map_err(|e| e.to_string())?;
        if lk_guard.is_some() {
            if registration.public_webrtc_url.is_some() {
                tracing::info!("Annex router is proxying WebRTC — remote voice will be available");
            } else {
                tracing::info!(
                    "WebRTC is running locally but the Annex router does not proxy WebRTC. \
                     Remote voice/video will be unavailable; text and invites will work."
                );
            }
        }
    }

    let public_url = registration.public_url.clone();

    // Store router session state.
    {
        let mut guard = state.router_session.lock().map_err(|e| e.to_string())?;
        *guard = Some(RouterSessionState {
            public_url: registration.public_url,
            public_webrtc_url: registration.public_webrtc_url,
            session_id: registration.session_id,
        });
    }

    Ok(public_url)
}

/// Release the public endpoint and end the router session.
#[tauri::command]
pub(crate) fn release_public_endpoint(
    state: tauri::State<'_, AppManagedState>,
) -> Result<(), String> {
    let mut guard = state.router_session.lock().map_err(|e| e.to_string())?;
    if let Some(session) = guard.take() {
        tracing::info!(
            public_url = %session.public_url,
            session_id = %session.session_id,
            "releasing public endpoint"
        );
        // Fire-and-forget release to the router. Non-fatal if it fails —
        // the router will expire the session on its own timeout.
        let router_url = router_base_url();
        let release_url = format!("{router_url}/v1/release");
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .ok();
        if let Some(client) = client {
            let _ = client
                .post(&release_url)
                .json(&serde_json::json!({ "session_id": session.session_id }))
                .send();
        }
    }
    Ok(())
}

/// Public endpoint info returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PublicEndpointInfo {
    public_url: String,
    public_webrtc_url: Option<String>,
}

/// Get the current public endpoint info, if a router session is active.
#[tauri::command]
pub(crate) fn get_public_endpoint(
    state: tauri::State<'_, AppManagedState>,
) -> Option<PublicEndpointInfo> {
    state.router_session.lock().ok().and_then(|guard| {
        guard.as_ref().map(|s| PublicEndpointInfo {
            public_url: s.public_url.clone(),
            public_webrtc_url: s.public_webrtc_url.clone(),
        })
    })
}

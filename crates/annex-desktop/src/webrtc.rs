//! WebRTC server lifecycle and configuration commands.
//!
//! Two responsibilities live here:
//!   * Persisted config — read/write the `[webrtc]` block in `config.toml`,
//!     with the API secret split off into the OS keychain (with a config-file
//!     fallback when the keychain is unavailable).
//!   * Process management — locate, download, and spawn `webrtc-server` for
//!     desktop host mode, and inject the resulting URL/credentials into env
//!     vars so the embedded Annex server can mint join tokens.
//!
//! Several commands are kept behind `#[allow(dead_code)]` because the
//! frontend settings UI was removed; they're retained so a future admin CLI
//! or plugin can re-use them without re-deriving the file/keychain plumbing.

use serde::{Deserialize, Serialize};

use annex_server::config;

use crate::app_state::{AppManagedState, WebRTCProcessState, WebRtcConfigOverride};
use crate::keyring::{
    delete_api_secret_from_keyring, load_api_secret_from_keyring, store_api_secret_in_keyring,
};

/// WebRTC configuration status returned to the frontend.
///
/// The `api_secret` is never exposed — only a boolean `has_api_secret`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WebRTCSettingsResponse {
    configured: bool,
    url: String,
    api_key: String,
    has_api_secret: bool,
    token_ttl_seconds: u64,
}

/// Read the current WebRTC configuration from config.toml + keyring.
#[tauri::command]
pub(crate) fn get_webrtc_config(
    state: tauri::State<'_, AppManagedState>,
) -> Result<WebRTCSettingsResponse, String> {
    let config_path_str = state.config_path.to_string_lossy().to_string();
    let cfg =
        config::load_config(Some(&config_path_str)).map_err(|e| format!("config error: {e}"))?;

    let has_secret_in_config = !cfg.webrtc.api_secret.is_empty();
    let has_secret_in_keyring = load_api_secret_from_keyring()
        .unwrap_or(None)
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    // Check whether the user has explicitly configured WebRTC by looking for
    // a [webrtc] section in the config file. When the section is absent (or
    // commented out), WebRtcConfig::default() provides dev values — but we
    // should NOT consider that "configured" because the user never set it up.
    let explicitly_configured = std::fs::read_to_string(&state.config_path)
        .ok()
        .and_then(|contents| contents.parse::<toml::Value>().ok())
        .map(|doc| doc.get("webrtc").is_some())
        .unwrap_or(false);

    Ok(WebRTCSettingsResponse {
        configured: explicitly_configured,
        url: cfg.webrtc.url,
        api_key: cfg.webrtc.api_key,
        has_api_secret: has_secret_in_config || has_secret_in_keyring,
        token_ttl_seconds: cfg.webrtc.token_ttl_seconds,
    })
}

/// Input from the frontend for saving WebRTC settings.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct SaveWebRTCInput {
    url: String,
    api_key: String,
    api_secret: String,
    #[serde(default = "default_token_ttl")]
    token_ttl_seconds: u64,
}

#[allow(dead_code)]
fn default_token_ttl() -> u64 {
    3600
}

/// Save WebRTC configuration to config.toml and the API secret to OS keyring.
///
/// If the keyring is unavailable, the secret falls back to config.toml storage
/// with a warning log.
///
/// Not currently registered in the invoke handler (frontend settings panel was
/// removed). Retained for potential future admin CLI or plugin use.
#[tauri::command]
#[allow(dead_code)]
pub(crate) fn save_webrtc_config(
    state: tauri::State<'_, AppManagedState>,
    input: SaveWebRTCInput,
) -> Result<(), String> {
    // Try to store secret in keyring first
    let secret_in_keyring = match store_api_secret_in_keyring(&input.api_secret) {
        Ok(()) => {
            tracing::info!("WebRTC API secret stored in OS keyring");
            true
        }
        Err(e) => {
            tracing::warn!("failed to store secret in keyring, storing in config file: {e}");
            false
        }
    };

    let config_path = &state.config_path;
    let contents =
        std::fs::read_to_string(config_path).map_err(|e| format!("failed to read config: {e}"))?;

    let mut doc: toml::Value =
        toml::from_str(&contents).map_err(|e| format!("failed to parse config: {e}"))?;

    let table = doc
        .as_table_mut()
        .ok_or("config root is not a TOML table")?;

    let lk = table
        .entry("webrtc")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let lk_table = lk.as_table_mut().ok_or("[webrtc] is not a TOML table")?;

    lk_table.insert("url".into(), toml::Value::String(input.url));
    lk_table.insert("api_key".into(), toml::Value::String(input.api_key));
    lk_table.insert(
        "token_ttl_seconds".into(),
        toml::Value::Integer(input.token_ttl_seconds as i64),
    );

    if secret_in_keyring {
        // Secret is in keyring — remove from config file for security
        lk_table.remove("api_secret");
    } else {
        // Fallback: store in config file
        lk_table.insert("api_secret".into(), toml::Value::String(input.api_secret));
    }

    let serialized =
        toml::to_string_pretty(&doc).map_err(|e| format!("failed to serialize config: {e}"))?;

    std::fs::write(config_path, serialized).map_err(|e| format!("failed to write config: {e}"))?;

    tracing::info!("WebRTC configuration saved");
    Ok(())
}

/// Clear WebRTC configuration from both config.toml and the OS keyring.
///
/// Not currently registered in the invoke handler. Retained for future use.
#[tauri::command]
#[allow(dead_code)]
pub(crate) fn clear_webrtc_config(state: tauri::State<'_, AppManagedState>) -> Result<(), String> {
    // Remove from keyring
    if let Err(e) = delete_api_secret_from_keyring() {
        tracing::warn!("failed to remove secret from keyring: {e}");
    }

    // Remove from config file
    let config_path = &state.config_path;
    let contents =
        std::fs::read_to_string(config_path).map_err(|e| format!("failed to read config: {e}"))?;

    let mut doc: toml::Value =
        toml::from_str(&contents).map_err(|e| format!("failed to parse config: {e}"))?;

    if let Some(table) = doc.as_table_mut() {
        table.remove("webrtc");
    }

    let serialized =
        toml::to_string_pretty(&doc).map_err(|e| format!("failed to serialize config: {e}"))?;

    std::fs::write(config_path, serialized).map_err(|e| format!("failed to write config: {e}"))?;

    tracing::info!("WebRTC configuration cleared");
    Ok(())
}

/// Check if a WebRTC server is reachable at the given URL.
///
/// Used by the frontend during host startup to verify that a "configured"
/// WebRTC endpoint is actually reachable before advertising voice capability.
#[tauri::command]
pub(crate) async fn check_webrtc_reachable(url: String) -> Result<serde_json::Value, String> {
    // WebRTC serves HTTP on the same port as WebSocket.
    // Replace ws:// with http:// for the health check.
    let http_url = url
        .replace("ws://", "http://")
        .replace("wss://", "https://");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    match client.get(&http_url).send().await {
        Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => {
            Ok(serde_json::json!({ "reachable": true }))
        }
        Ok(resp) => Ok(serde_json::json!({
            "reachable": false,
            "error": format!("HTTP {}", resp.status())
        })),
        Err(e) => Ok(serde_json::json!({
            "reachable": false,
            "error": format!("{e}")
        })),
    }
}

// ── Local WebRTC server management ──

/// Enable the host-mode WebRTC SFU.
///
/// Annex's SFU is embedded **in-process** in the Annex server: `annex-voice`
/// runs it directly on the `webrtc` crate and exchanges SDP/ICE over the app
/// WebSocket. There is **no external `webrtc-server`** to download or spawn —
/// the previous implementation fetched a binary from a GitHub release URL that
/// does not exist, and that 404 disabled voice on every default desktop host.
///
/// Enabling voice therefore just means handing the embedded server a non-empty
/// loopback `[webrtc]` config so its voice service reports `is_enabled() ==
/// true`. The loopback host keeps the URL from being advertised to remote peers
/// until a public endpoint is acquired. Must be called BEFORE
/// `start_embedded_server` so the override is applied when the server is built.
#[tauri::command]
pub(crate) async fn start_local_webrtc(
    state: tauri::State<'_, AppManagedState>,
) -> Result<serde_json::Value, String> {
    // Already enabled? return the cached marker.
    {
        let guard = state.webrtc.lock().map_err(|e| e.to_string())?;
        if let Some(ref lk) = *guard {
            return Ok(serde_json::json!({ "url": lk.url, "embedded": true }));
        }
    }

    // The override must be set before the embedded server is built.
    {
        let guard = state.server.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Err(
                "embedded server is already running — enable local voice before the server, or restart the application".to_string(),
            );
        }
    }

    // Generate fresh credentials. (Voice-join tokens are HMAC-derived from the
    // server's Ed25519 signing key, not this secret — see `annex_voice::token`
    // — but a non-default key/secret avoids shipping the dev placeholder.)
    let api_key = format!("annex_{}", uuid::Uuid::new_v4().simple());
    let api_secret = format!("secret_{}", uuid::Uuid::new_v4().simple());
    // Loopback marker: enables the in-process SFU without advertising a URL.
    // Signaling and media flow over the app WebSocket + ICE, not this address.
    let lk_url = "ws://127.0.0.1:7880".to_string();

    {
        let mut guard = state
            .webrtc_config_override
            .lock()
            .map_err(|e| e.to_string())?;
        *guard = Some(WebRtcConfigOverride {
            url: lk_url.clone(),
            public_url: lk_url.clone(),
            api_key,
            api_secret,
        });
    }

    {
        let mut guard = state.webrtc.lock().map_err(|e| e.to_string())?;
        *guard = Some(WebRTCProcessState {
            url: lk_url.clone(),
            child: None,
        });
    }

    tracing::info!(
        %lk_url,
        "enabled in-process WebRTC SFU for host mode (no external sidecar)"
    );
    Ok(serde_json::json!({ "url": lk_url, "embedded": true }))
}

/// Clear the in-memory WebRTC override so the embedded server falls back
/// to whatever's in `config.toml` (typically empty for desktop installs).
/// Must be called BEFORE `start_embedded_server` to take effect.
///
/// The previous implementation wrote four `std::env::set_var("ANNEX_WEBRTC_*", "")`
/// calls. That was UB under the live Tauri runtime — see
/// `app_state::WebRtcConfigOverride` for the full rationale. Clearing the
/// `Mutex<Option<…>>` is the equivalent operation, minus the UB.
#[tauri::command]
pub(crate) fn clear_webrtc_env(state: tauri::State<'_, AppManagedState>) -> Result<(), String> {
    let mut guard = state
        .webrtc_config_override
        .lock()
        .map_err(|e| e.to_string())?;
    *guard = None;
    tracing::info!("cleared webrtc config override (voice startup failed)");
    Ok(())
}

/// Tear down host-mode voice on exit. Idempotent.
///
/// The SFU is normally in-process, so there is no child to reap — this just
/// clears the tracked state. If an external sidecar was ever spawned (its
/// `std::process::Child` is NOT terminated on drop), it is killed here so it
/// does not orphan. Shared by the `stop_local_webrtc` command and the
/// application-exit handler in `main`.
pub(crate) fn shutdown_local_webrtc(state: &AppManagedState) {
    // Take the entry out under the lock, then reap outside any long hold.
    let entry = state.webrtc.lock().ok().and_then(|mut guard| guard.take());
    if let Some(lk) = entry {
        if let Some(mut child) = lk.child {
            tracing::info!(url = %lk.url, "stopping external webrtc-server sidecar");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Stop the local WebRTC server if running.
///
/// Not currently registered in the invoke handler. Retained for future use;
/// delegates to [`shutdown_local_webrtc`], which the exit handler also calls.
#[tauri::command]
#[allow(dead_code)]
pub(crate) fn stop_local_webrtc(state: tauri::State<'_, AppManagedState>) -> Result<(), String> {
    shutdown_local_webrtc(state.inner());
    Ok(())
}

/// Get the local WebRTC server URL, if a local instance is running.
///
/// Not currently registered in the invoke handler. Retained for future use.
#[tauri::command]
#[allow(dead_code)]
pub(crate) fn get_local_webrtc_url(state: tauri::State<'_, AppManagedState>) -> Option<String> {
    state
        .webrtc
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|lk| lk.url.clone()))
}

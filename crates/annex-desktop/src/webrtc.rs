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

use std::path::{Path, PathBuf};

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

/// Probe a range of ports and return the first one that is not already in use.
/// Uses a bind-and-drop approach: if we can bind to 127.0.0.1:port, the port
/// is available. The socket is closed immediately so webrtc-server can bind.
fn find_available_port(start: u16, end: u16) -> Option<u16> {
    (start..=end).find(|&port| std::net::TcpListener::bind(("127.0.0.1", port)).is_ok())
}

const WEBRTC_VERSION: &str = "1.7.2";

/// Returns the platform-specific WebRTC server binary name.
fn webrtc_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "webrtc-server.exe"
    } else {
        "webrtc-server"
    }
}

/// Returns the download URL for webrtc-server on this platform, if supported.
fn webrtc_download_url() -> Option<String> {
    let base = format!("https://github.com/webrtc/webrtc/releases/download/v{WEBRTC_VERSION}");
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(format!("{base}/webrtc_{WEBRTC_VERSION}_linux_amd64.tar.gz"))
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(format!("{base}/webrtc_{WEBRTC_VERSION}_linux_arm64.tar.gz"))
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some(format!(
            "{base}/webrtc_{WEBRTC_VERSION}_darwin_amd64.tar.gz"
        ))
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(format!(
            "{base}/webrtc_{WEBRTC_VERSION}_darwin_arm64.tar.gz"
        ))
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(format!("{base}/webrtc_{WEBRTC_VERSION}_windows_amd64.zip"))
    } else {
        None
    }
}

/// Searches PATH for the webrtc-server binary.
fn find_webrtc_in_path() -> Option<PathBuf> {
    let name = webrtc_binary_name();
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(name);
            if full.is_file() {
                Some(full)
            } else {
                None
            }
        })
    })
}

/// Ensures webrtc-server is available: checks PATH, then the local bin cache,
/// and downloads it if necessary. Returns the path to the binary.
async fn ensure_webrtc(data_dir: &Path) -> Result<PathBuf, String> {
    // 1. Check PATH
    if let Some(path) = find_webrtc_in_path() {
        tracing::info!(path = %path.display(), "found webrtc-server in PATH");
        return Ok(path);
    }

    // 2. Check local bin cache
    let bin_dir = data_dir.join("bin");
    let lk_path = bin_dir.join(webrtc_binary_name());
    if lk_path.exists() {
        tracing::info!(path = %lk_path.display(), "using cached webrtc-server");
        return Ok(lk_path);
    }

    // 3. Download
    let url = webrtc_download_url()
        .ok_or_else(|| "webrtc-server download not supported on this platform".to_string())?;

    tracing::info!(%url, "downloading webrtc-server");

    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("failed to create bin directory: {e}"))?;

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("webrtc-server download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "webrtc-server download failed: HTTP {}",
            resp.status()
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("webrtc-server download read failed: {e}"))?;

    if url.ends_with(".tar.gz") {
        let tgz_path = bin_dir.join("webrtc.tar.gz");
        std::fs::write(&tgz_path, &bytes)
            .map_err(|e| format!("failed to write webrtc archive: {e}"))?;
        let output = std::process::Command::new("tar")
            .args([
                "xzf",
                &tgz_path.to_string_lossy(),
                "-C",
                &bin_dir.to_string_lossy(),
            ])
            .output()
            .map_err(|e| format!("tar extract failed: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "tar extract failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let _ = std::fs::remove_file(&tgz_path);
    } else if url.ends_with(".zip") {
        let zip_path = bin_dir.join("webrtc.zip");
        std::fs::write(&zip_path, &bytes)
            .map_err(|e| format!("failed to write webrtc archive: {e}"))?;

        #[cfg(target_os = "windows")]
        {
            let output = std::process::Command::new("powershell")
                .args([
                    "-Command",
                    &format!(
                        "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                        zip_path.to_string_lossy(),
                        bin_dir.to_string_lossy()
                    ),
                ])
                .output()
                .map_err(|e| format!("zip extraction failed: {e}"))?;
            if !output.status.success() {
                return Err(format!(
                    "zip extraction failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let output = std::process::Command::new("unzip")
                .args([
                    "-o",
                    &zip_path.to_string_lossy(),
                    "-d",
                    &bin_dir.to_string_lossy(),
                ])
                .output()
                .map_err(|e| format!("zip extraction failed: {e}"))?;
            if !output.status.success() {
                return Err(format!(
                    "zip extraction failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        let _ = std::fs::remove_file(&zip_path);
    } else {
        std::fs::write(&lk_path, &bytes)
            .map_err(|e| format!("failed to write webrtc-server binary: {e}"))?;
    }

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&lk_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("failed to set webrtc-server permissions: {e}"))?;
    }

    tracing::info!(path = %lk_path.display(), "webrtc-server downloaded successfully");
    Ok(lk_path)
}

/// Start a local WebRTC server instance for desktop host mode.
///
/// Generates random API key/secret, spawns the process, and sets environment
/// variables so the embedded Annex server picks up the WebRTC config.
///
/// Must be called BEFORE `start_embedded_server` for the env vars to take effect.
#[tauri::command]
pub(crate) async fn start_local_webrtc(
    state: tauri::State<'_, AppManagedState>,
) -> Result<serde_json::Value, String> {
    // Check if already running
    {
        let guard = state.webrtc.lock().map_err(|e| e.to_string())?;
        if let Some(ref lk) = *guard {
            return Ok(serde_json::json!({ "url": lk.url }));
        }
    }

    // Check if the embedded server is already running — env vars won't help after that
    {
        let guard = state.server.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Err(
                "embedded server is already running — start local WebRTC before the server, or restart the application".to_string()
            );
        }
    }

    let lk_path = ensure_webrtc(&state.data_dir).await?;

    // Generate random API key + secret
    let api_key = format!("annex_{}", uuid::Uuid::new_v4().simple());
    let api_secret = format!("secret_{}", uuid::Uuid::new_v4().simple());

    // Probe for a free port starting from 7880. webrtc-server's --port flag
    // doesn't support auto-select (port 0), so we try ports 7880..7899 and pick
    // the first one that is not already in use.
    let port = find_available_port(7880, 7899)
        .ok_or("no available port in range 7880–7899 for webrtc-server")?;
    if port != 7880 {
        tracing::warn!(
            default_port = 7880,
            actual_port = port,
            "default WebRTC port 7880 was occupied — fell back to port {port}"
        );
    }
    let lk_url = format!("ws://127.0.0.1:{port}");

    tracing::info!(path = %lk_path.display(), %port, "starting local webrtc-server");

    let mut child = std::process::Command::new(&lk_path)
        .args([
            "--dev",
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--keys",
            &format!("{api_key}: {api_secret}"),
        ])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start webrtc-server: {e}"))?;

    // Read stderr in a background thread to detect readiness and keep pipe open.
    let stderr = child
        .stderr
        .take()
        .ok_or("failed to capture webrtc-server stderr")?;
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let reader = BufReader::new(stderr);
        let mut tx = Some(tx);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    tracing::debug!(line = %line, "webrtc-server");
                    if let Some(sender) = tx.take() {
                        // WebRTC logs readiness messages containing "started" or "ready"
                        if line.contains("started")
                            || line.contains("ready")
                            || line.contains("listening")
                        {
                            let _ = sender.send(Ok(()));
                            // Continue reading to drain the pipe
                        } else {
                            tx = Some(sender);
                        }
                    }
                }
                Err(e) => {
                    if let Some(sender) = tx.take() {
                        let _ = sender.send(Err(format!("webrtc-server stderr error: {e}")));
                    }
                    return;
                }
            }
        }
        if let Some(sender) = tx.take() {
            let _ = sender.send(Err("webrtc-server exited before becoming ready".to_string()));
        }
    });

    // Wait for readiness with timeout
    tokio::time::timeout(std::time::Duration::from_secs(15), rx)
        .await
        .map_err(|_| "webrtc-server startup timed out after 15 seconds".to_string())?
        .map_err(|_| "webrtc readiness channel dropped".to_string())??;

    // Stash the WebRTC config in `AppManagedState`. `start_embedded_server`
    // applies it to the `annex_server::config::Config` struct directly when
    // building the server, eliminating the previous `std::env::set_var`
    // approach (which was UB on Linux because the Tauri tokio runtime was
    // already multi-threaded by the time this command runs — Rust 1.85
    // marked `set_var` `unsafe` precisely to flag this hazard). See
    // `app_state::WebRtcConfigOverride` for the full rationale.
    //
    // ANNEX_WEBRTC_URL is the internal bind address used for server-side API
    // calls (token generation, room management). ANNEX_WEBRTC_PUBLIC_URL is
    // the browser-facing WebSocket URL sent to clients in join responses.
    // For local-only use both point at loopback. When a public endpoint is
    // later acquired (acquire_public_endpoint), the frontend pushes a
    // proper public URL via the server's admin API.
    {
        let mut guard = state
            .webrtc_config_override
            .lock()
            .map_err(|e| e.to_string())?;
        *guard = Some(WebRtcConfigOverride {
            url: lk_url.clone(),
            public_url: lk_url.clone(),
            api_key: api_key.clone(),
            api_secret: api_secret.clone(),
        });
    }

    tracing::info!(%lk_url, "local webrtc-server ready");

    {
        let mut guard = state.webrtc.lock().map_err(|e| e.to_string())?;
        *guard = Some(WebRTCProcessState {
            url: lk_url.clone(),
            child,
        });
    }

    Ok(serde_json::json!({ "url": lk_url, "port": port }))
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

/// Kill the local webrtc-server child process if one is running. Idempotent.
///
/// Shared by the `stop_local_webrtc` command and the application-exit handler
/// in `main`. A spawned `std::process::Child` is NOT terminated when dropped,
/// so without an explicit kill the webrtc-server is orphaned when the desktop
/// app exits and keeps holding its port (7880–7899), which then forces the
/// next launch to fall back to a different port or exhaust the range.
pub(crate) fn shutdown_local_webrtc(state: &AppManagedState) {
    // Take the child out under the lock, then kill outside any await/long hold.
    let child = state.webrtc.lock().ok().and_then(|mut guard| guard.take());
    if let Some(mut lk) = child {
        tracing::info!(url = %lk.url, "stopping local webrtc-server");
        let _ = lk.child.kill();
        let _ = lk.child.wait();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_available_port_returns_port_in_range() {
        // With 20 ports to choose from, at least one should be available.
        let port = find_available_port(49152, 49171);
        assert!(port.is_some(), "should find at least one available port");
        let port = port.unwrap();
        assert!(
            (49152..=49171).contains(&port),
            "port {port} should be in range 49152–49171"
        );
    }

    #[test]
    fn find_available_port_returns_none_for_invalid_range() {
        // Port 0 is never bindable in practice. Use a range that is definitely occupied
        // by the test itself (port 0 is special — the OS picks one).
        // Instead, test that an empty range returns None.
        let port = find_available_port(10, 9);
        assert!(port.is_none(), "invalid range should return None");
    }
}

//! Annex desktop application — a Tauri wrapper that can either embed the Annex
//! server or connect to a remote server as a client-only instance.
//!
//! The bundled React frontend loads immediately and presents a startup mode
//! selector. In **Host** mode the embedded Axum server binds to a free port on
//! localhost and the client connects to it. In **Client** mode the webview
//! connects directly to a user-supplied remote server URL.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use annex_server::{config, init_tracing, prepare_server};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Manager;

/// Resolve the application data directory.
///
/// Uses `dirs::data_dir()` to locate the platform-specific directory:
/// - Windows: `%APPDATA%\Annex`
/// - macOS: `~/Library/Application Support/Annex`
/// - Linux: `~/.local/share/Annex`
fn resolve_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Annex")
}

/// Writes a default `config.toml` into the data directory if one does not
/// already exist, and ensures any Windows backslash paths are corrected.
///
/// Returns the path to the config file on success. Returns an error if the
/// config file cannot be created or a backslash migration cannot be persisted.
fn ensure_config(data_dir: &std::path::Path) -> Result<PathBuf, String> {
    let config_path = data_dir.join("config.toml");
    if !config_path.exists() {
        let db_path = data_dir.join("annex.db");
        let upload_dir = data_dir.join("uploads");
        // Use forward slashes for the database path — Windows APIs accept
        // them, and TOML double-quoted strings treat backslashes as escape
        // sequences (e.g. \U → unicode escape), which breaks parsing.
        let db_path_safe = db_path.display().to_string().replace('\\', "/");
        let contents = format!(
            r#"# Annex desktop configuration (auto-generated).

[server]
host = "127.0.0.1"
port = 0

[database]
path = "{db_path}"
busy_timeout_ms = 5000
pool_max_size = 8

[logging]
level = "info"
json = false

[cors]
# Desktop defaults: allow Tauri webview origins (macOS/Linux + Windows).
# Override with ANNEX_CORS_ORIGINS env var if needed.
allowed_origins = ["tauri://localhost", "https://tauri.localhost", "http://tauri.localhost"]

# [livekit]
# Uncomment and configure to enable voice channels (LiveKit WebRTC).
# url = "ws://localhost:7880"
# api_key = ""
# api_secret = ""
# token_ttl_seconds = 3600
"#,
            db_path = db_path_safe,
        );
        std::fs::write(&config_path, contents).map_err(|e| {
            format!(
                "failed to write default config to {}: {e}",
                config_path.display()
            )
        })?;

        // Pre-create the upload directory (non-fatal if this fails).
        let _ = std::fs::create_dir_all(&upload_dir);
    }

    // Always fix backslash paths regardless of whether the config was just
    // created or already existed. This handles configs from older versions
    // that wrote Windows-style paths, and acts as a safety net in case the
    // forward-slash replacement above is ever bypassed.
    fix_backslash_paths(&config_path)?;

    Ok(config_path)
}

/// Replaces Windows backslashes with forward slashes in a config file.
///
/// TOML double-quoted strings treat `\U` as an 8-digit unicode escape, so a
/// path like `C:\Users\monty\AppData\...\annex.db` fails to parse. This
/// function detects the drive-letter pattern `:\` and replaces all backslashes
/// with forward slashes, which Windows APIs accept.
fn fix_backslash_paths(config_path: &std::path::Path) -> Result<(), String> {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(format!(
                "failed to read config at {}: {e}",
                config_path.display()
            ))
        }
    };

    if contents.contains(":\\") {
        let fixed = contents.replace('\\', "/");
        std::fs::write(config_path, fixed).map_err(|e| {
            format!(
                "failed to fix backslash paths in config {}: {e}",
                config_path.display()
            )
        })?;
    }

    Ok(())
}

// ── Startup mode preference types ──

/// Persisted startup mode choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode")]
enum StartupMode {
    #[serde(rename = "host")]
    Host,
    #[serde(rename = "client")]
    Client { server_url: String },
}

/// Wrapper for the preference file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartupPrefs {
    startup_mode: StartupMode,
}

/// Tracks whether the embedded server is running.
struct ServerState {
    url: String,
}

/// Tracks the cloudflared tunnel process.
struct TunnelState {
    url: String,
    child: std::process::Child,
}

/// Tracks a locally-managed LiveKit server process.
struct LiveKitProcessState {
    url: String,
    child: std::process::Child,
}

/// Tauri-managed application state.
struct AppManagedState {
    data_dir: PathBuf,
    config_path: PathBuf,
    server: Mutex<Option<ServerState>>,
    tunnel: Mutex<Option<TunnelState>>,
    livekit: Mutex<Option<LiveKitProcessState>>,
}

// ── Tauri commands ──

/// Read saved startup mode preference. Returns `null` if none saved.
#[tauri::command]
fn get_startup_mode(state: tauri::State<'_, AppManagedState>) -> Option<StartupPrefs> {
    let prefs_path = state.data_dir.join("startup_prefs.json");
    std::fs::read_to_string(&prefs_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Save startup mode preference to disk.
#[tauri::command]
fn save_startup_mode(
    state: tauri::State<'_, AppManagedState>,
    prefs: StartupPrefs,
) -> Result<(), String> {
    let prefs_path = state.data_dir.join("startup_prefs.json");
    let json = serde_json::to_string_pretty(&prefs).map_err(|e| format!("serialize error: {e}"))?;
    std::fs::write(&prefs_path, json).map_err(|e| format!("write error: {e}"))?;
    Ok(())
}

/// Clear saved startup mode preference (reset).
#[tauri::command]
fn clear_startup_mode(state: tauri::State<'_, AppManagedState>) -> Result<(), String> {
    let prefs_path = state.data_dir.join("startup_prefs.json");
    if prefs_path.exists() {
        std::fs::remove_file(&prefs_path).map_err(|e| format!("remove error: {e}"))?;
    }
    Ok(())
}

/// Start the embedded Axum server. Returns the server URL on success.
/// Idempotent — returns existing URL if already running.
#[tauri::command]
async fn start_embedded_server(state: tauri::State<'_, AppManagedState>) -> Result<String, String> {
    // Check if server is already running.
    {
        let guard = state.server.lock().map_err(|e| e.to_string())?;
        if let Some(ref srv) = *guard {
            return Ok(srv.url.clone());
        }
    }

    let config_path_str = state.config_path.to_string_lossy().to_string();

    // Load configuration.
    let cfg =
        config::load_config(Some(&config_path_str)).map_err(|e| format!("config error: {e}"))?;

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
    let health_url = format!("{url}/health");
    let client = reqwest::Client::new();
    let mut ready = false;
    for attempt in 0u32..50 {
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
        return Err("embedded server failed to become ready within 5 seconds".to_string());
    }

    Ok(url)
}

// ── Tunnel management ──

/// Returns the platform-specific cloudflared binary name.
fn cloudflared_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "cloudflared.exe"
    } else {
        "cloudflared"
    }
}

/// Returns the download URL for cloudflared on this platform, if supported.
fn cloudflared_download_url() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-amd64.tgz")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-arm64.tgz")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe")
    } else {
        None
    }
}

/// Searches PATH for the cloudflared binary.
fn find_cloudflared_in_path() -> Option<PathBuf> {
    let name = cloudflared_binary_name();
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

/// Ensures cloudflared is available: checks PATH, then the local bin cache,
/// and downloads it if necessary. Returns the path to the binary.
async fn ensure_cloudflared(data_dir: &Path) -> Result<PathBuf, String> {
    // 1. Check PATH
    if let Some(path) = find_cloudflared_in_path() {
        tracing::info!(path = %path.display(), "found cloudflared in PATH");
        return Ok(path);
    }

    // 2. Check local bin cache
    let bin_dir = data_dir.join("bin");
    let cf_path = bin_dir.join(cloudflared_binary_name());
    if cf_path.exists() {
        tracing::info!(path = %cf_path.display(), "using cached cloudflared");
        return Ok(cf_path);
    }

    // 3. Download
    let url = cloudflared_download_url()
        .ok_or_else(|| "cloudflared download not supported on this platform".to_string())?;

    tracing::info!(%url, "downloading cloudflared");

    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("failed to create bin directory: {e}"))?;

    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("cloudflared download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "cloudflared download failed: HTTP {}",
            resp.status()
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("cloudflared download read failed: {e}"))?;

    if url.ends_with(".tgz") {
        // macOS: extract tarball
        let tgz_path = bin_dir.join("cloudflared.tgz");
        std::fs::write(&tgz_path, &bytes)
            .map_err(|e| format!("failed to write cloudflared archive: {e}"))?;
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
    } else {
        std::fs::write(&cf_path, &bytes)
            .map_err(|e| format!("failed to write cloudflared binary: {e}"))?;
    }

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cf_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("failed to set cloudflared permissions: {e}"))?;
    }

    tracing::info!(path = %cf_path.display(), "cloudflared downloaded successfully");
    Ok(cf_path)
}

/// Extract a trycloudflare.com URL from a line of cloudflared output.
fn extract_tunnel_url(line: &str) -> Option<String> {
    // cloudflared outputs lines like:
    //   | https://random-words-here.trycloudflare.com |
    // or in log format:
    //   ... https://random-words-here.trycloudflare.com ...
    for word in line.split_whitespace() {
        let trimmed = word.trim_matches('|').trim();
        if trimmed.contains(".trycloudflare.com") && trimmed.starts_with("https://") {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Start a cloudflared quick tunnel to expose the local server.
/// Returns the public tunnel URL (e.g. https://random.trycloudflare.com).
#[tauri::command]
async fn start_tunnel(state: tauri::State<'_, AppManagedState>) -> Result<String, String> {
    // Check if tunnel is already running
    {
        let guard = state.tunnel.lock().map_err(|e| e.to_string())?;
        if let Some(ref t) = *guard {
            return Ok(t.url.clone());
        }
    }

    // Get the server port from the running embedded server
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

    // Ensure cloudflared is available
    let cf_path = ensure_cloudflared(&state.data_dir).await?;

    tracing::info!(%port, path = %cf_path.display(), "starting cloudflared tunnel");

    // Spawn cloudflared
    let mut child = std::process::Command::new(&cf_path)
        .args(["tunnel", "--url", &format!("http://127.0.0.1:{port}")])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start cloudflared: {e}"))?;

    let stderr = child
        .stderr
        .take()
        .ok_or("failed to capture cloudflared stderr")?;

    // Read stderr in a background thread to find the tunnel URL.
    // The thread continues draining stderr after finding the URL to keep the
    // pipe open and prevent cloudflared from receiving SIGPIPE.
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let reader = BufReader::new(stderr);
        let mut tx = Some(tx);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    tracing::debug!(line = %line, "cloudflared");
                    if let Some(sender) = tx.take() {
                        if let Some(url) = extract_tunnel_url(&line) {
                            let _ = sender.send(Ok(url));
                            // Continue reading to keep the pipe open
                        } else {
                            tx = Some(sender);
                        }
                    }
                }
                Err(e) => {
                    if let Some(sender) = tx.take() {
                        let _ = sender.send(Err(format!("cloudflared stderr read error: {e}")));
                    }
                    return;
                }
            }
        }
        if let Some(sender) = tx.take() {
            let _ = sender.send(Err(
                "cloudflared exited without providing a tunnel URL".to_string()
            ));
        }
    });

    // Wait for the URL with a 30-second timeout
    let url = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
        .await
        .map_err(|_| "tunnel creation timed out after 30 seconds".to_string())?
        .map_err(|_| "tunnel URL channel was dropped".to_string())??;

    tracing::info!(%url, "tunnel established");

    // Store tunnel state
    {
        let mut guard = state.tunnel.lock().map_err(|e| e.to_string())?;
        *guard = Some(TunnelState {
            url: url.clone(),
            child,
        });
    }

    Ok(url)
}

/// Stop the cloudflared tunnel if running.
#[tauri::command]
fn stop_tunnel(state: tauri::State<'_, AppManagedState>) -> Result<(), String> {
    let mut guard = state.tunnel.lock().map_err(|e| e.to_string())?;
    if let Some(mut tunnel) = guard.take() {
        tracing::info!(url = %tunnel.url, "stopping tunnel");
        let _ = tunnel.child.kill();
        let _ = tunnel.child.wait();
    }
    Ok(())
}

/// Get the current tunnel URL, if a tunnel is active.
#[tauri::command]
fn get_tunnel_url(state: tauri::State<'_, AppManagedState>) -> Option<String> {
    state
        .tunnel
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|t| t.url.clone()))
}

/// Open a save-file dialog and write the provided JSON to the selected path.
///
/// Returns `Ok(Some(path))` when the file is saved, `Ok(None)` when the user
/// cancels the dialog, and `Err(...)` for I/O failures.
#[tauri::command]
fn export_identity_json(json: String) -> Result<Option<String>, String> {
    let file_path = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .set_file_name("annex-identity-backup.json")
        .save_file();

    let Some(path) = file_path else {
        return Ok(None);
    };

    std::fs::write(&path, json)
        .map_err(|e| format!("failed to write export file {}: {e}", path.display()))?;

    Ok(Some(path.display().to_string()))
}

// ── OS credential storage (keyring) ──

const KEYRING_SERVICE: &str = "com.annex.desktop";
const KEYRING_LIVEKIT_SECRET: &str = "livekit-api-secret";

/// Store the LiveKit API secret in the OS keyring.
fn store_api_secret_in_keyring(secret: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_LIVEKIT_SECRET)
        .map_err(|e| format!("keyring entry creation failed: {e}"))?;
    entry
        .set_password(secret)
        .map_err(|e| format!("keyring store failed: {e}"))?;
    Ok(())
}

/// Retrieve the LiveKit API secret from the OS keyring.
///
/// Returns `Ok(None)` if no secret is stored or the keyring is unavailable.
fn load_api_secret_from_keyring() -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_LIVEKIT_SECRET)
        .map_err(|e| format!("keyring entry creation failed: {e}"))?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(keyring::Error::PlatformFailure(ref msg)) => {
            tracing::warn!("OS keyring platform failure (falling back to config): {msg}");
            Ok(None)
        }
        Err(keyring::Error::NoStorageAccess(ref msg)) => {
            tracing::warn!("OS keyring not accessible (falling back to config): {msg}");
            Ok(None)
        }
        Err(e) => Err(format!("keyring read failed: {e}")),
    }
}

/// Delete the LiveKit API secret from the OS keyring.
fn delete_api_secret_from_keyring() -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_LIVEKIT_SECRET)
        .map_err(|e| format!("keyring entry creation failed: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // Already absent
        Err(e) => Err(format!("keyring delete failed: {e}")),
    }
}

// ── LiveKit configuration commands ──

/// LiveKit configuration status returned to the frontend.
///
/// The `api_secret` is never exposed — only a boolean `has_api_secret`.
#[derive(Debug, Clone, Serialize)]
struct LiveKitSettingsResponse {
    configured: bool,
    url: String,
    api_key: String,
    has_api_secret: bool,
    token_ttl_seconds: u64,
}

/// Read the current LiveKit configuration from config.toml + keyring.
#[tauri::command]
fn get_livekit_config(state: tauri::State<'_, AppManagedState>) -> Result<LiveKitSettingsResponse, String> {
    let config_path_str = state.config_path.to_string_lossy().to_string();
    let cfg = config::load_config(Some(&config_path_str))
        .map_err(|e| format!("config error: {e}"))?;

    let has_secret_in_config = !cfg.livekit.api_secret.is_empty();
    let has_secret_in_keyring = load_api_secret_from_keyring()
        .unwrap_or(None)
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    Ok(LiveKitSettingsResponse {
        configured: !cfg.livekit.url.is_empty(),
        url: cfg.livekit.url,
        api_key: cfg.livekit.api_key,
        has_api_secret: has_secret_in_config || has_secret_in_keyring,
        token_ttl_seconds: cfg.livekit.token_ttl_seconds,
    })
}

/// Input from the frontend for saving LiveKit settings.
#[derive(Debug, Clone, Deserialize)]
struct SaveLiveKitInput {
    url: String,
    api_key: String,
    api_secret: String,
    #[serde(default = "default_token_ttl")]
    token_ttl_seconds: u64,
}

fn default_token_ttl() -> u64 {
    3600
}

/// Save LiveKit configuration to config.toml and the API secret to OS keyring.
///
/// If the keyring is unavailable, the secret falls back to config.toml storage
/// with a warning log.
#[tauri::command]
fn save_livekit_config(
    state: tauri::State<'_, AppManagedState>,
    input: SaveLiveKitInput,
) -> Result<(), String> {
    // Try to store secret in keyring first
    let secret_in_keyring = match store_api_secret_in_keyring(&input.api_secret) {
        Ok(()) => {
            tracing::info!("LiveKit API secret stored in OS keyring");
            true
        }
        Err(e) => {
            tracing::warn!("failed to store secret in keyring, storing in config file: {e}");
            false
        }
    };

    let config_path = &state.config_path;
    let contents = std::fs::read_to_string(config_path)
        .map_err(|e| format!("failed to read config: {e}"))?;

    let mut doc: toml::Value = toml::from_str(&contents)
        .map_err(|e| format!("failed to parse config: {e}"))?;

    let table = doc
        .as_table_mut()
        .ok_or("config root is not a TOML table")?;

    let lk = table
        .entry("livekit")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let lk_table = lk
        .as_table_mut()
        .ok_or("[livekit] is not a TOML table")?;

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
        lk_table.insert(
            "api_secret".into(),
            toml::Value::String(input.api_secret),
        );
    }

    let serialized =
        toml::to_string_pretty(&doc).map_err(|e| format!("failed to serialize config: {e}"))?;

    std::fs::write(config_path, serialized)
        .map_err(|e| format!("failed to write config: {e}"))?;

    tracing::info!("LiveKit configuration saved");
    Ok(())
}

/// Clear LiveKit configuration from both config.toml and the OS keyring.
#[tauri::command]
fn clear_livekit_config(state: tauri::State<'_, AppManagedState>) -> Result<(), String> {
    // Remove from keyring
    if let Err(e) = delete_api_secret_from_keyring() {
        tracing::warn!("failed to remove secret from keyring: {e}");
    }

    // Remove from config file
    let config_path = &state.config_path;
    let contents = std::fs::read_to_string(config_path)
        .map_err(|e| format!("failed to read config: {e}"))?;

    let mut doc: toml::Value = toml::from_str(&contents)
        .map_err(|e| format!("failed to parse config: {e}"))?;

    if let Some(table) = doc.as_table_mut() {
        table.remove("livekit");
    }

    let serialized =
        toml::to_string_pretty(&doc).map_err(|e| format!("failed to serialize config: {e}"))?;

    std::fs::write(config_path, serialized)
        .map_err(|e| format!("failed to write config: {e}"))?;

    tracing::info!("LiveKit configuration cleared");
    Ok(())
}

/// Check if a LiveKit server is reachable at the given URL.
#[tauri::command]
async fn check_livekit_reachable(url: String) -> Result<serde_json::Value, String> {
    // LiveKit serves HTTP on the same port as WebSocket.
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

// ── Local LiveKit server management ──

const LIVEKIT_VERSION: &str = "1.7.2";

/// Returns the platform-specific LiveKit server binary name.
fn livekit_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "livekit-server.exe"
    } else {
        "livekit-server"
    }
}

/// Returns the download URL for livekit-server on this platform, if supported.
fn livekit_download_url() -> Option<String> {
    let base = format!(
        "https://github.com/livekit/livekit/releases/download/v{LIVEKIT_VERSION}"
    );
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(format!(
            "{base}/livekit_{LIVEKIT_VERSION}_linux_amd64.tar.gz"
        ))
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(format!(
            "{base}/livekit_{LIVEKIT_VERSION}_linux_arm64.tar.gz"
        ))
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some(format!(
            "{base}/livekit_{LIVEKIT_VERSION}_darwin_amd64.tar.gz"
        ))
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(format!(
            "{base}/livekit_{LIVEKIT_VERSION}_darwin_arm64.tar.gz"
        ))
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(format!(
            "{base}/livekit_{LIVEKIT_VERSION}_windows_amd64.zip"
        ))
    } else {
        None
    }
}

/// Searches PATH for the livekit-server binary.
fn find_livekit_in_path() -> Option<PathBuf> {
    let name = livekit_binary_name();
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

/// Ensures livekit-server is available: checks PATH, then the local bin cache,
/// and downloads it if necessary. Returns the path to the binary.
async fn ensure_livekit(data_dir: &Path) -> Result<PathBuf, String> {
    // 1. Check PATH
    if let Some(path) = find_livekit_in_path() {
        tracing::info!(path = %path.display(), "found livekit-server in PATH");
        return Ok(path);
    }

    // 2. Check local bin cache
    let bin_dir = data_dir.join("bin");
    let lk_path = bin_dir.join(livekit_binary_name());
    if lk_path.exists() {
        tracing::info!(path = %lk_path.display(), "using cached livekit-server");
        return Ok(lk_path);
    }

    // 3. Download
    let url = livekit_download_url()
        .ok_or_else(|| "livekit-server download not supported on this platform".to_string())?;

    tracing::info!(%url, "downloading livekit-server");

    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("failed to create bin directory: {e}"))?;

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("livekit-server download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "livekit-server download failed: HTTP {}",
            resp.status()
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("livekit-server download read failed: {e}"))?;

    if url.ends_with(".tar.gz") {
        let tgz_path = bin_dir.join("livekit.tar.gz");
        std::fs::write(&tgz_path, &bytes)
            .map_err(|e| format!("failed to write livekit archive: {e}"))?;
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
        let zip_path = bin_dir.join("livekit.zip");
        std::fs::write(&zip_path, &bytes)
            .map_err(|e| format!("failed to write livekit archive: {e}"))?;

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
            .map_err(|e| format!("failed to write livekit-server binary: {e}"))?;
    }

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&lk_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("failed to set livekit-server permissions: {e}"))?;
    }

    tracing::info!(path = %lk_path.display(), "livekit-server downloaded successfully");
    Ok(lk_path)
}

/// Start a local LiveKit server instance for desktop host mode.
///
/// Generates random API key/secret, spawns the process, and sets environment
/// variables so the embedded Annex server picks up the LiveKit config.
///
/// Must be called BEFORE `start_embedded_server` for the env vars to take effect.
#[tauri::command]
async fn start_local_livekit(state: tauri::State<'_, AppManagedState>) -> Result<serde_json::Value, String> {
    // Check if already running
    {
        let guard = state.livekit.lock().map_err(|e| e.to_string())?;
        if let Some(ref lk) = *guard {
            return Ok(serde_json::json!({ "url": lk.url }));
        }
    }

    // Check if the embedded server is already running — env vars won't help after that
    {
        let guard = state.server.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Err(
                "embedded server is already running — start local LiveKit before the server, or restart the application".to_string()
            );
        }
    }

    let lk_path = ensure_livekit(&state.data_dir).await?;

    // Generate random API key + secret
    let api_key = format!("annex_{}", uuid::Uuid::new_v4().simple());
    let api_secret = format!("secret_{}", uuid::Uuid::new_v4().simple());

    let port: u16 = 7880;
    let lk_url = format!("ws://127.0.0.1:{port}");

    tracing::info!(path = %lk_path.display(), %port, "starting local livekit-server");

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
        .map_err(|e| format!("failed to start livekit-server: {e}"))?;

    // Read stderr in a background thread to detect readiness and keep pipe open.
    let stderr = child
        .stderr
        .take()
        .ok_or("failed to capture livekit-server stderr")?;
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let reader = BufReader::new(stderr);
        let mut tx = Some(tx);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    tracing::debug!(line = %line, "livekit-server");
                    if let Some(sender) = tx.take() {
                        // LiveKit logs readiness messages containing "started" or "ready"
                        if line.contains("started") || line.contains("ready") || line.contains("listening") {
                            let _ = sender.send(Ok(()));
                            // Continue reading to drain the pipe
                        } else {
                            tx = Some(sender);
                        }
                    }
                }
                Err(e) => {
                    if let Some(sender) = tx.take() {
                        let _ = sender.send(Err(format!("livekit-server stderr error: {e}")));
                    }
                    return;
                }
            }
        }
        if let Some(sender) = tx.take() {
            let _ = sender.send(Err(
                "livekit-server exited before becoming ready".to_string(),
            ));
        }
    });

    // Wait for readiness with timeout
    tokio::time::timeout(std::time::Duration::from_secs(15), rx)
        .await
        .map_err(|_| "livekit-server startup timed out after 15 seconds".to_string())?
        .map_err(|_| "livekit readiness channel dropped".to_string())??;

    // Set env vars so the embedded server picks up LiveKit config.
    // SAFETY: Called before `start_embedded_server` spawns any server threads.
    unsafe {
        std::env::set_var("ANNEX_LIVEKIT_URL", &lk_url);
        std::env::set_var("ANNEX_LIVEKIT_PUBLIC_URL", &lk_url);
        std::env::set_var("ANNEX_LIVEKIT_API_KEY", &api_key);
        std::env::set_var("ANNEX_LIVEKIT_API_SECRET", &api_secret);
    }

    tracing::info!(%lk_url, "local livekit-server ready");

    {
        let mut guard = state.livekit.lock().map_err(|e| e.to_string())?;
        *guard = Some(LiveKitProcessState {
            url: lk_url.clone(),
            child,
        });
    }

    Ok(serde_json::json!({ "url": lk_url }))
}

/// Stop the local LiveKit server if running.
#[tauri::command]
fn stop_local_livekit(state: tauri::State<'_, AppManagedState>) -> Result<(), String> {
    let mut guard = state.livekit.lock().map_err(|e| e.to_string())?;
    if let Some(mut lk) = guard.take() {
        tracing::info!(url = %lk.url, "stopping local livekit-server");
        let _ = lk.child.kill();
        let _ = lk.child.wait();
    }
    Ok(())
}

/// Get the local LiveKit server URL, if a local instance is running.
#[tauri::command]
fn get_local_livekit_url(state: tauri::State<'_, AppManagedState>) -> Option<String> {
    state
        .livekit
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|lk| lk.url.clone()))
}

/// Force the window to use dark mode chrome and a black border.
///
/// Two DWM attributes are set:
///   1. `DWMWA_USE_IMMERSIVE_DARK_MODE` (20) — forces the title bar and
///      window border to use dark-mode colors regardless of the system
///      theme.  Available on Windows 10 20H1 (build 18985) and later.
///      Without this, Windows uses the user's system accent color for
///      the border (which may be orange, blue, etc.).
///   2. `DWMWA_BORDER_COLOR` (34) — overrides the border to pure black.
///      Only available on Windows 11 build 22000+.  Ignored on Win10.
///
/// Both calls are harmless if unsupported — the HRESULT is logged but
/// does not affect the application.
#[cfg(target_os = "windows")]
fn set_dark_window_border(window: &tauri::WebviewWindow) {
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            dw_attribute: u32,
            pv_attribute: *const std::ffi::c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    use raw_window_handle::HasWindowHandle;
    let handle = match window.window_handle() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("failed to get window handle for border color: {e}");
            return;
        }
    };
    let hwnd = match handle.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(h) => h.hwnd.get() as isize,
        _ => return,
    };

    // SAFETY: hwnd is a valid window handle obtained from Tauri.
    // Both calls pass correctly-sized u32 values and are harmless if the
    // attribute is unsupported on the current Windows version.
    unsafe {
        // 1. Dark mode title bar + border (Windows 10 20H1+).
        const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
        let enabled: u32 = 1;
        let hr = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::addr_of!(enabled).cast(),
            std::mem::size_of::<u32>() as u32,
        );
        if hr != 0 {
            eprintln!("DwmSetWindowAttribute(DWMWA_USE_IMMERSIVE_DARK_MODE) returned 0x{hr:08X}");
        }

        // 2. Override border to pure black (Windows 11 22000+).
        const DWMWA_BORDER_COLOR: u32 = 34;
        let black: u32 = 0x00000000; // COLORREF 0x00BBGGRR
        let hr = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            std::ptr::addr_of!(black).cast(),
            std::mem::size_of::<u32>() as u32,
        );
        if hr != 0 {
            // Expected on Windows 10 where DWMWA_BORDER_COLOR is unsupported.
            eprintln!("DwmSetWindowAttribute(DWMWA_BORDER_COLOR) returned 0x{hr:08X}");
        }
    }
}

fn main() {
    let data_dir = resolve_data_dir();
    std::fs::create_dir_all(&data_dir).expect("failed to create Annex data directory");

    let config_path = ensure_config(&data_dir).expect("failed to initialize configuration");

    // Resolve resource paths. When running from a Tauri bundle, bundled
    // resources live next to the executable. During development they are
    // relative to the workspace root.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    // Try bundled resource locations first, then fall back to workspace paths
    // for development builds.
    let resource_base = if exe_dir.join("client").join("dist").exists() {
        exe_dir.clone()
    } else {
        // Development: resources relative to workspace root
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let client_dir = resource_base.join("client").join("dist");
    let upload_dir = data_dir.join("uploads");

    // Resolve the ZK verification key from multiple candidate locations.
    // Priority:
    //   1. Tauri bundle resource directory (platform-specific, set by bundle.resources)
    //   2. Alongside the executable (flat layout)
    //   3. Workspace root (development builds)
    // The server falls back to a dummy vkey if none is found (see lib.rs).
    let vkey_candidates = [
        // Tauri bundle resources (beside exe on Windows/Linux)
        exe_dir.join("membership_vkey.json"),
        // Legacy object-map layout
        exe_dir.join("zk").join("keys").join("membership_vkey.json"),
        // macOS .app bundle Resources directory
        exe_dir
            .parent()
            .map(|p| p.join("Resources").join("membership_vkey.json"))
            .unwrap_or_default(),
        // Workspace root (development)
        resource_base
            .join("zk")
            .join("keys")
            .join("membership_vkey.json"),
    ];
    let zk_vkey = vkey_candidates.iter().find(|p| p.exists());

    // Resolve Piper TTS binary from bundled resources or dev workspace.
    let piper_bin_name = if cfg!(target_os = "windows") {
        "piper.exe"
    } else {
        "piper"
    };
    let piper_candidates = [
        exe_dir.join("piper").join(piper_bin_name),
        exe_dir
            .parent()
            .map(|p| p.join("Resources").join("piper").join(piper_bin_name))
            .unwrap_or_default(),
        resource_base
            .join("assets")
            .join("piper")
            .join(piper_bin_name),
    ];
    let piper_binary = piper_candidates.iter().find(|p| p.exists());

    // Resolve voice models directory.
    let voices_candidates = [
        exe_dir.join("voices"),
        exe_dir
            .parent()
            .map(|p| p.join("Resources").join("voices"))
            .unwrap_or_default(),
        resource_base.join("assets").join("voices"),
    ];
    let voices_dir = voices_candidates.iter().find(|p| p.is_dir());

    // Set environment variables so the embedded server picks up the right paths.
    // SAFETY: Called before any threads are spawned, so this is single-threaded.
    unsafe {
        std::env::set_var("ANNEX_CLIENT_DIR", &client_dir);
        if let Some(vkey_path) = zk_vkey {
            std::env::set_var("ANNEX_ZK_KEY_PATH", vkey_path);
        }
        if let Some(piper_path) = piper_binary {
            std::env::set_var("ANNEX_TTS_BINARY_PATH", piper_path);
        }
        if let Some(voices_path) = voices_dir {
            std::env::set_var("ANNEX_TTS_VOICES_DIR", voices_path);
        }
        std::env::set_var("ANNEX_UPLOAD_DIR", &upload_dir);

        // Set desktop-safe CORS origins if not already configured by the user.
        // Tauri webview origins vary by platform:
        //   macOS/Linux: tauri://localhost
        //   Windows:     https://tauri.localhost
        //   Alternate:   http://tauri.localhost
        // Both are included so the desktop app works on all platforms.
        if std::env::var("ANNEX_CORS_ORIGINS").is_err() {
            std::env::set_var(
                "ANNEX_CORS_ORIGINS",
                "tauri://localhost,https://tauri.localhost,http://tauri.localhost",
            );
        }

        // Load LiveKit API secret from OS keychain if not already in env.
        // This injects the secret before any server thread reads the config.
        if std::env::var("ANNEX_LIVEKIT_API_SECRET").is_err() {
            match load_api_secret_from_keyring() {
                Ok(Some(secret)) => {
                    std::env::set_var("ANNEX_LIVEKIT_API_SECRET", &secret);
                    tracing::info!("loaded LiveKit API secret from OS keychain");
                }
                Ok(None) => {} // No secret stored — voice may be disabled
                Err(e) => {
                    tracing::warn!("failed to load LiveKit secret from keychain: {e}");
                }
            }
        }
    }

    tauri::Builder::default()
        .manage(AppManagedState {
            data_dir,
            config_path,
            server: Mutex::new(None),
            tunnel: Mutex::new(None),
            livekit: Mutex::new(None),
        })
        .setup(|app| {
            #[cfg(target_os = "windows")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    set_dark_window_border(&window);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_startup_mode,
            save_startup_mode,
            clear_startup_mode,
            start_embedded_server,
            start_tunnel,
            stop_tunnel,
            get_tunnel_url,
            export_identity_json,
            get_livekit_config,
            save_livekit_config,
            clear_livekit_config,
            check_livekit_reachable,
            start_local_livekit,
            stop_local_livekit,
            get_local_livekit_url,
        ])
        .run(tauri::generate_context!())
        .expect("error running Annex desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_config_creates_file_with_all_sections() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = ensure_config(dir.path()).expect("ensure_config should succeed");
        assert!(config_path.exists(), "config file must be created");

        let contents = std::fs::read_to_string(&config_path).expect("should read config");

        // Verify all expected sections are present
        assert!(contents.contains("[server]"), "missing [server] section");
        assert!(contents.contains("[database]"), "missing [database] section");
        assert!(contents.contains("[logging]"), "missing [logging] section");
        assert!(contents.contains("[cors]"), "missing [cors] section");

        // Verify the livekit comment block is present
        assert!(
            contents.contains("# [livekit]"),
            "missing commented [livekit] section"
        );
        assert!(
            contents.contains("# url = \"ws://localhost:7880\""),
            "missing commented livekit url"
        );
        assert!(
            contents.contains("# api_key = \"\""),
            "missing commented livekit api_key"
        );
        assert!(
            contents.contains("# api_secret = \"\""),
            "missing commented livekit api_secret"
        );
        assert!(
            contents.contains("# token_ttl_seconds = 3600"),
            "missing commented livekit token_ttl_seconds"
        );
    }

    #[test]
    fn ensure_config_is_valid_toml_with_voice_disabled() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = ensure_config(dir.path()).expect("ensure_config should succeed");
        let config_path_str = config_path.to_string_lossy();

        // The file should parse cleanly via the server config loader.
        // Since the [livekit] section is fully commented out, the TOML parser
        // should see no livekit fields and use LiveKitConfig::default().
        let cfg = annex_server::config::load_config(Some(&config_path_str))
            .expect("config should parse");

        // Voice must be disabled (empty url)
        assert!(cfg.livekit.url.is_empty(), "livekit.url should be empty");
        assert!(
            cfg.livekit.api_key.is_empty(),
            "livekit.api_key should be empty"
        );
        assert!(
            cfg.livekit.api_secret.is_empty(),
            "livekit.api_secret should be empty"
        );
        assert_eq!(
            cfg.livekit.token_ttl_seconds, 3600,
            "livekit.token_ttl_seconds should default to 3600"
        );
    }

    #[test]
    fn ensure_config_is_idempotent() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");

        // First call creates the file
        let path1 = ensure_config(dir.path()).expect("first call should succeed");
        let contents1 = std::fs::read_to_string(&path1).expect("should read");

        // Second call should not overwrite
        let path2 = ensure_config(dir.path()).expect("second call should succeed");
        let contents2 = std::fs::read_to_string(&path2).expect("should read");

        assert_eq!(path1, path2, "paths should match");
        assert_eq!(contents1, contents2, "contents should not change on second call");
    }

    #[test]
    fn ensure_config_creates_db_path_in_data_dir() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = ensure_config(dir.path()).expect("ensure_config should succeed");
        let contents = std::fs::read_to_string(&config_path).expect("should read");

        // Database path should point to the data directory
        let expected_db = dir.path().join("annex.db");
        let expected_db_safe = expected_db.display().to_string().replace('\\', "/");
        assert!(
            contents.contains(&expected_db_safe),
            "config should contain db path: {expected_db_safe}"
        );
    }

    #[test]
    fn fix_backslash_paths_is_noop_for_clean_config() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = ensure_config(dir.path()).expect("ensure_config should succeed");
        let before = std::fs::read_to_string(&config_path).expect("should read");

        fix_backslash_paths(&config_path).expect("fix should succeed");

        let after = std::fs::read_to_string(&config_path).expect("should read");
        assert_eq!(before, after, "clean config should not be modified");
    }
}

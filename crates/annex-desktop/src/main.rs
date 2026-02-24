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
/// already exist. Returns the path to the config file.
fn ensure_config(data_dir: &std::path::Path) -> PathBuf {
    let config_path = data_dir.join("config.toml");
    if !config_path.exists() {
        let db_path = data_dir.join("annex.db");
        let upload_dir = data_dir.join("uploads");
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
allowed_origins = ["tauri://localhost", "https://tauri.localhost"]
"#,
            db_path = db_path.display(),
        );
        let _ = std::fs::write(&config_path, contents);

        // Pre-create the upload directory.
        let _ = std::fs::create_dir_all(&upload_dir);
    }
    config_path
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

/// Tauri-managed application state.
struct AppManagedState {
    data_dir: PathBuf,
    config_path: PathBuf,
    server: Mutex<Option<ServerState>>,
    tunnel: Mutex<Option<TunnelState>>,
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
    let json =
        serde_json::to_string_pretty(&prefs).map_err(|e| format!("serialize error: {e}"))?;
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
async fn start_embedded_server(
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
async fn start_tunnel(
    state: tauri::State<'_, AppManagedState>,
) -> Result<String, String> {
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
                        let _ = sender.send(Err(format!(
                            "cloudflared stderr read error: {e}"
                        )));
                    }
                    return;
                }
            }
        }
        if let Some(sender) = tx.take() {
            let _ = sender.send(Err(
                "cloudflared exited without providing a tunnel URL".to_string(),
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

fn main() {
    let data_dir = resolve_data_dir();
    std::fs::create_dir_all(&data_dir).expect("failed to create Annex data directory");

    let config_path = ensure_config(&data_dir);

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
        // Tauri bundle resources (macOS: ../Resources/, Linux/Windows: beside exe)
        exe_dir.join("zk").join("keys").join("membership_vkey.json"),
        // macOS .app bundle Resources directory
        exe_dir.parent()
            .map(|p| p.join("Resources").join("zk").join("keys").join("membership_vkey.json"))
            .unwrap_or_default(),
        // Workspace root (development)
        resource_base.join("zk").join("keys").join("membership_vkey.json"),
    ];
    let zk_vkey = vkey_candidates.iter().find(|p| p.exists());

    // Set environment variables so the embedded server picks up the right paths.
    // SAFETY: Called before any threads are spawned, so this is single-threaded.
    unsafe {
        std::env::set_var("ANNEX_CLIENT_DIR", &client_dir);
        if let Some(vkey_path) = zk_vkey {
            std::env::set_var("ANNEX_ZK_KEY_PATH", vkey_path);
        }
        std::env::set_var("ANNEX_UPLOAD_DIR", &upload_dir);

        // Set desktop-safe CORS origins if not already configured by the user.
        // Tauri webview origins vary by platform:
        //   macOS/Linux: tauri://localhost
        //   Windows:     https://tauri.localhost
        // Both are included so the desktop app works on all platforms.
        if std::env::var("ANNEX_CORS_ORIGINS").is_err() {
            std::env::set_var(
                "ANNEX_CORS_ORIGINS",
                "tauri://localhost,https://tauri.localhost",
            );
        }
    }

    tauri::Builder::default()
        .manage(AppManagedState {
            data_dir,
            config_path,
            server: Mutex::new(None),
            tunnel: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            get_startup_mode,
            save_startup_mode,
            clear_startup_mode,
            start_embedded_server,
            start_tunnel,
            stop_tunnel,
            get_tunnel_url,
        ])
        .run(tauri::generate_context!())
        .expect("error running Annex desktop");
}

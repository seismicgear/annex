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
use std::path::PathBuf;
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

/// Tauri-managed application state.
struct AppManagedState {
    data_dir: PathBuf,
    config_path: PathBuf,
    server: Mutex<Option<ServerState>>,
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
    let zk_vkey = resource_base
        .join("zk")
        .join("keys")
        .join("membership_vkey.json");
    let upload_dir = data_dir.join("uploads");

    // Set environment variables so the embedded server picks up the right paths.
    // SAFETY: Called before any threads are spawned, so this is single-threaded.
    unsafe {
        std::env::set_var("ANNEX_CLIENT_DIR", &client_dir);
        if zk_vkey.exists() {
            std::env::set_var("ANNEX_ZK_KEY_PATH", &zk_vkey);
        }
        std::env::set_var("ANNEX_UPLOAD_DIR", &upload_dir);
    }

    tauri::Builder::default()
        .manage(AppManagedState {
            data_dir,
            config_path,
            server: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            get_startup_mode,
            save_startup_mode,
            clear_startup_mode,
            start_embedded_server,
        ])
        .run(tauri::generate_context!())
        .expect("error running Annex desktop");
}

//! Annex desktop application — a Tauri wrapper that embeds the Annex server
//! and displays the React client in a native window.
//!
//! On startup the embedded Axum server binds to a free port on localhost,
//! then the Tauri webview navigates to that address.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use annex_server::{config, init_tracing, prepare_server};
use std::net::SocketAddr;
use std::path::PathBuf;
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
    std::env::set_var("ANNEX_CLIENT_DIR", &client_dir);
    if zk_vkey.exists() {
        std::env::set_var("ANNEX_ZK_KEY_PATH", &zk_vkey);
    }
    std::env::set_var("ANNEX_UPLOAD_DIR", &upload_dir);

    tauri::Builder::default()
        .setup(move |app| {
            let config_path_str = config_path.to_string_lossy().to_string();
            let main_window = app
                .get_webview_window("main")
                .expect("main window not found");

            // Start the embedded Axum server in a background task.
            tauri::async_runtime::spawn(async move {
                // Load configuration.
                let config = match config::load_config(Some(&config_path_str)) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("failed to load config: {e}");
                        return;
                    }
                };

                // Initialize tracing (once per process).
                if let Err(e) = init_tracing(&config.logging) {
                    eprintln!("failed to init tracing: {e}");
                    // Non-fatal — continue without structured logging.
                }

                // Prepare the server (DB, state, listener).
                let (listener, router) = match prepare_server(config).await {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::error!("server startup failed: {e}");
                        eprintln!("server startup failed: {e}");
                        return;
                    }
                };

                let addr = listener
                    .local_addr()
                    .expect("listener should have a local address");
                let url = format!("http://127.0.0.1:{}", addr.port());

                tracing::info!(%url, "embedded server ready — navigating window");

                // Navigate the webview to the server.
                let target_url: tauri::Url = url.parse().expect("valid URL");
                if let Err(e) = main_window.navigate(target_url) {
                    tracing::error!("failed to navigate window: {e}");
                }

                // Drive the Axum server until the process exits.
                if let Err(e) = axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                {
                    tracing::error!("server error: {e}");
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error running Annex desktop");
}

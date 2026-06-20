//! Annex desktop application — a Tauri wrapper that can either embed the Annex
//! server or connect to a remote server as a client-only instance.
//!
//! The bundled React frontend loads immediately and presents a startup mode
//! selector. In **Host** mode the embedded Axum server binds to a free port on
//! localhost and the client connects to it. In **Client** mode the webview
//! connects directly to a user-supplied remote server URL.
//!
//! The orchestration is split across the modules below. `main()` only wires
//! the Tauri builder, registers managed state, and routes `invoke_handler`
//! commands to the right modules.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod commands;
mod config;
mod deep_links;
mod embedded_server;
mod keyring;
mod media;
mod public_endpoint;
mod startup_mode;
mod webrtc;
mod window;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::{Emitter, Listener, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

use crate::app_state::AppManagedState;

/// Resolve the on-disk locations a Tauri `bundle.resources` entry can occupy
/// at runtime once the app is *installed* (not just `cargo run`).
///
/// Tauri mangles resource paths that escape the project directory: each leading
/// `../` becomes a `_up_` path component, rooted at the platform resource
/// directory. Annex declares `../../zk/keys/membership_vkey.json`,
/// `../../assets/piper`, and `../../assets/voices`, so at runtime they live
/// under `<resource_root>/_up_/_up_/{zk,assets}/...`.
///
/// Crucially, the resource root is NOT simply the executable's directory:
///   * Windows (NSIS/MSI): beside the exe          -> `<exe_dir>`
///   * Linux (deb/AppImage): `<prefix>/lib/Annex`  -> `<exe_dir>/../lib/Annex`
///   * macOS (.app):       `Contents/Resources`    -> `<exe_dir>/../Resources`
///
/// `suffix` is the path *after* the `../../` prefix (e.g.
/// `["zk", "keys", "membership_vkey.json"]`). Returns one candidate per
/// platform root; callers probe each with `.exists()` / `.is_dir()`.
fn bundled_resource_paths(exe_dir: &Path, suffix: &[&str]) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = [
        Some(exe_dir.to_path_buf()), // Windows: beside exe
        exe_dir.parent().map(|p| p.join("lib").join("Annex")), // Linux deb/AppImage
        exe_dir.parent().map(|p| p.join("Resources")), // macOS .app Contents/Resources
    ]
    .into_iter()
    .flatten()
    .collect();
    roots
        .into_iter()
        .map(|root| {
            let mut p = root.join("_up_").join("_up_");
            for c in suffix {
                p = p.join(c);
            }
            p
        })
        .collect()
}

fn main() {
    // Tauri's async runtime drives `start_embedded_server` (which calls
    // `prepare_server` + builds the axum Router) and the spawned `axum::serve`
    // task. On Windows the default worker-thread stack (~2 MiB) risks the same
    // startup stack overflow the standalone server hit on its 1 MiB main thread
    // — the axum Router + tower layer future is a very large nested type.
    // Install a runtime with a 16 MiB worker/blocking stack BEFORE Tauri
    // lazily creates its default one, and keep it alive for the whole process.
    {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(16 * 1024 * 1024)
            .build()
            .expect("failed to build the desktop Tokio runtime");
        tauri::async_runtime::set(rt.handle().clone());
        std::mem::forget(rt);
    }

    let data_dir = config::resolve_data_dir();
    std::fs::create_dir_all(&data_dir).expect("failed to create Annex data directory");

    let config_path = config::ensure_config(&data_dir).expect("failed to initialize configuration");

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
    //
    // Behaviour when no candidate exists depends on the embedded server's
    // `security.enforce_zk_proofs`:
    //   - Default (enforce_zk_proofs = true, the production posture): the
    //     embedded server will refuse to start with a `StartupError`. We
    //     surface a clear stderr line up-front so the user doesn't have to
    //     fish the error out of the server log.
    //   - Dev override (enforce_zk_proofs = false in the user's config): the
    //     server falls back to the in-memory dummy vkey and accepts no real
    //     proofs. That's documented and only meaningful for local dev.
    let mut vkey_candidates: Vec<PathBuf> = vec![
        // Flat layout beside the exe (legacy / loose copies).
        exe_dir.join("membership_vkey.json"),
        exe_dir.join("zk").join("keys").join("membership_vkey.json"),
        // Workspace root (development / `cargo run`).
        resource_base
            .join("zk")
            .join("keys")
            .join("membership_vkey.json"),
    ];
    // Installed-bundle locations (deb/AppImage/NSIS/.app) — these are where the
    // resource actually lands and were previously unhandled, which left the
    // packaged desktop app unable to find its vkey and (with the default
    // enforce_zk_proofs=true) refusing to start the embedded server.
    vkey_candidates.extend(bundled_resource_paths(
        &exe_dir,
        &["zk", "keys", "membership_vkey.json"],
    ));
    let zk_vkey = vkey_candidates.iter().find(|p| p.exists());
    if zk_vkey.is_none() {
        eprintln!(
            "[annex-desktop] WARNING: no membership_vkey.json found in any of: {}",
            vkey_candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        eprintln!(
            "[annex-desktop] If security.enforce_zk_proofs is true (the default), \
             the embedded server will refuse to start. Reinstall the app or copy \
             the vkey file to one of the locations above."
        );
    }

    // Resolve the v2 membership vkey the same way. With the default
    // `enabled_zk_versions = ["v1","v2"]`, the embedded server loads this at
    // startup; under `enforce_zk_proofs=true` a missing v2 vkey is a hard
    // StartupError, so it must be bundled (see tauri.conf.json bundle.resources)
    // and resolved here just like v1.
    let mut vkey_v2_candidates: Vec<PathBuf> = vec![
        exe_dir.join("membership_v2_vkey.json"),
        exe_dir
            .join("zk")
            .join("keys")
            .join("membership_v2_vkey.json"),
        resource_base
            .join("zk")
            .join("keys")
            .join("membership_v2_vkey.json"),
    ];
    vkey_v2_candidates.extend(bundled_resource_paths(
        &exe_dir,
        &["zk", "keys", "membership_v2_vkey.json"],
    ));
    let zk_vkey_v2 = vkey_v2_candidates.iter().find(|p| p.exists());
    if zk_vkey_v2.is_none() {
        eprintln!(
            "[annex-desktop] WARNING: no membership_v2_vkey.json found; v2 ZK proofs \
             will be unavailable and the embedded server will refuse to start if \
             v2 is enabled under enforce_zk_proofs."
        );
    }

    // Resolve Piper TTS binary from bundled resources or dev workspace.
    let piper_bin_name = if cfg!(target_os = "windows") {
        "piper.exe"
    } else {
        "piper"
    };
    let mut piper_candidates: Vec<PathBuf> = vec![
        exe_dir.join("piper").join(piper_bin_name),
        resource_base
            .join("assets")
            .join("piper")
            .join(piper_bin_name),
    ];
    piper_candidates.extend(bundled_resource_paths(
        &exe_dir,
        &["assets", "piper", piper_bin_name],
    ));
    let piper_binary = piper_candidates.iter().find(|p| p.exists());

    // Resolve voice models directory.
    let mut voices_candidates: Vec<PathBuf> = vec![
        exe_dir.join("voices"),
        resource_base.join("assets").join("voices"),
    ];
    voices_candidates.extend(bundled_resource_paths(&exe_dir, &["assets", "voices"]));
    let voices_dir = voices_candidates.iter().find(|p| p.is_dir());

    // Pre-compute every value that the env::set_var block needs *before*
    // entering the unsafe block. In particular:
    //
    //   * `keyring::load_api_secret_from_keyring()` on Linux uses
    //     libsecret over zbus, which spins up an internal dbus worker
    //     thread on first call. After that thread is alive,
    //     `std::env::set_var` is no longer single-threaded — Rust 1.85
    //     made it `unsafe` precisely because glibc's `setenv` is
    //     undefined behaviour when another thread is concurrently
    //     calling `getenv`. The original code interleaved the keyring
    //     read with the final `set_var`, breaking the "no threads
    //     spawned yet" invariant claimed in the SAFETY comment.
    //
    //   * `std::env::var(name)` is a `getenv` call. We need to do every
    //     such read up-front and stash the result, then write all the
    //     set_var calls in one definitively-single-threaded block.
    //
    // Result: the only thread-spawn between this point and the
    // `unsafe { … }` block below is the keyring crate's worker (Linux
    // only), and it runs before any `set_var` happens.
    let cors_already_set = std::env::var("ANNEX_CORS_ORIGINS").is_ok();
    let webrtc_secret_already_set = std::env::var("ANNEX_WEBRTC_API_SECRET").is_ok();
    let webrtc_secret_from_keyring: Option<String> = if webrtc_secret_already_set {
        None
    } else {
        match keyring::load_api_secret_from_keyring() {
            Ok(Some(secret)) => {
                tracing::info!("loaded WebRTC API secret from OS keychain");
                Some(secret)
            }
            Ok(None) => None, // No secret stored — voice may be disabled
            Err(e) => {
                tracing::warn!("failed to load WebRTC secret from keychain: {e}");
                None
            }
        }
    };

    // Set environment variables so the embedded server picks up the right paths.
    // SAFETY: Every `getenv`-equivalent and every potentially-thread-spawning
    // call (notably the keyring read above) has already completed. No code
    // path between here and the closing `}` of this block spawns a thread or
    // reads the environment, so concurrent access to `environ` is impossible.
    unsafe {
        std::env::set_var("ANNEX_CLIENT_DIR", &client_dir);
        if let Some(vkey_path) = zk_vkey {
            std::env::set_var("ANNEX_ZK_KEY_PATH", vkey_path);
        }
        if let Some(vkey_path_v2) = zk_vkey_v2 {
            std::env::set_var("ANNEX_ZK_KEY_PATH_V2", vkey_path_v2);
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
        //
        // Under `cargo tauri dev`, the Vite dev server (default :5173) loads
        // the UI from `http://localhost:5173` and that origin is NOT in this
        // list. The server relaxes CORS for any `http(s)://localhost[:port]`,
        // `http(s)://127.0.0.1[:port]`, and `http(s)://[::1][:port]` origin
        // when built with `debug_assertions` — see
        // `annex_server::is_dev_localhost_origin`. Release builds keep the
        // strict allowlist.
        if !cors_already_set {
            std::env::set_var(
                "ANNEX_CORS_ORIGINS",
                "tauri://localhost,https://tauri.localhost,http://tauri.localhost",
            );
        }

        // Inject the keyring-loaded WebRTC secret before any server thread
        // reads the config. Loaded above, before this single-threaded block.
        if let Some(ref secret) = webrtc_secret_from_keyring {
            std::env::set_var("ANNEX_WEBRTC_API_SECRET", secret);
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .manage(AppManagedState {
            data_dir,
            config_path,
            server: Mutex::new(None),
            router_session: Mutex::new(None),
            webrtc: Mutex::new(None),
            webrtc_config_override: Mutex::new(None),
            pending_invite: Mutex::new(None),
        })
        .setup(|app| {
            #[cfg(target_os = "linux")]
            {
                media::check_pipewire_available();
            }

            #[cfg(target_os = "windows")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    window::set_dark_window_border(&window);
                    window::setup_webview2_media_permissions(&window);
                }
            }

            // Handle annex:// deep-link URLs for invite acceptance.
            //
            // Two paths:
            //   1. Cold start: the app was launched by a deep link. Load the initial
            //      URL(s) from tauri-plugin-deep-link and emit the same event.
            //   2. Runtime: the app is already open and receives a new deep link.
            //      The listener below handles that case.
            let _handle = app.handle().clone();

            // Cold start: buffer the invite in managed state so the frontend
            // can retrieve it via `get_pending_invite` once the React tree has
            // mounted. This replaces the old fire-and-forget `emit()` which
            // would be lost if the listener wasn't registered yet.
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for raw_url in urls.iter().map(|u| u.as_str()) {
                    if let Some(invite) = deep_links::parse_deep_link_invite(raw_url) {
                        tracing::info!(
                            server = %invite.server,
                            code = %invite.code,
                            "received annex:// invite deep link (cold start) — buffered"
                        );
                        let managed = app.state::<AppManagedState>();
                        if let Ok(mut guard) = managed.pending_invite.lock() {
                            *guard = Some(invite);
                        }
                        // Only buffer the first valid invite (one at a time).
                        break;
                    }
                }
            }

            // Runtime: listen for deep links while the app is already open.
            let handle2 = app.handle().clone();
            app.listen("deep-link://new-url", move |event| {
                let payload = event.payload();
                if let Ok(urls) = serde_json::from_str::<Vec<String>>(payload) {
                    for raw_url in urls {
                        if let Some(invite) = deep_links::parse_deep_link_invite(&raw_url) {
                            tracing::info!(
                                server = %invite.server,
                                code = %invite.code,
                                "received annex:// invite deep link"
                            );
                            let _ = handle2.emit("annex-invite", &invite);
                        }
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            media::media_keepalive_on_window_event(window, event);
        })
        .invoke_handler(tauri::generate_handler![
            startup_mode::get_startup_mode,
            startup_mode::save_startup_mode,
            startup_mode::clear_startup_mode,
            startup_mode::reset_server_data,
            deep_links::get_pending_invite,
            startup_mode::check_first_run_completed,
            startup_mode::mark_first_run_completed,
            embedded_server::start_embedded_server,
            public_endpoint::acquire_public_endpoint,
            public_endpoint::release_public_endpoint,
            public_endpoint::get_public_endpoint,
            commands::export_identity_json,
            webrtc::get_webrtc_config,
            webrtc::start_local_webrtc,
            webrtc::clear_webrtc_env,
            webrtc::check_webrtc_reachable,
            media::get_platform_media_status,
            media::set_media_keepalive,
        ])
        .build(tauri::generate_context!())
        .expect("error building Annex desktop")
        .run(|app_handle, event| {
            // Clean up out-of-process / external state when the event loop is
            // about to exit. `RunEvent::Exit` fires on every normal shutdown
            // (last window closed, quit, OS terminate) after the loop stops.
            if let tauri::RunEvent::Exit = event {
                let state = app_handle.state::<AppManagedState>();
                // The spawned webrtc-server is a `std::process::Child`, whose
                // Drop does NOT terminate the process — without this it orphans
                // and keeps holding its port across restarts.
                webrtc::shutdown_local_webrtc(state.inner());
                // Release the Annex router public-endpoint session so the public
                // HTTPS tunnel isn't left advertised after the local server dies.
                public_endpoint::release_router_session(state.inner());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_resource_paths_cover_installed_layouts() {
        // Mirrors the real deb/AppImage layout: exe at <prefix>/bin, resources
        // under <prefix>/lib/Annex/_up_/_up_/... (verified against the actual
        // `Annex_0.0.1_amd64.deb` bundle contents).
        let exe_dir = Path::new("/usr/bin");
        let paths = bundled_resource_paths(exe_dir, &["zk", "keys", "membership_vkey.json"]);

        // Linux deb/AppImage
        assert!(
            paths.contains(&PathBuf::from(
                "/usr/lib/Annex/_up_/_up_/zk/keys/membership_vkey.json"
            )),
            "must cover Linux <prefix>/lib/Annex resource root, got {paths:?}"
        );
        // Windows beside-exe
        assert!(
            paths.contains(&PathBuf::from(
                "/usr/bin/_up_/_up_/zk/keys/membership_vkey.json"
            )),
            "must cover Windows beside-exe resource root, got {paths:?}"
        );
        // macOS Contents/Resources
        assert!(
            paths.contains(&PathBuf::from(
                "/usr/Resources/_up_/_up_/zk/keys/membership_vkey.json"
            )),
            "must cover macOS Contents/Resources resource root, got {paths:?}"
        );
    }
}

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
#[cfg(target_os = "linux")]
mod appimage_protocol;
mod array_wallet;
mod array_wallet_bridge;
mod commands;
mod config;
mod deep_links;
mod direct_host;
mod embedded_server;
mod host_identity;
mod keyring;
#[cfg(target_os = "macos")]
mod macos_installer;
mod media;
mod profile;
mod public_endpoint;
mod public_transport;
mod startup_mode;
mod updates;
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

/// An installed application must use its sealed resources before probing a
/// build checkout. A checkout can still exist at CARGO_MANIFEST_DIR on a build
/// machine while the macOS sandbox permits stat but denies reading its files.
/// The same ordering keeps loose, potentially stale development assets from
/// replacing the installed verification keys or speech executables.
fn resource_candidates(
    exe_dir: &Path,
    workspace: &Path,
    suffix: &[&str],
    loose: &[&[&str]],
) -> Vec<PathBuf> {
    let mut candidates = bundled_resource_paths(exe_dir, suffix);
    for parts in loose {
        candidates.push(parts.iter().fold(exe_dir.to_path_buf(), |p, c| p.join(c)));
    }
    candidates.push(
        suffix
            .iter()
            .fold(workspace.to_path_buf(), |p, c| p.join(c)),
    );
    candidates
}

/// Resolve installed speech resources first, then loose/development layouts.
fn resolve_speech_asset(
    exe_dir: &Path,
    workspace: &Path,
    directory: &str,
    names: &[&str],
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for name in names {
        candidates.extend(bundled_resource_paths(
            exe_dir,
            &["assets", directory, name],
        ));
    }
    for name in names {
        candidates.push(exe_dir.join(directory).join(name));
        candidates.push(workspace.join("assets").join(directory).join(name));
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// Whether this build's compiled config carries a usable `plugins.updater`.
///
/// Null and empty both count as absent. `tauri.conf.json` omits the key
/// entirely, and a `--config` overlay that set it to `null` would deserialise
/// to the same panic the presence check exists to prevent, so both are treated
/// as "this build has no updater".
fn updater_is_configured(context: &tauri::Context) -> bool {
    updater_config_is_usable(context.config().plugins.0.get("updater"))
}

/// The decision itself, split out from the `Context` so it can be tested.
///
/// Constructing a `tauri::Context` in a unit test means compiling a whole app,
/// so the thing that decides whether a release ships a launchable binary would
/// otherwise be the one line with no test on it.
fn updater_config_is_usable(value: Option<&serde_json::Value>) -> bool {
    value
        .cloned()
        .and_then(|value| serde_json::from_value::<tauri_plugin_updater::Config>(value).ok())
        .is_some_and(|config| {
            !config.pubkey.trim().is_empty()
                && !config.endpoints.is_empty()
                && config
                    .endpoints
                    .iter()
                    .all(|url| url.scheme() == "https" && url.host_str().is_some())
                && !config.dangerous_insecure_transport_protocol
                && !config.dangerous_accept_invalid_certs
                && !config.dangerous_accept_invalid_hostnames
        })
}

fn main() {
    // Declare what this binary is, BEFORE anything spawns a thread.
    //
    // `annex_server::build_profile` defaults a release binary to `production`,
    // because a release binary is what an operator deploys. That default is
    // wrong for this one: the desktop app embeds the same server on loopback
    // for the single person sitting in front of it, and the production gates
    // would refuse to start it — an explicit CORS origin list and a founder
    // bootstrap token are meaningless here.
    //
    // `desktop` keeps the gates that matter for a shipped binary (ZK
    // verification keys and the VRP model must match their pins; a weak or
    // ephemeral signing key is refused) and drops the ones that only make
    // sense when strangers can reach the port.
    //
    // Position is load-bearing. `set_var` is only sound while the process is
    // single-threaded, and the Tokio runtime later in main spawns workers
    // — which is why `embedded_server.rs` routes its WebRTC overrides through
    // a Mutex instead of the environment. This is the last moment it is safe.
    //
    // Only when unset, so `ANNEX_BUILD_PROFILE=dev cargo tauri dev` still
    // works and a developer can still ask for the production gates on purpose.
    if std::env::var_os("ANNEX_BUILD_PROFILE").is_none() {
        std::env::set_var("ANNEX_BUILD_PROFILE", "desktop");
    }

    // Tauri's async runtime drives `start_embedded_server` (which calls
    // `prepare_server` + builds the axum Router) and the spawned `axum::serve`
    // task. On Windows the default worker-thread stack (~2 MiB) risks the same
    // startup stack overflow the standalone server hit on its 1 MiB main thread
    // — the axum Router + tower layer future is a very large nested type.
    // Its construction occurs after all environment writes below.
    let profile = profile::DesktopProfile::resolve(
        profile::profile_argument(std::env::args_os().skip(1))
            .expect("invalid desktop profile arguments"),
        config::resolve_data_dir(),
    )
    .expect("failed to initialize desktop profile");
    let isolated_profile = profile.is_isolated();
    let data_dir = profile.data_dir.clone();
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
    let vkey_candidates = resource_candidates(
        &exe_dir,
        &resource_base,
        &["zk", "keys", "membership_vkey.json"],
        &[
            &["membership_vkey.json"],
            &["zk", "keys", "membership_vkey.json"],
        ],
    );
    let zk_vkey = vkey_candidates.iter().find(|p| p.is_file());
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
    let vkey_v2_candidates = resource_candidates(
        &exe_dir,
        &resource_base,
        &["zk", "keys", "membership_v2_vkey.json"],
        &[
            &["membership_v2_vkey.json"],
            &["zk", "keys", "membership_v2_vkey.json"],
        ],
    );
    let zk_vkey_v2 = vkey_v2_candidates.iter().find(|p| p.is_file());
    if zk_vkey_v2.is_none() {
        eprintln!(
            "[annex-desktop] WARNING: no membership_v2_vkey.json found; v2 ZK proofs \
             will be unavailable and the embedded server will refuse to start if \
             v2 is enabled under enforce_zk_proofs."
        );
    }

    // Resolve the capability / linkage / federation circuit vkeys (AUDIT
    // P4-ID-1) exactly like the membership keys, and set the per-circuit env
    // override the embedded server reads. Under `enforce_zk_proofs=true` a
    // missing one is a hard StartupError, so each must be bundled
    // (tauri.conf.json bundle.resources) and resolvable here.
    let resolve_circuit_vkey = |file: &str| -> Option<PathBuf> {
        resource_candidates(
            &exe_dir,
            &resource_base,
            &["zk", "keys", file],
            &[&[file], &["zk", "keys", file]],
        )
        .into_iter()
        .find(|p| p.is_file())
    };
    let zk_vkey_channel_eligibility = resolve_circuit_vkey("channel_eligibility_vkey.json");
    let zk_vkey_link_pseudonyms = resolve_circuit_vkey("link_pseudonyms_vkey.json");
    let zk_vkey_federation_attestation = resolve_circuit_vkey("federation_attestation_vkey.json");
    for (label, found) in [
        ("channel_eligibility", &zk_vkey_channel_eligibility),
        ("link_pseudonyms", &zk_vkey_link_pseudonyms),
        ("federation_attestation", &zk_vkey_federation_attestation),
    ] {
        if found.is_none() {
            eprintln!(
                "[annex-desktop] WARNING: no {label}_vkey.json found; that ZK circuit's \
                 endpoint will be unavailable (503) and the embedded server will refuse \
                 to start under enforce_zk_proofs."
            );
        }
    }

    // Resolve Piper TTS binary from bundled resources or dev workspace.
    let piper_bin_name = if cfg!(target_os = "windows") {
        "piper.exe"
    } else {
        "piper"
    };
    let piper_candidates = resource_candidates(
        &exe_dir,
        &resource_base,
        &["assets", "piper", piper_bin_name],
        &[&["piper", piper_bin_name]],
    );
    let piper_binary = piper_candidates.iter().find(|p| p.is_file());

    // Whisper ships with sibling DLL/shared-library dependencies. The entire
    // runtime directory is bundled; never rely on the launch working directory.
    let whisper_names: &[&str] = if cfg!(target_os = "windows") {
        &["whisper-cli.exe"]
    } else {
        &["whisper-cli", "whisper"]
    };
    let whisper_binary = resolve_speech_asset(&exe_dir, &resource_base, "whisper", whisper_names);
    let whisper_model =
        resolve_speech_asset(&exe_dir, &resource_base, "models", &["ggml-base.en.bin"]);
    let stt_binary_already_set = std::env::var_os("ANNEX_STT_BINARY_PATH").is_some();
    let stt_model_already_set = std::env::var_os("ANNEX_STT_MODEL_PATH").is_some();

    // Resolve voice models directory.
    let voices_candidates = resource_candidates(
        &exe_dir,
        &resource_base,
        &["assets", "voices"],
        &[&["voices"]],
    );
    let voices_dir = voices_candidates.iter().find(|p| p.is_dir());

    // Resolve the VRP alignment model directory.
    //
    // `annex_vrp::embedding::DEFAULT_MODEL_DIR` is `assets/embedding` RELATIVE
    // to the working directory, which for an installed desktop app is wherever
    // the OS launched it from — almost never the bundle. Without this the
    // server falls back to the lexicon scorer on every desktop install, and the
    // only sign would be `lexicon-v1` in a federation handshake.
    let embedding_candidates = resource_candidates(
        &exe_dir,
        &resource_base,
        &["assets", "embedding"],
        &[&["embedding"]],
    );
    let embedding_dir = embedding_candidates
        .iter()
        .find(|p| p.join("model.safetensors").is_file());

    // Keyring reads happen when loading server configuration, after these
    // environment writes. Keyring backends and Tokio can create worker threads.
    let cors_already_set = std::env::var("ANNEX_CORS_ORIGINS").is_ok();

    // Set environment variables so the embedded server picks up the right paths.
    // SAFETY: No Tokio, Tauri, keyring, or other thread-spawning initialization
    // has run yet. All process environment writes finish before those start.
    unsafe {
        std::env::set_var("ANNEX_CLIENT_DIR", &client_dir);
        if let Some(vkey_path) = zk_vkey {
            std::env::set_var("ANNEX_ZK_KEY_PATH", vkey_path);
        }
        if let Some(vkey_path_v2) = zk_vkey_v2 {
            std::env::set_var("ANNEX_ZK_KEY_PATH_V2", vkey_path_v2);
        }
        if let Some(p) = zk_vkey_channel_eligibility {
            std::env::set_var("ANNEX_ZK_KEY_PATH_CHANNEL_ELIGIBILITY", p);
        }
        if let Some(p) = zk_vkey_link_pseudonyms {
            std::env::set_var("ANNEX_ZK_KEY_PATH_LINK_PSEUDONYMS", p);
        }
        if let Some(p) = zk_vkey_federation_attestation {
            std::env::set_var("ANNEX_ZK_KEY_PATH_FEDERATION_ATTESTATION", p);
        }
        if let Some(piper_path) = piper_binary {
            std::env::set_var("ANNEX_TTS_BINARY_PATH", piper_path);
        }
        if !stt_binary_already_set {
            if let Some(path) = whisper_binary {
                std::env::set_var("ANNEX_STT_BINARY_PATH", path);
            }
        }
        if !stt_model_already_set {
            if let Some(path) = whisper_model {
                std::env::set_var("ANNEX_STT_MODEL_PATH", path);
            }
        }
        if let Some(voices_path) = voices_dir {
            std::env::set_var("ANNEX_TTS_VOICES_DIR", voices_path);
        }
        if let Some(embedding_path) = embedding_dir {
            std::env::set_var("ANNEX_EMBEDDING_MODEL_DIR", embedding_path);
        }
        std::env::set_var("ANNEX_UPLOAD_DIR", &upload_dir);
        if isolated_profile {
            // Isolated profiles generate and retain their own signing key and
            // voice credential instead of inheriting another host's secrets.
            std::env::remove_var("ANNEX_SIGNING_KEY");
            std::env::remove_var("ANNEX_WEBRTC_API_SECRET");
        }

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
    }

    // Keep the runtime alive for the whole app, after the last set_var.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()
        .expect("failed to build the desktop Tokio runtime");
    tauri::async_runtime::set(rt.handle().clone());
    std::mem::forget(rt);

    // The compiled config is read BEFORE the builder, because one plugin can
    // only be registered when the config has something for it to read. See the
    // updater block below.
    let mut context = tauri::generate_context!();
    profile.configure(context.config_mut());

    let builder = tauri::Builder::default()
        // Single instance MUST be registered before the deep-link plugin.
        //
        // Without it, clicking an `annex://` link while Annex is already
        // running does not deliver `deep-link://new-url` to the running
        // process on Linux or Windows — the OS simply starts a second one.
        // Two Annex processes then share a data directory: two embedded
        // servers racing for the same port, and two SQLite connections to
        // the same file from separate processes, where WAL locking is the
        // only thing standing between them and a corrupted database.
        //
        // The `deep-link` feature makes the plugin forward the argv of the
        // second launch — which is where the URL is — into the first, so the
        // existing `deep-link://new-url` listener below receives it and the
        // invite is handled by the instance the user already has open.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tracing::info!("second instance launched — forwarding to the running one");
            // Raise the existing window: the user clicked something and
            // expects Annex to come forward, not to be silently ignored.
            if let Some(window) = app.webview_windows().values().next() {
                let _ = window.set_focus();
                let _ = window.unminimize();
            }
        }));
    let builder = if isolated_profile {
        builder
    } else {
        builder.plugin(tauri_plugin_deep_link::init())
    };

    // The updater is registered only when this build actually carries its
    // config, and that condition is the whole point.
    //
    // `plugins.updater` is deliberately NOT in `tauri.conf.json`: the public key
    // is the operator's, and the release workflow injects it with `--config`
    // from `TAURI_SIGNING_PUBLIC_KEY` alongside `createUpdaterArtifacts`. So a
    // build made WITHOUT that secret — every `cargo tauri dev`, every local
    // `cargo tauri build`, every unsigned dry run — has no `plugins.updater` at
    // all, and registering the plugin unconditionally made such a build panic
    // before its first window:
    //
    //     PluginInitialization("updater", "Error deserializing 'plugins.updater'
    //     within your Tauri configuration: invalid type: null, expected struct
    //     Config")
    //
    // Which is to say the app did not start. Found by `scripts/desktop-audit.sh`
    // — `cargo check`, `cargo clippy`, `cargo test -p annex-desktop` (24 tests)
    // and the bundle build were ALL green on that commit, because none of them
    // runs the binary. The audit installs the `.deb` and launches it under
    // Xvfb, and that is the only step that could have caught this.
    //
    // A signed release still gets the updater, and gets it meaningfully: the
    // plugin verifies each detached signature against the injected public key
    // before applying anything. An unsigned build gets no updater, which is the
    // honest behaviour — there is no key it could verify against.
    let updates_enabled = updater_is_configured(&context);
    let update_endpoints = context
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("endpoints"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let updates = updates::DesktopUpdates::new(
        context.package_info().version.to_string(),
        updates_enabled,
        isolated_profile,
        update_endpoints,
    );
    let restart_environment =
        profile::restart_environment(std::env::args_os()).expect("invalid restart arguments");
    let builder = if updates_enabled {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    } else {
        tracing::info!(
            "no `plugins.updater` in this build's config — updater disabled \
             (a release build injects it from TAURI_SIGNING_PUBLIC_KEY)"
        );
        builder
    };

    builder
        .manage(restart_environment)
        .manage(updates)
        .manage(array_wallet::ArrayWallet::from_environment(&data_dir))
        .manage(AppManagedState {
            data_dir,
            config_path,
            server_lifecycle: tokio::sync::Mutex::new(()),
            keyring_service: profile.identifier.clone(),
            isolated_profile,
            server: Mutex::new(None),
            router_session: Mutex::new(None),
            webrtc: Mutex::new(None),
            webrtc_config_override: Mutex::new(None),
            pending_invite: Mutex::new(deep_links::InviteInbox::default()),
        })
        .setup(move |app| {
            app.state::<array_wallet::ArrayWallet>().start_handshake();
            if isolated_profile {
                for config in &app.config().app.windows {
                    let directory = profile.webview_dir(&config.label)?;
                    tauri::WebviewWindowBuilder::from_config(app, config)?
                        .data_directory(directory)
                        .build()?;
                }
            }
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

            if isolated_profile {
                return Ok(());
            }
            // Subscribe before inspecting the startup URL. Runtime deliveries
            // are buffered even before React mounts, and take precedence over
            // a late cold snapshot. Events only notify the frontend to read.
            let handle = app.handle().clone();
            app.listen("deep-link://new-url", move |event| {
                if let Ok(urls) = serde_json::from_str::<Vec<String>>(event.payload()) {
                    for raw_url in urls {
                        if let Some(invite) = deep_links::parse_deep_link_invite(&raw_url) {
                            let managed = handle.state::<AppManagedState>();
                            let delivery = managed
                                .pending_invite
                                .lock()
                                .ok()
                                .map(|mut inbox| inbox.buffer_runtime(invite));
                            if let Some(delivery) = delivery {
                                tracing::info!("received annex:// invite deep link — buffered");
                                let _ = handle.emit("annex-invite", &delivery);
                            }
                        }
                    }
                }
            });
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for raw_url in urls.iter().map(|url| url.as_str()) {
                    if let Some(invite) = deep_links::parse_deep_link_invite(raw_url) {
                        let managed = app.state::<AppManagedState>();
                        if let Ok(mut inbox) = managed.pending_invite.lock() {
                            inbox.buffer_cold(invite);
                        }
                        break;
                    }
                }
            }

            #[cfg(target_os = "linux")]
            if let Err(error) =
                appimage_protocol::register_if_appimage(app.env().appimage.as_deref(), || {
                    app.deep_link().register_all()
                })
            {
                tracing::error!("{error}");
                rfd::MessageDialog::new()
                    .set_title("Annex invite setup failed")
                    .set_description(&error)
                    .set_level(rfd::MessageLevel::Error)
                    .show();
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            media::media_keepalive_on_window_event(window, event);
        })
        .invoke_handler(tauri::generate_handler![
            updates::get_update_status,
            updates::check_for_updates,
            updates::download_update,
            updates::install_update,
            startup_mode::get_startup_mode,
            startup_mode::save_startup_mode,
            startup_mode::clear_startup_mode,
            startup_mode::reset_server_data,
            deep_links::get_pending_invite,
            deep_links::ack_pending_invite,
            startup_mode::check_first_run_completed,
            startup_mode::mark_first_run_completed,
            embedded_server::start_embedded_server,
            host_identity::get_host_identity_context,
            public_endpoint::acquire_public_endpoint,
            public_endpoint::release_public_endpoint,
            public_endpoint::get_public_endpoint,
            commands::export_identity_json,
            array_wallet::array_wallet_status,
            array_wallet::array_wallet_save_identity,
            array_wallet::array_wallet_list_backups,
            array_wallet::array_wallet_restore_identity,
            array_wallet::array_wallet_disconnect,
            webrtc::get_webrtc_config,
            webrtc::start_local_webrtc,
            webrtc::clear_webrtc_env,
            webrtc::check_webrtc_reachable,
            media::get_platform_media_status,
            media::set_media_keepalive,
        ])
        .build(context)
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
                // Tell the embedded server's background workers to stop. The
                // serve task awaits them; this hook cannot, being synchronous.
                if let Ok(guard) = state.server.lock() {
                    if let Some(server) = guard.as_ref() {
                        server.shutdown.cancel();
                    }
                }
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
    fn installed_keys_and_speech_assets_win_over_stale_build_checkouts() {
        for platform in ["windows", "linux", "macos"] {
            let dir = tempfile::tempdir().unwrap();
            let workspace = dir.path().join("workspace");
            let (exe, resources) = match platform {
                "windows" => {
                    let exe = dir.path().join("installed");
                    (exe.clone(), exe)
                }
                "linux" => (dir.path().join("bin"), dir.path().join("lib/Annex")),
                _ => (
                    dir.path().join("Annex.app/Contents/MacOS"),
                    dir.path().join("Annex.app/Contents/Resources"),
                ),
            };
            for file in [
                "membership_vkey.json",
                "membership_v2_vkey.json",
                "channel_eligibility_vkey.json",
                "link_pseudonyms_vkey.json",
                "federation_attestation_vkey.json",
            ] {
                let installed = resources.join("_up_/_up_/zk/keys").join(file);
                let stale = workspace.join("zk/keys").join(file);
                let loose = exe.join(file);
                for (path, bytes) in [
                    (&installed, b"installed".as_slice()),
                    (&stale, b"stale checkout".as_slice()),
                    (&loose, b"stale loose copy".as_slice()),
                ] {
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(path, bytes).unwrap();
                }
                let candidates = resource_candidates(
                    &exe,
                    &workspace,
                    &["zk", "keys", file],
                    &[&[file], &["zk", "keys", file]],
                );
                let selected = candidates.iter().find(|p| p.is_file()).unwrap();
                assert_eq!(selected, &installed, "{platform}: {file}");
                assert_eq!(std::fs::read(selected).unwrap(), b"installed");
                // Preserve cargo-run and loose-layout support when no installed
                // bundle exists; neither fallback may shadow installed bytes.
                std::fs::remove_file(&installed).unwrap();
                assert_eq!(candidates.iter().find(|p| p.is_file()), Some(&loose));
                std::fs::remove_file(&loose).unwrap();
                assert_eq!(candidates.iter().find(|p| p.is_file()), Some(&stale));
            }
            for suffix in [
                vec!["assets", "piper", "piper"],
                vec!["assets", "voices"],
                vec!["assets", "embedding"],
            ] {
                let installed = suffix
                    .iter()
                    .fold(resources.join("_up_/_up_"), |p, part| p.join(part));
                let stale = suffix
                    .iter()
                    .fold(workspace.clone(), |p, part| p.join(part));
                std::fs::create_dir_all(&installed).unwrap();
                std::fs::create_dir_all(&stale).unwrap();
                let candidates = resource_candidates(&exe, &workspace, &suffix, &[]);
                assert_eq!(candidates.iter().find(|p| p.exists()), Some(&installed));
            }
        }
    }

    #[test]
    fn speech_resources_resolve_installed_windows_and_linux_layouts_before_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let exe_dir = dir.path().join("bin");
        let windows = exe_dir.join("_up_/_up_/assets/whisper/whisper-cli.exe");
        let linux = dir
            .path()
            .join("lib/Annex/_up_/_up_/assets/whisper/whisper-cli");
        let legacy = workspace.join("assets/whisper/whisper");
        let model = dir
            .path()
            .join("lib/Annex/_up_/_up_/assets/models/ggml-base.en.bin");
        for path in [&windows, &linux, &legacy, &model] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"resource placement test").unwrap();
        }
        assert_eq!(
            resolve_speech_asset(&exe_dir, &workspace, "whisper", &["whisper-cli.exe"]),
            Some(windows)
        );
        assert_eq!(
            resolve_speech_asset(&exe_dir, &workspace, "whisper", &["whisper-cli", "whisper"]),
            Some(linux)
        );
        assert_eq!(
            resolve_speech_asset(&exe_dir, &workspace, "models", &["ggml-base.en.bin"]),
            Some(model)
        );
        assert_eq!(
            resolve_speech_asset(&exe_dir, &workspace, "whisper", &["whisper"]),
            Some(legacy)
        );
        assert!(resolve_speech_asset(&exe_dir, &workspace, "whisper", &["missing"]).is_none());
    }

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

#[cfg(test)]
mod updater_config_tests {
    use super::updater_config_is_usable;
    use serde_json::json;

    /// The shipped state: `tauri.conf.json` has no `plugins.updater` because the
    /// public key belongs to the operator and is injected at build time.
    #[test]
    fn an_absent_updater_section_is_not_usable() {
        assert!(!updater_config_is_usable(None));
    }

    /// The case that actually panicked. A `--config` overlay, or a merge that
    /// produced the key with no value, deserialises as `invalid type: null,
    /// expected struct Config` INSIDE the plugin's initialiser — which is after
    /// the point where anything can catch it, so the app exits before its first
    /// window.
    #[test]
    fn a_null_updater_section_is_not_usable() {
        assert!(!updater_config_is_usable(Some(&json!(null))));
    }

    /// `{}` is missing the pubkey and the endpoints, so it is not an updater
    /// either. Tauri would reject it for the same reason.
    #[test]
    fn an_empty_updater_section_is_not_usable() {
        assert!(!updater_config_is_usable(Some(&json!({}))));
    }

    /// What the release workflow injects from `TAURI_SIGNING_PUBLIC_KEY`.
    #[test]
    fn a_populated_updater_section_is_usable() {
        assert!(updater_config_is_usable(Some(&json!({
            "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IHRlc3Q=",
            "endpoints": ["https://example.invalid/latest.json"],
        }))));
    }
}

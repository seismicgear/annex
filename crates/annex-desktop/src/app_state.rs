//! Tauri-managed application state that's shared across commands.
//!
//! All long-lived process handles, paths, and per-session bookkeeping live
//! here. The state is registered with [`tauri::Builder::manage`] in `main()`
//! and read by every command via `tauri::State<'_, AppManagedState>`.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::deep_links::InviteInbox;

/// Tracks whether the embedded server is running.
pub(crate) struct ServerState {
    pub(crate) url: String,
    pub(crate) database_path: PathBuf,
    /// Cancelled on `RunEvent::Exit` to stop the server's background workers.
    ///
    /// The desktop app used to leave them running until the process image was
    /// torn down. That is survivable for a timer and not for the federation
    /// outbox, which can be interrupted between marking a delivery attempted
    /// and sending it — so the peer never receives a message this server
    /// believes it delivered, and the row will not be retried.
    pub(crate) shutdown: tokio_util::sync::CancellationToken,
    /// Set only after the listener and background workers have finished.
    /// False means the bounded drain failed and an update must not proceed.
    pub(crate) shutdown_complete: tokio::sync::watch::Receiver<Option<bool>>,
}

/// Tracks an Annex router session that provides a public endpoint for the
/// embedded server. Unlike the old cloudflared tunnel, this is a first-party
/// integration with the Annex routing layer.
pub(crate) struct RouterSessionState {
    /// The publicly-reachable HTTPS base URL returned by the router.
    pub(crate) public_url: String,
    /// Optional public WebRTC WebSocket URL if the router supports proxying WebRTC.
    pub(crate) public_webrtc_url: Option<String>,
    /// Live transport state, never inferred from a cached URL.
    pub(crate) connected: tokio::sync::watch::Receiver<bool>,
    pub(crate) stop: tokio_util::sync::CancellationToken,
}

/// Tracks the host-mode WebRTC SFU.
///
/// The SFU is embedded **in-process** in the Annex server (`annex-voice` runs
/// it on the `webrtc` crate and signals over the app WebSocket), so there is
/// normally no child process — `child` is `None`. The field is retained as an
/// `Option` so an externally-spawned SFU sidecar (if ever reintroduced) can
/// still be reaped on shutdown.
pub(crate) struct WebRTCProcessState {
    pub(crate) url: String,
    pub(crate) child: Option<std::process::Child>,
}

/// Runtime override for the embedded server's `[webrtc]` config block, set by
/// `start_local_webrtc` and consumed by `start_embedded_server`.
///
/// Why this exists: the previous flow wrote `ANNEX_WEBRTC_URL`,
/// `ANNEX_WEBRTC_PUBLIC_URL`, `ANNEX_WEBRTC_API_KEY`, and
/// `ANNEX_WEBRTC_API_SECRET` via `std::env::set_var` from inside a Tauri
/// command. By the time any Tauri command runs, the Tauri tokio runtime
/// already has worker threads alive, so `set_var` is no longer
/// single-threaded. Rust 1.85 made `set_var` `unsafe` precisely because
/// glibc's `setenv` is undefined behaviour when another thread is calling
/// `getenv` at the same moment — a near-certainty in a multi-threaded
/// process that does TLS init, process spawning (`PATH` lookup), or any
/// other library call that consults the environment. The "SAFETY: Called
/// before `start_embedded_server` spawns any server threads" comment was
/// load-bearing on a foundation that didn't exist.
///
/// The fix: `start_local_webrtc` stores the config here as a plain Rust
/// struct, and `start_embedded_server` applies it to the loaded
/// `annex_server::config::Config` before calling `prepare_server`. The
/// embedded server reads its config from the struct it was given, not from
/// the environment — no `set_var` call required, no UB.
pub(crate) struct WebRtcConfigOverride {
    pub(crate) url: String,
    pub(crate) public_url: String,
    pub(crate) api_key: String,
    pub(crate) api_secret: String,
}

/// Tauri-managed application state.
pub(crate) struct AppManagedState {
    pub(crate) data_dir: PathBuf,
    pub(crate) config_path: PathBuf,
    /// Serializes host startup and reset so deletion cannot race server preparation.
    pub(crate) server_lifecycle: tokio::sync::Mutex<()>,
    pub(crate) keyring_service: String,
    pub(crate) isolated_profile: bool,
    pub(crate) server: Mutex<Option<ServerState>>,
    pub(crate) router_session: Mutex<Option<RouterSessionState>>,
    pub(crate) webrtc: Mutex<Option<WebRTCProcessState>>,
    /// In-memory override for the embedded server's `[webrtc]` config.
    /// Populated by `start_local_webrtc`; consumed by `start_embedded_server`
    /// to avoid the UB-prone `std::env::set_var` path under the live Tauri
    /// runtime. See [`WebRtcConfigOverride`] for the full rationale.
    pub(crate) webrtc_config_override: Mutex<Option<WebRtcConfigOverride>>,
    /// Latest cold/runtime delivery, retained until the frontend acknowledges it.
    pub(crate) pending_invite: Mutex<InviteInbox>,
}

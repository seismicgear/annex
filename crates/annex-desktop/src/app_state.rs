//! Tauri-managed application state that's shared across commands.
//!
//! All long-lived process handles, paths, and per-session bookkeeping live
//! here. The state is registered with [`tauri::Builder::manage`] in `main()`
//! and read by every command via `tauri::State<'_, AppManagedState>`.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::deep_links::DeepLinkInvite;

/// Tracks whether the embedded server is running.
pub(crate) struct ServerState {
    pub(crate) url: String,
}

/// Tracks an Annex router session that provides a public endpoint for the
/// embedded server. Unlike the old cloudflared tunnel, this is a first-party
/// integration with the Annex routing layer.
pub(crate) struct RouterSessionState {
    /// The publicly-reachable HTTPS base URL returned by the router.
    pub(crate) public_url: String,
    /// Optional public WebRTC WebSocket URL if the router supports proxying WebRTC.
    pub(crate) public_webrtc_url: Option<String>,
    /// Session identifier used to release the endpoint on shutdown.
    pub(crate) session_id: String,
}

/// Tracks a locally-managed WebRTC server process.
pub(crate) struct WebRTCProcessState {
    pub(crate) url: String,
    pub(crate) child: std::process::Child,
}

/// Tauri-managed application state.
pub(crate) struct AppManagedState {
    pub(crate) data_dir: PathBuf,
    pub(crate) config_path: PathBuf,
    pub(crate) server: Mutex<Option<ServerState>>,
    pub(crate) router_session: Mutex<Option<RouterSessionState>>,
    pub(crate) webrtc: Mutex<Option<WebRTCProcessState>>,
    /// Buffered cold-start invite parsed before the React listener mounts.
    /// Consumed exactly once via the `get_pending_invite` command.
    pub(crate) pending_invite: Mutex<Option<DeepLinkInvite>>,
}

//! Platform media-capability detection and the WebView2 media keepalive.
//!
//! Two responsibilities, kept together because both report on / influence
//! whether the user can actually capture audio/video:
//!   * Capability reporting — `get_platform_media_status` returns whether
//!     screen sharing and camera/mic are likely to work, plus actionable
//!     warnings (PipeWire / xdg-desktop-portal on Wayland, TCC on macOS,
//!     WebView2 prompts on Windows). The frontend uses this to render a
//!     banner instead of letting users hit silent failures.
//!   * Media keepalive — on Windows, wry sets `IsVisible = false` when the
//!     window is minimized, which kills active `MediaStreamTrack`s. The
//!     `set_media_keepalive` command flips a global flag that the window
//!     event handler watches; when active and the window is minimized, we
//!     re-assert `IsVisible = true` to keep the renderer alive.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// Tri-state media readiness: the runtime permission model on desktop webviews
/// cannot always be verified from Rust, so we expose `unknown` instead of
/// falsely claiming `true`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MediaReadiness {
    /// Permission verified or platform guarantees availability.
    Available,
    /// Cannot verify from Rust — the webview may prompt at runtime.
    Unknown,
    /// Known to be unavailable / blocked.
    Blocked,
}

/// Status of platform media capabilities, exposed to the frontend so users
/// see actionable guidance instead of silent failures.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlatformMediaStatus {
    /// Whether screen sharing is expected to work.
    screen_share_available: bool,
    /// Camera/mic readiness: `available`, `unknown`, or `blocked`.
    camera_mic_available: MediaReadiness,
    /// Human-readable warnings for missing dependencies.
    warnings: Vec<String>,
    /// Display session type (e.g. "wayland", "x11", "windows", "macos").
    display_server: String,
}

/// Detect PipeWire and xdg-desktop-portal on Linux.
/// Returns `(pipewire_ok, portal_ok, warnings)`.
#[cfg(target_os = "linux")]
fn detect_pipewire() -> (bool, bool, Vec<String>) {
    let mut warnings = Vec::new();

    // Check if the PipeWire daemon socket exists (standard XDG path).
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let pipewire_found = if runtime_dir.is_empty() {
        warnings.push("XDG_RUNTIME_DIR not set — cannot detect PipeWire.".into());
        false
    } else {
        let pipewire_socket = std::path::Path::new(&runtime_dir).join("pipewire-0");
        if pipewire_socket.exists() {
            tracing::info!("PipeWire detected (socket: {})", pipewire_socket.display());
            true
        } else {
            // Fallback: check if `pw-cli` is on PATH.
            std::process::Command::new("pw-cli")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    };

    if !pipewire_found {
        warnings.push(
            "PipeWire not detected — screen sharing will not work on Wayland. \
             Install pipewire and wireplumber, then restart the session."
                .into(),
        );
    }

    // Check for xdg-desktop-portal (required for screen sharing prompts on Wayland).
    let portal_running = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.DBus",
            "--type=method_call",
            "--print-reply",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.NameHasOwner",
            "string:org.freedesktop.portal.Desktop",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("true"))
        .unwrap_or(false);

    if !portal_running {
        warnings.push(
            "xdg-desktop-portal not running — screen sharing will not work on Wayland. \
             Install xdg-desktop-portal and a backend (e.g. xdg-desktop-portal-gtk or \
             xdg-desktop-portal-wlr for wlroots compositors)."
                .into(),
        );
    }

    (pipewire_found, portal_running, warnings)
}

/// Log PipeWire/portal warnings at startup on Linux.
#[cfg(target_os = "linux")]
pub(crate) fn check_pipewire_available() {
    let (_, _, warnings) = detect_pipewire();
    for w in &warnings {
        tracing::warn!("{w}");
    }
}

/// Return the display server type on Linux.
#[cfg(target_os = "linux")]
fn detect_display_server() -> String {
    if let Ok(val) = std::env::var("WAYLAND_DISPLAY") {
        if !val.is_empty() {
            return "wayland".into();
        }
    }
    if let Ok(val) = std::env::var("DISPLAY") {
        if !val.is_empty() {
            return "x11".into();
        }
    }
    "unknown".into()
}

/// Query platform media capabilities. Returns structured status the frontend
/// can display as a banner or tooltip.
#[tauri::command]
pub(crate) fn get_platform_media_status() -> PlatformMediaStatus {
    #[cfg(target_os = "linux")]
    {
        let display_server = detect_display_server();
        let (pipewire_ok, portal_ok, warnings) = detect_pipewire();
        let is_wayland = display_server == "wayland";
        PlatformMediaStatus {
            // On X11, screen sharing works via XComposite without PipeWire.
            // On Wayland, both PipeWire and xdg-desktop-portal are required.
            screen_share_available: if is_wayland {
                pipewire_ok && portal_ok
            } else {
                true
            },
            // Linux getUserMedia generally works, but Rust cannot verify the
            // runtime browser permission — report as available (PipeWire
            // audio capture is the only real dependency, and mic works even
            // without PipeWire on ALSA-backed kernels).
            camera_mic_available: MediaReadiness::Available,
            warnings,
            display_server,
        }
    }
    #[cfg(target_os = "macos")]
    {
        // macOS WebKit auto-prompts for camera/mic, but Rust cannot
        // verify TCC permission state at this layer — report unknown
        // and let the webview handle the prompt.
        let mut warnings = Vec::new();
        warnings.push(
            "Camera/microphone access will be requested when you first enable them. \
             Grant permission in System Settings → Privacy & Security if prompted."
                .into(),
        );
        warnings.push(
            "Screen sharing requires Screen Recording permission. \
             Enable it in System Settings → Privacy & Security → Screen Recording if prompted."
                .into(),
        );
        PlatformMediaStatus {
            // macOS Screen Recording permission cannot be verified from Rust —
            // report as boolean true but the frontend knows to treat macOS
            // screen sharing as potentially requiring a grant.
            screen_share_available: true,
            camera_mic_available: MediaReadiness::Unknown,
            warnings,
            display_server: "macos".into(),
        }
    }
    #[cfg(target_os = "windows")]
    {
        // The WebView2 PermissionRequested handler is installed at startup
        // (see setup_webview2_media_permissions) so getUserMedia requests for
        // Camera and Microphone are explicitly allowed. If the OS-level
        // privacy toggle is off, the user still needs to grant in Windows
        // Settings, but the webview layer will no longer silently deny.
        let mut warnings = Vec::new();
        warnings.push(
            "Camera/microphone requests are handled by the app. If devices are not \
             detected, check Windows Settings → Privacy & security → Camera / Microphone."
                .into(),
        );
        PlatformMediaStatus {
            screen_share_available: true,
            camera_mic_available: MediaReadiness::Available,
            warnings,
            display_server: "windows".into(),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        PlatformMediaStatus {
            screen_share_available: false,
            camera_mic_available: MediaReadiness::Blocked,
            warnings: vec!["Unsupported platform.".into()],
            display_server: "unknown".into(),
        }
    }
}

// ── Media keepalive (prevent WebView2 from killing tracks on minimize) ──

/// When true, the webview's `IsVisible` property is forced to `true` even
/// when the window is minimized.
///
/// **Why this is needed:** wry v0.22+ sets `ICoreWebView2Controller::IsVisible = false`
/// when the window is minimized (following Microsoft's performance guidance). This
/// triggers Chromium's page-hidden optimizations which kill active `MediaStreamTrack`s
/// (mic, camera, screen share). By immediately overriding `IsVisible` back to `true`,
/// the Chromium renderer stays active and media tracks survive.
///
/// The frontend toggles this flag via the `set_media_keepalive` command when
/// joining or leaving a voice call.
///
/// See: <https://github.com/MicrosoftEdge/WebView2Feedback/issues/2177>
static MEDIA_KEEPALIVE: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub(crate) fn set_media_keepalive(active: bool) {
    MEDIA_KEEPALIVE.store(active, Ordering::SeqCst);
    tracing::debug!(active, "media keepalive toggled");
}

/// On Windows, override `IsVisible = true` when the window is minimized
/// and a voice call is active. This is called from the window-event handler
/// via `tauri::async_runtime::spawn` to avoid deadlocking `with_webview`
/// in a synchronous event callback.
///
/// The async spawn dispatches back to the main thread via `PostMessage`,
/// so there is a ~1 message-loop-iteration delay. This is fast enough
/// because Chromium's browser process batches visibility state changes
/// asynchronously — the rapid `false→true` transition is seen as a no-op.
#[cfg(target_os = "windows")]
pub(crate) fn media_keepalive_on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    use tauri::Manager;

    if !MEDIA_KEEPALIVE.load(Ordering::SeqCst) {
        return;
    }

    let should_override = match event {
        // wry fires Resized when the window is minimized (SIZE_MINIMIZED).
        tauri::WindowEvent::Resized(_) => window.is_minimized().unwrap_or(false),
        // Also re-assert on focus-gain as a safety net, in case something
        // else set IsVisible=false (e.g. the OS power manager).
        tauri::WindowEvent::Focused(true) => true,
        _ => false,
    };

    if should_override {
        let handle = window.app_handle().clone();
        // Spawn async to avoid with_webview deadlock in synchronous handler.
        tauri::async_runtime::spawn(async move {
            if let Some(ww) = handle.get_webview_window("main") {
                let _ = ww.with_webview(|wv| {
                    // SAFETY: SetIsVisible is a standard COM call on the UI thread.
                    // The async_runtime dispatches this closure to the main thread
                    // via PostMessage, so we are on the correct apartment.
                    unsafe {
                        wv.controller().SetIsVisible(true.into()).ok();
                    }
                });
            }
        });
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn media_keepalive_on_window_event(
    _window: &tauri::Window,
    _event: &tauri::WindowEvent,
) {
    // macOS (WebKit) and Linux (WebKitGTK) do not set IsVisible on minimize.
    // Their webview engines handle background media differently and typically
    // do not kill MediaStreamTracks when the window is unfocused/minimized.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_platform_media_status_returns_valid_response() {
        let status = get_platform_media_status();
        // On any platform, display_server should not be empty.
        assert!(
            !status.display_server.is_empty(),
            "display_server should not be empty"
        );
        // The warnings list should be valid (possibly empty on Linux X11).
        // macOS reports camera_mic as `unknown`; Windows reports `available`
        // because the PermissionRequested handler is installed at startup.
        #[cfg(target_os = "macos")]
        {
            assert!(status.screen_share_available);
            assert!(
                matches!(status.camera_mic_available, MediaReadiness::Unknown),
                "macOS should report camera_mic as unknown"
            );
        }
        #[cfg(target_os = "windows")]
        {
            assert!(status.screen_share_available);
            assert!(
                matches!(status.camera_mic_available, MediaReadiness::Available),
                "Windows should report camera_mic as available (permission handler installed)"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detect_display_server_returns_known_type() {
        let ds = detect_display_server();
        assert!(
            ["wayland", "x11", "unknown"].contains(&ds.as_str()),
            "display_server should be wayland, x11, or unknown, got: {ds}"
        );
    }

    /// Verify that the Windows media status reflects the PermissionRequested
    /// handler being installed (camera_mic reports Available, not Unknown).
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_media_status_reports_available_with_permission_handler() {
        let status = get_platform_media_status();
        assert_eq!(status.display_server, "windows");
        assert!(
            matches!(status.camera_mic_available, MediaReadiness::Available),
            "Windows should report camera_mic as Available when permission handler is installed"
        );
        assert!(status.screen_share_available);
    }
}

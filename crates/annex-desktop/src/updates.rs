//! User-initiated, signed desktop updates. The webview never supplies URLs,
//! signatures, public keys, installer arguments, or executable bytes.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::Emitter;
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateStatus {
    phase: String,
    current_version: String,
    available_version: Option<String>,
    notes: Option<String>,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    message: Option<String>,
    endpoints: Vec<String>,
}

struct UpdateState {
    status: UpdateStatus,
    update: Option<Update>,
    verified_bytes: Option<Vec<u8>>,
}

pub(crate) struct DesktopUpdates {
    inner: Mutex<UpdateState>,
}

impl DesktopUpdates {
    pub(crate) fn new(
        version: String,
        enabled: bool,
        isolated: bool,
        endpoints: Vec<String>,
    ) -> Self {
        Self {
            inner: Mutex::new(UpdateState {
                status: UpdateStatus {
                    phase: if enabled && !isolated {
                        "idle"
                    } else {
                        "unavailable"
                    }
                    .into(),
                    current_version: version,
                    available_version: None,
                    notes: None,
                    downloaded_bytes: 0,
                    total_bytes: None,
                    endpoints,
                    message: if isolated {
                        Some("Updates are disabled for isolated profiles. Update the installed application from its regular profile.".into())
                    } else if !enabled {
                        Some("This build has no valid signed update configuration.".into())
                    } else {
                        None
                    },
                },
                update: None,
                verified_bytes: None,
            }),
        }
    }
}

fn available(state: &UpdateState) -> Result<(), String> {
    match state.status.phase.as_str() {
        "unavailable" => Err(state
            .status
            .message
            .clone()
            .unwrap_or_else(|| "Updates are unavailable.".into())),
        "checking" | "downloading" | "verifying" | "installing" => {
            Err("An update action is already running.".into())
        }
        _ => Ok(()),
    }
}

fn publish(app: &tauri::AppHandle, state: &DesktopUpdates) -> Result<UpdateStatus, String> {
    let status = state
        .inner
        .lock()
        .map_err(|_| "Update state is unavailable.")?
        .status
        .clone();
    let _ = app.emit("annex-update-status", &status);
    Ok(status)
}

fn fail(
    app: &tauri::AppHandle,
    state: &DesktopUpdates,
    message: String,
) -> Result<UpdateStatus, String> {
    {
        let mut guard = state
            .inner
            .lock()
            .map_err(|_| "Update state is unavailable.")?;
        guard.status.phase = "failed".into();
        guard.status.message = Some(message);
    }
    publish(app, state)
}

// Error strings from HTTP libraries may contain private endpoint URLs. Return
// actionable categories, never raw request URLs, manifest fields or signatures.
fn update_error(error: &tauri_plugin_updater::Error, action: &str) -> String {
    use tauri_plugin_updater::Error;
    match error {
        Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) =>
            "The update signature could not be verified. Nothing was installed.".into(),
        Error::ReleaseNotFound | Error::TargetNotFound(_) | Error::TargetsNotFound(_) =>
            "The update service has no valid release for this platform. Try checking again later.".into(),
        Error::InsecureTransportProtocol => "The update service must use HTTPS.".into(),
        _ => format!("Could not {action} the update. Nothing was installed. Check your connection and try again."),
    }
}

#[tauri::command]
pub(crate) fn get_update_status(
    state: tauri::State<'_, DesktopUpdates>,
) -> Result<UpdateStatus, String> {
    Ok(state
        .inner
        .lock()
        .map_err(|_| "Update state is unavailable.")?
        .status
        .clone())
}

#[tauri::command]
pub(crate) async fn check_for_updates(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopUpdates>,
) -> Result<UpdateStatus, String> {
    {
        let mut guard = state
            .inner
            .lock()
            .map_err(|_| "Update state is unavailable.")?;
        available(&guard)?;
        guard.update = None;
        guard.verified_bytes = None;
        guard.status.phase = "checking".into();
        guard.status.message = None;
        guard.status.available_version = None;
        guard.status.notes = None;
        guard.status.downloaded_bytes = 0;
        guard.status.total_bytes = None;
    }
    publish(&app, &state)?;
    let updater = match app
        .updater_builder()
        .timeout(Duration::from_secs(30))
        .build()
    {
        Ok(updater) => updater,
        Err(error) => return fail(&app, &state, update_error(&error, "check for")),
    };
    match updater.check().await {
        Ok(update) => {
            let mut guard = state
                .inner
                .lock()
                .map_err(|_| "Update state is unavailable.")?;
            guard.status.phase = if update.is_some() {
                "available"
            } else {
                "up_to_date"
            }
            .into();
            guard.status.available_version = update.as_ref().map(|item| item.version.clone());
            guard.status.notes = update.as_ref().and_then(|item| item.body.clone());
            guard.update = update;
        }
        Err(error) => return fail(&app, &state, update_error(&error, "check for")),
    }
    publish(&app, &state)
}

#[tauri::command]
pub(crate) async fn download_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopUpdates>,
) -> Result<UpdateStatus, String> {
    let update = {
        let mut guard = state
            .inner
            .lock()
            .map_err(|_| "Update state is unavailable.")?;
        available(&guard)?;
        let update = guard
            .update
            .clone()
            .ok_or("Check for an available update first.")?;
        guard.verified_bytes = None;
        guard.status.phase = "downloading".into();
        guard.status.message = None;
        guard.status.downloaded_bytes = 0;
        guard.status.total_bytes = None;
        update
    };
    publish(&app, &state)?;
    let result = update
        .download(
            |size, total| {
                if let Ok(mut guard) = state.inner.lock() {
                    guard.status.downloaded_bytes =
                        guard.status.downloaded_bytes.saturating_add(size as u64);
                    guard.status.total_bytes = total;
                }
                let _ = publish(&app, &state);
            },
            || {
                if let Ok(mut guard) = state.inner.lock() {
                    guard.status.phase = "verifying".into();
                }
                let _ = publish(&app, &state);
            },
        )
        .await;
    match result {
        Ok(bytes) => {
            let mut guard = state
                .inner
                .lock()
                .map_err(|_| "Update state is unavailable.")?;
            // download() returns only after the plugin verifies the detached
            // signature against this binary's compiled public key.
            guard.verified_bytes = Some(bytes);
            guard.status.phase = "verified".into();
        }
        Err(error) => return fail(&app, &state, update_error(&error, "download")),
    }
    publish(&app, &state)
}

#[tauri::command]
pub(crate) async fn install_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopUpdates>,
) -> Result<UpdateStatus, String> {
    let (update, bytes) = {
        let mut guard = state
            .inner
            .lock()
            .map_err(|_| "Update state is unavailable.")?;
        available(&guard)?;
        let update = guard
            .update
            .clone()
            .ok_or("Check for an available update first.")?;
        let bytes = guard
            .verified_bytes
            .take()
            .ok_or("Download and verify the update before installing.")?;
        guard.status.phase = "installing".into();
        guard.status.message = None;
        (update, bytes)
    };
    publish(&app, &state)?;
    if let Err(message) = crate::embedded_server::stop_for_update(&app).await {
        state
            .inner
            .lock()
            .map_err(|_| "Update state is unavailable.")?
            .verified_bytes = Some(bytes);
        return fail(&app, &state, message);
    }
    // The Windows plugin exits after launching its installer; other platforms
    // return after replacing files and must restart through Tauri.
    let result = tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await;
    match result {
        Ok(Ok(())) => app.restart(),
        Ok(Err(error)) => fail(&app, &state, update_error(&error, "install")),
        Err(_) => fail(
            &app,
            &state,
            "The update installer stopped unexpectedly. Restart Annex and try again.".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_and_isolated_builds_never_offer_update_actions() {
        for (enabled, isolated) in [(false, false), (false, true), (true, true)] {
            let state = DesktopUpdates::new("0.1.0".into(), enabled, isolated, vec![]);
            let state = state.inner.lock().unwrap();
            assert_eq!(state.status.phase, "unavailable");
            assert!(available(&state).is_err());
            assert!(state.update.is_none());
            assert!(state.verified_bytes.is_none());
        }
    }

    #[test]
    fn overlapping_update_actions_are_rejected_and_network_errors_are_redacted() {
        let state = DesktopUpdates::new("0.1.0".into(), true, false, vec![]);
        let mut state = state.inner.lock().unwrap();
        for phase in ["checking", "downloading", "verifying", "installing"] {
            state.status.phase = phase.into();
            assert!(available(&state).is_err());
        }
        let message = update_error(
            &tauri_plugin_updater::Error::Network("https://private/?secret=credential".into()),
            "download",
        );
        assert!(!message.contains("private"));
        assert!(!message.contains("credential"));
    }
}

//! User-initiated, signed desktop updates. The webview never supplies URLs,
//! signatures, public keys, installer arguments, or executable bytes.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::Emitter;
use tauri_plugin_updater::{Update, UpdaterExt};

const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_STALLED: &str =
    "The update download stopped receiving data. Nothing was installed. Try Download update again.";

/// Limit inactivity, not total download time: a large package on a slow
/// connection remains valid while it continues to deliver bytes.
async fn until_download_stalls<F: std::future::Future>(
    download: F,
    mut progress: tokio::sync::watch::Receiver<tokio::time::Instant>,
    idle_timeout: Duration,
) -> Result<F::Output, &'static str> {
    tokio::pin!(download);
    let idle = tokio::time::sleep_until(*progress.borrow_and_update() + idle_timeout);
    tokio::pin!(idle);
    let mut observing = true;
    loop {
        tokio::select! {
            biased;
            result = &mut download => return Ok(result),
            changed = progress.changed(), if observing => {
                if changed.is_ok() {
                    idle.as_mut().reset(*progress.borrow_and_update() + idle_timeout);
                } else {
                    observing = false;
                }
            }
            _ = &mut idle => return Err(DOWNLOAD_STALLED),
        }
    }
}

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
        _ if action == "install" => "The update installer could not finish. Close Annex and run the current installer to repair the installation.".into(),
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
    let (progress, last_progress) = tokio::sync::watch::channel(tokio::time::Instant::now());
    let download = update.download(
        |size, total| {
            if size > 0 {
                progress.send_replace(tokio::time::Instant::now());
            }
            if let Ok(mut guard) = state.inner.lock() {
                guard.status.downloaded_bytes =
                    guard.status.downloaded_bytes.saturating_add(size as u64);
                guard.status.total_bytes = total;
            }
            let _ = publish(&app, &state);
        },
        || {
            progress.send_replace(tokio::time::Instant::now());
            if let Ok(mut guard) = state.inner.lock() {
                guard.status.phase = "verifying".into();
            }
            let _ = publish(&app, &state);
        },
    );
    let result = match until_download_stalls(download, last_progress, DOWNLOAD_IDLE_TIMEOUT).await {
        Ok(result) => result,
        Err(message) => return fail(&app, &state, message.into()),
    };
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn download_fixture(mode: &str) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/update", listener.local_addr().unwrap());
        let mode = mode.to_owned();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let mut length = 0;
            loop {
                let read = socket.read(&mut request[length..]).await.unwrap();
                assert!(read > 0, "Client closed before completing its request");
                length += read;
                if request[..length].ends_with(b"\r\n\r\n") {
                    break;
                }
                assert!(length < request.len(), "Fixture request exceeded its bound");
            }
            if mode != "no-headers" {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                socket.write_all(b"a").await.unwrap();
            }
            if mode == "slow-progress" {
                for _ in 1..10 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    socket.write_all(b"a").await.unwrap();
                }
            } else {
                // An open connection with no more bytes, not a closed socket
                // or mocked rejection. The client must detect inactivity.
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
        (url, task)
    }

    async fn read_fixture(
        url: String,
        progress: tokio::sync::watch::Sender<tokio::time::Instant>,
    ) -> Result<Vec<u8>, reqwest::Error> {
        let mut response = reqwest::Client::builder()
            .no_proxy()
            .build()?
            .get(url)
            .send()
            .await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if !chunk.is_empty() {
                progress.send_replace(tokio::time::Instant::now());
            }
            bytes.extend(chunk);
        }
        Ok(bytes)
    }

    #[tokio::test]
    async fn real_http_stalls_before_headers_and_after_partial_body_release_download() {
        for mode in ["no-headers", "partial-body"] {
            let (url, server) = download_fixture(mode).await;
            let (progress, observed) = tokio::sync::watch::channel(tokio::time::Instant::now());
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                until_download_stalls(
                    read_fixture(url, progress),
                    observed,
                    Duration::from_millis(400),
                ),
            )
            .await
            .unwrap();
            server.abort();
            assert_eq!(result.unwrap_err(), DOWNLOAD_STALLED, "{mode}");
        }
    }

    #[tokio::test]
    async fn slow_real_download_outlives_idle_timeout_when_bytes_keep_arriving() {
        let (url, server) = download_fixture("slow-progress").await;
        let (progress, observed) = tokio::sync::watch::channel(tokio::time::Instant::now());
        let started = tokio::time::Instant::now();
        let bytes = until_download_stalls(
            read_fixture(url, progress),
            observed,
            Duration::from_millis(400),
        )
        .await
        .unwrap()
        .unwrap();
        server.await.unwrap();
        assert_eq!(bytes, b"aaaaaaaaaa");
        assert!(started.elapsed() > Duration::from_millis(800));
    }

    #[tokio::test]
    async fn download_failure_does_not_become_a_verified_result() {
        let (progress, observed) = tokio::sync::watch::channel(tokio::time::Instant::now());
        drop(progress);
        let result = until_download_stalls(
            async { Err::<Vec<u8>, _>("signature refused") },
            observed,
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(result, Ok(Err("signature refused")));
    }

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
        let installer_message = update_error(
            &tauri_plugin_updater::Error::Network("private installer error".into()),
            "install",
        );
        assert!(!installer_message.contains("Nothing was installed"));
        assert!(!installer_message.contains("private"));
    }
}

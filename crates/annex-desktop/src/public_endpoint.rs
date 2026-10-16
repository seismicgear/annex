//! First-party public hosting through monolithannex.com.

use crate::app_state::{AppManagedState, RouterSessionState};
use serde::Serialize;

const DEFAULT_GATEWAY: &str = "https://www.monolithannex.com";

#[tauri::command]
pub(crate) async fn acquire_public_endpoint(
    state: tauri::State<'_, AppManagedState>,
) -> Result<String, String> {
    let _lifecycle = state.server_lifecycle.lock().await;
    let (local_url, database_path) = {
        let guard = state.server.lock().map_err(|e| e.to_string())?;
        let server = guard.as_ref().ok_or("Start the embedded server first")?;
        (server.url.clone(), server.database_path.clone())
    };
    // No public listener is established before the local founder exists.
    let context = tokio::task::spawn_blocking(move || {
        crate::host_identity::read_host_identity(&database_path)
    })
    .await
    .map_err(|e| e.to_string())??;
    if context.founder_pseudonym_id.is_none() {
        return Err("Create the host founder before enabling public access".into());
    }
    let existing = state
        .router_session
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .map(|session| (session.public_url.clone(), session.connected.clone()));
    let (public_url, mut connected) = match existing {
        Some(existing) => existing,
        None => {
            let directory = state.data_dir.clone();
            let key =
                tokio::task::spawn_blocking(move || crate::public_transport::load_key(&directory))
                    .await
                    .map_err(|e| e.to_string())??;
            let gateway = std::env::var("ANNEX_PUBLIC_HOST_URL")
                .unwrap_or_else(|_| DEFAULT_GATEWAY.to_string());
            let transport = crate::public_transport::start(local_url.clone(), gateway, key)?;
            let public_url = transport.public_url.clone();
            let connected = transport.connected.clone();
            *state.router_session.lock().map_err(|e| e.to_string())? = Some(RouterSessionState {
                public_webrtc_url: Some(format!(
                    "{}/ws",
                    public_url
                        .replacen("https://", "wss://", 1)
                        .replacen("http://", "ws://", 1)
                )),
                public_url: public_url.clone(),
                connected: transport.connected,
                stop: transport.stop,
            });
            (public_url, connected)
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !*connected.borrow_and_update() {
            connected
                .changed()
                .await
                .map_err(|_| "Public hosting stopped")?;
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| {
        "Could not establish public hosting within 30 seconds. Check your connection and retry."
            .to_string()
    })??;
    // A registered socket alone is not proof that the route forwards requests.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let public = crate::public_transport::verify_public_summary(&public_url).await?;
    let local: serde_json::Value = client
        .get(format!("{local_url}/api/public/server/summary"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if public.get("slug").is_none() || public.get("slug") != local.get("slug") {
        return Err("The public route did not reach this hosted server".into());
    }
    Ok(public_url)
}

pub(crate) fn release_router_session(state: &AppManagedState) {
    if let Some(session) = state
        .router_session
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
    {
        session.stop.cancel();
    }
}

#[tauri::command]
pub(crate) fn release_public_endpoint(
    state: tauri::State<'_, AppManagedState>,
) -> Result<(), String> {
    release_router_session(state.inner());
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PublicEndpointInfo {
    public_url: String,
    public_webrtc_url: Option<String>,
}

#[tauri::command]
pub(crate) fn get_public_endpoint(
    state: tauri::State<'_, AppManagedState>,
) -> Option<PublicEndpointInfo> {
    state.router_session.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .filter(|session| *session.connected.borrow())
            .map(|session| PublicEndpointInfo {
                public_url: session.public_url.clone(),
                public_webrtc_url: session.public_webrtc_url.clone(),
            })
    })
}

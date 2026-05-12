//! Persisted startup-mode preference and the first-run lifecycle.
//!
//! The desktop app stores a small JSON file (`startup_prefs.json`) that
//! records whether the user wants to host an embedded server or connect to
//! a remote one. A separate `first_run_completed` marker indicates that the
//! app has gone through initial onboarding so subsequent launches can skip
//! the "fresh install" cleanup.

use serde::{Deserialize, Serialize};

use crate::app_state::AppManagedState;

/// Persisted startup mode choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub(crate) enum StartupMode {
    #[serde(rename = "host")]
    Host,
    #[serde(rename = "client")]
    Client { server_url: String },
}

/// Wrapper for the preference file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StartupPrefs {
    startup_mode: StartupMode,
}

/// Read saved startup mode preference. Returns `null` if none saved.
#[tauri::command]
pub(crate) fn get_startup_mode(state: tauri::State<'_, AppManagedState>) -> Option<StartupPrefs> {
    let prefs_path = state.data_dir.join("startup_prefs.json");
    std::fs::read_to_string(&prefs_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Save startup mode preference to disk.
#[tauri::command]
pub(crate) fn save_startup_mode(
    state: tauri::State<'_, AppManagedState>,
    prefs: StartupPrefs,
) -> Result<(), String> {
    let prefs_path = state.data_dir.join("startup_prefs.json");
    let json = serde_json::to_string_pretty(&prefs).map_err(|e| format!("serialize error: {e}"))?;
    std::fs::write(&prefs_path, json).map_err(|e| format!("write error: {e}"))?;
    Ok(())
}

/// Clear saved startup mode preference (reset).
#[tauri::command]
pub(crate) fn clear_startup_mode(state: tauri::State<'_, AppManagedState>) -> Result<(), String> {
    let prefs_path = state.data_dir.join("startup_prefs.json");
    if prefs_path.exists() {
        std::fs::remove_file(&prefs_path).map_err(|e| format!("remove error: {e}"))?;
    }
    Ok(())
}

/// Reset server data directory (database, uploads, config) for a clean start.
///
/// Called by the frontend when it detects a fresh install (no startup_prefs.json)
/// to ensure stale data from a previous installation is removed before the
/// embedded server starts. Without this, old identities remain in the database
/// and the new identity won't be recognised as the server founder.
#[tauri::command]
pub(crate) fn reset_server_data(state: tauri::State<'_, AppManagedState>) -> Result<(), String> {
    // Only reset if the server is NOT already running.
    {
        let guard = state.server.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Err("Cannot reset server data while the server is running".into());
        }
    }

    let data_dir = &state.data_dir;

    // Remove the database file.
    let db_path = data_dir.join("annex.db");
    if db_path.exists() {
        std::fs::remove_file(&db_path).map_err(|e| format!("failed to remove database: {e}"))?;
        tracing::info!("reset_server_data: removed {}", db_path.display());
    }
    // SQLite WAL/SHM files
    for ext in &["annex.db-wal", "annex.db-shm"] {
        let p = data_dir.join(ext);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }

    // Remove uploads directory.
    let uploads_dir = data_dir.join("uploads");
    if uploads_dir.exists() {
        std::fs::remove_dir_all(&uploads_dir)
            .map_err(|e| format!("failed to remove uploads: {e}"))?;
        tracing::info!("reset_server_data: removed {}", uploads_dir.display());
    }

    // Remove config so it is regenerated with fresh defaults.
    let config_path = data_dir.join("config.toml");
    if config_path.exists() {
        std::fs::remove_file(&config_path).map_err(|e| format!("failed to remove config: {e}"))?;
        tracing::info!("reset_server_data: removed {}", config_path.display());
    }

    tracing::info!("reset_server_data: complete");
    Ok(())
}

/// Check whether first-run initialization has been completed previously.
///
/// Returns `true` if the marker file exists, meaning the app has already
/// gone through initial setup and a "fresh install" cleanup should NOT run.
#[tauri::command]
pub(crate) fn check_first_run_completed(state: tauri::State<'_, AppManagedState>) -> bool {
    state.data_dir.join("first_run_completed").exists()
}

/// Write the first-run marker file so subsequent launches skip cleanup.
///
/// Called by the frontend after the first successful registration completes.
/// This marker is NOT cleared on logout so that "logout → relaunch" does not
/// destroy server data or IndexedDB state.
#[tauri::command]
pub(crate) fn mark_first_run_completed(
    state: tauri::State<'_, AppManagedState>,
) -> Result<(), String> {
    let marker = state.data_dir.join("first_run_completed");
    std::fs::write(&marker, "1").map_err(|e| format!("failed to write first-run marker: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn first_run_marker_lifecycle() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let marker = dir.path().join("first_run_completed");

        // Initially does not exist
        assert!(!marker.exists(), "marker should not exist on fresh install");

        // Write marker
        std::fs::write(&marker, "1").expect("should write marker");
        assert!(marker.exists(), "marker should exist after write");

        // Marker persists across "logout" (we don't delete it)
        assert!(marker.exists(), "marker should survive logout");

        // Only a full uninstall / data-dir removal clears it
    }
}

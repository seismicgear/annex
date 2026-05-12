//! Miscellaneous Tauri commands that don't fit into a more focused module.
//!
//! Currently only `export_identity_json`, which presents an OS save dialog
//! and writes a JSON payload chosen by the frontend. Kept separate from the
//! identity-storage logic because it touches `rfd` (a heavy GUI dep) and
//! has no other callers.

/// Open a save-file dialog and write the provided JSON to the selected path.
///
/// Returns `Ok(Some(path))` when the file is saved, `Ok(None)` when the user
/// cancels the dialog, and `Err(...)` for I/O failures.
#[tauri::command]
pub(crate) fn export_identity_json(json: String) -> Result<Option<String>, String> {
    let file_path = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .set_file_name("annex-identity-backup.json")
        .save_file();

    let Some(path) = file_path else {
        return Ok(None);
    };

    std::fs::write(&path, json)
        .map_err(|e| format!("failed to write export file {}: {e}", path.display()))?;

    Ok(Some(path.display().to_string()))
}

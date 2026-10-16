//! Read the running host's identity boundary. This never grants privileges.

use crate::app_state::AppManagedState;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub(crate) struct HostIdentityContext {
    server_slug: String,
    pub(crate) founder_pseudonym_id: Option<String>,
}

pub(crate) fn read_host_identity(path: &Path) -> Result<HostIdentityContext, String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("Could not read the hosted server identity: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let (server_id, server_slug): (i64, String) = conn
        .query_row("SELECT id, slug FROM servers LIMIT 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("The hosted server has no readable identity: {e}"))?;
    let founder_pseudonym_id = conn
        .query_row(
            "SELECT pseudonym_id FROM platform_identities WHERE server_id=?1 ORDER BY id LIMIT 1",
            [server_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Could not identify the hosted server's founder: {e}"))?;
    Ok(HostIdentityContext {
        server_slug,
        founder_pseudonym_id,
    })
}

#[tauri::command]
pub(crate) async fn get_host_identity_context(
    state: tauri::State<'_, AppManagedState>,
) -> Result<HostIdentityContext, String> {
    // Use the path captured when this listener started, never a caller path or
    // a subsequently edited config. Only public identifiers leave this command.
    let path = state
        .server
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .ok_or("Start the hosted server before selecting its identity.")?
        .database_path
        .clone();
    tauri::async_runtime::spawn_blocking(move || read_host_identity(&path))
        .await
        .map_err(|e| format!("Could not inspect the hosted server identity: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn founder_is_bound_to_the_running_database_and_is_never_reassigned() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("annex.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE servers(id INTEGER PRIMARY KEY, slug TEXT);
            CREATE TABLE platform_identities(id INTEGER PRIMARY KEY, server_id INTEGER, pseudonym_id TEXT);
            INSERT INTO servers VALUES(1,'host-one');").unwrap();
        assert!(read_host_identity(&path)
            .unwrap()
            .founder_pseudonym_id
            .is_none());
        conn.execute_batch(
            "INSERT INTO platform_identities VALUES(1,2,'other-server');
            INSERT INTO platform_identities VALUES(2,1,'original-founder');
            INSERT INTO platform_identities VALUES(3,1,'newest-member');",
        )
        .unwrap();
        let context = read_host_identity(&path).unwrap();
        assert_eq!(context.server_slug, "host-one");
        assert_eq!(
            context.founder_pseudonym_id.as_deref(),
            Some("original-founder")
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM platform_identities", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }

    #[test]
    fn missing_database_is_an_error_and_is_not_created() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.db");
        assert!(read_host_identity(&path).is_err());
        assert!(!path.exists());
    }
}

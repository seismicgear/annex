//! Profile reads and explicit grants share one authorization snapshot.
use crate::AppState;
use annex_graph::{
    get_graph_node, get_visible_profile, list_metadata_grantees, set_metadata_grant, GraphError,
    GraphProfile,
};
use annex_identity::PlatformIdentity;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum GraphServiceError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Internal(String),
}
impl From<rusqlite::Error> for GraphServiceError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Internal(error.to_string())
    }
}
impl From<GraphError> for GraphServiceError {
    fn from(error: GraphError) -> Self {
        match error {
            GraphError::NodeNotFound(_) => Self::NotFound(error.to_string()),
            GraphError::InvalidMetadataGrant(_) => Self::Invalid(error.to_string()),
            _ => Self::Internal(error.to_string()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct MetadataGrants {
    pub owner_pseudonym: String,
    pub metadata_json: Option<String>,
    pub grantees: Vec<String>,
}

fn require_current(
    conn: &Connection,
    server: i64,
    identity: &PlatformIdentity,
) -> Result<(), GraphServiceError> {
    let epoch: Option<i64> = conn.query_row(
        "SELECT token_epoch FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2 AND active=1",
        params![server, identity.pseudonym_id], |row| row.get(0),
    ).optional()?;
    if epoch != Some(identity.token_epoch) {
        return Err(GraphServiceError::Unauthorized(
            "session is inactive or revoked".into(),
        ));
    }
    Ok(())
}

async fn transaction<T, F>(
    state: Arc<AppState>,
    identity: PlatformIdentity,
    operation: F,
) -> Result<T, GraphServiceError>
where
    T: Send + 'static,
    F: FnOnce(&Connection, i64, &str) -> Result<T, GraphServiceError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut conn = state
            .pool
            .get()
            .map_err(|e| GraphServiceError::Internal(e.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_current(&tx, state.server_id, &identity)?;
        let result = operation(&tx, state.server_id, &identity.pseudonym_id)?;
        tx.commit()?;
        Ok(result)
    })
    .await
    .map_err(|e| GraphServiceError::Internal(e.to_string()))?
}

pub async fn profile(
    state: Arc<AppState>,
    identity: PlatformIdentity,
    target: String,
) -> Result<GraphProfile, GraphServiceError> {
    transaction(state, identity, move |conn, server, viewer| {
        Ok(get_visible_profile(conn, server, viewer, &target)?)
    })
    .await
}

pub async fn grants(
    state: Arc<AppState>,
    identity: PlatformIdentity,
) -> Result<MetadataGrants, GraphServiceError> {
    transaction(state, identity, |conn, server, owner| {
        let node = get_graph_node(conn, server, owner)?
            .ok_or_else(|| GraphServiceError::NotFound("profile not found".into()))?;
        Ok(MetadataGrants {
            owner_pseudonym: owner.into(),
            metadata_json: node.metadata_json,
            grantees: list_metadata_grantees(conn, server, owner)?,
        })
    })
    .await
}

pub async fn set_grant(
    state: Arc<AppState>,
    identity: PlatformIdentity,
    grantee: String,
    granted: bool,
) -> Result<(), GraphServiceError> {
    transaction(state, identity, move |conn, server, owner| {
        Ok(set_metadata_grant(conn, server, owner, &grantee, granted)?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_context_and_inactive_identity_cannot_read_or_mutate_grants() {
        let conn = Connection::open_in_memory().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'one','One','{}')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,active) VALUES(1,'owner','HUMAN',1)", []).unwrap();
        let identity = annex_identity::get_platform_identity(&conn, 1, "owner").unwrap();
        require_current(&conn, 1, &identity).unwrap();
        conn.execute(
            "UPDATE platform_identities SET token_epoch=token_epoch+1 WHERE pseudonym_id='owner'",
            [],
        )
        .unwrap();
        assert!(matches!(
            require_current(&conn, 1, &identity),
            Err(GraphServiceError::Unauthorized(_))
        ));
        let fresh = annex_identity::get_platform_identity(&conn, 1, "owner").unwrap();
        require_current(&conn, 1, &fresh).unwrap();
        conn.execute(
            "UPDATE platform_identities SET active=0 WHERE pseudonym_id='owner'",
            [],
        )
        .unwrap();
        assert!(matches!(
            require_current(&conn, 1, &fresh),
            Err(GraphServiceError::Unauthorized(_))
        ));
        assert!(matches!(
            require_current(&conn, 2, &fresh),
            Err(GraphServiceError::Unauthorized(_))
        ));
    }
}

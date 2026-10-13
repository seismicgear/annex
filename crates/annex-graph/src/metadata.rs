//! Explicit, local profile metadata sharing. Never changes graph topology.
use crate::GraphError;
use rusqlite::{params, Connection};

fn active_local(conn: &Connection, server: i64, pseudonym: &str) -> Result<bool, GraphError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM platform_identities p JOIN graph_nodes n
         ON n.server_id=p.server_id AND n.pseudonym_id=p.pseudonym_id
         WHERE p.server_id=?1 AND p.pseudonym_id=?2 AND p.active=1 AND n.active=1)",
        params![server, pseudonym],
        |row| row.get(0),
    )?)
}

/// The service must authenticate the owner inside the same transaction. All
/// grantees are retained in this list, including inactive ones, so the owner
/// can revoke a dormant grant before a recipient becomes active again.
pub fn list_metadata_grantees(
    conn: &Connection,
    server: i64,
    owner: &str,
) -> Result<Vec<String>, GraphError> {
    let mut statement = conn.prepare(
        "SELECT grantee_pseudonym FROM graph_metadata_grants
        WHERE server_id=?1 AND owner_pseudonym=?2 ORDER BY grantee_pseudonym",
    )?;
    let rows = statement
        .query_map(params![server, owner], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Grant creation requires two current local participants. Revocation is
/// idempotent and never depends on the recipient still being active/present.
pub fn set_metadata_grant(
    conn: &Connection,
    server: i64,
    owner: &str,
    grantee: &str,
    granted: bool,
) -> Result<(), GraphError> {
    if owner == grantee || grantee.is_empty() || grantee.len() > 256 {
        return Err(GraphError::InvalidMetadataGrant(
            "select another valid local participant".into(),
        ));
    }
    if !active_local(conn, server, owner)? {
        return Err(GraphError::NodeNotFound(owner.into()));
    }
    if granted {
        if !active_local(conn, server, grantee)? {
            return Err(GraphError::NodeNotFound(grantee.into()));
        }
        conn.execute(
            "INSERT INTO graph_metadata_grants(server_id,owner_pseudonym,grantee_pseudonym)
            VALUES(?1,?2,?3) ON CONFLICT(server_id,owner_pseudonym,grantee_pseudonym) DO NOTHING",
            params![server, owner, grantee],
        )?;
    } else {
        conn.execute("DELETE FROM graph_metadata_grants WHERE server_id=?1 AND owner_pseudonym=?2 AND grantee_pseudonym=?3",
            params![server, owner, grantee])?;
    }
    Ok(())
}

pub(super) fn can_read_metadata(
    conn: &Connection,
    server: i64,
    owner: &str,
    viewer: &str,
) -> Result<bool, GraphError> {
    if !active_local(conn, server, owner)? || !active_local(conn, server, viewer)? {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM graph_metadata_grants
        WHERE server_id=?1 AND owner_pseudonym=?2 AND grantee_pseudonym=?3)",
        params![server, owner, viewer],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_edge, ensure_graph_node, get_visible_profile};
    use annex_types::{EdgeKind, NodeType, VisibilityLevel};

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        annex_db::run_migrations(&conn).unwrap();
        for server in [1, 2] {
            conn.execute(
                "INSERT INTO servers(id,slug,label,policy_json) VALUES(?1,?2,'Test','{}')",
                params![server, format!("server-{server}")],
            )
            .unwrap();
            for name in ["owner", "direct", "second", "third", "outside"] {
                conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,active) VALUES(?1,?2,'HUMAN',1)", params![server, name]).unwrap();
                ensure_graph_node(
                    &conn,
                    server,
                    name,
                    NodeType::Human,
                    Some("{\"private\":1}".into()),
                )
                .unwrap();
            }
            for (from, to) in [
                ("owner", "direct"),
                ("direct", "second"),
                ("second", "third"),
            ] {
                create_edge(&conn, server, from, to, EdgeKind::Connected, 1.0).unwrap();
            }
        }
        conn
    }

    #[test]
    fn grant_shares_only_metadata_without_widening_graph_visibility() {
        let conn = fixture();
        for (viewer, level) in [
            ("direct", VisibilityLevel::Degree1),
            ("second", VisibilityLevel::Degree2),
            ("third", VisibilityLevel::Degree3),
            ("outside", VisibilityLevel::None),
        ] {
            let before = get_visible_profile(&conn, 1, viewer, "owner").unwrap();
            assert_eq!(before.visibility, level);
            assert_eq!(before.metadata_json, None);
            set_metadata_grant(&conn, 1, "owner", viewer, true).unwrap();
            let shared = get_visible_profile(&conn, 1, viewer, "owner").unwrap();
            assert_eq!(shared.visibility, before.visibility);
            assert_eq!(shared.last_seen_at, before.last_seen_at);
            assert_eq!(shared.metadata_json.is_some(), viewer != "outside");
            assert_eq!(
                get_visible_profile(&conn, 2, viewer, "owner")
                    .unwrap()
                    .metadata_json,
                None
            );
        }
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM graph_edges WHERE server_id=1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            3
        );
    }

    #[test]
    fn grant_is_directional_persistent_future_facing_and_revocable() {
        let conn = fixture();
        set_metadata_grant(&conn, 1, "owner", "direct", true).unwrap();
        set_metadata_grant(&conn, 1, "owner", "direct", true).unwrap();
        assert_eq!(
            list_metadata_grantees(&conn, 1, "owner").unwrap(),
            vec!["direct"]
        );
        assert_eq!(
            get_visible_profile(&conn, 1, "owner", "direct")
                .unwrap()
                .metadata_json,
            None
        );
        assert_eq!(
            get_visible_profile(&conn, 1, "second", "owner")
                .unwrap()
                .metadata_json,
            None
        );
        conn.execute(
            "UPDATE graph_nodes SET metadata_json=?1 WHERE server_id=1 AND pseudonym_id='owner'",
            ["{\"private\":2}"],
        )
        .unwrap();
        assert_eq!(
            get_visible_profile(&conn, 1, "direct", "owner")
                .unwrap()
                .metadata_json
                .as_deref(),
            Some("{\"private\":2}")
        );
        set_metadata_grant(&conn, 1, "owner", "direct", false).unwrap();
        set_metadata_grant(&conn, 1, "owner", "direct", false).unwrap();
        assert_eq!(
            get_visible_profile(&conn, 1, "direct", "owner")
                .unwrap()
                .metadata_json,
            None
        );
        assert!(list_metadata_grantees(&conn, 1, "owner")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn inactive_participants_cannot_receive_metadata_and_dormant_grants_can_be_revoked() {
        let conn = fixture();
        set_metadata_grant(&conn, 1, "owner", "direct", true).unwrap();
        for table in ["platform_identities", "graph_nodes"] {
            for name in ["owner", "direct"] {
                conn.execute(
                    &format!("UPDATE {table} SET active=0 WHERE server_id=1 AND pseudonym_id=?1"),
                    [name],
                )
                .unwrap();
                assert_eq!(
                    get_visible_profile(&conn, 1, "direct", "owner")
                        .unwrap()
                        .metadata_json,
                    None
                );
                conn.execute(
                    &format!("UPDATE {table} SET active=1 WHERE server_id=1 AND pseudonym_id=?1"),
                    [name],
                )
                .unwrap();
            }
        }
        conn.execute(
            "UPDATE platform_identities SET active=0 WHERE server_id=1 AND pseudonym_id='direct'",
            [],
        )
        .unwrap();
        assert_eq!(
            list_metadata_grantees(&conn, 1, "owner").unwrap(),
            vec!["direct"]
        );
        set_metadata_grant(&conn, 1, "owner", "direct", false).unwrap();
        assert!(list_metadata_grantees(&conn, 1, "owner")
            .unwrap()
            .is_empty());
        assert!(matches!(
            set_metadata_grant(&conn, 1, "owner", "direct", true),
            Err(GraphError::NodeNotFound(_))
        ));
    }

    #[test]
    fn invalid_recipient_rejected_and_account_deletion_cascades() {
        let conn = fixture();
        for name in ["owner", ""] {
            assert!(matches!(
                set_metadata_grant(&conn, 1, "owner", name, true),
                Err(GraphError::InvalidMetadataGrant(_))
            ));
        }
        assert!(matches!(
            set_metadata_grant(&conn, 1, "owner", "missing", true),
            Err(GraphError::NodeNotFound(_))
        ));
        set_metadata_grant(&conn, 1, "owner", "missing", false).unwrap();
        set_metadata_grant(&conn, 1, "owner", "direct", true).unwrap();
        conn.execute(
            "DELETE FROM platform_identities WHERE server_id=1 AND pseudonym_id='direct'",
            [],
        )
        .unwrap();
        assert!(list_metadata_grantees(&conn, 1, "owner")
            .unwrap()
            .is_empty());
        set_metadata_grant(&conn, 1, "owner", "second", true).unwrap();
        conn.execute(
            "DELETE FROM platform_identities WHERE server_id=1 AND pseudonym_id='owner'",
            [],
        )
        .unwrap();
        assert!(list_metadata_grantees(&conn, 1, "owner")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn actual_membership_enables_degree_two_sharing_and_leave_removes_visibility() {
        let conn = fixture();
        conn.execute("DELETE FROM graph_edges", []).unwrap();
        conn.execute("INSERT INTO channels(server_id,channel_id,name,channel_type) VALUES(1,'ordinary','Ordinary','TEXT'),(1,'outside','Looks like a person','TEXT')", []).unwrap();
        set_metadata_grant(&conn, 1, "owner", "direct", true).unwrap();
        assert_eq!(
            get_visible_profile(&conn, 1, "direct", "owner")
                .unwrap()
                .visibility,
            VisibilityLevel::None
        );
        for person in ["owner", "direct"] {
            conn.execute("INSERT INTO channel_members(server_id,channel_id,pseudonym_id) VALUES(1,'ordinary',?1)",[person]).unwrap();
        }
        let shared = get_visible_profile(&conn, 1, "direct", "owner").unwrap();
        assert_eq!(shared.visibility, VisibilityLevel::Degree2);
        assert_eq!(shared.metadata_json.as_deref(), Some("{\"private\":1}"));
        assert_eq!(shared.last_seen_at, None);
        conn.execute("INSERT INTO channel_members(server_id,channel_id,pseudonym_id) VALUES(1,'outside','owner')",[]).unwrap();
        // A channel named for someone cannot create a direct person relationship.
        set_metadata_grant(&conn, 1, "owner", "outside", true).unwrap();
        assert_eq!(
            get_visible_profile(&conn, 1, "outside", "owner")
                .unwrap()
                .visibility,
            VisibilityLevel::None
        );
        assert_eq!(
            get_visible_profile(&conn, 1, "outside", "owner")
                .unwrap()
                .metadata_json,
            None
        );
        conn.execute("DELETE FROM channel_members WHERE server_id=1 AND channel_id='ordinary' AND pseudonym_id='direct'",[]).unwrap();
        let departed = get_visible_profile(&conn, 1, "direct", "owner").unwrap();
        assert_eq!(departed.visibility, VisibilityLevel::None);
        assert_eq!(departed.metadata_json, None);
        // Grants do not grant channel access or recreate the removed relationship.
        assert!(list_metadata_grantees(&conn, 1, "owner")
            .unwrap()
            .contains(&"direct".into()));
    }
}

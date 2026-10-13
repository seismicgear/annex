//! Exercise real legacy upgrades and transaction boundaries, not a duplicate schema.
use super::{run_migrations_from_list, MIGRATIONS};
use rusqlite::{params, Connection};

fn legacy() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    let boundary = MIGRATIONS
        .iter()
        .position(|m| m.name == "063_graph_channel_membership")
        .unwrap();
    run_migrations_from_list(&conn, &MIGRATIONS[..boundary]).unwrap();
    for server in [1, 2] {
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(?1,?2,'Membership test','{}')",
            params![server, format!("server-{server}")],
        )
        .unwrap();
        for person in ["alice", "bob", "agent"] {
            conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type) VALUES(?1,?2,'HUMAN')", params![server, person]).unwrap();
        }
    }
    for (server, channel) in [
        (1, "room"),
        (1, "bob"),
        (1, "annex:channel:1:room"),
        (2, "other"),
    ] {
        conn.execute(
            "INSERT INTO channels(server_id,channel_id,name,channel_type) VALUES(?1,?2,?2,'TEXT')",
            params![server, channel],
        )
        .unwrap();
    }
    conn
}

fn member(conn: &Connection, server: i64, channel: &str, person: &str) {
    conn.execute(
        "INSERT OR IGNORE INTO channel_members(server_id,channel_id,pseudonym_id) VALUES(?1,?2,?3)",
        params![server, channel, person],
    )
    .unwrap();
}

fn edges(conn: &Connection, server: i64, person: &str, target: &str, kind: &str) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM graph_edges WHERE server_id=?1 AND from_node=?2 AND to_node=?3 AND kind=?4", params![server, person, target, kind], |row| row.get(0)).unwrap()
}

#[test]
fn graph_membership_upgrade_preserves_legacy_agents_and_backfills_scoped_members() {
    let conn = legacy();
    member(&conn, 1, "room", "alice");
    member(&conn, 1, "room", "agent");
    // The old FK permits a mismatched server; it must not become a graph edge.
    member(&conn, 2, "room", "bob");
    conn.execute("INSERT INTO graph_edges(server_id,from_node,to_node,kind,weight,created_at) VALUES(1,'agent','room','AGENT_SERVING',0.5,'2020-01-01'),(1,'agent','annex:channel:1:room','AGENT_SERVING',0.75,'2021-01-01'),(1,'alice','bob','CONNECTED',1.0,'2022-01-01')", []).unwrap();
    run_migrations_from_list(&conn, MIGRATIONS).unwrap();
    assert_eq!(
        edges(&conn, 1, "alice", "annex:channel:1:room", "MEMBER_OF"),
        1
    );
    assert_eq!(
        edges(&conn, 1, "agent", "annex:channel:1:room", "MEMBER_OF"),
        1
    );
    assert_eq!(
        edges(&conn, 2, "bob", "annex:channel:2:room", "MEMBER_OF"),
        0
    );
    assert_eq!(edges(&conn, 1, "agent", "room", "AGENT_SERVING"), 0);
    // Even a raw channel ID equal to another encoded vertex is distinct.
    assert_eq!(
        edges(&conn, 1, "agent", "annex:channel:1:room", "AGENT_SERVING"),
        1
    );
    assert_eq!(
        edges(
            &conn,
            1,
            "agent",
            "annex:channel:1:annex:channel:1:room",
            "AGENT_SERVING"
        ),
        1
    );
    assert_eq!(edges(&conn, 1, "alice", "bob", "CONNECTED"), 1);
    let retained: (f64,String) = conn.query_row("SELECT weight,created_at FROM graph_edges WHERE server_id=1 AND from_node='agent' AND to_node='annex:channel:1:room' AND kind='AGENT_SERVING'",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(retained, (0.5, "2020-01-01".into()));
    assert_eq!(run_migrations_from_list(&conn, MIGRATIONS).unwrap(), 0);
}

#[test]
fn graph_membership_join_leave_and_rollback_are_atomic_and_idempotent() {
    let conn = legacy();
    run_migrations_from_list(&conn, MIGRATIONS).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    member(&conn, 1, "room", "alice");
    assert_eq!(
        edges(&conn, 1, "alice", "annex:channel:1:room", "MEMBER_OF"),
        1
    );
    conn.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        edges(&conn, 1, "alice", "annex:channel:1:room", "MEMBER_OF"),
        0
    );
    member(&conn, 1, "room", "alice");
    member(&conn, 1, "room", "alice");
    member(&conn, 1, "room", "agent");
    conn.execute("INSERT INTO graph_edges(server_id,from_node,to_node,kind) VALUES(1,'agent','annex:channel:1:room','AGENT_SERVING')",[]).unwrap();
    assert_eq!(
        edges(&conn, 1, "alice", "annex:channel:1:room", "MEMBER_OF"),
        1
    );
    conn.execute_batch(
        "BEGIN IMMEDIATE; DELETE FROM channel_members WHERE server_id=1 AND pseudonym_id='agent'",
    )
    .unwrap();
    for kind in ["MEMBER_OF", "AGENT_SERVING"] {
        assert_eq!(edges(&conn, 1, "agent", "annex:channel:1:room", kind), 0);
    }
    conn.execute_batch("ROLLBACK").unwrap();
    for kind in ["MEMBER_OF", "AGENT_SERVING"] {
        assert_eq!(edges(&conn, 1, "agent", "annex:channel:1:room", kind), 1);
    }
    conn.execute(
        "DELETE FROM channel_members WHERE server_id=1 AND pseudonym_id='agent'",
        [],
    )
    .unwrap();
    for kind in ["MEMBER_OF", "AGENT_SERVING"] {
        assert_eq!(edges(&conn, 1, "agent", "annex:channel:1:room", kind), 0);
    }
    assert_eq!(
        edges(&conn, 1, "alice", "annex:channel:1:room", "MEMBER_OF"),
        1
    );
}

#[test]
fn graph_membership_update_moves_only_the_explicit_membership() {
    let conn = legacy();
    run_migrations_from_list(&conn, MIGRATIONS).unwrap();
    member(&conn, 1, "room", "alice");
    conn.execute("UPDATE channel_members SET server_id=2,channel_id='other',pseudonym_id='bob' WHERE server_id=1 AND channel_id='room' AND pseudonym_id='alice'",[]).unwrap();
    assert_eq!(
        edges(&conn, 1, "alice", "annex:channel:1:room", "MEMBER_OF"),
        0
    );
    assert_eq!(
        edges(&conn, 2, "bob", "annex:channel:2:other", "MEMBER_OF"),
        1
    );
    conn.execute(
        "UPDATE channel_members SET role='MODERATOR' WHERE server_id=2 AND channel_id='other'",
        [],
    )
    .unwrap();
    assert_eq!(
        edges(&conn, 2, "bob", "annex:channel:2:other", "MEMBER_OF"),
        1
    );
    // This old-schema inconsistency cannot cross the graph's server boundary.
    conn.execute(
        "UPDATE channel_members SET server_id=1 WHERE channel_id='other'",
        [],
    )
    .unwrap();
    assert_eq!(
        edges(&conn, 2, "bob", "annex:channel:2:other", "MEMBER_OF"),
        0
    );
    assert_eq!(
        edges(&conn, 1, "bob", "annex:channel:1:other", "MEMBER_OF"),
        0
    );
}

#[test]
fn graph_membership_channel_delete_is_namespaced_and_rollback_safe() {
    let conn = legacy();
    run_migrations_from_list(&conn, MIGRATIONS).unwrap();
    member(&conn, 1, "room", "alice");
    member(&conn, 1, "annex:channel:1:room", "alice");
    conn.execute("INSERT INTO graph_edges(server_id,from_node,to_node,kind) VALUES(1,'agent','annex:channel:1:annex:channel:1:room','AGENT_SERVING'),(1,'alice','bob','CONNECTED')",[]).unwrap();
    // A FK failure must roll back the BEFORE DELETE graph cleanup too.
    assert!(conn
        .execute(
            "DELETE FROM channels WHERE channel_id='annex:channel:1:room'",
            []
        )
        .is_err());
    assert_eq!(
        edges(
            &conn,
            1,
            "agent",
            "annex:channel:1:annex:channel:1:room",
            "AGENT_SERVING"
        ),
        1
    );
    conn.execute_batch("BEGIN IMMEDIATE; DELETE FROM channel_members WHERE channel_id='annex:channel:1:room'; DELETE FROM channels WHERE channel_id='annex:channel:1:room'; COMMIT").unwrap();
    assert_eq!(
        edges(
            &conn,
            1,
            "agent",
            "annex:channel:1:annex:channel:1:room",
            "AGENT_SERVING"
        ),
        0
    );
    assert_eq!(
        edges(&conn, 1, "alice", "annex:channel:1:room", "MEMBER_OF"),
        1
    );
    assert_eq!(edges(&conn, 1, "alice", "bob", "CONNECTED"), 1);
}

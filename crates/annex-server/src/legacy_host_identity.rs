//! Recover the identity namespace changed by the old public-URL setter.
//!
//! That setter replaced `servers.slug` with a hash of the network address,
//! while the persistent configuration and admitted founder retained the
//! original topic. Recovery is limited to that exact legacy shape and requires
//! a stored nullifier that derives the existing founder's pseudonym. It never
//! changes a member, permission, key, address, or configuration.

use annex_identity::derive_pseudonym_id;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

pub(crate) fn restore_namespace(
    conn: &mut Connection,
    server_id: i64,
    configured_slug: &str,
) -> rusqlite::Result<bool> {
    if configured_slug.is_empty() {
        return Ok(false);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (current_slug, public_url): (String, String) = tx.query_row(
        "SELECT slug,public_url FROM servers WHERE id=?1",
        [server_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if current_slug == configured_slug
        || public_url.trim().is_empty()
        || current_slug != crate::config::derive_server_slug_from_public_url(&public_url)
    {
        return Ok(false);
    }
    let founder: Option<String> = tx
        .query_row(
            "SELECT pseudonym_id FROM platform_identities WHERE server_id=?1 ORDER BY id LIMIT 1",
            [server_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(founder) = founder else {
        return Ok(false);
    };

    let mut proven = false;
    for version in ["v2", "v1"] {
        let topic = format!("annex:server:{configured_slug}:{version}");
        let mut statement = tx.prepare("SELECT nullifier_hex FROM zk_nullifiers WHERE topic=?1")?;
        let nullifiers = statement.query_map([&topic], |row| row.get::<_, String>(0))?;
        for nullifier in nullifiers {
            if matches!(derive_pseudonym_id(&topic, &nullifier?), Ok(pseudonym) if pseudonym == founder)
            {
                proven = true;
                break;
            }
        }
        if proven {
            break;
        }
    }
    if !proven {
        return Ok(false);
    }

    tx.execute(
        "UPDATE servers SET slug=?1 WHERE id=?2",
        rusqlite::params![configured_slug, server_id],
    )?;
    tx.commit()?;
    tracing::warn!(server_id, old_slug = %current_slug, restored_slug = configured_slug,
        "restored original founder identity namespace changed by legacy public URL update");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = "original-host";
    const URL: &str = "https://legacy-host.example.com";

    fn fixture(version: &str) -> (Connection, String) {
        let conn = Connection::open_in_memory().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        let moved = crate::config::derive_server_slug_from_public_url(URL);
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json,public_url) VALUES(1,?1,'Host','{}',?2)",
            rusqlite::params![moved, URL],
        )
        .unwrap();
        let topic = format!("annex:server:{ORIGINAL}:{version}");
        let nullifier = "12".repeat(32);
        let founder = derive_pseudonym_id(&topic, &nullifier).unwrap();
        conn.execute(
            "INSERT INTO zk_nullifiers(topic,nullifier_hex) VALUES(?1,?2)",
            rusqlite::params![topic, nullifier],
        )
        .unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_moderate,can_invite) VALUES(1,?1,'HUMAN',1,1)", [&founder]).unwrap();
        (conn, founder)
    }

    fn slug(conn: &Connection) -> String {
        conn.query_row("SELECT slug FROM servers WHERE id=1", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn restores_legacy_url_rename_without_replacing_founder_or_address() {
        for version in ["v1", "v2"] {
            let (mut conn, founder) = fixture(version);
            assert!(restore_namespace(&mut conn, 1, ORIGINAL).unwrap());
            assert_eq!(slug(&conn), ORIGINAL);
            let member: (String, i64, i64) = conn.query_row("SELECT pseudonym_id,can_moderate,can_invite FROM platform_identities WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
            assert_eq!(member, (founder, 1, 1));
            assert_eq!(
                conn.query_row("SELECT public_url FROM servers WHERE id=1", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                URL
            );
            assert!(!restore_namespace(&mut conn, 1, ORIGINAL).unwrap());
        }
    }

    #[test]
    fn configuration_alone_cannot_change_existing_server_identity() {
        let (mut conn, _) = fixture("v2");
        let before = slug(&conn);
        assert!(!restore_namespace(&mut conn, 1, "different-server").unwrap());
        assert_eq!(slug(&conn), before);
        conn.execute("DELETE FROM zk_nullifiers", []).unwrap();
        assert!(!restore_namespace(&mut conn, 1, ORIGINAL).unwrap());
        assert_eq!(slug(&conn), before);
    }

    #[test]
    fn later_member_proof_cannot_replace_the_founders_namespace() {
        let (mut conn, _) = fixture("v2");
        // The matching recorded proof now belongs to a later moderator.
        conn.execute("UPDATE platform_identities SET id=2 WHERE id=1", [])
            .unwrap();
        conn.execute("INSERT INTO platform_identities(id,server_id,pseudonym_id,participant_type,can_moderate) VALUES(1,1,'earlier-founder','HUMAN',1)", []).unwrap();
        let before = slug(&conn);
        assert!(!restore_namespace(&mut conn, 1, ORIGINAL).unwrap());
        assert_eq!(slug(&conn), before);
    }

    #[test]
    fn unrelated_slug_changes_are_not_assumed_to_be_the_legacy_url_bug() {
        let (mut conn, _) = fixture("v2");
        conn.execute("UPDATE servers SET slug='explicit-identity' WHERE id=1", [])
            .unwrap();
        assert!(!restore_namespace(&mut conn, 1, ORIGINAL).unwrap());
        assert_eq!(slug(&conn), "explicit-identity");
    }

    #[test]
    fn a_conflicting_server_identity_is_not_overwritten() {
        let (mut conn, _) = fixture("v2");
        let before = slug(&conn);
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(2,?1,'Other','{}')",
            [ORIGINAL],
        )
        .unwrap();
        assert!(restore_namespace(&mut conn, 1, ORIGINAL).is_err());
        assert_eq!(slug(&conn), before);
    }
}

-- Ordered federation channel authority. Unlike channel-owned key tables, the
-- signed ledger and removals survive channel deletion for replica cleanup.
CREATE TABLE channel_authorities (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL,
 authority_identity TEXT NOT NULL, authority_key_hex TEXT NOT NULL,
 authority_url TEXT NOT NULL, authority_peer_id INTEGER,
 local_authority INTEGER NOT NULL CHECK(local_authority IN(0,1)),
 head_seq INTEGER NOT NULL DEFAULT 0 CHECK(head_seq>=0),
 head_hash TEXT NOT NULL DEFAULT '', deleted_seq INTEGER,
 PRIMARY KEY(server_id,channel_id),
 FOREIGN KEY(server_id) REFERENCES servers(id),
 FOREIGN KEY(authority_peer_id) REFERENCES instances(id)
);
CREATE TABLE channel_authority_events (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL, event_seq INTEGER NOT NULL,
 message_id TEXT,event_kind TEXT NOT NULL, event_hash TEXT NOT NULL, payload_hash TEXT NOT NULL,
 header_json TEXT NOT NULL, signature TEXT NOT NULL, payload_json TEXT,
 removed_reason TEXT, removal_event_seq INTEGER,
 created_at TEXT NOT NULL, PRIMARY KEY(server_id,channel_id,event_seq),
 UNIQUE(server_id,channel_id,event_hash),
 FOREIGN KEY(server_id,channel_id) REFERENCES channel_authorities(server_id,channel_id),
 CHECK((payload_json IS NULL)=(removed_reason IS NOT NULL))
);
CREATE TABLE channel_authority_operations (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL,
 requesting_identity TEXT NOT NULL, pseudonym_id TEXT NOT NULL, operation_id TEXT NOT NULL,
 operation_hash TEXT NOT NULL, accepted_seq INTEGER NOT NULL,
 PRIMARY KEY(server_id,channel_id,requesting_identity,pseudonym_id,operation_id),
 FOREIGN KEY(server_id,channel_id,accepted_seq) REFERENCES channel_authority_events(server_id,channel_id,event_seq)
);
CREATE TABLE channel_authority_replicas (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL, peer_instance_id INTEGER NOT NULL,
 admitted_at TEXT NOT NULL DEFAULT(datetime('now')), revoked_at TEXT,
 acknowledged_seq INTEGER NOT NULL DEFAULT 0, acknowledged_hash TEXT NOT NULL DEFAULT '',
 PRIMARY KEY(server_id,channel_id,peer_instance_id),
 FOREIGN KEY(server_id,channel_id) REFERENCES channel_authorities(server_id,channel_id),
 FOREIGN KEY(peer_instance_id) REFERENCES instances(id)
);
CREATE TABLE channel_authority_nonces (
 server_id INTEGER NOT NULL, requesting_identity TEXT NOT NULL, nonce TEXT NOT NULL,
 request_hash TEXT NOT NULL, expires_at INTEGER NOT NULL,
 PRIMARY KEY(server_id,requesting_identity,nonce), FOREIGN KEY(server_id) REFERENCES servers(id)
);
CREATE INDEX channel_authority_nonce_expiry ON channel_authority_nonces(expires_at);
CREATE TABLE channel_authority_attachment_bindings (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL, message_id TEXT NOT NULL,
 attachment_id TEXT NOT NULL, accepted_seq INTEGER NOT NULL, accepted_hash TEXT NOT NULL,
 descriptor_json TEXT NOT NULL, removed_reason TEXT, removal_seq INTEGER,
 PRIMARY KEY(server_id,channel_id,message_id,attachment_id), UNIQUE(server_id,attachment_id),
 FOREIGN KEY(server_id,channel_id) REFERENCES channel_authorities(server_id,channel_id)
);
-- SQL-only authority changes reserve durable ordering while still inside their
-- originating transaction. Payloads are immutable snapshots, never reconstructed
-- from potentially newer membership after a restart.
CREATE TABLE channel_authority_pending_state (
 pending_id INTEGER PRIMARY KEY AUTOINCREMENT,
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,revision INTEGER NOT NULL,
 state_json TEXT NOT NULL,members_json TEXT NOT NULL,devices_json TEXT NOT NULL,
 created_at TEXT NOT NULL DEFAULT(datetime('now')),
 FOREIGN KEY(server_id,channel_id) REFERENCES channel_authorities(server_id,channel_id)
);
CREATE TRIGGER authority_key_state_changed AFTER UPDATE ON channel_key_state
 WHEN EXISTS(SELECT 1 FROM channel_authorities a WHERE a.server_id=NEW.server_id AND a.channel_id=NEW.channel_id AND a.local_authority=1 AND a.deleted_seq IS NULL)
BEGIN
 INSERT INTO channel_authority_pending_state(server_id,channel_id,revision,state_json,members_json,devices_json)
 VALUES(NEW.server_id,NEW.channel_id,NEW.revision,
 json_object('revision',NEW.revision,'current_epoch',NEW.current_epoch,'required_epoch',NEW.required_epoch,'current_key_id',NEW.current_key_id,'history_policy',NEW.history_policy,'e2e_enabled',(SELECT e2e_enabled FROM channels WHERE channel_id=NEW.channel_id AND server_id=NEW.server_id)),
 (SELECT json_group_array(json_object('pseudonym_id',p.pseudonym_id,'active',p.active,'participant_type',p.participant_type,'can_moderate',p.can_moderate)) FROM channel_members m JOIN platform_identities p ON p.server_id=m.server_id AND p.pseudonym_id=m.pseudonym_id WHERE m.server_id=NEW.server_id AND m.channel_id=NEW.channel_id),
 (SELECT json_group_array(json_object('pseudonym_id',d.pseudonym_id,'device_id',d.device_id,'x25519_pub_hex',d.x25519_pub_hex,'revoked_at',d.revoked_at,'device_certificate',d.device_certificate)) FROM member_device_keys d WHERE d.server_id=NEW.server_id AND EXISTS(SELECT 1 FROM channel_members m WHERE m.server_id=d.server_id AND m.pseudonym_id=d.pseudonym_id AND m.channel_id=NEW.channel_id)));
END;
-- Channel destruction must be recorded before its child records disappear.
CREATE TABLE channel_authority_pending_deletions (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,created_at TEXT NOT NULL DEFAULT(datetime('now')),
 PRIMARY KEY(server_id,channel_id),FOREIGN KEY(server_id,channel_id) REFERENCES channel_authorities(server_id,channel_id)
);
CREATE TRIGGER authority_channel_deleting BEFORE DELETE ON channels
 WHEN EXISTS(SELECT 1 FROM channel_authorities a WHERE a.server_id=OLD.server_id AND a.channel_id=OLD.channel_id AND a.local_authority=1 AND a.deleted_seq IS NULL)
BEGIN
 INSERT OR IGNORE INTO channel_authority_pending_deletions(server_id,channel_id) VALUES(OLD.server_id,OLD.channel_id);
END;

CREATE INDEX authority_events_message ON channel_authority_events(server_id,channel_id,message_id);
CREATE TABLE channel_authority_pending_message_removals (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,message_id TEXT NOT NULL,reason TEXT NOT NULL,
 PRIMARY KEY(server_id,channel_id,message_id),
 FOREIGN KEY(server_id,channel_id) REFERENCES channel_authorities(server_id,channel_id)
);
CREATE TRIGGER authority_message_soft_deleted AFTER UPDATE OF deleted_at ON messages
 WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL AND EXISTS(SELECT 1 FROM channel_authorities a WHERE a.server_id=NEW.server_id AND a.channel_id=NEW.channel_id AND a.local_authority=1)
BEGIN
 INSERT OR IGNORE INTO channel_authority_pending_message_removals(server_id,channel_id,message_id,reason) VALUES(NEW.server_id,NEW.channel_id,NEW.message_id,'deleted');
 UPDATE channel_authority_events SET payload_json=NULL,removed_reason='deleted' WHERE server_id=NEW.server_id AND channel_id=NEW.channel_id AND message_id=NEW.message_id AND payload_json IS NOT NULL;
END;
CREATE TRIGGER authority_message_hard_deleted BEFORE DELETE ON messages
 WHEN EXISTS(SELECT 1 FROM channel_authorities a WHERE a.server_id=OLD.server_id AND a.channel_id=OLD.channel_id AND a.local_authority=1)
BEGIN
 INSERT OR IGNORE INTO channel_authority_pending_message_removals(server_id,channel_id,message_id,reason) VALUES(OLD.server_id,OLD.channel_id,OLD.message_id,'retention');
 UPDATE channel_authority_events SET payload_json=NULL,removed_reason='retention' WHERE server_id=OLD.server_id AND channel_id=OLD.channel_id AND message_id=OLD.message_id AND payload_json IS NOT NULL;
END;
CREATE TRIGGER authority_channel_payloads_deleted BEFORE DELETE ON channels
 WHEN EXISTS(SELECT 1 FROM channel_authorities a WHERE a.server_id=OLD.server_id AND a.channel_id=OLD.channel_id AND a.local_authority=1)
BEGIN
 UPDATE channel_authority_events SET payload_json=NULL,removed_reason='channel_deleted' WHERE server_id=OLD.server_id AND channel_id=OLD.channel_id AND payload_json IS NOT NULL;
 DELETE FROM channel_authority_pending_state WHERE server_id=OLD.server_id AND channel_id=OLD.channel_id;
END;

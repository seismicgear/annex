-- Immutable device directory and compare-and-swap channel encryption epochs.
CREATE TABLE member_device_keys (
 server_id INTEGER NOT NULL, pseudonym_id TEXT NOT NULL, device_id TEXT NOT NULL,
 x25519_pub_hex TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT(datetime('now')), revoked_at TEXT,
 device_certificate TEXT,
 PRIMARY KEY(server_id,pseudonym_id,device_id),
 FOREIGN KEY(server_id,pseudonym_id) REFERENCES platform_identities(server_id,pseudonym_id) ON DELETE CASCADE
);
CREATE TABLE channel_key_state (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL, authority_server TEXT NOT NULL DEFAULT '',
 revision INTEGER NOT NULL DEFAULT 0 CHECK(revision BETWEEN 0 AND 9007199254740991),
 current_epoch INTEGER NOT NULL DEFAULT 0 CHECK(current_epoch BETWEEN 0 AND 9007199254740991),
 required_epoch INTEGER NOT NULL DEFAULT 1 CHECK(required_epoch BETWEEN 1 AND 9007199254740991),
 current_key_id TEXT, history_policy TEXT NOT NULL DEFAULT 'all_members' CHECK(history_policy='all_members'),
 PRIMARY KEY(server_id,channel_id), FOREIGN KEY(channel_id) REFERENCES channels(channel_id) ON DELETE CASCADE,
 CHECK(required_epoch >= current_epoch)
);
INSERT INTO channel_key_state(server_id,channel_id,required_epoch)
 SELECT c.server_id,c.channel_id,1+COALESCE((SELECT MAX(key_epoch) FROM channel_key_wraps w
 WHERE w.server_id=c.server_id AND w.channel_id=c.channel_id),0) FROM channels c;
CREATE TABLE channel_key_epochs (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL, epoch INTEGER NOT NULL,
 key_id TEXT NOT NULL, committed_revision INTEGER NOT NULL,
 committed_by_pseudonym TEXT NOT NULL, committed_by_device TEXT NOT NULL,
 created_at TEXT NOT NULL DEFAULT(datetime('now')),
 PRIMARY KEY(server_id,channel_id,epoch), UNIQUE(server_id,channel_id,epoch,key_id),
 UNIQUE(server_id,channel_id,key_id),
 FOREIGN KEY(channel_id) REFERENCES channels(channel_id) ON DELETE CASCADE
);
CREATE TABLE channel_device_key_wraps (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL, recipient_pseudonym_id TEXT NOT NULL,
 recipient_device_id TEXT NOT NULL, epoch INTEGER NOT NULL, key_id TEXT NOT NULL,
 sender_pseudonym_id TEXT NOT NULL, sender_device_id TEXT NOT NULL, wrapped_key_b64 TEXT NOT NULL,
 created_at TEXT NOT NULL DEFAULT(datetime('now')),
 PRIMARY KEY(server_id,channel_id,recipient_pseudonym_id,recipient_device_id,epoch),
 FOREIGN KEY(server_id,channel_id,epoch,key_id) REFERENCES channel_key_epochs(server_id,channel_id,epoch,key_id) ON DELETE CASCADE,
 FOREIGN KEY(server_id,recipient_pseudonym_id,recipient_device_id) REFERENCES member_device_keys(server_id,pseudonym_id,device_id) ON DELETE CASCADE
);
-- Legacy ciphertext has no epoch marker. Preserve each authenticated candidate
-- independently; these records can never satisfy the current-epoch write gate.
CREATE TABLE channel_legacy_key_candidates (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL, epoch INTEGER NOT NULL, key_id TEXT NOT NULL,
 PRIMARY KEY(server_id,channel_id,epoch,key_id),
 FOREIGN KEY(channel_id) REFERENCES channels(channel_id) ON DELETE CASCADE
);
CREATE TABLE channel_legacy_device_wraps (
 server_id INTEGER NOT NULL, channel_id TEXT NOT NULL, epoch INTEGER NOT NULL, key_id TEXT NOT NULL,
 recipient_pseudonym_id TEXT NOT NULL, recipient_device_id TEXT NOT NULL,
 sender_pseudonym_id TEXT NOT NULL, sender_device_id TEXT NOT NULL, wrapped_key_b64 TEXT NOT NULL,
 PRIMARY KEY(server_id,channel_id,epoch,key_id,recipient_pseudonym_id,recipient_device_id),
 FOREIGN KEY(server_id,channel_id,epoch,key_id) REFERENCES channel_legacy_key_candidates(server_id,channel_id,epoch,key_id) ON DELETE CASCADE,
 FOREIGN KEY(server_id,recipient_pseudonym_id,recipient_device_id) REFERENCES member_device_keys(server_id,pseudonym_id,device_id) ON DELETE CASCADE
);
-- A transactionally persisted notification obligation covers every membership
-- mutation, including non-HTTP paths. Notifications only prompt fresh reads.
CREATE TABLE channel_key_notifications (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,revision INTEGER NOT NULL,
 PRIMARY KEY(server_id,channel_id), FOREIGN KEY(channel_id) REFERENCES channels(channel_id) ON DELETE CASCADE
);
CREATE TRIGGER e2e_channel_insert AFTER INSERT ON channels BEGIN
 INSERT INTO channel_key_state(server_id,channel_id) VALUES(NEW.server_id,NEW.channel_id);
END;
CREATE TRIGGER e2e_state_notification AFTER UPDATE ON channel_key_state BEGIN
 INSERT INTO channel_key_notifications(server_id,channel_id,revision) VALUES(NEW.server_id,NEW.channel_id,NEW.revision)
 ON CONFLICT(server_id,channel_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER e2e_mode_change AFTER UPDATE OF e2e_enabled ON channels
 WHEN OLD.e2e_enabled <> NEW.e2e_enabled BEGIN
 UPDATE channel_key_state SET revision=revision+1,
 required_epoch=CASE WHEN NEW.e2e_enabled=1 THEN MAX(required_epoch,current_epoch+1) ELSE required_epoch END
 WHERE server_id=NEW.server_id AND channel_id=NEW.channel_id;
END;
CREATE TRIGGER e2e_device_immutable BEFORE UPDATE OF device_id,x25519_pub_hex,pseudonym_id,server_id ON member_device_keys
 BEGIN SELECT RAISE(ABORT,'device keys are immutable'); END;
CREATE TRIGGER e2e_device_no_restore BEFORE UPDATE OF revoked_at ON member_device_keys
 WHEN OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS NULL
 BEGIN SELECT RAISE(ABORT,'revoked device cannot be restored'); END;
CREATE TRIGGER e2e_member_insert AFTER INSERT ON channel_members BEGIN
 UPDATE channel_key_state SET revision=revision+1
 WHERE server_id=NEW.server_id AND channel_id=NEW.channel_id;
END;
CREATE TRIGGER e2e_member_delete AFTER DELETE ON channel_members BEGIN
 UPDATE channel_key_state SET revision=revision+1,required_epoch=MAX(required_epoch,current_epoch+1)
 WHERE server_id=OLD.server_id AND channel_id=OLD.channel_id;
END;
CREATE TRIGGER e2e_identity_activity AFTER UPDATE OF active ON platform_identities
 WHEN OLD.active <> NEW.active BEGIN
 UPDATE channel_key_state SET revision=revision+1,
 required_epoch=CASE WHEN NEW.active=0 THEN MAX(required_epoch,current_epoch+1) ELSE required_epoch END
 WHERE server_id=NEW.server_id AND channel_id IN(SELECT channel_id FROM channel_members WHERE server_id=NEW.server_id AND pseudonym_id=NEW.pseudonym_id);
END;
CREATE TRIGGER e2e_device_insert AFTER INSERT ON member_device_keys  BEGIN
 UPDATE channel_key_state SET revision=revision+1
 WHERE server_id=NEW.server_id AND channel_id IN(SELECT channel_id FROM channel_members WHERE server_id=NEW.server_id AND pseudonym_id=NEW.pseudonym_id);
END;
CREATE TRIGGER e2e_device_revoke AFTER UPDATE OF revoked_at ON member_device_keys WHEN OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL BEGIN
 UPDATE channel_key_state SET revision=revision+1,required_epoch=MAX(required_epoch,current_epoch+1)
 WHERE server_id=NEW.server_id AND channel_id IN(SELECT channel_id FROM channel_members WHERE server_id=NEW.server_id AND pseudonym_id=NEW.pseudonym_id);
END;
CREATE TRIGGER e2e_device_delete AFTER DELETE ON member_device_keys  BEGIN
 UPDATE channel_key_state SET revision=revision+1 ,required_epoch=MAX(required_epoch,current_epoch+1)
 WHERE server_id=OLD.server_id AND channel_id IN(SELECT channel_id FROM channel_members WHERE server_id=OLD.server_id AND pseudonym_id=OLD.pseudonym_id);
END;

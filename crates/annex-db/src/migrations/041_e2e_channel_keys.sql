-- End-to-end encrypted channels: content-blind key distribution.
--
-- The server stores ONLY public key material and OPAQUE wrapped blobs. It never
-- sees a device secret, a channel content key (CEK), or plaintext message
-- bodies for E2E channels. Humans and AI agents are both ordinary members: each
-- advertises an X25519 public key, and the CEK is sealed to every member's key
-- (see crates/annex-federation/src/seal.rs :: seal_x25519 and
-- client/src/lib/e2e.ts). This keeps agents working — they receive the CEK
-- wrapped to them just like a human client — while the server stays blind.

-- Opt-in flag. Default 0 keeps every existing channel exactly as it was.
ALTER TABLE channels ADD COLUMN e2e_enabled INTEGER NOT NULL DEFAULT 0;

-- Per-member public key directory. The advertised X25519 public key other
-- members seal the channel key to. Keyed by pseudonym, scoped per server.
CREATE TABLE member_keys (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  server_id INTEGER NOT NULL,
  pseudonym_id TEXT NOT NULL,
  x25519_pub_hex TEXT NOT NULL,        -- 64 lowercase hex chars (32 bytes)
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE (server_id, pseudonym_id),
  FOREIGN KEY (server_id) REFERENCES servers(id)
);

-- Sealed channel content keys, one row per (channel, recipient, epoch). The
-- wrapped blob is the output of seal_x25519 — ciphertext the server cannot
-- open. `key_epoch` lets a channel rotate its CEK (e.g. on membership change)
-- without clobbering older wraps. The first wrap for a (channel,recipient,epoch)
-- wins (INSERT OR IGNORE in the handler), so a member cannot clobber another
-- member's key material; rotation issues a fresh epoch instead.
CREATE TABLE channel_key_wraps (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  server_id INTEGER NOT NULL,
  channel_id TEXT NOT NULL,
  recipient_pseudonym_id TEXT NOT NULL,
  sender_pseudonym_id TEXT NOT NULL,
  key_epoch INTEGER NOT NULL DEFAULT 1,
  wrapped_key_b64 TEXT NOT NULL,       -- base64(seal_x25519(CEK, recipient_pub))
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE (channel_id, recipient_pseudonym_id, key_epoch),
  FOREIGN KEY (server_id) REFERENCES servers(id),
  FOREIGN KEY (channel_id) REFERENCES channels(channel_id)
);

CREATE INDEX idx_channel_key_wraps_recipient
  ON channel_key_wraps(channel_id, recipient_pseudonym_id);

-- A membership proof has to be evidence of a LIVE authentication.
--
-- Until this table existed, every field of a `POST /api/zk/verify-membership`
-- request was stable for a given member and topic: the Merkle root, the
-- commitment, the nullifier, the topic hash and the Groth16 proof over them.
-- The whole body was a bearer credential. Capture one successful request and
-- you could re-submit it verbatim — after the member's sessions had been
-- revoked, without ever holding `sk` — and the server minted a fresh session
-- token at the identity's CURRENT revocation epoch. Revocation did not survive
-- its own re-authentication path, which is the one path it most needs to.
--
-- The closure is a challenge that is issued by this server, bound to one
-- commitment, usable once, and — critically — carried INSIDE the proof as a
-- constrained public signal (see `zk/circuits/membership_v2.circom`). A nonce
-- sent alongside a replayable proof would not have helped: whoever replays the
-- proof can ask for a nonce of their own. Because the Groth16 verification
-- equation commits to every public input, a proof produced for challenge C is
-- rejected when presented with any other, so replaying the proof means
-- replaying its challenge — and a challenge can only be consumed once.
--
-- `challenge_hex` is the canonical lowercase hex of the BN254 field element
-- that appears in the proof's public signals, so the lookup here and the
-- comparison against the proof are the same value in the same encoding.
--
-- Rows are consumed in place rather than deleted: `consumed_at` is what makes
-- a second presentation distinguishable from an unknown challenge, which is
-- the difference between "this was already used" and "I have never seen this"
-- in the audit trail. A sweeper removes expired rows.
CREATE TABLE IF NOT EXISTS zk_auth_challenges (
    challenge_hex   TEXT    NOT NULL,
    server_id       INTEGER NOT NULL,
    -- The identity this challenge was issued to. A challenge minted for one
    -- commitment cannot be spent by a proof for another, so a challenge
    -- observed in flight is useless to anyone else.
    commitment_hex  TEXT    NOT NULL,
    topic           TEXT    NOT NULL,
    issued_at       INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,
    consumed_at     INTEGER,
    PRIMARY KEY (server_id, challenge_hex),
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);

-- The sweeper's access path, and the only index a single-use lookup needs
-- beyond the primary key.
CREATE INDEX IF NOT EXISTS idx_zk_auth_challenges_expiry
    ON zk_auth_challenges(expires_at);

-- Rate-limiting the issue endpoint per commitment reads this.
CREATE INDEX IF NOT EXISTS idx_zk_auth_challenges_commitment
    ON zk_auth_challenges(server_id, commitment_hex, issued_at);

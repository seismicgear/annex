-- Bind authored RTX publications to a specific accepted VRP decision. Legacy
-- registrations stay NULL and must re-handshake; inventing a ledger entry would
-- misrepresent what was negotiated.
ALTER TABLE agent_registrations ADD COLUMN current_handshake_id INTEGER
    REFERENCES vrp_handshake_log(id);

-- RTX shares the durable federation queue and operator retry controls. Existing
-- message/edit/redaction rows retain their original wire-format routing.
ALTER TABLE federation_outbox ADD COLUMN envelope_kind TEXT NOT NULL DEFAULT 'message'
    CHECK (envelope_kind IN ('message', 'rtx'));

-- A receipt binds the accepted incoming RTX bytes to the authenticated sender.
-- Identical retries acknowledge without repeating storage, fanout, or onward
-- enqueue; a changed envelope using the same bundle ID is not a receipt match.
CREATE TABLE rtx_federation_receipts (
    server_id INTEGER NOT NULL REFERENCES servers(id),
    bundle_id TEXT NOT NULL,
    relaying_server TEXT NOT NULL,
    envelope_digest TEXT NOT NULL,
    origin_content_digest TEXT NOT NULL,
    origin_attestation_json TEXT,
    received_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (server_id, bundle_id, relaying_server)
);

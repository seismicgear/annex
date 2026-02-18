-- Public event log for the observability layer (Phase 10).
--
-- Append-only log of all auditable events across domains: IDENTITY,
-- PRESENCE, FEDERATION, AGENT, MODERATION.  Queryable by domain,
-- event type, entity, and time range.

CREATE TABLE public_event_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    domain TEXT NOT NULL,
    event_type TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    occurred_at TEXT NOT NULL
);

-- Primary query path: filter by server + domain, ordered by time.
CREATE INDEX idx_event_log_server_domain_time
    ON public_event_log (server_id, domain, occurred_at);

-- Secondary: look up events for a specific entity.
CREATE INDEX idx_event_log_entity
    ON public_event_log (server_id, entity_type, entity_id);

-- Sequence ordering within a server.
CREATE INDEX idx_event_log_server_seq
    ON public_event_log (server_id, seq);

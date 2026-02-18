-- Production hardening indexes and constraints.
--
-- Addresses:
-- H-28: graph_edges missing indexes for BFS queries
-- L-21: graph_edges missing UNIQUE constraint on (server_id, from_node, to_node, kind)
-- M-34: vrp_handshake_log missing index for reputation queries
-- M-35: federation_agreements missing index for active lookups
-- M-36: vrp_leaves missing index for server-scoped queries
-- M-37: messages missing index on expires_at for retention cleanup

-- BFS queries scan from_node and to_node; without indexes these are full table scans.
CREATE INDEX IF NOT EXISTS idx_graph_edges_from
    ON graph_edges(server_id, from_node);

CREATE INDEX IF NOT EXISTS idx_graph_edges_to
    ON graph_edges(server_id, to_node);

-- Prevent duplicate edges of the same kind between the same pair of nodes.
CREATE UNIQUE INDEX IF NOT EXISTS idx_graph_edges_unique_triple
    ON graph_edges(server_id, from_node, to_node, kind);

-- Reputation queries look up all handshake outcomes for a specific peer.
CREATE INDEX IF NOT EXISTS idx_vrp_handshake_log_peer
    ON vrp_handshake_log(server_id, peer_pseudonym);

-- Federation agreement lookups filter by remote_instance_id and active.
CREATE INDEX IF NOT EXISTS idx_federation_agreements_remote_active
    ON federation_agreements(remote_instance_id, active);

-- Leaf lookups by commitment_hex for reverse-mapping operations.
CREATE INDEX IF NOT EXISTS idx_vrp_leaves_commitment
    ON vrp_leaves(commitment_hex);

-- Retention cleanup deletes messages WHERE expires_at <= now.
CREATE INDEX IF NOT EXISTS idx_messages_expires_at
    ON messages(expires_at)
    WHERE expires_at IS NOT NULL;

/**
 * Federation panel — shows federated peers and their trust status.
 *
 * Supports federation hopping: users can discover servers through the
 * trusted edges of their current community and seamlessly join them.
 * "View Upstream Federation" pulls peer metadata; "Join this Server"
 * establishes a new cryptographic identity on the remote node.
 */

import { useEffect, useState, useCallback } from 'react';
import * as api from '@/lib/api';
import { useServersStore } from '@/stores/servers';
import { InfoTip } from '@/components/InfoTip';
import type { FederationPeer, ServerSummary } from '@/types';

interface PeerDetailProps {
  peer: FederationPeer;
  onClose: () => void;
}

function PeerDetail({ peer, onClose }: PeerDetailProps) {
  const [summary, setSummary] = useState<ServerSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [joining, setJoining] = useState(false);
  const [joined, setJoined] = useState(false);
  const addRemoteServer = useServersStore((s) => s.addRemoteServer);
  const servers = useServersStore((s) => s.servers);

  const alreadyJoined = servers.some(
    (s) => s.baseUrl === peer.base_url || s.slug === summary?.slug,
  );

  useEffect(() => {
    let cancelled = false;
    api.getRemoteServerSummary(peer.base_url)
      .then((s) => { if (!cancelled) setSummary(s); })
      .catch(() => { if (!cancelled) setSummary(null); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [peer.base_url]);

  const handleJoin = useCallback(async () => {
    setJoining(true);
    try {
      const server = await addRemoteServer(peer.base_url);
      if (server) setJoined(true);
    } catch {
      // Join failed — user can retry via the Explore button
    } finally {
      setJoining(false);
    }
  }, [peer.base_url, addRemoteServer]);

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog peer-detail-dialog" onClick={(e) => e.stopPropagation()}>
        <h3>Upstream Federation</h3>

        {loading ? (
          <p className="loading-text">Fetching server metadata...</p>
        ) : summary ? (
          <div className="peer-detail-info">
            <div className="peer-detail-header">
              <span className="peer-detail-label">{summary.label}</span>
              <span className="peer-detail-slug">{summary.slug}</span>
            </div>
            <div className="peer-detail-stats">
              <div className="stat">
                <span className="stat-value">{summary.total_active_members}</span>
                <span className="stat-label">members</span>
              </div>
              <div className="stat">
                <span className="stat-value">{summary.channel_count}</span>
                <span className="stat-label">channels</span>
              </div>
              <div className="stat">
                <span className="stat-value">{summary.federation_peer_count}</span>
                <span className="stat-label">peers</span>
              </div>
              <div className="stat">
                <span className="stat-value">{summary.active_agent_count}</span>
                <span className="stat-label">agents</span>
              </div>
            </div>
            <div className="peer-detail-trust">
              <span className={`alignment-badge alignment-${peer.alignment_status.toLowerCase()}`}>
                {peer.alignment_status}<InfoTip text="Shows how well this server's values match yours. 'Aligned' means strong trust; 'Unverified' means no assessment yet." />
              </span>
              <span className="scope-badge">{peer.transfer_scope}<InfoTip text="What kind of data can flow between servers — for example, messages only, or messages and media." /></span>
            </div>
          </div>
        ) : (
          <p className="error-text">Could not reach server at {peer.base_url}</p>
        )}

        <div className="dialog-actions">
          <button onClick={onClose}>Close</button>
          {summary && !alreadyJoined && !joined && (
            <button
              className="primary-btn"
              onClick={handleJoin}
              disabled={joining}
            >
              {joining ? 'Joining...' : 'Join this Server'}
            </button>
          )}
          {(alreadyJoined || joined) && (
            <span className="joined-badge">Already in server list</span>
          )}
        </div>
      </div>
    </div>
  );
}

export function FederationPanel() {
  const [peers, setPeers] = useState<FederationPeer[]>([]);
  const [loading, setLoading] = useState(true);
  const [selectedPeer, setSelectedPeer] = useState<FederationPeer | null>(null);

  useEffect(() => {
    api
      .getFederationPeers()
      .then((r) => setPeers(r.peers))
      .catch(() => { /* fetch failed — empty peers list displayed */ })
      .finally(() => setLoading(false));
  }, []);

  if (loading) return <div className="federation-panel loading">Loading peers...</div>;

  if (peers.length === 0) {
    return (
      <div className="federation-panel empty">
        <p>No federation peers</p>
        <p className="federation-hint">
          Federation peers appear when your server operator establishes
          trust relationships with other Annex nodes.
        </p>
      </div>
    );
  }

  return (
    <>
      <div className="federation-panel">
        <h3>Federation Peers<InfoTip text="These are other Annex servers your community is connected to. You can explore them and join ones that interest you." /></h3>
        <p className="federation-description">
          Discover new communities through the trusted edges of your current network.
        </p>
        <ul className="peer-list">
          {peers.map((peer) => (
            <li key={peer.instance_id} className="peer-item">
              <div className="peer-info">
                <div className="peer-label">{peer.label}</div>
                <div className="peer-url">{peer.base_url}</div>
              </div>
              <div className="peer-trust">
                <div className={`peer-alignment alignment-${peer.alignment_status.toLowerCase()}`}>
                  {peer.alignment_status}<InfoTip text="Shows how well this server's values match yours. 'Aligned' means strong trust; 'Unverified' means no assessment yet." />
                </div>
                <div className="peer-scope">{peer.transfer_scope}<InfoTip text="What kind of data can flow between servers — for example, messages only, or messages and media." /></div>
              </div>
              <button
                className="peer-explore-btn"
                onClick={() => setSelectedPeer(peer)}
                title="View upstream federation"
              >
                Explore
              </button>
            </li>
          ))}
        </ul>
      </div>

      {selectedPeer && (
        <PeerDetail
          peer={selectedPeer}
          onClose={() => setSelectedPeer(null)}
        />
      )}
    </>
  );
}

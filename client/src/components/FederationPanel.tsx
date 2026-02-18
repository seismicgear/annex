/**
 * Federation panel — shows federated peers and their trust status.
 */

import { useEffect, useState } from 'react';
import * as api from '@/lib/api';
import type { FederationPeer } from '@/types';

export function FederationPanel() {
  const [peers, setPeers] = useState<FederationPeer[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    api
      .getFederationPeers()
      .then((r) => setPeers(r.peers))
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  if (loading) return <div className="federation-panel loading">Loading peers...</div>;

  if (peers.length === 0) {
    return (
      <div className="federation-panel empty">
        <p>No federation peers</p>
      </div>
    );
  }

  return (
    <div className="federation-panel">
      <h3>Federation Peers</h3>
      <ul className="peer-list">
        {peers.map((peer) => (
          <li key={peer.instance_id} className="peer-item">
            <div className="peer-label">{peer.label}</div>
            <div className="peer-url">{peer.base_url}</div>
            <div className={`peer-alignment alignment-${peer.alignment_status.toLowerCase()}`}>
              {peer.alignment_status}
            </div>
            <div className="peer-scope">{peer.transfer_scope}</div>
          </li>
        ))}
      </ul>
    </div>
  );
}

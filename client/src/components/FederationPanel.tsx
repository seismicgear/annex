/**
 * Federation panel — shows federated peers and their trust status.
 *
 * Supports federation hopping: users can discover servers through the
 * trusted edges of their current community and seamlessly join them.
 * "View Upstream Federation" pulls peer metadata; "Join this Server"
 * establishes a new cryptographic identity on the remote node.
 */

import { useEffect, useState, useCallback, type ReactNode } from 'react';
import * as api from '@/lib/api';
import { useServersStore } from '@/stores/servers';
import { InfoTip } from '@/components/InfoTip';
import type { FederationPeer, ServerSummary } from '@/types';
import { Modal } from '@/components/Modal';
import { useDialogTitleId } from '@/lib/use-dialog-title-id';

interface PeerDetailProps {
  peer: FederationPeer;
  onClose: () => void;
}

type JoinPhase = 'idle' | 'adding' | 'registering' | 'complete' | 'error';

function PeerDetail({ peer, onClose }: PeerDetailProps) {
  const titleId = useDialogTitleId();
  const [summary, setSummary] = useState<ServerSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const [joinPhase, setJoinPhase] = useState<JoinPhase>('idle');
  const [joinError, setJoinError] = useState<string | null>(null);
  const beginRemoteRegistration = useServersStore((s) => s.beginRemoteRegistration);
  const servers = useServersStore((s) => s.servers);

  // Only show "already joined" if the server has a real identityId (not a placeholder)
  const alreadyJoined = servers.some(
    (s) => s.identityId && (s.baseUrl === peer.base_url || s.slug === summary?.slug),
  );

  /**
   * Fetch the peer's public summary.
   *
   * The enclosing panel already learned this lesson — its list reports a
   * failed load and offers a retry rather than rendering "no peers" — and the
   * dialog nested inside it did not. A dropped cross-origin request left
   * "Could not reach server at …" with no way to try again, and because the
   * Join button is gated on `summary`, the whole dialog became a dead end:
   * the only way to retry was to close it and reopen. The reason matters too,
   * since a CORS refusal and an unreachable host are different problems for
   * whoever operates that server.
   */
  const loadSummary = useCallback(async () => {
    setLoading(true);
    setSummaryError(null);
    try {
      const s = await api.getRemoteServerSummary(peer.base_url);
      setSummary(s);
    } catch (err) {
      setSummary(null);
      setSummaryError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [peer.base_url]);

  useEffect(() => {
    void loadSummary();
  }, [loadSummary]);

  const handleJoin = useCallback(async () => {
    setJoinPhase('adding');
    setJoinError(null);
    try {
      const server = await beginRemoteRegistration(peer.base_url);
      if (server) {
        if (server.identityId) {
          // Already had identity — direct switch completed
          setJoinPhase('complete');
        } else {
          // Registration kicked off — App.tsx auto-register effect will handle it
          setJoinPhase('registering');
        }
      } else {
        setJoinError(
          useServersStore.getState().registrationError ?? 'Could not reach server.',
        );
        setJoinPhase('error');
      }
    } catch (err) {
      setJoinError(err instanceof Error ? err.message : 'Join failed');
      setJoinPhase('error');
    }
  }, [peer.base_url, beginRemoteRegistration]);

  return (
    <Modal onClose={onClose} className="peer-detail-dialog" titleId={titleId}>
      <h2 id={titleId}>Upstream Federation</h2>

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
        <div className="peer-detail-error" role="alert">
          <p className="error-text">
            Could not reach server at {peer.base_url}
            {summaryError ? `: ${summaryError}` : ''}
          </p>
          <button onClick={() => { void loadSummary(); }}>Retry</button>
        </div>
      )}

      {joinError && (
        <p className="error-text" role="alert">{joinError}</p>
      )}
      <div className="dialog-actions">
        <button onClick={onClose}>Close</button>
        {summary && !alreadyJoined && joinPhase !== 'complete' && joinPhase !== 'registering' && (
          <button
            className="primary-btn"
            onClick={handleJoin}
            disabled={joinPhase === 'adding'}
          >
            {joinPhase === 'adding' ? 'Adding server...' : joinPhase === 'error' ? 'Retry' : 'Join this Server'}
          </button>
        )}
        {joinPhase === 'registering' && (
          <span className="joined-badge">Registration in progress...</span>
        )}
        {(alreadyJoined || joinPhase === 'complete') && (
          <span className="joined-badge">Already in server list</span>
        )}
      </div>
    </Modal>
  );
}

export function FederationPanel() {
  const [peers, setPeers] = useState<FederationPeer[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedPeer, setSelectedPeer] = useState<FederationPeer | null>(null);

  const loadPeers = useCallback(async () => {
    setLoading(true);
    try {
      const r = await api.getFederationPeers();
      setPeers(r.peers);
      setError(null);
    } catch (err) {
      // A swallowed failure rendered as "No federation peers", so a broken
      // backend was indistinguishable from a standalone server with none —
      // and there was nothing to retry. Report it as what it is.
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadPeers();
  }, [loadPeers]);

  // One shell for every state.
  //
  // The loading, error and empty branches used to return a bare container with
  // no heading, so opening the Federation tab on a server with no peers showed
  // "No federation peers" and nothing saying which section that even was — the
  // title only appeared once there was something to title. It is the same shape
  // as ChannelList's loading/error and MessageView's empty branches: a
  // component whose non-happy paths quietly drop the structure its happy path
  // provides. Keeping the heading also keeps the document outline contiguous
  // in every state rather than only the populated one.
  const shell = (children: ReactNode, extraClass = '') => (
    <div className={`federation-panel ${extraClass}`.trim()}>
      <h2>
        Federation Peers
        <InfoTip text="These are other Annex servers your community is connected to. You can explore them and join ones that interest you." />
      </h2>
      {children}
    </div>
  );

  if (loading) {
    return shell(<p className="federation-hint">Loading peers...</p>, 'loading');
  }

  if (error) {
    return shell(
      <>
        <p className="error-text" role="alert">
          Could not load federation peers: {error}
        </p>
        <p className="federation-hint">
          This is a problem reaching your own server, not a sign that no peers exist.
        </p>
        <button className="primary-btn" onClick={() => { void loadPeers(); }}>
          Retry
        </button>
      </>,
      'empty',
    );
  }

  if (peers.length === 0) {
    return shell(
      <>
        <p>No federation peers</p>
        <p className="federation-hint">
          Federation peers appear when your server operator establishes
          trust relationships with other Annex nodes.
        </p>
      </>,
      'empty',
    );
  }

  return (
    <>
      <div className="federation-panel">
        <h2>
          Federation Peers
          <InfoTip text="These are other Annex servers your community is connected to. You can explore them and join ones that interest you." />
        </h2>
        <p className="federation-description">
          Discover new communities through the trusted edges of your current network.
        </p>
        <ul className="peer-list">
          {peers.map((peer) => (
            <li key={peer.agreement_id} className="peer-item">
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

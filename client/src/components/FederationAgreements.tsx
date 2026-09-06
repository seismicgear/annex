/**
 * Federation agreements, and the only way to end one.
 *
 * `DELETE /api/admin/federation/{id}` had no caller — and could not have
 * had one, because it takes an agreement id and nothing a client could see
 * returned one. `GET /api/public/federation/peers` sent base URL, label,
 * alignment and scope, and no identifier at all. So an operator who had
 * stopped trusting a peer could not cut it off from anywhere in the app.
 *
 * Severing is not reversible from here: a new agreement needs a fresh
 * handshake from the other side. The confirmation says so rather than
 * asking "are you sure".
 */

import { useCallback, useEffect, useState } from 'react';
import * as api from '@/lib/api';
import { Modal } from '@/components/Modal';
import { useDialogTitleId } from '@/lib/use-dialog-title-id';
import type { FederationPeer } from '@/types';

export function FederationAgreements({ pseudonymId }: { pseudonymId: string }) {
  const [peers, setPeers] = useState<FederationPeer[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pendingRevoke, setPendingRevoke] = useState<FederationPeer | null>(null);
  const [revoking, setRevoking] = useState(false);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const [severed, setSevered] = useState<string | null>(null);
  const titleId = useDialogTitleId();

  const load = useCallback(async () => {
    // A failed read is not "no peers". Rendering it as one would tell an
    // operator this server federates with nobody, which is the conclusion
    // they would act on and the one the request did not support.
    try {
      const result = await api.getFederationPeers();
      setPeers(result.peers);
      setError(null);
    } catch (err) {
      setPeers(null);
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    api
      .getFederationPeers()
      .then((result) => {
        if (!cancelled) {
          setPeers(result.peers);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setPeers(null);
          setError(err instanceof Error ? err.message : String(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [pseudonymId]);

  const handleRevoke = async () => {
    if (!pendingRevoke) return;
    const peer = pendingRevoke;
    setRevoking(true);
    setRevokeError(null);
    try {
      await api.revokeFederationAgreement(pseudonymId, peer.agreement_id);
      setPendingRevoke(null);
      setSevered(peer.label);
      await load();
    } catch (err) {
      setRevokeError(err instanceof Error ? err.message : String(err));
    } finally {
      setRevoking(false);
    }
  };

  return (
    <div className="policy-section federation-agreements">
      <h3>Federation Agreements</h3>
      <p className="field-hint">
        Servers this one exchanges content with. Severing an agreement stops
        the exchange in both directions; re-establishing it needs a fresh
        handshake from the other side.
      </p>

      {error && (
        <p className="error-message" role="alert">
          Could not read the federation agreements: {error}
        </p>
      )}

      {severed && (
        <p className="success-message" role="status">
          Agreement with {severed} severed.
        </p>
      )}

      {peers !== null &&
        (peers.length === 0 ? (
          <p className="agreements-empty">This server has no federation agreements.</p>
        ) : (
          <ul className="agreement-list">
            {peers.map((peer) => (
              <li key={peer.agreement_id} className="agreement-row">
                <div className="agreement-info">
                  <span className="agreement-label">{peer.label}</span>
                  <span className="agreement-url">{peer.base_url}</span>
                </div>
                <span className={`agreement-alignment alignment-${peer.alignment_status.toLowerCase()}`}>
                  {peer.alignment_status}
                </span>
                <span className="agreement-scope">{peer.transfer_scope}</span>
                <button
                  type="button"
                  className="danger-btn agreement-revoke-btn"
                  onClick={() => {
                    setRevokeError(null);
                    setSevered(null);
                    setPendingRevoke(peer);
                  }}
                >
                  Sever
                </button>
              </li>
            ))}
          </ul>
        ))}

      {pendingRevoke && (
        <Modal onClose={() => setPendingRevoke(null)} titleId={titleId}>
          <h2 id={titleId}>Sever federation with {pendingRevoke.label}?</h2>
          <p>
            Content stops flowing between this server and {pendingRevoke.base_url} in
            both directions. Anything already delivered stays where it is.
          </p>
          <p>
            This cannot be undone from here — a new agreement requires the other
            server to hand shake again.
          </p>
          {revokeError && (
            <p className="error-message" role="alert">
              Could not sever the agreement: {revokeError}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" onClick={() => setPendingRevoke(null)}>
              Cancel
            </button>
            <button type="button" className="danger-btn" onClick={handleRevoke} disabled={revoking}>
              {revoking ? 'Severing...' : 'Sever agreement'}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

/**
 * Federation delivery queue — the operator's view of what is or is not
 * reaching the servers this one federates with.
 *
 * `GET /api/admin/federation/outbox` and
 * `POST /api/admin/federation/outbox/{id}/retry` shipped with no caller in
 * the client. The list handler's own doc comment says the status counts
 * exist so "the UI show[s] queue depth and stuck deliveries at a glance" —
 * for a UI that did not exist. A server whose federation deliveries were all
 * failing looked, from every screen in the app, exactly like a server with
 * nothing to send.
 *
 * A retried row still passes the dequeue-time SSRF gate, so retrying a row
 * whose peer URL points at a private host simply re-fails it. That is the
 * server's business; this only has to be honest about what happened.
 */

import { useCallback, useEffect, useState } from 'react';
import * as api from '@/lib/api';
import type { OutboxEntry } from '@/lib/api';

/** The statuses the server accepts as a filter, in queue-lifecycle order. */
const STATUSES = ['pending', 'failed', 'paused', 'delivered'] as const;

/** Only these two can be retried; the server answers 409 for the others. */
const RETRYABLE = new Set(['failed', 'paused']);

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function FederationOutbox({ pseudonymId }: { pseudonymId: string }) {
  const [entries, setEntries] = useState<OutboxEntry[] | null>(null);
  const [counts, setCounts] = useState<Record<string, number>>({});
  const [filter, setFilter] = useState<string>('');
  const [error, setError] = useState<string | null>(null);
  const [retrying, setRetrying] = useState<number | null>(null);
  const [retryError, setRetryError] = useState<string | null>(null);
  const [retried, setRetried] = useState<string | null>(null);

  const load = useCallback(
    async (status: string) => {
      // A failed read is not an empty queue. Rendering it as one tells the
      // operator every envelope has been delivered, which is the single
      // conclusion this request did not support.
      try {
        const page = await api.getFederationOutbox(pseudonymId, {
          status: status || undefined,
          limit: 50,
        });
        setEntries(page.entries);
        setCounts(page.counts);
        setError(null);
      } catch (err) {
        setEntries(null);
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [pseudonymId],
  );

  useEffect(() => {
    let cancelled = false;
    // Keyed on both, so switching servers or filters cannot land a stale page.
    api
      .getFederationOutbox(pseudonymId, { status: filter || undefined, limit: 50 })
      .then((page) => {
        if (cancelled) return;
        setEntries(page.entries);
        setCounts(page.counts);
        setError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setEntries(null);
        setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [pseudonymId, filter]);

  const handleRetry = async (entry: OutboxEntry) => {
    setRetrying(entry.id);
    setRetryError(null);
    setRetried(null);
    try {
      const result = await api.retryFederationOutboxRow(pseudonymId, entry.id);
      setRetried(result.message_id);
      await load(filter);
    } catch (err) {
      setRetryError(err instanceof Error ? err.message : String(err));
    } finally {
      setRetrying(null);
    }
  };

  const total = Object.values(counts).reduce((a, b) => a + b, 0);

  return (
    <div className="federation-outbox">
      <div className="policy-section">
        <h3>Delivery Queue</h3>
        <p className="field-hint">
          Envelopes this server is sending to its federation peers. A row that
          keeps failing is a delivery nobody on the other side has received.
        </p>

        {error && (
          <p className="error-message" role="alert">
            Could not read the delivery queue: {error}
          </p>
        )}

        {entries !== null && (
          <>
            <ul className="outbox-counts">
              {STATUSES.map((s) => {
                const n = counts[s] ?? 0;
                // The failed tally is only tinted when there is something to
                // tint. A red zero is an alarm for a condition that is not
                // happening, and a panel that cries wolf on an empty queue is
                // one an operator learns to stop reading.
                const cls = `outbox-count outbox-count-${s}${n > 0 ? ' outbox-count-nonzero' : ''}`;
                return (
                  <li key={s} className={cls}>
                    <span className="outbox-count-n">{n}</span>
                    <span className="outbox-count-label">{s}</span>
                  </li>
                );
              })}
            </ul>

            <div className="outbox-filter">
              <label>
                Show
                <select value={filter} onChange={(e) => setFilter(e.target.value)}>
                  <option value="">all statuses</option>
                  {STATUSES.map((s) => (
                    <option key={s} value={s}>
                      {s}
                    </option>
                  ))}
                </select>
              </label>
            </div>

            {retried && (
              <p className="success-message" role="status">
                Message {retried} is back in the retry rotation.
              </p>
            )}
            {retryError && (
              <p className="error-message" role="alert">
                Could not retry that delivery: {retryError}
              </p>
            )}

            {entries.length === 0 ? (
              <p className="outbox-empty">
                {total === 0
                  ? 'Nothing has been queued for a federation peer yet.'
                  : `No ${filter} deliveries. The queue holds ${total} in other states.`}
              </p>
            ) : (
              <ul className="outbox-list">
                {entries.map((entry) => (
                  <li key={entry.id} className={`outbox-row outbox-row-${entry.status}`}>
                    <div className="outbox-row-head">
                      <span className={`outbox-status outbox-status-${entry.status}`}>
                        {entry.status}
                      </span>
                      <span className="outbox-peer">
                        {/* The instance row can be gone while its queued rows
                            remain, and "undefined" in the peer column is worse
                            than saying which id it pointed at. */}
                        {entry.peer_label ??
                          entry.peer_base_url ??
                          `peer #${entry.peer_instance_id} (removed)`}
                      </span>
                      {RETRYABLE.has(entry.status) && (
                        <button
                          type="button"
                          className="outbox-retry-btn"
                          onClick={() => handleRetry(entry)}
                          disabled={retrying === entry.id}
                        >
                          {retrying === entry.id ? 'Retrying...' : 'Retry'}
                        </button>
                      )}
                    </div>
                    <div className="outbox-row-meta">
                      <span>message {entry.message_id}</span>
                      <span>
                        {entry.attempts} {entry.attempts === 1 ? 'attempt' : 'attempts'}
                      </span>
                      <span>{formatBytes(entry.envelope_bytes)}</span>
                      <span>next try {entry.next_retry_at}</span>
                    </div>
                    {entry.last_error && (
                      <p className="outbox-row-error">{entry.last_error}</p>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </div>
    </div>
  );
}

/**
 * Event log viewer — displays server audit events with domain filtering.
 *
 * Uses the public events API so no auth is required.
 */

import { useEffect, useState, useCallback } from 'react';
import * as api from '@/lib/api';
import type { PublicEvent } from '@/types';

const DOMAINS = ['ALL', 'IDENTITY', 'PRESENCE', 'FEDERATION', 'AGENT', 'MODERATION'];

export function EventLog() {
  const [events, setEvents] = useState<PublicEvent[]>([]);
  const [domain, setDomain] = useState('ALL');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchEvents = useCallback(async () => {
    setLoading(true);
    try {
      const result = await api.getPublicEvents(
        domain === 'ALL' ? undefined : domain,
        undefined,
        100,
      );
      setEvents(result);
      setError(null);
    } catch (err) {
      // Previously this swallowed the failure and left the last-known list on
      // screen. Combined with the 10s auto-refresh that meant a dead backend
      // looked exactly like a quiet server — the audit log silently stopped
      // being an audit log. Surface it instead, while keeping whatever we
      // already fetched visible so a transient blip does not blank the table.
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [domain]);

  useEffect(() => {
    fetchEvents();
  }, [fetchEvents]);

  // Auto-refresh every 10 seconds
  useEffect(() => {
    const interval = setInterval(fetchEvents, 10000);
    return () => clearInterval(interval);
  }, [fetchEvents]);

  return (
    <div className="event-log">
      <div className="event-log-header">
        <h2>Event Log</h2>
        <div className="event-log-controls">
          <select
            className="domain-filter"
            value={domain}
            aria-label="Filter events by domain"
            onChange={(e) => setDomain(e.target.value)}
          >
            {DOMAINS.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
          <button onClick={fetchEvents} disabled={loading} className="refresh-btn">
            {loading ? 'Loading...' : 'Refresh'}
          </button>
        </div>
      </div>

      {error && (
        <div className="event-log-error" role="alert">
          <span>Could not load events: {error}</span>
          <button onClick={fetchEvents} className="secondary-btn" disabled={loading}>
            Retry
          </button>
        </div>
      )}

      {events.length === 0 && !loading && !error && (
        <p className="event-log-empty">No events found</p>
      )}

      <div className="event-table">
        {events.length > 0 && (
          <div className="event-table-header">
            <span className="event-col-time">Time</span>
            <span className="event-col-domain">Domain</span>
            <span className="event-col-type">Type</span>
            <span className="event-col-entity">Entity</span>
            <span className="event-col-detail">Detail</span>
          </div>
        )}
        {events.map((evt) => {
          let detail = '';
          try {
            const payload = JSON.parse(evt.payload_json);
            detail = payload.description || payload.action_type || JSON.stringify(payload).slice(0, 80);
          } catch {
            detail = evt.payload_json.slice(0, 80);
          }
          return (
            <div key={evt.id} className="event-row">
              <span className="event-col-time">
                {new Date(evt.occurred_at.endsWith('Z') ? evt.occurred_at : evt.occurred_at + 'Z').toLocaleTimeString()}
              </span>
              <span className={`event-col-domain domain-${evt.domain.toLowerCase()}`}>
                {evt.domain}
              </span>
              <span className="event-col-type">{evt.event_type}</span>
              <span className="event-col-entity" title={evt.entity_id ?? ''}>
                {evt.entity_id ? `${evt.entity_id.slice(0, 12)}...` : '—'}
              </span>
              <span className="event-col-detail" title={detail}>
                {detail}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

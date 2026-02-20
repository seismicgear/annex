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

  const fetchEvents = useCallback(async () => {
    setLoading(true);
    try {
      const result = await api.getPublicEvents(
        domain === 'ALL' ? undefined : domain,
        undefined,
        100,
      );
      setEvents(result);
    } catch {
      // Fetch failed — keep existing events visible
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

      {events.length === 0 && !loading && (
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
                {new Date(evt.occurred_at).toLocaleTimeString()}
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

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { parseMessageTimestamp } from './MessageView';

describe('parseMessageTimestamp', () => {
  it('parses SQLite-style history timestamp (YYYY-MM-DD HH:MM:SS)', () => {
    const ms = parseMessageTimestamp('2025-01-01 00:00:00');
    expect(ms).toBe(new Date('2025-01-01T00:00:00Z').getTime());
    expect(isNaN(ms)).toBe(false);
  });

  it('parses RFC3339 WebSocket timestamp with Z suffix', () => {
    const ms = parseMessageTimestamp('2025-01-01T00:00:00Z');
    expect(ms).toBe(new Date('2025-01-01T00:00:00Z').getTime());
    expect(isNaN(ms)).toBe(false);
  });

  it('parses RFC3339 WebSocket timestamp with timezone offset', () => {
    const ms = parseMessageTimestamp('2025-01-01T00:00:00+00:00');
    expect(ms).toBe(new Date('2025-01-01T00:00:00Z').getTime());
  });

  it('returns NaN for empty string', () => {
    expect(isNaN(parseMessageTimestamp(''))).toBe(true);
  });

  it('returns NaN for malformed values', () => {
    expect(isNaN(parseMessageTimestamp('not-a-date'))).toBe(true);
  });

  it('does not double-append Z to RFC3339 timestamps', () => {
    // This was the original bug: appending 'Z' to a timestamp that already has 'Z'
    const rfc3339 = '2025-06-15T12:30:45Z';
    const ms = parseMessageTimestamp(rfc3339);
    const expected = new Date(rfc3339).getTime();
    expect(ms).toBe(expected);
  });

  it('edit/delete buttons are shown during the first minute for a WebSocket timestamp', () => {
    // Create a timestamp that's 30 seconds ago
    const thirtySecondsAgo = new Date(Date.now() - 30_000).toISOString();
    const ms = parseMessageTimestamp(thirtySecondsAgo);
    const elapsed = Date.now() - ms;
    expect(elapsed).toBeLessThan(60_000);
    expect(elapsed).toBeGreaterThanOrEqual(0);
  });

  it('edit/delete buttons are shown during the first minute for a history timestamp', () => {
    // Create a SQLite-style timestamp that's 30 seconds ago
    const now = new Date(Date.now() - 30_000);
    const sqliteTs = now.toISOString().replace('T', ' ').replace(/\.\d+Z$/, '');
    const ms = parseMessageTimestamp(sqliteTs);
    const elapsed = Date.now() - ms;
    expect(elapsed).toBeLessThan(60_000);
    expect(elapsed).toBeGreaterThanOrEqual(0);
  });
});

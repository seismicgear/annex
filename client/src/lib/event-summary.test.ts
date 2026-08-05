import { describe, it, expect } from 'vitest';
import { summarizeEventPayload } from './event-summary';

/**
 * The thirteen variants of `EventPayload` in `crates/annex-observe/src/event.rs`,
 * serialised the way serde writes them (`#[serde(tag = "event", rename_all =
 * "SCREAMING_SNAKE_CASE")]`). If a variant is added there without a case here,
 * `covers_every_server_payload_variant` fails.
 */
const PAYLOADS: Record<string, object> = {
  IDENTITY_REGISTERED: { event: 'IDENTITY_REGISTERED', commitment_hex: 'a3f2'.repeat(16), role_code: 1 },
  IDENTITY_VERIFIED: { event: 'IDENTITY_VERIFIED', commitment_hex: 'a3f2'.repeat(16), topic: 'membership' },
  PSEUDONYM_DERIVED: { event: 'PSEUDONYM_DERIVED', pseudonym_id: '48bf17'.repeat(4), topic: 'membership' },
  NODE_ADDED: { event: 'NODE_ADDED', pseudonym_id: '48bf17'.repeat(4), node_type: 'AI_AGENT' },
  NODE_PRUNED: { event: 'NODE_PRUNED', pseudonym_id: '48bf17'.repeat(4) },
  NODE_REACTIVATED: { event: 'NODE_REACTIVATED', pseudonym_id: '48bf17'.repeat(4) },
  FEDERATION_ESTABLISHED: {
    event: 'FEDERATION_ESTABLISHED',
    remote_url: 'https://peer.example',
    alignment_status: 'FULLY_ALIGNED',
  },
  FEDERATION_REALIGNED: {
    event: 'FEDERATION_REALIGNED',
    remote_url: 'https://peer.example',
    alignment_status: 'PARTIALLY_ALIGNED',
    previous_status: 'FULLY_ALIGNED',
  },
  FEDERATION_SEVERED: {
    event: 'FEDERATION_SEVERED',
    remote_url: 'https://peer.example',
    reason: 'policy mismatch',
  },
  AGENT_CONNECTED: {
    event: 'AGENT_CONNECTED',
    pseudonym_id: '48bf17'.repeat(4),
    alignment_status: 'FULLY_ALIGNED',
  },
  AGENT_REALIGNED: {
    event: 'AGENT_REALIGNED',
    pseudonym_id: '48bf17'.repeat(4),
    alignment_status: 'UNALIGNED',
    previous_status: 'FULLY_ALIGNED',
  },
  AGENT_DISCONNECTED: {
    event: 'AGENT_DISCONNECTED',
    pseudonym_id: '48bf17'.repeat(4),
    reason: 'idle timeout',
  },
  MODERATION_ACTION: {
    event: 'MODERATION_ACTION',
    moderator_pseudonym: 'aa'.repeat(12),
    action_type: 'delete_message',
    target_pseudonym: 'bb'.repeat(12),
    description: 'Removed a message for spam',
  },
};

const summarize = (payload: object) => summarizeEventPayload(JSON.stringify(payload));

describe('summarizeEventPayload', () => {
  it('covers every server payload variant with a non-empty summary', () => {
    for (const [name, payload] of Object.entries(PAYLOADS)) {
      expect(summarize(payload), `${name} produced no summary`).not.toBe('');
    }
  });

  // The bug this replaced: the Detail column repeated the event name already
  // shown in the Type column, then spent its budget on an id already shown in
  // the Entity column, so the one new field fell off the end.
  it('never repeats the event tag or a raw identifier', () => {
    for (const [name, payload] of Object.entries(PAYLOADS)) {
      const summary = summarize(payload);
      expect(summary, `${name} leaked the event tag`).not.toContain(name);
      expect(summary, `${name} leaked a commitment`).not.toContain('a3f2a3f2');
      expect(summary, `${name} leaked a pseudonym`).not.toContain('48bf1748bf17');
      expect(summary, `${name} rendered raw JSON`).not.toContain('{"');
    }
  });

  it('names the role a registrant claimed', () => {
    expect(summarize(PAYLOADS.IDENTITY_REGISTERED)).toBe('Role: Human');
    expect(summarize({ event: 'IDENTITY_REGISTERED', role_code: 2 })).toBe('Role: AI agent');
  });

  it('falls back to the numeric code for a role this build does not know', () => {
    expect(summarize({ event: 'IDENTITY_REGISTERED', role_code: 99 })).toBe('Role code 99');
  });

  it('surfaces the topic a proof was verified against', () => {
    expect(summarize(PAYLOADS.IDENTITY_VERIFIED)).toBe('Topic: membership');
    expect(summarize(PAYLOADS.PSEUDONYM_DERIVED)).toBe('Topic: membership');
  });

  it('reads enum values as prose', () => {
    expect(summarize(PAYLOADS.NODE_ADDED)).toBe('Joined as Ai agent');
    expect(summarize(PAYLOADS.AGENT_CONNECTED)).toBe('Connected · Fully aligned');
  });

  it('shows both sides of a realignment', () => {
    expect(summarize(PAYLOADS.FEDERATION_REALIGNED)).toBe(
      'https://peer.example · Fully aligned → Partially aligned',
    );
    expect(summarize(PAYLOADS.AGENT_REALIGNED)).toBe('Fully aligned → Unaligned');
  });

  it('explains a prune rather than restating the pseudonym', () => {
    expect(summarize(PAYLOADS.NODE_PRUNED)).toBe('Pruned for inactivity');
    expect(summarize(PAYLOADS.NODE_REACTIVATED)).toBe('Reactivated after inactivity');
  });

  it('prefers the human-written description on a moderation action', () => {
    expect(summarize(PAYLOADS.MODERATION_ACTION)).toBe('Removed a message for spam');
  });

  it('falls back to the action verb when a moderation description is blank', () => {
    expect(summarize({ event: 'MODERATION_ACTION', action_type: 'ban', description: '' })).toBe('Ban');
  });

  // A newer server emitting an event type this client predates must still
  // produce something readable — and still must not dump raw JSON.
  it('describes an unknown event by its scalar fields', () => {
    const summary = summarize({
      event: 'QUORUM_REACHED',
      quorum_size: 5,
      channel_id: 'should-be-hidden',
      commitment_hex: 'should-be-hidden',
      participants: ['a', 'b'],
      meta: { nested: true },
    });
    expect(summary).toContain('quorum_size: 5');
    expect(summary).toContain('participants: 2 items');
    expect(summary).toContain('meta: object');
    expect(summary).not.toContain('should-be-hidden');
  });

  it('truncates non-JSON with an ellipsis instead of cutting mid-token', () => {
    const summary = summarizeEventPayload('x'.repeat(200));
    expect(summary).toHaveLength(80);
    expect(summary.endsWith('…')).toBe(true);
  });

  it('passes short non-JSON through unchanged', () => {
    expect(summarizeEventPayload('legacy plaintext note')).toBe('legacy plaintext note');
  });
});

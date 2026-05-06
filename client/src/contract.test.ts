/**
 * Contract tests for the shared protocol fixtures under `fixtures/`.
 *
 * Each fixture is the canonical wire representation of one request,
 * response, or WebSocket frame. These tests pin the TypeScript side of
 * every contract; `crates/annex-server/tests/contract_fixtures.rs`
 * pins the matching Rust side.
 *
 *   • Request fixtures   → match the body that the corresponding
 *     `@/lib/api` helper emits, plus the keys the Rust handler accepts.
 *   • Response fixtures  → match the typed response that consumers
 *     receive from `@/lib/api` (where one is exported), or the literal
 *     wire shape when the helper does not declare a return type.
 *   • Incoming WS frames → match the `WsSendFrame` union the client
 *     produces.
 *   • Outgoing WS frames → match the `WsReceiveFrame` union the client
 *     consumes.
 */

import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import type {
  Channel,
  RegistrationResponse,
  VerifyMembershipResponse,
  WsReceiveFrame,
  WsSendFrame,
} from '@/types';

const __dirname = dirname(fileURLToPath(import.meta.url));
const FIXTURES_ROOT = resolve(__dirname, '..', '..', 'fixtures');

function loadFixture(relativePath: string): unknown {
  return JSON.parse(readFileSync(resolve(FIXTURES_ROOT, relativePath), 'utf-8'));
}

function isString(v: unknown): v is string {
  return typeof v === 'string';
}
function isNumber(v: unknown): v is number {
  return typeof v === 'number';
}
function isBoolean(v: unknown): v is boolean {
  return typeof v === 'boolean';
}
function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

// ── HTTP API: requests ────────────────────────────────────────────────────

describe('contract: HTTP API requests', () => {
  it('register.request matches the api.register() body shape', () => {
    const fx = loadFixture('api/register.request.json');
    expect(isObject(fx)).toBe(true);
    const body = fx as Record<string, unknown>;
    // Required keys (api.register always emits these)
    expect(isString(body.commitmentHex)).toBe(true);
    expect(isNumber(body.roleCode)).toBe(true);
    expect(isNumber(body.nodeId)).toBe(true);
    // Optional keys — when present, must be strings (server treats both
    // missing and null as "no invite" / "no password").
    expect(body.inviteCode === undefined || isString(body.inviteCode)).toBe(true);
    expect(body.serverPassword === undefined || isString(body.serverPassword)).toBe(true);
  });

  it('verify-membership.request matches the api.verifyMembership() body shape', () => {
    const fx = loadFixture('api/verify-membership.request.json');
    expect(isObject(fx)).toBe(true);
    const body = fx as Record<string, unknown>;
    expect(isString(body.root)).toBe(true);
    expect(isString(body.commitment)).toBe(true);
    expect(isString(body.topic)).toBe(true);
    expect(isObject(body.proof)).toBe(true);
    expect(Array.isArray(body.publicSignals)).toBe(true);
    for (const sig of body.publicSignals as unknown[]) expect(isString(sig)).toBe(true);
  });

  it('create-channel.request matches the api.createChannel() body shape', () => {
    const fx = loadFixture('api/create-channel.request.json');
    expect(isObject(fx)).toBe(true);
    const body = fx as Record<string, unknown>;
    // api.createChannel always emits these five keys.
    expect(isString(body.channel_id)).toBe(true);
    expect(isString(body.name)).toBe(true);
    expect(isString(body.channel_type)).toBe(true);
    expect(['Text', 'Voice', 'Hybrid', 'Agent', 'Broadcast']).toContain(body.channel_type);
    expect(body.topic === null || isString(body.topic)).toBe(true);
    expect(['Local', 'Federated']).toContain(body.federation_scope);
  });
});

// ── HTTP API: responses ───────────────────────────────────────────────────

describe('contract: HTTP API responses', () => {
  it('register.response matches RegistrationResponse', () => {
    const fx = loadFixture('api/register.response.json') as RegistrationResponse;
    expect(isNumber(fx.identityId)).toBe(true);
    expect(isNumber(fx.leafIndex)).toBe(true);
    expect(isString(fx.rootHex)).toBe(true);
    expect(Array.isArray(fx.pathElements)).toBe(true);
    for (const e of fx.pathElements) expect(isString(e)).toBe(true);
    expect(Array.isArray(fx.pathIndexBits)).toBe(true);
    for (const b of fx.pathIndexBits) expect(isNumber(b)).toBe(true);
  });

  it('verify-membership.response matches VerifyMembershipResponse', () => {
    const fx = loadFixture('api/verify-membership.response.json') as VerifyMembershipResponse;
    expect(isBoolean(fx.ok)).toBe(true);
    expect(isString(fx.pseudonymId)).toBe(true);
    expect(isString(fx.sessionToken)).toBe(true);
  });

  it('create-channel.response is the literal `{status: "created"}` wire shape', () => {
    // PROTOCOL NOTE: the Rust handler returns `{"status": "created"}`,
    // not a `Channel`. The api.createChannel() helper's `Promise<Channel>`
    // return type is currently misleading — no caller actually reads any
    // Channel field from the resolved value (the channel itself is
    // delivered via the `channel_created` WebSocket broadcast). This
    // fixture pins the actual wire shape. If we ever change the contract
    // to return the Channel, both this test and the Rust contract test
    // need to be updated together.
    const fx = loadFixture('api/create-channel.response.json');
    expect(fx).toEqual({ status: 'created' });
    // Confirm we are NOT accidentally returning a Channel.
    const asChannel = fx as Partial<Channel>;
    expect(asChannel.channel_id).toBeUndefined();
    expect(asChannel.channel_type).toBeUndefined();
  });
});

// ── WebSocket: client → server ────────────────────────────────────────────

describe('contract: WebSocket client → server frames', () => {
  it('incoming-message matches WsSendFrame for type=message', () => {
    const fx = loadFixture('ws/incoming-message.json') as WsSendFrame;
    expect(fx.type).toBe('message');
    expect(isString(fx.channelId)).toBe(true);
    expect(isString(fx.content)).toBe(true);
    expect(fx.replyTo === null || fx.replyTo === undefined || isString(fx.replyTo)).toBe(true);
    expect(fx.clientRequestId === undefined || isString(fx.clientRequestId)).toBe(true);
  });

  it('incoming-edit-message matches WsSendFrame for type=edit_message', () => {
    const fx = loadFixture('ws/incoming-edit-message.json') as WsSendFrame;
    expect(fx.type).toBe('edit_message');
    expect(isString(fx.channelId)).toBe(true);
    expect(isString(fx.messageId)).toBe(true);
    expect(isString(fx.content)).toBe(true);
  });

  it('incoming-delete-message matches WsSendFrame for type=delete_message', () => {
    const fx = loadFixture('ws/incoming-delete-message.json') as WsSendFrame;
    expect(fx.type).toBe('delete_message');
    expect(isString(fx.channelId)).toBe(true);
    expect(isString(fx.messageId)).toBe(true);
  });

  it('incoming-typing matches WsSendFrame for type=typing', () => {
    const fx = loadFixture('ws/incoming-typing.json') as WsSendFrame;
    expect(fx.type).toBe('typing');
    expect(isString(fx.channelId)).toBe(true);
  });

  it('voice-offer matches WsSendFrame for type=webrtc_offer', () => {
    const fx = loadFixture('ws/voice-offer.json') as WsSendFrame;
    expect(fx.type).toBe('webrtc_offer');
    expect(isString(fx.channelId)).toBe(true);
    expect(isString(fx.sdp)).toBe(true);
    expect(fx.sdp!.startsWith('v=0')).toBe(true);
  });

  it('webrtc-ice-candidate matches WsSendFrame for type=webrtc_ice_candidate', () => {
    const fx = loadFixture('ws/webrtc-ice-candidate.json') as WsSendFrame;
    expect(fx.type).toBe('webrtc_ice_candidate');
    expect(isString(fx.channelId)).toBe(true);
    expect(isString(fx.candidate)).toBe(true);
    expect(fx.sdpMid === null || isString(fx.sdpMid)).toBe(true);
    expect(fx.sdpMLineIndex === null || isNumber(fx.sdpMLineIndex)).toBe(true);
    expect(fx.usernameFragment === null || isString(fx.usernameFragment)).toBe(true);
  });
});

// ── WebSocket: server → client ────────────────────────────────────────────

describe('contract: WebSocket server → client frames', () => {
  it('outgoing-message matches WsReceiveFrame for type=message', () => {
    const fx = loadFixture('ws/outgoing-message.json') as WsReceiveFrame;
    expect(fx.type).toBe('message');
    expect(isString(fx.channelId)).toBe(true);
    expect(isString(fx.messageId)).toBe(true);
    expect(isString(fx.senderPseudonym)).toBe(true);
    expect(isString(fx.content)).toBe(true);
    expect(fx.replyToMessageId === null || isString(fx.replyToMessageId)).toBe(true);
    expect(isString(fx.createdAt)).toBe(true);
    expect(fx.clientRequestId === undefined || isString(fx.clientRequestId)).toBe(true);
  });

  it('outgoing-resumed matches WsReceiveFrame for type=resumed', () => {
    const fx = loadFixture('ws/outgoing-resumed.json') as WsReceiveFrame;
    expect(fx.type).toBe('resumed');
    expect(isString(fx.channelId)).toBe(true);
    expect(isNumber(fx.missedCount)).toBe(true);
  });

  it('outgoing-error matches WsReceiveFrame for type=error', () => {
    const fx = loadFixture('ws/outgoing-error.json') as WsReceiveFrame;
    expect(fx.type).toBe('error');
    expect(isString(fx.message)).toBe(true);
    expect(fx.clientRequestId === undefined || isString(fx.clientRequestId)).toBe(true);
  });

  it('webrtc-ice-candidate also satisfies WsReceiveFrame for type=webrtc_ice_candidate', () => {
    // Same fixture, both directions: pin the consumer side too so a
    // future field rename can't pass one direction's test while breaking
    // the other.
    const fx = loadFixture('ws/webrtc-ice-candidate.json') as WsReceiveFrame;
    expect(fx.type).toBe('webrtc_ice_candidate');
    expect(isString(fx.channelId)).toBe(true);
    expect(isString(fx.candidate)).toBe(true);
  });
});

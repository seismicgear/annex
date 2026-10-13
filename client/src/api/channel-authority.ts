/** Explicit authority discovery/attachment. Requires the coordinated authority API. */
import type { ChannelType } from '@/types';
import { authHeaders, getCredentialContext, isCredentialContextCurrent, request, StaleCredentialContextError } from './core';
import { getE2eControls } from '@/lib/e2e-store';

export interface AuthorityPeer { peer_id: number; label: string; base_url: string; authority_identity: string }
export interface AuthorityChannel {
  channel_id: string; name: string; channel_type: ChannelType; e2e_enabled: boolean;
  initialized: boolean; head_seq: number; head_hash: string;
}
export interface AuthorityCatalog { peer: AuthorityPeer; channels: AuthorityChannel[]; next_after_channel_id: string | null }
export interface ChannelAuthority {
  channel_id: string; authority_identity: string; authority_server: string;
  local_authority: boolean; initialized: boolean; head_seq: number; head_hash: string;
}
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid channel authority response.');
  return value as Record<string, unknown>;
}
function text(value: unknown, max = 512): string {
  if (typeof value !== 'string' || !value.length || value.length > max || [...value].some(c => c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127)) throw new Error('Invalid channel authority text.');
  return value;
}
function counter(value: unknown, minimum = 0): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < minimum) throw new Error('Invalid channel authority counter.');
  return value;
}
function flag(value: unknown): boolean { if (typeof value !== 'boolean') throw new Error('Invalid channel authority flag.'); return value; }
function hash(value: unknown): string { if (typeof value !== 'string' || !/^[0-9a-f]{64}$/.test(value)) throw new Error('Invalid channel authority hash.'); return value; }
function fingerprint(value: unknown): string {
  if (typeof value !== 'string' || !/^annex-server:[0-9a-f]{64}$/.test(value)) throw new Error('Invalid channel authority fingerprint.');
  return value;
}
function url(value: unknown): string {
  const source = text(value, 2048), parsed = new URL(source);
  if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password || parsed.search || parsed.hash) throw new Error('Invalid channel authority URL.');
  return source;
}
function head(value: Record<string, unknown>): { head_seq: number; head_hash: string; initialized: boolean } {
  const head_seq = counter(value.head_seq), initialized = flag(value.initialized);
  const head_hash = head_seq === 0 && value.head_hash === '' ? '' : hash(value.head_hash);
  if (initialized !== (head_seq > 0)) throw new Error('The authority history has not been initialized consistently.');
  return { head_seq, head_hash, initialized };
}
export function parseAuthorityPeer(value: unknown): AuthorityPeer {
  const peer = record(value);
  return { peer_id: counter(peer.peer_id, 1), label: text(peer.label), base_url: url(peer.base_url), authority_identity: fingerprint(peer.authority_identity) };
}
export function parseAuthorityCatalog(value: unknown, peer: AuthorityPeer, after?: string): AuthorityCatalog {
  const source = record(value), returnedPeer = parseAuthorityPeer(source.peer);
  if (returnedPeer.peer_id !== peer.peer_id || returnedPeer.authority_identity !== peer.authority_identity || returnedPeer.base_url !== peer.base_url) throw new Error('The selected partner identity changed. Refresh the partner list.');
  if (!Array.isArray(source.channels) || source.channels.length > 100) throw new Error('Invalid federated channel page.');
  const channels = source.channels.map((entry: unknown): AuthorityChannel => {
    const channel = record(entry), channel_type = text(channel.channel_type);
    if (!['Text', 'Voice', 'Hybrid', 'Agent', 'Broadcast'].includes(channel_type)) throw new Error('Unsupported federated channel type.');
    return { channel_id: text(channel.channel_id, 128), name: text(channel.name), channel_type: channel_type as ChannelType,
      e2e_enabled: flag(channel.e2e_enabled), ...head(channel) };
  });
  if (new Set(channels.map(channel => channel.channel_id)).size !== channels.length) throw new Error('Duplicate federated channel identity.');
  const next = source.next_after_channel_id === null ? null : text(source.next_after_channel_id, 128);
  if (next !== null && (next === after || next !== channels.at(-1)?.channel_id)) throw new Error('The federated channel page did not advance.');
  return { peer: returnedPeer, channels, next_after_channel_id: next };
}
export function parseChannelAuthority(value: unknown, channelId: string): ChannelAuthority {
  const source = record(value);
  if (source.channel_id !== channelId) throw new Error('The server returned another channel authority.');
  return { channel_id: channelId, authority_identity: fingerprint(source.authority_identity), authority_server: url(source.authority_server),
    local_authority: flag(source.local_authority), ...head(source) };
}
function caller(pseudonym: string) {
  const context = getCredentialContext();
  const check = () => { if (!isCredentialContextCurrent(context) || context.pseudonymId !== pseudonym) throw new StaleCredentialContextError('Channel authority action'); };
  check();
  return { check, async call(path: string, options?: RequestInit): Promise<unknown> {
    check(); const result = await request<unknown>(path, { ...options, headers: authHeaders(pseudonym) }); check(); return result;
  } };
}
export async function listAuthorityPeers(pseudonym: string): Promise<AuthorityPeer[]> {
  const source = record(await caller(pseudonym).call('/api/federation/authority-peers'));
  if (!Array.isArray(source.peers) || source.peers.length > 10000) throw new Error('Invalid federation partner list.');
  const peers = source.peers.map(parseAuthorityPeer);
  if (new Set(peers.map(peer => peer.peer_id)).size !== peers.length) throw new Error('Duplicate federation partner.');
  return peers;
}
export async function listAuthorityChannels(pseudonym: string, peer: AuthorityPeer, after?: string): Promise<AuthorityCatalog> {
  const query = new URLSearchParams({ limit: '50' }); if (after) query.set('after_channel_id', after);
  return parseAuthorityCatalog(await caller(pseudonym).call(`/api/federation/authority-peers/${peer.peer_id}/channels?${query}`), peer, after);
}
export async function getChannelAuthority(pseudonym: string, channelId: string): Promise<ChannelAuthority> {
  return parseChannelAuthority(await caller(pseudonym).call(`/api/channels/${encodeURIComponent(channelId)}/authority`), channelId);
}
export async function initializeChannelAuthority(pseudonym: string, expected: ChannelAuthority): Promise<ChannelAuthority> {
  if (!expected.local_authority || expected.initialized) throw new Error('This channel cannot be initialized as a new local authority.');
  const operation = caller(pseudonym);
  const result = record(await operation.call(`/api/channels/${encodeURIComponent(expected.channel_id)}/authority/initialize`, {
    method: 'POST', body: JSON.stringify({ expected_authority_identity: expected.authority_identity }),
  }));
  if (result.status !== 'ok') throw new Error('Channel authority initialization was not confirmed.');
  const observed = await getChannelAuthority(pseudonym, expected.channel_id); operation.check();
  if (!observed.local_authority || !observed.initialized || observed.authority_identity !== expected.authority_identity) throw new Error('The initialized channel differs from the confirmed authority.');
  return observed;
}
export async function attachAuthorityChannel(pseudonym: string, peer: AuthorityPeer, channel: AuthorityChannel): Promise<void> {
  if (!channel.initialized) throw new Error('The partner must initialize this channel before it can be joined.');
  const operation = caller(pseudonym);
  const device = await getE2eControls(pseudonym).ensureCertifiedDevice(); operation.check();
  const result = record(await operation.call(`/api/federation/channels/${encodeURIComponent(channel.channel_id)}/attach`, {
    method: 'POST', body: JSON.stringify({ peer_id: peer.peer_id, authority_identity: peer.authority_identity, device_id: device.device_id }),
  }));
  const accepted = counter(result.accepted_seq, 1), acceptedHash = hash(result.accepted_hash);
  text(result.operation_id, 128);
  const event = record(result.event), header = record(event.header), payload = record(event.payload);
  if (event.record_type !== 'present' || header.channel_id !== channel.channel_id || header.authority_identity !== peer.authority_identity
    || header.event_kind !== 'state_changed' || header.seq !== accepted || event.event_hash !== acceptedHash
    || payload.action !== 'member_joined' || payload.pseudonym_id !== pseudonym || payload.requesting_identity !== device.server_identity) {
    throw new Error('The authority did not confirm this identity’s channel membership.');
  }
}

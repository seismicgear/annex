import { authHeaders, getCredentialContext, isCredentialContextCurrent, request, StaleCredentialContextError } from './core';
import { getE2eControls } from '@/lib/e2e-store';
import type { AuthorityChannel, AuthorityPeer } from './channel-authority';

export interface ReconciliationPreview {
  channel_id: string; snapshot_sha256: string; messages: number; edits: number; attachments: number; key_records: number;
}
export interface ReconciliationRequest {
  peer_id: number; authority_identity: string; device_id: string; operation_id: string; expected_snapshot_sha256: string;
}
export interface ReconciliationResult {
  operation_id: string; status: 'pending' | 'conflict' | 'complete' | 'cancelled'; remote_join_accepted: boolean;
  after_seq: number; head_seq: number; head_hash: string; retained_messages: number; removed_messages: number;
  conflicts: Array<{ kind: string; id: string }>;
}
export interface ReconciliationOperation extends ReconciliationRequest, ReconciliationResult { channel_id: string }

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid reconciliation response.');
  return value as Record<string, unknown>;
}
function text(value: unknown, max = 512): string {
  if (typeof value !== 'string' || !value || value.length > max || [...value].some(character => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127)) throw new Error('Invalid reconciliation text.');
  return value;
}
function count(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) throw new Error('Invalid reconciliation count.');
  return value;
}
function hash(value: unknown): string {
  if (typeof value !== 'string' || !/^[0-9a-f]{64}$/.test(value)) throw new Error('Invalid reconciliation fingerprint.');
  return value;
}
function operationId(value: unknown): string {
  return text(value, 128);
}
function fingerprint(value: unknown): string {
  if (typeof value !== 'string' || !/^annex-server:[0-9a-f]{64}$/.test(value)) throw new Error('Invalid reconciliation authority.');
  return value;
}
export function parseReconciliationPreview(value: unknown, channelId: string): ReconciliationPreview {
  const input = object(value);
  if (input.channel_id !== channelId) throw new Error('The preview describes another channel.');
  return { channel_id: channelId, snapshot_sha256: hash(input.snapshot_sha256), messages: count(input.messages),
    edits: count(input.edits), attachments: count(input.attachments), key_records: count(input.key_records) };
}
export function parseReconciliationResult(value: unknown, expectedOperationId: string): ReconciliationResult {
  const input = object(value), operation_id = operationId(input.operation_id);
  if (operation_id !== expectedOperationId) throw new Error('The server acknowledged another reconciliation operation.');
  if (!['pending', 'conflict', 'complete', 'cancelled'].includes(String(input.status)) || typeof input.remote_join_accepted !== 'boolean'
    || !Array.isArray(input.conflicts) || input.conflicts.length > 128) throw new Error('Invalid reconciliation progress.');
  const status = input.status as ReconciliationResult['status'], after_seq = count(input.after_seq), head_seq = count(input.head_seq);
  const head_hash = head_seq === 0 && input.head_hash === '' ? '' : hash(input.head_hash);
  if (after_seq > head_seq) throw new Error('Reconciliation advanced past its signed history.');
  const conflicts = input.conflicts.map(value => { const entry = object(value); return { kind: text(entry.kind, 128), id: text(entry.id) }; });
  if (status === 'complete' && (!input.remote_join_accepted || conflicts.length || head_seq === 0 || after_seq !== head_seq)) throw new Error('The reconciliation is not complete.');
  return { operation_id, status, remote_join_accepted: input.remote_join_accepted, after_seq, head_seq, head_hash,
    retained_messages: count(input.retained_messages), removed_messages: count(input.removed_messages), conflicts };
}
function parseRequest(value: unknown): ReconciliationRequest {
  const input = object(value), peer_id = count(input.peer_id);
  if (peer_id === 0) throw new Error('Invalid reconciliation partner.');
  return { peer_id, authority_identity: fingerprint(input.authority_identity), device_id: hash(input.device_id),
    operation_id: operationId(input.operation_id), expected_snapshot_sha256: hash(input.expected_snapshot_sha256) };
}
function caller(pseudonym: string) {
  const context = getCredentialContext();
  const check = () => { if (context.pseudonymId !== pseudonym || !isCredentialContextCurrent(context)) throw new StaleCredentialContextError('Channel reconciliation'); };
  check();
  return { check, async call(path: string, options?: RequestInit): Promise<unknown> {
    check(); const result = await request<unknown>(path, { ...options, headers: authHeaders(pseudonym),
      signal: options?.signal ?? AbortSignal.timeout(options?.method === 'POST' ? 120_000 : 60_000),
    }); check(); return result;
  } };
}
const endpoint = (channel: string) => `/api/channels/${encodeURIComponent(channel)}/authority`;
export async function getReconciliationPreview(pseudonym: string, channel: string): Promise<ReconciliationPreview> {
  const operation = caller(pseudonym);
  // A first device publication may advance a joined channel's key-directory
  // revision. Complete it before capturing the immutable local snapshot.
  await getE2eControls(pseudonym).ensureCertifiedDevice(); operation.check();
  return parseReconciliationPreview(await operation.call(`${endpoint(channel)}/reconciliation-preview`), channel);
}
export async function listReconciliations(pseudonym: string, channel: string): Promise<ReconciliationOperation[]> {
  const input = object(await caller(pseudonym).call(`${endpoint(channel)}/reconciliations`));
  if (!Array.isArray(input.operations) || input.operations.length > 64) throw new Error('Invalid reconciliation operation list.');
  const operations = input.operations.map(value => {
    const source = object(value), request = parseRequest(source.request);
    return { ...request, ...parseReconciliationResult(source.result, request.operation_id), channel_id: channel };
  });
  if (new Set(operations.map(value => value.operation_id)).size !== operations.length) throw new Error('Duplicate reconciliation operation.');
  return operations;
}
export async function prepareReconciliation(pseudonym: string, preview: ReconciliationPreview, peer: AuthorityPeer, channel: AuthorityChannel): Promise<ReconciliationRequest> {
  const operation = caller(pseudonym);
  parseReconciliationPreview(preview, channel.channel_id);
  if (!channel.initialized) throw new Error('The partner must initialize this channel before reconciliation.');
  const device = await getE2eControls(pseudonym).ensureCertifiedDevice(); operation.check();
  return parseRequest({ peer_id: peer.peer_id, authority_identity: peer.authority_identity, device_id: device.device_id,
    operation_id: crypto.randomUUID(), expected_snapshot_sha256: preview.snapshot_sha256 });
}
export async function reconcileChannel(pseudonym: string, channel: string, intent: ReconciliationRequest): Promise<ReconciliationResult> {
  const operation = caller(pseudonym), body = parseRequest(intent);
  const device = await getE2eControls(pseudonym).ensureCertifiedDevice(); operation.check();
  if (device.device_id !== body.device_id) throw new Error('This operation belongs to another encryption device. Resume from the original device.');
  return parseReconciliationResult(await operation.call(`${endpoint(channel)}/reconcile`, { method: 'POST', body: JSON.stringify(body) }), body.operation_id);
}
export async function cancelReconciliation(pseudonym: string, channel: string, id: string): Promise<ReconciliationResult> {
  const result = parseReconciliationResult(await caller(pseudonym).call(`${endpoint(channel)}/reconciliations/${encodeURIComponent(operationId(id))}`, { method: 'DELETE' }), id);
  if (result.status !== 'cancelled') throw new Error('The server did not confirm cancellation of the remaining reconciliation.');
  return result;
}

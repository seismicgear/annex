import { beforeEach, describe, expect, it, vi } from 'vitest';
const fake = vi.hoisted(() => ({ request: vi.fn(), certified: vi.fn(), serial: 1 }));
vi.mock('./core', () => ({ request: fake.request, authHeaders: () => ({ Authorization: 'Bearer current' }),
  getCredentialContext: () => ({ pseudonymId: 'moderator', serial: fake.serial }),
  isCredentialContextCurrent: (value: { serial: number }) => value.serial === fake.serial,
  StaleCredentialContextError: class extends Error {},
}));
vi.mock('@/lib/e2e-store', () => ({ getE2eControls: () => ({ ensureCertifiedDevice: fake.certified }) }));
import { cancelReconciliation, getReconciliationPreview, listReconciliations, parseReconciliationResult, prepareReconciliation, reconcileChannel } from './authority-reconciliation';
const id = 'ee8c9fda-3939-46fa-9083-2dc8d4b4a0dc';
const peer = { peer_id: 7, label: 'Remote', base_url: 'https://remote.example', authority_identity: `annex-server:${'a'.repeat(64)}` };
const channel = { channel_id: 'shared', name: 'Shared', channel_type: 'Text' as const, initialized: true, e2e_enabled: false, head_seq: 4, head_hash: 'b'.repeat(64) };
const preview = { channel_id: 'shared', snapshot_sha256: 'd'.repeat(64), messages: 5, edits: 2, attachments: 3, key_records: 7 };
const intent = { peer_id: 7, authority_identity: peer.authority_identity, device_id: 'c'.repeat(64), operation_id: id, expected_snapshot_sha256: preview.snapshot_sha256 };
const pending = { operation_id: id, status: 'pending', remote_join_accepted: true, after_seq: 2, head_seq: 4, head_hash: 'b'.repeat(64), retained_messages: 0, removed_messages: 0, conflicts: [] };
beforeEach(() => { vi.clearAllMocks(); fake.serial = 1; fake.certified.mockResolvedValue({ device_id: intent.device_id }); });
describe('retained-history reconciliation API', () => {
  it('captures and validates the local snapshot after device verification, without submitting reconciliation', async () => {
    fake.request.mockResolvedValueOnce(preview).mockResolvedValueOnce({ ...preview, channel_id: 'another' }).mockResolvedValueOnce({ ...preview, attachments: -1 });
    await expect(getReconciliationPreview('moderator', 'shared')).resolves.toEqual(preview);
    await expect(getReconciliationPreview('moderator', 'shared')).rejects.toThrow('another channel');
    await expect(getReconciliationPreview('moderator', 'shared')).rejects.toThrow('count');
    expect(fake.certified).toHaveBeenCalledTimes(3);
    expect(fake.request.mock.calls.every(([, options]) => !options.method)).toBe(true);
  });
  it('prepares the exact confirmed fingerprint and snapshot with a genuine current device', async () => {
    const prepared = await prepareReconciliation('moderator', preview, peer, channel);
    expect(prepared).toEqual({ ...intent, operation_id: expect.stringMatching(/^[a-f0-9-]{36}$/) });
    expect(fake.request).not.toHaveBeenCalled();
    await expect(prepareReconciliation('moderator', preview, peer, { ...channel, initialized: false })).rejects.toThrow('initialize');
    await expect(prepareReconciliation('moderator', preview, peer, { ...channel, channel_id: 'another' })).rejects.toThrow('another channel');
    expect(fake.certified).toHaveBeenCalledTimes(1);
  });
  it('replays the same durable operation after a lost acknowledgement rather than replacing its snapshot or ID', async () => {
    fake.request.mockRejectedValueOnce(new Error('Connection lost')).mockResolvedValueOnce(pending);
    await expect(reconcileChannel('moderator', 'shared', intent)).rejects.toThrow('Connection lost');
    await expect(reconcileChannel('moderator', 'shared', intent)).resolves.toEqual(pending);
    expect(fake.request.mock.calls).toEqual(Array.from({ length: 2 }, () => ['/api/channels/shared/authority/reconcile', {
      method: 'POST', headers: { Authorization: 'Bearer current' }, body: JSON.stringify(intent), signal: expect.any(AbortSignal),
    }]));
  });
  it('never silently substitutes a different linked device while resuming', async () => {
    fake.certified.mockResolvedValue({ device_id: 'e'.repeat(64) });
    await expect(reconcileChannel('moderator', 'shared', intent)).rejects.toThrow('original device');
    expect(fake.request).not.toHaveBeenCalled();
  });
  it('rejects stale enrollment and late responses after the identity or server changes', async () => {
    fake.certified.mockImplementationOnce(async () => { fake.serial++; return { device_id: intent.device_id }; });
    await expect(prepareReconciliation('moderator', preview, peer, channel)).rejects.toThrow();
    expect(fake.request).not.toHaveBeenCalled();
    fake.request.mockImplementationOnce(async () => { fake.serial++; return pending; });
    await expect(reconcileChannel('moderator', 'shared', intent)).rejects.toThrow();
  });
  it('restores the server-owned nested request/result without reconstructing a new request', async () => {
    fake.request.mockResolvedValue({ operations: [{ request: intent, result: pending }] });
    await expect(listReconciliations('moderator', 'shared')).resolves.toEqual([{ ...intent, ...pending, channel_id: 'shared' }]);
    expect(fake.certified).not.toHaveBeenCalled();
    fake.request.mockResolvedValue({ operations: [{ request: intent, result: { ...pending, operation_id: 'd669f329-c578-419d-a6ac-3ac949a81d1f' } }] });
    await expect(listReconciliations('moderator', 'shared')).rejects.toThrow('another reconciliation');
  });
  it('does not accept impossible completion, contradictory history or fabricated progress', () => {
    for (const value of [
      { ...pending, status: 'complete' }, { ...pending, after_seq: 5 }, { ...pending, status: 'unknown' },
      { ...pending, status: 'complete', after_seq: 4, remote_join_accepted: false },
      { ...pending, status: 'complete', after_seq: 4, conflicts: [{ kind: 'message', id: 'retained' }] },
    ]) expect(() => parseReconciliationResult(value, id)).toThrow();
    expect(parseReconciliationResult({ ...pending, status: 'complete', after_seq: 4, retained_messages: 5 }, id)).toMatchObject({ status: 'complete', retained_messages: 5 });
  });
  it('cancels only staging and retains the explicitly acknowledged remote Join', async () => {
    fake.request.mockResolvedValueOnce({ ...pending, status: 'cancelled' }).mockResolvedValueOnce(pending);
    await expect(cancelReconciliation('moderator', 'shared', id)).resolves.toMatchObject({ status: 'cancelled', remote_join_accepted: true });
    expect(fake.request).toHaveBeenCalledWith(`/api/channels/shared/authority/reconciliations/${id}`, { method: 'DELETE', headers: { Authorization: 'Bearer current' }, signal: expect.any(AbortSignal) });
    await expect(cancelReconciliation('moderator', 'shared', id)).rejects.toThrow('did not confirm cancellation');
    expect(fake.certified).not.toHaveBeenCalled();
  });
});

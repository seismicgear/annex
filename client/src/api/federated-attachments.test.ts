import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
const mocks = vi.hoisted(() => ({ device: vi.fn(), grant: vi.fn(), request: vi.fn(), serial: 1 }));
vi.mock('@/lib/e2e-store', () => ({ getE2eControls: () => ({ ensureCertifiedDevice: mocks.device }) }));
vi.mock('./encrypted-uploads', () => ({ encryptedUploadGrant: mocks.grant }));
vi.mock('./core', () => ({
  authHeaders: () => ({ Authorization: 'Bearer original' }), request: mocks.request,
  getCredentialContext: () => ({ serial: mocks.serial, pseudonymId: 'reader', baseUrl: 'https://replica.example' }),
  isCredentialContextCurrent: (context: { serial: number }) => context.serial === mocks.serial,
  StaleCredentialContextError: class extends Error {},
  throwApiError: async (response: Response) => { throw new Error(`Actual fetch refused ${response.status}`); },
}));
import { parseOrdinaryAttachment, readOrdinaryFederatedAttachment, retryOrdinaryFederatedAttachment } from './federated-attachments';
const id = 'a8db7399-913d-4d62-9b19-149d327021fa';
const path = `/uploads/chat/files/${id}.txt`;
beforeEach(() => { vi.clearAllMocks(); mocks.serial = 1; mocks.device.mockResolvedValue({ device_id: 'c'.repeat(64) }); mocks.grant.mockResolvedValue('device-bound-grant'); vi.stubGlobal('fetch', vi.fn()); });
afterEach(() => vi.unstubAllGlobals());
describe('ordinary federated attachment access', () => {
  it.each([1, 7, 27])('preserves %d plaintext bytes without encrypted overhead or a nonce minimum', async size => {
    const bytes = new Uint8Array(size).fill(97);
    vi.mocked(fetch).mockResolvedValue(new Response(bytes, { headers: { 'Content-Length': String(size), 'Content-Type': 'text/plain' } }));
    const blob = await readOrdinaryFederatedAttachment('reader', path, 'file', new AbortController().signal);
    expect(blob.size).toBe(size); expect(blob.type).toBe('application/octet-stream');
    expect(mocks.grant).toHaveBeenCalledWith('reader', 'c'.repeat(64));
    expect(fetch).toHaveBeenCalledWith(`https://replica.example${path}?t=device-bound-grant`, expect.objectContaining({ redirect: 'error' }));
  });
  it('refuses empty bytes because no accepted ordinary upload can be empty', async () => {
    for (const headers of [new Headers({ 'Content-Length': '0' }), new Headers()]) {
      vi.mocked(fetch).mockResolvedValue(new Response(new Uint8Array(), { headers }));
      await expect(readOrdinaryFederatedAttachment('reader', path, 'file', new AbortController().signal)).rejects.toThrow();
    }
  });
  it.each(['https://attacker.example/uploads/chat/files/abc.txt', `${path}?t=stolen`, `${path}#fragment`, `/uploads/chat/files/../${id}.txt`, `/uploads/chat/images/${id}.bin`])('refuses noncanonical path %s before publishing a device', async value => {
    await expect(readOrdinaryFederatedAttachment('reader', value, 'file', new AbortController().signal)).rejects.toThrow();
    expect(mocks.device).not.toHaveBeenCalled(); expect(fetch).not.toHaveBeenCalled();
  });
  it('does not downgrade a refused device grant to an anonymous/v1 fetch', async () => {
    mocks.grant.mockRejectedValue(new Error('Device revoked'));
    await expect(readOrdinaryFederatedAttachment('reader', path, 'file', new AbortController().signal)).rejects.toThrow('Device revoked');
    expect(fetch).not.toHaveBeenCalled();
  });
  it('does not send the grant after switching servers during enrollment', async () => {
    mocks.device.mockImplementation(async () => { mocks.serial++; return { device_id: 'c'.repeat(64) }; });
    await expect(readOrdinaryFederatedAttachment('reader', path, 'file', new AbortController().signal)).rejects.toThrow();
    expect(mocks.grant).not.toHaveBeenCalled(); expect(fetch).not.toHaveBeenCalled();
  });
  it('rejects incomplete and forged length responses', async () => {
    for (const length of ['5', '1', '52428801', 'bad']) {
      vi.mocked(fetch).mockResolvedValue(new Response(new Uint8Array([1, 2]), { headers: { 'Content-Length': length } }));
      await expect(readOrdinaryFederatedAttachment('reader', path, 'file', new AbortController().signal)).rejects.toThrow();
    }
  });
  it('does not render an image response as a different active content type', async () => {
    vi.mocked(fetch).mockResolvedValue(new Response(new Uint8Array([1]), { headers: { 'Content-Type': 'text/html' } }));
    await expect(readOrdinaryFederatedAttachment('reader', `/uploads/chat/images/${id}.png`, 'image', new AbortController().signal)).rejects.toThrow('media type');
  });
  it('reauthorizes retry using this actual certified device and requires a matching acknowledgement', async () => {
    mocks.request.mockResolvedValueOnce({ status: 'pending', attachment_id: id }).mockResolvedValueOnce({ status: 'ready', attachment_id: 'different' });
    await retryOrdinaryFederatedAttachment('reader', 'shared', id);
    expect(mocks.request).toHaveBeenCalledWith(`/api/channels/shared/uploads/${id}/retry`, {
      method: 'POST', headers: { Authorization: 'Bearer original' }, body: JSON.stringify({ device_id: 'c'.repeat(64) }),
    });
    await expect(retryOrdinaryFederatedAttachment('reader', 'shared', id)).rejects.toThrow('did not confirm');
    expect(parseOrdinaryAttachment(path, 'file').id).toBe(id);
  });
});

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getServerSummary, register, setApiBaseUrl, verifyMembership } from '@/lib/api';

function okJsonResponse(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as Response;
}

describe('request header behavior', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setApiBaseUrl('');
    global.fetch = vi.fn();
  });

  it('does not send JSON Content-Type by default for GET /api/public/server/summary', async () => {
    vi.mocked(global.fetch).mockResolvedValue(
      okJsonResponse({
        slug: 'default',
        label: 'Default',
        members_by_type: {},
        total_active_members: 0,
        channel_count: 0,
        federation_peer_count: 0,
        active_agent_count: 0,
      }),
    );

    await getServerSummary();

    expect(global.fetch).toHaveBeenCalledWith(
      '/api/public/server/summary',
      expect.objectContaining({
        headers: expect.any(Headers),
      }),
    );

    const [, init] = vi.mocked(global.fetch).mock.calls[0];
    const headers = init?.headers as Headers;
    expect(headers.has('Content-Type')).toBe(false);
  });

  it('sends JSON Content-Type for POST register and verifyMembership', async () => {
    vi.mocked(global.fetch)
      .mockResolvedValueOnce(
        okJsonResponse({
          identityId: 1,
          leafIndex: 0,
          rootHex: '0xabc',
          pathElements: ['0x1'],
          pathIndexBits: [0],
        }),
      )
      .mockResolvedValueOnce(
        okJsonResponse({
          ok: true,
          pseudonymId: 'pseudo-123',
        }),
      );

    await register('0xdeadbeef', 2, 99);
    await verifyMembership('0xroot', '0xcommitment', 'annex:test:v1', {}, ['1']);

    const registerInit = vi.mocked(global.fetch).mock.calls[0][1];
    const verifyInit = vi.mocked(global.fetch).mock.calls[1][1];

    const registerHeaders = registerInit?.headers as Headers;
    const verifyHeaders = verifyInit?.headers as Headers;

    expect(registerHeaders.get('Content-Type')).toBe('application/json');
    expect(verifyHeaders.get('Content-Type')).toBe('application/json');
  });
});

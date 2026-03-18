import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getServerSummary, register, setApiBaseUrl, verifyMembership, joinVoice, leaveVoice, getVoiceStatus, setSessionToken } from '@/lib/api';

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

describe('voice endpoints use _apiBaseUrl', () => {
  const REMOTE_URL = 'https://remote.annex.example';

  beforeEach(() => {
    vi.clearAllMocks();
    setApiBaseUrl(REMOTE_URL);
    setSessionToken(null);
    global.fetch = vi.fn();
  });

  afterEach(() => {
    setApiBaseUrl('');
  });

  it('joinVoice targets the remote host', async () => {
    vi.mocked(global.fetch).mockResolvedValue(
      okJsonResponse({ token: 'tok', url: 'wss://lk', ice_servers: [] }),
    );

    await joinVoice('pseudo-1', 'chan-1');

    const url = vi.mocked(global.fetch).mock.calls[0][0] as string;
    expect(url).toBe(`${REMOTE_URL}/api/channels/chan-1/voice/join`);
  });

  it('leaveVoice targets the remote host', async () => {
    vi.mocked(global.fetch).mockResolvedValue(
      okJsonResponse({}),
    );

    await leaveVoice('pseudo-1', 'chan-1');

    const url = vi.mocked(global.fetch).mock.calls[0][0] as string;
    expect(url).toBe(`${REMOTE_URL}/api/channels/chan-1/voice/leave`);
  });

  it('getVoiceStatus targets the remote host', async () => {
    vi.mocked(global.fetch).mockResolvedValue(
      okJsonResponse({ participants: 2, active: true }),
    );

    await getVoiceStatus('pseudo-1', 'chan-1');

    const url = vi.mocked(global.fetch).mock.calls[0][0] as string;
    expect(url).toBe(`${REMOTE_URL}/api/channels/chan-1/voice/status`);
  });
});

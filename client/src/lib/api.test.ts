import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getServerSummary, register, setApiBaseUrl, verifyMembership, joinVoice, leaveVoice, getVoiceStatus, setSessionToken, setZkProofPayload, joinChannel } from '@/lib/api';
import { extractErrorMessage } from '@/api/core';

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

describe('x-annex-zk-proof header (server contract)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setApiBaseUrl('');
    global.fetch = vi.fn();
  });

  afterEach(() => {
    setSessionToken(null);
    setZkProofPayload(null);
    setApiBaseUrl('');
  });

  it('joinChannel sends a base64-encoded full ZkProofPayload the server can decode', async () => {
    setSessionToken('sess-token');
    // Shape MUST match the server's ZkProofPayload (proof + root_hex +
    // commitment_hex required). This is what verify_zk_membership_header
    // base64-decodes and deserializes.
    const payload = JSON.stringify({
      proof: { pi_a: ['1', '2', '3'] },
      root_hex: '0xroot',
      commitment_hex: '0xcommit',
      protocolVersion: 'v1',
      publicSignals: ['1', '2'],
    });
    setZkProofPayload(payload);

    vi.mocked(global.fetch).mockResolvedValue(okJsonResponse({ status: 'joined' }));
    await joinChannel('pseudo-1', 'chan-1');

    const init = vi.mocked(global.fetch).mock.calls[0][1];
    const headers = init?.headers as Headers;
    const headerVal = headers.get('x-annex-zk-proof');
    expect(headerVal).toBeTruthy();
    // Regression: the header must be base64, NOT raw JSON (raw starts with '{').
    expect(headerVal!.startsWith('{')).toBe(false);
    // Base64-decoding must yield the full payload with the required fields.
    const decoded = JSON.parse(atob(headerVal!)) as Record<string, unknown>;
    expect(decoded.root_hex).toBe('0xroot');
    expect(decoded.commitment_hex).toBe('0xcommit');
    expect(decoded.proof).toBeTruthy();
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

describe('extractErrorMessage', () => {
  it('unwraps the {"error": ...} shape most handlers return', () => {
    expect(
      extractErrorMessage(409, '{"error":"nullifier already exists for topic \'annex:server:x:v2\'"}'),
    ).toBe("nullifier already exists for topic 'annex:server:x:v2'");
  });

  it('prefers "message" over the machine-readable "error" code', () => {
    // The voice-join handler returns both: `error` is a code
    // (`voice_disabled`), `message` is the sentence meant for a person.
    expect(
      extractErrorMessage(403, '{"error":"voice_disabled","message":"Voice is disabled on this server."}'),
    ).toBe('Voice is disabled on this server.');
  });

  it('falls back to status wording when the body is empty', () => {
    // Every route in api_channels.rs returns a bare status with no body,
    // which previously surfaced to users as an empty error message.
    expect(extractErrorMessage(403, '')).toBe('You do not have permission to do that.');
    expect(extractErrorMessage(404, '   ')).toBe('That item no longer exists.');
  });

  it('passes plain-text bodies through unchanged', () => {
    expect(extractErrorMessage(500, 'internal server error')).toBe('internal server error');
  });

  it('describes unmapped statuses rather than returning nothing', () => {
    expect(extractErrorMessage(418, '')).toBe('Request failed (HTTP 418).');
  });
});

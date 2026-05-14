// api/signal.test.mjs — node:test suite for the federation signaling relay.
//
// Exercises the real handler exported by signal.js with a minimal Vercel-
// shaped request/response mock. Covers:
//   - structural validation (slug format, sdp_type, size limits, freshness)
//   - method allowlist
//   - production profile gate: unsigned requests rejected
//   - signed envelope round-trip: accepted, queued, drainable via GET
//   - GET drain validation: slug format
//   - per-IP rate limiting on POST and GET
//
// Run from the repo root:
//   node --test api/signal.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  generateKeyPairSync,
  sign as cryptoSign,
} from 'node:crypto';

import handler, { __test } from './signal.js';

// ─── Helpers ──────────────────────────────────────────────────────────────

function mockReq({ method = 'POST', body = undefined, query = {}, ip = '198.51.100.1', headers = {} } = {}) {
  return {
    method,
    body,
    query,
    headers: { 'x-forwarded-for': ip, ...headers },
    socket: { remoteAddress: ip },
  };
}

function mockRes() {
  const res = {
    statusCode: 200,
    body: undefined,
    headers: {},
    ended: false,
    status(code) {
      this.statusCode = code;
      return this;
    },
    setHeader(name, value) {
      this.headers[name.toLowerCase()] = String(value);
    },
    json(payload) {
      this.body = payload;
      this.ended = true;
    },
    end() {
      this.ended = true;
    },
  };
  return res;
}

// Generate a fresh Ed25519 keypair and return { rawPubHex, sign(canonicalString) }
function freshSigner() {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  // Extract the raw 32-byte pubkey from the SPKI export (last 32 bytes).
  const spki = publicKey.export({ format: 'der', type: 'spki' });
  const rawPub = spki.subarray(spki.length - 32);
  return {
    rawPubHex: rawPub.toString('hex'),
    sign(canonicalString) {
      return cryptoSign(null, Buffer.from(canonicalString, 'utf-8'), privateKey).toString('base64');
    },
  };
}

function canonicalEnvelope(p) {
  return `${p.from_server_slug}|${p.to_server_slug}|${p.session_id}|${p.sdp_type}|${p.sdp}|${p.sent_at_ms}|${p.from_pubkey_hex}`;
}

const VALID_FROM = 'a1b2c3d4e5f6';
const VALID_TO = '0102030405ab';

function basePayload({ from = VALID_FROM, to = VALID_TO, sdp = 'v=0\r\n' } = {}) {
  return {
    from_server_slug: from,
    to_server_slug: to,
    session_id: 'sess-123',
    sdp_type: 'offer',
    sdp,
    sent_at_ms: Date.now(),
  };
}

function signedPayload(signer, base = basePayload()) {
  const p = { ...base, from_pubkey_hex: signer.rawPubHex };
  p.vrp_signature = signer.sign(canonicalEnvelope(p));
  return p;
}

// Snapshot + restore ANNEX_BUILD_PROFILE around each test.
function withProfile(profile, fn) {
  const prev = process.env.ANNEX_BUILD_PROFILE;
  const prevNodeEnv = process.env.NODE_ENV;
  if (profile === null) {
    delete process.env.ANNEX_BUILD_PROFILE;
    delete process.env.NODE_ENV;
  } else {
    process.env.ANNEX_BUILD_PROFILE = profile;
    delete process.env.NODE_ENV;
  }
  try {
    return fn();
  } finally {
    if (prev === undefined) delete process.env.ANNEX_BUILD_PROFILE;
    else process.env.ANNEX_BUILD_PROFILE = prev;
    if (prevNodeEnv === undefined) delete process.env.NODE_ENV;
    else process.env.NODE_ENV = prevNodeEnv;
  }
}

// ─── Structural validation ────────────────────────────────────────────────

test('POST rejects body that is not a JSON object', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    // A bare number is the simplest way to exercise the "not an object"
    // branch — `null`/`undefined` get normalised to `{}` by the handler
    // before the validator sees them.
    await handler(mockReq({ body: 42, ip: '198.51.100.10' }), res);
    assert.equal(res.statusCode, 400);
    assert.match(res.body?.error || '', /body must be a JSON object/i);
  });
});

test('POST rejects missing required fields', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    await handler(mockReq({ body: { foo: 'bar' }, ip: '198.51.100.11' }), res);
    assert.equal(res.statusCode, 400);
    assert.match(res.body?.error || '', /missing required signaling fields/i);
  });
});

test('POST rejects malformed from_server_slug', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    const body = { ...basePayload(), from_server_slug: 'not-a-slug!' };
    await handler(mockReq({ body, ip: '198.51.100.12' }), res);
    assert.equal(res.statusCode, 400);
    assert.match(res.body?.error || '', /invalid from_server_slug/i);
  });
});

test('POST rejects malformed to_server_slug', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    const body = { ...basePayload(), to_server_slug: 'TOOLONGSLUG12345' };
    await handler(mockReq({ body, ip: '198.51.100.13' }), res);
    assert.equal(res.statusCode, 400);
    assert.match(res.body?.error || '', /invalid to_server_slug/i);
  });
});

test('POST rejects invalid sdp_type', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    const body = { ...basePayload(), sdp_type: 'rollback' };
    await handler(mockReq({ body, ip: '198.51.100.14' }), res);
    assert.equal(res.statusCode, 400);
    assert.match(res.body?.error || '', /invalid sdp_type/i);
  });
});

test('POST rejects oversized SDP body with 413', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    // 40 KiB SDP — exceeds MAX_SDP_BYTES (30 KiB) but under MAX_BODY_BYTES (50 KiB)
    const body = { ...basePayload(), sdp: 'x'.repeat(40 * 1024) };
    await handler(mockReq({ body, ip: '198.51.100.15' }), res);
    assert.equal(res.statusCode, 413);
    assert.match(res.body?.error || '', /sdp too large/i);
  });
});

test('POST rejects total body exceeding MAX_BODY_BYTES with 413', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    // 60 KiB SDP — exceeds total body cap before per-field check fires.
    // (Per-field SDP cap also catches this, but the total-body branch is
    // what protects against many small fields adding up.)
    const body = { ...basePayload(), sdp: 'x'.repeat(60 * 1024) };
    await handler(mockReq({ body, ip: '198.51.100.16' }), res);
    assert.equal(res.statusCode, 413);
  });
});

test('POST rejects stale sent_at_ms outside freshness window', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    const body = { ...basePayload(), sent_at_ms: Date.now() - 5 * 60_000 };
    await handler(mockReq({ body, ip: '198.51.100.17' }), res);
    assert.equal(res.statusCode, 400);
    assert.match(res.body?.error || '', /freshness window/i);
  });
});

test('POST rejects invalid session_id (special chars)', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    const body = { ...basePayload(), session_id: 'sess id with spaces' };
    await handler(mockReq({ body, ip: '198.51.100.18' }), res);
    assert.equal(res.statusCode, 400);
    assert.match(res.body?.error || '', /invalid session_id/i);
  });
});

// ─── Production profile signature gate ───────────────────────────────────

test('production POST rejects unsigned envelope with 401', async () => {
  __test.resetState();
  await withProfile('production', async () => {
    const res = mockRes();
    await handler(mockReq({ body: basePayload(), ip: '198.51.100.20' }), res);
    assert.equal(res.statusCode, 401);
    assert.match(res.body?.error || '', /unsigned signaling payload rejected/i);
  });
});

test('production POST rejects tampered signature with 401', async () => {
  __test.resetState();
  const signer = freshSigner();
  await withProfile('production', async () => {
    const payload = signedPayload(signer);
    payload.sdp = 'tampered after signing';
    const res = mockRes();
    await handler(mockReq({ body: payload, ip: '198.51.100.21' }), res);
    assert.equal(res.statusCode, 401);
    assert.match(res.body?.error || '', /invalid signaling signature/i);
  });
});

test('production POST rejects bad from_pubkey_hex format', async () => {
  __test.resetState();
  const signer = freshSigner();
  await withProfile('production', async () => {
    const payload = signedPayload(signer);
    payload.from_pubkey_hex = 'not-a-hex-string';
    const res = mockRes();
    await handler(mockReq({ body: payload, ip: '198.51.100.22' }), res);
    assert.equal(res.statusCode, 401);
  });
});

test('production POST accepts valid signed envelope and queues it', async () => {
  __test.resetState();
  const signer = freshSigner();
  await withProfile('production', async () => {
    const payload = signedPayload(signer);
    const res = mockRes();
    await handler(mockReq({ body: payload, ip: '198.51.100.23' }), res);
    assert.equal(res.statusCode, 202);
    assert.deepEqual(res.body, { ok: true });

    // GET drain with the right slug returns the queued payload.
    const drainRes = mockRes();
    await handler(
      mockReq({ method: 'GET', query: { slug: VALID_TO, wait: 1 }, ip: '198.51.100.24' }),
      drainRes,
    );
    assert.equal(drainRes.statusCode, 200);
    assert.equal(drainRes.body.from_server_slug, VALID_FROM);
    assert.equal(drainRes.body.session_id, 'sess-123');
    assert.equal(drainRes.body.from_pubkey_hex, signer.rawPubHex);
  });
});

test('dev POST accepts unsigned envelope (legacy path)', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    await handler(mockReq({ body: basePayload(), ip: '198.51.100.25' }), res);
    assert.equal(res.statusCode, 202);
  });
});

test('dev POST rejects malformed signature when fields are present', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    // Dev tolerates unsigned, but a present-but-broken signature must
    // still fail loudly so a bug in the signer doesn't get silently
    // ignored.
    const res = mockRes();
    const body = {
      ...basePayload(),
      from_pubkey_hex: 'a'.repeat(64),
      vrp_signature: Buffer.from('not-a-real-signature').toString('base64'),
    };
    await handler(mockReq({ body, ip: '198.51.100.26' }), res);
    assert.equal(res.statusCode, 401);
  });
});

// ─── GET drain validation ────────────────────────────────────────────────

test('GET rejects missing slug query param', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    await handler(mockReq({ method: 'GET', query: {}, ip: '198.51.100.30' }), res);
    assert.equal(res.statusCode, 400);
    assert.match(res.body?.error || '', /missing slug query/i);
  });
});

test('GET rejects malformed slug', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    await handler(
      mockReq({ method: 'GET', query: { slug: 'not-hex!', wait: 1 }, ip: '198.51.100.31' }),
      res,
    );
    assert.equal(res.statusCode, 400);
    assert.match(res.body?.error || '', /invalid slug/i);
  });
});

test('GET returns 204 when queue is empty after wait expires', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    await handler(
      mockReq({ method: 'GET', query: { slug: VALID_TO, wait: 0 }, ip: '198.51.100.32' }),
      res,
    );
    assert.equal(res.statusCode, 204);
  });
});

// ─── Method allowlist ────────────────────────────────────────────────────

test('PUT is rejected with 405', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    await handler(mockReq({ method: 'PUT', ip: '198.51.100.40' }), res);
    assert.equal(res.statusCode, 405);
    assert.equal(res.headers['allow'], 'GET, POST');
  });
});

// ─── Rate limiting ───────────────────────────────────────────────────────

test('POST rate limit caps an aggressive sender per IP', async () => {
  __test.resetState();
  const signer = freshSigner();
  await withProfile('production', async () => {
    // POST_LIMIT_PER_MIN is high (60). Burst until we cross it.
    const ip = '198.51.100.50';
    let lastStatus = 0;
    for (let i = 0; i < __test.POST_LIMIT_PER_MIN + 5; i++) {
      const res = mockRes();
      await handler(mockReq({ body: signedPayload(signer), ip }), res);
      lastStatus = res.statusCode;
      if (res.statusCode === 429) break;
    }
    assert.equal(lastStatus, 429, 'eventual 429 expected after burst exceeds POST_LIMIT_PER_MIN');

    // A different IP must still be unaffected.
    const freshRes = mockRes();
    await handler(mockReq({ body: signedPayload(signer), ip: '198.51.100.51' }), freshRes);
    assert.equal(freshRes.statusCode, 202);
  });
});

test('GET rate limit caps an aggressive drainer per IP', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const ip = '198.51.100.60';
    let lastStatus = 0;
    // `wait: 0` so each empty drain returns immediately — otherwise the
    // 1-second polling clamp makes a 120-request burst exceed the rate
    // window and the limit never trips.
    for (let i = 0; i < __test.GET_LIMIT_PER_MIN + 5; i++) {
      const res = mockRes();
      await handler(
        mockReq({ method: 'GET', query: { slug: VALID_TO, wait: 0 }, ip }),
        res,
      );
      lastStatus = res.statusCode;
      if (res.statusCode === 429) break;
    }
    assert.equal(lastStatus, 429, 'eventual 429 expected after burst exceeds GET_LIMIT_PER_MIN');
  });
});

// ─── Drain authorization invariant ────────────────────────────────────────

test('GET drain cannot fetch a payload addressed to a different slug', async () => {
  __test.resetState();
  const signer = freshSigner();
  await withProfile('production', async () => {
    // Enqueue addressed to VALID_TO.
    const postRes = mockRes();
    await handler(mockReq({ body: signedPayload(signer), ip: '198.51.100.70' }), postRes);
    assert.equal(postRes.statusCode, 202);

    // Drain with a DIFFERENT slug → must return 204 (empty queue for that
    // slug). The relay does NOT cross slugs, so an attacker who knows
    // their own slug cannot poll for someone else's signals.
    const otherSlug = 'ffffffffffff';
    const drainRes = mockRes();
    await handler(
      mockReq({ method: 'GET', query: { slug: otherSlug, wait: 1 }, ip: '198.51.100.71' }),
      drainRes,
    );
    assert.equal(drainRes.statusCode, 204);

    // And the original queue still has the message — confirmed by
    // draining with the correct slug.
    const correctDrain = mockRes();
    await handler(
      mockReq({ method: 'GET', query: { slug: VALID_TO, wait: 1 }, ip: '198.51.100.72' }),
      correctDrain,
    );
    assert.equal(correctDrain.statusCode, 200);
    assert.equal(correctDrain.body.session_id, 'sess-123');
  });
});

// ─── Profile detection ──────────────────────────────────────────────────

test('isProductionProfile honours ANNEX_BUILD_PROFILE', () => {
  withProfile('production', () => {
    assert.equal(__test.isProductionProfile(), true);
  });
  withProfile('release', () => {
    assert.equal(__test.isProductionProfile(), true);
  });
  withProfile('dev', () => {
    assert.equal(__test.isProductionProfile(), false);
  });
  withProfile(null, () => {
    assert.equal(__test.isProductionProfile(), false);
  });
});

test('isProductionProfile falls back to NODE_ENV=production', () => {
  const prevProfile = process.env.ANNEX_BUILD_PROFILE;
  const prevNodeEnv = process.env.NODE_ENV;
  delete process.env.ANNEX_BUILD_PROFILE;
  process.env.NODE_ENV = 'production';
  try {
    assert.equal(__test.isProductionProfile(), true);
  } finally {
    if (prevProfile === undefined) delete process.env.ANNEX_BUILD_PROFILE;
    else process.env.ANNEX_BUILD_PROFILE = prevProfile;
    if (prevNodeEnv === undefined) delete process.env.NODE_ENV;
    else process.env.NODE_ENV = prevNodeEnv;
  }
});

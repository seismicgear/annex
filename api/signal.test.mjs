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
import { readFileSync } from 'node:fs';
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

// The suite signs with the implementation's own canonicaliser rather than a
// local copy of the format string. A local copy is how the original mismatch
// with the Rust signer survived: this file agreed with signal.js because both
// were written from the same misreading, and the party that disagreed —
// crates/annex-federation — was never in the room. The cross-language
// agreement is asserted separately, against api/canonical-vectors.json.
const canonicalEnvelope = __test.canonicalEnvelope;

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

// Snapshot + restore ANNEX_BUILD_PROFILE and ANNEX_SIGNAL_TRUSTED_PEERS
// around each test. Tests that don't pass `trustMap` clear the trust map
// env var entirely, which mirrors a freshly-deployed relay.
async function withProfile(profile, fn, { trustMap } = {}) {
  const prev = process.env.ANNEX_BUILD_PROFILE;
  const prevNodeEnv = process.env.NODE_ENV;
  const prevPeers = process.env.ANNEX_SIGNAL_TRUSTED_PEERS;
  if (profile === null) {
    delete process.env.ANNEX_BUILD_PROFILE;
    delete process.env.NODE_ENV;
  } else {
    process.env.ANNEX_BUILD_PROFILE = profile;
    delete process.env.NODE_ENV;
  }
  if (trustMap === undefined) delete process.env.ANNEX_SIGNAL_TRUSTED_PEERS;
  else process.env.ANNEX_SIGNAL_TRUSTED_PEERS = trustMap;
  // `await fn()` — not bare `return fn()` — is load-bearing. The body
  // is async and reads `process.env.*` from inside the handler across
  // multiple `await`s; without awaiting here, `finally` would tear the
  // env vars down before the handler ran its second pass and a
  // production test would silently see `isProd = false`.
  try {
    return await fn();
  } finally {
    if (prev === undefined) delete process.env.ANNEX_BUILD_PROFILE;
    else process.env.ANNEX_BUILD_PROFILE = prev;
    if (prevNodeEnv === undefined) delete process.env.NODE_ENV;
    else process.env.NODE_ENV = prevNodeEnv;
    if (prevPeers === undefined) delete process.env.ANNEX_SIGNAL_TRUSTED_PEERS;
    else process.env.ANNEX_SIGNAL_TRUSTED_PEERS = prevPeers;
  }
}

// Build a trust-map JSON string for the given slug→pubkey pairs.
function trustMapJson(entries) {
  return JSON.stringify(entries);
}

// Sign a GET-drain header set with `signer` for `slug` at `ts`.
function signDrainHeaders(signer, slug, ts = Date.now()) {
  const canonical = `drain|${slug.toLowerCase()}|${ts}`;
  return {
    'x-annex-drain-slug': slug,
    'x-annex-drain-timestamp': String(ts),
    'x-annex-drain-signature': signer.sign(canonical),
  };
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
  const sender = freshSigner();
  const receiver = freshSigner();
  await withProfile(
    'production',
    async () => {
      const payload = signedPayload(sender);
      const res = mockRes();
      await handler(mockReq({ body: payload, ip: '198.51.100.23' }), res);
      assert.equal(res.statusCode, 202, `expected 202; got ${res.statusCode} body=${JSON.stringify(res.body)}`);
      assert.deepEqual(res.body, { ok: true });

      // GET drain by the receiver (who owns VALID_TO) returns the payload.
      const drainRes = mockRes();
      await handler(
        mockReq({
          method: 'GET',
          query: { slug: VALID_TO, wait: 0 },
          ip: '198.51.100.24',
          headers: signDrainHeaders(receiver, VALID_TO),
        }),
        drainRes,
      );
      assert.equal(
        drainRes.statusCode,
        200,
        `expected 200; got ${drainRes.statusCode} body=${JSON.stringify(drainRes.body)}`,
      );
      assert.equal(drainRes.body.from_server_slug, VALID_FROM);
      assert.equal(drainRes.body.session_id, 'sess-123');
      assert.equal(drainRes.body.from_pubkey_hex, sender.rawPubHex);
    },
    {
      trustMap: trustMapJson({
        [VALID_FROM]: sender.rawPubHex,
        [VALID_TO]: receiver.rawPubHex,
      }),
    },
  );
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
  await withProfile(
    'production',
    async () => {
      // POST_LIMIT_PER_MIN is high (60). Burst until we cross it. Each
      // payload uses a fresh session_id so the replay defence doesn't
      // mask the rate-limit cap.
      const ip = '198.51.100.50';
      let lastStatus = 0;
      for (let i = 0; i < __test.POST_LIMIT_PER_MIN + 5; i++) {
        const base = { ...basePayload(), session_id: `sess-burst-${i}` };
        const res = mockRes();
        await handler(mockReq({ body: signedPayload(signer, base), ip }), res);
        lastStatus = res.statusCode;
        if (res.statusCode === 429) break;
      }
      assert.equal(lastStatus, 429, 'eventual 429 expected after burst exceeds POST_LIMIT_PER_MIN');

      // A different IP must still be unaffected.
      const freshRes = mockRes();
      const freshBase = { ...basePayload(), session_id: 'sess-fresh-ip' };
      await handler(mockReq({ body: signedPayload(signer, freshBase), ip: '198.51.100.51' }), freshRes);
      assert.equal(freshRes.statusCode, 202);
    },
    { trustMap: trustMapJson({ [VALID_FROM]: signer.rawPubHex }) },
  );
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
  const sender = freshSigner();
  const receiver = freshSigner();
  const otherOwner = freshSigner();
  const otherSlug = 'ffffffffffff';
  await withProfile(
    'production',
    async () => {
      // Enqueue addressed to VALID_TO.
      const postRes = mockRes();
      await handler(mockReq({ body: signedPayload(sender), ip: '198.51.100.70' }), postRes);
      assert.equal(postRes.statusCode, 202);

      // Drain with a DIFFERENT slug (owned by otherOwner), correctly
      // authenticated. The queue for that slug is empty so the
      // response must be 204 — the relay does not cross slugs.
      const drainRes = mockRes();
      await handler(
        mockReq({
          method: 'GET',
          query: { slug: otherSlug, wait: 0 },
          ip: '198.51.100.71',
          headers: signDrainHeaders(otherOwner, otherSlug),
        }),
        drainRes,
      );
      assert.equal(drainRes.statusCode, 204);

      // And the original queue still has the message — confirmed by
      // draining with the correct slug under the correct key.
      const correctDrain = mockRes();
      await handler(
        mockReq({
          method: 'GET',
          query: { slug: VALID_TO, wait: 0 },
          ip: '198.51.100.72',
          headers: signDrainHeaders(receiver, VALID_TO),
        }),
        correctDrain,
      );
      assert.equal(correctDrain.statusCode, 200);
      assert.equal(correctDrain.body.session_id, 'sess-123');
    },
    {
      trustMap: trustMapJson({
        [VALID_FROM]: sender.rawPubHex,
        [VALID_TO]: receiver.rawPubHex,
        [otherSlug]: otherOwner.rawPubHex,
      }),
    },
  );
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

// ─── Authorization: trust map (POST) ─────────────────────────────────────

test('production POST without trust map config returns 503', async () => {
  __test.resetState();
  const signer = freshSigner();
  // No trustMap → empty under production.
  await withProfile('production', async () => {
    const res = mockRes();
    await handler(mockReq({ body: signedPayload(signer), ip: '198.51.100.80' }), res);
    assert.equal(res.statusCode, 503);
    assert.match(res.body?.error || '', /ANNEX_SIGNAL_TRUSTED_PEERS is empty/i);
  });
});

test('production POST rejects valid signature from unknown key', async () => {
  __test.resetState();
  const signer = freshSigner();
  const unrelated = freshSigner();
  await withProfile(
    'production',
    async () => {
      const res = mockRes();
      // Trust map registers `unrelated` for VALID_FROM; payload signed
      // by `signer` (a different key) must be rejected even though the
      // signature itself verifies.
      await handler(mockReq({ body: signedPayload(signer), ip: '198.51.100.81' }), res);
      assert.equal(res.statusCode, 401);
      assert.match(res.body?.error || '', /does not match the trusted key/i);
    },
    { trustMap: trustMapJson({ [VALID_FROM]: unrelated.rawPubHex }) },
  );
});

test('production POST rejects signature for slug not in trust map', async () => {
  __test.resetState();
  const signer = freshSigner();
  await withProfile(
    'production',
    async () => {
      const res = mockRes();
      // Trust map registers `signer` for a DIFFERENT slug only. The
      // payload claims VALID_FROM, which is not in the map → 401.
      await handler(mockReq({ body: signedPayload(signer), ip: '198.51.100.82' }), res);
      assert.equal(res.statusCode, 401);
      assert.match(res.body?.error || '', /not in the trusted peer list/i);
    },
    { trustMap: trustMapJson({ '000000000000': signer.rawPubHex }) },
  );
});

test('production POST rejects key authorised for a different slug', async () => {
  __test.resetState();
  const signer = freshSigner();
  const decoy = 'aaaaaaaaaaaa';
  await withProfile(
    'production',
    async () => {
      // Trust map: `signer` is authorised for `decoy`, NOT for VALID_FROM.
      // Even though the payload's signature verifies against signer's
      // pubkey, the slug binding rejects it.
      const res = mockRes();
      await handler(mockReq({ body: signedPayload(signer), ip: '198.51.100.83' }), res);
      assert.equal(res.statusCode, 401);
      assert.match(
        res.body?.error || '',
        /not in the trusted peer list|does not match the trusted key/i,
      );
    },
    { trustMap: trustMapJson({ [decoy]: signer.rawPubHex }) },
  );
});

test('production POST canonical-field mismatch is rejected', async () => {
  __test.resetState();
  const signer = freshSigner();
  await withProfile(
    'production',
    async () => {
      // Sign a payload, then mutate the outer `to_server_slug` while
      // leaving the signature in place. The signature now no longer
      // matches the outer fields → rejected as invalid.
      const payload = signedPayload(signer);
      payload.to_server_slug = '000000000000';
      const res = mockRes();
      await handler(mockReq({ body: payload, ip: '198.51.100.84' }), res);
      assert.equal(res.statusCode, 401);
      assert.match(res.body?.error || '', /invalid signaling signature/i);
    },
    { trustMap: trustMapJson({ [VALID_FROM]: signer.rawPubHex }) },
  );
});

test('production POST rejects replayed signed envelope', async () => {
  __test.resetState();
  const signer = freshSigner();
  await withProfile(
    'production',
    async () => {
      const payload = signedPayload(signer);
      // First send succeeds.
      const firstRes = mockRes();
      await handler(mockReq({ body: payload, ip: '198.51.100.85' }), firstRes);
      assert.equal(firstRes.statusCode, 202);
      // Same exact bytes replayed → 409 Conflict (replay).
      const replayRes = mockRes();
      await handler(mockReq({ body: payload, ip: '198.51.100.86' }), replayRes);
      assert.equal(replayRes.statusCode, 409);
      assert.match(replayRes.body?.error || '', /replayed signaling envelope/i);
    },
    { trustMap: trustMapJson({ [VALID_FROM]: signer.rawPubHex }) },
  );
});

test('production POST returns 503 on malformed trust map', async () => {
  __test.resetState();
  const signer = freshSigner();
  await withProfile(
    'production',
    async () => {
      const res = mockRes();
      await handler(mockReq({ body: signedPayload(signer), ip: '198.51.100.87' }), res);
      assert.equal(res.statusCode, 503);
      assert.match(res.body?.error || '', /relay misconfigured/i);
    },
    // Bad pubkey length (32 chars instead of 64) — must surface as 503.
    { trustMap: `${VALID_FROM}:deadbeefdeadbeefdeadbeefdeadbeef` },
  );
});

test('parseTrustedPeers parses both JSON and CSV forms', () => {
  const csv = __test.parseTrustedPeers(
    `${VALID_FROM}:${'a'.repeat(64)},${VALID_TO}:${'b'.repeat(64)}`,
  );
  assert.equal(csv.size, 2);
  assert.equal(csv.get(VALID_FROM), 'a'.repeat(64));
  assert.equal(csv.get(VALID_TO), 'b'.repeat(64));

  const json = __test.parseTrustedPeers(
    JSON.stringify({ [VALID_FROM]: 'a'.repeat(64), [VALID_TO]: 'b'.repeat(64) }),
  );
  assert.equal(json.size, 2);
  assert.equal(json.get(VALID_FROM), 'a'.repeat(64));

  assert.throws(
    () => __test.parseTrustedPeers(`badslug:${'a'.repeat(64)}`),
    /invalid slug/i,
  );
  assert.throws(
    () => __test.parseTrustedPeers(`${VALID_FROM}:nothex`),
    /invalid pubkey/i,
  );
  // CSV duplicates DO throw (we walk every comma-separated entry).
  assert.throws(
    () => __test.parseTrustedPeers(
      `${VALID_FROM}:${'a'.repeat(64)},${VALID_FROM}:${'b'.repeat(64)}`,
    ),
    /duplicate slug/i,
  );
  // JSON dedupes itself before our walk (last value wins per ECMA-262),
  // so the parser sees one entry and must NOT throw.
  const dedup = __test.parseTrustedPeers(
    `{"${VALID_FROM}": "${'a'.repeat(64)}", "${VALID_FROM}": "${'b'.repeat(64)}"}`,
  );
  assert.equal(dedup.size, 1);
  assert.equal(dedup.get(VALID_FROM), 'b'.repeat(64));
});

test('dev profile: unsigned POST accepted regardless of trust map', async () => {
  __test.resetState();
  // Dev with NO trust map: unsigned accepted.
  await withProfile('dev', async () => {
    const res = mockRes();
    await handler(mockReq({ body: basePayload(), ip: '198.51.100.88' }), res);
    assert.equal(res.statusCode, 202);
  });
  // Dev with a populated trust map: unsigned STILL accepted (dev does
  // not enforce authz). The map is only relevant under production.
  __test.resetState();
  await withProfile(
    'dev',
    async () => {
      const res = mockRes();
      await handler(mockReq({ body: basePayload(), ip: '198.51.100.89' }), res);
      assert.equal(res.statusCode, 202);
    },
    { trustMap: trustMapJson({ [VALID_FROM]: 'a'.repeat(64) }) },
  );
});

// ─── Authorization: GET drain ───────────────────────────────────────────

test('production GET without auth headers returns 401', async () => {
  __test.resetState();
  const receiver = freshSigner();
  await withProfile(
    'production',
    async () => {
      const res = mockRes();
      await handler(
        mockReq({ method: 'GET', query: { slug: VALID_TO, wait: 0 }, ip: '198.51.100.90' }),
        res,
      );
      assert.equal(res.statusCode, 401);
      assert.match(res.body?.error || '', /drain requires x-annex-drain/i);
    },
    { trustMap: trustMapJson({ [VALID_TO]: receiver.rawPubHex }) },
  );
});

test('production GET with mismatched slug header returns 401', async () => {
  __test.resetState();
  const receiver = freshSigner();
  await withProfile(
    'production',
    async () => {
      // Drain headers are signed for VALID_TO but query asks for a
      // different slug → mismatch → 401.
      const res = mockRes();
      await handler(
        mockReq({
          method: 'GET',
          query: { slug: '000000000000', wait: 0 },
          ip: '198.51.100.91',
          headers: {
            ...signDrainHeaders(receiver, VALID_TO),
            'x-annex-drain-slug': '000000000000',
          },
        }),
        res,
      );
      assert.equal(res.statusCode, 401);
    },
    {
      trustMap: trustMapJson({
        [VALID_TO]: receiver.rawPubHex,
        '000000000000': receiver.rawPubHex,
      }),
    },
  );
});

test('production GET with wrong key for slug returns 401', async () => {
  __test.resetState();
  const realOwner = freshSigner();
  const attacker = freshSigner();
  await withProfile(
    'production',
    async () => {
      // Headers signed by `attacker`, but trust map says the slug
      // belongs to `realOwner` → signature does not verify → 401.
      const res = mockRes();
      await handler(
        mockReq({
          method: 'GET',
          query: { slug: VALID_TO, wait: 0 },
          ip: '198.51.100.92',
          headers: signDrainHeaders(attacker, VALID_TO),
        }),
        res,
      );
      assert.equal(res.statusCode, 401);
      assert.match(res.body?.error || '', /signature failed verification/i);
    },
    { trustMap: trustMapJson({ [VALID_TO]: realOwner.rawPubHex }) },
  );
});

test('production GET for slug not in trust map returns 401', async () => {
  __test.resetState();
  const stranger = freshSigner();
  await withProfile(
    'production',
    async () => {
      const res = mockRes();
      await handler(
        mockReq({
          method: 'GET',
          query: { slug: '111111111111', wait: 0 },
          ip: '198.51.100.93',
          headers: signDrainHeaders(stranger, '111111111111'),
        }),
        res,
      );
      assert.equal(res.statusCode, 401);
      assert.match(res.body?.error || '', /not in the trusted peer list/i);
    },
    { trustMap: trustMapJson({ [VALID_TO]: stranger.rawPubHex }) },
  );
});

test('production GET with correct signature accepted (returns 204 when empty)', async () => {
  __test.resetState();
  const receiver = freshSigner();
  await withProfile(
    'production',
    async () => {
      const res = mockRes();
      await handler(
        mockReq({
          method: 'GET',
          query: { slug: VALID_TO, wait: 0 },
          ip: '198.51.100.94',
          headers: signDrainHeaders(receiver, VALID_TO),
        }),
        res,
      );
      // No payload queued → 204. Status 401 here would mean auth failed.
      assert.equal(res.statusCode, 204);
    },
    { trustMap: trustMapJson({ [VALID_TO]: receiver.rawPubHex }) },
  );
});

test('production GET with stale timestamp returns 401', async () => {
  __test.resetState();
  const receiver = freshSigner();
  const ancientTs = Date.now() - 5 * 60_000;
  await withProfile(
    'production',
    async () => {
      const res = mockRes();
      await handler(
        mockReq({
          method: 'GET',
          query: { slug: VALID_TO, wait: 0 },
          ip: '198.51.100.95',
          headers: signDrainHeaders(receiver, VALID_TO, ancientTs),
        }),
        res,
      );
      assert.equal(res.statusCode, 401);
      assert.match(res.body?.error || '', /timestamp outside freshness window/i);
    },
    { trustMap: trustMapJson({ [VALID_TO]: receiver.rawPubHex }) },
  );
});

test('production GET drain rejects replayed signature', async () => {
  __test.resetState();
  const receiver = freshSigner();
  const ts = Date.now();
  const headers = signDrainHeaders(receiver, VALID_TO, ts);
  await withProfile(
    'production',
    async () => {
      // First drain accepted (204 — empty queue).
      const first = mockRes();
      await handler(
        mockReq({ method: 'GET', query: { slug: VALID_TO, wait: 0 }, ip: '198.51.100.96', headers }),
        first,
      );
      assert.equal(first.statusCode, 204);
      // Same headers replayed → 409.
      const second = mockRes();
      await handler(
        mockReq({ method: 'GET', query: { slug: VALID_TO, wait: 0 }, ip: '198.51.100.97', headers }),
        second,
      );
      assert.equal(second.statusCode, 409);
      assert.match(second.body?.error || '', /replayed GET drain/i);
    },
    { trustMap: trustMapJson({ [VALID_TO]: receiver.rawPubHex }) },
  );
});

test('dev GET drain remains permissive (no headers required)', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    await handler(
      mockReq({ method: 'GET', query: { slug: VALID_TO, wait: 0 }, ip: '198.51.100.98' }),
      res,
    );
    // No payload queued, no headers required in dev → 204 (empty).
    assert.equal(res.statusCode, 204);
  });
});

// ─── Cross-implementation contract ────────────────────────────────────────
//
// `crates/annex-federation` is a second implementation of this wire format,
// in another language, deployed separately. For the entire life of the relay
// transport the two disagreed about what gets signed: Rust interpolated
// `rendezvous_tag` as a fourth field of the canonical string and this file did
// not. Every envelope a Rust peer sent would have been a 401 and the only
// symptom would have been federation never connecting. Nothing failed, because
// nothing instantiated the transport and each side's tests agreed with itself.
//
// These read vectors GENERATED BY THE RUST SIDE
// (`cargo test -p annex-federation --test canonical_vectors -- --ignored emit`).
// A signature produced by ed25519-dalek is verified here by node:crypto, so
// what is asserted is not "we both build the same string" but "a real envelope
// crosses the boundary".

const CANONICAL_VECTORS = JSON.parse(
  readFileSync(new URL('./canonical-vectors.json', import.meta.url), 'utf-8'),
);
const RENDEZVOUS_VECTORS = JSON.parse(
  readFileSync(new URL('./rendezvous-vectors.json', import.meta.url), 'utf-8'),
);

test('canonical string matches the Rust implementation, byte for byte', () => {
  for (const vec of CANONICAL_VECTORS.envelopes) {
    assert.equal(
      __test.canonicalEnvelope(vec.envelope),
      vec.canonical,
      `format_version ${vec.envelope.format_version}: canonical string differs from Rust`,
    );
  }
});

test('queue key matches the Rust implementation', () => {
  for (const vec of CANONICAL_VECTORS.envelopes) {
    assert.equal(__test.queueKeyFor(vec.envelope), vec.queue_key);
  }
});

test('a signature produced by the Rust signer verifies here', () => {
  for (const vec of CANONICAL_VECTORS.envelopes) {
    assert.equal(
      __test.verifySignedEnvelope(vec.envelope),
      true,
      `format_version ${vec.envelope.format_version}: a Rust-signed envelope failed verification`,
    );
  }
});

test('tampering with any signed field breaks a Rust-produced signature', () => {
  const v2 = CANONICAL_VECTORS.envelopes.find((e) => e.envelope.format_version === 2);
  for (const field of ['rendezvous_tag', 'session_id', 'sdp_type', 'sdp', 'from_pubkey_hex']) {
    const tampered = { ...v2.envelope };
    tampered[field] = field === 'sdp_type' ? 'answer-x' : `${tampered[field]}x`;
    assert.equal(
      __test.verifySignedEnvelope(tampered),
      false,
      `rewriting ${field} should invalidate the signature`,
    );
  }
  assert.equal(
    __test.verifySignedEnvelope({ ...v2.envelope, sent_at_ms: v2.envelope.sent_at_ms + 1 }),
    false,
  );
});

test('a v2 envelope cannot be replayed as v1 (downgrade)', () => {
  const v2 = CANONICAL_VECTORS.envelopes.find((e) => e.envelope.format_version === 2);
  // Strip the version and the addressing field, leaving a v1-shaped envelope
  // that still carries the v2 signature. If `format_version` were outside the
  // canonical string, this would verify — and the attacker would have removed
  // the only field that says where the envelope was addressed.
  const downgraded = { ...v2.envelope };
  delete downgraded.format_version;
  downgraded.rendezvous_tag = '';
  downgraded.from_server_slug = 'a1b2c3d4e5f6';
  downgraded.to_server_slug = '0102030405ab';
  assert.equal(__test.verifySignedEnvelope(downgraded), false);
});

test('rendezvous tags derive identically to the Rust implementation', () => {
  assert.equal(Number(__test.BUCKET_SECONDS), RENDEZVOUS_VECTORS.bucket_seconds);
  assert.ok(RENDEZVOUS_VECTORS.tags.length >= 12, 'vector file looks truncated');
  for (const row of RENDEZVOUS_VECTORS.tags) {
    assert.equal(
      __test.rendezvousTagForPubkey(row.ed25519_pubkey_hex, BigInt(row.bucket)),
      row.tag,
      `tag for ${row.ed25519_pubkey_hex.slice(0, 8)}… bucket ${row.bucket} differs from Rust`,
    );
  }
});

test('a malformed pubkey yields no tag rather than throwing', () => {
  assert.equal(__test.rendezvousTagForPubkey('not-hex', 0n), null);
  assert.equal(__test.rendezvousTagForPubkey('ab'.repeat(31), 0n), null);
  // y == 1 is the identity point: the conversion divides by zero.
  const identity = Buffer.alloc(32);
  identity[0] = 1;
  assert.equal(__test.rendezvousTagForPubkey(identity.toString('hex'), 0n), null);
  assert.equal(__test.pubkeyOwnsTag('not-hex', 'x'.repeat(43)), false);
});

// ─── format_version 2: rendezvous addressing ──────────────────────────────

function v2Payload(signer, { tag, sdp = 'c2VhbGVk' } = {}) {
  const recipient = tag ?? __test.rendezvousTagForPubkey(signer.rawPubHex, __test.currentBucket());
  const p = {
    format_version: 2,
    rendezvous_tag: recipient,
    session_id: 'sess-v2',
    sdp_type: 'offer',
    sdp,
    sent_at_ms: Date.now(),
    from_pubkey_hex: signer.rawPubHex,
  };
  p.vrp_signature = signer.sign(__test.canonicalEnvelope(p));
  return p;
}

function signDrainHeadersV2(signer, tag, ts = Date.now()) {
  return {
    'x-annex-drain-tag': tag,
    'x-annex-drain-pubkey': signer.rawPubHex,
    'x-annex-drain-timestamp': String(ts),
    'x-annex-drain-signature': signer.sign(`drain|v2|${tag}|${ts}`),
  };
}

test('v2 envelope carrying a server slug is rejected', () => {
  const signer = freshSigner();
  const p = v2Payload(signer);
  p.from_server_slug = VALID_FROM;
  const err = __test.validatePostPayload(p);
  assert.equal(err?.status, 400);
  assert.match(err.error, /must not carry server slugs/);
});

test('v1 envelope carrying a rendezvous_tag is rejected', () => {
  const p = { ...basePayload(), rendezvous_tag: 'x'.repeat(43) };
  const err = __test.validatePostPayload(p);
  assert.equal(err?.status, 400);
  assert.match(err.error, /requires format_version 2/);
});

test('an unknown format_version is rejected rather than treated as v1', () => {
  const p = { ...basePayload(), format_version: 99 };
  const err = __test.validatePostPayload(p);
  assert.equal(err?.status, 400);
  assert.match(err.error, /unsupported format_version/);
});

test('v2 envelope with a malformed tag is rejected', () => {
  const signer = freshSigner();
  const p = v2Payload(signer, { tag: 'too-short' });
  const err = __test.validatePostPayload(p);
  assert.equal(err?.status, 400);
  assert.match(err.error, /invalid rendezvous_tag/);
});

test('production v2 round trip: signed envelope queues under the tag and drains', async () => {
  const peer = freshSigner();
  const recipient = freshSigner();
  __test.resetState();
  const tag = __test.rendezvousTagForPubkey(recipient.rawPubHex, __test.currentBucket());
  // The trust map still binds a slug to a key — v2 uses only the key half,
  // because a relay that must not learn the slug graph cannot check slugs.
  const trustMap = trustMapJson({
    [VALID_FROM]: peer.rawPubHex,
    [VALID_TO]: recipient.rawPubHex,
  });

  await withProfile('production', async () => {
    const post = mockRes();
    await handler(mockReq({ body: v2Payload(peer, { tag }), ip: '203.0.113.9' }), post);
    assert.equal(post.statusCode, 202, JSON.stringify(post.body));

    // Drained by TAG, not slug.
    const get = mockRes();
    await handler(
      mockReq({
        method: 'GET',
        query: { tag, wait: '0' },
        ip: '203.0.113.10',
        headers: signDrainHeadersV2(recipient, tag),
      }),
      get,
    );
    assert.equal(get.statusCode, 200, JSON.stringify(get.body));
    assert.equal(get.body.rendezvous_tag, tag);
    assert.equal(get.body.format_version, 2);
    assert.equal(get.body.from_server_slug, '', 'the relay must not learn a slug');
  }, { trustMap });
});

test('production: an authorised peer cannot drain a tag that is not its own', async () => {
  const peer = freshSigner();
  const recipient = freshSigner();
  __test.resetState();
  const tag = __test.rendezvousTagForPubkey(recipient.rawPubHex, __test.currentBucket());
  const trustMap = trustMapJson({
    [VALID_FROM]: peer.rawPubHex,
    [VALID_TO]: recipient.rawPubHex,
  });

  await withProfile('production', async () => {
    const post = mockRes();
    await handler(mockReq({ body: v2Payload(peer, { tag }), ip: '203.0.113.11' }), post);
    assert.equal(post.statusCode, 202);

    // `peer` is in the trust map and signs correctly — the only thing wrong is
    // that this is not its queue. Without the tag-ownership check every
    // authorised peer could intercept every other peer's bootstrap, which is
    // strictly worse than v1's slug binding.
    const get = mockRes();
    await handler(
      mockReq({
        method: 'GET',
        query: { tag, wait: '0' },
        ip: '203.0.113.12',
        headers: signDrainHeadersV2(peer, tag),
      }),
      get,
    );
    assert.equal(get.statusCode, 401, JSON.stringify(get.body));
    assert.match(get.body.error, /rendezvous address/);
  }, { trustMap });
});

test('production: a key outside the trust map cannot post a v2 envelope', async () => {
  const stranger = freshSigner();
  const recipient = freshSigner();
  __test.resetState();
  const tag = __test.rendezvousTagForPubkey(recipient.rawPubHex, __test.currentBucket());
  await withProfile('production', async () => {
    const res = mockRes();
    await handler(mockReq({ body: v2Payload(stranger, { tag }), ip: '203.0.113.13' }), res);
    assert.equal(res.statusCode, 401, JSON.stringify(res.body));
    assert.match(res.body.error, /not in the trusted peer list/);
  }, { trustMap: trustMapJson({ [VALID_TO]: recipient.rawPubHex }) });
});

test('GET rejects passing both tag and slug', async () => {
  const res = mockRes();
  await handler(
    mockReq({ method: 'GET', query: { tag: 'x'.repeat(43), slug: VALID_TO, wait: '0' } }),
    res,
  );
  assert.equal(res.statusCode, 400);
  assert.match(res.body.error, /not both/);
});

test('GET rejects a malformed tag', async () => {
  const res = mockRes();
  await handler(mockReq({ method: 'GET', query: { tag: 'short', wait: '0' } }), res);
  assert.equal(res.statusCode, 400);
  assert.match(res.body.error, /invalid tag/);
});

test('a v1 slug queue and a v2 tag queue do not collide', async () => {
  __test.resetState();
  const peer = freshSigner();
  const recipient = freshSigner();
  const tag = __test.rendezvousTagForPubkey(recipient.rawPubHex, __test.currentBucket());

  await withProfile('dev', async () => {
    const a = mockRes();
    await handler(mockReq({ body: basePayload(), ip: '203.0.113.20' }), a);
    assert.equal(a.statusCode, 202);
    const b = mockRes();
    await handler(mockReq({ body: v2Payload(peer, { tag }), ip: '203.0.113.21' }), b);
    assert.equal(b.statusCode, 202);

    const bySlug = mockRes();
    await handler(mockReq({ method: 'GET', query: { slug: VALID_TO, wait: '0' }, ip: '203.0.113.22' }), bySlug);
    assert.equal(bySlug.statusCode, 200);
    assert.equal(bySlug.body.format_version, 1);

    const byTag = mockRes();
    await handler(mockReq({ method: 'GET', query: { tag, wait: '0' }, ip: '203.0.113.23' }), byTag);
    assert.equal(byTag.statusCode, 200);
    assert.equal(byTag.body.format_version, 2);
  });
});

test('the relay drops fields it did not verify rather than forwarding them', async () => {
  __test.resetState();
  await withProfile('dev', async () => {
    const res = mockRes();
    await handler(
      mockReq({ body: { ...basePayload(), injected_by_relay_operator: 'hello' }, ip: '203.0.113.24' }),
      res,
    );
    assert.equal(res.statusCode, 202);
    const get = mockRes();
    await handler(mockReq({ method: 'GET', query: { slug: VALID_TO, wait: '0' }, ip: '203.0.113.25' }), get);
    assert.equal(get.statusCode, 200);
    assert.equal(get.body.injected_by_relay_operator, undefined);
  });
});

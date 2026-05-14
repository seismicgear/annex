// api/signal.js — Annex federation SDP/ICE signaling relay.
//
// Stateless WebRTC peer-to-peer bootstrap. Servers POST signed
// SignalingPayload envelopes addressed to a peer's `to_server_slug`,
// and the recipient long-polls GET ?slug=… to drain its queue.
//
// ── Security model ────────────────────────────────────────────────
//
// Under a production profile (ANNEX_BUILD_PROFILE=production|release,
// or NODE_ENV=production as a fallback), the relay enforces:
//
//   1. Strict slug format: /^[0-9a-f]{12}$/ on both `from_server_slug`
//      and `to_server_slug`. Matches the slug derivation in
//      `crates/annex-server/src/config.rs::derive_server_slug_from_public_url`
//      (SHA-256 of the public URL, first 6 bytes hex-encoded). Random
//      ASCII / overlong / control-character slugs are rejected before
//      any state is touched.
//
//   2. Payload size cap: 50 KiB total JSON body; SDP body capped at
//      30 KiB. Anything larger is rejected with 413.
//
//   3. Freshness: `sent_at_ms` must be within ±60s of the relay clock.
//      Anything outside that window is dropped — relay-side replay
//      protection in addition to the receiver's own freshness check.
//
//   4. Ed25519 signature: `vrp_signature` must be a base64-encoded
//      Ed25519 signature over the canonical envelope
//        `${from_server_slug}|${to_server_slug}|${session_id}|${sdp_type}|${sdp}|${sent_at_ms}|${from_pubkey_hex}`
//      verifiable with `from_pubkey_hex` (64 hex chars = 32 bytes raw
//      pubkey). The relay does NOT bind slug→pubkey — that's the
//      recipient's job via its `SignalVerifier` (see
//      crates/annex-federation/src/transport.rs). The relay's job is
//      to refuse anything that doesn't even hold a private key.
//
//   5. Per-IP rate limits, separate POST and GET budgets. Sliding
//      60-second window. Crossing either limit returns 429.
//
// Under a dev profile (anything other than production/release),
// signatures are optional but still validated when present. This lets
// local SFU-less testing run without keys, but any deployment that
// claims production must set ANNEX_BUILD_PROFILE=production and ship
// signed envelopes.
//
// The relay is intentionally stateless across cold starts: queues live
// in `globalThis.__annexSignalQueues` so the Vercel function instance
// can warm-start with state, but a redeploy/restart drops everything.
// Signaling is bootstrap-only; after the WebRTC handshake completes,
// federated traffic flows over the encrypted data channel and never
// touches this relay again.

import { createPublicKey, verify as cryptoVerify } from 'node:crypto';

const SIGNAL_TTL_MS = 120_000;
const MAX_QUEUE_LENGTH = 128;
const MAX_BODY_BYTES = 50 * 1024;
const MAX_SDP_BYTES = 30 * 1024;
const MAX_SESSION_ID_LEN = 128;
const FRESHNESS_WINDOW_MS = 60_000;
const SLUG_REGEX = /^[0-9a-f]{12}$/;
const HEX64_REGEX = /^[0-9a-f]{64}$/;
const SESSION_ID_REGEX = /^[A-Za-z0-9_-]{1,128}$/;
const SDP_TYPES = new Set(['offer', 'answer']);

// Per-IP rate limit budgets per 60s sliding window.
const POST_LIMIT_PER_MIN = 60;
const GET_LIMIT_PER_MIN = 120;
const RATE_LIMIT_WINDOW_MS = 60_000;

// Ed25519 DER SPKI prefix. Prepending these 12 bytes to a 32-byte raw
// public key produces a valid SPKI blob `crypto.createPublicKey` accepts.
// 302a300506032b6570032100 == SEQUENCE { SEQUENCE { OID 1.3.101.112 (Ed25519) } BIT STRING (0 unused, 32 bytes follow) }
const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

/** @type {Map<string, Array<{payload: any, expiresAt: number}>>} */
const signalQueues = globalThis.__annexSignalQueues || new Map();
globalThis.__annexSignalQueues = signalQueues;

/**
 * @typedef {{ window_start: number, count: number }} RateWindow
 */
/** @type {Map<string, RateWindow>} */
const rateState = globalThis.__annexSignalRate || new Map();
globalThis.__annexSignalRate = rateState;

/**
 * Returns true iff this run is treated as a production deployment.
 * `ANNEX_BUILD_PROFILE` is the canonical signal (matches the convention
 * used by `zk/scripts/verify-artifacts.js` and the Rust server config
 * gate). `NODE_ENV=production` is honoured as a Vercel-friendly fallback.
 */
function isProductionProfile() {
  const profile = String(process.env.ANNEX_BUILD_PROFILE || '').trim().toLowerCase();
  if (profile === 'production' || profile === 'release') return true;
  if (profile === 'dev' || profile === 'development') return false;
  return String(process.env.NODE_ENV || '').trim().toLowerCase() === 'production';
}

function purgeExpired(slug) {
  const queue = signalQueues.get(slug);
  if (!queue) return;
  const now = Date.now();
  const live = queue.filter((item) => item.expiresAt > now);
  if (live.length === 0) {
    signalQueues.delete(slug);
    return;
  }
  signalQueues.set(slug, live);
}

function enqueueSignal(slug, payload) {
  purgeExpired(slug);
  const queue = signalQueues.get(slug) || [];
  if (queue.length >= MAX_QUEUE_LENGTH) {
    queue.shift();
  }
  queue.push({ payload, expiresAt: Date.now() + SIGNAL_TTL_MS });
  signalQueues.set(slug, queue);
}

function dequeueSignal(slug) {
  purgeExpired(slug);
  const queue = signalQueues.get(slug);
  if (!queue || queue.length === 0) return null;
  const next = queue.shift();
  if (queue.length === 0) signalQueues.delete(slug);
  else signalQueues.set(slug, queue);
  return next?.payload || null;
}

/**
 * Sliding-window rate limit per (ip, method). Returns `true` if the
 * request is allowed, `false` if the caller is over budget. Mirrors the
 * shape of `crates/annex-server/src/middleware.rs::RateLimiter` so the
 * relay's behaviour is unsurprising to anyone who has read the server's
 * rate limiter.
 */
function rateLimitCheck(ip, method, limit) {
  const key = `${method}:${ip}`;
  const now = Date.now();
  let entry = rateState.get(key);
  if (!entry || now - entry.window_start > RATE_LIMIT_WINDOW_MS) {
    entry = { window_start: now, count: 0 };
  }
  entry.count += 1;
  rateState.set(key, entry);

  // Inline cleanup to keep the map bounded. Anything older than two
  // windows can be forgotten outright.
  if (rateState.size > 5000) {
    for (const [k, v] of rateState) {
      if (now - v.window_start > RATE_LIMIT_WINDOW_MS * 2) rateState.delete(k);
    }
  }
  return entry.count <= limit;
}

function clientIp(req) {
  const forwarded = req.headers['x-forwarded-for'];
  if (typeof forwarded === 'string' && forwarded.length > 0) {
    const first = forwarded.split(',')[0].trim();
    if (first) return first;
  }
  return req.socket?.remoteAddress || 'unknown';
}

function bodyByteLength(body) {
  try {
    return Buffer.byteLength(JSON.stringify(body), 'utf-8');
  } catch {
    return Number.POSITIVE_INFINITY;
  }
}

/**
 * Validate the structural shape and field sizes of a POST payload.
 * Returns `null` on success, otherwise an error object suitable for
 * the JSON response body. Crypto checks happen later — this is the
 * pre-filter that keeps junk traffic from ever reaching the verifier.
 */
function validatePostPayload(payload) {
  if (payload === null || typeof payload !== 'object') {
    return { status: 400, error: 'body must be a JSON object' };
  }
  const { from_server_slug, to_server_slug, session_id, sdp_type, sdp, sent_at_ms } = payload;
  if (!from_server_slug || !to_server_slug || !session_id || !sdp_type || !sdp) {
    return { status: 400, error: 'missing required signaling fields' };
  }
  if (typeof from_server_slug !== 'string' || !SLUG_REGEX.test(from_server_slug)) {
    return { status: 400, error: 'invalid from_server_slug' };
  }
  if (typeof to_server_slug !== 'string' || !SLUG_REGEX.test(to_server_slug)) {
    return { status: 400, error: 'invalid to_server_slug' };
  }
  if (typeof session_id !== 'string' || !SESSION_ID_REGEX.test(session_id)) {
    return { status: 400, error: 'invalid session_id' };
  }
  if (typeof sdp_type !== 'string' || !SDP_TYPES.has(sdp_type)) {
    return { status: 400, error: 'invalid sdp_type' };
  }
  if (typeof sdp !== 'string' || sdp.length === 0) {
    return { status: 400, error: 'invalid sdp' };
  }
  if (Buffer.byteLength(sdp, 'utf-8') > MAX_SDP_BYTES) {
    return { status: 413, error: 'sdp too large' };
  }
  if (sent_at_ms !== undefined) {
    if (typeof sent_at_ms !== 'number' || !Number.isFinite(sent_at_ms)) {
      return { status: 400, error: 'invalid sent_at_ms' };
    }
    if (Math.abs(Date.now() - sent_at_ms) > FRESHNESS_WINDOW_MS) {
      return { status: 400, error: 'sent_at_ms outside freshness window' };
    }
  }
  // Session id length already constrained by regex; double-check defensively.
  if (session_id.length > MAX_SESSION_ID_LEN) {
    return { status: 400, error: 'session_id too long' };
  }
  return null;
}

/**
 * Verify the Ed25519 signature over the canonical envelope. Returns
 * `true` on a verified signature. Catches every thrown error from the
 * crypto library so a malformed pubkey can't crash the request.
 */
function verifySignedEnvelope(payload) {
  const {
    from_server_slug,
    to_server_slug,
    session_id,
    sdp_type,
    sdp,
    sent_at_ms,
    from_pubkey_hex,
    vrp_signature,
  } = payload;

  if (typeof from_pubkey_hex !== 'string' || !HEX64_REGEX.test(from_pubkey_hex)) {
    return false;
  }
  if (typeof vrp_signature !== 'string' || vrp_signature.length === 0) {
    return false;
  }
  // `sent_at_ms` is required for a signed envelope — the signature binds it.
  if (typeof sent_at_ms !== 'number' || !Number.isFinite(sent_at_ms)) {
    return false;
  }

  const canonical = `${from_server_slug}|${to_server_slug}|${session_id}|${sdp_type}|${sdp}|${sent_at_ms}|${from_pubkey_hex}`;

  let sigBuf;
  try {
    sigBuf = Buffer.from(vrp_signature, 'base64');
  } catch {
    return false;
  }
  // Ed25519 signatures are always 64 bytes.
  if (sigBuf.length !== 64) return false;

  let pubKey;
  try {
    const spki = Buffer.concat([ED25519_SPKI_PREFIX, Buffer.from(from_pubkey_hex, 'hex')]);
    pubKey = createPublicKey({ key: spki, format: 'der', type: 'spki' });
  } catch {
    return false;
  }

  try {
    return cryptoVerify(null, Buffer.from(canonical, 'utf-8'), pubKey, sigBuf);
  } catch {
    return false;
  }
}

export default async function handler(req, res) {
  const ip = clientIp(req);
  const isProd = isProductionProfile();

  if (req.method === 'POST') {
    if (!rateLimitCheck(ip, 'POST', POST_LIMIT_PER_MIN)) {
      res.setHeader('retry-after', '60');
      res.status(429).json({ error: 'rate limit exceeded' });
      return;
    }

    if (bodyByteLength(req.body) > MAX_BODY_BYTES) {
      res.status(413).json({ error: 'payload too large' });
      return;
    }

    const payload = req.body || {};
    const validationError = validatePostPayload(payload);
    if (validationError) {
      res.status(validationError.status).json({ error: validationError.error });
      return;
    }

    const hasSignatureFields =
      typeof payload.from_pubkey_hex === 'string' && typeof payload.vrp_signature === 'string';
    if (isProd) {
      if (!hasSignatureFields) {
        res.status(401).json({
          error:
            'unsigned signaling payload rejected under production profile; include from_pubkey_hex and vrp_signature',
        });
        return;
      }
      if (!verifySignedEnvelope(payload)) {
        res.status(401).json({ error: 'invalid signaling signature' });
        return;
      }
    } else if (hasSignatureFields && !verifySignedEnvelope(payload)) {
      // Dev mode tolerates unsigned, but if the caller bothered to
      // include fields they must be valid — silent acceptance of a
      // broken signature would mask bugs in the signer.
      res.status(401).json({ error: 'invalid signaling signature' });
      return;
    }

    const queued = {
      from_server_slug: payload.from_server_slug,
      to_server_slug: payload.to_server_slug,
      session_id: payload.session_id,
      sdp_type: payload.sdp_type,
      sdp: payload.sdp,
      sent_at_ms: payload.sent_at_ms,
      from_pubkey_hex: payload.from_pubkey_hex,
      vrp_signature: payload.vrp_signature,
      created_at: new Date().toISOString(),
    };
    enqueueSignal(payload.to_server_slug, queued);

    res.status(202).json({ ok: true });
    return;
  }

  if (req.method === 'GET') {
    if (!rateLimitCheck(ip, 'GET', GET_LIMIT_PER_MIN)) {
      res.setHeader('retry-after', '60');
      res.status(429).json({ error: 'rate limit exceeded' });
      return;
    }

    const slug = String(req.query?.slug || '').trim();
    const waitSecondsRaw = Number(req.query?.wait ?? 25);
    // `wait=0` is allowed and means "respond immediately with whatever is
    // currently queued (or 204)" — useful for non-polling drains and for
    // testing rate limits without burning real time.
    const waitSeconds = Number.isFinite(waitSecondsRaw)
      ? Math.min(Math.max(waitSecondsRaw, 0), 90)
      : 25;

    if (!slug) {
      res.status(400).json({ error: 'missing slug query parameter' });
      return;
    }
    if (!SLUG_REGEX.test(slug)) {
      res.status(400).json({ error: 'invalid slug' });
      return;
    }

    const deadline = Date.now() + waitSeconds * 1000;
    while (Date.now() < deadline) {
      const payload = dequeueSignal(slug);
      if (payload) {
        res.status(200).json(payload);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, 250));
    }

    res.status(204).end();
    return;
  }

  res.setHeader('Allow', 'GET, POST');
  res.status(405).json({ error: 'method not allowed' });
}

// Exported for tests. Not part of the public Vercel handler contract.
export const __test = {
  isProductionProfile,
  validatePostPayload,
  verifySignedEnvelope,
  SLUG_REGEX,
  HEX64_REGEX,
  SESSION_ID_REGEX,
  POST_LIMIT_PER_MIN,
  GET_LIMIT_PER_MIN,
  // Reset state between tests so they're independent.
  resetState() {
    signalQueues.clear();
    rateState.clear();
  },
};

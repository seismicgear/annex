// api/signal.js — Annex federation SDP/ICE signaling relay.
//
// Stateless WebRTC peer-to-peer bootstrap. Servers POST signed
// SignalingPayload envelopes addressed to a peer's `to_server_slug`,
// and the recipient long-polls GET ?slug=… to drain its queue.
//
// ── Trust model ─────────────────────────────────────────────────────
//
// Under a production profile (ANNEX_BUILD_PROFILE=production|release,
// or NODE_ENV=production as a fallback) the relay enforces THREE
// independent checks:
//
//   1. Authentication — every payload carries an Ed25519 signature
//      over the canonical envelope, verifiable with `from_pubkey_hex`.
//      Possessing the matching private key is required to even reach
//      the queue.
//
//   2. Authorization — the relay consults a trust map
//      (`ANNEX_SIGNAL_TRUSTED_PEERS`) that binds each
//      `from_server_slug` to ONE specific `from_pubkey_hex`. A
//      signature from a key that is not the registered key for the
//      claimed slug is rejected even if the signature is mathematically
//      valid. A signature from a key that IS in the map but for a
//      DIFFERENT slug is rejected (no cross-slug reuse).
//
//   3. Replay defense — every accepted envelope is fingerprinted by
//      `(session_id, sdp_type, vrp_signature)` and refused if seen
//      again inside the freshness window. The signature itself binds
//      the timestamp so a replayed-after-window attack also fails the
//      freshness check.
//
// GET drain is gated by the same trust map. Production drains must
// present headers proving control of the receiving server's signing
// key (see "GET drain authorization" below).
//
// Under a dev profile (anything other than production/release), the
// trust map is optional and unsigned envelopes are tolerated. This is
// for local SFU-less testing; never deploy with a dev profile and
// expect any of the above guarantees.
//
// ── ANNEX_SIGNAL_TRUSTED_PEERS format ───────────────────────────────
//
//   Either a JSON object:
//     {"abcdef012345": "ed25519_pubkey_hex_64chars",
//      "fedcba543210": "another_pubkey_hex_64chars"}
//   Or a comma-separated list:
//     "abcdef012345:ed25519_pubkey_hex,fedcba543210:other_pubkey_hex"
//
//   Slugs MUST match /^[0-9a-f]{12}$/. Pubkeys MUST be 64 lowercase hex
//   chars. A malformed map is a startup error under production: the
//   relay refuses to serve until the operator fixes the config.
//
// ── Canonical signature input ───────────────────────────────────────
//
//   from_server_slug | to_server_slug | session_id | sdp_type | sdp |
//   sent_at_ms | from_pubkey_hex
//
//   Every field that appears in the outer JSON also appears in the
//   signed input, so an attacker rewriting an outer field while
//   preserving the signature is caught at verify-time.
//
// ── GET drain authorization ─────────────────────────────────────────
//
//   Headers required under production:
//     x-annex-drain-slug:      <12-hex slug of the draining server>
//     x-annex-drain-timestamp: <unix milliseconds, ±60s of relay clock>
//     x-annex-drain-signature: <base64 Ed25519 sig over canonical>
//
//   Canonical: `drain|<slug>|<timestamp>`
//   The slug header MUST equal the `?slug=` query parameter (no
//   draining a different queue than the one you signed for). The
//   signature is verified with the pubkey the trust map associates
//   with that slug, so only the legitimate owner of a slug can drain
//   its queue.
//
// The relay is intentionally stateless across cold starts: queues live
// in `globalThis.__annexSignalQueues` so the Vercel function instance
// can warm-start with state, but a redeploy/restart drops everything.

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

// Replay-defence cache: signature fingerprints we have already accepted.
// Keep entries for 2× the freshness window so anything still within
// signature-valid time is also still seen-recently. Bounded LRU-ish via
// inline cleanup.
const REPLAY_CACHE_TTL_MS = FRESHNESS_WINDOW_MS * 2;
const REPLAY_CACHE_MAX_ENTRIES = 10_000;

// Ed25519 DER SPKI prefix. Prepending these 12 bytes to a 32-byte raw
// public key produces a valid SPKI blob `crypto.createPublicKey` accepts.
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

/** @type {Map<string, number>} signature fingerprint -> expiresAt */
const replayCache = globalThis.__annexSignalReplay || new Map();
globalThis.__annexSignalReplay = replayCache;

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

/**
 * Parse the `ANNEX_SIGNAL_TRUSTED_PEERS` env var into a strict
 * `Map<slug, pubkey_hex>`. Accepts two encodings:
 *
 *   1. JSON object: `{"abcdef012345":"<64-hex-pubkey>", ...}`
 *   2. Comma-separated: `"abcdef012345:<pubkey>,fedcba...:..."`
 *
 * Every key MUST be a 12-char hex slug and every value MUST be a
 * 64-char lowercase hex Ed25519 pubkey. Anything that doesn't parse
 * raises a synchronous error — caller decides what to do (production
 * surfaces it as 503 + a structured error body).
 *
 * Returns an empty Map when the env var is unset OR empty, so callers
 * can distinguish "operator didn't configure trust" from "operator
 * shipped a broken trust map".
 */
function parseTrustedPeers(raw) {
  const out = new Map();
  if (typeof raw !== 'string') return out;
  const trimmed = raw.trim();
  if (!trimmed) return out;

  const ingest = (slug, pubkey) => {
    const slugLc = String(slug || '').trim().toLowerCase();
    const pubLc = String(pubkey || '').trim().toLowerCase();
    if (!SLUG_REGEX.test(slugLc)) {
      throw new Error(`invalid slug ${JSON.stringify(slug)} in ANNEX_SIGNAL_TRUSTED_PEERS`);
    }
    if (!HEX64_REGEX.test(pubLc)) {
      throw new Error(
        `invalid pubkey for slug ${slugLc} in ANNEX_SIGNAL_TRUSTED_PEERS (expected 64 hex chars)`,
      );
    }
    if (out.has(slugLc)) {
      throw new Error(`duplicate slug ${slugLc} in ANNEX_SIGNAL_TRUSTED_PEERS`);
    }
    out.set(slugLc, pubLc);
  };

  if (trimmed.startsWith('{')) {
    let obj;
    try {
      obj = JSON.parse(trimmed);
    } catch (e) {
      throw new Error(`ANNEX_SIGNAL_TRUSTED_PEERS is not valid JSON: ${e.message}`);
    }
    if (obj === null || typeof obj !== 'object' || Array.isArray(obj)) {
      throw new Error('ANNEX_SIGNAL_TRUSTED_PEERS must be a JSON object {slug: pubkey, ...}');
    }
    for (const [slug, pubkey] of Object.entries(obj)) {
      ingest(slug, pubkey);
    }
  } else {
    for (const piece of trimmed.split(',')) {
      const t = piece.trim();
      if (!t) continue;
      const sep = t.indexOf(':');
      if (sep <= 0) {
        throw new Error(
          `malformed entry ${JSON.stringify(t)} in ANNEX_SIGNAL_TRUSTED_PEERS (expected slug:pubkey)`,
        );
      }
      ingest(t.slice(0, sep), t.slice(sep + 1));
    }
  }
  return out;
}

/**
 * Look up the trust map for a slug. Throws if the env var is set but
 * malformed (caller's job to wrap and return 503). Returns `undefined`
 * if the slug is not registered.
 */
function trustedPubkeyForSlug(slug) {
  const raw = process.env.ANNEX_SIGNAL_TRUSTED_PEERS;
  const map = parseTrustedPeers(raw);
  return map.get(String(slug || '').toLowerCase());
}

function trustMapEntryCount() {
  return parseTrustedPeers(process.env.ANNEX_SIGNAL_TRUSTED_PEERS).size;
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
 * request is allowed, `false` if the caller is over budget.
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

  if (rateState.size > 5000) {
    for (const [k, v] of rateState) {
      if (now - v.window_start > RATE_LIMIT_WINDOW_MS * 2) rateState.delete(k);
    }
  }
  return entry.count <= limit;
}

/**
 * Record a signature fingerprint in the replay cache. Returns `true`
 * iff this fingerprint is new (i.e. the envelope is NOT a replay). The
 * fingerprint combines `(session_id, sdp_type, signature)` so a single
 * session can still send offer+answer (different sdp_type), but neither
 * direction can be replayed inside the freshness window.
 */
function recordReplayFingerprint(sessionId, sdpType, signature) {
  const fp = `${sessionId}|${sdpType}|${signature.slice(0, 32)}`;
  const now = Date.now();

  // Inline cleanup so the cache stays bounded.
  if (replayCache.size > REPLAY_CACHE_MAX_ENTRIES) {
    for (const [k, v] of replayCache) {
      if (v <= now) replayCache.delete(k);
    }
  }

  const existing = replayCache.get(fp);
  if (existing !== undefined && existing > now) return false;
  replayCache.set(fp, now + REPLAY_CACHE_TTL_MS);
  return true;
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

/**
 * Verify a GET-drain authorization header set against the trust map.
 *
 * Returns `null` on success, otherwise an error object suitable for
 * the JSON response body.
 *
 * Required headers (production):
 *   - `x-annex-drain-slug`      — MUST equal the `?slug=` query param.
 *   - `x-annex-drain-timestamp` — Unix ms, must be within ±FRESHNESS_WINDOW_MS.
 *   - `x-annex-drain-signature` — base64 Ed25519 sig over `drain|<slug>|<ts>`.
 *
 * The signature is verified against the trust map's pubkey for the
 * claimed slug. The slug header is cross-checked against the query
 * parameter so a signature for slug A cannot drain slug B.
 */
function verifyGetDrainAuth(requestSlug, headers) {
  const slugHeader = String(headers['x-annex-drain-slug'] || '').trim().toLowerCase();
  const tsHeader = String(headers['x-annex-drain-timestamp'] || '').trim();
  const sigHeader = String(headers['x-annex-drain-signature'] || '').trim();

  if (!slugHeader || !tsHeader || !sigHeader) {
    return { status: 401, error: 'GET drain requires x-annex-drain-{slug,timestamp,signature} headers' };
  }
  if (!SLUG_REGEX.test(slugHeader)) {
    return { status: 401, error: 'invalid x-annex-drain-slug' };
  }
  if (slugHeader !== String(requestSlug || '').toLowerCase()) {
    return { status: 401, error: 'x-annex-drain-slug does not match ?slug= query parameter' };
  }
  const ts = Number(tsHeader);
  if (!Number.isFinite(ts) || Math.abs(Date.now() - ts) > FRESHNESS_WINDOW_MS) {
    return { status: 401, error: 'x-annex-drain-timestamp outside freshness window' };
  }

  let expectedPubkey;
  try {
    expectedPubkey = trustedPubkeyForSlug(slugHeader);
  } catch (e) {
    return { status: 503, error: `relay misconfigured: ${e.message}` };
  }
  if (!expectedPubkey) {
    return { status: 401, error: 'slug is not in the trusted peer list' };
  }

  let sigBuf;
  try {
    sigBuf = Buffer.from(sigHeader, 'base64');
  } catch {
    return { status: 401, error: 'malformed x-annex-drain-signature' };
  }
  if (sigBuf.length !== 64) {
    return { status: 401, error: 'invalid x-annex-drain-signature length' };
  }

  let pubKey;
  try {
    const spki = Buffer.concat([ED25519_SPKI_PREFIX, Buffer.from(expectedPubkey, 'hex')]);
    pubKey = createPublicKey({ key: spki, format: 'der', type: 'spki' });
  } catch {
    return { status: 503, error: 'relay misconfigured: trusted pubkey is not a valid Ed25519 key' };
  }

  const canonical = `drain|${slugHeader}|${ts}`;
  let ok = false;
  try {
    ok = cryptoVerify(null, Buffer.from(canonical, 'utf-8'), pubKey, sigBuf);
  } catch {
    ok = false;
  }
  if (!ok) {
    return { status: 401, error: 'GET drain signature failed verification' };
  }

  // Replay defence on drains: bind to (slug, ts, sig) so a captured
  // header set can't be reused inside the freshness window.
  if (!recordReplayFingerprint(`drain:${slugHeader}`, 'drain', sigHeader)) {
    return { status: 409, error: 'replayed GET drain' };
  }
  return null;
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
      // (1) Authentication.
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

      // (2) Authorization via trust map. The relay must reject signed
      // payloads from a key the operator has not authorised for the
      // claimed `from_server_slug`. Missing/empty trust map under
      // production is a 503 (the operator has not configured the
      // relay) — never a silent accept.
      let trustedPubkey;
      try {
        if (trustMapEntryCount() === 0) {
          res.status(503).json({
            error:
              'relay misconfigured: ANNEX_SIGNAL_TRUSTED_PEERS is empty under production profile',
          });
          return;
        }
        trustedPubkey = trustedPubkeyForSlug(payload.from_server_slug);
      } catch (e) {
        res.status(503).json({ error: `relay misconfigured: ${e.message}` });
        return;
      }
      if (!trustedPubkey) {
        res
          .status(401)
          .json({ error: 'from_server_slug is not in the trusted peer list' });
        return;
      }
      if (trustedPubkey !== payload.from_pubkey_hex.toLowerCase()) {
        res.status(401).json({
          error: 'from_pubkey_hex does not match the trusted key for this from_server_slug',
        });
        return;
      }

      // (3) Replay defence. The signature itself binds the timestamp,
      // so a payload that survived signature + freshness but matches a
      // recently-seen fingerprint is a replay attempt — reject.
      if (
        !recordReplayFingerprint(
          payload.session_id,
          payload.sdp_type,
          payload.vrp_signature,
        )
      ) {
        res.status(409).json({ error: 'replayed signaling envelope' });
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

    const slug = String(req.query?.slug || '').trim().toLowerCase();
    const waitSecondsRaw = Number(req.query?.wait ?? 25);
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

    if (isProd) {
      // Production drains MUST prove ownership of the receiving
      // server's signing key. Knowledge of the slug alone is not
      // sufficient — without this check, anyone who learns a slug
      // can drain its queue and intercept federation bootstrap.
      const drainError = verifyGetDrainAuth(slug, req.headers || {});
      if (drainError) {
        res.status(drainError.status).json({ error: drainError.error });
        return;
      }
    }

    // Always attempt at least one drain before sleeping, so `wait=0`
    // returns whatever is queued right now (an immediate-mode poll)
    // rather than 204'ing past a fresh enqueue. The polling loop only
    // kicks in for `wait > 0`.
    const deadline = Date.now() + waitSeconds * 1000;
    do {
      const payload = dequeueSignal(slug);
      if (payload) {
        res.status(200).json(payload);
        return;
      }
      if (Date.now() >= deadline) break;
      await new Promise((resolve) => setTimeout(resolve, 250));
    } while (Date.now() < deadline);

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
  verifyGetDrainAuth,
  parseTrustedPeers,
  trustedPubkeyForSlug,
  recordReplayFingerprint,
  SLUG_REGEX,
  HEX64_REGEX,
  SESSION_ID_REGEX,
  POST_LIMIT_PER_MIN,
  GET_LIMIT_PER_MIN,
  FRESHNESS_WINDOW_MS,
  resetState() {
    signalQueues.clear();
    rateState.clear();
    replayCache.clear();
  },
};

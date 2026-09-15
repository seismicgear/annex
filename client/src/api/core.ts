/**
 * Shared HTTP infrastructure for the Annex API client.
 *
 * Holds module-private state (base URL, session token, ZK proof payload),
 * exposes fetch helpers (`request`, `requestRemote`, `fetchWithTimeout`), and
 * provides the auth header builder used by every domain module.
 *
 * Domain modules under `@/api/*` should import what they need from this file
 * rather than reaching into the legacy `@/lib/api` re-export.
 */

/** Base error class for API responses. */
export class ApiError extends Error {
  status: number;
  /**
   * The unparsed response body, kept for diagnostics. `message` is the
   * human-readable form; anything shown to a user should use `message`.
   */
  rawBody: string;
  constructor(status: number, message: string, rawBody = message) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.rawBody = rawBody;
  }
}

/** Last-resort wording when the server gives us nothing to work with. */
const STATUS_FALLBACKS: Record<number, string> = {
  400: 'The server rejected that request.',
  401: 'Your session is no longer valid. Try signing in again.',
  403: 'You do not have permission to do that.',
  404: 'That item no longer exists.',
  409: 'That conflicts with the current state on the server.',
  413: 'That file is too large for this server’s limits.',
  429: 'Too many requests — please wait a moment and try again.',
  500: 'The server hit an unexpected error.',
  503: 'That feature is not available on this server right now.',
  507: 'The server is out of storage and cannot accept writes.',
};

/**
 * Turns a raw error response into something worth showing a person.
 *
 * The backend does not speak one error dialect: most handlers return
 * `{"error": "..."}`, the channel routes return a bare status code with an
 * EMPTY body, and voice-join returns a JSON-shaped body with a
 * `text/plain` content type. Passing the raw body straight through meant
 * users saw things like
 *   {"error":"nullifier already exists for topic 'annex:server:…:v2'"}
 * or, on the channel routes, an empty string. Normalising here gives every
 * caller one predictable `message` regardless of which dialect replied.
 */
export function extractErrorMessage(status: number, body: string): string {
  const trimmed = body.trim();
  if (trimmed) {
    try {
      const parsed: unknown = JSON.parse(trimmed);
      if (typeof parsed === 'string' && parsed.trim()) return parsed.trim();
      if (parsed && typeof parsed === 'object') {
        const obj = parsed as Record<string, unknown>;
        // `error` is the common field; `message` is used by the voice-join
        // structured error alongside `error` as a machine-readable code.
        for (const key of ['message', 'error'] as const) {
          const value = obj[key];
          if (typeof value === 'string' && value.trim()) return value.trim();
        }
      }
    } catch {
      // Not JSON — a plain-text body is already human-readable enough.
      return trimmed;
    }
  }
  return STATUS_FALLBACKS[status] ?? `Request failed (HTTP ${status}).`;
}

/**
 * The (server, identity) pairing that a credential belongs to.
 *
 * A bearer token is minted by ONE server for ONE identity; a membership proof
 * names one server's Merkle root and carries one nullifier; an attachment
 * grant is a signed capability for one server's storage. None of them mean
 * anything anywhere else, and presenting one to a second server DISCLOSES it —
 * a bearer token is all that server needs to act as the identity on the first.
 *
 * So every credential in this module is stamped with the serial of the context
 * it was established in, and a change of server or identity mints a new
 * context. Two rules follow, and both are load-bearing:
 *
 *   - `authHeaders` will not emit a credential stamped with a stale serial.
 *   - every ASYNCHRONOUS credential write re-reads the serial it captured at
 *     entry and declines to mutate if it has moved.
 *
 * The second rule is the one that was missing. `refreshSessionToken()` awaited
 * the network and then assigned the result to the global token unconditionally;
 * the only generation check lived in `startTokenRefresh`, downstream of that
 * assignment. Start a refresh against server A, switch to B while it is in
 * flight, let A answer: the active target is B and the next `Authorization`
 * header carries A's newly refreshed token. Pinned by
 * `credential-context.test.ts`.
 */
interface CredentialContext {
  readonly serial: number;
  /** Empty string = current origin (relative paths). Otherwise a URL prefix. */
  readonly baseUrl: string;
  readonly pseudonymId: string | null;
}

let _context: CredentialContext = { serial: 0, baseUrl: '', pseudonymId: null };

/**
 * Establish a new credential context, invalidating everything minted for the
 * previous one. A no-op when neither the server nor the identity has changed,
 * so a redundant `setApiBaseUrl` with the same URL does not destroy a live
 * session.
 *
 * Returns the current context either way, so callers can capture it.
 */
function enterContext(baseUrl: string, pseudonymId: string | null): CredentialContext {
  if (baseUrl === _context.baseUrl && pseudonymId === _context.pseudonymId) {
    return _context;
  }
  _context = { serial: _context.serial + 1, baseUrl, pseudonymId };

  _sessionToken = null;
  _sessionTokenSerial = -1;
  _zkProofPayload = null;
  _zkProofSerial = -1;
  clearUploadGrant();

  // Cancel the auto-refresh loop and any pending retry. A timer scheduled for
  // the old context would otherwise fire with no credential to refresh, and
  // (before this) would have refreshed the wrong one.
  _refreshGeneration++;
  if (_refreshTimer !== null) {
    clearTimeout(_refreshTimer);
    _refreshTimer = null;
  }

  return _context;
}

/**
 * Thrown by an asynchronous credential operation whose context was replaced
 * while it was in flight.
 *
 * A distinct type rather than a generic `Error` because the callers treat it
 * differently: a refresh that FAILED means the session is over and the user
 * should be offered re-registration, while a refresh that was SUPERSEDED means
 * someone else already owns the session and there is nothing to report.
 * Collapsing the two is defect class 6 in CLAUDE.md, and it would have shown
 * the user "your session expired" every time they switched servers.
 */
export class StaleCredentialContextError extends Error {
  constructor(what: string) {
    super(`${what} was superseded: its server/identity context is no longer current.`);
    this.name = 'StaleCredentialContextError';
  }
}

/** The active credential context. Exported for callers that outlive an await. */
export function getCredentialContext(): CredentialContext {
  return _context;
}

/**
 * True when `ctx` is still the context every credential is being issued under.
 * Callers that await anything before touching identity-scoped state must check
 * this — including state outside this module, such as the IndexedDB identity
 * record that `useSessionConnection` writes a refreshed token into.
 */
export function isCredentialContextCurrent(ctx: CredentialContext): boolean {
  return ctx.serial === _context.serial;
}

/**
 * HMAC-signed session token for authenticated API calls.
 * Set after ZK verify-membership or loaded from IndexedDB on cold start.
 * Used as `Authorization: Bearer <token>` when enforce_zk_proofs is enabled.
 */
let _sessionToken: string | null = null;
/** The context serial `_sessionToken` was minted for. -1 when there is none. */
let _sessionTokenSerial = -1;

/**
 * Cached ZK membership proof payload (JSON string of { proof, publicSignals }).
 * Set after successful registration/verification. Sent as `x-annex-zk-proof`
 * on routes that require `verify_zk_membership_header`.
 */
let _zkProofPayload: string | null = null;
/** The context serial `_zkProofPayload` was established in. */
let _zkProofSerial = -1;

/** Auto-refresh interval handle. */
let _refreshTimer: ReturnType<typeof setTimeout> | null = null;
/**
 * Bumped by every `start`/`stop`. A refresh already awaiting the network
 * when the session tears down would otherwise reschedule itself onto a
 * session that no longer exists.
 */
let _refreshGeneration = 0;

/**
 * Retry delays after a failed refresh, as fractions of the token's REMAINING
 * validity (the 20% of the TTL still on the clock when the refresh fires).
 *
 * Expressing them as fractions rather than fixed seconds keeps every retry
 * inside that window whatever the TTL is: they sum to 0.9375 of it, so the
 * last attempt lands just before the token actually dies. The refresh
 * endpoint accepts expired-but-validly-signed tokens, so retrying with the
 * same credential is exactly what the server expects.
 */
const REFRESH_RETRY_FRACTIONS = [1 / 16, 1 / 8, 1 / 4, 1 / 2];

/**
 * Point the client at a server. Empty string for the current origin.
 *
 * Changing the target ENDS the current credential context: the session token,
 * the membership proof and the attachment grant were all issued by the server
 * being left, and none of them may be presented to the one being entered.
 *
 * The corollary is that a caller who changes the target must re-assert the
 * credentials for the new one — `useIdentityStore.selectIdentity()` /
 * `reassertCredentials()` do this, and every call site here does one of them.
 * Re-setting the SAME URL (trailing slashes normalised) is a no-op, so the
 * redundant calls on the startup path do not tear down a live session.
 */
export function setApiBaseUrl(baseUrl: string): void {
  const normalized = baseUrl.replace(/\/+$/, '');
  // The identity resets to null: which identity belongs to the new server is
  // not known here, and carrying the previous one forward would let a token
  // minted for it look current.
  if (normalized !== _context.baseUrl) {
    enterContext(normalized, null);
  }
}

/** Get the current API base URL. */
export function getApiBaseUrl(): string {
  return _context.baseUrl;
}

/**
 * The current attachment grant, and when it stops being usable.
 *
 * Chat attachments are no longer served by a public static mount — they need a
 * short-lived signed grant, because a browser cannot attach an `Authorization`
 * header to `<img src>`. See `crates/annex-server/src/api_uploads_access.rs`.
 *
 * Held in a module-level cache rather than fetched per image: a channel with
 * thirty pictures would otherwise mint thirty grants on scroll.
 */
/**
 * The pseudonym the current session belongs to.
 *
 * `authHeaders` needs it for the `X-Annex-Pseudonym` fallback used when no
 * session token is held (dev and the e2e harness, where `enforce_zk_proofs` is
 * off). Tracked here so `ensureUploadGrant` — which is reached from `resolveUrl`
 * during render, with no caller to pass it — can authenticate the same way
 * every other call does.
 *
 * The first version of the grant fetch used bare `request()`, which attaches no
 * credentials at all. The UI audit caught it: 14 `request-failed` findings,
 * `POST /api/uploads/grant — HTTP 401`, and every attachment a broken image.
 *
 * It lives on the credential context rather than in a variable of its own, so
 * that "which identity" and "which server" move together and cannot drift.
 */

let _uploadGrant: string | null = null;
let _uploadGrantExpiresAt = 0;
/** The context serial the cached grant belongs to. */
let _uploadGrantSerial = -1;
/**
 * The in-flight grant fetch, and the context it was started for.
 *
 * Tagged rather than bare: a caller in a NEW context that awaited the old
 * promise would be told a grant had arrived when the one that arrived belongs
 * to the server it just left.
 */
let _uploadGrantInFlight: { ctx: CredentialContext; promise: Promise<void> } | null = null;

/**
 * Bumped whenever the cached grant changes.
 *
 * `resolveUrl` is called during render and cannot await, so the first paint
 * after a cold start emits an unsigned URL. Something has to tell React to
 * render again once the grant lands — and the first version of this assumed
 * "the state update when the grant arrives re-renders it", which was simply
 * not true: the grant lives in a module variable and no state was touched.
 *
 * The UI audit found it. `/api/uploads/grant` stopped 401-ing and the images
 * kept 401-ing, on exactly the surface (`message-image-lightbox`) whose
 * attachments render on first paint.
 */
let _uploadGrantVersion = 0;
const _uploadGrantListeners = new Set<() => void>();

function notifyUploadGrantChanged(): void {
  _uploadGrantVersion += 1;
  for (const listener of _uploadGrantListeners) listener();
}

/** `useSyncExternalStore` subscribe half. */
export function subscribeUploadGrant(listener: () => void): () => void {
  _uploadGrantListeners.add(listener);
  return () => {
    _uploadGrantListeners.delete(listener);
  };
}

/** `useSyncExternalStore` snapshot half. */
export function getUploadGrantVersion(): number {
  return _uploadGrantVersion;
}

/** Cleared on sign-out and on identity switch, so a grant cannot outlive its owner. */
export function clearUploadGrant(): void {
  const had = _uploadGrant !== null;
  _uploadGrant = null;
  _uploadGrantExpiresAt = 0;
  _uploadGrantSerial = -1;
  _uploadGrantInFlight = null;
  if (had) notifyUploadGrantChanged();
}

/**
 * Ensure a usable attachment grant is cached.
 *
 * Refreshed a minute early so a render that begins just before expiry does not
 * produce broken images. Concurrent callers share one request — a channel
 * switch renders many attachments at once, and without this they would each
 * start their own.
 */
export async function ensureUploadGrant(): Promise<void> {
  const ctx = _context;
  const now = Date.now();
  if (_uploadGrant && _uploadGrantSerial === ctx.serial && now < _uploadGrantExpiresAt - 60_000) {
    return;
  }
  // Share an in-flight fetch only with callers in the SAME context.
  if (_uploadGrantInFlight && _uploadGrantInFlight.ctx.serial === ctx.serial) {
    return _uploadGrantInFlight.promise;
  }

  const promise = (async () => {
    try {
      const resp = await request<{ token: string; expiresInSecs: number }>(
        '/api/uploads/grant',
        {
          method: 'POST',
          body: '{}',
          headers: authHeaders(ctx.pseudonymId ?? ''),
        },
      );
      // The server may have changed under us while this was out. A grant is a
      // signed capability for the storage of the server that minted it; caching
      // it now would append it to URLs pointed somewhere else.
      if (!isCredentialContextCurrent(ctx)) return;
      _uploadGrant = resp.token;
      _uploadGrantExpiresAt = Date.now() + resp.expiresInSecs * 1000;
      _uploadGrantSerial = ctx.serial;
      notifyUploadGrantChanged();
    } catch {
      // A failed grant must not break the app: the images 401 and the next
      // render tries again. Throwing here would take the message list with it.
      if (!isCredentialContextCurrent(ctx)) return;
      _uploadGrant = null;
      _uploadGrantExpiresAt = 0;
      _uploadGrantSerial = -1;
    } finally {
      if (_uploadGrantInFlight?.ctx.serial === ctx.serial) {
        _uploadGrantInFlight = null;
      }
    }
  })();
  _uploadGrantInFlight = { ctx, promise };
  return promise;
}

/** Chat attachments need a grant; server branding is public. */
function needsUploadGrant(path: string): boolean {
  return path.startsWith('/uploads/chat/');
}

/**
 * Resolve a relative path against the API base URL.
 *
 * When the app is loaded from a Tauri bundle (`tauri://localhost`), relative
 * paths like `/uploads/abc.png` would resolve against the Tauri origin and
 * fail. This helper ensures they resolve against the server instead.
 *
 * Chat attachment paths additionally get the cached grant appended. This is
 * the one place every attachment URL passes through, which is why the grant
 * goes here rather than at each of the three call sites in `MessageView`.
 *
 * Absolute URLs (http/https) are returned unchanged.
 */
export function resolveUrl(path: string): string {
  if (!path || path.startsWith('http://') || path.startsWith('https://')) {
    return path;
  }
  const base = _context.baseUrl ? `${_context.baseUrl}${path}` : path;
  if (!needsUploadGrant(path)) return base;

  // Kick off a refresh if the cache is cold or stale. Synchronous on purpose —
  // `resolveUrl` is called from render. A first paint before the grant arrives
  // emits an unsigned URL; `subscribeUploadGrant` is what makes the component
  // render again once it lands. Consumers that render attachments must use
  // `useUploadGrant()`, or they will keep the unsigned URL forever.
  void ensureUploadGrant();
  if (!_uploadGrant || _uploadGrantSerial !== _context.serial) return base;
  const sep = base.includes('?') ? '&' : '?';
  return `${base}${sep}t=${encodeURIComponent(_uploadGrant)}`;
}

/**
 * Fetch with a bounded timeout using AbortController.
 * @param url - The request URL
 * @param init - Fetch init options
 * @param timeoutMs - Timeout in milliseconds (default: none)
 */
export async function fetchWithTimeout(
  url: string,
  init?: RequestInit,
  timeoutMs?: number,
): Promise<Response> {
  if (!timeoutMs) return fetch(url, init);

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } catch (err) {
    if (err instanceof DOMException && err.name === 'AbortError') {
      throw new Error(`Request timed out after ${timeoutMs}ms`);
    }
    throw err;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * The request headers, with a JSON content type when the request carries a
 * body and the caller has not named one itself.
 *
 * Shared because `request` and `requestRemote` held byte-identical copies of
 * it, and the pair has already drifted once elsewhere — see `throwApiError`.
 * The upload helpers deliberately do not use this: a multipart body must not
 * be labelled JSON.
 */
function jsonHeaders(options?: RequestInit): Headers {
  const method = (options?.method ?? 'GET').toUpperCase();
  const hasBody = options?.body !== undefined && options?.body !== null;
  const headers = new Headers(options?.headers);
  if (
    ['POST', 'PUT', 'PATCH', 'DELETE'].includes(method) &&
    hasBody &&
    !headers.has('Content-Type')
  ) {
    headers.set('Content-Type', 'application/json');
  }
  return headers;
}

export async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const url = _context.baseUrl ? `${_context.baseUrl}${path}` : path;
  const res = await fetch(url, {
    ...options,
    headers: jsonHeaders(options),
  });
  if (!res.ok) await throwApiError(res);
  return res.json() as Promise<T>;
}

/**
 * Turn a non-ok `Response` into an `ApiError` carrying a message a person can
 * read, and never return.
 *
 * Extracted from `request` because the upload helpers cannot use it — a
 * multipart body must not get a JSON `Content-Type` — and so threw
 * `new ApiError(status, await res.text())` instead. That put the raw response
 * body in `err.message`, and the composer renders it: an upload rejected by
 * the storage gate showed the user
 * `Upload failed: {"error":"storage unavailable"}`, while every other request
 * in the app decoded the same body to `storage unavailable`. Uploads also
 * missed the 429 handling, so a rate-limited attachment reported raw JSON
 * where the rest of the app says when to try again.
 */
export async function throwApiError(res: Response): Promise<never> {
  // Enhance rate limit errors with Retry-After guidance
  if (res.status === 429) {
    const retryAfter = res.headers.get('Retry-After');
    const waitMsg = retryAfter ? ` Try again in ${retryAfter} seconds.` : ' Please wait and try again.';
    throw new ApiError(429, `Rate limit exceeded.${waitMsg}`);
  }
  const body = await res.text();
  throw new ApiError(res.status, extractErrorMessage(res.status, body), body);
}

/**
 * Fetch from a specific remote server (for federation hopping / discovery).
 * Does NOT use the active credential context’s base URL — targets the given
 * URL directly, and attaches no credentials of its own.
 */
export async function requestRemote<T>(
  baseUrl: string,
  path: string,
  options?: RequestInit,
): Promise<T> {
  const url = `${baseUrl.replace(/\/+$/, '')}${path}`;
  const res = await fetch(url, {
    ...options,
    headers: jsonHeaders(options),
  });
  // Through `throwApiError`, not an inline throw.
  //
  // This used to build the `ApiError` itself, which is the same thing for
  // every status except 429: `throwApiError` short-circuits that one before
  // reading the body and adds the `Retry-After` seconds, while
  // `extractErrorMessage` prefers whatever the body says and never sees the
  // header. So a rate-limited request to a remote server — federation
  // discovery is all remote — told the user to wait without saying how long,
  // and said it in different words from every other request in the app.
  //
  // That is the same gap `throwApiError` was extracted to close for the
  // upload helpers, as its own doc comment says. This caller was left behind.
  if (!res.ok) await throwApiError(res);
  return res.json() as Promise<T>;
}

/**
 * Establish the session credential for an identity on the current server.
 *
 * The pseudonym is REQUIRED rather than optional, so the compiler enumerates
 * every call site. There are eleven in the identity store alone, and the
 * attachment grant is a credential for one identity — a site that set a new
 * token while leaving the old pseudonym behind would mint grants naming the
 * wrong person. An optional parameter would have compiled everywhere and been
 * wrong in the places nobody revisited.
 *
 * Changing the identity mints a new context, so anything still in flight for
 * the previous one (a token refresh, a grant fetch) finds its context gone and
 * declines to write. `setSessionToken(null, null)` is therefore a complete
 * sign-out: it invalidates the token, the proof, the grant and the refresh
 * loop in one move.
 */
export function setSessionToken(token: string | null, pseudonymId: string | null): void {
  const ctx = enterContext(_context.baseUrl, pseudonymId);
  // An identity re-asserting the same pseudonym with a different token (a
  // completed refresh, a fresh verify-membership) does not change context, but
  // the grant was minted against the old token and has to go.
  if (token !== _sessionToken) {
    clearUploadGrant();
  }
  _sessionToken = token;
  _sessionTokenSerial = token === null ? -1 : ctx.serial;

  // Warm the grant as soon as there is a session to mint one against, rather
  // than waiting for the first attachment to render. Belt and braces with the
  // subscription above: this removes the race in the common path, and the
  // subscription covers the rest.
  if (token !== null || pseudonymId !== null) {
    void ensureUploadGrant();
  }
}

/** Get the current session token, or null if none is current. */
export function getSessionToken(): string | null {
  return _sessionTokenSerial === _context.serial ? _sessionToken : null;
}

/** The pseudonym the current session belongs to, if any. */
export function getCurrentPseudonym(): string | null {
  return _context.pseudonymId;
}

/**
 * Cache the latest ZK proof payload for use in protected API calls.
 *
 * Stamped with the context like the token: a membership proof names one
 * server's Merkle root and carries a nullifier scoped to that server's topic.
 * Sending it elsewhere leaks the commitment and the nullifier to a server that
 * has no business seeing either.
 */
export function setZkProofPayload(payload: string | null): void {
  _zkProofPayload = payload;
  _zkProofSerial = payload === null ? -1 : _context.serial;
}

/** Get the cached ZK proof payload, or null if it belongs to another context. */
export function getZkProofPayload(): string | null {
  return _zkProofSerial === _context.serial ? _zkProofPayload : null;
}

/**
 * Check whether an HMAC session token has expired.
 * Token format: base64(pseudonym|expires_unix_secs|hmac_signature)
 */
export function isTokenExpired(token: string): boolean {
  try {
    const decoded = atob(token.replace(/-/g, '+').replace(/_/g, '/'));
    const parts = decoded.split('|');

    // Two layouts, and reading the wrong one is not a parse error — it is a
    // plausible number in the wrong position.
    //
    //   v1: pseudonym|expires|signature            (3 fields)
    //   v2: pseudonym|epoch|expires|signature      (4 fields)
    //
    // This only understood v1 and returned `true` for anything else, so once
    // the server started minting v2 every token read as ALREADY EXPIRED and
    // the client refreshed on every check. Had it instead been written to take
    // `parts[1]` regardless, it would have read the EPOCH as a unix timestamp
    // — 0 for a never-revoked identity, i.e. expired in 1970 — which is the
    // same failure with a more convincing cause.
    let expiresField: string | undefined;
    if (parts.length === 4) {
      expiresField = parts[2];
    } else if (parts.length === 3) {
      expiresField = parts[1];
    } else {
      return true;
    }

    const expires = parseInt(expiresField, 10);
    if (isNaN(expires)) return true;
    // Treat as expired 30 seconds early to avoid edge-case races
    return Date.now() / 1000 >= expires - 30;
  } catch {
    return true;
  }
}

/**
 * Refresh the session token using the current valid Bearer auth.
 * Calls POST /api/session/refresh which accepts expired-but-validly-signed tokens.
 */
export async function refreshSessionToken(): Promise<string> {
  const ctx = _context;
  const token = getSessionToken();
  if (!token) {
    throw new Error('No session token to refresh');
  }
  // Built from the CAPTURED context, so the request cannot be re-pointed at a
  // different server between here and the fetch.
  const url = ctx.baseUrl ? `${ctx.baseUrl}/api/session/refresh` : '/api/session/refresh';
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Authorization': `Bearer ${token}` },
  });
  if (!res.ok) {
    const body = await res.text();
    throw new ApiError(res.status, extractErrorMessage(res.status, body), body);
  }
  const data = await res.json() as { sessionToken: string };

  // Validate BEFORE mutating. This is the whole point: the check used to live
  // downstream in `startTokenRefresh`, by which time the global token had
  // already been overwritten with a credential for a server the user had left.
  if (!isCredentialContextCurrent(ctx)) {
    throw new StaleCredentialContextError('the session refresh');
  }
  _sessionToken = data.sessionToken;
  _sessionTokenSerial = ctx.serial;
  return data.sessionToken;
}

/**
 * Start auto-refreshing the session token at 80% of the given TTL, retrying
 * inside the remaining 20% if an attempt fails. Call stopTokenRefresh() to
 * cancel.
 *
 * The retries are the point. A plain interval that shrugged off a failure
 * would not try again until a full cycle later — 48 minutes for the standard
 * 1-hour TTL — by which time the token has been dead for 36 of them, with
 * every API call 401-ing behind a UI that still looks signed in. Now a
 * transient failure is retried while the credential is still refreshable,
 * and `onError` fires only once the retries are exhausted, so callers can
 * treat it as "this session is over" rather than "one request failed".
 */
export function startTokenRefresh(
  ttlSecs: number,
  onRefreshed?: (newToken: string) => void,
  onError?: (err: unknown) => void,
): void {
  stopTokenRefresh();
  const generation = ++_refreshGeneration;
  const ctx = _context;
  const cycleMs = ttlSecs * 0.8 * 1000;
  const remainingMs = ttlSecs * 0.2 * 1000;

  const schedule = (delayMs: number, attempt: number) => {
    _refreshTimer = setTimeout(async () => {
      _refreshTimer = null;
      // The loop belongs to one context. A server or identity change already
      // bumped `_refreshGeneration`, but check the context explicitly too: the
      // two are separate facts and a future caller could move one without the
      // other.
      if (generation !== _refreshGeneration || !isCredentialContextCurrent(ctx)) return;
      let newToken: string;
      try {
        newToken = await refreshSessionToken();
      } catch (err) {
        if (generation !== _refreshGeneration || !isCredentialContextCurrent(ctx)) return;
        // A superseded refresh is not a failed one — there is no session left
        // to retry for, and nothing to report.
        if (err instanceof StaleCredentialContextError) return;
        if (attempt < REFRESH_RETRY_FRACTIONS.length) {
          schedule(remainingMs * REFRESH_RETRY_FRACTIONS[attempt], attempt + 1);
        } else {
          onError?.(err);
        }
        return;
      }
      if (generation !== _refreshGeneration || !isCredentialContextCurrent(ctx)) return;
      onRefreshed?.(newToken);
      schedule(cycleMs, 0);
    }, delayMs);
  };

  schedule(cycleMs, 0);
}

/** Stop auto-refreshing the session token, including any pending retry. */
export function stopTokenRefresh(): void {
  _refreshGeneration++;
  if (_refreshTimer !== null) {
    clearTimeout(_refreshTimer);
    _refreshTimer = null;
  }
}

/** UTF-8-safe base64 (btoa only handles Latin-1). */
function toBase64Utf8(s: string): string {
  const bytes = new TextEncoder().encode(s);
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

/**
 * Build the standard auth header set for an authenticated request.
 * Prefers the HMAC session token; falls back to the pseudonym header for
 * unauthenticated/legacy paths. Always includes the cached ZK proof when
 * available so routes that require `verify_zk_membership_header` succeed.
 *
 * The server's `verify_zk_membership_header` base64-decodes the
 * `x-annex-zk-proof` header and deserializes it as a full `ZkProofPayload`
 * (proof + root_hex + commitment_hex [+ protocolVersion/publicSignals]), so
 * the cached payload MUST be base64-encoded here — sending raw JSON makes the
 * base64 decode fail and the server rejects every join/send with 403.
 */
export function authHeaders(pseudonymId: string): Record<string, string> {
  const headers: Record<string, string> = {};
  // Through the accessors on purpose: they return null for a credential minted
  // in a context that is no longer current, which is the last line of defence
  // against a stale token reaching a server that never issued it.
  const token = getSessionToken();
  const proof = getZkProofPayload();
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  } else {
    headers['X-Annex-Pseudonym'] = pseudonymId;
  }
  if (proof) {
    headers['x-annex-zk-proof'] = toBase64Utf8(proof);
  }
  return headers;
}

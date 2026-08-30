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
 * Active base URL for multi-server connections.
 * Empty string = current origin (relative paths). Otherwise, full URL prefix.
 */
let _apiBaseUrl = '';

/**
 * HMAC-signed session token for authenticated API calls.
 * Set after ZK verify-membership or loaded from IndexedDB on cold start.
 * Used as `Authorization: Bearer <token>` when enforce_zk_proofs is enabled.
 */
let _sessionToken: string | null = null;

/**
 * Cached ZK membership proof payload (JSON string of { proof, publicSignals }).
 * Set after successful registration/verification. Sent as `x-annex-zk-proof`
 * on routes that require `verify_zk_membership_header`.
 */
let _zkProofPayload: string | null = null;

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

/** Set the API base URL for cross-server requests. Empty string for current origin. */
export function setApiBaseUrl(baseUrl: string): void {
  _apiBaseUrl = baseUrl.replace(/\/+$/, '');
}

/** Get the current API base URL. */
export function getApiBaseUrl(): string {
  return _apiBaseUrl;
}

/**
 * Resolve a relative path against the API base URL.
 *
 * When the app is loaded from a Tauri bundle (`tauri://localhost`), relative
 * paths like `/uploads/abc.png` would resolve against the Tauri origin and
 * fail. This helper ensures they resolve against the server instead.
 *
 * Absolute URLs (http/https) are returned unchanged.
 */
export function resolveUrl(path: string): string {
  if (!path || path.startsWith('http://') || path.startsWith('https://')) {
    return path;
  }
  return _apiBaseUrl ? `${_apiBaseUrl}${path}` : path;
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
  const url = _apiBaseUrl ? `${_apiBaseUrl}${path}` : path;
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
 * Does NOT use the global _apiBaseUrl — targets the given URL directly.
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

/** Set the session token (after verify-membership or token refresh). */
export function setSessionToken(token: string | null): void {
  _sessionToken = token;
}

/** Get the current session token. */
export function getSessionToken(): string | null {
  return _sessionToken;
}

/** Cache the latest ZK proof payload for use in protected API calls. */
export function setZkProofPayload(payload: string | null): void {
  _zkProofPayload = payload;
}

/** Get the cached ZK proof payload. */
export function getZkProofPayload(): string | null {
  return _zkProofPayload;
}

/**
 * Check whether an HMAC session token has expired.
 * Token format: base64(pseudonym|expires_unix_secs|hmac_signature)
 */
export function isTokenExpired(token: string): boolean {
  try {
    const decoded = atob(token.replace(/-/g, '+').replace(/_/g, '/'));
    const parts = decoded.split('|');
    if (parts.length !== 3) return true;
    const expires = parseInt(parts[1], 10);
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
  if (!_sessionToken) {
    throw new Error('No session token to refresh');
  }
  const url = _apiBaseUrl ? `${_apiBaseUrl}/api/session/refresh` : '/api/session/refresh';
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Authorization': `Bearer ${_sessionToken}` },
  });
  if (!res.ok) {
    const body = await res.text();
    throw new ApiError(res.status, extractErrorMessage(res.status, body), body);
  }
  const data = await res.json() as { sessionToken: string };
  _sessionToken = data.sessionToken;
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
  const cycleMs = ttlSecs * 0.8 * 1000;
  const remainingMs = ttlSecs * 0.2 * 1000;

  const schedule = (delayMs: number, attempt: number) => {
    _refreshTimer = setTimeout(async () => {
      _refreshTimer = null;
      let newToken: string;
      try {
        newToken = await refreshSessionToken();
      } catch (err) {
        if (generation !== _refreshGeneration) return;
        if (attempt < REFRESH_RETRY_FRACTIONS.length) {
          schedule(remainingMs * REFRESH_RETRY_FRACTIONS[attempt], attempt + 1);
        } else {
          onError?.(err);
        }
        return;
      }
      if (generation !== _refreshGeneration) return;
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
  if (_sessionToken) {
    headers['Authorization'] = `Bearer ${_sessionToken}`;
  } else {
    headers['X-Annex-Pseudonym'] = pseudonymId;
  }
  if (_zkProofPayload) {
    headers['x-annex-zk-proof'] = toBase64Utf8(_zkProofPayload);
  }
  return headers;
}

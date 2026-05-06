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
  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
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
let _refreshInterval: ReturnType<typeof setInterval> | null = null;

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

export async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const url = _apiBaseUrl ? `${_apiBaseUrl}${path}` : path;
  const method = (options?.method ?? 'GET').toUpperCase();
  const hasBody = options?.body !== undefined && options?.body !== null;
  const shouldSetJsonContentType =
    ['POST', 'PUT', 'PATCH', 'DELETE'].includes(method) && hasBody;
  const headers = new Headers(options?.headers);

  if (shouldSetJsonContentType && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  const res = await fetch(url, {
    ...options,
    headers,
  });
  if (!res.ok) {
    // Enhance rate limit errors with Retry-After guidance
    if (res.status === 429) {
      const retryAfter = res.headers.get('Retry-After');
      const waitMsg = retryAfter ? ` Try again in ${retryAfter} seconds.` : ' Please wait and try again.';
      throw new ApiError(429, `Rate limit exceeded.${waitMsg}`);
    }
    const body = await res.text();
    throw new ApiError(res.status, body);
  }
  return res.json() as Promise<T>;
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
  const method = (options?.method ?? 'GET').toUpperCase();
  const hasBody = options?.body !== undefined && options?.body !== null;
  const shouldSetJsonContentType =
    ['POST', 'PUT', 'PATCH', 'DELETE'].includes(method) && hasBody;
  const headers = new Headers(options?.headers);

  if (shouldSetJsonContentType && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  const res = await fetch(url, {
    ...options,
    headers,
  });
  if (!res.ok) {
    const body = await res.text();
    throw new ApiError(res.status, body);
  }
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
    throw new ApiError(res.status, body);
  }
  const data = await res.json() as { sessionToken: string };
  _sessionToken = data.sessionToken;
  return data.sessionToken;
}

/**
 * Start auto-refreshing the session token at 80% of the given TTL.
 * Call stopTokenRefresh() to cancel.
 */
export function startTokenRefresh(
  ttlSecs: number,
  onRefreshed?: (newToken: string) => void,
  onError?: (err: unknown) => void,
): void {
  stopTokenRefresh();
  const intervalMs = ttlSecs * 0.8 * 1000;
  _refreshInterval = setInterval(async () => {
    try {
      const newToken = await refreshSessionToken();
      onRefreshed?.(newToken);
    } catch (err) {
      onError?.(err);
    }
  }, intervalMs);
}

/** Stop auto-refreshing the session token. */
export function stopTokenRefresh(): void {
  if (_refreshInterval !== null) {
    clearInterval(_refreshInterval);
    _refreshInterval = null;
  }
}

/**
 * Build the standard auth header set for an authenticated request.
 * Prefers the HMAC session token; falls back to the pseudonym header for
 * unauthenticated/legacy paths. Always includes the cached ZK proof when
 * available so routes that require `verify_zk_membership_header` succeed.
 */
export function authHeaders(pseudonymId: string): Record<string, string> {
  const headers: Record<string, string> = {};
  if (_sessionToken) {
    headers['Authorization'] = `Bearer ${_sessionToken}`;
  } else {
    headers['X-Annex-Pseudonym'] = pseudonymId;
  }
  if (_zkProofPayload) {
    headers['x-annex-zk-proof'] = _zkProofPayload;
  }
  return headers;
}

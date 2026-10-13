/** A taken-over tab stays suspended across remounts and reloads until explicit reconnection. */
export const TAKEOVER_MESSAGE = 'This identity is active on another device.';
const suspended = new Set<string>();
const operations = new Map<string, Set<AbortController>>();

function contextKey(baseUrl: string, pseudonymId: string): string {
  const origin = typeof window === 'undefined' ? '' : (window.location.origin || `${window.location.protocol}//${window.location.host}`);
  return `annex:session-taken-over:${JSON.stringify([baseUrl.replace(/\/+$/, '') || origin, pseudonymId])}`;
}

export function isSessionTakenOver(baseUrl: string, pseudonymId: string): boolean {
  const key = contextKey(baseUrl, pseudonymId);
  try { if (sessionStorage.getItem(key) === '1') suspended.add(key); } catch { /* memory still protects this tab */ }
  return suspended.has(key);
}

export function markSessionTakenOver(baseUrl: string, pseudonymId: string): void {
  const key = contextKey(baseUrl, pseudonymId);
  suspended.add(key);
  try { sessionStorage.setItem(key, '1'); } catch { /* restricted storage */ }
  for (const operation of operations.get(key) ?? []) operation.abort(new Error(TAKEOVER_MESSAGE));
  operations.delete(key);
}

/** Call only from the user's Reconnect here action. */
export function reclaimSession(baseUrl: string, pseudonymId: string): void {
  const key = contextKey(baseUrl, pseudonymId);
  suspended.delete(key);
  try { sessionStorage.removeItem(key); } catch { /* restricted storage */ }
}

export function assertSessionOwnership(baseUrl: string, pseudonymId: string): void {
  if (isSessionTakenOver(baseUrl, pseudonymId)) throw new Error(TAKEOVER_MESSAGE);
}

/** Abort pending HTTP writes/uploads when the live session moves elsewhere. */
export function beginLiveOperation(baseUrl: string, pseudonymId: string) {
  assertSessionOwnership(baseUrl, pseudonymId);
  const key = contextKey(baseUrl, pseudonymId);
  const controller = new AbortController();
  const active = operations.get(key) ?? new Set<AbortController>();
  active.add(controller);
  operations.set(key, active);
  return {
    signal: controller.signal,
    release: () => {
      active.delete(controller);
      if (!active.size && operations.get(key) === active) operations.delete(key);
    },
  };
}

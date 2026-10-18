/**
 * Invite routing — parses invite links and orchestrates background
 * identity verification + server join.
 *
 * New invite URL format (monolithannex.com):
 *   https://monolithannex.com/invite/{base64url_payload}
 *   → deep-links to annex://invite?server={encoded}&code={encoded}
 *
 * The `annex://` half is parsed in `annex-desktop`, not here — the OS hands
 * the URL to the Tauri process, which validates it and emits the result as
 * an event. This module owns the http(s) links the browser sees.
 *
 * Legacy invite URL format (direct server links):
 *   https://<host>/invite/<channelId>?slug=<serverSlug>&label=<label>
 */

import type { InvitePayload, LegacyInvitePayload } from '@/types';

/** Decode a pasted share/deep link locally; only the destination redeems it. */
export function parseInviteLink(input: string): InvitePayload | null {
  const text = input.trim();
  let url: URL;
  try { url = new URL(text); } catch { return null; }
  const protocolLink = url.protocol === 'annex:' && url.hostname === 'invite';
  const shareLink = /^https?:$/.test(url.protocol) && url.pathname.startsWith('/invite/');
  if (!protocolLink && !shareLink) return null;
  const invalid = () => new Error('This invite link is incomplete or invalid. Ask the host for a new invite.');
  if (text.length > 8192 || url.username || url.password || url.hash) throw invalid();
  let value: unknown;
  if (protocolLink) {
    if (url.pathname && url.pathname !== '/') throw invalid();
    if (url.searchParams.getAll('server').length !== 1 || url.searchParams.getAll('code').length !== 1) throw invalid();
    value = { server: url.searchParams.get('server'), code: url.searchParams.get('code') };
  } else {
    const encoded = url.pathname.slice('/invite/'.length);
    if (url.protocol !== 'https:' || url.search || !/^[A-Za-z0-9_-]+$/.test(encoded)) throw invalid();
    try {
      const bytes = Uint8Array.from(atob(encoded.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
      value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
    } catch { throw invalid(); }
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalid();
  const { server, code } = value as Record<string, unknown>;
  if (typeof server !== 'string' || server.length > 2048 || typeof code !== 'string'
    || !code.trim() || code.length > 100
    || [...code].some(c => c.charCodeAt(0) <= 32 || c.charCodeAt(0) === 127)) throw invalid();
  let destination: URL;
  try { destination = new URL(server); } catch { throw invalid(); }
  if (destination.protocol !== 'https:' || destination.username || destination.password
    || destination.search || destination.hash) throw invalid();
  return { server: destination.href.replace(/\/+$/, ''), code };
}

/**
 * Parse an invite from the current window location.
 *
 * Supports legacy channel-based invite links for backwards compatibility.
 * Returns null if the current URL is not an invite link.
 */
export function parseLegacyInviteFromUrl(): LegacyInvitePayload | null {
  const url = new URL(window.location.href);
  const pathParts = url.pathname.split('/').filter(Boolean);

  // Match /invite/<channelId>
  if (pathParts.length >= 2 && pathParts[0] === 'invite') {
    const channelId = pathParts[1];
    const serverSlug = url.searchParams.get('slug') ?? 'default';
    const label = url.searchParams.get('label') ?? undefined;

    return {
      server: url.host,
      channelId,
      serverSlug,
      label,
    };
  }

  // Also check for hash-based invites: #/invite/<channelId>
  if (url.hash.startsWith('#/invite/')) {
    const hashParts = url.hash.slice(2).split('/').filter(Boolean);
    if (hashParts.length >= 2) {
      const channelId = hashParts[1];
      const hashParams = new URLSearchParams(url.hash.split('?')[1] ?? '');
      return {
        server: url.host,
        channelId,
        serverSlug: hashParams.get('slug') ?? 'default',
        label: hashParams.get('label') ?? undefined,
      };
    }
  }

  return null;
}

/**
 * Create a new invite by calling the server API.
 *
 * Uses the shared auth plumbing from api.ts so the Bearer token and any
 * required proof headers are sent consistently (required for secure desktop
 * builds where X-Annex-Pseudonym alone is insufficient).
 *
 * Returns the full monolithannex.com shareable URL.
 */
/**
 * Can an invite link be built from this server's public URL?
 *
 * The invite format requires HTTPS — `InvitePayload::validate` on the server
 * rejects anything else, because the link carries a join secret that must not
 * be readable in transit. Callers used to test only that a public URL was
 * *set*, so every http:// deployment fired an invite request that could never
 * succeed: the admin panel did it twice (on open, and after saving a URL) and
 * startup did it once more, each swallowing the failure in an empty catch. The
 * operator saw no invite link and no reason for its absence, while the server
 * log filled with rejected requests. The UI audit caught it as a repeated 400
 * on `admin-server-settings`.
 */
export function canCreateInviteLink(publicUrl: string | null | undefined): boolean {
  return /^https:\/\//i.test((publicUrl ?? '').trim());
}

export async function createInviteLink(
  _apiBaseUrl: string,
  pseudonymId: string,
  options?: { maxUses?: number; expiresInHours?: number },
): Promise<{ code: string; url: string; expiresAt?: string }> {
  // Dynamically import to avoid circular dependency at module level
  const api = await import('@/lib/api');
  return api.createInvite(pseudonymId, options);
}

/** Clear the invite state from the URL without a page reload. */
export function clearInviteFromUrl(): void {
  const url = new URL(window.location.href);
  if (url.pathname.startsWith('/invite/') || url.hash.startsWith('#/invite/')) {
    window.history.replaceState(null, '', '/');
  }
}

export interface InviteProgress {
  stage: 'parsing' | 'registering' | 'proving' | 'joining' | 'complete' | 'error';
  message: string;
}

export type InviteProgressCallback = (progress: InviteProgress) => void;

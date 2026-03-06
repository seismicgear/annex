/**
 * Invite routing — parses invite links and orchestrates background
 * identity verification + server join.
 *
 * New invite URL format (monolithannex.com):
 *   https://monolithannex.com/invite/{base64url_payload}
 *   → deep-links to annex://invite?server={encoded}&code={encoded}
 *
 * Legacy invite URL format (direct server links):
 *   https://<host>/invite/<channelId>?slug=<serverSlug>&label=<label>
 */

import type { InvitePayload, LegacyInvitePayload } from '@/types';

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
 * Parse an annex:// protocol invite URL.
 *
 * Expected format: annex://invite?server={percent_encoded}&code={percent_encoded}
 *
 * This is called when the desktop app receives a deep-link from
 * monolithannex.com's "Open in Annex" button.
 */
export function parseProtocolInvite(rawUrl: string): InvitePayload | null {
  try {
    const url = new URL(rawUrl);
    if (url.protocol !== 'annex:') return null;
    if (url.hostname !== 'invite') return null;

    const server = url.searchParams.get('server');
    const code = url.searchParams.get('code');

    if (!server || !code) return null;
    if (!server.startsWith('https://')) return null;

    return { server, code };
  } catch {
    return null;
  }
}

/**
 * Create a new invite by calling the server API.
 *
 * Returns the full monolithannex.com shareable URL.
 */
export async function createInviteLink(
  apiBaseUrl: string,
  pseudonymId: string,
  options?: { maxUses?: number; expiresInHours?: number },
): Promise<{ code: string; url: string; expiresAt?: string }> {
  const response = await fetch(`${apiBaseUrl}/api/invites`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'X-Annex-Pseudonym': pseudonymId,
    },
    body: JSON.stringify({
      maxUses: options?.maxUses,
      expiresInHours: options?.expiresInHours,
    }),
  });

  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error || `Failed to create invite: ${response.status}`);
  }

  return response.json();
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

/**
 * Invite routing — parses invite links and orchestrates background
 * identity verification + channel join.
 *
 * Invite URL format:
 *   https://<host>/invite/<channelId>?slug=<serverSlug>&label=<label>
 *
 * The invite link carries enough state to:
 * 1. Route the user to the correct server
 * 2. Trigger background Groth16 proof generation
 * 3. Execute VRP handshake
 * 4. Join the target channel
 */

import type { InvitePayload } from '@/types';

/**
 * Parse an invite link from the current window location.
 * Returns null if the current URL is not an invite link.
 */
export function parseInviteFromUrl(): InvitePayload | null {
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
 * Generate an invite link for a channel on the current server.
 */
export function generateInviteLink(
  channelId: string,
  serverSlug: string,
  label?: string,
): string {
  const base = `${window.location.origin}/invite/${encodeURIComponent(channelId)}`;
  const params = new URLSearchParams({ slug: serverSlug });
  if (label) params.set('label', label);
  return `${base}?${params.toString()}`;
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

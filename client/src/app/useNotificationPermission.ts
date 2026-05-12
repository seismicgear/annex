/**
 * Prompt for browser Notification permission once chat is fully active:
 * the user is registered, the WebSocket is connected, channels have loaded,
 * and a channel is selected. Persists a flag so we don't keep prompting.
 */

import { useEffect } from 'react';
import type { IdentityPhase } from '@/stores/identity';

const NOTIFICATION_PERMISSION_PROMPTED_KEY = 'annex:notificationPermissionPrompted';

function hasPromptedNotificationPermission(): boolean {
  if (typeof localStorage === 'undefined') return false;
  try {
    return localStorage.getItem(NOTIFICATION_PERMISSION_PROMPTED_KEY) === '1';
  } catch {
    return false;
  }
}

function markNotificationPermissionPrompted(): void {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(NOTIFICATION_PERMISSION_PROMPTED_KEY, '1');
  } catch {
    // Ignore storage failures (private mode, quota errors, etc.)
  }
}

interface UseNotificationPermissionArgs {
  phase: IdentityPhase;
  pseudonymId: string | null | undefined;
  wsConnected: boolean;
  loadedChannelsLength: number;
  activeChannelId: string | null | undefined;
}

export function useNotificationPermission({
  phase,
  pseudonymId,
  wsConnected,
  loadedChannelsLength,
  activeChannelId,
}: UseNotificationPermissionArgs): void {
  useEffect(() => {
    if (phase !== 'ready' || !pseudonymId) return;
    if (!wsConnected) return;
    if (loadedChannelsLength === 0) return;
    if (!activeChannelId) return;
    if (!('Notification' in globalThis)) return;
    if (Notification.permission !== 'default') return;
    if (hasPromptedNotificationPermission()) return;

    markNotificationPermissionPrompted();
    Notification.requestPermission().catch(() => {});
  }, [phase, pseudonymId, wsConnected, loadedChannelsLength, activeChannelId]);
}

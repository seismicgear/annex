/**
 * Username cache store — caches visible usernames from server.
 *
 * Fetches the list of usernames the current user has been granted
 * visibility to, and provides a lookup function for message display.
 */

import { create } from 'zustand';
import * as api from '@/lib/api';

interface UsernameStore {
  /** Map of pseudonymId -> display name for users who granted us visibility. */
  cache: Record<string, string>;
  /** Whether we're currently loading usernames. */
  loading: boolean;
  /** Load visible usernames from the server. */
  loadVisibleUsernames: (pseudonymId: string) => Promise<void>;
  /** Look up a display name for a pseudonym. Returns null if not cached. */
  getDisplayName: (pseudonymId: string) => string | null;
  /** Clear the cache (e.g., on disconnect or server switch). */
  clear: () => void;
}

export const useUsernameStore = create<UsernameStore>((set, get) => ({
  cache: {},
  loading: false,

  loadVisibleUsernames: async (pseudonymId: string) => {
    set({ loading: true });
    try {
      const resp = await api.getVisibleUsernames(pseudonymId);
      set({ cache: resp.usernames });
    } catch (err) {
      console.warn('Failed to load visible usernames:', err);
    } finally {
      set({ loading: false });
    }
  },

  getDisplayName: (pseudonymId: string) => {
    return get().cache[pseudonymId] ?? null;
  },

  clear: () => {
    set({ cache: {} });
  },
}));

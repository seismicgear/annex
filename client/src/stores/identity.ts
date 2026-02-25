/**
 * Identity store — manages the current user's identity lifecycle.
 *
 * The lifecycle is split into two stages:
 *   1. Local key generation (offline): generateLocalKeys()
 *   2. Server registration (online):  registerWithServer()
 *
 * This split ensures the identity creation screen (Screen 1) never makes
 * network requests.  Server interaction only starts after the user has
 * selected a server on Screen 2.
 */

import { create } from 'zustand';
import type { StoredIdentity, IdentityInfo } from '@/types';
import * as db from '@/lib/db';
import * as zk from '@/lib/zk';
import * as api from '@/lib/api';

export type IdentityPhase =
  | 'uninitialized'
  | 'generating'
  | 'keys_ready'
  | 'registering'
  | 'proving'
  | 'verifying'
  | 'ready'
  | 'error';

interface IdentityState {
  /** Current lifecycle phase. */
  phase: IdentityPhase;
  /** Active identity or null. */
  identity: StoredIdentity | null;
  /** Error message if phase is 'error'. */
  error: string | null;
  /** Structured diagnostics for startup/register failures. */
  errorDetails: string | null;
  /** All stored identities from IndexedDB. */
  storedIdentities: StoredIdentity[];
  /** Server-side permissions for the current identity. */
  permissions: IdentityInfo | null;

  /** Load stored identities and auto-select the most recent one. */
  loadIdentities: () => Promise<void>;
  /** Fetch permissions from the server for the current identity. */
  loadPermissions: () => Promise<void>;
  /** Generate ZK identity keys locally (no network requests). */
  generateLocalKeys: (roleCode: number) => Promise<void>;
  /** Register existing local keys with a server (requires network). */
  registerWithServer: (serverSlug: string) => Promise<void>;
  /** Select an existing identity by ID. */
  selectIdentity: (id: string) => Promise<void>;
  /** Export current identity for backup. */
  exportCurrent: () => string | null;
  /** Import an identity from backup JSON. */
  importBackup: (json: string) => Promise<void>;
  /** Clear the current identity selection (logout). */
  logout: () => void;
}

export const useIdentityStore = create<IdentityState>((set, get) => ({
  phase: 'uninitialized',
  identity: null,
  error: null,
  errorDetails: null,
  storedIdentities: [],
  permissions: null,

  loadIdentities: async () => {
    const identities = await db.listIdentities();
    // Prefer a fully registered identity (has pseudonymId).
    const ready = identities.find((i) => i.pseudonymId !== null);
    if (ready) {
      set({ storedIdentities: identities, identity: ready, phase: 'ready', error: null, errorDetails: null });
      return;
    }
    // Otherwise select one that has keys but isn't registered yet.
    const withKeys = identities.find((i) => !!i.sk);
    if (withKeys) {
      set({ storedIdentities: identities, identity: withKeys, phase: 'keys_ready', error: null, errorDetails: null });
      return;
    }
    set({ storedIdentities: identities, identity: null, phase: 'uninitialized', error: null, errorDetails: null });
  },

  loadPermissions: async () => {
    const { identity } = get();
    if (!identity?.pseudonymId) return;
    try {
      const info = await api.getIdentityInfo(identity.pseudonymId);
      set({ permissions: info });
    } catch {
      // Non-fatal: permissions won't gate admin UI. Expected when the
      // server is unreachable or the identity session has expired.
    }
  },

  generateLocalKeys: async (roleCode: number) => {
    try {
      set({ phase: 'generating', error: null, errorDetails: null });
      await zk.initPoseidon();
      const sk = zk.generateSecretKey();
      const nodeId = zk.generateNodeId();
      const commitmentHex = await zk.computeCommitment(sk, roleCode, nodeId);

      const identity: StoredIdentity = {
        id: crypto.randomUUID(),
        sk: sk.toString(16),
        roleCode,
        nodeId,
        commitmentHex,
        pseudonymId: null,
        serverSlug: '',
        leafIndex: null,
        createdAt: new Date().toISOString(),
      };
      await db.saveIdentity(identity);
      const identities = await db.listIdentities();
      set({ identity, storedIdentities: identities, phase: 'keys_ready' });
    } catch (e) {
      set({
        phase: 'error',
        error: e instanceof Error ? e.message : String(e),
        errorDetails: e instanceof Error ? `${e.name}: ${e.message}` : String(e),
      });
    }
  },

  registerWithServer: async (serverSlug: string) => {
    const { identity } = get();
    if (!identity?.sk || !identity.commitmentHex) {
      set({
        phase: 'error',
        error: 'No identity keys found',
        errorDetails: 'registerWithServer aborted: missing local key material.',
      });
      return;
    }

    try {
      const sk = BigInt('0x' + identity.sk);

      // Update server slug on the identity.
      identity.serverSlug = serverSlug;
      await db.saveIdentity(identity);
      set({ identity: { ...identity } });

      // Register commitment with server.
      set({ phase: 'registering', error: null, errorDetails: null });
      const reg = await api.register(identity.commitmentHex, identity.roleCode, identity.nodeId);
      identity.leafIndex = reg.leafIndex;
      await db.saveIdentity(identity);

      // Generate proof.
      set({ phase: 'proving', error: null, errorDetails: null });
      const { proof, publicSignals } = await zk.generateMembershipProof({
        sk,
        roleCode: identity.roleCode,
        nodeId: identity.nodeId,
        leafIndex: reg.leafIndex,
        pathElements: reg.pathElements,
        pathIndexBits: reg.pathIndexBits,
      });

      // Verify membership — the VRP topic scopes pseudonym derivation
      // to this specific server.
      set({ phase: 'verifying', error: null, errorDetails: null });
      const vrpTopic = `annex:server:${serverSlug}:v1`;
      const verification = await api.verifyMembership(
        reg.rootHex,
        identity.commitmentHex,
        vrpTopic,
        proof,
        publicSignals,
      );

      identity.pseudonymId = verification.pseudonymId;
      await db.saveIdentity(identity);

      const identities = await db.listIdentities();
      set({
        phase: 'ready',
        identity: { ...identity },
        storedIdentities: identities,
        error: null,
        errorDetails: null,
      });
    } catch (e) {
      let userError = e instanceof Error ? e.message : String(e);

      if (e instanceof zk.ZkProofAssetsError) {
        userError = 'Proof assets missing. Please restart and try again.';
      } else if (e instanceof zk.ZkProofTimeoutError) {
        userError = 'Proof generation timed out. Please retry (the first proof can take longer on slow hardware).';
      }

      set({
        phase: 'error',
        error: userError,
        errorDetails: e instanceof Error ? `${e.name}: ${e.message}` : String(e),
      });
    }
  },

  selectIdentity: async (id: string) => {
    const identity = await db.getIdentity(id);
    if (!identity) return;
    if (identity.pseudonymId) {
      set({ identity, phase: 'ready', error: null, errorDetails: null });
    } else if (identity.sk) {
      set({ identity, phase: 'keys_ready', error: null, errorDetails: null });
    }
  },

  exportCurrent: () => {
    const { identity } = get();
    return identity ? db.exportIdentity(identity) : null;
  },

  importBackup: async (json: string) => {
    const identity = await db.importIdentity(json);
    const identities = await db.listIdentities();
    if (identity.pseudonymId) {
      set({ storedIdentities: identities, identity, phase: 'ready', error: null, errorDetails: null });
    } else if (identity.sk) {
      set({ storedIdentities: identities, identity, phase: 'keys_ready', error: null, errorDetails: null });
    } else {
      set({ storedIdentities: identities });
    }
  },

  logout: () => {
    set({ identity: null, phase: 'uninitialized', error: null, errorDetails: null, permissions: null });
  },
}));

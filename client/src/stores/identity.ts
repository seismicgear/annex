/**
 * Identity store — manages the current user's identity lifecycle.
 *
 * Handles: key generation, commitment, registration, proof generation,
 * membership verification, and pseudonym persistence.
 */

import { create } from 'zustand';
import type { StoredIdentity, IdentityInfo } from '@/types';
import * as db from '@/lib/db';
import * as zk from '@/lib/zk';
import * as api from '@/lib/api';

export type IdentityPhase =
  | 'uninitialized'
  | 'generating'
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
  /** All stored identities from IndexedDB. */
  storedIdentities: StoredIdentity[];
  /** Server-side permissions for the current identity. */
  permissions: IdentityInfo | null;

  /** Load stored identities and auto-select the most recent ready one. */
  loadIdentities: () => Promise<void>;
  /** Fetch permissions from the server for the current identity. */
  loadPermissions: () => Promise<void>;
  /** Run the full registration flow: generate keys -> register -> prove -> verify. */
  createIdentity: (roleCode: number, serverSlug: string) => Promise<void>;
  /** Select an existing identity (must already have pseudonymId). */
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
  storedIdentities: [],
  permissions: null,

  loadIdentities: async () => {
    const identities = await db.listIdentities();
    const ready = identities.find((i) => i.pseudonymId !== null);
    set({
      storedIdentities: identities,
      identity: ready ?? null,
      phase: ready ? 'ready' : 'uninitialized',
    });
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

  createIdentity: async (roleCode: number, serverSlug: string) => {
    try {
      // Phase 1: Generate keys
      set({ phase: 'generating', error: null });
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
        serverSlug,
        leafIndex: null,
        createdAt: new Date().toISOString(),
      };
      await db.saveIdentity(identity);
      set({ identity });

      // Phase 2: Register commitment
      set({ phase: 'registering' });
      const reg = await api.register(commitmentHex, roleCode, nodeId);
      identity.leafIndex = reg.leafIndex;
      await db.saveIdentity(identity);

      // Phase 3: Generate proof
      set({ phase: 'proving' });
      const { proof, publicSignals } = await zk.generateMembershipProof({
        sk,
        roleCode,
        nodeId,
        leafIndex: reg.leafIndex,
        pathElements: reg.pathElements,
        pathIndexBits: reg.pathIndexBits,
      });

      // Phase 4: Verify membership — the VRP topic scopes pseudonym derivation
      // to this specific server. The resulting pseudonymId is stored in IndexedDB
      // and never re-derived, so changing this format only affects new identities.
      set({ phase: 'verifying' });
      const vrpTopic = `annex:server:${serverSlug}:v1`;
      const verification = await api.verifyMembership(
        reg.rootHex,
        commitmentHex,
        vrpTopic,
        proof,
        publicSignals,
      );

      identity.pseudonymId = verification.pseudonymId;
      await db.saveIdentity(identity);

      const identities = await db.listIdentities();
      set({ phase: 'ready', identity, storedIdentities: identities });
    } catch (e) {
      set({
        phase: 'error',
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  selectIdentity: async (id: string) => {
    const identity = await db.getIdentity(id);
    if (identity?.pseudonymId) {
      set({ identity, phase: 'ready', error: null });
    }
  },

  exportCurrent: () => {
    const { identity } = get();
    return identity ? db.exportIdentity(identity) : null;
  },

  importBackup: async (json: string) => {
    const identity = await db.importIdentity(json);
    const identities = await db.listIdentities();
    set({
      storedIdentities: identities,
      identity: identity.pseudonymId ? identity : get().identity,
      phase: identity.pseudonymId ? 'ready' : get().phase,
    });
  },

  logout: () => {
    set({ identity: null, phase: 'uninitialized', error: null, permissions: null });
  },
}));

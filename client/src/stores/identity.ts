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
import { useVoiceStore } from './voice';

/**
 * Whether a restored/imported identity can enter `ready` directly. Requires a
 * live session token AND a cached ZK proof whose Merkle root is still the
 * server's current root. The cached `x-annex-zk-proof` is bound to the root
 * that was active at proof time; once the tree grows and the grace window
 * passes, the server rejects it (`is_root_acceptable`), so a stale/missing
 * proof must be regenerated via the normal register flow rather than entering
 * `ready` with credentials that will 403.
 *
 * The two checks below fail in opposite directions on purpose. Reading the
 * stored payload is a LOCAL check: a payload that will not parse, or that
 * carries no root, is a corrupt credential — it must fail closed, because
 * entering `ready` with it means every protected call 403s and the app
 * believes it is signed in, so nothing routes the user back to
 * re-registration. Asking the server for the current root is a REMOTE check
 * and is best-effort: offline, nothing works anyway, so a well-formed cached
 * proof is trusted rather than forcing a re-prove that cannot run either.
 */
async function cachedProofIsUsable(identity: StoredIdentity): Promise<boolean> {
  // The session token is refreshed separately (App.tsx /api/session/refresh);
  // the proof is the credential that goes stale/missing, so gate on it.
  if (!identity.zkProofPayload) return false;

  let cachedRoot: string;
  try {
    const payload = JSON.parse(identity.zkProofPayload) as { root_hex?: unknown };
    if (typeof payload?.root_hex !== 'string' || payload.root_hex === '') return false;
    cachedRoot = payload.root_hex;
  } catch {
    return false;
  }

  try {
    const current = await api.getCurrentRoot();
    // An unexpected response shape is a server problem, not a corrupt local
    // credential, so it takes the same best-effort path as being offline.
    if (typeof current?.rootHex !== 'string') return true;
    return current.rootHex.toLowerCase() === cachedRoot.toLowerCase();
  } catch {
    return true;
  }
}

export type IdentityPhase =
  | 'uninitialized'
  | 'generating'
  | 'keys_ready'
  | 'registering'
  | 'proving'
  | 'verifying'
  | 'ready'
  | 'error';

export type ProvingStatus =
  | 'idle'
  | 'loading_assets'
  | 'computing_witness'
  | 'generating_proof';

export type PermissionsStatus = 'idle' | 'loading' | 'ready' | 'error' | 'denied';

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
  /** Fetch state for server-side permissions. */
  permissionsStatus: PermissionsStatus;
  /** The pseudonymId for which current permissions were fetched (cache key). */
  permissionsPseudonymId: string | null;
  /** True while snarkjs fullProve is still running. */
  proofInFlight: boolean;
  /** Detailed proving stage surfaced from the proof worker. */
  provingStatus: ProvingStatus;

  /** Load stored identities and auto-select the most recent one. */
  loadIdentities: () => Promise<void>;
  /** Fetch permissions from the server for the current identity. */
  loadPermissions: () => Promise<void>;
  /** Generate ZK identity keys locally (no network requests). */
  generateLocalKeys: (roleCode: number) => Promise<void>;
  /** Register existing local keys with a server (requires network). */
  registerWithServer: (serverSlug: string, inviteCode?: string, serverPassword?: string) => Promise<void>;
  /** Select an existing identity by ID. */
  selectIdentity: (id: string) => Promise<void>;
  /** Export current identity for backup. */
  exportCurrent: () => string | null;
  /** Import an identity from backup JSON. */
  importBackup: (json: string) => Promise<void>;
  /** Clone the current identity for use on a different server. */
  cloneForServer: () => Promise<string | null>;
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
  permissionsStatus: 'idle',
  permissionsPseudonymId: null,
  proofInFlight: false,
  provingStatus: 'idle',

  loadIdentities: async () => {
    const identities = await db.listIdentities();
    // Sort by lastUsedAt descending (most recent first), falling back to createdAt.
    const sorted = [...identities].sort((a, b) => {
      const aTime = a.lastUsedAt ?? a.createdAt;
      const bTime = b.lastUsedAt ?? b.createdAt;
      return bTime.localeCompare(aTime);
    });
    // Prefer a fully registered identity (has pseudonymId), most recently used first.
    const ready = sorted.find((i) => i.pseudonymId !== null);
    if (ready) {
      if (await cachedProofIsUsable(ready)) {
        // Set whatever token we have (even expired), or clear if absent.
        // The App.tsx effect will call /api/session/refresh if it's expired.
        api.setSessionToken(ready.sessionToken ?? null);
        // Restore the cached proof so protected calls (channel join/send) work
        // after a cold start without re-running the proof.
        api.setZkProofPayload(ready.zkProofPayload ?? null);
        set({ storedIdentities: identities, identity: ready, phase: 'ready', error: null, errorDetails: null, proofInFlight: false, provingStatus: 'idle' });
      } else {
        // Missing or stale-root proof — re-prove via the normal register flow
        // rather than entering `ready` with credentials the server will 403.
        api.setSessionToken(null);
        api.setZkProofPayload(null);
        set({ storedIdentities: identities, identity: ready, phase: 'keys_ready', error: null, errorDetails: null, proofInFlight: false, provingStatus: 'idle' });
      }
      return;
    }
    // Otherwise select one that has keys but isn't registered yet.
    const withKeys = sorted.find((i) => !!i.sk);
    if (withKeys) {
      api.setSessionToken(null);
      api.setZkProofPayload(null);
      set({ storedIdentities: identities, identity: withKeys, phase: 'keys_ready', error: null, errorDetails: null, proofInFlight: false, provingStatus: 'idle' });
      return;
    }
    api.setSessionToken(null);
    api.setZkProofPayload(null);
    set({ storedIdentities: identities, identity: null, phase: 'uninitialized', error: null, errorDetails: null, proofInFlight: false, provingStatus: 'idle' });
  },

  loadPermissions: async () => {
    const { identity, permissionsPseudonymId } = get();
    if (!identity?.pseudonymId) return;
    set({ permissionsStatus: 'loading' });
    try {
      const info = await api.getIdentityInfo(identity.pseudonymId);
      set({ permissions: info, permissionsStatus: 'ready', permissionsPseudonymId: identity.pseudonymId });
    } catch (err) {
      // Distinguish authoritative "denied/forbidden" from transient errors
      const isAuthoritative = err instanceof api.ApiError && (err.status === 403 || err.status === 401);
      const status: PermissionsStatus = isAuthoritative ? 'denied' : 'error';

      // If the pseudonym changed since the last successful fetch, clear
      // stale permissions so capabilities from the previous server don't
      // bleed into the current context.
      if (permissionsPseudonymId !== identity.pseudonymId) {
        set({ permissions: null, permissionsStatus: status, permissionsPseudonymId: null });
      } else {
        set({ permissionsStatus: status });
      }
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
        sessionToken: null,
        serverSlug: '',
        leafIndex: null,
        createdAt: new Date().toISOString(),
      };
      await db.saveIdentity(identity);
      const identities = await db.listIdentities();
      set({ identity, storedIdentities: identities, phase: 'keys_ready', proofInFlight: false, provingStatus: 'idle' });
    } catch (e) {
      set({
        phase: 'error',
        error: e instanceof Error ? e.message : String(e),
        errorDetails: e instanceof Error ? `${e.name}: ${e.message}` : String(e),
      });
    }
  },

  registerWithServer: async (serverSlug: string, inviteCode?: string, serverPassword?: string) => {
    if (zk.isProofGenerationInFlight()) {
      await zk.cancelMembershipProofGeneration('Proof generation cancelled before retry.');
    }

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
      set({ phase: 'registering', error: null, errorDetails: null, proofInFlight: false, provingStatus: 'idle' });
      const reg = await api.register(identity.commitmentHex, identity.roleCode, identity.nodeId, inviteCode, serverPassword);
      identity.leafIndex = reg.leafIndex;
      await db.saveIdentity(identity);

      // Generate a v2 membership proof: the per-topic nullifier is derived
      // from the secret key INSIDE the circuit (Poseidon(sk, topicHash, 1)),
      // not from the public commitment. This closes the v1 linkability hole
      // where anyone holding the public Merkle leaf could compute every topic
      // pseudonym. The VRP topic scopes pseudonym derivation to this server.
      const vrpTopic = `annex:server:${serverSlug}:v2`;
      set({ phase: 'proving', proofInFlight: true, provingStatus: 'loading_assets', error: null, errorDetails: null });
      const { proof, publicSignals, nullifierHex, topicHashHex } = await zk.generateMembershipProofV2({
        sk,
        roleCode: identity.roleCode,
        nodeId: identity.nodeId,
        leafIndex: reg.leafIndex,
        pathElements: reg.pathElements,
        pathIndexBits: reg.pathIndexBits,
      }, vrpTopic, {
        onStage: (stage) => {
          set({ provingStatus: stage });
        },
      });

      // Verify membership.
      set({ phase: 'verifying', error: null, errorDetails: null, provingStatus: 'idle' });
      const verification = await api.verifyMembership(
        reg.rootHex,
        identity.commitmentHex,
        vrpTopic,
        proof,
        publicSignals,
        { nullifierHex, topicHashHex },
      );

      identity.pseudonymId = verification.pseudonymId;
      identity.sessionToken = verification.sessionToken;
      identity.lastUsedAt = new Date().toISOString();
      api.setSessionToken(verification.sessionToken);
      // Cache the ZK proof so protected endpoints can include it. The shape
      // MUST match the server's `ZkProofPayload`. For v2 the middleware
      // re-verifies the full proof on channel access, so the payload carries
      // `proof` + `root_hex` + `commitment_hex` + `publicSignals` (length 4) +
      // `protocolVersion: 'v2'` + `topic` (to recompute and match the topic
      // hash). Omitting any of these makes every ZK-enforced channel join/send
      // fail with 403 ("Not a member of channel"). We persist it on the
      // identity too so a cold
      // start / identity switch can restore it without re-proving.
      const zkProofPayload = JSON.stringify({
        proof,
        root_hex: reg.rootHex,
        commitment_hex: identity.commitmentHex,
        protocolVersion: 'v2',
        publicSignals,
        // v2 requires the topic so the middleware can recompute and match
        // publicSignals[3] (topicHash) when re-verifying on channel access.
        topic: vrpTopic,
      });
      api.setZkProofPayload(zkProofPayload);
      identity.zkProofPayload = zkProofPayload;
      await db.saveIdentity(identity);

      const identities = await db.listIdentities();
      set({
        phase: 'ready',
        proofInFlight: false,
        provingStatus: 'idle',
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
      } else if (e instanceof zk.ZkProofInFlightError) {
        userError = 'proof still running.';
      } else if (e instanceof zk.ZkProofCancelledError) {
        userError = 'Proof generation was cancelled. Please retry.';
      }

      set({
        phase: 'error',
        proofInFlight: zk.isProofGenerationInFlight(),
        provingStatus: 'idle',
        error: userError,
        errorDetails: e instanceof Error ? `${e.name}: ${e.message}` : String(e),
      });
    }
  },

  selectIdentity: async (id: string) => {
    const identity = await db.getIdentity(id);
    if (!identity) return;
    // Clear permissions from the previous identity/server so stale
    // capability flags are never reused across contexts.
    if (identity.pseudonymId && (await cachedProofIsUsable(identity))) {
      api.setSessionToken(identity.sessionToken ?? null);
      api.setZkProofPayload(identity.zkProofPayload ?? null);
      set({ identity, phase: 'ready', error: null, errorDetails: null, proofInFlight: false, provingStatus: 'idle', permissions: null, permissionsStatus: 'idle', permissionsPseudonymId: null });
    } else if (identity.sk) {
      // Either keys-only, or registered but with a missing/stale-root proof —
      // re-prove via the normal register flow.
      api.setSessionToken(null);
      api.setZkProofPayload(null);
      set({ identity, phase: 'keys_ready', error: null, errorDetails: null, proofInFlight: false, provingStatus: 'idle', permissions: null, permissionsStatus: 'idle', permissionsPseudonymId: null });
    }
    // Update lastUsedAt
    identity.lastUsedAt = new Date().toISOString();
    await db.saveIdentity(identity);
  },

  exportCurrent: () => {
    const { identity } = get();
    return identity ? db.exportIdentity(identity) : null;
  },

  importBackup: async (json: string) => {
    // A backup that will not parse, or is not an Annex backup, threw out of
    // here with nothing catching it. The screen that calls this renders
    // `{error && ...}` from the store, and no failure path ever set it — so
    // picking the wrong file did nothing at all, on the one screen people
    // reach when something has already gone wrong for them.
    let identity: StoredIdentity;
    try {
      identity = await db.importIdentity(json);
    } catch (err) {
      set({
        error: 'That file is not a usable Annex backup.',
        errorDetails: err instanceof Error ? `${err.name}: ${err.message}` : String(err),
      });
      return;
    }
    const identities = await db.listIdentities();
    if (identity.pseudonymId && (await cachedProofIsUsable(identity))) {
      api.setSessionToken(identity.sessionToken ?? null);
      api.setZkProofPayload(identity.zkProofPayload ?? null);
      set({ storedIdentities: identities, identity, phase: 'ready', error: null, errorDetails: null, proofInFlight: false, provingStatus: 'idle' });
    } else if (identity.sk) {
      // Imported backups drop the proof + null the session token, so a
      // registered identity restores here without usable creds — re-prove via
      // the normal flow instead of entering `ready` and 403-ing immediately.
      api.setSessionToken(null);
      api.setZkProofPayload(null);
      set({ storedIdentities: identities, identity, phase: 'keys_ready', error: null, errorDetails: null, proofInFlight: false, provingStatus: 'idle' });
    } else {
      api.setSessionToken(null);
      api.setZkProofPayload(null);
      set({ storedIdentities: identities });
    }
  },

  cloneForServer: async () => {
    const { identity } = get();
    if (!identity) return null;
    const cloned = await db.cloneIdentityForServer(identity.id);
    if (!cloned) return null;
    const identities = await db.listIdentities();
    set({ storedIdentities: identities });
    return cloned.id;
  },

  logout: () => {
    void zk.cancelMembershipProofGeneration();

    // Tear down voice state BEFORE clearing identity / session token.
    // This ensures we attempt a graceful leaveCall while credentials are
    // still valid, and force-resets all voice UI state regardless.
    const voiceStore = useVoiceStore.getState();
    const { identity: currentIdentity } = get();
    if (currentIdentity?.pseudonymId && (voiceStore.connectedChannelId || voiceStore.voiceToken)) {
      void voiceStore.leaveCall(currentIdentity.pseudonymId).catch(() => {});
    }
    voiceStore.forceReset();

    api.setSessionToken(null);
    api.setZkProofPayload(null);
    set({ identity: null, phase: 'uninitialized', error: null, errorDetails: null, permissions: null, permissionsStatus: 'idle', permissionsPseudonymId: null, proofInFlight: false, provingStatus: 'idle' });
  },
}));

/**
 * Client-side ZK proof generation using snarkjs and circomlibjs.
 *
 * Handles:
 * - Poseidon commitment computation
 * - Groth16 membership proof generation
 * - Witness generation via WASM circuit
 */

import type * as snarkjs from 'snarkjs';

const MEMBERSHIP_WASM_PATH = '/zk/membership.wasm';
const MEMBERSHIP_ZKEY_PATH = '/zk/membership_final.zkey';
const MEMBERSHIP_V2_WASM_PATH = '/zk/membership_v2.wasm';
const MEMBERSHIP_V2_ZKEY_PATH = '/zk/membership_v2_final.zkey';
const DEFAULT_PROOF_TIMEOUT_MS = 120_000;

/** BN254 scalar field order (Fr). */
const BN254_FR =
  21888242871839275222246405745257275088548364400416034343698204186575808495617n;

/** Domain separator for the v2 topic hash — MUST match the Rust server's
 * `annex_identity::zk::V2_TOPIC_HASH_DOMAIN`. */
const V2_TOPIC_HASH_DOMAIN = 'annex/v2/topicHash:';

/**
 * Compute the v2 topicHash as a BN254 scalar, byte-for-byte identical to the
 * server's `topic_hash_for_v2`: `Fr::from_be_bytes_mod_order(SHA256(domain +
 * topic))`. The proof is bound to this value (a public input), and the server
 * recomputes it from the `topic` field, so any mismatch is rejected.
 */
export async function computeTopicHashV2(topic: string): Promise<bigint> {
  if (topic.length === 0) {
    throw new Error('topic must not be empty for v2 topic hash');
  }
  const bytes = new TextEncoder().encode(V2_TOPIC_HASH_DOMAIN + topic);
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
  let acc = 0n;
  for (const b of digest) acc = (acc << 8n) | BigInt(b);
  return acc % BN254_FR;
}

function parseTimeoutMs(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value) && value > 0) {
    return Math.floor(value);
  }

  if (typeof value === 'string') {
    const parsed = Number.parseInt(value, 10);
    if (Number.isFinite(parsed) && parsed > 0) {
      return parsed;
    }
  }

  return null;
}

export function getProofTimeoutMs(): number {
  const runtimeConfig = (globalThis as { __ANNEX_CONFIG__?: { zkProofTimeoutMs?: number | string } })
    .__ANNEX_CONFIG__;
  const runtimeValue = parseTimeoutMs(runtimeConfig?.zkProofTimeoutMs);
  if (runtimeValue !== null) {
    return runtimeValue;
  }

  const env = (import.meta as ImportMeta & { env?: Record<string, string | undefined> }).env;
  const envValue = parseTimeoutMs(env?.VITE_ZK_PROOF_TIMEOUT_MS);
  if (envValue !== null) {
    return envValue;
  }

  return DEFAULT_PROOF_TIMEOUT_MS;
}

export class ZkProofAssetsError extends Error {
  readonly kind = 'assets';

  constructor(message: string) {
    super(message);
    this.name = 'ZkProofAssetsError';
  }
}

export class ZkProofTimeoutError extends Error {
  readonly kind = 'timeout';

  constructor(message: string) {
    super(message);
    this.name = 'ZkProofTimeoutError';
  }
}

export class ZkProofInFlightError extends Error {
  readonly kind = 'in_flight';

  constructor(message: string) {
    super(message);
    this.name = 'ZkProofInFlightError';
  }
}

export class ZkProofCancelledError extends Error {
  readonly kind = 'cancelled';

  constructor(message: string) {
    super(message);
    this.name = 'ZkProofCancelledError';
  }
}

// circomlibjs doesn't have TS types; we import and cast
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let poseidonFn: any = null;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let F: any = null;

/** Initialize the Poseidon hash function. Must be called before computeCommitment. */
export async function initPoseidon(): Promise<void> {
  if (poseidonFn) return;
  // Dynamic import for circomlibjs (CommonJS module)
  const circomlibjs = await import('circomlibjs');
  const buildPoseidon = circomlibjs.buildPoseidon || circomlibjs.default?.buildPoseidon;
  const poseidon = await buildPoseidon();
  poseidonFn = poseidon;
  F = poseidon.F;
}

/**
 * Compute identity commitment: Poseidon(sk, roleCode, nodeId).
 * Returns commitment as hex string (without 0x prefix).
 */
export async function computeCommitment(
  sk: bigint,
  roleCode: number,
  nodeId: number,
): Promise<string> {
  await initPoseidon();
  const hash = poseidonFn([sk, BigInt(roleCode), BigInt(nodeId)]);
  const val = F.toObject(hash);
  return val.toString(16).padStart(64, '0');
}

/**
 * Generate a random secret key as a BN254 scalar field element.
 * Uses Web Crypto for randomness.
 */
export function generateSecretKey(): bigint {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  // BN254 scalar field order
  const p = BigInt('21888242871839275222246405745257275088548364400416034343698204186575808495617');
  let sk = BigInt(0);
  for (const b of bytes) {
    sk = (sk << BigInt(8)) | BigInt(b);
  }
  // Reduce mod p to ensure valid field element
  sk = sk % p;
  // Ensure non-zero
  if (sk === BigInt(0)) sk = BigInt(1);
  return sk;
}

/** Generate a random nodeId (positive integer). */
export function generateNodeId(): number {
  const arr = new Uint32Array(1);
  crypto.getRandomValues(arr);
  return (arr[0] % 1000000) + 1;
}

export interface MembershipProofInput {
  sk: bigint;
  roleCode: number;
  nodeId: number;
  leafIndex: number;
  pathElements: string[];
  pathIndexBits: number[];
}

export interface MembershipProofOutput {
  proof: snarkjs.Groth16Proof;
  publicSignals: string[];
}

export type ProofGenerationStage =
  | 'loading_assets'
  | 'computing_witness'
  | 'generating_proof';

interface GenerateMembershipProofOptions {
  onStage?: (stage: ProofGenerationStage) => void;
}

type ActiveProofJob = {
  promise: Promise<MembershipProofOutput>;
  cancel: (reason?: string) => void;
};

let activeProofJob: ActiveProofJob | null = null;

export function isProofGenerationInFlight(): boolean {
  return activeProofJob !== null;
}

function mapWorkerError(name: string | undefined, message: string): Error {
  if (name === 'ZkProofAssetsError') {
    return new ZkProofAssetsError(message);
  }

  const err = new Error(message);
  err.name = name ?? 'Error';
  return err;
}

export async function cancelMembershipProofGeneration(reason = 'Proof generation cancelled.'): Promise<void> {
  if (!activeProofJob) return;
  activeProofJob.cancel(reason);
  try {
    await activeProofJob.promise;
  } catch {
    // Expected when cancellation rejects the active promise.
  }
}

function runProofInWorker(
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  circuitInput: any,
  paths: { wasmPath: string; zkeyPath: string },
  options?: GenerateMembershipProofOptions,
): Promise<MembershipProofOutput> {
  if (activeProofJob) {
    throw new ZkProofInFlightError('proof still running.');
  }

  const timeoutMs = getProofTimeoutMs();
  const worker = new Worker(new URL('../workers/proof.worker.ts', import.meta.url), { type: 'module' });
  const jobId = crypto.randomUUID();

  let finished = false;
  let timeoutHandle: ReturnType<typeof setTimeout> | undefined;
  let rejectPromise: ((reason?: unknown) => void) | null = null;

  const cleanup = () => {
    if (timeoutHandle) clearTimeout(timeoutHandle);
    worker.onmessage = null;
    worker.onerror = null;
    worker.terminate();
    activeProofJob = null;
  };

  const proofPromise = new Promise<MembershipProofOutput>((resolve, reject) => {
    rejectPromise = reject;

    const fail = (error: Error) => {
      if (finished) return;
      finished = true;
      cleanup();
      reject(error);
    };

    const succeed = (result: MembershipProofOutput) => {
      if (finished) return;
      finished = true;
      cleanup();
      resolve(result);
    };

    timeoutHandle = setTimeout(() => {
      fail(new ZkProofTimeoutError(
        `Proof generation timed out after ${Math.round(timeoutMs / 1000)}s (configured timeout: ${timeoutMs}ms).`,
      ));
    }, timeoutMs);

    worker.onmessage = (event: MessageEvent) => {
      const message = event.data as
        | { kind: 'status'; jobId: string; stage: ProofGenerationStage }
        | { kind: 'result'; jobId: string; proof: snarkjs.Groth16Proof; publicSignals: string[] }
        | { kind: 'error'; jobId: string; error: { name?: string; message?: string } };

      if (!message || message.jobId !== jobId) return;

      if (message.kind === 'status') {
        options?.onStage?.(message.stage);
        return;
      }

      if (message.kind === 'error') {
        fail(
          mapWorkerError(
            message.error?.name,
            message.error?.message ?? 'Unknown proof worker failure',
          ),
        );
        return;
      }

      succeed({ proof: message.proof, publicSignals: message.publicSignals });
    };

    worker.onerror = () => {
      fail(new Error('Proof worker crashed.'));
    };

    worker.postMessage({
      kind: 'start',
      jobId,
      input: circuitInput,
      wasmPath: paths.wasmPath,
      zkeyPath: paths.zkeyPath,
    });
  });

  activeProofJob = {
    promise: proofPromise,
    cancel: (reason?: string) => {
      if (finished) return;
      finished = true;
      cleanup();
      rejectPromise?.(new ZkProofCancelledError(reason ?? 'Proof generation cancelled.'));
    },
  };

  return proofPromise;
}

/**
 * Generate a Groth16 membership proof.
 */
export async function generateMembershipProof(
  input: MembershipProofInput,
  options?: GenerateMembershipProofOptions,
): Promise<MembershipProofOutput> {
  const circuitInput = {
    sk: input.sk.toString(),
    roleCode: input.roleCode.toString(),
    nodeId: input.nodeId.toString(),
    leafIndex: input.leafIndex.toString(),
    pathElements: input.pathElements.map((s) => '0x' + s),
    pathIndexBits: input.pathIndexBits.map(String),
  };

  return runProofInWorker(
    circuitInput,
    { wasmPath: MEMBERSHIP_WASM_PATH, zkeyPath: MEMBERSHIP_ZKEY_PATH },
    options,
  );
}

/** Result of a v2 proof: the standard output plus the extracted nullifier /
 * topicHash (as `0x`-prefixed hex), which the verify endpoint requires. */
export interface MembershipProofV2Output extends MembershipProofOutput {
  nullifierHex: string;
  topicHashHex: string;
}

/** Canonical BN254 field-element hex: 64-char zero-padded lowercase, NO `0x`
 * prefix — byte-for-byte what the server's `fr_to_canonical_hex` /
 * `parse_fr_from_hex` expect. */
function fieldDecToCanonicalHex(dec: string): string {
  return BigInt(dec).toString(16).padStart(64, '0');
}

/**
 * Generate a v2 Groth16 membership proof with a secret-derived nullifier.
 *
 * Unlike v1 (where the per-topic nullifier is `sha256(commitment + topic)` and
 * thus publicly derivable from the Merkle leaf), v2 computes the nullifier as
 * `Poseidon(sk, topicHash, 1)` inside the circuit — observing the commitment
 * does not reveal the topic pseudonym. `topicHash` is a public input bound to
 * the proof; the server recomputes it from the topic string and rejects any
 * mismatch.
 *
 * publicSignals ordering is `[root, commitment, nullifier, topicHash]`.
 */
export async function generateMembershipProofV2(
  input: MembershipProofInput,
  topic: string,
  options?: GenerateMembershipProofOptions,
): Promise<MembershipProofV2Output> {
  const topicHash = await computeTopicHashV2(topic);
  const circuitInput = {
    sk: input.sk.toString(),
    roleCode: input.roleCode.toString(),
    nodeId: input.nodeId.toString(),
    leafIndex: input.leafIndex.toString(),
    pathElements: input.pathElements.map((s) => '0x' + s),
    pathIndexBits: input.pathIndexBits.map(String),
    topicHash: topicHash.toString(),
  };

  const out = await runProofInWorker(
    circuitInput,
    { wasmPath: MEMBERSHIP_V2_WASM_PATH, zkeyPath: MEMBERSHIP_V2_ZKEY_PATH },
    options,
  );

  // publicSignals = [root, commitment, nullifier, topicHash] (decimal strings).
  if (out.publicSignals.length < 4) {
    throw new Error(
      `v2 proof produced ${out.publicSignals.length} public signals, expected 4`,
    );
  }
  return {
    ...out,
    nullifierHex: fieldDecToCanonicalHex(out.publicSignals[2]),
    topicHashHex: fieldDecToCanonicalHex(out.publicSignals[3]),
  };
}

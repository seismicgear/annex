/**
 * Client-side ZK proof generation using snarkjs and circomlibjs.
 *
 * Handles:
 * - Poseidon commitment computation
 * - Groth16 membership proof generation
 * - Witness generation via WASM circuit
 */

import * as snarkjs from 'snarkjs';

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

/**
 * Generate a Groth16 membership proof.
 *
 * Requires the membership circuit WASM and zkey files to be available
 * at /zk/membership.wasm and /zk/membership_final.zkey respectively.
 */
export async function generateMembershipProof(
  input: MembershipProofInput,
): Promise<MembershipProofOutput> {
  const circuitInput = {
    sk: input.sk.toString(),
    roleCode: input.roleCode.toString(),
    nodeId: input.nodeId.toString(),
    leafIndex: input.leafIndex.toString(),
    pathElements: input.pathElements,
    pathIndexBits: input.pathIndexBits.map(String),
  };

  const { proof, publicSignals } = await snarkjs.groth16.fullProve(
    circuitInput,
    '/zk/membership.wasm',
    '/zk/membership_final.zkey',
  );

  return { proof, publicSignals };
}

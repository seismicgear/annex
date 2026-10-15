import type { Groth16Proof } from 'snarkjs';

const FR = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
const FQ = 21888242871839275222246405745257275088696311157297823662689037894645226208583n;

function decimal(value: unknown, bound: bigint): value is string {
  return typeof value === 'string' && /^(0|[1-9][0-9]{0,76})$/.test(value) && BigInt(value) < bound;
}
function g1(value: unknown): boolean {
  return Array.isArray(value) && value.length === 3 && value[2] === '1'
    && decimal(value[0], FQ) && decimal(value[1], FQ);
}
function pair(value: unknown): boolean {
  return Array.isArray(value) && value.length === 2 && value.every(part => decimal(part, FQ));
}

/** Validate before posting, and again at the worker boundary; no caller selects a verifier. */
export function validateMembershipVerification(signals: unknown, proof: unknown): asserts proof is Groth16Proof {
  if (!Array.isArray(signals) || signals.length !== 5 || !signals.every(signal => decimal(signal, FR))) {
    throw new Error('Membership verification requires five canonical field elements.');
  }
  if (!proof || typeof proof !== 'object' || Array.isArray(proof)) throw new Error('Invalid membership proof.');
  const value = proof as Record<string, unknown>;
  const b = value.pi_b;
  if (value.protocol !== 'groth16' || value.curve !== 'bn128' || !g1(value.pi_a) || !g1(value.pi_c)
    || !Array.isArray(b) || b.length !== 3 || !pair(b[0]) || !pair(b[1])
    || !Array.isArray(b[2]) || b[2].length !== 2 || b[2][0] !== '1' || b[2][1] !== '0') {
    throw new Error('Invalid membership proof.');
  }
}

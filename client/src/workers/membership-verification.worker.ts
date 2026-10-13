import { verifyMembershipProof } from '../lib/membership-verification-core';
import type { Groth16Proof } from 'snarkjs';

self.onmessage = async (event: MessageEvent<{ kind: string; jobId: string; signals: string[]; proof: Groth16Proof }>) => {
  const message = event.data;
  if (!message || message.kind !== 'verify' || typeof message.jobId !== 'string') return;
  try {
    const valid = await verifyMembershipProof(message.signals, message.proof);
    self.postMessage({ kind: 'result', jobId: message.jobId, valid });
  } catch {
    // Do not expose worker input or library error strings that could contain proof material.
    self.postMessage({ kind: 'error', jobId: message.jobId });
  }
};

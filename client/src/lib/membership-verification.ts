import type { Groth16Proof } from 'snarkjs';
import { validateMembershipVerification } from './membership-verification-input';

export const MEMBERSHIP_VERIFICATION_TIMEOUT_MS = 30_000;
type Job = { signals: string[]; proof: Groth16Proof; resolve: (valid: boolean) => void; reject: (error: Error) => void };
const queued: Job[] = [];
let active: { cancel: () => void } | null = null;

/** One bounded same-origin worker at a time avoids both UI blocking and an unbounded worker fan-out. */
export function verifyMembershipProof(signals: string[], proof: Groth16Proof): Promise<boolean> {
  validateMembershipVerification(signals, proof);
  return new Promise((resolve, reject) => {
    queued.push({ signals: [...signals], proof: structuredClone(proof), resolve, reject });
    startNext();
  });
}

export function cancelMembershipVerifications(): void {
  const error = new Error('Membership verification was cancelled. Retry after reconnecting.');
  for (const job of queued.splice(0)) job.reject(error);
  active?.cancel();
}

function startNext(): void {
  if (active) return;
  const job = queued.shift();
  if (!job) return;
  let worker: Worker;
  try {
    worker = new Worker(new URL('../workers/membership-verification.worker.ts', import.meta.url), { type: 'module' });
  } catch {
    job.reject(new Error('The membership verifier could not start. Retry after reloading.'));
    queueMicrotask(startNext);
    return;
  }
  const jobId = crypto.randomUUID();
  let finished = false;
  const finish = (result: boolean | Error) => {
    if (finished) return;
    finished = true;
    clearTimeout(timer);
    worker.onmessage = null; worker.onerror = null; worker.onmessageerror = null;
    worker.terminate();
    active = null;
    if (result instanceof Error) job.reject(result); else job.resolve(result);
    queueMicrotask(startNext);
  };
  active = { cancel: () => finish(new Error('Membership verification was cancelled. Retry after reconnecting.')) };
  const timer = setTimeout(() => finish(new Error('Membership verification timed out. Retry to verify the channel.')), MEMBERSHIP_VERIFICATION_TIMEOUT_MS);
  worker.onerror = () => finish(new Error('The membership verifier failed. Retry to verify the channel.'));
  worker.onmessageerror = () => finish(new Error('The membership verifier returned an unreadable response.'));
  worker.onmessage = event => {
    const result = event.data as Record<string, unknown> | null;
    if (!result || result.jobId !== jobId) return;
    if (result.kind === 'result' && typeof result.valid === 'boolean') finish(result.valid);
    else finish(new Error('The membership verifier could not verify this proof.'));
  };
  try { worker.postMessage({ kind: 'verify', jobId, signals: job.signals, proof: job.proof }); }
  catch { finish(new Error('The membership verifier could not receive this proof.')); }
}

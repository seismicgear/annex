import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Groth16Proof } from 'snarkjs';
import { cancelMembershipVerifications, MEMBERSHIP_VERIFICATION_TIMEOUT_MS, verifyMembershipProof } from './membership-verification';

const proof = { protocol: 'groth16', curve: 'bn128', pi_a: ['1', '2', '1'], pi_b: [['1', '2'], ['3', '4'], ['1', '0']], pi_c: ['1', '2', '1'] } as Groth16Proof;
const signals = ['1', '2', '3', '4', '5'];
class TestWorker {
  static instances: TestWorker[] = [];
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessageerror: (() => void) | null = null;
  terminate = vi.fn();
  postMessage = vi.fn();
  constructor(readonly url: URL, readonly options: WorkerOptions) { TestWorker.instances.push(this); }
  result(valid: boolean) {
    this.onmessage?.({ data: { kind: 'result', jobId: this.postMessage.mock.calls[0][0].jobId, valid } });
  }
}
beforeEach(() => { vi.useFakeTimers(); TestWorker.instances = []; vi.stubGlobal('Worker', TestWorker); });
afterEach(() => { cancelMembershipVerifications(); vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('bounded browser membership verification', () => {
  it('uses a same-origin module worker without sending a selectable verifier', async () => {
    const pending = verifyMembershipProof(signals, proof);
    const worker = TestWorker.instances[0];
    expect(worker.url.pathname).toContain('/workers/membership-verification.worker.ts');
    expect(worker.url.protocol).not.toBe('data:');
    expect(worker.options).toEqual({ type: 'module' });
    expect(Object.keys(worker.postMessage.mock.calls[0][0]).sort()).toEqual(['jobId', 'kind', 'proof', 'signals']);
    worker.result(true);
    await expect(pending).resolves.toBe(true);
    expect(worker.terminate).toHaveBeenCalledOnce();
  });
  it('rejects malformed public inputs before starting expensive work', () => {
    for (const invalid of [signals.slice(0, 4), ['01', ...signals.slice(1)], ['-1', ...signals.slice(1)],
      ['21888242871839275222246405745257275088548364400416034343698204186575808495617', ...signals.slice(1)]]) {
      expect(() => verifyMembershipProof(invalid, proof)).toThrow('canonical');
    }
    expect(() => verifyMembershipProof(signals, { ...proof, pi_c: ['1', '2', '0'] })).toThrow('Invalid membership proof');
    expect(TestWorker.instances).toHaveLength(0);
  });
  it('returns an invalid proof result without accepting an unrelated worker message', async () => {
    const pending = verifyMembershipProof(signals, proof);
    const worker = TestWorker.instances[0];
    worker.onmessage?.({ data: { kind: 'result', jobId: 'another-job', valid: true } });
    expect(worker.terminate).not.toHaveBeenCalled();
    worker.result(false);
    await expect(pending).resolves.toBe(false);
  });
  it.each(['onerror', 'onmessageerror'] as const)('fails closed and releases workers on %s', async event => {
    const pending = verifyMembershipProof(signals, proof);
    const expectation = expect(pending).rejects.toThrow(event === 'onerror' ? 'failed' : 'unreadable');
    TestWorker.instances[0][event]?.();
    await expectation;
    expect(TestWorker.instances[0].terminate).toHaveBeenCalledOnce();
  });
  it('terminates a worker whose dependency never responds and permits a clean retry', async () => {
    const pending = verifyMembershipProof(signals, proof);
    const expectation = expect(pending).rejects.toThrow('timed out');
    await vi.advanceTimersByTimeAsync(MEMBERSHIP_VERIFICATION_TIMEOUT_MS);
    await expectation;
    expect(TestWorker.instances[0].terminate).toHaveBeenCalledOnce();
    const retry = verifyMembershipProof(signals, proof);
    TestWorker.instances[1].result(true);
    await expect(retry).resolves.toBe(true);
  });
  it('serializes independent verifications and cancels active and queued work on context exit', async () => {
    const first = verifyMembershipProof(signals, proof);
    const second = verifyMembershipProof(signals, proof);
    expect(TestWorker.instances).toHaveLength(1);
    TestWorker.instances[0].result(true);
    await expect(first).resolves.toBe(true);
    await vi.runAllTicks();
    expect(TestWorker.instances).toHaveLength(2);
    const third = verifyMembershipProof(signals, proof);
    const secondFailure = expect(second).rejects.toThrow('cancelled');
    const thirdFailure = expect(third).rejects.toThrow('cancelled');
    cancelMembershipVerifications();
    await Promise.all([secondFailure, thirdFailure]);
    expect(TestWorker.instances[1].terminate).toHaveBeenCalledOnce();
    await vi.runAllTicks();
    expect(TestWorker.instances).toHaveLength(2);
  });
});

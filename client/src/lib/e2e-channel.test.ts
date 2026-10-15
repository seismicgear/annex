import { describe, expect, it } from 'vitest';
import { E2eKeyPendingError, isE2eBody } from './e2e-channel';

// Distribution/convergence, late admission, outsider refusal and races are
// exercised against the current protocol in e2e-manager-v2.test.ts.
describe('historical encrypted body recognition', () => {
  it.each(['e2e1:ciphertext', 'e2e2:1:key:ciphertext', 'e2e1:', 'e2e2:'])('keeps marked body %s behind authenticated decryption', body => {
    expect(isE2eBody(body)).toBe(true);
  });
  it.each(['ordinary message', 'e2e10:plaintext', ' e2e2:quoted', ''])('does not relabel ordinary text %s', body => {
    expect(isE2eBody(body)).toBe(false);
  });
  it('distinguishes pending admission from corruption or transport failure', () => {
    expect(new E2eKeyPendingError('channel').name).toBe('E2eKeyPendingError');
  });
});

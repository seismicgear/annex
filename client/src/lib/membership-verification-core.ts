import { groth16, type Groth16Proof } from 'snarkjs';
import pinnedVerifier from './membership-v2-verifier';
import { validateMembershipVerification } from './membership-verification-input';

/** Worker-only computation; the verifier is shipped with the application, never supplied by a server. */
export async function verifyMembershipProof(signals: string[], proof: Groth16Proof): Promise<boolean> {
  validateMembershipVerification(signals, proof);
  return groth16.verify(pinnedVerifier, signals, proof);
}

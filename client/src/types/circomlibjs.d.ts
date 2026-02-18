/** Type declarations for circomlibjs (no official @types package). */
declare module 'circomlibjs' {
  export function buildPoseidon(): Promise<PoseidonHasher>;

  interface PoseidonHasher {
    (inputs: (bigint | number | string)[]): Uint8Array;
    F: FieldOperations;
  }

  interface FieldOperations {
    toObject(val: Uint8Array): bigint;
    toString(val: Uint8Array, radix?: number): string;
  }
}

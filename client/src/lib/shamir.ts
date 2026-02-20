/**
 * Shamir's Secret Sharing — split a secret key into shards that can be
 * distributed to trusted peers for social recovery.
 *
 * Operates over GF(2^8) using the irreducible polynomial x^8 + x^4 + x^3 + x + 1
 * (AES field). This allows splitting arbitrary byte strings.
 *
 * Security properties:
 * - Any (threshold) shards can reconstruct the secret.
 * - Fewer than (threshold) shards reveal zero information about the secret.
 */

// GF(2^8) arithmetic with polynomial 0x11b
function gfMul(a: number, b: number): number {
  let r = 0;
  let x = a;
  let y = b;
  while (y > 0) {
    if (y & 1) r ^= x;
    x <<= 1;
    if (x & 0x100) x ^= 0x11b;
    y >>= 1;
  }
  return r;
}

function gfInv(a: number): number {
  if (a === 0) throw new Error('Cannot invert zero in GF(2^8)');
  // Fermat's little theorem: a^(254) = a^(-1) in GF(2^8)
  let r = a;
  for (let i = 0; i < 6; i++) {
    r = gfMul(r, r);
    r = gfMul(r, a);
  }
  // One more square to reach exponent 254
  r = gfMul(r, r);
  return r;
}

function gfDiv(a: number, b: number): number {
  return gfMul(a, gfInv(b));
}

/**
 * Evaluate a polynomial at point x in GF(2^8).
 * coefficients[0] is the constant term (the secret byte).
 */
function polyEval(coefficients: number[], x: number): number {
  let result = 0;
  let xPow = 1;
  for (const coeff of coefficients) {
    result ^= gfMul(coeff, xPow);
    xPow = gfMul(xPow, x);
  }
  return result;
}

/**
 * Split a secret into n shares with a threshold of k.
 * Returns an array of shares, each is { index, data } where data matches
 * the length of the secret.
 */
export function split(
  secret: Uint8Array,
  totalShards: number,
  threshold: number,
): Array<{ index: number; data: Uint8Array }> {
  if (threshold < 2) throw new Error('Threshold must be at least 2');
  if (totalShards < threshold) throw new Error('Total shards must be >= threshold');
  if (totalShards > 255) throw new Error('Maximum 255 shards');

  const shares: Array<{ index: number; data: Uint8Array }> = [];
  for (let i = 0; i < totalShards; i++) {
    shares.push({ index: i + 1, data: new Uint8Array(secret.length) });
  }

  // For each byte of the secret, generate a random polynomial and evaluate
  for (let byteIdx = 0; byteIdx < secret.length; byteIdx++) {
    // coefficients[0] = secret byte, rest are random
    const coefficients = new Uint8Array(threshold);
    coefficients[0] = secret[byteIdx];
    crypto.getRandomValues(coefficients.subarray(1));

    for (let i = 0; i < totalShards; i++) {
      shares[i].data[byteIdx] = polyEval(Array.from(coefficients), i + 1);
    }
  }

  return shares;
}

/**
 * Reconstruct the secret from a subset of shares using Lagrange interpolation.
 * Requires exactly `threshold` shares.
 */
export function reconstruct(
  shares: Array<{ index: number; data: Uint8Array }>,
): Uint8Array {
  if (shares.length < 2) throw new Error('Need at least 2 shares to reconstruct');

  const secretLen = shares[0].data.length;
  const result = new Uint8Array(secretLen);

  for (let byteIdx = 0; byteIdx < secretLen; byteIdx++) {
    let value = 0;
    for (let i = 0; i < shares.length; i++) {
      const xi = shares[i].index;
      const yi = shares[i].data[byteIdx];

      // Compute Lagrange basis polynomial L_i(0)
      let basis = 1;
      for (let j = 0; j < shares.length; j++) {
        if (i === j) continue;
        const xj = shares[j].index;
        // L_i(0) = product of (0 - xj) / (xi - xj) = product of xj / (xi ^ xj)
        basis = gfMul(basis, gfDiv(xj, xi ^ xj));
      }

      value ^= gfMul(yi, basis);
    }
    result[byteIdx] = value;
  }

  return result;
}

/** Convert a hex string to Uint8Array. */
export function hexToBytes(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2) {
    bytes[i / 2] = parseInt(hex.slice(i, i + 2), 16);
  }
  return bytes;
}

/** Convert Uint8Array to hex string. */
export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

/**
 * Split a secret key (hex string) into recovery shards.
 * Returns hex-encoded shards.
 */
export function splitSecretKey(
  skHex: string,
  totalShards: number,
  threshold: number,
): Array<{ index: number; data: string }> {
  const secret = hexToBytes(skHex);
  const shares = split(secret, totalShards, threshold);
  return shares.map((s) => ({ index: s.index, data: bytesToHex(s.data) }));
}

/**
 * Reconstruct a secret key from hex-encoded shards.
 */
export function reconstructSecretKey(
  shards: Array<{ index: number; data: string }>,
): string {
  const shares = shards.map((s) => ({
    index: s.index,
    data: hexToBytes(s.data),
  }));
  return bytesToHex(reconstruct(shares));
}

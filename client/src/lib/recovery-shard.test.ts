import { describe, it, expect } from 'vitest';
import { splitSecretKey, reconstructSecretKey } from '@/lib/shamir';
import { parseShardPayload, serializeShardPayload, SHARD_FORMAT_VERSION } from '@/lib/recovery-shard';

const SK = 'a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90';

describe('Shamir under-threshold reconstruction', () => {
  it('returns a plausible WRONG key rather than failing', () => {
    // This is the property the recovery UI has to defend against. It is not a
    // bug in the scheme — interpolating fewer than `threshold` points is
    // well-defined, it just does not give you the secret. Nothing about the
    // return value says so.
    const shards = splitSecretKey(SK, 5, 3);
    expect(reconstructSecretKey(shards.slice(0, 3))).toBe(SK);

    const under = reconstructSecretKey(shards.slice(0, 2));
    expect(under).not.toBe(SK);
    expect(under).toMatch(/^[0-9a-f]{64}$/); // indistinguishable from a real key
  });

  it('reconstructs from any threshold-sized subset, in any order', () => {
    const shards = splitSecretKey(SK, 5, 3);
    expect(reconstructSecretKey([shards[4], shards[0], shards[2]])).toBe(SK);
  });
});

describe('parseShardPayload', () => {
  const valid = {
    v: SHARD_FORMAT_VERSION,
    index: 2,
    data: 'abcdef01',
    threshold: 3,
    totalShards: 5,
    roleCode: 1,
    nodeId: 482913,
    commitment: 'deadbeef',
    for: 'pseudo-abcdef',
  };

  it('round-trips a shard', () => {
    expect(parseShardPayload(serializeShardPayload(valid))).toEqual(valid);
  });

  it('tolerates surrounding whitespace from a paste', () => {
    expect(parseShardPayload(`\n  ${JSON.stringify(valid)}  \n`)).toEqual(valid);
  });

  it('rejects a bare hex share, which carries nothing to verify against', () => {
    expect(parseShardPayload('abcdef01')).toBeNull();
  });

  it('rejects malformed JSON', () => {
    expect(parseShardPayload('{"index": 2,')).toBeNull();
  });

  it.each([
    ['index', { index: 0 }],
    ['index', { index: 1.5 }],
    ['data', { data: 'nothex!' }],
    ['data', { data: '' }],
    ['threshold', { threshold: '3' }],
    ['totalShards', { totalShards: null }],
    ['roleCode', { roleCode: 'human' }],
    ['nodeId', { nodeId: undefined }],
    ['commitment', { commitment: 'zz' }],
  ])('rejects a bad %s', (_field, override) => {
    expect(parseShardPayload(JSON.stringify({ ...valid, ...override }))).toBeNull();
  });

  it('defaults the version when absent, so a v1-shaped blob is still typed', () => {
    const noVersion: Record<string, unknown> = { ...valid };
    delete noVersion.v;
    expect(parseShardPayload(JSON.stringify(noVersion))?.v).toBe(1);
  });
});

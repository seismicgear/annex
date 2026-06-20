import { describe, it, expect } from 'vitest';
import { computeTopicHashV2 } from '@/lib/zk';

describe('computeTopicHashV2', () => {
  it('matches the Rust topic_hash_for_v2 reference vectors', async () => {
    // Cross-checked against annex_identity::zk::topic_hash_for_v2 (Rust):
    //   Fr::from_be_bytes_mod_order(SHA256("annex/v2/topicHash:" + topic))
    // If these drift, every v2 proof would be rejected by the server, so this
    // is the canary that the client/server topic-hash derivations agree.
    expect(await computeTopicHashV2('annex:server:test:v2')).toBe(
      9005196282310870232398872685246750006718383063110863192771175239174093402005n,
    );
    expect(await computeTopicHashV2('annex:server:demo:v2')).toBe(
      19703114131541642153970314707561279789718245223453784328119590798701108187304n,
    );
  });

  it('is deterministic and topic-dependent', async () => {
    const a = await computeTopicHashV2('annex:server:x:v2');
    const b = await computeTopicHashV2('annex:server:x:v2');
    const c = await computeTopicHashV2('annex:server:y:v2');
    expect(a).toBe(b);
    expect(a).not.toBe(c);
  });

  it('is always a valid BN254 scalar (< field order)', async () => {
    const FR = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
    const h = await computeTopicHashV2('annex:server:bounds:v2');
    expect(h).toBeGreaterThanOrEqual(0n);
    expect(h).toBeLessThan(FR);
  });

  it('rejects an empty topic', async () => {
    await expect(computeTopicHashV2('')).rejects.toThrow();
  });
});

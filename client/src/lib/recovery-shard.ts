/**
 * The wire format of a single recovery shard — what one guardian is given,
 * and what gets pasted back in to recover.
 *
 * This exists because the two ends disagreed. The setup screen copied a JSON
 * blob to the clipboard; the recover screen asked for a number and a hex
 * string in two separate boxes, and said nothing about the JSON. So the
 * normal path — guardian sends you what the app gave them — did not work.
 *
 * The payload also has to carry enough to VERIFY the reconstruction. Shamir's
 * scheme cannot detect an under-threshold recovery: interpolating any k points
 * yields some value, and with k < threshold that value is simply the wrong
 * key, returned without error. `reconstruct` only refuses fewer than two
 * shares. Everything above that came back looking like a successful recovery.
 *
 * So each shard also carries the identity's public parameters. The commitment
 * is already public (it is a Merkle leaf on the server) and `roleCode` /
 * `nodeId` are not what protects the key — `sk` is, and that is the part
 * actually split. Carrying them means a recovery can recompute the commitment
 * and check it against the one the shards agree on, and it means the recovered
 * identity is the ORIGINAL identity rather than a new one derived from the old
 * secret key.
 */

/** Current shard format version. */
export const SHARD_FORMAT_VERSION = 2;

export interface ShardPayload {
  v: number;
  /** 1-based share index. */
  index: number;
  /** Hex-encoded share of the secret key. */
  data: string;
  /** How many shards are needed to reconstruct. */
  threshold: number;
  /** How many shards were handed out in total. */
  totalShards: number;
  /** Identity role code, needed to recompute the commitment. */
  roleCode: number;
  /** Identity node id, needed to recompute the commitment. */
  nodeId: number;
  /** The identity commitment this shard set reconstructs to. */
  commitment: string;
  /** Truncated pseudonym, so a guardian can tell whose shard this is. */
  for?: string;
}

function isHex(s: unknown): s is string {
  return typeof s === 'string' && s.length > 0 && /^[0-9a-fA-F]+$/.test(s);
}

/**
 * Parse text a guardian sent back.
 *
 * Accepts the JSON blob the setup screen produces. Returns null for anything
 * else — including a bare hex string, which is a valid share but carries none
 * of the parameters needed to verify the result. The caller decides what to
 * say about that; it must not be treated as a shard that can be trusted.
 */
export function parseShardPayload(text: string): ShardPayload | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith('{')) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(trimmed);
  } catch {
    return null;
  }
  if (typeof raw !== 'object' || raw === null) return null;
  const o = raw as Record<string, unknown>;
  if (
    typeof o.index !== 'number' ||
    !Number.isInteger(o.index) ||
    o.index < 1 ||
    !isHex(o.data) ||
    typeof o.threshold !== 'number' ||
    typeof o.totalShards !== 'number' ||
    typeof o.roleCode !== 'number' ||
    typeof o.nodeId !== 'number' ||
    !isHex(o.commitment)
  ) {
    return null;
  }
  return {
    v: typeof o.v === 'number' ? o.v : 1,
    index: o.index,
    data: o.data,
    threshold: o.threshold,
    totalShards: o.totalShards,
    roleCode: o.roleCode,
    nodeId: o.nodeId,
    commitment: o.commitment,
    for: typeof o.for === 'string' ? o.for : undefined,
  };
}

/** Serialize a shard for a guardian to store. */
export function serializeShardPayload(p: ShardPayload): string {
  return JSON.stringify(p);
}

/**
 * Whether `text` is shaped like a shard blob at all — a JSON object carrying
 * an `index` and `data` — regardless of whether it carries the fields a
 * verified recovery needs.
 *
 * Lets the caller tell "you pasted a bare hex string" apart from "you pasted a
 * shard from an older version". Both fail `parseShardPayload`, and telling
 * someone holding the latter to paste "the block starting with `{`" is
 * nonsense: they did.
 */
export function looksLikeShardJson(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed.startsWith('{')) return false;
  try {
    const raw = JSON.parse(trimmed) as Record<string, unknown>;
    return typeof raw === 'object' && raw !== null && 'index' in raw && 'data' in raw;
  } catch {
    return false;
  }
}

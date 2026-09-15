-- `agent_min_alignment_score` changed scale, and a stored value did not.
--
-- The column has always held a bare f32 inside `servers.policy_json`, and it
-- used to be compared against a RAW cosine from the lexicon scorer. It is now
-- compared against a score normalised against that scorer's own noise floor
-- (`annex_vrp::semantic::normalize_against_floor`), so that the same
-- configured number means the same strictness on the pinned potion-base-2M
-- table and on the lexicon fallback. Without this migration a server upgraded
-- in place keeps a number that meant one thing and is now read as another.
--
-- The shipped default also moved, 0.8 -> 0.06, and the reason matters here:
-- 0.8 was never reachable. Measured on the labelled corpus in
-- `crates/annex-vrp/tests/alignment_calibration.rs`, the lexicon scorer puts
-- every unrelated pair at or below a raw 0.3060 and every genuine paraphrase
-- at or above 0.3918. A threshold of 0.8 sat above BOTH bands: the only
-- anchors that ever passed were the ones matching by hash, which short-circuit
-- before the comparison runs. Faithfully converting 0.8 to the new scale gives
-- (0.8 - 0.306) / 0.694 = 0.712, which is equally unreachable — so a faithful
-- conversion would preserve the defect rather than fix it.
--
-- Hence two cases:
--
--   * A value that IS the old default was never chosen by anyone. It becomes
--     the new default.
--   * Any other value was set deliberately, on the old raw scale, so it is
--     converted through that scale's floor and clamped to [0, 1]. That may
--     still be strict — an operator who deliberately set 0.9 gets 0.856 and
--     will still refuse almost everything — but it is what they asked for,
--     expressed in the new units, and it is theirs to change.
--
-- `server_policy_versions` is deliberately NOT rewritten. It is an append-only
-- record of what was active at a point in time, written by
-- `api_admin.rs::update_policy` and never read back into a running server.
-- Rewriting history to match a later scale would make the audit trail lie.

UPDATE servers
SET policy_json = json_set(
      policy_json,
      '$.agent_min_alignment_score',
      CASE
        -- The old shipped default, to a tolerance that survives the f32 ->
        -- JSON text -> f64 round trip.
        WHEN abs(json_extract(policy_json, '$.agent_min_alignment_score') - 0.8) < 1e-6
          THEN 0.06
        ELSE max(
               0.0,
               min(1.0,
                 (json_extract(policy_json, '$.agent_min_alignment_score') - 0.3060)
                 / (1.0 - 0.3060)))
      END)
WHERE json_valid(policy_json)
  AND json_type(policy_json, '$.agent_min_alignment_score') IN ('integer', 'real');

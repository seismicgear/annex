-- Promote the first identity on each server to founder with full capabilities.
-- This is a one-time migration for databases created before the auto-founder logic.
UPDATE platform_identities
SET can_voice = 1,
    can_moderate = 1,
    can_invite = 1,
    can_federate = 1,
    updated_at = datetime('now')
WHERE id IN (
    SELECT MIN(id)
    FROM platform_identities
    GROUP BY server_id
);

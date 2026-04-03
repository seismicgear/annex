-- Add public_url column to servers table so admin-set public URLs
-- survive server restarts. Previously the URL was only in-memory.
ALTER TABLE servers ADD COLUMN public_url TEXT NOT NULL DEFAULT '';

-- Add a free-text description column to servers.
-- Used by monolithannex.com for OG meta tag previews on invite links.
ALTER TABLE servers ADD COLUMN description TEXT NOT NULL DEFAULT '';

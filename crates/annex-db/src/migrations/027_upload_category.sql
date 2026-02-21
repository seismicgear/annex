-- Add category column to uploads for image/video/file classification.
ALTER TABLE uploads ADD COLUMN category TEXT NOT NULL DEFAULT 'image';

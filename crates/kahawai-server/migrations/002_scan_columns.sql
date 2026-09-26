-- S1: columns the scanner needs for incremental rescan and richer cataloging.
-- Applied by db::run_migrations via the schema_migrations version table.

ALTER TABLE tracks ADD COLUMN file_size INTEGER;
ALTER TABLE tracks ADD COLUMN file_mtime INTEGER;
ALTER TABLE tracks ADD COLUMN missing INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tracks ADD COLUMN decodable INTEGER NOT NULL DEFAULT 1;
ALTER TABLE tracks ADD COLUMN genre TEXT;
ALTER TABLE tracks ADD COLUMN year INTEGER;
ALTER TABLE tracks ADD COLUMN artwork_hash TEXT;
CREATE INDEX IF NOT EXISTS idx_tracks_missing ON tracks(missing);

ALTER TABLE scan_log ADD COLUMN files_updated INTEGER NOT NULL DEFAULT 0;
ALTER TABLE scan_log ADD COLUMN files_missing INTEGER NOT NULL DEFAULT 0;
ALTER TABLE scan_log ADD COLUMN files_skipped INTEGER NOT NULL DEFAULT 0;

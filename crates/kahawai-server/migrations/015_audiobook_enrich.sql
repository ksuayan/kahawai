-- When a book was last looked up online (Open Library, Google Books), so a
-- book is asked about once. NULL = not yet. Lookups only fill blanks.
ALTER TABLE audiobooks ADD COLUMN enriched_at INTEGER;

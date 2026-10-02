-- A book whose files are byte-for-byte the same as another book's (the same
-- folder copied, or a backup) points at the one kept, and is left out of the
-- library. It stays in the table, with its files, so nothing is lost if the
-- kept copy goes away: the next scan promotes it.
ALTER TABLE audiobooks ADD COLUMN duplicate_of INTEGER;

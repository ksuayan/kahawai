-- The scanner matches every cataloged track to an album by title (and, for
-- files without an album-artist tag, by the folders of that title's
-- tracks). Without this index each lookup scanned the albums table, and the
-- same-folder join scanned every track, so writes slowed as the catalog grew.

CREATE INDEX IF NOT EXISTS idx_albums_title ON albums(title);

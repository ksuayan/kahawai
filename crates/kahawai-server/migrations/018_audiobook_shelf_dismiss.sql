-- A book taken off the Continue listening shelf keeps its place: the position
-- stays, it is just not listed. Listening again (the position moving) puts it
-- back.
ALTER TABLE audiobook_positions ADD COLUMN dismissed INTEGER NOT NULL DEFAULT 0;

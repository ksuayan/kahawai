-- 003: durable job queue (spec §3.7, phase 5 S9).
--
-- Jobs used to live only in process memory, so a restart silently lost
-- them. Now every mutation is written through to this table. At startup
-- any row still marked queued/running is failed with the error
-- "server restarted" (never silently resumed), while done/failed rows
-- stay intact.
--
-- Column contract: payload is the machine-readable job input (e.g. the
-- SACD ISO path for extract_iso jobs), result carries the success message
-- (scan counts for scan jobs), and error carries the failure message.
-- label is the human toast label from the API Job type, kept so the job
-- list still reads well after a restart.

CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    progress REAL NOT NULL DEFAULT 0,
    payload TEXT,
    result TEXT,
    error TEXT,
    label TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs (status);

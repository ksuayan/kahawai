-- When each job ran, for the desktop app's Recent scans list (the local
-- time it started, and how long it took, done or failed).
--
-- Unix milliseconds (UTC), set by the job store: started_at when the job
-- first runs, finished_at when it ends (done, failed, cancelled, or failed
-- by the restart rule). Existing rows get the best guess the old columns
-- allow: created_at as the start, and updated_at as the end of a job that
-- has ended.

ALTER TABLE jobs ADD COLUMN started_at INTEGER;
ALTER TABLE jobs ADD COLUMN finished_at INTEGER;
UPDATE jobs SET started_at = CAST(strftime('%s', created_at) AS INTEGER) * 1000
 WHERE status <> 'queued';
UPDATE jobs SET finished_at = CAST(strftime('%s', updated_at) AS INTEGER) * 1000
 WHERE status IN ('done', 'failed', 'cancelled');

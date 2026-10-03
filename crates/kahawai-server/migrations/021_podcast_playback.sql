-- Podcast playback (docs/v2/kahawai-podcast-spec.md, D4 and the playback half
-- of D7): where you are in each episode, when you listened (sessions, derived
-- like the audiobooks'), and each show's speed, skips and auto-advance.
CREATE TABLE IF NOT EXISTS podcast_positions (
  episode_id  INTEGER PRIMARY KEY REFERENCES podcast_episodes(id) ON DELETE CASCADE,
  offset_ms   INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS podcast_sessions (
  id               INTEGER PRIMARY KEY,
  episode_id       INTEGER NOT NULL REFERENCES podcast_episodes(id) ON DELETE CASCADE,
  started_at       INTEGER NOT NULL,
  ended_at         INTEGER NOT NULL,
  start_offset_ms  INTEGER NOT NULL,
  end_offset_ms    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_podcast_sessions_episode ON podcast_sessions(episode_id, started_at DESC);

ALTER TABLE podcast_feeds ADD COLUMN speed REAL NOT NULL DEFAULT 1.0;
ALTER TABLE podcast_feeds ADD COLUMN skip_back_s INTEGER NOT NULL DEFAULT 15;
ALTER TABLE podcast_feeds ADD COLUMN skip_forward_s INTEGER NOT NULL DEFAULT 30;
-- When an episode ends with nothing in Up Next, play this show's next unplayed one.
ALTER TABLE podcast_feeds ADD COLUMN auto_advance INTEGER NOT NULL DEFAULT 0;

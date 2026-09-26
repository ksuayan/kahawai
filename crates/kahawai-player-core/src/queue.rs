//! Playback queue with shuffle and repeat. Pure state machine — no I/O,
//! no threads — so iOS/Android shells share it verbatim.
//! (Spec: kahawai-player-design.md §"Playback core".)

use kahawai_core::{NewPlaylist, Track};
use serde::{Deserialize, Serialize};

/// Repeat behavior for the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

/// A playback queue: an ordered list plus a cursor.
#[derive(Debug, Default)]
pub struct Queue {
    tracks: Vec<Track>,
    /// Index into `tracks` of the currently playing item.
    cursor: Option<usize>,
    pub repeat: RepeatMode,
    pub shuffle: bool,
    /// Shuffled playback order: permutation of indices into `tracks`.
    order: Vec<usize>,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the queue contents. Cursor resets to the first item.
    pub fn set_tracks(&mut self, tracks: Vec<Track>) {
        self.tracks = tracks;
        self.cursor = if self.tracks.is_empty() {
            None
        } else {
            Some(0)
        };
        self.rebuild_order();
    }

    pub fn current(&self) -> Option<&Track> {
        let i = self.cursor?;
        if self.shuffle {
            self.order.get(i).and_then(|&j| self.tracks.get(j))
        } else {
            self.tracks.get(i)
        }
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    /// Index of the current track in stable list order (`usize::MAX` when
    /// the queue is empty — callers should pair with [`Self::current`]).
    pub fn index(&self) -> usize {
        self.cursor.unwrap_or(usize::MAX)
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Tracks in stable list order (not shuffle order).
    pub fn ordered_tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Track ids in stable list order (not shuffle order).
    pub fn ordered_ids(&self) -> Vec<i64> {
        self.tracks.iter().map(|t| t.id).collect()
    }

    /// Build the `POST /api/playlists` body that saves the current queue
    /// as a playlist (spec S11). Dependency-free: just the ordered id
    /// list. The queue itself stays client-side — the server only persists
    /// the ids it is given. Saved in queue (list) order, not shuffle
    /// order: a playlist is the stable list, shuffle is a playback mode.
    pub fn save_as_playlist_request(&self, name: &str) -> NewPlaylist {
        NewPlaylist {
            name: name.to_string(),
            track_ids: Vec::new(),
            from_queue: true,
            queue_track_ids: self.tracks.iter().map(|t| t.id).collect(),
        }
    }

    /// The track that would play after the current one, without moving the
    /// cursor. The engine uses this for `?next=` gapless chaining; repeat-one
    /// yields `None` (the current track repeats instead of chaining).
    pub fn peek_next(&self) -> Option<&Track> {
        let i = self.cursor?;
        if self.repeat == RepeatMode::One {
            return None;
        }
        let next_i = i + 1;
        if next_i < self.tracks.len() {
            self.track_at(next_i)
        } else if self.repeat == RepeatMode::All && !self.tracks.is_empty() {
            self.track_at(0)
        } else {
            None
        }
    }

    fn track_at(&self, i: usize) -> Option<&Track> {
        if self.shuffle {
            self.order.get(i).and_then(|&j| self.tracks.get(j))
        } else {
            self.tracks.get(i)
        }
    }
    /// Advance to the next item. Honors repeat-one (stay) and repeat-off
    /// (stop at the end). Returns the new current item, or None if the queue
    /// ended.
    pub fn next_track(&mut self) -> Option<&Track> {
        let i = self.cursor?;
        if self.repeat == RepeatMode::One {
            return self.current();
        }
        let next_i = i + 1;
        if next_i < self.tracks.len() {
            self.cursor = Some(next_i);
            self.current()
        } else if self.repeat == RepeatMode::All && !self.tracks.is_empty() {
            self.cursor = Some(0);
            self.current()
        } else {
            self.cursor = None;
            None
        }
    }

    pub fn prev_track(&mut self) -> Option<&Track> {
        let i = self.cursor?;
        if self.repeat == RepeatMode::One {
            return self.current();
        }
        if i > 0 {
            self.cursor = Some(i - 1);
            self.current()
        } else if self.repeat == RepeatMode::All && !self.tracks.is_empty() {
            self.cursor = Some(self.tracks.len() - 1);
            self.current()
        } else {
            self.current()
        }
    }

    pub fn set_shuffle(&mut self, on: bool) {
        // Preserve the currently playing track across the toggle.
        let current_id = self.current().map(|t| t.id);
        self.shuffle = on;
        self.rebuild_order();
        if let Some(id) = current_id {
            let pos = self
                .order
                .iter()
                .position(|&j| self.tracks.get(j).map(|t| t.id) == Some(id));
            self.cursor = pos.or(Some(0));
        }
    }

    fn rebuild_order(&mut self) {
        self.order = (0..self.tracks.len()).collect();
        if self.shuffle {
            // Deterministic Fisher-Yates with a fixed seed: reproducible in
            // tests; the player shell will seed from entropy.
            // TODO(player): proper RNG (rand crate) seeded from the OS.
            let mut seed: u64 = 0x9E3779B97F4A7C15;
            let mut rand = move || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed
            };
            for i in (1..self.order.len()).rev() {
                let j = (rand() % (i as u64 + 1)) as usize;
                self.order.swap(i, j);
            }
        }
    }

    /// Append tracks to the end of the queue (stable list order). The
    /// current item and playback position are untouched; when the queue was
    /// empty the cursor lands on the first new track so a later play starts
    /// there. Used for "add to queue".
    pub fn append(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        let base = self.tracks.len();
        let n = tracks.len();
        self.tracks.extend(tracks);
        self.order.extend(base..base + n);
        if self.cursor.is_none() {
            self.cursor = Some(self.playback_pos_of(base));
        }
    }

    /// Insert tracks immediately after the current item in *playback* order
    /// (shuffle-aware). With no cursor (empty or exhausted queue) this
    /// behaves like [`Self::append`]. Used for "play next".
    pub fn insert_after_current(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        let n = tracks.len();
        let Some(cursor) = self.cursor else {
            self.append(tracks);
            return;
        };
        // Stable index in `tracks` right after the current item.
        let stable_pos = if self.shuffle {
            self.order.get(cursor).copied().unwrap_or(self.tracks.len())
        } else {
            cursor
        };
        let insert_at = (stable_pos + 1).min(self.tracks.len());
        self.tracks.splice(insert_at..insert_at, tracks);
        // Shift permutation entries that pointed at/after the insertion point.
        for j in self.order.iter_mut() {
            if *j >= insert_at {
                *j += n;
            }
        }
        // Splice the new stable indices into playback order after `cursor`.
        let at = (cursor + 1).min(self.order.len());
        self.order.splice(at..at, insert_at..insert_at + n);
    }

    /// Playback-order position of the stable track index `stable`, i.e. the
    /// cursor value that would make it current.
    fn playback_pos_of(&self, stable: usize) -> usize {
        if self.shuffle {
            self.order.iter().position(|&j| j == stable).unwrap_or(0)
        } else {
            stable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kahawai_core::{format::AudioFormat, Track};

    fn track(id: i64) -> Track {
        Track {
            id,
            path: format!("/m/{id}.mp3"),
            hash: String::new(),
            format: AudioFormat::Mp3,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            duration_ms: None,
            bitrate: None,
            title: None,
            album: None,
            artist: None,
            album_id: None,
            track_no: None,
            disc_no: None,
            genre: None,
            year: None,
            missing: false,
            decodable: true,
            mqa: false,
            original_sample_rate: None,
        }
    }

    #[test]
    fn next_walks_and_stops_at_end_by_default() {
        let mut q = Queue::new();
        q.set_tracks(vec![track(1), track(2), track(3)]);
        assert_eq!(q.current().unwrap().id, 1);
        assert_eq!(q.next_track().unwrap().id, 2);
        assert_eq!(q.next_track().unwrap().id, 3);
        assert!(q.next_track().is_none());
        assert!(q.current().is_none());
    }

    #[test]
    fn repeat_all_wraps() {
        let mut q = Queue::new();
        q.repeat = RepeatMode::All;
        q.set_tracks(vec![track(1), track(2)]);
        assert_eq!(q.next_track().unwrap().id, 2);
        assert_eq!(q.next_track().unwrap().id, 1); // wrapped
        assert_eq!(q.prev_track().unwrap().id, 2); // wrapped backwards
    }

    #[test]
    fn repeat_one_stays() {
        let mut q = Queue::new();
        q.repeat = RepeatMode::One;
        q.set_tracks(vec![track(1), track(2)]);
        assert_eq!(q.next_track().unwrap().id, 1);
        assert_eq!(q.prev_track().unwrap().id, 1);
    }

    #[test]
    fn shuffle_produces_a_full_permutation() {
        let mut q = Queue::new();
        q.set_tracks((1..=20).map(track).collect());
        q.set_shuffle(true);
        // The shuffled order must contain every index exactly once.
        let mut sorted = q.order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..20).collect::<Vec<_>>());

        // And walking from the first shuffled position visits all 20 tracks.
        q.cursor = Some(0);
        let mut seen: Vec<i64> = vec![q.current().unwrap().id];
        while q.next_track().is_some() {
            seen.push(q.current().unwrap().id);
        }
        seen.sort_unstable();
        assert_eq!(seen, (1..=20).collect::<Vec<_>>());
    }

    #[test]
    fn shuffle_toggle_preserves_current_track() {
        let mut q = Queue::new();
        q.set_tracks(vec![track(1), track(2), track(3)]);
        q.next_track(); // now on track 2
        q.set_shuffle(true);
        assert_eq!(q.current().unwrap().id, 2);
        q.set_shuffle(false);
        assert_eq!(q.current().unwrap().id, 2);
    }

    #[test]
    fn empty_queue_is_safe() {
        let mut q = Queue::new();
        assert!(q.current().is_none());
        assert!(q.next_track().is_none());
        assert!(q.prev_track().is_none());
        q.set_shuffle(true);
        assert!(q.current().is_none());
    }

    /// S11: the queue can build the `POST /api/playlists` body that saves
    /// it server-side — ordered id list in queue (list) order, with
    /// `from_queue` set so the server knows the ids are the source of
    /// truth. Shuffle order is a playback mode, not the saved order.
    #[test]
    fn save_as_playlist_request_uses_queue_order() {
        let mut q = Queue::new();
        q.set_tracks(vec![track(7), track(3), track(9)]);
        q.set_shuffle(true);
        let req = q.save_as_playlist_request("Road trip");
        assert_eq!(req.name, "Road trip");
        assert!(req.from_queue);
        assert_eq!(req.queue_track_ids, vec![7, 3, 9]);
        assert!(req.track_ids.is_empty());
        // Serializes to the shape POST /api/playlists expects.
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""from_queue":true"#));
        assert!(json.contains(r#""queue_track_ids":[7,3,9]"#));
    }

    #[test]
    fn repeat_mode_serde_round_trip() {
        for m in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
            let s = serde_json::to_string(&m).unwrap();
            let back: RepeatMode = serde_json::from_str(&s).unwrap();
            assert_eq!(back, m);
        }
        assert_eq!(serde_json::to_string(&RepeatMode::All).unwrap(), "\"all\"");
    }

    fn ids(q: &Queue) -> Vec<i64> {
        q.ordered_tracks().iter().map(|t| t.id).collect()
    }

    #[test]
    fn append_keeps_cursor_and_current() {
        let mut q = Queue::new();
        q.set_tracks(vec![track(1), track(2)]);
        q.next_track(); // now on track 2
        q.append(vec![track(3)]);
        assert_eq!(ids(&q), vec![1, 2, 3]);
        assert_eq!(q.current().unwrap().id, 2);
        assert_eq!(q.index(), 1);
        // Appending to an empty queue points the cursor at the first track.
        let mut q2 = Queue::new();
        q2.append(vec![track(7)]);
        assert_eq!(q2.current().unwrap().id, 7);
    }

    #[test]
    fn insert_after_current_plain_order() {
        let mut q = Queue::new();
        q.set_tracks(vec![track(1), track(2), track(3)]);
        q.insert_after_current(vec![track(9)]);
        assert_eq!(ids(&q), vec![1, 9, 2, 3]);
        assert_eq!(q.current().unwrap().id, 1);
        assert_eq!(q.next_track().unwrap().id, 9);
        assert_eq!(q.next_track().unwrap().id, 2);
    }

    #[test]
    fn insert_after_current_shuffle_order() {
        let mut q = Queue::new();
        q.set_tracks((1..=5).map(track).collect());
        q.set_shuffle(true);
        let first = q.current().unwrap().id;
        q.insert_after_current(vec![track(99)]);
        // Current track unchanged; 99 plays immediately next in shuffle order.
        assert_eq!(q.current().unwrap().id, first);
        assert_eq!(q.next_track().unwrap().id, 99);
        // Permutation stays valid: every stable index exactly once.
        let mut sorted = q.order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..6).collect::<Vec<_>>());
    }

    #[test]
    fn insert_after_current_empty_queue_appends() {
        let mut q = Queue::new();
        q.insert_after_current(vec![track(5)]);
        assert_eq!(ids(&q), vec![5]);
        assert_eq!(q.current().unwrap().id, 5);
    }
}

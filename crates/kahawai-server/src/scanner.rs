//! Library scanner: walk music dirs, BLAKE3-hash, extract metadata via lofty,
//! write the SQLite catalog, incremental rescan. (Spec §3.1, S1.)
//!
//! Design notes:
//! - The walk + per-file analysis (hash + lofty) run in `spawn_blocking`
//!   workers; all SQLite I/O stays on the async side. One scan at a time is
//!   enforced by the `scan_lock` in `AppState`.
//! - Incremental rescan: a file whose (path, size, mtime) is unchanged is
//!   skipped without hashing. A changed file is re-hashed; if the content
//!   hash matches, only the stat columns are refreshed, otherwise the full
//!   metadata row is rewritten. Files gone from disk are marked
//!   `missing = 1`, never deleted (relink-friendly, spec §3.1).
//! - Technical properties (duration, sample rate, bit depth, channels,
//!   bitrate) come from **lofty** alone, not symphonia: one metadata path,
//!   and lofty's `FileProperties` proved reliable against ffmpeg-generated
//!   fixtures for MP3/FLAC/OGG/M4A/WAV (see tests below).
//! - DSF/DFF are parsed with the server's own DSD parser (`dsd.rs`: rate,
//!   channels, duration) and their ID3v2 tags are read by `dsd_meta.rs`
//!   (lofty cannot open DSD). They are cataloged like any other track and are
//!   `decodable`: the server plays them through its DSD→PCM/DoP paths.
//! - A DSF/DFF that does not parse, and SACD ISOs, are still cataloged with
//!   `decodable = 0` and NULL technical fields: browsable, not playable.

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    time::{Instant, UNIX_EPOCH},
};

use kahawai_core::{AudioFormat, MusicError};
use lofty::prelude::*;
use lofty::tag::{Accessor, ItemKey};
use sqlx::{sqlite::SqlitePool, Row};
use tracing::{info, warn};

use crate::db;

/// Outcome of one scan run. Recorded in `scan_log`.
#[derive(Debug, Default)]
pub struct ScanReport {
    pub files_seen: u64,
    pub audio_files: u64,
    pub files_added: u64,
    pub files_updated: u64,
    pub files_skipped: u64,
    pub files_missing: u64,
    pub elapsed_secs: f64,
}

/// Everything the blocking worker learns about one file.
struct FileAnalysis {
    path: String,
    hash: String,
    size: i64,
    mtime: Option<i64>,
    format: AudioFormat,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    genre: Option<String>,
    year: Option<u16>,
    track_no: Option<u32>,
    disc_no: Option<u32>,
    duration_ms: Option<u64>,
    sample_rate: Option<u32>,
    bit_depth: Option<u8>,
    channels: Option<u8>,
    bitrate: Option<u32>,
    artwork: Option<ArtworkData>,
    /// The server can play it (directly, or via its DSD paths).
    decodable: bool,
    /// MQA-encoded (MQAENCODER tag) and the pre-fold master rate.
    mqa: bool,
    original_sample_rate: Option<u32>,
}

struct ArtworkData {
    mime: String,
    bytes: Vec<u8>,
    hash: String,
}

/// Run a full scan of `dirs`, writing to the catalog. Async; per-file
/// analysis is offloaded to blocking workers.
/// [`run_scan`] with a progress callback. `on_progress(processed, total)`
/// fires once per audio file, so `processed / total` is monotonic 0→1.
/// Total comes from a first metadata-only walk (S9: "two-phase walk, count
/// then scan"); the hashing walk below dominates the cost.
pub async fn run_scan_with_progress(
    pool: &SqlitePool,
    dirs: &[PathBuf],
    on_progress: impl Fn(u64, u64) + Send + Sync,
) -> Result<ScanReport, MusicError> {
    let start = Instant::now();
    let total_audio: u64 = dirs
        .iter()
        .map(|dir| {
            walkdir::WalkDir::new(dir)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file() && is_audio(e.path()))
                .count() as u64
        })
        .sum();
    let mut processed_audio: u64 = 0;
    let scan_id: i64 =
        sqlx::query("INSERT INTO scan_log (started_at) VALUES (datetime('now')) RETURNING id")
            .fetch_one(pool)
            .await
            .map_err(db::cvt)?
            .get("id");

    // path -> (hash, size, mtime, missing) for change detection.
    let rows = sqlx::query("SELECT path, hash, file_size, file_mtime, missing FROM tracks")
        .fetch_all(pool)
        .await
        .map_err(db::cvt)?;
    let mut known: HashMap<String, (String, Option<i64>, Option<i64>, i64)> =
        HashMap::with_capacity(rows.len());
    // DSD rows cataloged before DSD support (no rate = never analyzed) are
    // re-read even though the file itself is unchanged.
    let stale: HashSet<String> = sqlx::query(
        "SELECT path FROM tracks WHERE format IN ('dsf', 'dff') AND sample_rate IS NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(db::cvt)?
    .iter()
    .map(|r| r.get::<String, _>("path"))
    .collect();
    for r in &rows {
        known.insert(
            r.get("path"),
            (
                r.get("hash"),
                r.get("file_size"),
                r.get("file_mtime"),
                r.get("missing"),
            ),
        );
    }

    // Rows cataloged before MQA detection existed: tags are read (no hashing)
    // the next time the file is visited.
    let mut mqa_unchecked: HashSet<String> =
        sqlx::query("SELECT path FROM tracks WHERE mqa_checked = 0")
            .fetch_all(pool)
            .await
            .map_err(db::cvt)?
            .iter()
            .map(|r| r.get::<String, _>("path"))
            .collect();

    let mut report = ScanReport::default();
    let mut seen: HashSet<String> = HashSet::with_capacity(known.len().max(1024));

    for dir in dirs {
        let walker = walkdir::WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok());
        for entry in walker {
            if !entry.file_type().is_file() {
                continue;
            }
            report.files_seen += 1;
            let path = entry.path();
            if !is_audio(path) {
                continue;
            }
            report.audio_files += 1;
            processed_audio += 1;
            on_progress(processed_audio, total_audio);
            let path_str = path.to_string_lossy().to_string();
            // The file exists on disk; record that before analysis so a
            // failed read doesn't mark a present file as missing.
            seen.insert(path_str.clone());

            let (size, mtime) = match std::fs::metadata(path) {
                Ok(m) => (
                    m.len() as i64,
                    m.modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64),
                ),
                Err(e) => {
                    warn!(path = %path_str, error = %e, "stat failed; skipping");
                    continue;
                }
            };

            // Fast path: unchanged since last scan.
            match known.get(&path_str) {
                Some((_, ksize, kmtime, _))
                    if *ksize == Some(size) && *kmtime == mtime && !stale.contains(&path_str) =>
                {
                    report.files_skipped += 1;
                    if mqa_unchecked.remove(&path_str) {
                        let owned = path.to_path_buf();
                        let (mqa, original) = tokio::task::spawn_blocking(move || read_mqa(&owned))
                            .await
                            .map_err(|e| {
                                MusicError::JobFailed(format!("scan worker panicked: {e}"))
                            })?;
                        sqlx::query(
                            "UPDATE tracks SET mqa = ?, original_sample_rate = ?, mqa_checked = 1
                             WHERE path = ?",
                        )
                        .bind(i64::from(mqa))
                        .bind(original.map(i64::from))
                        .bind(&path_str)
                        .execute(pool)
                        .await
                        .map_err(db::cvt)?;
                    }
                }
                previous => {
                    let is_new = previous.is_none();
                    let owned = path.to_path_buf();
                    let analysis = tokio::task::spawn_blocking(move || analyze_file(&owned))
                        .await
                        .map_err(|e| MusicError::JobFailed(format!("scan worker panicked: {e}")))?;
                    match analysis {
                        Ok(a) => {
                            let hash_changed =
                                previous.map(|(h, _, _, _)| h.as_str()) != Some(a.hash.as_str());
                            if is_new || hash_changed || stale.contains(&path_str) {
                                // One transaction per track: track row, album,
                                // artists, artwork, and FTS all land together.
                                let mut tx = pool.begin().await.map_err(db::cvt)?;
                                upsert_track(&mut tx, &a).await?;
                                tx.commit().await.map_err(db::cvt)?;
                                if is_new {
                                    report.files_added += 1;
                                } else {
                                    report.files_updated += 1;
                                }
                            } else {
                                // Touched but identical content: refresh stat columns only.
                                sqlx::query(
                                    "UPDATE tracks SET file_size = ?, file_mtime = ?, missing = 0 WHERE path = ?",
                                )
                                .bind(a.size)
                                .bind(a.mtime)
                                .bind(&path_str)
                                .execute(pool)
                                .await
                                .map_err(db::cvt)?;
                                report.files_updated += 1;
                            }
                        }
                        Err(e) => {
                            warn!(path = %path_str, error = %e, "analyze failed; skipping file");
                            continue;
                        }
                    }
                }
            }

            if report.files_seen % 200 == 0 {
                let mins = (start.elapsed().as_secs_f64() / 60.0).max(1e-9);
                info!(
                    files_seen = report.files_seen,
                    audio_files = report.audio_files,
                    files_per_min = (report.files_seen as f64 / mins) as u64,
                    "scan progress"
                );
            }
        }
    }

    // Anything in the catalog but not on disk is marked missing, not deleted.
    // files_missing counts newly-missing files only, matching the delta
    // semantics of files_added / files_updated.
    for (path_str, (_, _, _, was_missing)) in &known {
        if !seen.contains(path_str) && *was_missing == 0 {
            sqlx::query("UPDATE tracks SET missing = 1 WHERE path = ?")
                .bind(path_str)
                .execute(pool)
                .await
                .map_err(db::cvt)?;
            report.files_missing += 1;
        }
    }

    let merged = consolidate_albums(pool).await?;
    if merged > 0 {
        info!(merged, "merged duplicate album rows");
    }

    report.elapsed_secs = start.elapsed().as_secs_f64();
    sqlx::query(
        "UPDATE scan_log SET finished_at = datetime('now'), files_scanned = ?,
         files_added = ?, files_updated = ?, files_missing = ?, files_skipped = ?
         WHERE id = ?",
    )
    .bind(report.files_seen as i64)
    .bind(report.files_added as i64)
    .bind(report.files_updated as i64)
    .bind(report.files_missing as i64)
    .bind(report.files_skipped as i64)
    .bind(scan_id)
    .execute(pool)
    .await
    .map_err(db::cvt)?;

    info!(
        files_seen = report.files_seen,
        audio_files = report.audio_files,
        added = report.files_added,
        updated = report.files_updated,
        skipped = report.files_skipped,
        missing = report.files_missing,
        elapsed_secs = report.elapsed_secs,
        "scan complete"
    );
    Ok(report)
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| AudioFormat::from_extension(ext) != AudioFormat::Unknown)
        .unwrap_or(false)
}

/// Hash + read one file. Runs on a blocking worker: no `.await` here.
fn analyze_file(path: &Path) -> Result<FileAnalysis, MusicError> {
    let meta = std::fs::metadata(path).map_err(MusicError::Io)?;
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .map_err(MusicError::Io)?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| MusicError::Io(std::io::Error::other(e)))
        .map(|d| d.as_secs() as i64)
        .ok();
    let format = path
        .extension()
        .and_then(|s| s.to_str())
        .map(AudioFormat::from_extension)
        .unwrap_or(AudioFormat::Unknown);

    // Stream through BLAKE3 in 64 KiB chunks: O(1) memory regardless of size.
    let mut f = std::fs::File::open(path).map_err(MusicError::Io)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).map_err(MusicError::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let hash = hasher.finalize().to_hex().to_string();

    let mut a = FileAnalysis {
        path: path.to_string_lossy().to_string(),
        hash,
        size,
        mtime,
        format,
        title: None,
        artist: None,
        album: None,
        album_artist: None,
        genre: None,
        year: None,
        track_no: None,
        disc_no: None,
        duration_ms: None,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        bitrate: None,
        artwork: None,
        decodable: format.is_directly_streamable(),
        mqa: false,
        original_sample_rate: None,
    };

    // Best effort: undecodable-yet sources (DSD/ISO) may not parse; they are
    // still cataloged with whatever came through, technical fields NULL.
    match lofty::read_from_path(path) {
        Ok(tagged) => {
            let props = tagged.properties();
            let dur = props.duration();
            a.duration_ms = (dur.as_millis() > 0).then_some(dur.as_millis() as u64);
            a.sample_rate = props.sample_rate();
            a.bit_depth = props.bit_depth();
            a.channels = props.channels();
            a.bitrate = props.audio_bitrate();
            if let Some(tag) = tagged.primary_tag() {
                a.title = tag.title().map(|c| c.into_owned());
                a.artist = tag.artist().map(|c| c.into_owned());
                a.album = tag.album().map(|c| c.into_owned());
                a.album_artist = tag.get_string(&ItemKey::AlbumArtist).map(str::to_string);
                a.genre = tag.genre().map(|c| c.into_owned());
                a.year = tag.year().and_then(|y| u16::try_from(y).ok());
                a.track_no = tag.track();
                a.disc_no = tag.disk();
                (a.mqa, a.original_sample_rate) = mqa_of(tag);
                if let Some(pic) = tag.pictures().iter().find(|p| !p.data().is_empty()) {
                    let mime = pic
                        .mime_type()
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_else(|| sniff_mime(pic.data()).to_string());
                    a.artwork = Some(ArtworkData {
                        hash: blake3::hash(pic.data()).to_hex().to_string(),
                        bytes: pic.data().to_vec(),
                        mime,
                    });
                }
            }
        }
        // lofty cannot open DSD containers; `apply_dsd` below handles them.
        Err(_) if matches!(a.format, AudioFormat::Dsf | AudioFormat::Dff) => {}
        Err(e) => {
            warn!(path = %a.path, error = %e, "lofty read failed; cataloging without metadata");
        }
    }

    if matches!(a.format, AudioFormat::Dsf | AudioFormat::Dff) {
        apply_dsd(path, &mut a);
    }

    // A track with no title tag still needs a display name: the file stem.
    if a.title.is_none() {
        a.title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string);
    }
    Ok(a)
}

/// MQA markers in a file's tags. MQA-encoded FLACs carry an `MQAENCODER`
/// Vorbis comment (the encoder build) and usually `ORIGINALSAMPLERATE` (the
/// master's rate before folding). Keys are matched case-insensitively.
///
/// This is tag-based: a file whose tags were stripped is not recognised (the
/// MQA signal itself lives in the audio LSBs and is not inspected).
fn mqa_of(tag: &lofty::tag::Tag) -> (bool, Option<u32>) {
    let mut mqa = false;
    let mut original = None;
    for item in tag.items() {
        let lofty::tag::ItemKey::Unknown(key) = item.key() else {
            continue;
        };
        let lofty::tag::ItemValue::Text(value) = item.value() else {
            continue;
        };
        if key.eq_ignore_ascii_case("MQAENCODER") && !value.trim().is_empty() {
            mqa = true;
        } else if key.eq_ignore_ascii_case("ORIGINALSAMPLERATE") {
            original = value.trim().parse::<u32>().ok().filter(|&r| r > 0);
        }
    }
    (mqa, original.filter(|_| mqa))
}

/// Tags only (no hashing): used to backfill MQA info for rows cataloged
/// before the column existed.
fn read_mqa(path: &Path) -> (bool, Option<u32>) {
    match lofty::read_from_path(path) {
        Ok(tagged) => tagged.primary_tag().map(mqa_of).unwrap_or((false, None)),
        Err(_) => (false, None),
    }
}

/// Technical properties and tags of a DSF/DFF file. `decodable` becomes true
/// only when the container parses, so a corrupt "DSF" stays unplayable.
fn apply_dsd(path: &Path, a: &mut FileAnalysis) {
    let info = match std::fs::File::open(path)
        .map(std::io::BufReader::new)
        .map_err(MusicError::Io)
        .and_then(|mut r| crate::dsd::parse_dsd(&mut r))
    {
        Ok(info) => info,
        Err(e) => {
            warn!(path = %a.path, error = %e, "DSD parse failed; cataloging without metadata");
            return;
        }
    };
    // `sample_rate` is the DSD rate (2,822,400 for DSD64): the player uses it
    // to pick the DoP rate; 1-bit samples.
    a.sample_rate = Some(info.dsd_rate);
    a.bit_depth = Some(1);
    a.channels = u8::try_from(info.channels).ok();
    let ms = info.samples_per_channel.saturating_mul(1000) / u64::from(info.dsd_rate.max(1));
    a.duration_ms = (ms > 0).then_some(ms);
    a.bitrate = u32::try_from(u64::from(info.dsd_rate) * info.channels as u64 / 1000).ok();
    a.decodable = true;

    if let Some(t) = crate::dsd_meta::read_tags(path, &info) {
        a.title = t.title.or(a.title.take());
        a.artist = t.artist;
        a.album = t.album;
        a.album_artist = t.album_artist;
        a.genre = t.genre;
        a.year = t.year;
        a.track_no = t.track_no;
        a.disc_no = t.disc_no;
        if let Some(p) = t.picture {
            let mime = if p.mime.starts_with("image/") {
                p.mime
            } else {
                sniff_mime(&p.data).to_string()
            };
            a.artwork = Some(ArtworkData {
                hash: blake3::hash(&p.data).to_hex().to_string(),
                bytes: p.data,
                mime,
            });
        }
    }
}

/// Guess an image MIME type from magic bytes, for pictures lofty couldn't type.
fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.starts_with(b"BM") {
        "image/bmp"
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

// ---------------------------------------------------------------------------
// Catalog writes
// ---------------------------------------------------------------------------

async fn ensure_artist(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
) -> Result<i64, MusicError> {
    sqlx::query("INSERT OR IGNORE INTO artists (name) VALUES (?)")
        .bind(name)
        .execute(&mut **tx)
        .await
        .map_err(db::cvt)?;
    let id: i64 = sqlx::query("SELECT id FROM artists WHERE name = ?")
        .bind(name)
        .fetch_one(&mut **tx)
        .await
        .map_err(db::cvt)?
        .get(0);
    Ok(id)
}

/// Find or create the album row for a track.
///
/// Files are matched to an existing album with the same title, in order:
///
/// 1. **Album-artist tag** equal to the album's artist (tagged rips).
///
/// For files WITHOUT an album-artist tag — most rips only tag the track
/// artist — the tag can't be the key, so:
///
/// 2. **Same folder.** A track sitting next to another track of an album with
///    the same title belongs to it. If the track artists differ this is an
///    untagged compilation and the display artist is promoted to
///    "Various Artists".
/// 3. **Same artist.** Same title and the same track artist elsewhere (e.g.
///    `CD1/`, `CD2/` folders) is the same album.
/// 4. A "Various Artists" row already promoted for this title.
///
/// Otherwise a new album is created. (Before this ordering, untagged files
/// only ever matched an empty album-artist, so every track got its own album.)
async fn resolve_album(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    title: &str,
    album_artist: Option<&str>,
    track_artist: Option<&str>,
    year: Option<u16>,
    artwork_hash: Option<&str>,
    dir: Option<&str>,
) -> Result<i64, MusicError> {
    let display = album_artist.or(track_artist);
    let rows = sqlx::query("SELECT id, artist, year, artwork_hash FROM albums WHERE title = ?")
        .bind(title)
        .fetch_all(&mut **tx)
        .await
        .map_err(db::cvt)?;

    struct AlbumChoice {
        id: i64,
        artist: Option<String>,
        year: Option<i64>,
        artwork_hash: Option<String>,
    }
    let choice_of = |r: &sqlx::sqlite::SqliteRow| AlbumChoice {
        id: r.get("id"),
        artist: r.get("artist"),
        year: r.get("year"),
        artwork_hash: r.get("artwork_hash"),
    };

    let mut chosen: Option<AlbumChoice> = None;
    if let Some(aa) = album_artist {
        // 1. Album-artist tag.
        chosen = rows
            .iter()
            .find(|r| r.get::<Option<String>, _>("artist").as_deref() == Some(aa))
            .map(choice_of);
    } else {
        // 2. Same folder.
        if let Some(dir) = dir {
            let siblings = sqlx::query(
                "SELECT DISTINCT a.id AS id, t.path AS path FROM albums a
                 JOIN tracks t ON t.album_id = a.id WHERE a.title = ?",
            )
            .bind(title)
            .fetch_all(&mut **tx)
            .await
            .map_err(db::cvt)?;
            let same_dir = siblings
                .iter()
                .find(|r| parent_dir(&r.get::<String, _>("path")) == Some(dir))
                .map(|r| r.get::<i64, _>("id"));
            if let Some(id) = same_dir {
                chosen = rows
                    .iter()
                    .find(|r| r.get::<i64, _>("id") == id)
                    .map(choice_of);
            }
        }
        // 3. Same track artist.
        if chosen.is_none() {
            if let Some(artist) = track_artist {
                chosen = rows
                    .iter()
                    .find(|r| r.get::<Option<String>, _>("artist").as_deref() == Some(artist))
                    .map(choice_of);
            }
        }
        // 4. An already-promoted "Various Artists" row.
        if chosen.is_none() {
            chosen = rows
                .iter()
                .find(|r| {
                    r.get::<Option<String>, _>("artist").as_deref() == Some("Various Artists")
                })
                .map(choice_of);
        }
    }

    if let Some(choice) = chosen {
        let mut new_artist: Option<String> = None;
        match (choice.artist.as_deref(), display) {
            (Some(cur), Some(d)) if cur != d && cur != "Various Artists" => {
                new_artist = Some("Various Artists".to_string());
            }
            _ => {}
        }
        let new_year = (choice.year.is_none() && year.is_some()).then(|| year.unwrap() as i64);
        let new_art = (choice.artwork_hash.is_none() && artwork_hash.is_some())
            .then(|| artwork_hash.unwrap());
        if new_artist.is_some() || new_year.is_some() || new_art.is_some() {
            sqlx::query(
                "UPDATE albums SET artist = COALESCE(?, artist), year = COALESCE(?, year),
                 artwork_hash = COALESCE(?, artwork_hash) WHERE id = ?",
            )
            .bind(new_artist)
            .bind(new_year)
            .bind(new_art)
            .bind(choice.id)
            .execute(&mut **tx)
            .await
            .map_err(db::cvt)?;
        }
        return Ok(choice.id);
    }

    let id: i64 = sqlx::query(
        "INSERT INTO albums (title, artist, year, artwork_hash) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(title)
    .bind(display)
    .bind(year.map(|y| y as i64))
    .bind(artwork_hash)
    .fetch_one(&mut **tx)
    .await
    .map_err(db::cvt)?
    .get("id");
    Ok(id)
}

/// Parent directory of a catalog path (as stored: a plain string).
fn parent_dir(path: &str) -> Option<&str> {
    Path::new(path).parent().and_then(|p| p.to_str())
}

/// Merge album rows that are really one album. Runs at the end of every scan,
/// which repairs catalogs split by the old untagged-file grouping without
/// re-reading a single file.
///
/// Two rows with the same title are the same album when they share the
/// album artist, or when they have tracks in the same folder (an untagged
/// compilation whose tracks were each given their own row). Merged rows keep
/// the lowest id; tracks and artist links move over, missing year/artwork are
/// filled in, and a folder with several artists becomes "Various Artists".
/// Returns how many duplicate rows were removed.
pub async fn consolidate_albums(pool: &SqlitePool) -> Result<u64, MusicError> {
    let rows = sqlx::query(
        "SELECT DISTINCT a.id AS id, a.title AS title, a.artist AS artist, t.path AS path
         FROM albums a JOIN tracks t ON t.album_id = a.id",
    )
    .fetch_all(pool)
    .await
    .map_err(db::cvt)?;

    // Union-find over album ids.
    let mut parent: HashMap<i64, i64> = HashMap::new();
    fn find(p: &mut HashMap<i64, i64>, x: i64) -> i64 {
        let px = *p.entry(x).or_insert(x);
        if px == x {
            return x;
        }
        let r = find(p, px);
        p.insert(x, r);
        r
    }
    let union = |p: &mut HashMap<i64, i64>, a: i64, b: i64| {
        let (ra, rb) = (find(p, a), find(p, b));
        if ra != rb {
            // Lowest id survives.
            p.insert(ra.max(rb), ra.min(rb));
        }
    };
    let mut by_artist: HashMap<(String, String), i64> = HashMap::new();
    let mut by_dir: HashMap<(String, String), i64> = HashMap::new();
    for r in &rows {
        let id: i64 = r.get("id");
        let title: String = r.get("title");
        let artist: Option<String> = r.get("artist");
        let path: String = r.get("path");
        find(&mut parent, id);
        if let Some(a) = artist {
            match by_artist.get(&(title.clone(), a.clone())) {
                Some(&other) => union(&mut parent, id, other),
                None => {
                    by_artist.insert((title.clone(), a), id);
                }
            }
        }
        if let Some(d) = parent_dir(&path) {
            match by_dir.get(&(title.clone(), d.to_string())) {
                Some(&other) => union(&mut parent, id, other),
                None => {
                    by_dir.insert((title, d.to_string()), id);
                }
            }
        }
    }

    let ids: Vec<i64> = parent.keys().copied().collect();
    let mut groups: HashMap<i64, Vec<i64>> = HashMap::new();
    for id in ids {
        let root = find(&mut parent, id);
        groups.entry(root).or_default().push(id);
    }

    let mut removed = 0u64;
    let mut tx = pool.begin().await.map_err(db::cvt)?;
    for (keep, members) in groups.into_iter().filter(|(_, m)| m.len() > 1) {
        for &dup in members.iter().filter(|&&m| m != keep) {
            sqlx::query("UPDATE tracks SET album_id = ? WHERE album_id = ?")
                .bind(keep)
                .bind(dup)
                .execute(&mut *tx)
                .await
                .map_err(db::cvt)?;
            sqlx::query(
                "INSERT OR IGNORE INTO album_artists (album_id, artist_id)
                 SELECT ?, artist_id FROM album_artists WHERE album_id = ?",
            )
            .bind(keep)
            .bind(dup)
            .execute(&mut *tx)
            .await
            .map_err(db::cvt)?;
            sqlx::query("DELETE FROM album_artists WHERE album_id = ?")
                .bind(dup)
                .execute(&mut *tx)
                .await
                .map_err(db::cvt)?;
            sqlx::query(
                "UPDATE albums SET
                   year = COALESCE(year, (SELECT year FROM albums WHERE id = ?)),
                   artwork_hash = COALESCE(artwork_hash, (SELECT artwork_hash FROM albums WHERE id = ?)),
                   artist = CASE
                     WHEN artist IS NULL THEN (SELECT artist FROM albums WHERE id = ?)
                     WHEN artist = COALESCE((SELECT artist FROM albums WHERE id = ?), artist) THEN artist
                     ELSE 'Various Artists' END
                 WHERE id = ?",
            )
            .bind(dup)
            .bind(dup)
            .bind(dup)
            .bind(dup)
            .bind(keep)
            .execute(&mut *tx)
            .await
            .map_err(db::cvt)?;
            sqlx::query("DELETE FROM albums WHERE id = ?")
                .bind(dup)
                .execute(&mut *tx)
                .await
                .map_err(db::cvt)?;
            removed += 1;
        }
    }
    tx.commit().await.map_err(db::cvt)?;
    Ok(removed)
}

async fn upsert_track(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    a: &FileAnalysis,
) -> Result<(), MusicError> {
    let decodable = i64::from(a.decodable);

    let artwork_hash: Option<String> = match &a.artwork {
        Some(art) => {
            sqlx::query("INSERT OR IGNORE INTO artwork (hash, mime, bytes) VALUES (?, ?, ?)")
                .bind(&art.hash)
                .bind(&art.mime)
                .bind(&art.bytes)
                .execute(&mut **tx)
                .await
                .map_err(db::cvt)?;
            Some(art.hash.clone())
        }
        None => None,
    };

    let album_id: Option<i64> = match &a.album {
        Some(title) => Some(
            resolve_album(
                tx,
                title,
                a.album_artist.as_deref(),
                a.artist.as_deref(),
                a.year,
                artwork_hash.as_deref(),
                parent_dir(&a.path),
            )
            .await?,
        ),
        None => None,
    };

    let track_id: i64 = match sqlx::query("SELECT id FROM tracks WHERE path = ?")
        .bind(&a.path)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db::cvt)?
    {
        Some(r) => {
            let id: i64 = r.get("id");
            sqlx::query(
                "UPDATE tracks SET hash = ?, format = ?, sample_rate = ?, bit_depth = ?,
                     channels = ?, duration_ms = ?, bitrate = ?, title = ?, album = ?,
                     artist = ?, album_id = ?, track_no = ?, disc_no = ?, genre = ?,
                     year = ?, artwork_hash = ?, file_size = ?, file_mtime = ?,
                     missing = 0, decodable = ?, mqa = ?, original_sample_rate = ?,
                     mqa_checked = 1 WHERE id = ?",
            )
            .bind(&a.hash)
            .bind(a.format.wire_name())
            .bind(a.sample_rate.map(i64::from))
            .bind(a.bit_depth.map(i64::from))
            .bind(a.channels.map(i64::from))
            .bind(a.duration_ms.map(|v| v as i64))
            .bind(a.bitrate.map(i64::from))
            .bind(&a.title)
            .bind(&a.album)
            .bind(&a.artist)
            .bind(album_id)
            .bind(a.track_no.map(i64::from))
            .bind(a.disc_no.map(i64::from))
            .bind(&a.genre)
            .bind(a.year.map(|y| y as i64))
            .bind(&artwork_hash)
            .bind(a.size)
            .bind(a.mtime)
            .bind(decodable)
            .bind(i64::from(a.mqa))
            .bind(a.original_sample_rate.map(i64::from))
            .bind(id)
            .execute(&mut **tx)
            .await
            .map_err(db::cvt)?;
            sqlx::query("DELETE FROM track_artists WHERE track_id = ?")
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(db::cvt)?;
            id
        }
        None => sqlx::query(
            "INSERT INTO tracks (path, hash, format, sample_rate, bit_depth, channels,
                     duration_ms, bitrate, title, album, artist, album_id, track_no, disc_no,
                     genre, year, artwork_hash, file_size, file_mtime, missing, decodable,
                     mqa, original_sample_rate, mqa_checked)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, 1)
                     RETURNING id",
        )
        .bind(&a.path)
        .bind(&a.hash)
        .bind(a.format.wire_name())
        .bind(a.sample_rate.map(i64::from))
        .bind(a.bit_depth.map(i64::from))
        .bind(a.channels.map(i64::from))
        .bind(a.duration_ms.map(|v| v as i64))
        .bind(a.bitrate.map(i64::from))
        .bind(&a.title)
        .bind(&a.album)
        .bind(&a.artist)
        .bind(album_id)
        .bind(a.track_no.map(i64::from))
        .bind(a.disc_no.map(i64::from))
        .bind(&a.genre)
        .bind(a.year.map(|y| y as i64))
        .bind(&artwork_hash)
        .bind(a.size)
        .bind(a.mtime)
        .bind(decodable)
        .bind(i64::from(a.mqa))
        .bind(a.original_sample_rate.map(i64::from))
        .fetch_one(&mut **tx)
        .await
        .map_err(db::cvt)?
        .get("id"),
    };

    // Multi-artist: every split name gets a track_artists row; all distinct
    // track artists (plus the album-artist tag) link the album so artist
    // detail pages surface compilation appearances.
    if let Some(artist) = &a.artist {
        for name in split_artists(artist) {
            let aid = ensure_artist(tx, &name).await?;
            sqlx::query("INSERT INTO track_artists (track_id, artist_id) VALUES (?, ?)")
                .bind(track_id)
                .bind(aid)
                .execute(&mut **tx)
                .await
                .map_err(db::cvt)?;
            if let Some(album_id) = album_id {
                sqlx::query(
                    "INSERT OR IGNORE INTO album_artists (album_id, artist_id) VALUES (?, ?)",
                )
                .bind(album_id)
                .bind(aid)
                .execute(&mut **tx)
                .await
                .map_err(db::cvt)?;
            }
        }
    }
    if let Some(aa) = &a.album_artist {
        for name in split_artists(aa) {
            let aid = ensure_artist(tx, &name).await?;
            if let Some(album_id) = album_id {
                sqlx::query(
                    "INSERT OR IGNORE INTO album_artists (album_id, artist_id) VALUES (?, ?)",
                )
                .bind(album_id)
                .bind(aid)
                .execute(&mut **tx)
                .await
                .map_err(db::cvt)?;
            }
        }
    }

    // FTS5 is maintained manually (plain table, no triggers): delete + insert.
    sqlx::query("DELETE FROM search_fts WHERE rowid = ?")
        .bind(track_id)
        .execute(&mut **tx)
        .await
        .map_err(db::cvt)?;
    sqlx::query("INSERT INTO search_fts (rowid, title, album, artist) VALUES (?, ?, ?, ?)")
        .bind(track_id)
        .bind(a.title.as_deref().unwrap_or(""))
        .bind(a.album.as_deref().unwrap_or(""))
        .bind(a.artist.as_deref().unwrap_or(""))
        .execute(&mut **tx)
        .await
        .map_err(db::cvt)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Artist-name splitting
// ---------------------------------------------------------------------------

/// Separators that introduce featured artists, matched case-insensitively.
/// Ordered longest-first so `"ft. "` wins over `"ft "`.
const FEAT_PREFIXES: &[&str] = &["featuring ", "feat. ", "feat ", "ft. ", "ft "];

fn strip_feat_prefix(s: &str) -> Option<&str> {
    let t = s.trim_start();
    FEAT_PREFIXES.iter().find_map(|p| {
        (t.len() >= p.len() && t[..p.len()].eq_ignore_ascii_case(p)).then(|| t[p.len()..].trim())
    })
}

/// Split an artist tag into individual artist names.
///
/// Rule (documented, v1):
/// 1. Featured artists are extracted first: case-insensitive `feat.` / `ft.`
///    / `featuring`, with or without surrounding parentheses —
///    `"A (feat. B)"` and `"A feat. B"` both yield main `"A"` + extra `"B"`.
/// 2. What remains splits on `;`, and on `/` only when the slash touches
///    whitespace (`"A / B"` splits; `"AC/DC"` does not).
/// 3. Trim, drop empties, dedupe case-insensitively, keep first-seen order.
///
/// Deliberately NOT split: `,`, `&`, "and" — band names like
/// "Crosby, Stills & Nash" or "Simon & Garfunkel" must survive intact.
pub fn split_artists(raw: &str) -> Vec<String> {
    // Pass 1: lift parenthesized "(feat. X)" segments out.
    let mut main = String::with_capacity(raw.len());
    let mut extras: Vec<String> = Vec::new();
    let mut rest = raw;
    while let Some(open) = rest.find('(') {
        main.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find(')') {
            Some(close) => {
                let inner = after[..close].trim();
                match strip_feat_prefix(inner) {
                    Some(feat) if !feat.is_empty() => {
                        extras.push(feat.to_string());
                        main.push(' ');
                    }
                    _ => main.push_str(&rest[open..open + 1 + close + 1]),
                }
                rest = &after[close + 1..];
            }
            None => {
                main.push_str(&rest[open..]);
                rest = "";
                break;
            }
        }
    }
    main.push_str(rest);

    // Pass 2: bare "feat. X" suffixes (no parens).
    let mut chunks: Vec<String> = Vec::new();
    for seed in std::iter::once(main).chain(extras) {
        chunks.extend(split_bare_feat(&seed));
    }

    // Pass 3: ';' and whitespace-adjacent '/'; trim; dedupe.
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for chunk in &chunks {
        for piece in split_multi(chunk) {
            let name = piece.trim();
            if name.is_empty() {
                continue;
            }
            if seen.insert(name.to_ascii_lowercase()) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Split `s` on bare feat-word separators (`"A feat. B"` → `["A", "B"]`).
/// `to_ascii_lowercase` preserves byte length, so byte indices stay valid.
fn split_bare_feat(s: &str) -> Vec<String> {
    let lower = s.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < lower.len() {
        let hit = FEAT_PREFIXES.iter().find_map(|p| {
            if (i == 0 || bytes[i - 1] == b' ') && lower[i..].starts_with(p) {
                Some(p.len())
            } else {
                None
            }
        });
        match hit {
            Some(len) => {
                parts.push(s[start..i].to_string());
                i += len;
                start = i;
            }
            None => i += 1,
        }
    }
    parts.push(s[start..].to_string());
    parts
}

/// Split on `;`, and on `/` only when it touches whitespace on either side.
fn split_multi(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (idx, c) = chars[i];
        let is_sep = c == ';'
            || (c == '/'
                && ((i > 0 && chars[i - 1].1.is_whitespace())
                    || (i + 1 < chars.len() && chars[i + 1].1.is_whitespace())));
        if is_sep {
            parts.push(s[start..idx].trim());
            start = idx + c.len_utf8();
        }
        i += 1;
    }
    parts.push(s[start..].trim());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_artists_basic_separators() {
        assert_eq!(split_artists("Miles Davis"), vec!["Miles Davis"]);
        assert_eq!(
            split_artists("John Coltrane; Miles Davis"),
            vec!["John Coltrane", "Miles Davis"]
        );
        assert_eq!(split_artists("A / B"), vec!["A", "B"]);
    }

    #[test]
    fn split_artists_preserves_band_names() {
        // Commas, ampersands, and tight slashes are NOT separators.
        assert_eq!(
            split_artists("Crosby, Stills & Nash"),
            vec!["Crosby, Stills & Nash"]
        );
        assert_eq!(split_artists("AC/DC"), vec!["AC/DC"]);
        assert_eq!(
            split_artists("Simon & Garfunkel"),
            vec!["Simon & Garfunkel"]
        );
    }

    #[test]
    fn split_artists_extracts_featured() {
        assert_eq!(
            split_artists("Thelonious Monk feat. Dizzy Gillespie"),
            vec!["Thelonious Monk", "Dizzy Gillespie"]
        );
        assert_eq!(split_artists("A (feat. B)"), vec!["A", "B"]);
        assert_eq!(split_artists("A (Featuring B & C)"), vec!["A", "B & C"]);
        assert_eq!(split_artists("A ft. B"), vec!["A", "B"]);
        // Non-feat parens are left alone.
        assert_eq!(split_artists("A (Remaster)"), vec!["A (Remaster)"]);
    }

    #[test]
    fn split_artists_trims_and_dedupes() {
        assert_eq!(split_artists("A; a ; B"), vec!["A", "B"]);
        assert_eq!(split_artists("  "), Vec::<String>::new());
    }
}

// ---------------------------------------------------------------------------
// Test fixtures: a small tagged library generated with ffmpeg (hermetic:
// temp dirs only, no network; requires the ffmpeg binary).
// ---------------------------------------------------------------------------

#[cfg(test)]
pub mod fixtures {
    use super::*;
    use std::process::Command;

    pub struct TrackSpec<'a> {
        pub dir: &'a str,
        pub file: &'a str,
        /// ffmpeg audio codec: "flac" | "libmp3lame" | "libvorbis" | "aac" | "pcm_s16le"
        pub codec: &'a str,
        pub title: &'a str,
        pub artist: &'a str,
        pub album: &'a str,
        pub album_artist: Option<&'a str>,
        pub track_no: u32,
        pub year: Option<&'a str>,
        pub genre: Option<&'a str>,
        pub art: bool,
    }

    fn ffmpeg(args: &[&str]) {
        let st = Command::new("ffmpeg")
            .args(args)
            .status()
            .expect("ffmpeg binary is required for scanner fixtures");
        assert!(st.success(), "ffmpeg failed: {args:?}");
    }

    pub fn cover_png(dir: &Path) -> PathBuf {
        let p = dir.join("cover.png");
        ffmpeg(&[
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:size=16x16:duration=1",
            "-frames:v",
            "1",
            p.to_str().unwrap(),
        ]);
        p
    }

    pub fn make_track(lib: &Path, cover: Option<&Path>, spec: &TrackSpec) {
        let dir = lib.join(spec.dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join(spec.file);
        let mut tags: Vec<String> = vec![
            format!("title={}", spec.title),
            format!("artist={}", spec.artist),
            format!("album={}", spec.album),
            format!("track={}", spec.track_no),
        ];
        if let Some(aa) = spec.album_artist {
            tags.push(format!("album_artist={aa}"));
        }
        if let Some(y) = spec.year {
            tags.push(format!("date={y}"));
        }
        if let Some(g) = spec.genre {
            tags.push(format!("genre={g}"));
        }

        let out_s = out.to_str().unwrap().to_string();
        let mut args: Vec<&str> = vec![
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1:sample_rate=44100",
        ];
        let cover_s;
        if let Some(c) = cover {
            cover_s = c.to_str().unwrap().to_string();
            args.extend(["-i", &cover_s]);
        }
        if cover.is_some() {
            args.extend([
                "-map",
                "0:a",
                "-map",
                "1:v",
                "-c:v",
                "copy",
                "-disposition:v",
                "attached_pic",
            ]);
        }
        let codec = spec.codec.to_string();
        // Stereo so the scanner records channels=2. Placed here (after all
        // inputs) so ffmpeg treats it as an output option.
        args.extend(["-ac", "2"]);
        args.extend(["-c:a", &codec]);
        if spec.codec == "libmp3lame" {
            args.extend(["-id3v2_version", "3"]);
        }
        let meta_args: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
        let mut owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        // lofty does not read the RIFF INFO tags ffmpeg writes on WAV, so the
        // WAV fixture skips ffmpeg metadata and is tagged with lofty itself.
        let tag_wav_with_lofty = spec.codec == "pcm_s16le";
        if !tag_wav_with_lofty {
            for t in &meta_args {
                owned.push("-metadata".to_string());
                owned.push(t.clone());
            }
        }
        owned.push(out_s);
        let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
        ffmpeg(&refs);
        assert!(out.exists(), "fixture not created: {}", out.display());
        if tag_wav_with_lofty {
            tag_wav_fixture(&out, spec);
        }
    }

    /// lofty reads ID3v2 on WAV but not the RIFF INFO tags ffmpeg writes, so
    /// the WAV fixture is tagged with lofty itself after ffmpeg encodes it.
    fn tag_wav_fixture(out: &Path, spec: &TrackSpec) {
        use lofty::tag::{Tag, TagType};
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title(spec.title.to_owned());
        tag.set_artist(spec.artist.to_owned());
        tag.set_album(spec.album.to_owned());
        if let Some(aa) = spec.album_artist {
            assert!(
                tag.insert_text(ItemKey::AlbumArtist, aa.to_owned()),
                "insert album artist"
            );
        }
        tag.set_track(spec.track_no);
        if let Some(y) = spec.year {
            tag.set_year(y.parse::<u32>().expect("year parses"));
        }
        if let Some(g) = spec.genre {
            tag.set_genre(g.to_owned());
        }
        tag.save_to_path(out, lofty::config::WriteOptions::default())
            .expect("lofty wav tag write");
    }

    /// The standard fixture library: 3 albums (one compilation), 5 codecs,
    /// multi-artist tags, one embedded cover.
    ///
    /// - "Blue Train" / John Coltrane: 2 FLAC + 1 WAV (one with cover art)
    /// - "Kind of Blue" / Miles Davis: 2 MP3
    /// - "Jazz Compilation" / Various Artists: 2 OGG + 1 M4A, mixed artists
    pub fn build_library(root: &Path) -> PathBuf {
        let lib = root.join("lib");
        let cover = cover_png(root);
        let tracks = [
            TrackSpec {
                dir: "Blue Train",
                file: "01.flac",
                codec: "flac",
                title: "Blue Train",
                artist: "John Coltrane",
                album: "Blue Train",
                album_artist: Some("John Coltrane"),
                track_no: 1,
                year: Some("1957"),
                genre: Some("Jazz"),
                art: true,
            },
            TrackSpec {
                dir: "Blue Train",
                file: "02.flac",
                codec: "flac",
                title: "Moment's Notice",
                artist: "John Coltrane",
                album: "Blue Train",
                album_artist: Some("John Coltrane"),
                track_no: 2,
                year: Some("1957"),
                genre: Some("Jazz"),
                art: false,
            },
            TrackSpec {
                dir: "Blue Train",
                file: "03.wav",
                codec: "pcm_s16le",
                title: "Locomotion",
                artist: "John Coltrane",
                album: "Blue Train",
                album_artist: Some("John Coltrane"),
                track_no: 3,
                year: Some("1957"),
                genre: Some("Jazz"),
                art: false,
            },
            TrackSpec {
                dir: "Kind of Blue",
                file: "01.mp3",
                codec: "libmp3lame",
                title: "So What",
                artist: "Miles Davis",
                album: "Kind of Blue",
                album_artist: Some("Miles Davis"),
                track_no: 1,
                year: Some("1959"),
                genre: Some("Jazz"),
                art: false,
            },
            TrackSpec {
                dir: "Kind of Blue",
                file: "02.mp3",
                codec: "libmp3lame",
                title: "Freddie Freeloader",
                artist: "Miles Davis",
                album: "Kind of Blue",
                album_artist: Some("Miles Davis"),
                track_no: 2,
                year: Some("1959"),
                genre: Some("Jazz"),
                art: false,
            },
            TrackSpec {
                dir: "Jazz Compilation",
                file: "01.ogg",
                codec: "libvorbis",
                title: "Take Five",
                artist: "Dave Brubeck",
                album: "Jazz Compilation",
                album_artist: Some("Various Artists"),
                track_no: 1,
                year: Some("1998"),
                genre: Some("Jazz"),
                art: false,
            },
            TrackSpec {
                dir: "Jazz Compilation",
                file: "02.ogg",
                codec: "libvorbis",
                title: "My Favorite Things",
                artist: "John Coltrane; Miles Davis",
                album: "Jazz Compilation",
                album_artist: Some("Various Artists"),
                track_no: 2,
                year: Some("1998"),
                genre: Some("Jazz"),
                art: false,
            },
            TrackSpec {
                dir: "Jazz Compilation",
                file: "03.m4a",
                codec: "aac",
                title: "Round Midnight",
                artist: "Thelonious Monk feat. Dizzy Gillespie",
                album: "Jazz Compilation",
                album_artist: Some("Various Artists"),
                track_no: 3,
                year: Some("1998"),
                genre: Some("Jazz"),
                art: false,
            },
        ];
        for t in &tracks {
            make_track(&lib, t.art.then_some(cover.as_path()), t);
        }
        lib
    }

    /// Count rows in a table. Test helper.
    pub async fn count(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap()
            .get(0)
    }
}

#[cfg(test)]
mod scan_tests {
    use super::fixtures::*;
    use super::*;

    async fn scanned_pool() -> (SqlitePool, tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let lib = build_library(dir.path());
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        (pool, dir, lib)
    }

    #[tokio::test]
    async fn scan_populates_catalog() {
        let (pool, _dir, _lib) = scanned_pool().await;
        assert_eq!(count(&pool, "tracks").await, 8);
        assert_eq!(count(&pool, "albums").await, 3);

        // Compilation grouped under one album.
        let comp: i64 = sqlx::query("SELECT COUNT(*) FROM tracks WHERE album = 'Jazz Compilation'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(comp, 3);
        let comp_albums: i64 =
            sqlx::query("SELECT COUNT(*) FROM albums WHERE title = 'Jazz Compilation'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(comp_albums, 1);

        // Technical fields came through lofty for a FLAC track.
        let r = sqlx::query(
            "SELECT title, artist, album, genre, year, track_no, duration_ms,
                    sample_rate, bit_depth, channels, decodable, missing, format, hash
             FROM tracks WHERE title = 'Blue Train'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(r.get::<String, _>("artist"), "John Coltrane");
        assert_eq!(r.get::<String, _>("genre"), "Jazz");
        assert_eq!(r.get::<i64, _>("year"), 1957);
        assert_eq!(r.get::<i64, _>("track_no"), 1);
        assert_eq!(r.get::<i64, _>("duration_ms"), 1000);
        assert_eq!(r.get::<i64, _>("sample_rate"), 44100);
        assert_eq!(r.get::<i64, _>("channels"), 2);
        assert_eq!(r.get::<i64, _>("decodable"), 1);
        assert_eq!(r.get::<i64, _>("missing"), 0);
        assert_eq!(r.get::<String, _>("format"), "flac");
        let hash: String = r.get("hash");
        assert_eq!(hash.len(), 64, "BLAKE3 hex digest");

        // Multi-artist ";" split recorded in track_artists.
        let duo: i64 = sqlx::query(
            "SELECT COUNT(*) FROM track_artists ta JOIN tracks t ON t.id = ta.track_id
             WHERE t.title = 'My Favorite Things'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
        assert_eq!(duo, 2);

        // feat. extraction created a separate artist row.
        let dizzy: i64 = sqlx::query("SELECT COUNT(*) FROM artists WHERE name = 'Dizzy Gillespie'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(dizzy, 1);

        // Embedded artwork deduplicated into the artwork table.
        assert_eq!(count(&pool, "artwork").await, 1);
        let art_album: Option<String> =
            sqlx::query("SELECT artwork_hash FROM albums WHERE title = 'Blue Train'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("artwork_hash");
        assert!(art_album.unwrap().len() == 64);

        // FTS5 populated.
        let fts: i64 = sqlx::query("SELECT COUNT(*) FROM search_fts")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(fts, 8);

        // scan_log recorded the run.
        let log = sqlx::query("SELECT files_added, files_skipped, finished_at FROM scan_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(log.get::<i64, _>("files_added"), 8);
        assert_eq!(log.get::<i64, _>("files_skipped"), 0);
        let finished: Option<String> = log.get("finished_at");
        assert!(finished.is_some());
    }

    #[tokio::test]
    async fn second_scan_is_a_noop() {
        let (pool, _dir, lib) = scanned_pool().await;
        let report = run_scan_with_progress(&pool, &[lib], |_, _| {})
            .await
            .unwrap();
        assert_eq!(report.files_added, 0);
        assert_eq!(report.files_updated, 0);
        assert_eq!(report.files_missing, 0);
        assert_eq!(report.files_skipped, 8);
        assert_eq!(count(&pool, "tracks").await, 8);
    }

    /// DSD/ISO sources are catalog-only: lofty cannot parse them, so they land
    /// with NULL technical fields and decodable=false, never failing the scan.
    #[tokio::test]
    async fn scan_catalogs_undecodable_dsd_without_technical_fields() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        // Byte stand-ins: real DSD/ISO payloads aren't needed — the point is
        // that lofty can't parse these, exercising the catalog-only path.
        for name in ["track01.dsf", "track02.dff", "album.iso"] {
            std::fs::write(lib.join(name), vec![0xABu8; 4096]).unwrap();
        }
        let pool = db::open(&dir.path().join("t.db")).await.unwrap();
        let r = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(r.files_added, 3);

        let rows = sqlx::query(
            "SELECT format, decodable, missing, title, sample_rate, bit_depth,
                    channels, duration_ms, bitrate
             FROM tracks ORDER BY path",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 3);
        let fmts: Vec<String> = rows.iter().map(|r| r.get("format")).collect();
        assert_eq!(fmts, vec!["sacd_iso", "dsf", "dff"]);
        for r in &rows {
            assert_eq!(r.get::<i64, _>("decodable"), 0);
            assert_eq!(r.get::<i64, _>("missing"), 0);
            assert!(r.get::<Option<i64>, _>("sample_rate").is_none());
            assert!(r.get::<Option<i64>, _>("bit_depth").is_none());
            assert!(r.get::<Option<i64>, _>("channels").is_none());
            assert!(r.get::<Option<i64>, _>("duration_ms").is_none());
            assert!(r.get::<Option<i64>, _>("bitrate").is_none());
        }
        // File-stem fallback titles, no album rows without album tags.
        let titles: Vec<String> = rows.iter().map(|r| r.get("title")).collect();
        assert_eq!(titles, vec!["album", "track01", "track02"]);
        assert_eq!(count(&pool, "albums").await, 0);
    }

    #[tokio::test]
    async fn scan_detects_added_removed_and_changed() {
        let (pool, dir, lib) = scanned_pool().await;

        // Added file.
        make_track(
            &lib,
            None,
            &TrackSpec {
                dir: "Kind of Blue",
                file: "03.mp3",
                codec: "libmp3lame",
                title: "Blue in Green",
                artist: "Miles Davis",
                album: "Kind of Blue",
                album_artist: Some("Miles Davis"),
                track_no: 3,
                year: Some("1959"),
                genre: Some("Jazz"),
                art: false,
            },
        );
        let r = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(r.files_added, 1);
        assert_eq!(count(&pool, "tracks").await, 9);

        // Removed file: marked missing, never deleted.
        std::fs::remove_file(lib.join("Kind of Blue").join("03.mp3")).unwrap();
        let r = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(r.files_missing, 1);
        assert_eq!(count(&pool, "tracks").await, 9);
        let missing: i64 = sqlx::query("SELECT missing FROM tracks WHERE title = 'Blue in Green'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("missing");
        assert_eq!(missing, 1);

        // Changed file: rewritten with a new title tag -> re-hashed, updated.
        std::fs::remove_file(lib.join("Kind of Blue").join("01.mp3")).unwrap();
        make_track(
            &lib,
            None,
            &TrackSpec {
                dir: "Kind of Blue",
                file: "01.mp3",
                codec: "libmp3lame",
                title: "So What (Remaster)",
                artist: "Miles Davis",
                album: "Kind of Blue",
                album_artist: Some("Miles Davis"),
                track_no: 1,
                year: Some("1959"),
                genre: Some("Jazz"),
                art: false,
            },
        );
        let r = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(r.files_updated, 1);
        let title: String =
            sqlx::query("SELECT title FROM tracks WHERE path LIKE '%Kind of Blue/01.mp3'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("title");
        assert_eq!(title, "So What (Remaster)");
        // FTS follows the update.
        let fts: i64 =
            sqlx::query("SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH '\"Remaster\"*'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(fts, 1);

        // Touched file (mtime changed, content identical): stat refresh, same hash.
        let wav = lib.join("Blue Train").join("03.wav");
        let before: String = sqlx::query("SELECT hash FROM tracks WHERE title = 'Locomotion'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("hash");
        let new_mtime = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&wav)
            .unwrap()
            .set_modified(new_mtime)
            .unwrap();
        let r = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(r.files_updated, 1);
        let after: String = sqlx::query("SELECT hash FROM tracks WHERE title = 'Locomotion'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("hash");
        assert_eq!(before, after, "identical content keeps its hash");

        // scan_log has one row per run.
        let runs: i64 = sqlx::query("SELECT COUNT(*) FROM scan_log")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(runs, 5);
        let _ = dir; // keep tempdir alive
    }
}

/// Album grouping for files WITHOUT an album-artist tag (very common: most
/// rips only tag the track artist). Regression: every such track used to get
/// its own album row.
#[cfg(test)]
mod album_grouping_tests {
    use super::fixtures::*;
    use super::*;

    fn spec<'a>(
        dir: &'a str,
        file: &'a str,
        title: &'a str,
        artist: &'a str,
        album: &'a str,
        album_artist: Option<&'a str>,
        track_no: u32,
    ) -> TrackSpec<'a> {
        TrackSpec {
            dir,
            file,
            codec: "flac",
            title,
            artist,
            album,
            album_artist,
            track_no,
            year: Some("2012"),
            genre: None,
            art: false,
        }
    }

    async fn scan(specs: &[TrackSpec<'_>]) -> (SqlitePool, tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        for s in specs {
            make_track(&lib, None, s);
        }
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        (pool, dir, lib)
    }

    /// (title, artist, track count) of every album, ordered.
    async fn albums(pool: &SqlitePool) -> Vec<(String, Option<String>, i64)> {
        sqlx::query(
            "SELECT a.title, a.artist, COUNT(t.id) AS n FROM albums a
             LEFT JOIN tracks t ON t.album_id = a.id
             GROUP BY a.id ORDER BY a.title, a.artist, a.id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get("title"), r.get("artist"), r.get("n")))
        .collect()
    }

    #[tokio::test]
    async fn untagged_album_with_one_artist_is_one_album() {
        // "Come Away With Me": 4 tracks, track-artist tag only.
        let (pool, _d, _l) = scan(&[
            spec(
                "Norah",
                "01.flac",
                "Don't Know Why",
                "Norah Jones",
                "Come Away With Me",
                None,
                1,
            ),
            spec(
                "Norah",
                "02.flac",
                "Seven Years",
                "Norah Jones",
                "Come Away With Me",
                None,
                2,
            ),
            spec(
                "Norah",
                "03.flac",
                "Cold Cold Heart",
                "Norah Jones",
                "Come Away With Me",
                None,
                3,
            ),
            spec(
                "Norah",
                "04.flac",
                "Feelin' The Same Way",
                "Norah Jones",
                "Come Away With Me",
                None,
                4,
            ),
        ])
        .await;
        assert_eq!(
            albums(&pool).await,
            vec![("Come Away With Me".into(), Some("Norah Jones".into()), 4)]
        );
    }

    #[tokio::test]
    async fn untagged_compilation_in_one_folder_is_one_various_artists_album() {
        let (pool, _d, _l) = scan(&[
            spec(
                "Comp",
                "01.flac",
                "A",
                "Artist One",
                "Best of 2012",
                None,
                1,
            ),
            spec(
                "Comp",
                "02.flac",
                "B",
                "Artist Two",
                "Best of 2012",
                None,
                2,
            ),
            spec(
                "Comp",
                "03.flac",
                "C",
                "Artist Three",
                "Best of 2012",
                None,
                3,
            ),
        ])
        .await;
        assert_eq!(
            albums(&pool).await,
            vec![("Best of 2012".into(), Some("Various Artists".into()), 3)]
        );
    }

    #[tokio::test]
    async fn same_title_by_different_artists_in_different_folders_stays_separate() {
        let (pool, _d, _l) = scan(&[
            spec("X", "01.flac", "One", "Artist X", "Greatest Hits", None, 1),
            spec("X", "02.flac", "Two", "Artist X", "Greatest Hits", None, 2),
            spec("Y", "01.flac", "One", "Artist Y", "Greatest Hits", None, 1),
        ])
        .await;
        assert_eq!(
            albums(&pool).await,
            vec![
                ("Greatest Hits".into(), Some("Artist X".into()), 2),
                ("Greatest Hits".into(), Some("Artist Y".into()), 1),
            ]
        );
    }

    #[tokio::test]
    async fn same_artist_and_title_across_folders_is_one_album() {
        // e.g. a two-disc rip in CD1/ and CD2/.
        let (pool, _d, _l) = scan(&[
            spec("Album/CD1", "01.flac", "One", "Band", "Double", None, 1),
            spec("Album/CD2", "01.flac", "Two", "Band", "Double", None, 1),
        ])
        .await;
        assert_eq!(
            albums(&pool).await,
            vec![("Double".into(), Some("Band".into()), 2)]
        );
    }

    #[tokio::test]
    async fn tagged_and_untagged_files_of_one_album_group_together() {
        let (pool, _d, _l) = scan(&[
            spec(
                "Mixed",
                "01.flac",
                "One",
                "Band",
                "Mixed Tags",
                Some("Band"),
                1,
            ),
            spec("Mixed", "02.flac", "Two", "Band", "Mixed Tags", None, 2),
            spec(
                "Mixed",
                "03.flac",
                "Three",
                "Band",
                "Mixed Tags",
                Some("Band"),
                3,
            ),
        ])
        .await;
        assert_eq!(
            albums(&pool).await,
            vec![("Mixed Tags".into(), Some("Band".into()), 3)]
        );
    }

    #[tokio::test]
    async fn tagged_albums_still_group_by_album_artist() {
        let (pool, _d, _l) = scan(&[
            spec(
                "V",
                "01.flac",
                "A",
                "Solo A",
                "Sampler",
                Some("Various Artists"),
                1,
            ),
            spec(
                "V",
                "02.flac",
                "B",
                "Solo B",
                "Sampler",
                Some("Various Artists"),
                2,
            ),
        ])
        .await;
        assert_eq!(
            albums(&pool).await,
            vec![("Sampler".into(), Some("Various Artists".into()), 2)]
        );
    }

    #[tokio::test]
    async fn rescanning_does_not_split_or_duplicate_albums() {
        let (pool, _d, lib) = scan(&[
            spec(
                "N",
                "01.flac",
                "One",
                "Norah Jones",
                "Come Away With Me",
                None,
                1,
            ),
            spec(
                "N",
                "02.flac",
                "Two",
                "Norah Jones",
                "Come Away With Me",
                None,
                2,
            ),
        ])
        .await;
        for _ in 0..2 {
            run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
                .await
                .unwrap();
        }
        assert_eq!(
            albums(&pool).await,
            vec![("Come Away With Me".into(), Some("Norah Jones".into()), 2)]
        );
        assert_eq!(count(&pool, "tracks").await, 2);
    }

    /// Recreate what the old scanner produced: one album row per track.
    async fn split_into_one_album_per_track(pool: &SqlitePool, title: &str) {
        let ids: Vec<i64> = sqlx::query("SELECT id FROM tracks WHERE album = ? ORDER BY id")
            .bind(title)
            .fetch_all(pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        for track_id in ids.into_iter().skip(1) {
            let old: i64 = sqlx::query("SELECT album_id FROM tracks WHERE id = ?")
                .bind(track_id)
                .fetch_one(pool)
                .await
                .unwrap()
                .get(0);
            let new_id: i64 = sqlx::query(
                "INSERT INTO albums (title, artist, year, artwork_hash)
                 SELECT title, artist, year, artwork_hash FROM albums WHERE id = ? RETURNING id",
            )
            .bind(old)
            .fetch_one(pool)
            .await
            .unwrap()
            .get(0);
            sqlx::query("UPDATE tracks SET album_id = ? WHERE id = ?")
                .bind(new_id)
                .bind(track_id)
                .execute(pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT OR IGNORE INTO album_artists (album_id, artist_id)
                 SELECT ?, artist_id FROM album_artists WHERE album_id = ?",
            )
            .bind(new_id)
            .bind(old)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn consolidate_repairs_an_already_split_catalog() {
        let (pool, _d, _l) = scan(&[
            spec(
                "N",
                "01.flac",
                "One",
                "Norah Jones",
                "Come Away With Me",
                None,
                1,
            ),
            spec(
                "N",
                "02.flac",
                "Two",
                "Norah Jones",
                "Come Away With Me",
                None,
                2,
            ),
            spec(
                "N",
                "03.flac",
                "Three",
                "Norah Jones",
                "Come Away With Me",
                None,
                3,
            ),
            spec(
                "Other",
                "01.flac",
                "Solo",
                "Someone Else",
                "Another Album",
                None,
                1,
            ),
        ])
        .await;
        split_into_one_album_per_track(&pool, "Come Away With Me").await;
        assert_eq!(
            count(&pool, "albums").await,
            4,
            "precondition: split like the bug"
        );

        let removed = consolidate_albums(&pool).await.unwrap();
        assert_eq!(removed, 2);
        assert_eq!(
            albums(&pool).await,
            vec![
                ("Another Album".into(), Some("Someone Else".into()), 1),
                ("Come Away With Me".into(), Some("Norah Jones".into()), 3),
            ]
        );
        // Every track still has a valid album, and artist links survived.
        let orphans: i64 = sqlx::query(
            "SELECT COUNT(*) FROM tracks t LEFT JOIN albums a ON a.id = t.album_id WHERE a.id IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
        assert_eq!(orphans, 0);
        let links: i64 = sqlx::query(
            "SELECT COUNT(*) FROM album_artists aa JOIN artists ar ON ar.id = aa.artist_id
             JOIN albums a ON a.id = aa.album_id WHERE a.title = 'Come Away With Me' AND ar.name = 'Norah Jones'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
        assert_eq!(links, 1);
        // Idempotent.
        assert_eq!(consolidate_albums(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_scan_repairs_a_split_catalog_without_touching_any_file() {
        let (pool, _d, lib) = scan(&[
            spec(
                "N",
                "01.flac",
                "One",
                "Norah Jones",
                "Come Away With Me",
                None,
                1,
            ),
            spec(
                "N",
                "02.flac",
                "Two",
                "Norah Jones",
                "Come Away With Me",
                None,
                2,
            ),
        ])
        .await;
        split_into_one_album_per_track(&pool, "Come Away With Me").await;
        assert_eq!(count(&pool, "albums").await, 2);
        let report = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(
            report.files_updated + report.files_added,
            0,
            "nothing changed on disk"
        );
        assert_eq!(
            albums(&pool).await,
            vec![("Come Away With Me".into(), Some("Norah Jones".into()), 2)]
        );
    }

    #[tokio::test]
    async fn consolidate_merges_a_split_untagged_compilation_into_various_artists() {
        let (pool, _d, _l) = scan(&[
            spec("Comp", "01.flac", "A", "Artist One", "Mix", None, 1),
            spec("Comp", "02.flac", "B", "Artist Two", "Mix", None, 2),
        ])
        .await;
        split_into_one_album_per_track(&pool, "Mix").await;
        consolidate_albums(&pool).await.unwrap();
        assert_eq!(
            albums(&pool).await,
            vec![("Mix".into(), Some("Various Artists".into()), 2)]
        );
    }

    #[tokio::test]
    async fn consolidate_keeps_the_year_and_artwork_that_only_a_duplicate_had() {
        let (pool, _d, _l) = scan(&[
            spec(
                "N",
                "01.flac",
                "One",
                "Norah Jones",
                "Come Away With Me",
                None,
                1,
            ),
            spec(
                "N",
                "02.flac",
                "Two",
                "Norah Jones",
                "Come Away With Me",
                None,
                2,
            ),
        ])
        .await;
        split_into_one_album_per_track(&pool, "Come Away With Me").await;
        // Only the later duplicate has artwork; the lowest id survives.
        sqlx::query(
            "INSERT INTO artwork (hash, mime, bytes) VALUES ('abc123', 'image/png', x'00')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE albums SET artwork_hash = NULL, year = NULL WHERE id = (SELECT MIN(id) FROM albums)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE albums SET artwork_hash = 'abc123', year = 2012 WHERE id = (SELECT MAX(id) FROM albums)")
            .execute(&pool)
            .await
            .unwrap();
        consolidate_albums(&pool).await.unwrap();
        let r = sqlx::query("SELECT year, artwork_hash FROM albums")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(r.get::<Option<i64>, _>("year"), Some(2012));
        assert_eq!(
            r.get::<Option<String>, _>("artwork_hash").as_deref(),
            Some("abc123")
        );
    }

    #[tokio::test]
    async fn consolidate_never_merges_different_albums_that_share_only_a_title_or_artist() {
        let (pool, _d, _l) = scan(&[
            spec("X", "01.flac", "One", "Artist X", "Greatest Hits", None, 1),
            spec("Y", "01.flac", "One", "Artist Y", "Greatest Hits", None, 1), // same title, other artist + folder
            spec("X", "02.flac", "Live", "Artist X", "Live at Home", None, 2), // same artist, other title
        ])
        .await;
        assert_eq!(consolidate_albums(&pool).await.unwrap(), 0);
        assert_eq!(count(&pool, "albums").await, 3);
    }
}

/// DSF / DFF files: parsed by the server's own DSD reader and tagged through
/// their ID3v2 block (lofty cannot open them). Regression: they used to be
/// cataloged with no title/artist/album, no duration, and `decodable = 0`, so
/// they never appeared in the Albums grid and could not be played.
#[cfg(test)]
mod dsd_scan_tests {
    use super::fixtures::count;
    use super::*;
    use crate::dsd_meta::fixture::{dff, dsf, id3, PNG};

    fn write(lib: &Path, rel: &str, bytes: &[u8]) {
        let p = lib.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    async fn scan_dir(files: &[(&str, Vec<u8>)]) -> (SqlitePool, tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        for (rel, bytes) in files {
            write(&lib, rel, bytes);
        }
        let pool = db::open(&dir.path().join("t.db")).await.unwrap();
        run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        (pool, dir, lib)
    }

    fn sketches(n: u32, title: &str) -> Vec<u8> {
        dsf(
            2,
            Some(&id3(
                &[
                    ("TIT2", title),
                    ("TPE1", "Miles Davis"),
                    ("TALB", "Sketches of Spain"),
                    ("TRCK", &format!("{n}/5")),
                    ("TDRC", "1960"),
                    ("TCON", "Jazz"),
                ],
                Some(PNG),
            )),
        )
    }

    #[tokio::test]
    async fn a_tagged_dsf_is_cataloged_with_tags_technical_fields_and_playable() {
        let (pool, _d, _l) = scan_dir(&[(
            "Sketches/01 - Concierto.dsf",
            sketches(1, "Concierto De Aranjuez"),
        )])
        .await;
        let r = sqlx::query(
            "SELECT format, decodable, missing, title, artist, album, genre, year, track_no,
                    sample_rate, bit_depth, channels, duration_ms, bitrate, album_id, artwork_hash
             FROM tracks",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(r.get::<String, _>("format"), "dsf");
        assert_eq!(r.get::<i64, _>("decodable"), 1, "the server plays DSD");
        assert_eq!(r.get::<String, _>("title"), "Concierto De Aranjuez");
        assert_eq!(r.get::<String, _>("artist"), "Miles Davis");
        assert_eq!(r.get::<String, _>("album"), "Sketches of Spain");
        assert_eq!(r.get::<String, _>("genre"), "Jazz");
        assert_eq!(r.get::<i64, _>("year"), 1960);
        assert_eq!(r.get::<i64, _>("track_no"), 1);
        assert_eq!(
            r.get::<i64, _>("sample_rate"),
            2_822_400,
            "the DSD rate, which the player uses for DoP"
        );
        assert_eq!(r.get::<i64, _>("bit_depth"), 1);
        assert_eq!(r.get::<i64, _>("channels"), 2);
        let ms = r.get::<i64, _>("duration_ms");
        assert!((1990..=2010).contains(&ms), "duration {ms} ms");
        assert_eq!(r.get::<i64, _>("bitrate"), 5644);
        assert!(
            r.get::<Option<i64>, _>("album_id").is_some(),
            "it belongs to an album, so the grid shows it"
        );
        assert!(
            r.get::<Option<String>, _>("artwork_hash").is_some(),
            "embedded cover extracted"
        );
    }

    #[tokio::test]
    async fn dsf_tracks_group_into_one_album_and_appear_in_the_catalog() {
        let (pool, _d, _l) = scan_dir(&[
            ("Sketches/01.dsf", sketches(1, "One")),
            ("Sketches/02.dsf", sketches(2, "Two")),
            ("Sketches/03.dsf", sketches(3, "Three")),
        ])
        .await;
        let a = sqlx::query(
            "SELECT a.title, a.artist, a.year, COUNT(t.id) AS n FROM albums a JOIN tracks t ON t.album_id = a.id GROUP BY a.id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].get::<String, _>("title"), "Sketches of Spain");
        assert_eq!(a[0].get::<String, _>("artist"), "Miles Davis");
        assert_eq!(a[0].get::<i64, _>("n"), 3);
        // Searchable by title.
        let hits: i64 =
            sqlx::query("SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH 'Sketches'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(hits, 3);
    }

    #[tokio::test]
    async fn a_tagged_dff_is_cataloged_too() {
        let tags = id3(
            &[
                ("TIT2", "Moon Ray"),
                ("TPE1", "Roy Haynes Quartet"),
                ("TALB", "Out of the Afternoon"),
            ],
            None,
        );
        let (pool, _d, _l) = scan_dir(&[("Roy/01 Moon Ray.dff", dff(2, Some(&tags)))]).await;
        let r = sqlx::query(
            "SELECT format, decodable, title, album, sample_rate, duration_ms FROM tracks",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(r.get::<String, _>("format"), "dff");
        assert_eq!(r.get::<i64, _>("decodable"), 1);
        assert_eq!(r.get::<String, _>("title"), "Moon Ray");
        assert_eq!(r.get::<String, _>("album"), "Out of the Afternoon");
        assert_eq!(r.get::<i64, _>("sample_rate"), 2_822_400);
        assert!(r.get::<i64, _>("duration_ms") >= 1990);
    }

    #[tokio::test]
    async fn an_untagged_dsf_is_still_playable_with_technical_fields_and_a_filename_title() {
        let (pool, _d, _l) = scan_dir(&[("Loose/01 My Foolish Heart.dsf", dsf(1, None))]).await;
        let r = sqlx::query("SELECT decodable, title, album, sample_rate, duration_ms FROM tracks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(r.get::<i64, _>("decodable"), 1);
        assert_eq!(r.get::<String, _>("title"), "01 My Foolish Heart");
        assert!(r.get::<Option<String>, _>("album").is_none());
        assert_eq!(r.get::<i64, _>("sample_rate"), 2_822_400);
        assert!(r.get::<i64, _>("duration_ms") >= 990);
    }

    #[tokio::test]
    async fn a_corrupt_dsf_is_cataloged_but_stays_unplayable() {
        let (pool, _d, _l) = scan_dir(&[("Bad/broken.dsf", vec![0xABu8; 4096])]).await;
        let r = sqlx::query("SELECT decodable, sample_rate, title FROM tracks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(r.get::<i64, _>("decodable"), 0);
        assert!(r.get::<Option<i64>, _>("sample_rate").is_none());
        assert_eq!(r.get::<String, _>("title"), "broken");
    }

    #[tokio::test]
    async fn rescanning_unchanged_dsd_skips_it() {
        let (pool, _d, lib) = scan_dir(&[
            ("A/01.dsf", sketches(1, "One")),
            ("A/02.dsf", sketches(2, "Two")),
        ])
        .await;
        let r = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!((r.files_added, r.files_updated, r.files_skipped), (0, 0, 2));
    }

    /// The real-world upgrade path: the catalog already holds DSD rows from the
    /// old scanner (filename title, NULL everything, decodable = 0) and the
    /// files have not changed. A scan must still refresh them.
    #[tokio::test]
    async fn a_scan_upgrades_dsd_rows_cataloged_by_the_old_scanner() {
        let (pool, _d, lib) = scan_dir(&[
            ("Sketches/01.dsf", sketches(1, "One")),
            ("Sketches/02.dsf", sketches(2, "Two")),
        ])
        .await;
        // Put the catalog back into the pre-fix state.
        sqlx::query(
            "UPDATE tracks SET artist = NULL, album = NULL, album_id = NULL, genre = NULL, year = NULL,
                track_no = NULL, sample_rate = NULL, bit_depth = NULL, channels = NULL, duration_ms = NULL,
                bitrate = NULL, artwork_hash = NULL, decodable = 0, title = 'old-' || id",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM album_artists")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM albums")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(count(&pool, "albums").await, 0);

        let r = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(
            r.files_updated, 2,
            "refreshed although the files did not change"
        );
        let rows = sqlx::query(
            "SELECT title, album, decodable, sample_rate FROM tracks ORDER BY track_no",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let titles: Vec<String> = rows.iter().map(|x| x.get("title")).collect();
        assert_eq!(titles, vec!["One", "Two"]);
        assert!(rows.iter().all(|x| x.get::<i64, _>("decodable") == 1));
        assert_eq!(count(&pool, "albums").await, 1);

        // ...and once upgraded they are skipped again.
        let again = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!((again.files_updated, again.files_skipped), (0, 2));
    }
}

/// MQA detection from the `MQAENCODER` / `ORIGINALSAMPLERATE` tags MQA-encoded
/// FLACs carry.
#[cfg(test)]
mod mqa_tests {
    use super::fixtures::*;
    use super::*;
    use lofty::{
        file::TaggedFileExt,
        tag::{ItemKey, ItemValue, Tag, TagExt, TagItem, TagType},
    };

    const ENCODER: &str = "MQAEncode v1.1, 3091 (afa6eeb9), F8EC1703, Oct 22 2020 00:45:31";

    fn vorbis(pairs: &[(&str, &str)]) -> Tag {
        let mut t = Tag::new(TagType::VorbisComments);
        for (k, v) in pairs {
            t.insert_unchecked(TagItem::new(
                ItemKey::Unknown((*k).to_string()),
                ItemValue::Text((*v).to_string()),
            ));
        }
        t
    }

    #[test]
    fn detects_mqa_from_the_encoder_tag_and_reads_the_original_rate() {
        let t = vorbis(&[("MQAENCODER", ENCODER), ("ORIGINALSAMPLERATE", "96000")]);
        assert_eq!(mqa_of(&t), (true, Some(96_000)));
    }

    #[test]
    fn keys_are_matched_case_insensitively() {
        let t = vorbis(&[("mqaencoder", ENCODER), ("OriginalSampleRate", "48000")]);
        assert_eq!(mqa_of(&t), (true, Some(48_000)));
    }

    #[test]
    fn mqa_without_an_original_rate_is_still_mqa() {
        assert_eq!(mqa_of(&vorbis(&[("MQAENCODER", ENCODER)])), (true, None));
        assert_eq!(
            mqa_of(&vorbis(&[
                ("MQAENCODER", ENCODER),
                ("ORIGINALSAMPLERATE", "abc")
            ])),
            (true, None)
        );
        assert_eq!(
            mqa_of(&vorbis(&[
                ("MQAENCODER", ENCODER),
                ("ORIGINALSAMPLERATE", "0")
            ])),
            (true, None)
        );
    }

    #[test]
    fn a_stray_original_rate_or_empty_encoder_is_not_mqa() {
        assert_eq!(
            mqa_of(&vorbis(&[("ORIGINALSAMPLERATE", "96000")])),
            (false, None)
        );
        assert_eq!(mqa_of(&vorbis(&[("MQAENCODER", "   ")])), (false, None));
        assert_eq!(mqa_of(&vorbis(&[("TITLE", "x")])), (false, None));
        assert_eq!(mqa_of(&Tag::new(TagType::VorbisComments)), (false, None));
    }

    fn flac(
        lib: &Path,
        dir: &str,
        file: &str,
        title: &str,
        mqa: Option<&[(&str, &str)]>,
    ) -> PathBuf {
        make_track(
            lib,
            None,
            &fixtures::TrackSpec {
                dir,
                file,
                codec: "flac",
                title,
                artist: "Dave Brubeck",
                album: "Lullabies",
                album_artist: None,
                track_no: 1,
                year: Some("2020"),
                genre: None,
                art: false,
            },
        );
        let path = lib.join(dir).join(file);
        if let Some(extra) = mqa {
            let mut tagged = lofty::read_from_path(&path).unwrap();
            let tag = tagged.primary_tag_mut().unwrap();
            for (k, v) in extra {
                tag.insert_unchecked(TagItem::new(
                    ItemKey::Unknown((*k).to_string()),
                    ItemValue::Text((*v).to_string()),
                ));
            }
            tag.save_to_path(&path, lofty::config::WriteOptions::default())
                .unwrap();
        }
        path
    }

    async fn scan(lib: &Path, db: &Path) -> (SqlitePool, ScanReport) {
        let pool = db::open(db).await.unwrap();
        let r = run_scan_with_progress(&pool, &[lib.to_path_buf()], |_, _| {})
            .await
            .unwrap();
        (pool, r)
    }

    #[tokio::test]
    async fn scan_records_mqa_and_leaves_ordinary_flac_alone() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        flac(
            &lib,
            "A",
            "01.flac",
            "Brahms Lullaby",
            Some(&[("MQAENCODER", ENCODER), ("ORIGINALSAMPLERATE", "48000")]),
        );
        flac(&lib, "A", "02.flac", "Plain FLAC", None);
        let (pool, _) = scan(&lib, &dir.path().join("t.db")).await;
        let rows = sqlx::query(
            "SELECT title, mqa, original_sample_rate, mqa_checked FROM tracks ORDER BY title",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let got: Vec<(String, i64, Option<i64>, i64)> = rows
            .iter()
            .map(|r| {
                (
                    r.get("title"),
                    r.get("mqa"),
                    r.get("original_sample_rate"),
                    r.get("mqa_checked"),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("Brahms Lullaby".into(), 1, Some(48_000), 1),
                ("Plain FLAC".into(), 0, None, 1)
            ]
        );
    }

    #[tokio::test]
    async fn the_track_api_shape_carries_the_mqa_fields() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        flac(
            &lib,
            "A",
            "01.flac",
            "Brahms Lullaby",
            Some(&[("MQAENCODER", ENCODER), ("ORIGINALSAMPLERATE", "96000")]),
        );
        let (pool, _) = scan(&lib, &dir.path().join("t.db")).await;
        let t = db::get_track(&pool, 1).await.unwrap().unwrap();
        assert!(t.mqa);
        assert_eq!(t.original_sample_rate, Some(96_000));
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["mqa"], true);
        assert_eq!(json["original_sample_rate"], 96_000);
    }

    /// Catalogs built before the column existed: the next scan reads just the
    /// tags (the file is not re-hashed) and the row is never revisited.
    #[tokio::test]
    async fn a_scan_backfills_mqa_for_rows_cataloged_before_detection_existed() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        flac(
            &lib,
            "A",
            "01.flac",
            "Brahms Lullaby",
            Some(&[("MQAENCODER", ENCODER), ("ORIGINALSAMPLERATE", "48000")]),
        );
        flac(&lib, "A", "02.flac", "Plain FLAC", None);
        let db_path = dir.path().join("t.db");
        let (pool, _) = scan(&lib, &db_path).await;
        // Back to the pre-detection state.
        sqlx::query("UPDATE tracks SET mqa = 0, original_sample_rate = NULL, mqa_checked = 0")
            .execute(&pool)
            .await
            .unwrap();
        let hash_before: String =
            sqlx::query("SELECT hash FROM tracks WHERE title = 'Brahms Lullaby'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);

        let r = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(
            (r.files_added, r.files_updated, r.files_skipped),
            (0, 0, 2),
            "no re-analysis"
        );
        let rows = sqlx::query(
            "SELECT title, mqa, original_sample_rate, mqa_checked, hash FROM tracks ORDER BY title",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows[0].get::<i64, _>("mqa"), 1);
        assert_eq!(
            rows[0].get::<Option<i64>, _>("original_sample_rate"),
            Some(48_000)
        );
        assert_eq!(rows[1].get::<i64, _>("mqa"), 0);
        assert!(rows.iter().all(|r| r.get::<i64, _>("mqa_checked") == 1));
        assert_eq!(
            rows[0].get::<String, _>("hash"),
            hash_before,
            "hash untouched"
        );

        // Checked rows are not read again: tag changes on an UNCHANGED file
        // (same size/mtime) are not picked up until the file changes.
        sqlx::query("UPDATE tracks SET mqa = 0 WHERE title = 'Brahms Lullaby'")
            .execute(&pool)
            .await
            .unwrap();
        run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        let still: i64 = sqlx::query("SELECT mqa FROM tracks WHERE title = 'Brahms Lullaby'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(still, 0);
    }

    #[tokio::test]
    async fn a_changed_file_is_re_evaluated() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        let p = flac(&lib, "A", "01.flac", "Song", None);
        let db_path = dir.path().join("t.db");
        let (pool, _) = scan(&lib, &db_path).await;
        assert_eq!(count(&pool, "tracks").await, 1);
        let m0: i64 = sqlx::query("SELECT mqa FROM tracks")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(m0, 0);
        // The user re-tags the file with MQA info (file changes on disk).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut tagged = lofty::read_from_path(&p).unwrap();
        let tag = tagged.primary_tag_mut().unwrap();
        tag.insert_unchecked(TagItem::new(
            ItemKey::Unknown("MQAENCODER".into()),
            ItemValue::Text(ENCODER.into()),
        ));
        tag.save_to_path(&p, lofty::config::WriteOptions::default())
            .unwrap();
        let r = run_scan_with_progress(&pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(r.files_updated, 1);
        let m1: i64 = sqlx::query("SELECT mqa FROM tracks")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(m1, 1);
    }
}

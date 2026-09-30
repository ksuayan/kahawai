//! Genre normalization (docs/v2/kahawai-genre-normalization-spec.md).
//!
//! `tracks.genre` keeps the raw tag. Every distinct raw value is mapped to
//! zero or more canonical genres ("Pop, Rock" is two), and `track_genres`
//! joins tracks to them. The raw values are already in the database, so the
//! mapping is rebuilt from there after every scan: no file is re-read, and
//! an edit to the alias table applies on the next scan.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use kahawai_core::MusicError;
use sqlx::{Row, SqlitePool};

use crate::db;
use crate::genre_aliases::{ALIASES, KEYWORDS};

/// How a raw value's genre was found, for the curation report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// The alias table.
    Alias,
    /// A genre word inside an unlisted value ("Pinoy Rock" -> Rock).
    Keyword,
    /// Nothing matched: the cleaned value is its own genre, never dropped.
    Unmapped,
}

impl How {
    pub fn as_str(self) -> &'static str {
        match self {
            How::Alias => "alias",
            How::Keyword => "keyword",
            How::Unmapped => "unmapped",
        }
    }
}

fn aliases() -> &'static HashMap<&'static str, &'static [&'static str]> {
    static MAP: OnceLock<HashMap<&str, &[&str]>> = OnceLock::new();
    MAP.get_or_init(|| ALIASES.iter().copied().collect())
}

fn keywords() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&str, &str>> = OnceLock::new();
    MAP.get_or_init(|| KEYWORDS.iter().copied().collect())
}

/// Lowercase, keeping only letters, digits and `&`: the alias-table key.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || *c == '&')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Split a tag into its values on `/ ; | ,` and a spaced dash (" - "). Not
/// on `&`: "R&B" and "Drum & Bass" are one value each.
fn split_values(raw: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for part in raw.split(['/', ';', '|', ',']) {
        for piece in part.split(" - ") {
            let piece = piece.trim();
            if !piece.is_empty() {
                out.push(piece);
            }
        }
    }
    out
}

/// One value, tidied: leading ID3v1 references ("(17)Rock") and bracketed
/// asides ("Rock [80s]", "New Wave (A-Z)") removed, dots and underscores
/// read as spaces ("new.wave"), whitespace collapsed.
fn clean(value: &str) -> String {
    let mut s = String::with_capacity(value.len());
    let mut depth = 0u32;
    for c in value.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            '.' | '_' => s.push(' '),
            _ => s.push(c),
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Not a genre at all: nothing left, a bare number (a year, an ID3v1 index
/// without a name), or a numbered placeholder ("Genre_013").
fn is_junk(key: &str) -> bool {
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    key.is_empty()
        || digits(key)
        || key
            .strip_prefix("genre")
            .is_some_and(|rest| !rest.is_empty() && digits(rest))
}

/// A web address, checked before splitting (its slashes would split it).
fn is_url(raw: &str) -> bool {
    let lower = raw.to_lowercase();
    lower.contains("://") || lower.trim_start().starts_with("www.")
}

/// Title Case for an all-lowercase value; otherwise the value as tagged
/// (keeps "EBM"-style capitals).
fn display(cleaned: &str) -> String {
    if cleaned.chars().any(char::is_uppercase) {
        return cleaned.to_string();
    }
    cleaned
        .split(' ')
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().chain(cs).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Canonical genres for one raw tag, with how each was found. Order follows
/// the tag, without repeats.
pub fn normalize_detailed(raw: &str) -> Vec<(String, How)> {
    let mut out: Vec<(String, How)> = Vec::new();
    let mut push = |g: String, how: How| {
        if !out.iter().any(|(e, _)| *e == g) {
            out.push((g, how));
        }
    };
    if is_url(raw) {
        return out;
    }
    for value in split_values(raw) {
        let cleaned = clean(value);
        let key = squash(&cleaned);
        if is_junk(&key) {
            continue;
        }
        if let Some(genres) = aliases().get(key.as_str()) {
            for g in genres.iter() {
                push((*g).to_string(), How::Alias);
            }
            continue;
        }
        let words: Vec<String> = cleaned
            .split([' ', '-'])
            .map(squash)
            .filter(|w| !w.is_empty())
            .collect();
        let pairs = words.windows(2).map(|p| format!("{}{}", p[0], p[1]));
        let mut found: Vec<&str> = Vec::new();
        for w in words.iter().cloned().chain(pairs) {
            if let Some(g) = keywords().get(w.as_str()) {
                if !found.contains(g) {
                    found.push(*g);
                }
            }
        }
        if found.is_empty() {
            push(display(&cleaned), How::Unmapped);
        } else {
            for g in found {
                push(g.to_string(), How::Keyword);
            }
        }
    }
    out
}

/// Canonical genres for one raw tag (spec's `normalize_genre`). The server
/// itself uses [`normalize_detailed`]; this is the plain form for tests.
#[cfg(test)]
pub fn normalize_genre(raw: &str) -> Vec<String> {
    normalize_detailed(raw)
        .into_iter()
        .map(|(g, _)| g)
        .collect()
}

/// Rebuild `genre_map` and `track_genres` from the raw tags in `tracks`.
/// `track_genres` links present tracks only (tracks go missing only in a
/// scan, which ends with this), so genre counts need no join.
/// Every distinct raw value is mapped (a few hundred strings even for a big
/// library), so this is cheap enough to run after every scan.
pub async fn refresh_genres(pool: &SqlitePool) -> Result<(), MusicError> {
    let raws: Vec<String> =
        sqlx::query("SELECT DISTINCT genre FROM tracks WHERE genre IS NOT NULL")
            .fetch_all(pool)
            .await
            .map_err(db::cvt)?
            .iter()
            .map(|r| r.get(0))
            .collect();
    let mut tx = pool.begin().await.map_err(db::cvt)?;
    sqlx::query("DELETE FROM genre_map")
        .execute(&mut *tx)
        .await
        .map_err(db::cvt)?;
    sqlx::query("DELETE FROM track_genres")
        .execute(&mut *tx)
        .await
        .map_err(db::cvt)?;
    let mut canonical = HashSet::new();
    for raw in &raws {
        let mapped = normalize_detailed(raw);
        if mapped.is_empty() {
            // Kept so the report can show what was ignored.
            sqlx::query("INSERT INTO genre_map (raw, genre, how) VALUES (?, NULL, 'ignored')")
                .bind(raw)
                .execute(&mut *tx)
                .await
                .map_err(db::cvt)?;
        }
        for (genre, how) in mapped {
            canonical.insert(genre.clone());
            sqlx::query("INSERT INTO genre_map (raw, genre, how) VALUES (?, ?, ?)")
                .bind(raw)
                .bind(genre)
                .bind(how.as_str())
                .execute(&mut *tx)
                .await
                .map_err(db::cvt)?;
        }
    }
    let linked = sqlx::query(
        "INSERT OR IGNORE INTO track_genres (track_id, genre)
         SELECT t.id, m.genre FROM tracks t JOIN genre_map m ON m.raw = t.genre
         WHERE m.genre IS NOT NULL AND t.missing = 0",
    )
    .execute(&mut *tx)
    .await
    .map_err(db::cvt)?
    .rows_affected();
    tx.commit().await.map_err(db::cvt)?;
    tracing::info!(
        raw = raws.len(),
        genres = canonical.len(),
        links = linked,
        "genres refreshed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(raw: &str) -> Vec<String> {
        normalize_genre(raw)
    }

    #[test]
    fn case_spacing_and_spelling_variants_collapse() {
        for raw in [
            "Synthpop",
            "Synth-pop",
            "Synth Pop",
            "SynthPop",
            "synthie pop",
            "Synth-Pop",
        ] {
            assert_eq!(g(raw), ["Synth-Pop"], "{raw}");
        }
        for raw in ["Hip-Hop", "Hip Hop", "hiphop", "Rap"] {
            assert_eq!(g(raw), ["Hip-Hop"], "{raw}");
        }
        assert_eq!(g("  jazz "), ["Jazz"]);
        assert_eq!(g("Post Punk"), g("Post-Punk"));
        assert_eq!(g("Postpunk"), ["Post-Punk"]);
    }

    #[test]
    fn multi_value_tags_split_but_ampersands_survive() {
        assert_eq!(g("Pop, Rock"), ["Pop", "Rock"]);
        assert_eq!(g("Pop/Rock"), ["Pop", "Rock"]);
        assert_eq!(g("Rock; Alternative"), ["Rock", "Alternative"]);
        assert_eq!(g("Jazz | Blues"), ["Jazz", "Blues"]);
        assert_eq!(g("Punk - New Wave - Pop"), ["Punk", "New Wave", "Pop"]);
        assert_eq!(g("R&B"), ["R&B"]);
        assert_eq!(g("Drum & Bass"), ["Drum & Bass"]);
        assert_eq!(g("Drum'n'Bass"), ["Drum & Bass"]);
        assert_eq!(g("Rhythm & Blues"), ["R&B"]);
        assert_eq!(g("R&B / Soul & Funk"), ["R&B", "Soul", "Funk"]);
        assert_eq!(g("Hip-Hop/Rap"), ["Hip-Hop"], "no repeats");
        assert_eq!(
            g("Chill Out, Trip Hop, Lounge, Light Music,"),
            ["Chillout", "Trip-Hop", "Lounge", "Easy Listening"]
        );
    }

    #[test]
    fn brackets_numbers_and_placeholders() {
        assert_eq!(g("Rock [80s]"), ["Rock"]);
        assert_eq!(g("New Wave (A-Z)"), ["New Wave"]);
        assert_eq!(g("(17)Rock"), ["Rock"]);
        assert!(g("(17)").is_empty(), "bare ID3v1 index");
        assert!(g("0").is_empty());
        assert!(g("2011").is_empty(), "a year");
        assert!(g("Other").is_empty());
        assert!(g("Unknown genre").is_empty());
        assert!(g("").is_empty());
        assert!(g("   ").is_empty());
        assert!(g("http://theultimatebootlegexperience.blogspot.com").is_empty());
        assert_eq!(g("Other/rock"), ["Rock"]);
        assert_eq!(g("Pop/General"), ["Pop"]);
        assert_eq!(g("new.wave, post.punk"), ["New Wave", "Post-Punk"]);
        assert!(g("Genre_013").is_empty());
        assert_eq!(g("Pop/Jazzy"), ["Pop", "Jazz"]);
        assert_eq!(
            g("Singer/Songwriter, Alternative"),
            ["Singer-Songwriter", "Alternative"]
        );
    }

    #[test]
    fn other_languages() {
        assert_eq!(g("Klassiek"), ["Classical"]);
        assert_eq!(g("Musica classica"), ["Classical"]);
        assert_eq!(g("Électronique, Dance"), ["Electronic", "Dance"]);
        assert_eq!(g("Электронная музыка"), ["Electronic"]);
        assert_eq!(g("Bandes originales de films"), ["Soundtrack"]);
        assert_eq!(g("B.S.O."), ["Soundtrack"]);
        assert_eq!(g("Alternativa e indie"), ["Alternative", "Indie"]);
        assert_eq!(
            g("Musiques du monde, Brésil, Bossa Nova"),
            ["World", "Latin", "Bossa Nova"]
        );
    }

    #[test]
    fn unlisted_values_fall_back_to_genre_words_then_to_themselves() {
        assert_eq!(g("Pinoy Rock"), ["Rock"]);
        assert_eq!(g("Uplifting Trance"), ["Trance"]);
        assert_eq!(g("90s Pop"), ["Pop"]);
        assert_eq!(g("Glitch-Rock"), ["Rock"]);
        assert_eq!(g("Punk New wave"), ["Punk", "New Wave"]);
        assert_eq!(
            normalize_detailed("Pinoy Rock"),
            [("Rock".to_string(), How::Keyword)]
        );
        // Never dropped, never "Other": the long tail stays visible.
        assert_eq!(
            normalize_detailed("China Crisis"),
            [("China Crisis".to_string(), How::Unmapped)]
        );
        assert_eq!(g("like no other"), ["Like No Other"]);
    }

    #[test]
    fn sub_genres_fold_into_the_taxonomy() {
        assert_eq!(g("Deep House"), ["House"]);
        assert_eq!(g("Progressive Trance"), ["Trance"]);
        assert_eq!(g("Hard Bop"), ["Jazz"]);
        assert_eq!(g("Latin Jazz"), ["Jazz", "Latin"]);
        assert_eq!(g("Jazz Fusion/Smooth Jazz"), ["Jazz Fusion", "Smooth Jazz"]);
        assert_eq!(g("Classic Rock"), ["Rock"]);
        assert_eq!(g("Classic Jazz"), ["Jazz"], "not Classical");
        assert_eq!(g("Films/Games / Film Scores"), ["Soundtrack"]);
        assert_eq!(g("Rock'n'Roll"), ["Rock & Roll"]);
    }

    /// Curation aid, run by hand against a real library's tags:
    /// `sqlite3 -readonly music.db "SELECT genre, COUNT(*) FROM tracks
    /// WHERE genre IS NOT NULL GROUP BY genre" > genres.txt`, then
    /// `KAHAWAI_GENRES_FILE=genres.txt cargo test -p kahawai-server
    /// real_library_genres -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads a real library's genre list; run by hand"]
    fn real_library_genres() {
        let path = std::env::var("KAHAWAI_GENRES_FILE").expect("KAHAWAI_GENRES_FILE");
        let text = std::fs::read_to_string(path).unwrap();
        let mut tracks: HashMap<String, u64> = HashMap::new();
        // (tracks, raw value, genres) per way of mapping.
        type Rows = Vec<(u64, String, Vec<String>)>;
        let mut by_how: HashMap<&str, Rows> = HashMap::new();
        let (mut raw_n, mut tagged) = (0, 0u64);
        for line in text.lines() {
            let Some((raw, n)) = line.rsplit_once('|') else {
                continue;
            };
            let n: u64 = n.trim().parse().unwrap_or(0);
            raw_n += 1;
            tagged += n;
            let mapped = normalize_detailed(raw);
            let how = mapped
                .iter()
                .map(|(_, h)| *h)
                .max_by_key(|h| *h as u8)
                .map_or("ignored", |h| h.as_str());
            for (g, _) in &mapped {
                *tracks.entry(g.clone()).or_default() += n;
            }
            by_how.entry(how).or_default().push((
                n,
                raw.to_string(),
                mapped.into_iter().map(|(g, _)| g).collect(),
            ));
        }
        let mut genres: Vec<_> = tracks.into_iter().collect();
        genres.sort_by(|a, b| b.1.cmp(&a.1));
        println!(
            "{raw_n} raw values on {tagged} tracks -> {} genres",
            genres.len()
        );
        for (g, n) in &genres {
            println!("  {n:>6}  {g}");
        }
        for how in ["keyword", "unmapped", "ignored"] {
            let mut v = by_how.remove(how).unwrap_or_default();
            v.sort_by(|a, b| b.0.cmp(&a.0));
            let n: u64 = v.iter().map(|x| x.0).sum();
            println!("{how}: {} values, {n} tracks", v.len());
            for (n, raw, gs) in v.iter().take(60) {
                println!("  {n:>6}  {raw:?} -> {gs:?}");
            }
        }
    }

    /// Every alias target is a canonical name, and the canonical set stays
    /// small (spec: about 60).
    #[test]
    fn the_taxonomy_stays_small() {
        let canonical: HashSet<&str> = ALIASES
            .iter()
            .flat_map(|(_, gs)| gs.iter().copied())
            .chain(KEYWORDS.iter().map(|(_, g)| *g))
            .collect();
        assert!(
            canonical.len() <= 65,
            "{} genres: {canonical:?}",
            canonical.len()
        );
        for (key, _) in ALIASES {
            assert_eq!(*key, squash(key), "alias key {key:?} isn't squashed");
        }
        let keys: HashSet<&str> = ALIASES.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys.len(), ALIASES.len(), "duplicate alias key");
    }
}

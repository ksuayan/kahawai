//! Audiobooks (docs/v1/kahawai-audiobook-spec.md): scanning book folders into
//! books, parts and chapters, and the position, bookmark and history store.
//!
//! A position is always `book_offset_ms`, the time from the start of the
//! book, never a file offset: that is what makes a multi-part book seamless
//! and a saved position portable between clients.
//!
//! This file holds the pure rules (folder names, grouping, part order, MP4
//! chapters, sessions, offset resolution), then the database side. The HTTP
//! handlers are in [`crate::audiobooks_api`].

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Folder-name metadata
// ---------------------------------------------------------------------------

/// What the folder names say about a book. Tags win over these, except for
/// the series fields, which prefer the folder (see [`merge_field`]).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FolderMeta {
    pub author: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub year: Option<u16>,
    pub title: Option<String>,
    pub narrator: Option<String>,
}

fn clean(s: &str) -> Option<String> {
    let t = s.trim().trim_matches(|c: char| c == '-' || c == '_').trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// `Vol 1`, `Volume 2.5`, `Book 3`, `#4`, or a bare `5` at the start of a
/// folder name, with the separator after it. Returns the number and the
/// rest of the name.
fn split_volume(s: &str) -> Option<(f64, &str)> {
    let lower = s.to_ascii_lowercase();
    let mut skip = 0usize;
    for prefix in ["volume", "vol.", "vol", "book", "#"] {
        if lower.starts_with(prefix) {
            skip = prefix.len();
            break;
        }
    }
    let after = s[skip..].trim_start();
    let end = after
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(after.len());
    let num_s = after[..end].trim_end_matches('.');
    if num_s.is_empty() {
        return None;
    }
    let n: f64 = num_s.parse().ok()?;
    let rest = after[end..].trim_start();
    // A bare number needs a " - " after it, or "1984" would be a volume.
    let rest = if let Some(r) = rest.strip_prefix('-') {
        r.trim_start()
    } else if skip > 0 {
        rest
    } else {
        return None;
    };
    Some((n, rest))
}

/// A leading `YYYY - ` (1000 to 2999).
fn split_year(s: &str) -> Option<(u16, &str)> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() != 4 {
        return None;
    }
    let y: u16 = digits.parse().ok()?;
    if !(1000..=2999).contains(&y) {
        return None;
    }
    let rest = s[4..].trim_start().strip_prefix('-')?.trim_start();
    Some((y, rest))
}

/// Parse the folders from the library root down to the book's folder, using
/// the Audiobookshelf-compatible convention (reimplemented from the
/// published convention, not from its code):
///
/// `Author/Series/Vol 1 - 1999 - Title {Narrator}/`, with `Author/Title/`
/// and `Author/Series/Title/` as the shorter forms.
pub fn parse_folder(components: &[String]) -> FolderMeta {
    let mut meta = FolderMeta::default();
    let Some((last, parents)) = components.split_last() else {
        return meta;
    };
    let mut name = last.trim().to_string();

    // {Narrator} at the end.
    if let (Some(open), true) = (name.rfind('{'), name.trim_end().ends_with('}')) {
        let inner = name[open + 1..name.trim_end().len() - 1].to_string();
        meta.narrator = clean(&inner);
        name = name[..open].trim_end().to_string();
    }
    let mut rest = name.as_str();
    let mut had_volume = false;
    if let Some((n, r)) = split_volume(rest) {
        meta.series_index = Some(n);
        had_volume = true;
        rest = r;
    }
    if let Some((y, r)) = split_year(rest) {
        meta.year = Some(y);
        rest = r;
    }
    // A year with no volume ("1999 - Title") is the other common order.
    if !had_volume {
        if let Some((n, r)) = split_volume(rest) {
            meta.series_index = Some(n);
            had_volume = true;
            rest = r;
        }
    }
    meta.title = clean(rest).or_else(|| clean(&name));

    match parents {
        [] => {}
        [author] => meta.author = clean(author),
        [author, .., series] => {
            meta.author = clean(author);
            meta.series = clean(series);
        }
    }
    // Without a volume number, a middle folder is still a series (the
    // Author/Series/Title form), but a lone parent is the author.
    if parents.len() == 1 && had_volume {
        // "Author/Vol 2 - Title": a volume of an unnamed series. Keep the
        // author, leave the series blank.
        meta.series = None;
    }
    meta
}

/// Field precedence: embedded tags win, the folder fills blanks.
pub fn merge_field(tag: Option<String>, folder: Option<String>) -> Option<String> {
    tag.and_then(|t| clean(&t)).or(folder)
}

// ---------------------------------------------------------------------------
// Grouping and part order
// ---------------------------------------------------------------------------

/// `Disc 2`, `CD2`, `Disk 03`: a folder inside a book, not a book.
pub fn is_disc_folder(name: &str) -> bool {
    let l = name.trim().to_ascii_lowercase();
    for p in ["disc", "disk", "cd"] {
        if let Some(rest) = l.strip_prefix(p) {
            let rest = rest.trim_start_matches([' ', '_', '-', '.']);
            return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
        }
    }
    false
}

/// The disc number a file's `Disc N` / `CD N` folder names, if it is in one.
pub fn disc_from_path(file: &Path) -> Option<u32> {
    let name = file.parent()?.file_name()?.to_str()?;
    if !is_disc_folder(name) {
        return None;
    }
    name.chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// The directory that stands for the book a file belongs to: its own
/// directory, or the parent when that directory is a `Disc N` / `CD N`
/// folder (and again if discs are nested).
pub fn book_dir(file: &Path) -> PathBuf {
    let mut dir = file.parent().unwrap_or(Path::new("")).to_path_buf();
    while dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_disc_folder)
    {
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => break,
        }
    }
    dir
}

/// Natural order: runs of digits compare as numbers, so `2` sorts before `10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(&c) = it.peek() {
                        if c.is_ascii_digit() {
                            s.push(c);
                            it.next();
                        } else {
                            break;
                        }
                    }
                    s.trim_start_matches('0').to_string()
                };
                let (na, nb) = (take(&mut ai), take(&mut bi));
                let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(&nb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
                let ord = lower(x).cmp(&lower(y));
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// One audio file of a book, before it is given a place in the book.
#[derive(Debug, Clone, PartialEq)]
pub struct PartInput {
    pub path: PathBuf,
    pub disc: Option<u32>,
    pub track: Option<u32>,
    pub duration_ms: u64,
    pub title: Option<String>,
}

/// Order the parts: by `(disc, track)` tags when every part has a track
/// number, otherwise by natural file name (disc folders first, so
/// `CD1/01.mp3` precedes `CD2/01.mp3`).
pub fn order_parts(parts: &mut [PartInput]) {
    let all_tagged = parts.iter().all(|p| p.track.is_some());
    if all_tagged {
        parts.sort_by(|a, b| {
            // Track numbers that restart in each disc folder need the folder's
            // disc number when the tags carry none.
            let disc = |p: &PartInput| p.disc.or_else(|| disc_from_path(&p.path)).unwrap_or(1);
            (disc(a), a.track)
                .cmp(&(disc(b), b.track))
                .then_with(|| natural_cmp(&a.path.to_string_lossy(), &b.path.to_string_lossy()))
        });
    } else {
        parts.sort_by(|a, b| natural_cmp(&a.path.to_string_lossy(), &b.path.to_string_lossy()));
    }
}

/// Where each part starts: the sum of the durations before it.
pub fn start_offsets(parts: &[PartInput]) -> Vec<u64> {
    let mut at = 0u64;
    parts
        .iter()
        .map(|p| {
            let start = at;
            at += p.duration_ms;
            start
        })
        .collect()
}

// ---------------------------------------------------------------------------
// MP4 / m4b chapters
// ---------------------------------------------------------------------------

/// A chapter inside one file: its start within that file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileChapter {
    pub title: String,
    pub start_ms: u64,
}

fn be32(b: &[u8]) -> u64 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64
}

/// Walk the boxes in `data`, calling `f(fourcc, payload)`.
fn boxes<'a>(data: &'a [u8], mut f: impl FnMut(&'a [u8], &'a [u8])) {
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let mut size = be32(&data[at..]) as usize;
        let kind = &data[at + 4..at + 8];
        let mut header = 8usize;
        if size == 1 {
            // 64-bit size follows the type.
            if at + 16 > data.len() {
                return;
            }
            size = u64::from_be_bytes(data[at + 8..at + 16].try_into().unwrap()) as usize;
            header = 16;
        } else if size == 0 {
            size = data.len() - at; // runs to the end
        }
        if size < header || at + size > data.len() {
            return;
        }
        f(kind, &data[at + header..at + size]);
        at += size;
    }
}

/// The Nero `chpl` chapter list (`moov/udta/chpl`), which is what m4b files
/// from most tools carry: a count, then per chapter a start (in 100 ns
/// units) and a length-prefixed title. `moov` is passed in; the file
/// reader below finds it. QuickTime text-track chapters are not read.
pub fn parse_chpl(moov: &[u8]) -> Vec<FileChapter> {
    let mut out = Vec::new();
    boxes(moov, |kind, payload| {
        if kind != b"udta" {
            return;
        }
        boxes(payload, |kind, chpl| {
            if kind != b"chpl" || chpl.len() < 9 {
                return;
            }
            let version = chpl[0];
            // version(1) flags(3) [reserved(4) when version = 1] count(1)
            let mut at = 4 + if version == 1 { 4 } else { 0 };
            if at >= chpl.len() {
                return;
            }
            let count = chpl[at] as usize;
            at += 1;
            for _ in 0..count {
                if at + 9 > chpl.len() {
                    break;
                }
                let start = u64::from_be_bytes(chpl[at..at + 8].try_into().unwrap());
                let len = chpl[at + 8] as usize;
                at += 9;
                if at + len > chpl.len() {
                    break;
                }
                let title = String::from_utf8_lossy(&chpl[at..at + len])
                    .trim()
                    .to_string();
                at += len;
                out.push(FileChapter {
                    title,
                    start_ms: start / 10_000,
                });
            }
        });
    });
    out.sort_by_key(|c| c.start_ms);
    out
}

/// Chapters of an `.m4b` / `.m4a` file, read from its `moov` box. Only the
/// top-level boxes are scanned, so `mdat` is skipped by its size and a large
/// file is never read whole. Empty when there are none.
pub fn read_mp4_chapters(path: &Path) -> Vec<FileChapter> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let mut at = 0u64;
    while at + 8 <= len {
        let mut head = [0u8; 16];
        if f.seek(SeekFrom::Start(at)).is_err() || f.read(&mut head[..8]).is_err() {
            return Vec::new();
        }
        let mut size = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
        let kind = [head[4], head[5], head[6], head[7]];
        let mut header = 8u64;
        if size == 1 {
            if f.read_exact(&mut head[8..16]).is_err() {
                return Vec::new();
            }
            size = u64::from_be_bytes(head[8..16].try_into().unwrap());
            header = 16;
        } else if size == 0 {
            size = len - at;
        }
        if size < header {
            return Vec::new();
        }
        if &kind == b"moov" {
            // A moov is metadata, not audio: a few MB at most.
            let body = size - header;
            if body > 64 << 20 {
                return Vec::new();
            }
            let mut buf = vec![0u8; body as usize];
            if f.seek(SeekFrom::Start(at + header)).is_err() || f.read_exact(&mut buf).is_err() {
                return Vec::new();
            }
            return parse_chpl(&buf);
        }
        at += size;
    }
    Vec::new()
}

/// A book chapter: its start within the book.
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterOut {
    pub part_index: usize,
    pub title: String,
    pub start_offset_ms: u64,
    pub duration_ms: u64,
}

/// Build the book's chapter list. A part with embedded chapters contributes
/// those (a chapter ends where the next begins, the last at the end of its
/// part); a part without any is one chapter, titled like the part.
pub fn build_chapters(
    parts: &[PartInput],
    starts: &[u64],
    embedded: &[Vec<FileChapter>],
) -> Vec<ChapterOut> {
    let mut out = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        let part_title = p
            .title
            .clone()
            .or_else(|| {
                p.path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("Part {}", i + 1));
        let chaps: Vec<&FileChapter> = embedded
            .get(i)
            .map(|c| {
                c.iter()
                    .filter(|c| c.start_ms < p.duration_ms.max(1))
                    .collect()
            })
            .unwrap_or_default();
        if chaps.is_empty() {
            out.push(ChapterOut {
                part_index: i,
                title: part_title,
                start_offset_ms: starts[i],
                duration_ms: p.duration_ms,
            });
            continue;
        }
        for (k, c) in chaps.iter().enumerate() {
            let end = chaps
                .get(k + 1)
                .map(|n| n.start_ms)
                .unwrap_or(p.duration_ms)
                .max(c.start_ms);
            out.push(ChapterOut {
                part_index: i,
                title: if c.title.is_empty() {
                    format!("Chapter {}", out.len() + 1)
                } else {
                    c.title.clone()
                },
                start_offset_ms: starts[i] + c.start_ms,
                duration_ms: end - c.start_ms,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Positions, sessions, finishing
// ---------------------------------------------------------------------------

/// A listening session closes after this long without a position update.
pub const SESSION_GAP_MS: i64 = 15 * 60 * 1000;
/// A book counts as finished at this share of its length.
pub const FINISHED_SHARE: f64 = 0.97;

/// Is `now` the same calendar day as `then`? Days are UTC: the server has no
/// idea of the listener's time zone, and a session is a coarse marker.
pub fn same_day(then_ms: i64, now_ms: i64) -> bool {
    then_ms.div_euclid(86_400_000) == now_ms.div_euclid(86_400_000)
}

/// Does a position update at `now_ms` start a new listening session, given
/// the previous update at `last_ms`?
pub fn starts_new_session(last_ms: i64, now_ms: i64) -> bool {
    now_ms - last_ms > SESSION_GAP_MS || !same_day(last_ms, now_ms)
}

pub fn is_finished(offset_ms: i64, duration_ms: i64) -> bool {
    duration_ms > 0 && offset_ms as f64 >= FINISHED_SHARE * duration_ms as f64
}

/// A part's place in the book, for resolving an offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartSpan {
    pub track_id: i64,
    pub start_offset_ms: i64,
    pub duration_ms: i64,
}

/// Map a book offset to `(track_id, offset within that track)`. An offset
/// past the end lands at the end of the last part; a negative one at the
/// start. `None` when the book has no parts.
pub fn resolve_offset(parts: &[PartSpan], offset_ms: i64) -> Option<(i64, i64)> {
    let first = parts.first()?;
    let offset = offset_ms.max(0);
    for p in parts {
        if offset < p.start_offset_ms + p.duration_ms {
            return Some((p.track_id, (offset - p.start_offset_ms).max(0)));
        }
    }
    let last = parts.last().unwrap_or(first);
    Some((last.track_id, last.duration_ms))
}

#[cfg(test)]
mod pure_tests {
    use super::*;

    fn comps(s: &str) -> Vec<String> {
        s.split('/').map(str::to_string).collect()
    }

    #[test]
    fn the_full_folder_convention_is_parsed() {
        let m = parse_folder(&comps(
            "Brandon Sanderson/Stormlight Archive/Vol 2 - 2014 - Words of Radiance {Michael Kramer}",
        ));
        assert_eq!(m.author.as_deref(), Some("Brandon Sanderson"));
        assert_eq!(m.series.as_deref(), Some("Stormlight Archive"));
        assert_eq!(m.series_index, Some(2.0));
        assert_eq!(m.year, Some(2014));
        assert_eq!(m.title.as_deref(), Some("Words of Radiance"));
        assert_eq!(m.narrator.as_deref(), Some("Michael Kramer"));
    }

    #[test]
    fn the_short_forms_are_parsed() {
        let m = parse_folder(&comps("Ursula K. Le Guin/The Dispossessed"));
        assert_eq!(m.author.as_deref(), Some("Ursula K. Le Guin"));
        assert_eq!(m.title.as_deref(), Some("The Dispossessed"));
        assert_eq!((m.series, m.series_index, m.year), (None, None, None));

        let m = parse_folder(&comps("Author/Series Name/Some Title"));
        assert_eq!(m.series.as_deref(), Some("Series Name"));
        assert_eq!(m.title.as_deref(), Some("Some Title"));

        let m = parse_folder(&comps("Just A Title"));
        assert_eq!(m.title.as_deref(), Some("Just A Title"));
        assert_eq!(m.author, None);
    }

    #[test]
    fn volume_forms_and_decimal_volumes() {
        for (name, idx, title) in [
            ("Book 3 - Title", 3.0, "Title"),
            ("Volume 12 - Title", 12.0, "Title"),
            ("#4 - Title", 4.0, "Title"),
            ("5 - Title", 5.0, "Title"),
            ("Vol 1.5 - Title", 1.5, "Title"),
        ] {
            let m = parse_folder(&comps(&format!("A/S/{name}")));
            assert_eq!(m.series_index, Some(idx), "{name}");
            assert_eq!(m.title.as_deref(), Some(title), "{name}");
        }
    }

    #[test]
    fn a_title_that_is_a_number_is_not_a_volume() {
        let m = parse_folder(&comps("George Orwell/1984"));
        assert_eq!(m.series_index, None);
        assert_eq!(m.title.as_deref(), Some("1984"));
        let m = parse_folder(&comps("George Orwell/1984 {Simon Prebble}"));
        assert_eq!(m.title.as_deref(), Some("1984"));
        assert_eq!(m.narrator.as_deref(), Some("Simon Prebble"));
    }

    #[test]
    fn tags_win_and_the_folder_fills_blanks() {
        assert_eq!(
            merge_field(Some("Tagged".into()), Some("Folder".into())).as_deref(),
            Some("Tagged")
        );
        assert_eq!(
            merge_field(None, Some("Folder".into())).as_deref(),
            Some("Folder")
        );
        assert_eq!(
            merge_field(Some("  ".into()), Some("Folder".into())).as_deref(),
            Some("Folder")
        );
    }

    #[test]
    fn disc_folders_merge_into_their_book() {
        for n in ["Disc 1", "CD2", "disk 03", "CD-1", "disc_4"] {
            assert!(is_disc_folder(n), "{n}");
        }
        for n in ["Discworld", "Part 1", "CDs", "Disc"] {
            assert!(!is_disc_folder(n), "{n}");
        }
        assert_eq!(
            book_dir(Path::new("/a/Book/CD 2/01.mp3")),
            PathBuf::from("/a/Book")
        );
        assert_eq!(
            book_dir(Path::new("/a/Book/01.mp3")),
            PathBuf::from("/a/Book")
        );
    }

    #[test]
    fn natural_order_counts_digits_as_numbers() {
        let mut v = vec!["10.mp3", "2.mp3", "1.mp3", "02a.mp3"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["1.mp3", "2.mp3", "02a.mp3", "10.mp3"]);
    }

    fn part(path: &str, disc: Option<u32>, track: Option<u32>, ms: u64) -> PartInput {
        PartInput {
            path: PathBuf::from(path),
            disc,
            track,
            duration_ms: ms,
            title: None,
        }
    }

    #[test]
    fn parts_follow_tags_then_fall_back_to_file_names() {
        let mut p = vec![
            part("/b/z.mp3", Some(1), Some(2), 10),
            part("/b/a.mp3", Some(2), Some(1), 10),
            part("/b/m.mp3", Some(1), Some(1), 10),
        ];
        order_parts(&mut p);
        let names: Vec<_> = p.iter().map(|x| x.path.to_str().unwrap()).collect();
        assert_eq!(
            names,
            ["/b/m.mp3", "/b/z.mp3", "/b/a.mp3"],
            "by disc, track"
        );

        let mut p = vec![
            part("/b/Part 10.mp3", None, None, 10),
            part("/b/Part 2.mp3", None, None, 10),
            part("/b/Part 1.mp3", None, Some(5), 10),
        ];
        order_parts(&mut p);
        let names: Vec<_> = p.iter().map(|x| x.path.to_str().unwrap()).collect();
        assert_eq!(
            names,
            ["/b/Part 1.mp3", "/b/Part 2.mp3", "/b/Part 10.mp3"],
            "one missing track number: names decide"
        );

        let mut p = vec![
            part("/b/CD2/01.mp3", None, None, 10),
            part("/b/CD1/02.mp3", None, None, 10),
            part("/b/CD1/01.mp3", None, None, 10),
        ];
        order_parts(&mut p);
        let names: Vec<_> = p.iter().map(|x| x.path.to_str().unwrap()).collect();
        assert_eq!(names, ["/b/CD1/01.mp3", "/b/CD1/02.mp3", "/b/CD2/01.mp3"]);
    }

    #[test]
    fn track_numbers_that_restart_per_disc_folder_still_order_correctly() {
        let mut p = vec![
            part("/b/CD 2/01.mp3", None, Some(1), 10),
            part("/b/CD 1/02.mp3", None, Some(2), 10),
            part("/b/CD 1/01.mp3", None, Some(1), 10),
        ];
        order_parts(&mut p);
        let names: Vec<_> = p.iter().map(|x| x.path.to_str().unwrap()).collect();
        assert_eq!(
            names,
            ["/b/CD 1/01.mp3", "/b/CD 1/02.mp3", "/b/CD 2/01.mp3"]
        );
        assert_eq!(disc_from_path(Path::new("/b/Disc 03/x.mp3")), Some(3));
        assert_eq!(disc_from_path(Path::new("/b/x.mp3")), None);
    }

    #[test]
    fn part_offsets_are_the_sum_of_the_parts_before() {
        let p = vec![
            part("a", None, None, 1000),
            part("b", None, None, 2500),
            part("c", None, None, 400),
        ];
        assert_eq!(start_offsets(&p), [0, 1000, 3500]);
    }

    /// An MP4 box.
    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    fn chpl_payload(version: u8, chapters: &[(u64, &str)]) -> Vec<u8> {
        let mut v = vec![version, 0, 0, 0];
        if version == 1 {
            v.extend_from_slice(&[0, 0, 0, 0]);
        }
        v.push(chapters.len() as u8);
        for (ms, title) in chapters {
            v.extend_from_slice(&(ms * 10_000).to_be_bytes());
            v.push(title.len() as u8);
            v.extend_from_slice(title.as_bytes());
        }
        v
    }

    #[test]
    fn nero_chapters_are_read_from_moov() {
        for version in [0u8, 1] {
            let chpl = mp4_box(
                b"chpl",
                &chpl_payload(version, &[(0, "Intro"), (60_000, "One"), (185_500, "Two")]),
            );
            let moov = [
                mp4_box(b"mvhd", &[0; 20]),
                mp4_box(b"udta", &[mp4_box(b"meta", &[0; 4]), chpl].concat()),
            ]
            .concat();
            let c = parse_chpl(&moov);
            assert_eq!(
                c,
                vec![
                    FileChapter {
                        title: "Intro".into(),
                        start_ms: 0
                    },
                    FileChapter {
                        title: "One".into(),
                        start_ms: 60_000
                    },
                    FileChapter {
                        title: "Two".into(),
                        start_ms: 185_500
                    },
                ],
                "chpl v{version}"
            );
        }
    }

    #[test]
    fn a_truncated_or_missing_chapter_list_never_panics() {
        assert!(parse_chpl(&[]).is_empty());
        assert!(parse_chpl(&mp4_box(b"udta", &[])).is_empty());
        let chpl = mp4_box(b"chpl", &chpl_payload(0, &[(0, "A"), (5_000, "Bee")]));
        let udta = mp4_box(b"udta", &chpl);
        for cut in 0..udta.len() {
            let _ = parse_chpl(&udta[..cut]);
        }
    }

    #[test]
    fn chapters_run_to_the_next_start_and_a_part_without_any_is_one_chapter() {
        let parts = vec![
            part("/b/one.m4b", None, None, 100_000),
            part("/b/two.mp3", None, None, 50_000),
        ];
        let starts = start_offsets(&parts);
        let embedded = vec![
            vec![
                FileChapter {
                    title: "A".into(),
                    start_ms: 0,
                },
                FileChapter {
                    title: "B".into(),
                    start_ms: 40_000,
                },
            ],
            vec![],
        ];
        let c = build_chapters(&parts, &starts, &embedded);
        assert_eq!(c.len(), 3);
        assert_eq!(
            (c[0].title.as_str(), c[0].start_offset_ms, c[0].duration_ms),
            ("A", 0, 40_000)
        );
        assert_eq!(
            (c[1].title.as_str(), c[1].start_offset_ms, c[1].duration_ms),
            ("B", 40_000, 60_000)
        );
        assert_eq!(
            (c[2].title.as_str(), c[2].start_offset_ms, c[2].duration_ms),
            ("two", 100_000, 50_000)
        );
        assert_eq!(c[2].part_index, 1);
    }

    #[test]
    fn a_chapter_past_the_end_of_its_file_is_dropped() {
        let parts = vec![part("/b/x.m4b", None, None, 10_000)];
        let embedded = vec![vec![
            FileChapter {
                title: "A".into(),
                start_ms: 0,
            },
            FileChapter {
                title: "Bogus".into(),
                start_ms: 99_000,
            },
        ]];
        let c = build_chapters(&parts, &[0], &embedded);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].duration_ms, 10_000);
    }

    #[test]
    fn sessions_split_on_a_long_gap_or_a_new_day() {
        let t0 = 1_800_000_000_000i64; // some instant
        let day_start = t0 - t0.rem_euclid(86_400_000);
        let noon = day_start + 12 * 3_600_000;
        assert!(
            !starts_new_session(noon, noon + 10_000),
            "10 s later: same session"
        );
        assert!(
            !starts_new_session(noon, noon + SESSION_GAP_MS),
            "exactly the gap: same"
        );
        assert!(
            starts_new_session(noon, noon + SESSION_GAP_MS + 1),
            "past the gap"
        );
        let late = day_start + 86_400_000 - 60_000;
        assert!(
            starts_new_session(late, late + 120_000),
            "across midnight, even a minute apart"
        );
    }

    #[test]
    fn a_book_is_finished_at_97_percent() {
        assert!(!is_finished(96_999, 100_000));
        assert!(is_finished(97_000, 100_000));
        assert!(!is_finished(5, 0), "an unknown length is never finished");
    }

    #[test]
    fn offsets_resolve_to_a_part_and_a_position_inside_it() {
        let parts = [
            PartSpan {
                track_id: 10,
                start_offset_ms: 0,
                duration_ms: 1000,
            },
            PartSpan {
                track_id: 11,
                start_offset_ms: 1000,
                duration_ms: 2000,
            },
        ];
        assert_eq!(resolve_offset(&parts, 0), Some((10, 0)));
        assert_eq!(resolve_offset(&parts, 999), Some((10, 999)));
        assert_eq!(
            resolve_offset(&parts, 1000),
            Some((11, 0)),
            "a boundary belongs to the next part"
        );
        assert_eq!(resolve_offset(&parts, 2500), Some((11, 1500)));
        assert_eq!(
            resolve_offset(&parts, 9999),
            Some((11, 2000)),
            "past the end: the end"
        );
        assert_eq!(resolve_offset(&parts, -5), Some((10, 0)));
        assert_eq!(resolve_offset(&[], 5), None);
    }
}

// ---------------------------------------------------------------------------
// Database: roots, scan, books
// ---------------------------------------------------------------------------

use kahawai_core::{AudioFormat, MusicError};
use lofty::prelude::*;
use lofty::tag::ItemKey;
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use tracing::{info, warn};

use crate::db::cvt;

pub fn now_ms() -> i64 {
    crate::jobs::now_ms()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Root {
    pub id: i64,
    pub path: String,
    pub name: String,
}

pub async fn list_roots(pool: &SqlitePool) -> Result<Vec<Root>, MusicError> {
    let rows = sqlx::query("SELECT id, path, name FROM audiobook_roots ORDER BY name, id")
        .fetch_all(pool)
        .await
        .map_err(cvt)?;
    Ok(rows
        .iter()
        .map(|r| Root {
            id: r.get("id"),
            path: r.get("path"),
            name: r.get("name"),
        })
        .collect())
}

/// The audiobook folders, for the music scan to leave alone.
pub async fn root_paths(pool: &SqlitePool) -> Result<Vec<PathBuf>, MusicError> {
    Ok(list_roots(pool)
        .await?
        .into_iter()
        .map(|r| PathBuf::from(r.path))
        .collect())
}

/// Register a folder of audiobooks. It must exist, and must not be, contain
/// or sit inside another audiobook folder (a book would be found twice).
pub async fn add_root(
    pool: &SqlitePool,
    path: &str,
    name: Option<&str>,
) -> Result<Root, MusicError> {
    let p = Path::new(path.trim());
    let meta = std::fs::metadata(p)
        .map_err(|_| MusicError::BadRequest(format!("{} is not a folder", p.display())))?;
    if !meta.is_dir() {
        return Err(MusicError::BadRequest(format!(
            "{} is not a folder",
            p.display()
        )));
    }
    let canon = std::fs::canonicalize(p)
        .map_err(|e| MusicError::BadRequest(format!("{}: {e}", p.display())))?;
    for r in list_roots(pool).await? {
        let other = PathBuf::from(&r.path);
        if canon == other {
            return Err(MusicError::Conflict(
                "that folder is already an audiobook folder".into(),
            ));
        }
        if canon.starts_with(&other) || other.starts_with(&canon) {
            return Err(MusicError::Conflict(format!(
                "that folder overlaps the audiobook folder {}",
                r.path
            )));
        }
    }
    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .or_else(|| {
            canon
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Audiobooks".to_string());
    let id: i64 =
        sqlx::query("INSERT INTO audiobook_roots (path, name) VALUES (?, ?) RETURNING id")
            .bind(canon.to_string_lossy().to_string())
            .bind(&name)
            .fetch_one(pool)
            .await
            .map_err(cvt)?
            .get(0);
    Ok(Root {
        id,
        path: canon.to_string_lossy().to_string(),
        name,
    })
}

/// Forget a folder, its books, and their positions, bookmarks and history.
/// The files stay on disk; their track rows are removed.
pub async fn delete_root(pool: &SqlitePool, id: i64) -> Result<(), MusicError> {
    let mut tx = pool.begin().await.map_err(cvt)?;
    let found = sqlx::query("SELECT 1 FROM audiobook_roots WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(cvt)?;
    if found.is_none() {
        return Err(MusicError::NotFound(format!("audiobook folder {id}")));
    }
    // Foreign keys cascade from the books; the track rows are ours to remove.
    let track_ids: Vec<i64> = sqlx::query(
        "SELECT p.track_id FROM audiobook_parts p JOIN audiobooks b ON b.id = p.book_id WHERE b.root_id = ?",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await
    .map_err(cvt)?
    .iter()
    .map(|r| r.get(0))
    .collect();
    sqlx::query("DELETE FROM audiobooks WHERE root_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(cvt)?;
    for t in track_ids {
        sqlx::query("DELETE FROM tracks WHERE id = ? AND kind = 'audiobook'")
            .bind(t)
            .execute(&mut *tx)
            .await
            .map_err(cvt)?;
    }
    sqlx::query("DELETE FROM audiobook_roots WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(cvt)?;
    tx.commit().await.map_err(cvt)?;
    Ok(())
}

/// Audio extensions an audiobook folder may hold: everything the music
/// scanner knows except SACD images, plus `.m4b`.
fn book_audio_format(path: &Path) -> Option<AudioFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ext == "m4b" {
        return Some(AudioFormat::M4a);
    }
    match AudioFormat::from_extension(&ext) {
        AudioFormat::Unknown | AudioFormat::SacdIso => None,
        f => Some(f),
    }
}

/// Everything one file tells us.
struct FileInfo {
    path: PathBuf,
    format: AudioFormat,
    size: i64,
    mtime: Option<i64>,
    part: PartInput,
    sample_rate: Option<u32>,
    bit_depth: Option<u8>,
    channels: Option<u8>,
    bitrate: Option<u32>,
    album: Option<String>,
    artist: Option<String>,
    album_artist: Option<String>,
    narrator: Option<String>,
    series: Option<String>,
    series_index: Option<f64>,
    year: Option<u16>,
    cover: Option<(String, Vec<u8>)>,
    chapters: Vec<FileChapter>,
}

fn read_file(path: &Path, format: AudioFormat, size: i64, mtime: Option<i64>) -> FileInfo {
    let mut f = FileInfo {
        path: path.to_path_buf(),
        format,
        size,
        mtime,
        part: PartInput {
            path: path.to_path_buf(),
            disc: None,
            track: None,
            duration_ms: 0,
            title: None,
        },
        sample_rate: None,
        bit_depth: None,
        channels: None,
        bitrate: None,
        album: None,
        artist: None,
        album_artist: None,
        narrator: None,
        series: None,
        series_index: None,
        year: None,
        cover: None,
        chapters: Vec::new(),
    };
    match lofty::read_from_path(path) {
        Ok(tagged) => {
            let props = tagged.properties();
            f.part.duration_ms = props.duration().as_millis() as u64;
            f.sample_rate = props.sample_rate();
            f.bit_depth = props.bit_depth();
            f.channels = props.channels();
            f.bitrate = props.audio_bitrate();
            if let Some(tag) = tagged.primary_tag() {
                f.part.title = tag.title().map(|c| c.into_owned());
                f.part.track = tag.track();
                f.part.disc = tag.disk();
                f.album = tag.album().map(|c| c.into_owned());
                f.artist = tag.artist().map(|c| c.into_owned());
                f.album_artist = tag.get_string(&ItemKey::AlbumArtist).map(str::to_string);
                // Narrators are most often tagged as the composer.
                f.narrator = tag.get_string(&ItemKey::Composer).map(str::to_string);
                f.series = tag.get_string(&ItemKey::Movement).map(str::to_string);
                f.series_index = tag
                    .get_string(&ItemKey::MovementNumber)
                    .and_then(|s| s.trim().parse().ok());
                f.year = crate::normalize::sane_year(
                    tag.year().and_then(|y| u16::try_from(y).ok()),
                    crate::normalize::current_year(),
                );
                if let Some(pic) = tag.pictures().iter().find(|p| !p.data().is_empty()) {
                    let mime = pic
                        .mime_type()
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_else(|| "image/jpeg".to_string());
                    f.cover = Some((mime, pic.data().to_vec()));
                }
            }
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "audiobook: unreadable file; cataloged without tags")
        }
    }
    if matches!(format, AudioFormat::M4a) {
        f.chapters = read_mp4_chapters(path);
    }
    if f.part.title.is_none() {
        f.part.title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string);
    }
    f
}

/// Result of one audiobook scan.
#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct ScanSummary {
    pub books: u64,
    pub files: u64,
    pub unreadable_roots: u64,
}

/// A cover picture beside the audio.
fn folder_cover(dir: &Path) -> Option<(String, Vec<u8>)> {
    for stem in ["cover", "folder", "front", "poster"] {
        for (ext, mime) in [
            ("jpg", "image/jpeg"),
            ("jpeg", "image/jpeg"),
            ("png", "image/png"),
        ] {
            let p = dir.join(format!("{stem}.{ext}"));
            if let Ok(bytes) = std::fs::read(&p) {
                if !bytes.is_empty() {
                    return Some((mime.to_string(), bytes));
                }
            }
        }
    }
    None
}

/// Walk one root and read every audio file, grouped by book directory.
/// Blocking.
fn walk_root(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<FileInfo>> {
    let mut books: std::collections::BTreeMap<PathBuf, Vec<FileInfo>> = Default::default();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!(root = %root.display(), error = %e, "audiobook walk error; entry skipped");
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(format) = book_audio_format(entry.path()) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let info = read_file(
            entry.path(),
            format,
            meta.len() as i64,
            crate::scanner::mtime_secs(&meta),
        );
        books.entry(book_dir(entry.path())).or_default().push(info);
    }
    books
}

/// Scan every audiobook folder: files become `tracks` rows
/// (`kind = 'audiobook'`), directories become books with ordered parts and
/// chapters. Rescans keep each book's id, so positions, bookmarks and
/// history survive; fields the user edited by hand are left alone.
pub async fn scan(pool: &SqlitePool) -> Result<ScanSummary, MusicError> {
    let mut summary = ScanSummary::default();
    for root in list_roots(pool).await? {
        let root_path = PathBuf::from(&root.path);
        // An unmounted drive must not look like a deleted library.
        if !root_path.is_dir() {
            warn!(root = %root.path, "audiobook folder is missing; left as it was");
            summary.unreadable_roots += 1;
            continue;
        }
        let walked = {
            let rp = root_path.clone();
            tokio::task::spawn_blocking(move || walk_root(&rp))
                .await
                .map_err(|e| MusicError::JobFailed(format!("audiobook walk: {e}")))?
        };
        let mut seen_tracks: Vec<i64> = Vec::new();
        for (dir, files) in walked {
            summary.files += files.len() as u64;
            let (book_id, track_ids) = store_book(pool, &root, &root_path, &dir, files).await?;
            seen_tracks.extend(track_ids);
            let _ = book_id;
            summary.books += 1;
        }
        // Files gone from disk: mark missing, keep the rows (and with them
        // positions and bookmarks) in case they come back.
        let present: std::collections::HashSet<i64> = seen_tracks.into_iter().collect();
        let rows = sqlx::query(
            "SELECT p.track_id FROM audiobook_parts p JOIN audiobooks b ON b.id = p.book_id
             WHERE b.root_id = ?",
        )
        .bind(root.id)
        .fetch_all(pool)
        .await
        .map_err(cvt)?;
        for r in rows {
            let t: i64 = r.get(0);
            if !present.contains(&t) {
                sqlx::query("UPDATE tracks SET missing = 1 WHERE id = ? AND kind = 'audiobook'")
                    .bind(t)
                    .execute(pool)
                    .await
                    .map_err(cvt)?;
            }
        }
    }
    info!(
        books = summary.books,
        files = summary.files,
        "audiobook scan complete"
    );
    Ok(summary)
}

async fn store_book(
    pool: &SqlitePool,
    root: &Root,
    root_path: &Path,
    dir: &Path,
    files: Vec<FileInfo>,
) -> Result<(i64, Vec<i64>), MusicError> {
    let mut files = files;
    // Order the parts, then carry the file info along in that order.
    let mut inputs: Vec<PartInput> = files.iter().map(|f| f.part.clone()).collect();
    order_parts(&mut inputs);
    let pos: std::collections::HashMap<&PathBuf, usize> = inputs
        .iter()
        .enumerate()
        .map(|(i, p)| (&p.path, i))
        .collect();
    files.sort_by_key(|f| pos[&f.path]);
    let starts = start_offsets(&inputs);
    let embedded: Vec<Vec<FileChapter>> = files.iter().map(|f| f.chapters.clone()).collect();
    let chapters = build_chapters(&inputs, &starts, &embedded);
    let total: u64 = inputs.iter().map(|p| p.duration_ms).sum();

    // Book fields: tags win, the folder fills blanks, series prefer the folder.
    let rel: Vec<String> = dir
        .strip_prefix(root_path)
        .unwrap_or(dir)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    // A book directly in the root has no folder metadata beyond its name.
    let rel = if rel.is_empty() {
        vec![dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.name.clone())]
    } else {
        rel
    };
    let folder = parse_folder(&rel);
    let first = |f: &dyn Fn(&FileInfo) -> Option<String>| files.iter().find_map(f);
    let tag_album = first(&|f| f.album.clone());
    let tag_author = first(&|f| f.album_artist.clone()).or_else(|| first(&|f| f.artist.clone()));
    let tag_narrator = first(&|f| f.narrator.clone());
    let title = merge_field(tag_album, folder.title.clone())
        .unwrap_or_else(|| rel.last().cloned().unwrap_or_default());
    let author = merge_field(tag_author, folder.author.clone());
    let narrator = merge_field(tag_narrator, folder.narrator.clone());
    let series = folder
        .series
        .clone()
        .or_else(|| first(&|f| f.series.clone()).and_then(|s| clean(&s)));
    let series_index = folder
        .series_index
        .or_else(|| files.iter().find_map(|f| f.series_index));
    let year = folder.year.or_else(|| files.iter().find_map(|f| f.year));
    let cover = files
        .iter()
        .find_map(|f| f.cover.clone())
        .or_else(|| folder_cover(dir));

    let mut tx = pool.begin().await.map_err(cvt)?;
    // Cover.
    let cover_hash = if let Some((mime, bytes)) = cover {
        let hash = blake3::hash(&bytes).to_hex().to_string();
        sqlx::query("INSERT OR IGNORE INTO artwork (hash, mime, bytes) VALUES (?, ?, ?)")
            .bind(&hash)
            .bind(mime)
            .bind(bytes)
            .execute(&mut *tx)
            .await
            .map_err(cvt)?;
        Some(hash)
    } else {
        None
    };
    // The book row, keeping its id (and so everything hung on it).
    let dir_s = dir.to_string_lossy().to_string();
    let existing = sqlx::query("SELECT id, meta_edited FROM audiobooks WHERE path = ?")
        .bind(&dir_s)
        .fetch_optional(&mut *tx)
        .await
        .map_err(cvt)?;
    let book_id: i64 = match existing {
        Some(r) => {
            let id: i64 = r.get("id");
            let edited: i64 = r.get("meta_edited");
            if edited == 0 {
                sqlx::query(
                    // A blank from the scan keeps what a lookup filled in.
                    "UPDATE audiobooks SET root_id = ?, title = ?, author = COALESCE(?, author),
                       narrator = COALESCE(?, narrator), series = COALESCE(?, series),
                       series_index = COALESCE(?, series_index), year = COALESCE(?, year),
                       cover_hash = COALESCE(?, cover_hash), duration_ms = ? WHERE id = ?",
                )
                .bind(root.id)
                .bind(&title)
                .bind(&author)
                .bind(&narrator)
                .bind(&series)
                .bind(series_index)
                .bind(year.map(i64::from))
                .bind(&cover_hash)
                .bind(total as i64)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(cvt)?;
            } else {
                sqlx::query("UPDATE audiobooks SET root_id = ?, duration_ms = ?, cover_hash = COALESCE(cover_hash, ?) WHERE id = ?")
                    .bind(root.id)
                    .bind(total as i64)
                    .bind(&cover_hash)
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .map_err(cvt)?;
            }
            id
        }
        None => sqlx::query(
            "INSERT INTO audiobooks (root_id, path, title, author, narrator, series, series_index,
                                     year, cover_hash, duration_ms, added_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(root.id)
        .bind(&dir_s)
        .bind(&title)
        .bind(&author)
        .bind(&narrator)
        .bind(&series)
        .bind(series_index)
        .bind(year.map(i64::from))
        .bind(&cover_hash)
        .bind(total as i64)
        .bind(now_ms())
        .fetch_one(&mut *tx)
        .await
        .map_err(cvt)?
        .get(0),
    };

    // Track rows for the files, then the book's parts and chapters.
    let mut track_ids = Vec::with_capacity(files.len());
    for f in &files {
        let id: i64 = sqlx::query(
            "INSERT INTO tracks (path, format, kind, file_size, file_mtime, duration_ms, sample_rate,
                                 bit_depth, channels, bitrate, title, album, artist, track_no, disc_no,
                                 missing, decodable, mqa_checked, mbid_checked)
             VALUES (?, ?, 'audiobook', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 1, 1, 1)
             ON CONFLICT(path) DO UPDATE SET
               format = excluded.format, kind = 'audiobook', file_size = excluded.file_size,
               file_mtime = excluded.file_mtime, duration_ms = excluded.duration_ms,
               sample_rate = excluded.sample_rate, bit_depth = excluded.bit_depth,
               channels = excluded.channels, bitrate = excluded.bitrate, title = excluded.title,
               album = excluded.album, artist = excluded.artist, track_no = excluded.track_no,
               disc_no = excluded.disc_no, missing = 0, decodable = 1
             RETURNING id",
        )
        .bind(f.path.to_string_lossy().to_string())
        .bind(f.format.wire_name())
        .bind(f.size)
        .bind(f.mtime)
        .bind(f.part.duration_ms as i64)
        .bind(f.sample_rate.map(i64::from))
        .bind(f.bit_depth.map(i64::from))
        .bind(f.channels.map(i64::from))
        .bind(f.bitrate.map(i64::from))
        .bind(&f.part.title)
        .bind(&title)
        .bind(&author)
        .bind(f.part.track.map(i64::from))
        .bind(f.part.disc.map(i64::from))
        .fetch_one(&mut *tx)
        .await
        .map_err(cvt)?
        .get(0);
        track_ids.push(id);
    }
    // Free the parts this book had and any other book held for these files
    // (a file that moved between folders), then lay the parts out again.
    sqlx::query("DELETE FROM audiobook_chapters WHERE book_id = ?")
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .map_err(cvt)?;
    sqlx::query("DELETE FROM audiobook_parts WHERE book_id = ?")
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .map_err(cvt)?;
    for t in &track_ids {
        sqlx::query("DELETE FROM audiobook_parts WHERE track_id = ?")
            .bind(t)
            .execute(&mut *tx)
            .await
            .map_err(cvt)?;
    }
    let mut part_ids = Vec::with_capacity(files.len());
    for (i, (f, t)) in files.iter().zip(&track_ids).enumerate() {
        let pid: i64 = sqlx::query(
            "INSERT INTO audiobook_parts (book_id, track_id, part_index, title, start_offset_ms, duration_ms)
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(book_id)
        .bind(t)
        .bind(i as i64)
        .bind(&f.part.title)
        .bind(starts[i] as i64)
        .bind(f.part.duration_ms as i64)
        .fetch_one(&mut *tx)
        .await
        .map_err(cvt)?
        .get(0);
        part_ids.push(pid);
    }
    for c in &chapters {
        sqlx::query(
            "INSERT INTO audiobook_chapters (book_id, part_id, title, start_offset_ms, duration_ms)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(book_id)
        .bind(part_ids[c.part_index])
        .bind(&c.title)
        .bind(c.start_offset_ms as i64)
        .bind(c.duration_ms as i64)
        .execute(&mut *tx)
        .await
        .map_err(cvt)?;
    }
    tx.commit().await.map_err(cvt)?;
    Ok((book_id, track_ids))
}

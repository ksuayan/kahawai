//! Tags for DSD files (DSF / DFF).
//!
//! `lofty` cannot open DSD containers, so before this module DSD files were
//! cataloged with no title/artist/album at all (and therefore never showed up
//! in the Albums grid). DSD files carry their tags as an **ID3v2** block:
//!
//! - **DSF**: the header holds a "pointer to metadata" (file offset 20,
//!   little-endian u64); the ID3v2 block runs from there to the end of file.
//! - **DFF** (DSDIFF): an `ID3 ` chunk among the top-level chunks (big-endian
//!   sizes, chunks padded to even length).
//!
//! The ID3v2 reader here handles what real tools write: v2.3 and v2.4, all
//! four text encodings, the unsynchronisation flag, an extended header,
//! padding, and an `APIC` cover. It is defensive: malformed or truncated tags
//! yield whatever could be read, never a panic.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use crate::dsd::DsdInfo;

/// Largest ID3 block we will read (embedded covers can be large, not huge).
const MAX_TAG_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// MIME type from the tag; may be empty (the caller sniffs the bytes).
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DsdTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u16>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub picture: Option<Picture>,
}

/// Read the tags of a DSF/DFF file. `None` when the file has no ID3 block.
pub fn read_tags(path: &Path, info: &DsdInfo) -> Option<DsdTags> {
    let mut f = File::open(path).ok()?;
    let block = if info.is_dsf {
        dsf_id3_block(&mut f)?
    } else {
        dff_id3_block(&mut f)?
    };
    parse_id3v2(&block)
}

fn read_exact_vec(f: &mut File, len: u64) -> Option<Vec<u8>> {
    if !(10..=MAX_TAG_BYTES).contains(&len) {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn dsf_id3_block(f: &mut File) -> Option<Vec<u8>> {
    let file_len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(20)).ok()?;
    let mut p = [0u8; 8];
    f.read_exact(&mut p).ok()?;
    let ptr = u64::from_le_bytes(p);
    if ptr == 0 || ptr >= file_len {
        return None;
    }
    f.seek(SeekFrom::Start(ptr)).ok()?;
    read_exact_vec(f, file_len - ptr)
}

fn dff_id3_block(f: &mut File) -> Option<Vec<u8>> {
    let file_len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(16)).ok()?; // past "FRM8" + size + "DSD "
    let mut pos = 16u64;
    while pos + 12 <= file_len {
        let mut h = [0u8; 12];
        f.read_exact(&mut h).ok()?;
        let size = u64::from_be_bytes(h[4..12].try_into().ok()?);
        pos += 12;
        if &h[0..4] == b"ID3 " {
            return read_exact_vec(f, size.min(file_len.saturating_sub(pos)));
        }
        // Chunks are padded to an even length.
        pos = pos.checked_add(size + (size & 1))?;
        f.seek(SeekFrom::Start(pos)).ok()?;
    }
    None
}

// ---------------------------------------------------------------------------
// ID3v2
// ---------------------------------------------------------------------------

fn synchsafe(b: &[u8]) -> u32 {
    b.iter()
        .take(4)
        .fold(0u32, |acc, &x| (acc << 7) | u32::from(x & 0x7F))
}

fn be32(b: &[u8]) -> u32 {
    b.iter()
        .take(4)
        .fold(0u32, |acc, &x| (acc << 8) | u32::from(x))
}

/// Undo the unsynchronisation scheme: `FF 00` -> `FF`.
fn deunsync(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        out.push(data[i]);
        if data[i] == 0xFF && data.get(i + 1) == Some(&0) {
            i += 1;
        }
        i += 1;
    }
    out
}

/// Decode ID3 text in one of the four encodings.
fn decode_text(enc: u8, bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], big: bool| -> String {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| {
                if big {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match enc {
        1 => match bytes {
            [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
            [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
            _ => utf16(bytes, false),
        },
        2 => utf16(bytes, true),
        3 => String::from_utf8_lossy(bytes).into_owned(),
        _ => bytes.iter().map(|&b| b as char).collect(), // ISO-8859-1
    }
}

/// A text frame's values (v2.4 allows several, NUL separated), joined with
/// "; " so the scanner's artist splitter sees them as separate names.
fn text_value(data: &[u8]) -> Option<String> {
    let (&enc, rest) = data.split_first()?;
    let s = decode_text(enc, rest);
    let parts: Vec<&str> = s
        .split('\0')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// Leading number of "3/12", "03", " 7 ".
fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|&n| n > 0)
}

/// Year from "2012", "2012-05-01", "2012-05-01T10:00".
fn year_of(s: &str) -> Option<u16> {
    let y: String = s.trim().chars().take(4).collect();
    (y.len() == 4)
        .then(|| y.parse().ok())
        .flatten()
        .filter(|&y| y > 0)
}

/// "(13)Pop" -> "Pop"; "(13)" or "13" -> None (a bare ID3v1 index).
fn genre_of(s: &str) -> Option<String> {
    let mut rest = s.trim();
    while let Some(after) = rest.strip_prefix('(') {
        match after.find(')') {
            Some(end) => rest = after[end + 1..].trim_start(),
            None => break,
        }
    }
    let rest = rest.trim();
    if rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit()) {
        None
    } else {
        Some(rest.to_string())
    }
}

fn parse_apic(data: &[u8]) -> Option<(u8, Picture)> {
    let (&enc, rest) = data.split_first()?;
    let mime_end = rest.iter().position(|&b| b == 0)?;
    let mime: String = rest[..mime_end].iter().map(|&b| b as char).collect();
    let rest = &rest[mime_end + 1..];
    let (&pic_type, rest) = rest.split_first()?;
    // Description: terminated by one NUL (8-bit encodings) or two (UTF-16).
    let wide = enc == 1 || enc == 2;
    let mut i = 0;
    loop {
        if wide {
            if i + 1 >= rest.len() {
                return None;
            }
            if rest[i] == 0 && rest[i + 1] == 0 {
                i += 2;
                break;
            }
            i += 2;
        } else {
            if i >= rest.len() {
                return None;
            }
            if rest[i] == 0 {
                i += 1;
                break;
            }
            i += 1;
        }
    }
    let image = &rest[i..];
    if image.is_empty() || mime == "-->" {
        return None;
    }
    Some((
        pic_type,
        Picture {
            mime,
            data: image.to_vec(),
        },
    ))
}

/// Parse an ID3v2.3 / v2.4 block. `None` if it is not an ID3v2 tag.
pub fn parse_id3v2(block: &[u8]) -> Option<DsdTags> {
    if block.len() < 10 || &block[0..3] != b"ID3" {
        return None;
    }
    let major = block[3];
    if major != 3 && major != 4 {
        return None; // v2.2 (3-char frame ids) is not used by DSD tools
    }
    let flags = block[5];
    let size = synchsafe(&block[6..10]) as usize;
    let end = (10 + size).min(block.len());
    let mut body = block[10..end].to_vec();
    if flags & 0x80 != 0 && major == 3 {
        body = deunsync(&body);
    }
    let mut pos = 0usize;
    if flags & 0x40 != 0 && body.len() >= 4 {
        // Extended header: v2.3 size excludes its own 4 bytes; v2.4 includes them.
        pos = if major == 4 {
            synchsafe(&body[0..4]) as usize
        } else {
            be32(&body[0..4]) as usize + 4
        };
    }

    let mut tags = DsdTags::default();
    let mut best_pic: Option<(u8, Picture)> = None;
    while pos + 10 <= body.len() {
        let id = &body[pos..pos + 4];
        if id[0] == 0 {
            break; // padding
        }
        let fsize = if major == 4 {
            synchsafe(&body[pos + 4..pos + 8])
        } else {
            be32(&body[pos + 4..pos + 8])
        } as usize;
        let fflags = [body[pos + 8], body[pos + 9]];
        let start = pos + 10;
        let Some(fend) = start.checked_add(fsize).filter(|&e| e <= body.len()) else {
            break; // truncated frame
        };
        pos = fend;

        // Skip compressed / encrypted frames; honour per-frame v2.4 flags.
        let (skip, unsync, data_len_indicator) = if major == 4 {
            (
                fflags[1] & 0x0C != 0,
                fflags[1] & 0x02 != 0,
                fflags[1] & 0x01 != 0,
            )
        } else {
            (fflags[1] & 0xC0 != 0, false, false)
        };
        if skip {
            continue;
        }
        let mut data = body[start..fend].to_vec();
        if data_len_indicator && data.len() >= 4 {
            data.drain(0..4);
        }
        if unsync {
            data = deunsync(&data);
        }

        match id {
            b"TIT2" => tags.title = text_value(&data),
            b"TPE1" => tags.artist = text_value(&data),
            b"TALB" => tags.album = text_value(&data),
            b"TPE2" => tags.album_artist = text_value(&data),
            b"TCON" => tags.genre = text_value(&data).and_then(|g| genre_of(&g)),
            b"TRCK" => tags.track_no = text_value(&data).and_then(|t| leading_number(&t)),
            b"TPOS" => tags.disc_no = text_value(&data).and_then(|t| leading_number(&t)),
            // v2.4 recording time; v2.3 year. TDRC wins if both are present.
            b"TDRC" => tags.year = text_value(&data).and_then(|t| year_of(&t)).or(tags.year),
            b"TYER" => {
                tags.year = tags
                    .year
                    .or_else(|| text_value(&data).and_then(|t| year_of(&t)))
            }
            b"APIC" => {
                if let Some((t, pic)) = parse_apic(&data) {
                    // Prefer the front cover (type 3); otherwise the first picture.
                    let better = match &best_pic {
                        None => true,
                        Some((bt, _)) => t == 3 && *bt != 3,
                    };
                    if better {
                        best_pic = Some((t, pic));
                    }
                }
            }
            _ => {}
        }
    }
    tags.picture = best_pic.map(|(_, p)| p);
    Some(tags)
}

/// Builders for tagged DSD fixtures, shared with the scanner tests.
#[cfg(test)]
pub(crate) mod fixture {
    use crate::dop::fixture::{make_dff, make_dsf, BLOCK_LEN};

    // -- ID3 builders ------------------------------------------------------

    pub fn ss(n: usize) -> [u8; 4] {
        [
            (n >> 21 & 0x7F) as u8,
            (n >> 14 & 0x7F) as u8,
            (n >> 7 & 0x7F) as u8,
            (n & 0x7F) as u8,
        ]
    }

    pub fn frame(major: u8, id: &str, body: &[u8]) -> Vec<u8> {
        let mut f = id.as_bytes().to_vec();
        if major == 4 {
            f.extend_from_slice(&ss(body.len()));
        } else {
            f.extend_from_slice(&(body.len() as u32).to_be_bytes());
        }
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(body);
        f
    }

    pub fn text(enc: u8, s: &str) -> Vec<u8> {
        let mut b = vec![enc];
        match enc {
            1 => {
                b.extend_from_slice(&[0xFF, 0xFE]);
                for u in s.encode_utf16() {
                    b.extend_from_slice(&u.to_le_bytes());
                }
            }
            2 => {
                for u in s.encode_utf16() {
                    b.extend_from_slice(&u.to_be_bytes());
                }
            }
            3 => b.extend_from_slice(s.as_bytes()),
            _ => b.extend(s.chars().map(|c| c as u8)),
        }
        b
    }

    pub fn tag(major: u8, flags: u8, frames: &[Vec<u8>], pad: usize) -> Vec<u8> {
        let body: Vec<u8> = frames
            .iter()
            .flatten()
            .copied()
            .chain(std::iter::repeat_n(0, pad))
            .collect();
        let mut t = b"ID3".to_vec();
        t.extend_from_slice(&[major, 0, flags]);
        t.extend_from_slice(&ss(body.len()));
        t.extend(body);
        t
    }

    pub const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4];

    pub fn apic(enc: u8, mime: &str, pic_type: u8, desc: &str, data: &[u8]) -> Vec<u8> {
        let mut b = vec![enc];
        b.extend_from_slice(mime.as_bytes());
        b.push(0);
        b.push(pic_type);
        match enc {
            1 | 2 => {
                for u in desc.encode_utf16() {
                    b.extend_from_slice(&u.to_le_bytes());
                }
                b.extend_from_slice(&[0, 0]);
            }
            _ => {
                b.extend_from_slice(desc.as_bytes());
                b.push(0);
            }
        }
        b.extend_from_slice(data);
        b
    }

    /// ID3v2.4 tag of UTF-8 text frames, e.g. `[("TIT2", "Moon Ray")]`, plus an
    /// optional front cover.
    pub fn id3(frames: &[(&str, &str)], cover: Option<&[u8]>) -> Vec<u8> {
        let mut fs: Vec<Vec<u8>> = frames
            .iter()
            .map(|(id, v)| frame(4, id, &text(3, v)))
            .collect();
        if let Some(c) = cover {
            fs.push(frame(4, "APIC", &apic(0, "image/png", 3, "", c)));
        }
        tag(4, 0, &fs, 16)
    }

    /// A valid stereo DSD64 DSF about `seconds` long, optionally tagged.
    pub fn dsf(seconds: u32, id3_block: Option<&[u8]>) -> Vec<u8> {
        let blocks = (seconds as usize * 2_822_400 / 8).div_ceil(BLOCK_LEN);
        let payload = vec![0x69u8; blocks * BLOCK_LEN];
        let mut f = make_dsf(
            2,
            2_822_400,
            &[payload.clone(), payload],
            seconds as u64 * 2_822_400,
        );
        if let Some(t) = id3_block {
            let ptr = f.len() as u64;
            f.extend_from_slice(t);
            f[20..28].copy_from_slice(&ptr.to_le_bytes());
        }
        f
    }

    /// A valid stereo DSD64 DFF about `seconds` long, optionally tagged.
    pub fn dff(seconds: u32, id3_block: Option<&[u8]>) -> Vec<u8> {
        let frames = seconds as usize * 2_822_400 / 8;
        let mut f = make_dff(2, 2_822_400, &vec![0x69u8; frames * 2]);
        if let Some(t) = id3_block {
            f.extend_from_slice(b"ID3 ");
            f.extend_from_slice(&(t.len() as u64).to_be_bytes());
            f.extend_from_slice(t);
            if t.len() % 2 == 1 {
                f.push(0);
            }
            let form = (f.len() - 12) as u64;
            f[4..12].copy_from_slice(&form.to_be_bytes());
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::dop::fixture::{make_dff, make_dsf, BLOCK_LEN};

    // -- parser ------------------------------------------------------------

    #[test]
    fn reads_the_usual_v24_fields() {
        let t = tag(
            4,
            0,
            &[
                frame(4, "TIT2", &text(3, "Moon Ray")),
                frame(4, "TPE1", &text(3, "Roy Haynes Quartet")),
                frame(4, "TALB", &text(3, "Out of the Afternoon")),
                frame(4, "TPE2", &text(3, "Roy Haynes")),
                frame(4, "TRCK", &text(3, "1/8")),
                frame(4, "TPOS", &text(3, "2/2")),
                frame(4, "TDRC", &text(3, "1962-05-01")),
                frame(4, "TCON", &text(3, "Jazz")),
            ],
            32,
        );
        let g = parse_id3v2(&t).unwrap();
        assert_eq!(g.title.as_deref(), Some("Moon Ray"));
        assert_eq!(g.artist.as_deref(), Some("Roy Haynes Quartet"));
        assert_eq!(g.album.as_deref(), Some("Out of the Afternoon"));
        assert_eq!(g.album_artist.as_deref(), Some("Roy Haynes"));
        assert_eq!(
            (g.track_no, g.disc_no, g.year),
            (Some(1), Some(2), Some(1962))
        );
        assert_eq!(g.genre.as_deref(), Some("Jazz"));
        assert!(g.picture.is_none());
    }

    #[test]
    fn reads_v23_with_year_frame_and_big_endian_sizes() {
        let t = tag(
            3,
            0,
            &[
                frame(3, "TIT2", &text(0, "Caf\u{e9}")),
                frame(3, "TYER", &text(0, "1959")),
            ],
            0,
        );
        let g = parse_id3v2(&t).unwrap();
        assert_eq!(g.title.as_deref(), Some("Café"), "latin-1 decoded");
        assert_eq!(g.year, Some(1959));
    }

    #[test]
    fn decodes_every_text_encoding() {
        for enc in 0..=3u8 {
            let t = tag(4, 0, &[frame(4, "TIT2", &text(enc, "Blue Train"))], 0);
            assert_eq!(
                parse_id3v2(&t).unwrap().title.as_deref(),
                Some("Blue Train"),
                "enc {enc}"
            );
        }
        for enc in [1u8, 2, 3] {
            let t = tag(4, 0, &[frame(4, "TPE1", &text(enc, "Björk 日本語"))], 0);
            assert_eq!(
                parse_id3v2(&t).unwrap().artist.as_deref(),
                Some("Björk 日本語"),
                "enc {enc}"
            );
        }
    }

    #[test]
    fn multiple_values_become_a_semicolon_list_the_artist_splitter_understands() {
        let mut body = vec![3u8];
        body.extend_from_slice(b"Artist One\0Artist Two");
        let t = tag(4, 0, &[frame(4, "TPE1", &body)], 0);
        assert_eq!(
            parse_id3v2(&t).unwrap().artist.as_deref(),
            Some("Artist One; Artist Two")
        );
    }

    #[test]
    fn genre_strips_id3v1_index_references() {
        for (raw, want) in [
            ("(13)Pop", Some("Pop")),
            ("(13)", None),
            ("13", None),
            ("Jazz", Some("Jazz")),
            ("(9)(13)Rock", Some("Rock")),
        ] {
            let t = tag(3, 0, &[frame(3, "TCON", &text(0, raw))], 0);
            assert_eq!(parse_id3v2(&t).unwrap().genre.as_deref(), want, "{raw}");
        }
    }

    #[test]
    fn track_and_year_variants() {
        let t = tag(
            4,
            0,
            &[
                frame(4, "TRCK", &text(3, "07")),
                frame(4, "TDRC", &text(3, "2012")),
            ],
            0,
        );
        let g = parse_id3v2(&t).unwrap();
        assert_eq!((g.track_no, g.year), (Some(7), Some(2012)));
        let bad = tag(
            4,
            0,
            &[
                frame(4, "TRCK", &text(3, "x")),
                frame(4, "TDRC", &text(3, "??")),
            ],
            0,
        );
        let g = parse_id3v2(&bad).unwrap();
        assert_eq!((g.track_no, g.year), (None, None));
    }

    #[test]
    fn prefers_the_front_cover_and_handles_wide_descriptions() {
        let t = tag(
            4,
            0,
            &[
                frame(
                    4,
                    "APIC",
                    &apic(3, "image/jpeg", 4, "back", &[0xFF, 0xD8, 0xFF, 9]),
                ),
                frame(4, "APIC", &apic(1, "image/png", 3, "Front", PNG)),
            ],
            0,
        );
        let pic = parse_id3v2(&t).unwrap().picture.unwrap();
        assert_eq!(pic.mime, "image/png");
        assert_eq!(pic.data, PNG);
    }

    #[test]
    fn takes_the_only_picture_whatever_its_type() {
        let t = tag(
            3,
            0,
            &[frame(3, "APIC", &apic(0, "image/png", 0, "", PNG))],
            0,
        );
        assert_eq!(parse_id3v2(&t).unwrap().picture.unwrap().data, PNG);
    }

    #[test]
    fn honours_the_unsynchronisation_flag_and_skips_an_extended_header() {
        // v2.3, unsync flag: the frame is built with its DECODED size, then the
        // whole body is escaped (FF -> FF 00); the reader must undo that.
        let decoded = frame(3, "TIT2", &[0, b'A', 0xFF, b'B']);
        let mut escaped = Vec::new();
        for &b in &decoded {
            escaped.push(b);
            if b == 0xFF {
                escaped.push(0);
            }
        }
        assert!(escaped.len() > decoded.len());
        let mut t = b"ID3".to_vec();
        t.extend_from_slice(&[3, 0, 0x80]);
        t.extend_from_slice(&ss(escaped.len()));
        t.extend(&escaped);
        assert_eq!(parse_id3v2(&t).unwrap().title.as_deref(), Some("A\u{ff}B"));

        // v2.4 extended header (size 6, flag byte count 1, flags 0).
        let mut ext = ss(6).to_vec();
        ext.extend_from_slice(&[1, 0]);
        let mut frames = ext;
        frames.extend(frame(4, "TIT2", &text(3, "After ext")));
        let mut t2 = b"ID3".to_vec();
        t2.extend_from_slice(&[4, 0, 0x40]);
        t2.extend_from_slice(&ss(frames.len()));
        t2.extend(frames);
        assert_eq!(
            parse_id3v2(&t2).unwrap().title.as_deref(),
            Some("After ext")
        );
    }

    #[test]
    fn stops_at_padding_and_ignores_unknown_frames() {
        let t = tag(
            4,
            0,
            &[
                frame(4, "TXXX", b"\x03desc\0val"),
                frame(4, "TIT2", &text(3, "Kept")),
            ],
            100,
        );
        assert_eq!(parse_id3v2(&t).unwrap().title.as_deref(), Some("Kept"));
    }

    #[test]
    fn rejects_non_id3_and_unsupported_versions() {
        assert!(parse_id3v2(b"").is_none());
        assert!(parse_id3v2(b"not an id3 tag at all").is_none());
        let mut v22 = tag(4, 0, &[], 0);
        v22[3] = 2;
        assert!(parse_id3v2(&v22).is_none());
    }

    #[test]
    fn never_panics_on_truncated_or_corrupt_tags() {
        let full = tag(
            4,
            0,
            &[
                frame(4, "TIT2", &text(1, "Truncate me")),
                frame(4, "APIC", &apic(3, "image/png", 3, "c", PNG)),
                frame(4, "TRCK", &text(3, "3/9")),
            ],
            16,
        );
        for cut in 0..full.len() {
            let _ = parse_id3v2(&full[..cut]);
        }
        // Frame sizes that lie about their length.
        let mut lie = full.clone();
        lie[10 + 4..10 + 8].copy_from_slice(&ss(0x0FFF_FFFF));
        let _ = parse_id3v2(&lie);
        // Random-ish garbage after a valid header.
        let mut junk = tag(3, 0, &[], 0);
        junk.extend((0..500u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8));
        let _ = parse_id3v2(&junk);
    }

    // -- containers ----------------------------------------------------------

    fn dsf_with_tags(tags: &[u8]) -> Vec<u8> {
        let payload = vec![0x69u8; BLOCK_LEN];
        let mut f = make_dsf(
            2,
            2_822_400,
            &[payload.clone(), payload],
            BLOCK_LEN as u64 * 8,
        );
        let ptr = f.len() as u64;
        f.extend_from_slice(tags);
        f[20..28].copy_from_slice(&ptr.to_le_bytes());
        f
    }

    fn info_of(bytes: &[u8]) -> DsdInfo {
        crate::dsd::parse_dsd(&mut std::io::Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn reads_tags_from_a_dsf_metadata_block() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = dsf_with_tags(&tag(
            3,
            0,
            &[
                frame(3, "TIT2", &text(0, "Concierto")),
                frame(3, "TALB", &text(0, "Sketches of Spain")),
            ],
            0,
        ));
        let path = dir.path().join("a.dsf");
        std::fs::write(&path, &bytes).unwrap();
        let t = read_tags(&path, &info_of(&bytes)).unwrap();
        assert_eq!(t.title.as_deref(), Some("Concierto"));
        assert_eq!(t.album.as_deref(), Some("Sketches of Spain"));
    }

    #[test]
    fn a_dsf_without_a_metadata_pointer_has_no_tags() {
        let dir = tempfile::tempdir().unwrap();
        let payload = vec![0u8; BLOCK_LEN];
        let bytes = make_dsf(
            2,
            2_822_400,
            &[payload.clone(), payload],
            BLOCK_LEN as u64 * 8,
        );
        let path = dir.path().join("plain.dsf");
        std::fs::write(&path, &bytes).unwrap();
        assert!(read_tags(&path, &info_of(&bytes)).is_none());
    }

    #[test]
    fn a_bogus_dsf_metadata_pointer_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = dsf_with_tags(&tag(3, 0, &[frame(3, "TIT2", &text(0, "x"))], 0));
        bytes[20..28].copy_from_slice(&(u64::MAX / 2).to_le_bytes());
        let path = dir.path().join("bad.dsf");
        std::fs::write(&path, &bytes).unwrap();
        assert!(read_tags(&path, &info_of(&bytes)).is_none());
    }

    #[test]
    fn reads_tags_from_a_dff_id3_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = make_dff(2, 2_822_400, &vec![0x69u8; 2 * 4096]);
        let id3 = tag(
            4,
            0,
            &[
                frame(4, "TIT2", &text(3, "Dff Title")),
                frame(4, "TPE1", &text(3, "Dff Artist")),
            ],
            0,
        );
        bytes.extend_from_slice(b"ID3 ");
        bytes.extend_from_slice(&(id3.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&id3);
        if id3.len() % 2 == 1 {
            bytes.push(0);
        }
        let form_size = (bytes.len() - 12) as u64;
        bytes[4..12].copy_from_slice(&form_size.to_be_bytes());
        let path = dir.path().join("a.dff");
        std::fs::write(&path, &bytes).unwrap();
        let t = read_tags(&path, &info_of(&bytes)).unwrap();
        assert_eq!(t.title.as_deref(), Some("Dff Title"));
        assert_eq!(t.artist.as_deref(), Some("Dff Artist"));
    }
}

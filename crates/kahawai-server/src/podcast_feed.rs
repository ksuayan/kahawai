//! Reading podcast feeds (docs/v2/kahawai-podcast-spec.md, D1).
//!
//! Real feeds are messy, and RSS has no schema to validate against, so this is
//! deliberately lenient and defensive: whatever can be read is read, and what
//! cannot is reported as a warning, never as a reason to drop the feed. The XML
//! is cleaned first (byte order mark, junk before the first tag, non-UTF-8
//! encodings, illegal control characters, bare `&`, HTML entities such as
//! `&nbsp;`, a feed cut off mid-item), then `feed-rs` reads it, and what comes
//! out is checked item by item.
//!
//! No network here: this takes bytes and returns a [`ParsedFeed`].

use std::collections::HashSet;

use serde::Serialize;

/// Most episodes read from one feed. Some feeds carry thousands.
pub const MAX_EPISODES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct ParsedEpisode {
    /// The feed's own `<guid>`, or the enclosure address when there is none.
    pub guid: String,
    pub title: String,
    /// The notes as the feed has them (HTML); sanitized when shown.
    pub description_html: Option<String>,
    /// Unix milliseconds.
    pub published_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub enclosure_url: String,
    pub enclosure_type: Option<String>,
    pub enclosure_bytes: Option<i64>,
    pub image_url: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct ParsedFeed {
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub link: Option<String>,
    pub image_url: Option<String>,
    pub language: Option<String>,
    pub explicit: bool,
    /// Newest first.
    pub episodes: Vec<ParsedEpisode>,
    /// Entries that carry video, not audio: left out (audio only in v1).
    pub video_skipped: usize,
    /// Things that were off but did not stop the feed from loading.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FeedError {
    /// A web page (or something else), not a feed. `feeds` lists the feed
    /// addresses the page points to, if any.
    NotAFeed { feeds: Vec<String> },
    /// It looked like a feed but could not be read.
    Unreadable(String),
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedError::NotAFeed { feeds } if feeds.is_empty() => {
                f.write_str("that address is not a podcast feed")
            }
            FeedError::NotAFeed { feeds } => write!(
                f,
                "that address is a web page, not a feed; it points to {}",
                feeds.join(", ")
            ),
            FeedError::Unreadable(why) => write!(f, "the feed could not be read: {why}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Addresses
// ---------------------------------------------------------------------------

/// A feed address in one canonical form, so the same feed pasted two ways is
/// one subscription: `feed:`/`itpc:`/`pcast:` become http(s), the scheme and
/// host are lowercase, the default port, the fragment and a lone trailing `/`
/// on a bare host go. Credentials in the address (member feeds) are kept.
pub fn normalize_feed_url(raw: &str) -> Result<String, String> {
    let mut s = raw.trim().to_string();
    for (prefix, to) in [
        ("feed:https://", "https://"),
        ("feed:http://", "http://"),
        ("feed://", "https://"),
        ("itpc://", "https://"),
        ("pcast://", "https://"),
        ("podcast://", "https://"),
    ] {
        if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
            s = format!("{to}{}", &s[prefix.len()..]);
            break;
        }
    }
    if !s.contains("://") {
        s = format!("https://{s}");
    }
    let mut u = reqwest::Url::parse(&s).map_err(|_| "that is not a web address".to_string())?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("a feed address starts with http:// or https://".into());
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err("there is no host in that address".into());
    }
    u.set_fragment(None);
    let mut out: String = u.into();
    // `Url` already lowercases scheme and host and drops default ports; a bare
    // host gets a `/` path, which is dropped again so `https://a.com` and
    // `https://a.com/` agree.
    if out.matches('/').count() == 3 && out.ends_with('/') && !out.contains('?') {
        out.pop();
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Cleaning the XML
// ---------------------------------------------------------------------------

const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

/// The text of the bytes, whatever they are encoded in: UTF-16 with a byte
/// order mark, UTF-8, or else Windows-1252 (what feeds that say `ISO-8859-1`
/// or `windows-1252` nearly always are).
pub fn decode_to_utf8(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let le = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => char::from_u32(u32::from(CP1252_HIGH[(b - 0x80) as usize]))
                    .unwrap_or('\u{FFFD}'),
                _ => b as char,
            })
            .collect(),
    }
}

/// HTML entities that appear in feeds but do not exist in XML.
const HTML_ENTITIES: [(&str, u32); 28] = [
    ("nbsp", 160),
    ("copy", 169),
    ("reg", 174),
    ("trade", 8482),
    ("hellip", 8230),
    ("mdash", 8212),
    ("ndash", 8211),
    ("lsquo", 8216),
    ("rsquo", 8217),
    ("ldquo", 8220),
    ("rdquo", 8221),
    ("bull", 8226),
    ("middot", 183),
    ("euro", 8364),
    ("pound", 163),
    ("yen", 165),
    ("cent", 162),
    ("deg", 176),
    ("eacute", 233),
    ("egrave", 232),
    ("agrave", 224),
    ("aacute", 225),
    ("uuml", 252),
    ("ouml", 246),
    ("auml", 228),
    ("ntilde", 241),
    ("ccedil", 231),
    ("iexcl", 161),
];

fn illegal_xml_char(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}')
}

/// Repair one run of text outside CDATA: bare `&` becomes `&amp;`, HTML-only
/// entities become numeric ones.
fn fix_ampersands(text: &str, out: &mut String) {
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        // An entity is `&name;` or `&#123;` / `&#x1F;`, short.
        let end = after
            .char_indices()
            .take(12)
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '#'));
        match end {
            Some((j, ';')) if j > 0 => {
                let name = &after[..j];
                let xml_ok = matches!(name, "amp" | "lt" | "gt" | "quot" | "apos")
                    || (name.starts_with('#') && name.len() > 1);
                if xml_ok {
                    out.push('&');
                    out.push_str(&after[..=j]);
                } else if let Some((_, cp)) = HTML_ENTITIES.iter().find(|(n, _)| *n == name) {
                    out.push_str(&format!("&#{cp};"));
                } else {
                    out.push_str("&amp;");
                    out.push_str(&after[..=j]);
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push_str("&amp;");
                rest = after;
            }
        }
    }
    out.push_str(rest);
}

/// Clean XML text so a strict parser accepts it.
pub fn sanitize_xml(text: &str) -> String {
    // Anything before the first tag (whitespace, a stray byte, "<!-- -->" is
    // left alone) goes.
    let start = text.find('<').unwrap_or(0);
    let text: String = text[start..]
        .chars()
        .filter(|c| !illegal_xml_char(*c))
        .collect();
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text.as_str();
    while let Some(i) = rest.find("<![CDATA[") {
        fix_ampersands(&rest[..i], &mut out);
        let body = &rest[i..];
        match body.find("]]>") {
            Some(j) => {
                out.push_str(&body[..j + 3]);
                rest = &body[j + 3..];
            }
            None => {
                // An unclosed CDATA runs to the end; close it.
                out.push_str(body);
                out.push_str("]]>");
                rest = "";
            }
        }
    }
    fix_ampersands(rest, &mut out);
    // The declared encoding no longer applies: the text is UTF-8 now.
    if out.starts_with("<?xml") {
        if let Some(end) = out.find("?>") {
            let decl = &out[..end];
            if let Some(e) = decl.find("encoding") {
                let tail = &decl[e..];
                if let Some(q) = tail.find(['"', '\'']) {
                    let quote = tail.as_bytes()[q] as char;
                    if let Some(q2) = tail[q + 1..].find(quote) {
                        let from = e + q + 1;
                        let to = from + q2;
                        out.replace_range(from..to, "UTF-8");
                    }
                }
            }
        }
    }
    out
}

/// A feed cut off mid-way (a dropped download): keep everything up to the last
/// complete item and close the document.
fn repair_truncated(text: &str) -> Option<String> {
    let cut = text
        .rfind("</item>")
        .map(|i| i + "</item>".len())
        .or_else(|| text.rfind("</entry>").map(|i| i + "</entry>".len()))?;
    let mut out = text[..cut].to_string();
    if text.contains("<rss") {
        out.push_str("</channel></rss>");
    } else {
        out.push_str("</feed>");
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Discovering a feed from a web page
// ---------------------------------------------------------------------------

/// The feed addresses a web page advertises with
/// `<link rel="alternate" type="application/rss+xml" href="...">`.
pub fn discover_feed_links(html: &str, base: &str) -> Vec<String> {
    let base = reqwest::Url::parse(base).ok();
    let lower = html.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<link") {
        let start = at + i;
        let Some(len) = lower[start..].find('>') else {
            break;
        };
        let tag = &html[start..start + len];
        let tag_lower = &lower[start..start + len];
        at = start + len;
        let is_feed =
            tag_lower.contains("rel=\"alternate\"") || tag_lower.contains("rel='alternate'");
        let typed = tag_lower.contains("rss+xml") || tag_lower.contains("atom+xml");
        if !(is_feed && typed) {
            continue;
        }
        if let Some(href) = attr(tag, "href") {
            let abs = match &base {
                Some(b) => b.join(&href).map(|u| u.to_string()).unwrap_or(href),
                None => href,
            };
            if !found.contains(&abs) {
                found.push(abs);
            }
        }
    }
    found
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let i = lower.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[i..];
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let end = rest[1..].find(quote)?;
    Some(rest[1..1 + end].replace("&amp;", "&"))
}

fn looks_like_html(text: &str) -> bool {
    let head: String = text
        .chars()
        .take(2000)
        .collect::<String>()
        .to_ascii_lowercase();
    head.contains("<!doctype html") || head.contains("<html") || head.contains("<head")
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn text_of(t: &Option<feed_rs::model::Text>) -> Option<String> {
    t.as_ref()
        .map(|t| t.content.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn audio_type(mime: &Option<String>, url: &str) -> Option<bool> {
    // Some(true) = audio, Some(false) = video, None = unknown (assume audio).
    if let Some(m) = mime {
        let m = m.to_ascii_lowercase();
        if m.starts_with("video/") {
            return Some(false);
        }
        if m.starts_with("audio/") || m.contains("mpeg") || m.contains("ogg") || m.contains("mp4a")
        {
            return Some(true);
        }
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if [".mp4", ".m4v", ".mov", ".webm", ".mkv"]
        .iter()
        .any(|e| path.ends_with(e))
    {
        return Some(false);
    }
    None
}

/// A duration as feeds write it: seconds (`3723`, `3723.5`), `MM:SS` or
/// `HH:MM:SS`. Anything else, or zero, is `None`.
pub fn parse_duration_ms(text: &str) -> Option<i64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let mut total = 0f64;
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    for p in &parts {
        let v: f64 = p
            .trim()
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
        total = total * 60.0 + v;
    }
    let ms = (total * 1000.0).round() as i64;
    (ms > 0).then_some(ms)
}

/// The text of the first `<tag>...</tag>` in `block` (CDATA unwrapped).
fn tag_text<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let open = block.find(&format!("<{tag}"))?;
    let rest = &block[open..];
    let start = rest.find('>')? + 1;
    let end = rest[start..].find(&format!("</{tag}>"))?;
    let inner = rest[start..start + end].trim();
    Some(
        inner
            .strip_prefix("<![CDATA[")
            .and_then(|s| s.strip_suffix("]]>"))
            .unwrap_or(inner)
            .trim(),
    )
}

/// The `<item>...</item>` blocks of an RSS document, in order.
fn item_blocks(xml: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = xml[at..].find("<item") {
        let start = at + i;
        let after = xml.as_bytes().get(start + 5).copied();
        if !matches!(
            after,
            Some(b'>') | Some(b' ') | Some(b'\n') | Some(b'\r') | Some(b'\t')
        ) {
            at = start + 5;
            continue;
        }
        let end = xml[start..]
            .find("</item>")
            .map(|e| start + e + 7)
            .unwrap_or(xml.len());
        out.push(&xml[start..end]);
        at = end;
    }
    out
}

/// Read a feed from its bytes. `feed_url` is where it came from (relative
/// addresses inside it are resolved against it).
pub fn parse_feed(bytes: &[u8], feed_url: &str) -> Result<ParsedFeed, FeedError> {
    let decoded = decode_to_utf8(bytes);
    if looks_like_html(&decoded) && !decoded.contains("<rss") && !decoded.contains("<feed") {
        return Err(FeedError::NotAFeed {
            feeds: discover_feed_links(&decoded, feed_url),
        });
    }
    let mut warnings = Vec::new();
    let cleaned = sanitize_xml(&decoded);
    let base = reqwest::Url::parse(feed_url).ok();
    let parse = |xml: &str| {
        let mut b = feed_rs::parser::Builder::new();
        if let Some(u) = &base {
            b = b.base_uri(Some(u.as_str()));
        }
        b.build().parse(xml.as_bytes())
    };
    let feed = match parse(&cleaned) {
        Ok(f) => f,
        Err(first) => match repair_truncated(&cleaned).map(|r| parse(&r)) {
            Some(Ok(f)) => {
                warnings.push("the feed was cut off; the last episodes may be missing".into());
                f
            }
            _ => {
                return Err(if cleaned.contains("<rss") || cleaned.contains("<feed") {
                    FeedError::Unreadable(first.to_string())
                } else {
                    FeedError::NotAFeed { feeds: Vec::new() }
                })
            }
        },
    };

    let mut out = ParsedFeed {
        title: text_of(&feed.title).unwrap_or_default(),
        author: feed.authors.iter().find_map(|a| {
            let n = a.name.as_deref()?.trim();
            (!n.is_empty()).then(|| n.to_string())
        }),
        description: text_of(&feed.description),
        link: feed
            .links
            .iter()
            .find(|l| l.rel.as_deref() != Some("self"))
            .map(|l| l.href.clone()),
        image_url: feed
            .logo
            .as_ref()
            .map(|i| i.uri.clone())
            .or_else(|| feed.icon.as_ref().map(|i| i.uri.clone())),
        language: feed.language.clone().filter(|l| !l.trim().is_empty()),
        explicit: feed.rating.is_some(),
        episodes: Vec::new(),
        video_skipped: 0,
        warnings,
    };
    if out.title.is_empty() {
        out.title = base
            .as_ref()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "Untitled podcast".into());
        out.warnings.push("the feed has no title".into());
    }

    // `feed-rs` does not read `<itunes:duration>` reliably, so it is read here,
    // item by item (the entries come in document order).
    let blocks = item_blocks(&cleaned);
    let aligned = blocks.len() == feed.entries.len();
    let mut seen: HashSet<String> = HashSet::new();
    let (mut no_audio, mut dup) = (0usize, 0usize);
    for (idx, e) in feed.entries.iter().enumerate().take(MAX_EPISODES) {
        // The enclosure: the audio file. RSS `<enclosure>` arrives as media
        // content; Atom as a link with rel="enclosure".
        let mut enclosure: Option<(String, Option<String>, Option<i64>)> = None;
        for m in &e.media {
            for c in &m.content {
                if let Some(u) = &c.url {
                    enclosure = Some((
                        u.to_string(),
                        c.content_type.as_ref().map(|t| t.to_string()),
                        c.size.map(|s| s as i64),
                    ));
                    break;
                }
            }
            if enclosure.is_some() {
                break;
            }
        }
        if enclosure.is_none() {
            if let Some(l) = e
                .links
                .iter()
                .find(|l| l.rel.as_deref() == Some("enclosure"))
            {
                enclosure = Some((
                    l.href.clone(),
                    l.media_type.clone(),
                    l.length.map(|n| n as i64),
                ));
            }
        }
        let Some((url, mime, bytes)) = enclosure else {
            no_audio += 1;
            continue;
        };
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            no_audio += 1;
            continue;
        }
        if audio_type(&mime, &url) == Some(false) {
            out.video_skipped += 1;
            continue;
        }
        // feed-rs invents an id (a hash or a UUID) when the entry has no
        // `<guid>`; an invented one is not in the text, and would change on
        // every refresh. Use the enclosure address then.
        let guid = if !e.id.is_empty() && decoded.contains(&e.id) {
            e.id.clone()
        } else {
            url.clone()
        };
        if !seen.insert(guid.clone()) {
            dup += 1;
            continue;
        }
        let media = e.media.first();
        out.episodes.push(ParsedEpisode {
            guid,
            title: text_of(&e.title).unwrap_or_else(|| "Untitled episode".into()),
            description_html: e
                .content
                .as_ref()
                .and_then(|c| c.body.clone())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| text_of(&e.summary))
                .or_else(|| media.and_then(|m| text_of(&m.description))),
            published_ms: e.published.or(e.updated).map(|d| d.timestamp_millis()),
            duration_ms: media
                .and_then(|m| m.duration)
                .map(|d| d.as_millis() as i64)
                .filter(|d| *d > 0)
                .or_else(|| {
                    let block = if aligned {
                        blocks.get(idx).copied()
                    } else {
                        blocks.iter().copied().find(|b| b.contains(&url))
                    }?;
                    parse_duration_ms(tag_text(block, "itunes:duration")?)
                }),
            enclosure_url: url,
            enclosure_type: mime,
            enclosure_bytes: bytes.filter(|b| *b > 0),
            image_url: media.and_then(|m| m.thumbnails.first().map(|t| t.image.uri.clone())),
            season: media.and_then(|m| m.season.as_ref().map(|s| s.number)),
            episode: media.and_then(|m| m.episode.as_ref().map(|s| s.number as u32)),
            link: e
                .links
                .iter()
                .find(|l| l.rel.as_deref() != Some("enclosure"))
                .map(|l| l.href.clone()),
        });
    }
    if feed.entries.len() > MAX_EPISODES {
        out.warnings
            .push(format!("only the newest {MAX_EPISODES} episodes were read"));
    }
    if no_audio > 0 {
        out.warnings.push(format!(
            "{no_audio} entries have no audio file and were left out"
        ));
    }
    if dup > 0 {
        out.warnings.push(format!(
            "{dup} entries repeated an earlier episode's id and were left out"
        ));
    }
    // Newest first, whatever order the feed uses; undated ones keep feed order.
    out.episodes
        .sort_by(|a, b| b.published_ms.cmp(&a.published_ms));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM: &str = r#"<item><title>Ep</title><guid>g1</guid><pubDate>Mon, 02 Mar 2026 10:00:00 +0000</pubDate>
        <enclosure url="https://cdn.example/ep1.mp3" length="1234" type="audio/mpeg"/><itunes:duration>1:02:03</itunes:duration></item>"#;

    fn rss(channel_extra: &str, items: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd" xmlns:content="http://purl.org/rss/1.0/modules/content/">
<channel><title>Show</title><link>https://show.example</link><description>About</description>
<language>en</language><itunes:author>Host</itunes:author><itunes:explicit>true</itunes:explicit>
<itunes:image href="https://show.example/art.jpg"/>{channel_extra}{items}</channel></rss>"#
        )
    }

    #[test]
    fn a_clean_feed_is_read_in_full() {
        let f = parse_feed(rss("", ITEM).as_bytes(), "https://show.example/feed.xml").unwrap();
        assert_eq!(f.title, "Show");
        assert_eq!(f.author.as_deref(), Some("Host"));
        assert_eq!(f.language.as_deref(), Some("en"));
        assert!(f.explicit);
        assert_eq!(f.image_url.as_deref(), Some("https://show.example/art.jpg"));
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        let e = &f.episodes[0];
        assert_eq!(e.guid, "g1");
        assert_eq!(e.title, "Ep");
        assert_eq!(e.enclosure_url, "https://cdn.example/ep1.mp3");
        assert_eq!(e.enclosure_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(e.enclosure_bytes, Some(1234));
        assert_eq!(e.duration_ms, Some(3_723_000));
        assert_eq!(e.published_ms, Some(1_772_445_600_000));
    }

    #[test]
    fn a_byte_order_mark_and_junk_before_the_xml_are_ignored() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"\n\n  garbage \r\n");
        bytes.extend_from_slice(rss("", ITEM).as_bytes());
        let f = parse_feed(&bytes, "https://show.example/f").unwrap();
        assert_eq!(f.episodes.len(), 1);
    }

    #[test]
    fn windows_1252_and_utf_16_feeds_are_decoded() {
        // "Café – “quoted”" in Windows-1252, declared as ISO-8859-1 as feeds do.
        let mut bytes =
            br#"<?xml version="1.0" encoding="ISO-8859-1"?><rss version="2.0"><channel><title>Caf"#
                .to_vec();
        bytes.extend_from_slice(&[0xE9, b' ', 0x96, b' ', 0x93]);
        bytes.extend_from_slice(b"quoted");
        bytes.push(0x94);
        bytes.extend_from_slice(
            br#"</title><item><title>x</title><guid>a</guid><enclosure url="https://c/a.mp3" type="audio/mpeg"/></item></channel></rss>"#,
        );
        let f = parse_feed(&bytes, "https://c/f").unwrap();
        assert_eq!(f.title, "Café – “quoted”");

        let xml = rss("", ITEM);
        let mut utf16 = vec![0xFF, 0xFE];
        for u in xml.encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(parse_feed(&utf16, "https://c/f").unwrap().episodes.len(), 1);
    }

    #[test]
    fn bare_ampersands_html_entities_and_control_characters_do_not_break_it() {
        let items = "<item><title>Tom & Jerry&nbsp;&mdash; &copy; 2026 &unknown; &#169; &lt;b&gt;</title><guid>g</guid>\
            <description><![CDATA[Cats & dogs <b>bold</b> &nbsp;]]></description>\
            <enclosure url=\"https://c/a.mp3?x=1&y=2\" type=\"audio/mpeg\"/></item>";
        let mut xml = rss("", items);
        xml.insert(xml.find("<channel>").unwrap() + 9, '\u{1}');
        let f = parse_feed(xml.as_bytes(), "https://c/f").unwrap();
        let e = &f.episodes[0];
        assert_eq!(e.title, "Tom & Jerry\u{a0}— © 2026 &unknown; © <b>");
        assert_eq!(e.enclosure_url, "https://c/a.mp3?x=1&y=2");
        assert_eq!(
            e.description_html.as_deref(),
            Some("Cats & dogs <b>bold</b> &nbsp;"),
            "CDATA is left exactly as written"
        );
    }

    #[test]
    fn durations_come_in_several_forms() {
        let item = |d: &str| {
            format!(
                "<item><title>t{d}</title><guid>g{d}</guid><enclosure url=\"https://c/{d}.mp3\" type=\"audio/mpeg\"/>\
                 <itunes:duration>{d}</itunes:duration></item>"
            )
        };
        let items: String = ["3723", "62:03", "1:02:03", "00:45", "garbage", "0"]
            .iter()
            .map(|d| item(d))
            .collect();
        let f = parse_feed(rss("", &items).as_bytes(), "https://c/f").unwrap();
        let by = |g: &str| f.episodes.iter().find(|e| e.guid == g).unwrap().duration_ms;
        assert_eq!(by("g3723"), Some(3_723_000), "seconds");
        assert_eq!(by("g62:03"), Some(3_723_000), "minutes:seconds");
        assert_eq!(by("g1:02:03"), Some(3_723_000), "hours:minutes:seconds");
        assert_eq!(by("g00:45"), Some(45_000));
        assert_eq!(by("ggarbage"), None, "unreadable is none, not an error");
        assert_eq!(by("g0"), None);
    }

    #[test]
    fn an_episode_without_a_guid_is_keyed_by_its_audio_and_stays_put_between_reads() {
        let items = r#"<item><title>No id</title><enclosure url="https://c/noid.mp3" type="audio/mpeg"/></item>"#;
        let a = parse_feed(rss("", items).as_bytes(), "https://c/f").unwrap();
        let b = parse_feed(rss("", items).as_bytes(), "https://c/f").unwrap();
        assert_eq!(a.episodes[0].guid, "https://c/noid.mp3");
        assert_eq!(a.episodes[0].guid, b.episodes[0].guid);
    }

    #[test]
    fn video_and_audioless_entries_are_left_out_and_counted() {
        let items = format!(
            "{ITEM}<item><title>Video</title><guid>v</guid><enclosure url=\"https://c/v.mp4\" type=\"video/mp4\"/></item>\
             <item><title>Blog post</title><guid>b</guid></item>\
             <item><title>Bad url</title><guid>x</guid><enclosure url=\"ftp://c/x.mp3\" type=\"audio/mpeg\"/></item>"
        );
        let f = parse_feed(rss("", &items).as_bytes(), "https://c/f").unwrap();
        assert_eq!(f.episodes.len(), 1);
        assert_eq!(f.video_skipped, 1);
        assert!(
            f.warnings
                .iter()
                .any(|w| w.contains("2 entries have no audio")),
            "{:?}",
            f.warnings
        );
    }

    #[test]
    fn repeated_guids_keep_the_first_and_episodes_come_newest_first() {
        let it = |g: &str, d: &str| {
            format!(
                "<item><title>{g}</title><guid>{g}</guid><pubDate>{d}</pubDate><enclosure url=\"https://c/{g}-{d}.mp3\" type=\"audio/mpeg\"/></item>"
            )
        };
        let items = format!(
            "{}{}{}{}",
            it("a", "Mon, 02 Mar 2026 10:00:00 +0000"),
            it("b", "Wed, 04 Mar 2026 10:00:00 +0000"),
            it("a", "Tue, 03 Mar 2026 10:00:00 +0000"),
            it("c", "Tue, 03 Mar 2026 11:00:00 +0000")
        );
        let f = parse_feed(rss("", &items).as_bytes(), "https://c/f").unwrap();
        let order: Vec<&str> = f.episodes.iter().map(|e| e.guid.as_str()).collect();
        assert_eq!(order, ["b", "c", "a"]);
        assert!(f.warnings.iter().any(|w| w.contains("repeated")));
    }

    #[test]
    fn a_relative_enclosure_address_is_resolved_against_the_feed() {
        let items = r#"<item><title>t</title><guid>r</guid><enclosure url="/media/r.mp3" type="audio/mpeg"/></item>"#;
        let f = parse_feed(
            rss("", items).as_bytes(),
            "https://show.example/feeds/f.xml",
        )
        .unwrap();
        assert_eq!(
            f.episodes[0].enclosure_url,
            "https://show.example/media/r.mp3"
        );
    }

    #[test]
    fn a_feed_cut_off_mid_item_keeps_the_complete_episodes() {
        let full = rss(
            "",
            &format!("{ITEM}{}", ITEM.replace("g1", "g2").replace("ep1", "ep2")),
        );
        let cut = &full[..full.rfind("<enclosure").unwrap() + 12];
        let f = parse_feed(cut.as_bytes(), "https://c/f").unwrap();
        assert_eq!(f.episodes.len(), 1);
        assert!(f.warnings.iter().any(|w| w.contains("cut off")));
    }

    #[test]
    fn a_web_page_is_refused_and_its_feed_is_found() {
        let html = r#"<!DOCTYPE html><html><head><title>Show</title>
            <link rel="alternate" type="application/rss+xml" title="Feed" href="/feed.xml">
            <link rel='alternate' type='application/atom+xml' href='https://other.example/atom?a=1&amp;b=2'>
            <link rel="stylesheet" href="/x.css"></head><body>hi</body></html>"#;
        match parse_feed(html.as_bytes(), "https://show.example/") {
            Err(FeedError::NotAFeed { feeds }) => assert_eq!(
                feeds,
                [
                    "https://show.example/feed.xml",
                    "https://other.example/atom?a=1&b=2"
                ]
            ),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse_feed(b"just some text", "https://x/"),
            Err(FeedError::NotAFeed { .. })
        ));
        let msg = parse_feed(html.as_bytes(), "https://show.example/")
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("web page") && msg.contains("feed.xml"),
            "{msg}"
        );
    }

    #[test]
    fn an_atom_feed_with_enclosure_links_is_read() {
        let atom = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><title>Atom Show</title>
            <id>urn:x</id><updated>2026-03-01T00:00:00Z</updated>
            <entry><title>One</title><id>urn:e1</id><updated>2026-03-01T00:00:00Z</updated>
            <link rel="enclosure" type="audio/mpeg" href="https://c/one.mp3" length="99"/></entry></feed>"#;
        let f = parse_feed(atom.as_bytes(), "https://c/atom").unwrap();
        assert_eq!(f.title, "Atom Show");
        assert_eq!(f.episodes[0].enclosure_url, "https://c/one.mp3");
        assert_eq!(f.episodes[0].guid, "urn:e1");
    }

    #[test]
    fn an_untitled_feed_gets_its_host_and_a_warning() {
        let xml = r#"<rss version="2.0"><channel><item><title>t</title><guid>g</guid><enclosure url="https://c/a.mp3" type="audio/mpeg"/></item></channel></rss>"#;
        let f = parse_feed(xml.as_bytes(), "https://myshow.example/feed").unwrap();
        assert_eq!(f.title, "myshow.example");
        assert!(f.warnings.iter().any(|w| w.contains("no title")));
    }

    #[test]
    fn a_feed_with_no_episodes_is_still_a_feed() {
        let f = parse_feed(rss("", "").as_bytes(), "https://c/f").unwrap();
        assert!(f.episodes.is_empty());
    }

    #[test]
    fn broken_xml_that_is_clearly_a_feed_reports_unreadable() {
        let r = parse_feed(
            b"<rss version=\"2.0\"><channel><title>x</wrong>",
            "https://c/f",
        );
        assert!(matches!(r, Err(FeedError::Unreadable(_))), "{r:?}");
    }

    #[test]
    fn duration_text_is_understood() {
        assert_eq!(parse_duration_ms("3723"), Some(3_723_000));
        assert_eq!(parse_duration_ms(" 3723.5 "), Some(3_723_500));
        assert_eq!(parse_duration_ms("62:03"), Some(3_723_000));
        assert_eq!(parse_duration_ms("1:02:03"), Some(3_723_000));
        assert_eq!(parse_duration_ms("01:02:03.250"), Some(3_723_250));
        for bad in ["", "abc", "1:2:3:4", "-5", "0", "0:00", "1:xx"] {
            assert_eq!(parse_duration_ms(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn feed_addresses_have_one_form() {
        let n = |s: &str| normalize_feed_url(s).unwrap();
        assert_eq!(
            n(" HTTPS://Show.Example:443/Feed.xml#top "),
            "https://show.example/Feed.xml"
        );
        assert_eq!(n("feed://show.example/rss"), "https://show.example/rss");
        assert_eq!(n("itpc://show.example/rss"), "https://show.example/rss");
        assert_eq!(
            n("feed:https://show.example/rss"),
            "https://show.example/rss"
        );
        assert_eq!(n("show.example/rss"), "https://show.example/rss");
        assert_eq!(n("https://show.example/"), n("https://show.example"));
        assert_eq!(
            n("https://user:pass@members.example/feed?token=abc"),
            "https://user:pass@members.example/feed?token=abc",
            "member feeds keep their credentials"
        );
        assert!(normalize_feed_url("ftp://x/y").is_err());
        assert!(normalize_feed_url("").is_err());
    }
}

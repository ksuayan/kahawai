//! Internet radio: the station directory and stream probe
//! (docs/v2/kahawai-radio-spec.md, D1 and D2).
//!
//! The directory is radio-browser.info, a community database with a free API
//! and no key. Their servers come and go, so a request names a mirror found
//! from `all.api.radio-browser.info/json/servers` and moves to the next one
//! when it fails. Answers are kept for a day, and a stale answer beats no
//! answer when the network is down. The service is used as it asks to be: a
//! real User-Agent, and a call to `/json/url/{uuid}` when a station is played
//! (that is what counts a listen).
//!
//! The players connect to the streams themselves; the server only stores the
//! favorites and proxies this directory. Because the directory is online it
//! sits behind `online_sources_enabled` (off by default).

use std::time::Duration;

use kahawai_core::MusicError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{sqlite::SqlitePool, Row};

use crate::db::cvt;
use crate::musicbrainz::USER_AGENT;

/// Where the list of mirrors comes from.
pub const MIRROR_LIST_URL: &str = "https://all.api.radio-browser.info/json/servers";
/// Used when even the mirror list can't be fetched.
const FALLBACK_MIRRORS: [&str; 3] = [
    "https://de1.api.radio-browser.info",
    "https://nl1.api.radio-browser.info",
    "https://at1.api.radio-browser.info",
];
/// How long a directory answer is trusted.
pub const CACHE_TTL_S: i64 = 24 * 3600;
/// Most stations one search may return.
pub const MAX_LIMIT: u32 = 200;

// ---------------------------------------------------------------------------
// Stations (pure)
// ---------------------------------------------------------------------------

/// A station as the directory describes it, trimmed to what the Player shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Station {
    pub station_uuid: String,
    pub name: String,
    /// The address the station publishes.
    pub url: String,
    /// The directory's resolved form (playlists followed), when it has one.
    pub url_resolved: Option<String>,
    pub homepage: Option<String>,
    pub favicon: Option<String>,
    /// Comma separated, as the directory has it.
    pub tags: Option<String>,
    pub country: Option<String>,
    pub language: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<i64>,
    /// How many people have clicked it: a rough popularity.
    pub clicks: i64,
    /// An HLS (`.m3u8`) station: not playable in v1.
    pub hls: bool,
    /// Needs the server to decode it first: HE-AAC (AAC+) is more than the
    /// Player's own decoder can do.
    pub needs_relay: bool,
}

/// Does this codec or content type name HE-AAC (AAC+)?
pub fn is_he_aac(codec: &str) -> bool {
    let c = codec.trim().to_ascii_lowercase();
    c.contains("aac+")
        || c.contains("aacp")
        || c.contains("aac plus")
        || c.contains("he-aac")
        || c.contains("heaac")
        || c.contains("mp4a.40.5")
        || c.contains("mp4a.40.29")
}

fn text(v: &Value, key: &str) -> Option<String> {
    v[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn number(v: &Value, key: &str) -> Option<i64> {
    // Directory fields drift between numbers and numeric strings.
    v[key]
        .as_i64()
        .or_else(|| v[key].as_f64().map(|f| f as i64))
        .or_else(|| v[key].as_str().and_then(|s| s.trim().parse().ok()))
}

fn flag(v: &Value, key: &str) -> bool {
    number(v, key).map(|n| n != 0).unwrap_or(false) || v[key].as_bool() == Some(true)
}

/// Stations from a `stations/search` (or similar) answer. An entry without a
/// name or an address is skipped; nothing else about an entry can fail it.
pub fn parse_stations(v: &Value) -> Vec<Station> {
    v.as_array()
        .map(|a| a.iter().filter_map(parse_station).collect())
        .unwrap_or_default()
}

fn parse_station(v: &Value) -> Option<Station> {
    let name = text(v, "name")?;
    let url = text(v, "url_resolved").or_else(|| text(v, "url"))?;
    let published = text(v, "url").unwrap_or_else(|| url.clone());
    let codec = text(v, "codec");
    let needs_relay = codec.as_deref().is_some_and(is_he_aac);
    Some(Station {
        station_uuid: text(v, "stationuuid").unwrap_or_default(),
        name,
        url: published,
        url_resolved: text(v, "url_resolved"),
        homepage: text(v, "homepage"),
        favicon: text(v, "favicon"),
        tags: text(v, "tags"),
        country: text(v, "country"),
        language: text(v, "language"),
        codec,
        bitrate: number(v, "bitrate").filter(|b| *b > 0),
        clicks: number(v, "clickcount").unwrap_or(0).max(0),
        hls: flag(v, "hls"),
        needs_relay,
    })
}

// ---------------------------------------------------------------------------
// Search parameters (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
    pub tag: Option<String>,
    pub country: Option<String>,
    pub language: Option<String>,
    /// `clickcount` (default), `votes`, `name`, `bitrate`.
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

fn clean(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Percent-encode a query value (RFC 3986 unreserved stay as they are).
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl SearchParams {
    /// The directory path and query for these parameters. Broken stations
    /// are always hidden. The same words always give the same string, so it
    /// doubles as the cache key.
    pub fn path(&self) -> String {
        let order = match clean(&self.order).as_deref() {
            Some(o @ ("votes" | "name" | "bitrate" | "clickcount")) => o.to_string(),
            _ => "clickcount".to_string(),
        };
        let mut q = vec![
            format!("order={order}"),
            format!("reverse={}", order != "name"),
            "hidebroken=true".to_string(),
            format!("limit={}", self.limit.unwrap_or(50).clamp(1, MAX_LIMIT)),
            format!("offset={}", self.offset.unwrap_or(0)),
        ];
        for (k, v) in [
            ("name", clean(&self.q)),
            ("tag", clean(&self.tag)),
            ("country", clean(&self.country)),
            ("language", clean(&self.language)),
        ] {
            if let Some(v) = v {
                // The directory matches case-insensitively; normalizing
                // makes "Jazz" and "jazz" one cache entry.
                q.push(format!("{k}={}", encode_component(&v.to_lowercase())));
            }
        }
        q.sort();
        format!("/json/stations/search?{}", q.join("&"))
    }
}

// ---------------------------------------------------------------------------
// The directory client
// ---------------------------------------------------------------------------

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn client() -> Result<reqwest::Client, MusicError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| MusicError::Http(e.to_string()))
}

/// Mirrors to try, best first. `forced` (config) wins; otherwise the public
/// list, in the order it came, with a built-in few when that can't be read.
pub async fn mirrors(http: &reqwest::Client, forced: Option<&str>) -> Vec<String> {
    if let Some(f) = forced.map(str::trim).filter(|f| !f.is_empty()) {
        return vec![f.trim_end_matches('/').to_string()];
    }
    let listed: Vec<String> = match http.get(MIRROR_LIST_URL).send().await {
        Ok(r) if r.status().is_success() => r
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| {
                v.as_array().map(|a| {
                    a.iter()
                        .filter_map(|m| m["name"].as_str())
                        .filter(|n| n.ends_with("radio-browser.info"))
                        .map(|n| format!("https://{n}"))
                        .collect()
                })
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if listed.is_empty() {
        FALLBACK_MIRRORS.iter().map(|m| m.to_string()).collect()
    } else {
        listed
    }
}

async fn cached(pool: &SqlitePool, key: &str) -> Result<Option<(String, i64)>, MusicError> {
    Ok(
        sqlx::query("SELECT body, fetched_at FROM radio_cache WHERE key = ?")
            .bind(key)
            .fetch_optional(pool)
            .await
            .map_err(cvt)?
            .map(|r| (r.get(0), r.get(1))),
    )
}

/// One directory GET as JSON: from the day's cache when it is fresh, else
/// from the first mirror that answers (cached), else the stale copy.
pub async fn directory_json(
    pool: &SqlitePool,
    forced: Option<&str>,
    path: &str,
    use_cache: bool,
) -> Result<Value, MusicError> {
    let known = if use_cache {
        cached(pool, path).await?
    } else {
        None
    };
    if let Some((body, at)) = &known {
        if now_s() - at < CACHE_TTL_S {
            if let Ok(v) = serde_json::from_str(body) {
                return Ok(v);
            }
        }
    }
    let http = client()?;
    let mut last = String::from("no radio directory could be reached");
    for base in mirrors(&http, forced).await {
        let url = format!("{base}{path}");
        match http.get(&url).send().await {
            Ok(r) if r.status().is_success() => match r.text().await {
                Ok(body) => match serde_json::from_str::<Value>(&body) {
                    Ok(v) => {
                        if use_cache {
                            let _ = sqlx::query(
                                "INSERT INTO radio_cache (key, body, fetched_at) VALUES (?, ?, ?)
                                 ON CONFLICT(key) DO UPDATE SET body = excluded.body,
                                                                fetched_at = excluded.fetched_at",
                            )
                            .bind(path)
                            .bind(&body)
                            .bind(now_s())
                            .execute(pool)
                            .await;
                        }
                        return Ok(v);
                    }
                    Err(e) => last = format!("{base}: unreadable answer ({e})"),
                },
                Err(e) => last = format!("{base}: {e}"),
            },
            Ok(r) => last = format!("{base}: HTTP {}", r.status()),
            Err(e) => last = format!("{base}: {e}"),
        }
    }
    // Nothing answered: an old answer is better than an error.
    if let Some((body, _)) = known {
        if let Ok(v) = serde_json::from_str(&body) {
            return Ok(v);
        }
    }
    Err(MusicError::Http(last))
}

/// `GET /json/url/{uuid}`: the address to play, which also counts the click.
/// Never cached (the click must be sent). Falls back to nothing: the caller
/// plays the saved address when this fails.
pub async fn resolve_station_url(
    forced: Option<&str>,
    uuid: &str,
) -> Result<Option<String>, MusicError> {
    if uuid.is_empty() || !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(MusicError::BadRequest("not a station id".into()));
    }
    let http = client()?;
    let mut last = String::new();
    for base in mirrors(&http, forced).await {
        match http.get(format!("{base}/json/url/{uuid}")).send().await {
            Ok(r) if r.status().is_success() => {
                let v: Value = r.json().await.unwrap_or(Value::Null);
                return Ok(text(&v, "url"));
            }
            Ok(r) => last = format!("{base}: HTTP {}", r.status()),
            Err(e) => last = format!("{base}: {e}"),
        }
    }
    Err(MusicError::Http(last))
}

/// A tag, country or language the pickers offer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Facet {
    pub name: String,
    pub stations: i64,
}

pub fn parse_facets(v: &Value) -> Vec<Facet> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some(Facet {
                        name: text(f, "name")?,
                        stations: number(f, "stationcount").unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Probing a stream a listener typed in
// ---------------------------------------------------------------------------

/// What a stream says about itself when asked.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Probe {
    pub name: Option<String>,
    pub genre: Option<String>,
    pub bitrate: Option<i64>,
    /// "MP3", "AAC", "AAC+", "OGG", "FLAC", "OPUS" when the content type says.
    pub codec: Option<String>,
    pub content_type: Option<String>,
    /// Bytes between metadata blocks; present when it has track titles.
    pub metaint: Option<i64>,
    /// A playlist (`.pls`, `.m3u`) or HLS manifest rather than audio.
    pub playlist: bool,
    pub needs_relay: bool,
}

/// The codec a content type names.
pub fn codec_of(content_type: &str) -> Option<&'static str> {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match ct.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg" => "MP3",
        "audio/aacp" => "AAC+",
        "audio/aac" | "audio/x-aac" | "audio/mp4" | "audio/x-m4a" => "AAC",
        "audio/ogg" | "application/ogg" | "audio/vorbis" => "OGG",
        "audio/opus" => "OPUS",
        "audio/flac" | "audio/x-flac" => "FLAC",
        _ => return None,
    })
}

/// A probe from the head of a response: the status line, then headers.
/// Shoutcast v1 answers `ICY 200 OK`, which is why this is not left to an
/// HTTP library. Returns the status code too.
pub fn parse_probe_head(head: &str) -> Option<(u16, Probe)> {
    let mut lines = head.lines();
    let status_line = lines.next()?;
    let mut parts = status_line.split_whitespace();
    let proto = parts.next()?;
    if !(proto.starts_with("HTTP/") || proto == "ICY") {
        return None;
    }
    let status: u16 = parts.next()?.parse().ok()?;
    let mut p = Probe::default();
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "icy-name" => p.name = Some(v.to_string()).filter(|s| !s.is_empty()),
            "icy-genre" => p.genre = Some(v.to_string()).filter(|s| !s.is_empty()),
            "icy-br" => {
                p.bitrate = v
                    .split(',')
                    .next()
                    .and_then(|b| b.trim().parse().ok())
                    .filter(|b| *b > 0)
            }
            "icy-metaint" => p.metaint = v.parse().ok().filter(|m| *m > 0),
            "content-type" => {
                p.content_type = Some(v.to_string());
                p.codec = codec_of(v).map(str::to_string);
                let lower = v.to_ascii_lowercase();
                p.playlist = lower.contains("mpegurl")
                    || lower.contains("x-scpls")
                    || lower.contains("pls+xml");
            }
            _ => {}
        }
    }
    p.needs_relay = p.codec.as_deref().is_some_and(is_he_aac)
        || p.content_type.as_deref().is_some_and(is_he_aac);
    Some((status, p))
}

/// Where a redirect points, relative to `base`.
fn redirect_target(head: &str, base: &str) -> Option<String> {
    let loc = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("location").then(|| v.trim())
    })?;
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return Some(loc.to_string());
    }
    let u = reqwest::Url::parse(base).ok()?;
    u.join(loc).ok().map(|u| u.to_string())
}

/// Ask a stream what it is: connect, say we want metadata, read the reply's
/// head and hang up. Plain `http://` goes over a raw socket (Shoutcast v1's
/// `ICY 200 OK` is not valid HTTP); `https://` goes through reqwest.
pub async fn probe(url: &str) -> Result<Probe, MusicError> {
    let mut target = url.trim().to_string();
    for _ in 0..4 {
        let u = reqwest::Url::parse(&target)
            .map_err(|_| MusicError::BadRequest("that is not a web address".into()))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(MusicError::BadRequest(
                "only http:// and https:// streams can be played".into(),
            ));
        }
        let (status, probe, head) = if u.scheme() == "http" {
            raw_probe(&u).await?
        } else {
            https_probe(&target).await?
        };
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            target = redirect_target(&head, &target)
                .ok_or_else(|| MusicError::BadRequest("the stream redirects nowhere".into()))?;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(MusicError::BadRequest(format!(
                "the stream answered HTTP {status}"
            )));
        }
        return Ok(probe);
    }
    Err(MusicError::BadRequest("too many redirects".into()))
}

async fn raw_probe(u: &reqwest::Url) -> Result<(u16, Probe, String), MusicError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let host = u
        .host_str()
        .ok_or_else(|| MusicError::BadRequest("no host in that address".into()))?;
    let port = u.port_or_known_default().unwrap_or(80);
    let work = async {
        let mut s = tokio::net::TcpStream::connect((host, port)).await?;
        let mut path = u.path().to_string();
        if let Some(q) = u.query() {
            path.push('?');
            path.push_str(q);
        }
        let req = format!(
            "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: {USER_AGENT}\r\n\
             Icy-MetaData: 1\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes()).await?;
        let mut buf = Vec::with_capacity(2048);
        let mut chunk = [0u8; 1024];
        while buf.len() < 16 * 1024 {
            let n = s.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        Ok::<_, std::io::Error>(buf)
    };
    let buf = tokio::time::timeout(Duration::from_secs(10), work)
        .await
        .map_err(|_| MusicError::Http("the stream did not answer in time".into()))?
        .map_err(|e| MusicError::Http(format!("could not connect: {e}")))?;
    let head_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let (status, probe) = parse_probe_head(&head)
        .ok_or_else(|| MusicError::BadRequest("that is not a stream".into()))?;
    Ok((status, probe, head))
}

async fn https_probe(url: &str) -> Result<(u16, Probe, String), MusicError> {
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| MusicError::Http(e.to_string()))?;
    let res = http
        .get(url)
        .header("Icy-MetaData", "1")
        .send()
        .await
        .map_err(|e| MusicError::Http(format!("could not connect: {e}")))?;
    let mut head = format!("HTTP/1.1 {}\r\n", res.status().as_u16());
    for (k, v) in res.headers() {
        if let Ok(v) = v.to_str() {
            head.push_str(&format!("{}: {v}\r\n", k.as_str()));
        }
    }
    let (status, probe) = parse_probe_head(&head)
        .ok_or_else(|| MusicError::BadRequest("that is not a stream".into()))?;
    Ok((status, probe, head))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stations_are_read_leniently() {
        let v = json!([
            {"stationuuid": "960e57c5-0601-11e8-ae97-52543be04c81", "name": " Jazz FM ",
             "url": "http://x/pls", "url_resolved": "http://x/live.mp3",
             "tags": "jazz,smooth", "country": "UK", "codec": "MP3",
             "bitrate": "128", "clickcount": 42, "hls": 0, "favicon": ""},
            {"stationuuid": "b", "name": "Aac plus", "url": "http://y", "codec": "AAC+",
             "bitrate": 0, "hls": 1},
            {"name": "", "url": "http://nameless"},
            {"name": "No address"},
            "garbage"
        ]);
        let s = parse_stations(&v);
        assert_eq!(s.len(), 2, "the two unusable entries are skipped");
        assert_eq!(s[0].name, "Jazz FM");
        assert_eq!(s[0].url, "http://x/pls");
        assert_eq!(s[0].url_resolved.as_deref(), Some("http://x/live.mp3"));
        assert_eq!(s[0].bitrate, Some(128), "a numeric string is a number");
        assert_eq!(s[0].favicon, None, "blank is absent");
        assert!(!s[0].needs_relay && !s[0].hls);
        assert!(s[1].needs_relay, "AAC+ needs the server");
        assert!(s[1].hls);
        assert_eq!(s[1].bitrate, None);
        assert!(parse_stations(&json!({"not": "a list"})).is_empty());
    }

    #[test]
    fn he_aac_is_told_from_plain_aac() {
        for yes in ["AAC+", "aacp", "audio/aacp", "HE-AAC", "mp4a.40.5"] {
            assert!(is_he_aac(yes), "{yes}");
        }
        for no in ["AAC", "MP3", "audio/aac", "OGG", "mp4a.40.2", ""] {
            assert!(!is_he_aac(no), "{no}");
        }
    }

    #[test]
    fn a_search_is_one_cache_key_however_it_is_spelled() {
        let a = SearchParams {
            q: Some("  Jazz  FM ".into()),
            country: Some("France".into()),
            ..Default::default()
        };
        let b = SearchParams {
            country: Some("france".into()),
            q: Some("jazz  fm".into()),
            ..Default::default()
        };
        assert_eq!(a.path(), b.path());
        assert!(a.path().contains("name=jazz%20%20fm"));
        assert!(a.path().contains("hidebroken=true"));
        let c = SearchParams {
            limit: Some(99_999),
            order: Some("; drop table".into()),
            ..Default::default()
        };
        assert!(c.path().contains("limit=200"));
        assert!(
            c.path().contains("order=clickcount"),
            "unknown order falls back"
        );
        assert!(!c.path().contains("drop"));
        assert!(SearchParams {
            order: Some("name".into()),
            ..Default::default()
        }
        .path()
        .contains("reverse=false"));
    }

    #[test]
    fn a_shoutcast_v1_reply_is_read() {
        let head = "ICY 200 OK\r\nicy-name: Radio Paradise\r\nicy-br: 128\r\n\
                    icy-metaint: 16000\r\ncontent-type: audio/mpeg\r\nicy-genre: Eclectic";
        let (status, p) = parse_probe_head(head).unwrap();
        assert_eq!(status, 200);
        assert_eq!(p.name.as_deref(), Some("Radio Paradise"));
        assert_eq!(p.bitrate, Some(128));
        assert_eq!(p.metaint, Some(16000));
        assert_eq!(p.codec.as_deref(), Some("MP3"));
        assert_eq!(p.genre.as_deref(), Some("Eclectic"));
        assert!(!p.needs_relay && !p.playlist);
    }

    #[test]
    fn a_plain_http_reply_and_the_odd_cases_are_read() {
        let (s, p) =
            parse_probe_head("HTTP/1.1 200 OK\r\nContent-Type: audio/aacp\r\nicy-br: 64, 64")
                .unwrap();
        assert_eq!(s, 200);
        assert_eq!(p.codec.as_deref(), Some("AAC+"));
        assert!(p.needs_relay);
        assert_eq!(p.bitrate, Some(64), "the first of a list of bitrates");
        let (_, pl) = parse_probe_head("HTTP/1.0 200 OK\r\nContent-Type: audio/x-mpegurl").unwrap();
        assert!(pl.playlist);
        assert!(parse_probe_head("<html>nope").is_none());
        assert!(parse_probe_head("").is_none());
        assert_eq!(codec_of("audio/mpeg; charset=x"), Some("MP3"));
        assert_eq!(codec_of("text/html"), None);
    }

    #[test]
    fn redirects_may_be_relative() {
        let head = "HTTP/1.1 302 Found\r\nLocation: /live/stream.mp3";
        assert_eq!(
            redirect_target(head, "http://host:8000/old").as_deref(),
            Some("http://host:8000/live/stream.mp3")
        );
        let abs = "HTTP/1.1 301 Moved\r\nlocation: https://other/x";
        assert_eq!(
            redirect_target(abs, "http://a/b").as_deref(),
            Some("https://other/x")
        );
    }

    #[test]
    fn facets_are_read() {
        let f = parse_facets(&json!([
            {"name": "jazz", "stationcount": 1200},
            {"name": "", "stationcount": 3},
            {"name": "rock", "stationcount": "88"}
        ]));
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].stations, 88);
    }
}

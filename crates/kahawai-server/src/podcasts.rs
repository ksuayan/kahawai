//! Podcast subscriptions: fetching feeds, keeping their episodes, OPML
//! (docs/v2/kahawai-podcast-spec.md, D1 and D2).
//!
//! The server fetches the feeds the listener subscribes to. That is the
//! listener's own request, so it is not behind the online-sources opt-in
//! (which is for searching a directory with words you typed).

use std::time::Duration;

use kahawai_core::MusicError;
use serde::Serialize;
use sqlx::{sqlite::SqlitePool, Row};

use crate::db::cvt;
use crate::musicbrainz::USER_AGENT;
use crate::podcast_feed::{self, FeedError, ParsedFeed};

/// A feed bigger than this is refused (they are text; the biggest real ones
/// are a few MB).
pub const MAX_FEED_BYTES: usize = 25 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

pub fn now_ms() -> i64 {
    crate::audiobooks::now_ms()
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// What a fetch got.
pub enum Fetched {
    /// New bytes, and the validators to ask with next time.
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    /// "Not modified": nothing new since the validators given.
    NotModified,
}

fn client() -> Result<reqwest::Client, MusicError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| MusicError::Http(e.to_string()))
}

/// Fetch a feed. Credentials in the address (`https://user:pass@host/...`, the
/// usual way member feeds work) are sent as Basic auth.
pub async fn fetch(
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Fetched, MusicError> {
    let mut u = reqwest::Url::parse(url)
        .map_err(|_| MusicError::BadRequest("that is not a web address".into()))?;
    let auth = (!u.username().is_empty())
        .then(|| (u.username().to_string(), u.password().map(str::to_string)));
    let _ = u.set_username("");
    let _ = u.set_password(None);
    let mut req = client()?.get(u).header(
        "Accept",
        "application/rss+xml, application/atom+xml, application/xml, text/xml;q=0.9, */*;q=0.5",
    );
    if let Some((user, pass)) = auth {
        req = req.basic_auth(percent_decode(&user), pass.map(|p| percent_decode(&p)));
    }
    if let Some(e) = etag {
        req = req.header("If-None-Match", e);
    }
    if let Some(m) = last_modified {
        req = req.header("If-Modified-Since", m);
    }
    let mut res = req
        .send()
        .await
        .map_err(|e| MusicError::Http(format!("could not reach the feed: {e}")))?;
    let status = res.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if !status.is_success() {
        return Err(MusicError::Http(match status.as_u16() {
            401 | 403 => format!("the feed wants a login or refused access (HTTP {status}); member feeds put it in the address"),
            404 | 410 => format!("the feed is gone (HTTP {status})"),
            _ => format!("the feed answered HTTP {status}"),
        }));
    }
    let header = |name: &str| {
        res.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let (etag, last_modified) = (header("etag"), header("last-modified"));
    let mut bytes = Vec::new();
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| MusicError::Http(format!("the feed download broke off: {e}")))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_FEED_BYTES {
            return Err(MusicError::PayloadTooLarge(format!(
                "the feed is bigger than {} MB",
                MAX_FEED_BYTES / 1024 / 1024
            )));
        }
    }
    Ok(Fetched::Body {
        bytes,
        etag,
        last_modified,
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Fetch and read a feed that is not subscribed yet.
pub async fn fetch_and_parse(
    url: &str,
) -> Result<(ParsedFeed, Option<String>, Option<String>), MusicError> {
    match fetch(url, None, None).await? {
        Fetched::NotModified => Err(MusicError::Http("the feed sent nothing".into())),
        Fetched::Body {
            bytes,
            etag,
            last_modified,
        } => {
            let parsed = podcast_feed::parse_feed(&bytes, url).map_err(feed_error)?;
            Ok((parsed, etag, last_modified))
        }
    }
}

fn feed_error(e: FeedError) -> MusicError {
    MusicError::BadRequest(e.to_string())
}

// ---------------------------------------------------------------------------
// The catalog
// ---------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct FeedRow {
    pub id: i64,
    pub feed_url: String,
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub link: Option<String>,
    pub image_url: Option<String>,
    pub language: Option<String>,
    pub explicit: bool,
    pub last_fetched: Option<i64>,
    /// Why the last refresh failed; absent when it worked. Shown, never hidden.
    pub last_error: Option<String>,
    pub auto_download: bool,
    pub keep_n: i64,
    pub delete_played_after_days: i64,
    pub sort_order: i64,
    pub added_at: i64,
    pub episode_count: i64,
    pub unplayed_count: i64,
    /// Playback for this show: speed (0.5 to 3), skip seconds, and whether an
    /// episode that ends with nothing in Up Next goes on to the next unplayed one.
    pub speed: f64,
    pub skip_back_s: i64,
    pub skip_forward_s: i64,
    pub auto_advance: bool,
}

const FEED_SELECT: &str = "SELECT f.id, f.feed_url, f.title, f.author, f.description, f.link, f.image_url,
       f.language, f.explicit, f.last_fetched, f.last_error, f.auto_download, f.keep_n,
       f.delete_played_after_days, f.sort_order, f.added_at,
       f.speed, f.skip_back_s, f.skip_forward_s, f.auto_advance,
       (SELECT COUNT(*) FROM podcast_episodes e WHERE e.feed_id = f.id) AS episode_count,
       (SELECT COUNT(*) FROM podcast_episodes e WHERE e.feed_id = f.id AND e.played_at IS NULL) AS unplayed_count
     FROM podcast_feeds f";

fn feed_row(r: &sqlx::sqlite::SqliteRow) -> FeedRow {
    FeedRow {
        id: r.get("id"),
        feed_url: r.get("feed_url"),
        title: r.get("title"),
        author: r.get("author"),
        description: r.get("description"),
        link: r.get("link"),
        image_url: r.get("image_url"),
        language: r.get("language"),
        explicit: r.get::<i64, _>("explicit") != 0,
        last_fetched: r.get("last_fetched"),
        last_error: r.get("last_error"),
        auto_download: r.get::<i64, _>("auto_download") != 0,
        keep_n: r.get("keep_n"),
        delete_played_after_days: r.get("delete_played_after_days"),
        sort_order: r.get("sort_order"),
        added_at: r.get("added_at"),
        episode_count: r.get("episode_count"),
        unplayed_count: r.get("unplayed_count"),
        speed: r.get("speed"),
        skip_back_s: r.get("skip_back_s"),
        skip_forward_s: r.get("skip_forward_s"),
        auto_advance: r.get::<i64, _>("auto_advance") != 0,
    }
}

pub async fn list_feeds(pool: &SqlitePool) -> Result<Vec<FeedRow>, MusicError> {
    Ok(
        sqlx::query(&format!("{FEED_SELECT} ORDER BY f.sort_order, f.id"))
            .fetch_all(pool)
            .await
            .map_err(cvt)?
            .iter()
            .map(feed_row)
            .collect(),
    )
}

pub async fn get_feed(pool: &SqlitePool, id: i64) -> Result<FeedRow, MusicError> {
    sqlx::query(&format!("{FEED_SELECT} WHERE f.id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(cvt)?
        .map(|r| feed_row(&r))
        .ok_or_else(|| MusicError::NotFound(format!("podcast {id}")))
}

/// What storing a read feed changed.
#[derive(Serialize, Debug, Default, PartialEq, Clone)]
pub struct Stored {
    pub episodes_added: usize,
    pub episodes_updated: usize,
}

/// Write a read feed's details and episodes. Episodes are keyed by
/// `(feed, guid)`: new ones are added, known ones refreshed (a feed fixes a
/// title or moves its audio), and ones the feed no longer lists are kept and
/// marked, because feeds that show only their latest N must not lose the rest.
pub async fn store(
    pool: &SqlitePool,
    feed_id: i64,
    parsed: &ParsedFeed,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Stored, MusicError> {
    let mut tx = pool.begin().await.map_err(cvt)?;
    sqlx::query(
        "UPDATE podcast_feeds SET title = ?, author = ?, description = ?, link = ?, image_url = ?,
           language = ?, explicit = ?, last_fetched = ?, last_error = NULL, etag = ?, last_modified = ?
         WHERE id = ?",
    )
    .bind(&parsed.title)
    .bind(&parsed.author)
    .bind(&parsed.description)
    .bind(&parsed.link)
    .bind(&parsed.image_url)
    .bind(&parsed.language)
    .bind(i64::from(parsed.explicit))
    .bind(now_ms())
    .bind(etag)
    .bind(last_modified)
    .bind(feed_id)
    .execute(&mut *tx)
    .await
    .map_err(cvt)?;
    let mut stored = Stored::default();
    let now = now_ms();
    for e in &parsed.episodes {
        let existing: Option<i64> =
            sqlx::query("SELECT id FROM podcast_episodes WHERE feed_id = ? AND guid = ?")
                .bind(feed_id)
                .bind(&e.guid)
                .fetch_optional(&mut *tx)
                .await
                .map_err(cvt)?
                .map(|r| r.get(0));
        match existing {
            Some(id) => {
                // The audio address is left alone once the file is downloaded:
                // the file on disk is what plays.
                sqlx::query(
                    "UPDATE podcast_episodes SET title = ?, description_html = ?, published_at = ?,
                       duration_ms = ?, enclosure_url = CASE WHEN file_path IS NULL THEN ? ELSE enclosure_url END,
                       enclosure_type = ?, enclosure_bytes = ?, image_url = ?, season = ?, episode = ?,
                       link = ?, dropped_from_feed = 0
                     WHERE id = ?",
                )
                .bind(&e.title)
                .bind(&e.description_html)
                .bind(e.published_ms)
                .bind(e.duration_ms)
                .bind(&e.enclosure_url)
                .bind(&e.enclosure_type)
                .bind(e.enclosure_bytes)
                .bind(&e.image_url)
                .bind(e.season.map(i64::from))
                .bind(e.episode.map(i64::from))
                .bind(&e.link)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(cvt)?;
                stored.episodes_updated += 1;
            }
            None => {
                sqlx::query(
                    "INSERT INTO podcast_episodes (feed_id, guid, title, description_html, published_at,
                       duration_ms, enclosure_url, enclosure_type, enclosure_bytes, image_url, season,
                       episode, link, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(feed_id)
                .bind(&e.guid)
                .bind(&e.title)
                .bind(&e.description_html)
                .bind(e.published_ms)
                .bind(e.duration_ms)
                .bind(&e.enclosure_url)
                .bind(&e.enclosure_type)
                .bind(e.enclosure_bytes)
                .bind(&e.image_url)
                .bind(e.season.map(i64::from))
                .bind(e.episode.map(i64::from))
                .bind(&e.link)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(cvt)?;
                stored.episodes_added += 1;
            }
        }
    }
    // Mark what the feed stopped listing (only when it listed something: an
    // empty read is more likely a broken feed than a deleted back catalog).
    if !parsed.episodes.is_empty() {
        let keep: Vec<&str> = parsed.episodes.iter().map(|e| e.guid.as_str()).collect();
        let ids: Vec<(i64, String)> =
            sqlx::query("SELECT id, guid FROM podcast_episodes WHERE feed_id = ?")
                .bind(feed_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(cvt)?
                .iter()
                .map(|r| (r.get(0), r.get(1)))
                .collect();
        for (id, guid) in ids {
            if !keep.contains(&guid.as_str()) {
                sqlx::query("UPDATE podcast_episodes SET dropped_from_feed = 1 WHERE id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .map_err(cvt)?;
            }
        }
    }
    tx.commit().await.map_err(cvt)?;
    Ok(stored)
}

/// Add a feed row (details come from the first read).
pub async fn insert_feed(pool: &SqlitePool, url: &str, title: &str) -> Result<i64, MusicError> {
    let next: i64 = sqlx::query("SELECT COALESCE(MAX(sort_order), 0) + 1 FROM podcast_feeds")
        .fetch_one(pool)
        .await
        .map_err(cvt)?
        .get(0);
    sqlx::query(
        "INSERT INTO podcast_feeds (feed_url, title, sort_order, added_at) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(url)
    .bind(title)
    .bind(next)
    .bind(now_ms())
    .fetch_one(pool)
    .await
    .map(|r| r.get(0))
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            MusicError::Conflict("you already subscribe to that podcast".into())
        }
        e => cvt(e),
    })
}

pub async fn delete_feed(pool: &SqlitePool, id: i64) -> Result<(), MusicError> {
    let mut tx = pool.begin().await.map_err(cvt)?;
    sqlx::query("DELETE FROM podcast_episodes WHERE feed_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(cvt)?;
    let n = sqlx::query("DELETE FROM podcast_feeds WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(cvt)?
        .rows_affected();
    if n == 0 {
        return Err(MusicError::NotFound(format!("podcast {id}")));
    }
    tx.commit().await.map_err(cvt)?;
    Ok(())
}

/// Result of refreshing one feed.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Refreshed {
    pub feed_id: i64,
    /// Nothing new (the server said "not modified").
    pub unchanged: bool,
    pub episodes_added: usize,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

/// Refresh one subscribed feed. A failure is recorded on the feed
/// (`last_error`) and reported; the feed and its episodes stay.
pub async fn refresh_feed(pool: &SqlitePool, id: i64) -> Result<Refreshed, MusicError> {
    let row = sqlx::query("SELECT feed_url, etag, last_modified FROM podcast_feeds WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("podcast {id}")))?;
    let (url, etag, modified): (String, Option<String>, Option<String>) =
        (row.get(0), row.get(1), row.get(2));
    let outcome: Result<Refreshed, MusicError> = async {
        match fetch(&url, etag.as_deref(), modified.as_deref()).await? {
            Fetched::NotModified => {
                sqlx::query(
                    "UPDATE podcast_feeds SET last_fetched = ?, last_error = NULL WHERE id = ?",
                )
                .bind(now_ms())
                .bind(id)
                .execute(pool)
                .await
                .map_err(cvt)?;
                Ok(Refreshed {
                    feed_id: id,
                    unchanged: true,
                    episodes_added: 0,
                    warnings: Vec::new(),
                    error: None,
                })
            }
            Fetched::Body {
                bytes,
                etag,
                last_modified,
            } => {
                let parsed = podcast_feed::parse_feed(&bytes, &url).map_err(feed_error)?;
                let stored =
                    store(pool, id, &parsed, etag.as_deref(), last_modified.as_deref()).await?;
                Ok(Refreshed {
                    feed_id: id,
                    unchanged: false,
                    episodes_added: stored.episodes_added,
                    warnings: parsed.warnings,
                    error: None,
                })
            }
        }
    }
    .await;
    match outcome {
        Ok(r) => Ok(r),
        Err(e) => {
            let msg = match &e {
                MusicError::BadRequest(m)
                | MusicError::Http(m)
                | MusicError::PayloadTooLarge(m) => m.clone(),
                other => other.to_string(),
            };
            sqlx::query("UPDATE podcast_feeds SET last_error = ? WHERE id = ?")
                .bind(&msg)
                .bind(id)
                .execute(pool)
                .await
                .map_err(cvt)?;
            Ok(Refreshed {
                feed_id: id,
                unchanged: false,
                episodes_added: 0,
                warnings: Vec::new(),
                error: Some(msg),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// OPML
// ---------------------------------------------------------------------------

/// A subscription found in an OPML file.
#[derive(Debug, Clone, PartialEq)]
pub struct OpmlEntry {
    pub url: String,
    pub title: Option<String>,
}

fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find(';').filter(|j| *j <= 8) {
            let name = &after[..j];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16)
                    .ok()
                    .and_then(char::from_u32),
                n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(c) = ch {
                out.push(c);
                rest = &after[j + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn outline_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(&format!("{name}=")) {
        let at = from + i;
        // A whole attribute name: preceded by whitespace.
        if at > 0 && !lower.as_bytes()[at - 1].is_ascii_whitespace() {
            from = at + 1;
            continue;
        }
        let rest = &tag[at + name.len() + 1..];
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let end = rest[1..].find(quote)?;
        return Some(xml_unescape(&rest[1..1 + end]));
    }
    None
}

/// The feeds in an OPML file: every `<outline>` with an `xmlUrl`, at any
/// depth (folders are flattened).
pub fn parse_opml(text: &str) -> Vec<OpmlEntry> {
    let text = podcast_feed::decode_to_utf8(text.as_bytes());
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<outline") {
        let start = at + i;
        let Some(len) = text[start..].find('>') else {
            break;
        };
        let tag = &text[start..start + len];
        at = start + len;
        if let Some(url) = outline_attr(tag, "xmlurl").filter(|u| !u.trim().is_empty()) {
            let title = outline_attr(tag, "text")
                .or_else(|| outline_attr(tag, "title"))
                .filter(|t| !t.trim().is_empty());
            out.push(OpmlEntry {
                url: url.trim().to_string(),
                title,
            });
        }
    }
    out
}

/// Subscriptions as an OPML 2.0 file.
pub fn write_opml(feeds: &[FeedRow]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n  <head><title>Kahawai podcasts</title></head>\n  <body>\n",
    );
    for f in feeds {
        s.push_str(&format!(
            "    <outline type=\"rss\" text=\"{t}\" title=\"{t}\" xmlUrl=\"{u}\"{h}/>\n",
            t = xml_escape(&f.title),
            u = xml_escape(&f.feed_url),
            h = f
                .link
                .as_ref()
                .map(|l| format!(" htmlUrl=\"{}\"", xml_escape(l)))
                .unwrap_or_default(),
        ));
    }
    s.push_str("  </body>\n</opml>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opml_is_read_from_nested_folders_with_entities() {
        let opml = r#"<?xml version="1.0"?><opml version="2.0"><head/><body>
          <outline text="News">
            <outline type="rss" text="Show &amp; Tell" xmlUrl="https://a.example/feed?x=1&amp;y=2" htmlUrl="https://a.example"/>
            <outline type="rss" title="Only Title" XMLURL='https://b.example/rss'/>
          </outline>
          <outline text="A folder with no url"/>
          <outline type="rss" text="Blank" xmlUrl="  "/>
        </body></opml>"#;
        let e = parse_opml(opml);
        assert_eq!(
            e,
            [
                OpmlEntry {
                    url: "https://a.example/feed?x=1&y=2".into(),
                    title: Some("Show & Tell".into())
                },
                OpmlEntry {
                    url: "https://b.example/rss".into(),
                    title: Some("Only Title".into())
                },
            ]
        );
        assert!(parse_opml("not opml at all").is_empty());
    }

    #[test]
    fn opml_round_trips() {
        let f = |id: i64, title: &str, url: &str| FeedRow {
            id,
            feed_url: url.into(),
            title: title.into(),
            author: None,
            description: None,
            link: Some("https://h.example/?a=1&b=2".into()),
            image_url: None,
            language: None,
            explicit: false,
            last_fetched: None,
            last_error: None,
            auto_download: true,
            keep_n: 5,
            delete_played_after_days: 7,
            sort_order: id,
            added_at: 0,
            episode_count: 0,
            unplayed_count: 0,
            speed: 1.0,
            skip_back_s: 15,
            skip_forward_s: 30,
            auto_advance: false,
        };
        let feeds = [
            f(1, "Tom & \"Jerry\" <live>", "https://a.example/f?x=1&y=2"),
            f(2, "Two", "https://b.example/f"),
        ];
        let back = parse_opml(&write_opml(&feeds));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].url, "https://a.example/f?x=1&y=2");
        assert_eq!(back[0].title.as_deref(), Some("Tom & \"Jerry\" <live>"));
    }

    #[test]
    fn credentials_in_an_address_decode_for_basic_auth() {
        assert_eq!(percent_decode("p%40ss%20word"), "p@ss word");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("bad%zz"), "bad%zz");
    }
}

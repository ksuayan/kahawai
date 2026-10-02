//! Online details for audiobooks (docs/v1/kahawai-audiobook-spec.md, D6).
//!
//! There is no MusicBrainz for audiobooks, and narrators in particular have
//! no open source, so this only fills what can be found: the author, the
//! year and the cover. The chain is Open Library (CC0 data, no key, kept to
//! about three requests a second, a polite User-Agent) and then Google Books
//! for what is still missing. Nothing is scraped from Audible, and nothing
//! the listener edited or the tags already say is ever overwritten.
//!
//! It sends titles and authors off the LAN, so it sits behind the same
//! opt-in as the album lookup (`enrichment_enabled`, off by default).

use std::time::Duration;

use kahawai_core::{JobStatus, MusicError};
use serde_json::Value;
use sqlx::{sqlite::SqlitePool, Row};
use tokio::{sync::Mutex, time::Instant};

use crate::db::cvt;
use crate::jobs::JobStore;
use crate::musicbrainz::{similarity, USER_AGENT};
use crate::normalize::group_key;

/// At least this long between requests: Open Library asks for about three
/// per second at most.
pub const MIN_INTERVAL: Duration = Duration::from_millis(350);
/// Confidence needed to trust a match.
pub const MIN_CONFIDENCE: f32 = 0.8;
const MAX_RETRIES: u32 = 3;

#[derive(Debug)]
pub enum LookupError {
    /// The service can't be reached (offline, DNS, down, or refusing after
    /// the retries): stop, and do not count it against the book.
    Unreachable(String),
    /// This one answer was unusable.
    Failed(String),
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookupError::Unreachable(m) | LookupError::Failed(m) => f.write_str(m),
        }
    }
}

/// A book a search returned.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<u16>,
    pub cover_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsing and matching (pure)
// ---------------------------------------------------------------------------

/// Open Library `search.json` documents.
pub fn parse_open_library(v: &Value, covers_base: &str) -> Vec<Candidate> {
    v["docs"]
        .as_array()
        .map(|docs| {
            docs.iter()
                .filter_map(|d| {
                    let title = d["title"].as_str()?.to_string();
                    let authors = d["author_name"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let year = d["first_publish_year"]
                        .as_u64()
                        .and_then(|y| u16::try_from(y).ok())
                        .filter(|y| (1000..=2999).contains(y));
                    // `default=false` makes a missing cover a 404 instead of a blank image.
                    let cover_url = d["cover_i"]
                        .as_u64()
                        .map(|id| format!("{covers_base}/b/id/{id}-L.jpg?default=false"));
                    Some(Candidate {
                        title,
                        authors,
                        year,
                        cover_url,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Google Books `volumes` items.
pub fn parse_google_books(v: &Value) -> Vec<Candidate> {
    v["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|i| {
                    let info = &i["volumeInfo"];
                    let title = info["title"].as_str()?.to_string();
                    let authors = info["authors"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let year = info["publishedDate"]
                        .as_str()
                        .and_then(|d| d.get(..4))
                        .and_then(|y| y.parse::<u16>().ok())
                        .filter(|y| (1000..=2999).contains(y));
                    let cover_url = info["imageLinks"]["thumbnail"]
                        .as_str()
                        .map(|u| u.replacen("http://", "https://", 1));
                    Some(Candidate {
                        title,
                        authors,
                        year,
                        cover_url,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The part of a title before a subtitle ("Dune: Deluxe Edition" → "Dune").
fn main_title(t: &str) -> &str {
    t.split([':', '(']).next().unwrap_or(t).trim()
}

/// How well a candidate fits the book, 0 to 1. The title carries most of it;
/// the author must agree when we know it.
pub fn confidence(title: &str, author: Option<&str>, c: &Candidate) -> f32 {
    let (want, got) = (
        group_key(main_title(title)),
        group_key(main_title(&c.title)),
    );
    let title_score = similarity(&want, &got).max(if !want.is_empty() && want == got {
        1.0
    } else {
        0.0
    });
    match author.map(group_key).filter(|a| !a.is_empty()) {
        Some(a) if !c.authors.is_empty() => {
            let best = c
                .authors
                .iter()
                .map(|x| {
                    let x = group_key(x);
                    // "Le Guin, Ursula" and "Ursula K. Le Guin" share words, not order.
                    let words: Vec<&str> = a.split_whitespace().collect();
                    let overlap = words
                        .iter()
                        .filter(|w| x.split_whitespace().any(|y| y == **w))
                        .count();
                    similarity(&a, &x).max(overlap as f32 / words.len().max(1) as f32)
                })
                .fold(0.0f32, f32::max);
            title_score * (0.4 + 0.6 * best.min(1.0))
        }
        // Nothing to check the author against: a sure title is not quite enough.
        _ => title_score * 0.9,
    }
}

pub fn best_match<'a>(
    title: &str,
    author: Option<&str>,
    candidates: &'a [Candidate],
    min: f32,
) -> Option<&'a Candidate> {
    candidates
        .iter()
        .map(|c| (c, confidence(title, author, c)))
        .filter(|(_, conf)| *conf >= min)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, _)| c)
}

/// What a lookup fills in; only blanks are ever written.
#[derive(Debug, Default, PartialEq)]
pub struct Fill {
    pub author: Option<String>,
    pub year: Option<i64>,
    pub cover_url: Option<String>,
}

/// Choose what to take from a match given what the book already has.
pub fn fill_from(c: &Candidate, has_author: bool, has_year: bool, has_cover: bool) -> Fill {
    Fill {
        author: (!has_author).then(|| c.authors.first().cloned()).flatten(),
        year: (!has_year).then_some(c.year).flatten().map(i64::from),
        cover_url: (!has_cover).then(|| c.cover_url.clone()).flatten(),
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct BookClient {
    http: reqwest::Client,
    ol_base: String,
    covers_base: String,
    gb_base: String,
    next_slot: Mutex<Instant>,
    min_interval: Duration,
}

impl BookClient {
    pub fn new() -> Self {
        Self::with_bases(
            "https://openlibrary.org",
            "https://covers.openlibrary.org",
            "https://www.googleapis.com/books/v1",
            MIN_INTERVAL,
        )
    }

    /// Test seam: other hosts (a local stub) and interval.
    pub fn with_bases(ol: &str, covers: &str, gb: &str, min_interval: Duration) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("static client config is valid");
        Self {
            http,
            ol_base: ol.trim_end_matches('/').to_string(),
            covers_base: covers.trim_end_matches('/').to_string(),
            gb_base: gb.trim_end_matches('/').to_string(),
            next_slot: Mutex::new(Instant::now()),
            min_interval,
        }
    }

    async fn wait_turn(&self) {
        let mut next = self.next_slot.lock().await;
        let now = Instant::now();
        if *next > now {
            tokio::time::sleep_until(*next).await;
        }
        *next = Instant::now() + self.min_interval;
    }

    /// GET with spacing and a few retries on 429/503. `Ok(None)` for 404.
    async fn get(&self, url: reqwest::Url) -> Result<Option<reqwest::Response>, LookupError> {
        let mut attempt = 0;
        loop {
            self.wait_turn().await;
            let res = self.http.get(url.clone()).send().await.map_err(|e| {
                LookupError::Unreachable(format!(
                    "can't reach {}: {e}",
                    url.host_str().unwrap_or("the lookup service")
                ))
            })?;
            let status = res.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
            {
                attempt += 1;
                if attempt > MAX_RETRIES {
                    return Err(LookupError::Unreachable(format!(
                        "{} is busy (HTTP {status})",
                        url.host_str().unwrap_or("the service")
                    )));
                }
                tokio::time::sleep(self.min_interval * (1 << attempt)).await;
                continue;
            }
            if status.is_server_error() {
                return Err(LookupError::Unreachable(format!(
                    "{} is down (HTTP {status})",
                    url.host_str().unwrap_or("the service")
                )));
            }
            if !status.is_success() {
                return Err(LookupError::Failed(format!("HTTP {status}")));
            }
            return Ok(Some(res));
        }
    }

    async fn get_json(&self, url: reqwest::Url) -> Result<Value, LookupError> {
        match self.get(url).await? {
            Some(r) => r
                .json()
                .await
                .map_err(|e| LookupError::Failed(format!("unreadable reply: {e}"))),
            None => Ok(Value::Null),
        }
    }

    pub async fn search_open_library(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Result<Vec<Candidate>, LookupError> {
        let mut url = reqwest::Url::parse(&format!("{}/search.json", self.ol_base))
            .map_err(|e| LookupError::Failed(e.to_string()))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("title", main_title(title));
            if let Some(a) = author {
                q.append_pair("author", a);
            }
            q.append_pair("limit", "5");
            q.append_pair("fields", "title,author_name,first_publish_year,cover_i");
        }
        Ok(parse_open_library(
            &self.get_json(url).await?,
            &self.covers_base,
        ))
    }

    pub async fn search_google_books(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Result<Vec<Candidate>, LookupError> {
        let mut q = format!("intitle:\"{}\"", main_title(title).replace('"', ""));
        if let Some(a) = author {
            q.push_str(&format!(" inauthor:\"{}\"", a.replace('"', "")));
        }
        let mut url = reqwest::Url::parse(&format!("{}/volumes", self.gb_base))
            .map_err(|e| LookupError::Failed(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("q", &q)
            .append_pair("maxResults", "5")
            .append_pair("printType", "books");
        Ok(parse_google_books(&self.get_json(url).await?))
    }

    /// A cover image, or `None` when there is none (404 or not an image).
    pub async fn fetch_cover(&self, url: &str) -> Result<Option<(String, Vec<u8>)>, LookupError> {
        let url = reqwest::Url::parse(url).map_err(|e| LookupError::Failed(e.to_string()))?;
        let Some(res) = self.get(url).await? else {
            return Ok(None);
        };
        let mime = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if !mime.starts_with("image/") {
            return Ok(None);
        }
        let bytes = res
            .bytes()
            .await
            .map_err(|e| LookupError::Failed(e.to_string()))?;
        // A placeholder pixel is not a cover.
        Ok((bytes.len() > 1024).then(|| (mime, bytes.to_vec())))
    }
}

impl Default for BookClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------------

#[derive(Debug, Default, PartialEq)]
pub struct EnrichReport {
    pub looked_up: u64,
    pub matched: u64,
    pub covers: u64,
    pub not_found: u64,
    /// Stopped early because the service could not be reached.
    pub offline: Option<String>,
    pub cancelled: bool,
}

/// Books worth asking about: not asked before, not hand-edited, with a blank
/// to fill. `only` limits it to one book (and ignores the "asked before").
async fn pending(
    pool: &SqlitePool,
    only: Option<i64>,
) -> Result<Vec<(i64, String, Option<String>, bool, bool, bool)>, MusicError> {
    let sql = "SELECT id, title, author, year IS NOT NULL AS has_year, cover_hash IS NOT NULL AS has_cover
               FROM audiobooks
               WHERE meta_edited = 0 AND (author IS NULL OR year IS NULL OR cover_hash IS NULL)
                 AND (? IS NOT NULL OR enriched_at IS NULL) AND (? IS NULL OR id = ?)
               ORDER BY id";
    let rows = sqlx::query(sql)
        .bind(only)
        .bind(only)
        .bind(only)
        .fetch_all(pool)
        .await
        .map_err(cvt)?;
    Ok(rows
        .iter()
        .map(|r| {
            let author: Option<String> = r.get("author");
            (
                r.get("id"),
                r.get("title"),
                author.clone(),
                author.is_some(),
                r.get::<bool, _>("has_year"),
                r.get::<bool, _>("has_cover"),
            )
        })
        .collect())
}

pub async fn enrich_books(
    pool: &SqlitePool,
    client: &BookClient,
    only: Option<i64>,
    jobs: &JobStore,
    job_id: &str,
    on_progress: impl Fn(u64, u64) + Send + Sync,
) -> Result<EnrichReport, MusicError> {
    let todo = pending(pool, only).await?;
    let total = todo.len() as u64;
    let mut report = EnrichReport::default();
    for (n, (id, title, author, has_author, has_year, has_cover)) in todo.into_iter().enumerate() {
        if !matches!(
            jobs.get(job_id).map(|j| j.status),
            Some(JobStatus::Queued | JobStatus::Running)
        ) {
            report.cancelled = true;
            return Ok(report);
        }
        let mut fill = Fill::default();
        let (mut got_author, mut got_year, mut got_cover) = (has_author, has_year, has_cover);
        let mut matched = false;
        // Open Library first, then Google Books for what is still missing.
        for source in 0..2 {
            if got_author && got_year && got_cover {
                break;
            }
            let found = if source == 0 {
                client.search_open_library(&title, author.as_deref()).await
            } else {
                client.search_google_books(&title, author.as_deref()).await
            };
            let candidates = match found {
                Ok(c) => c,
                Err(LookupError::Unreachable(m)) => {
                    report.offline = Some(m);
                    return Ok(report);
                }
                Err(LookupError::Failed(m)) => {
                    tracing::warn!(book = id, error = %m, "audiobook lookup failed");
                    continue;
                }
            };
            let Some(c) = best_match(&title, author.as_deref(), &candidates, MIN_CONFIDENCE) else {
                continue;
            };
            matched = true;
            let f = fill_from(c, got_author, got_year, got_cover);
            if fill.author.is_none() && f.author.is_some() {
                got_author = true;
                fill.author = f.author;
            }
            if fill.year.is_none() && f.year.is_some() {
                got_year = true;
                fill.year = f.year;
            }
            if let Some(url) = f.cover_url {
                match client.fetch_cover(&url).await {
                    Ok(Some(img)) => {
                        let hash = blake3::hash(&img.1).to_hex().to_string();
                        sqlx::query(
                            "INSERT OR IGNORE INTO artwork (hash, mime, bytes) VALUES (?, ?, ?)",
                        )
                        .bind(&hash)
                        .bind(img.0)
                        .bind(img.1)
                        .execute(pool)
                        .await
                        .map_err(cvt)?;
                        sqlx::query("UPDATE audiobooks SET cover_hash = COALESCE(cover_hash, ?) WHERE id = ?")
                            .bind(hash)
                            .bind(id)
                            .execute(pool)
                            .await
                            .map_err(cvt)?;
                        got_cover = true;
                        report.covers += 1;
                    }
                    Ok(None) => {}
                    // The details were found; a cover that will not download
                    // (a dead image host) is not worth stopping the job for.
                    Err(e) => tracing::warn!(book = id, error = %e, "cover download failed"),
                }
            }
        }
        sqlx::query(
            "UPDATE audiobooks SET author = COALESCE(author, ?), year = COALESCE(year, ?), enriched_at = ? WHERE id = ?",
        )
        .bind(fill.author)
        .bind(fill.year)
        .bind(crate::audiobooks::now_ms())
        .bind(id)
        .execute(pool)
        .await
        .map_err(cvt)?;
        report.looked_up += 1;
        if matched {
            report.matched += 1;
        } else {
            report.not_found += 1;
        }
        on_progress(n as u64 + 1, total);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::Query,
        http::{header, StatusCode},
        response::IntoResponse,
        routing::get,
        Json, Router,
    };
    use serde_json::json;
    use std::{collections::HashMap, sync::Arc};

    fn cand(title: &str, authors: &[&str], year: Option<u16>) -> Candidate {
        Candidate {
            title: title.into(),
            authors: authors.iter().map(|s| s.to_string()).collect(),
            year,
            cover_url: None,
        }
    }

    #[test]
    fn open_library_and_google_books_replies_are_read() {
        let ol = json!({"docs": [
            {"title": "Dune", "author_name": ["Frank Herbert"], "first_publish_year": 1965, "cover_i": 12345},
            {"title": "No Cover", "author_name": [], "first_publish_year": 99999},
            {"author_name": ["Nameless"]}
        ]});
        let c = parse_open_library(&ol, "https://covers.example");
        assert_eq!(c.len(), 2, "an entry with no title is dropped");
        assert_eq!(c[0].year, Some(1965));
        assert_eq!(
            c[0].cover_url.as_deref(),
            Some("https://covers.example/b/id/12345-L.jpg?default=false")
        );
        assert_eq!(
            (c[1].year, c[1].cover_url.clone()),
            (None, None),
            "an impossible year is dropped"
        );
        assert!(parse_open_library(&Value::Null, "x").is_empty());

        let gb = json!({"items": [{"volumeInfo": {
            "title": "Dune", "authors": ["Frank Herbert"], "publishedDate": "2005-08-02",
            "imageLinks": {"thumbnail": "http://books.google.com/x.jpg"}}}]});
        let c = parse_google_books(&gb);
        assert_eq!(c[0].year, Some(2005));
        assert_eq!(
            c[0].cover_url.as_deref(),
            Some("https://books.google.com/x.jpg")
        );
        assert!(parse_google_books(&json!({})).is_empty());
    }

    #[test]
    fn matching_trusts_the_title_and_checks_the_author() {
        let exact = cand("Dune", &["Frank Herbert"], Some(1965));
        assert!(confidence("Dune", Some("Frank Herbert"), &exact) > 0.95);
        // A subtitle on either side does not hurt.
        let long = cand("Dune: Deluxe Edition", &["Frank Herbert"], None);
        assert!(confidence("Dune", Some("Frank Herbert"), &long) > 0.95);
        // Same title, different author: not our book.
        let other = cand("Dune", &["Someone Else"], None);
        assert!(confidence("Dune", Some("Frank Herbert"), &other) < MIN_CONFIDENCE);
        // Name order differs, words agree.
        let comma = cand("The Dispossessed", &["Le Guin, Ursula K."], None);
        assert!(
            confidence("The Dispossessed", Some("Ursula K. Le Guin"), &comma) >= MIN_CONFIDENCE
        );
        // No author known: a sure title still passes, but not with a different title.
        assert!(confidence("Dune", None, &exact) >= MIN_CONFIDENCE);
        assert!(confidence("Dune Messiah", None, &exact) < MIN_CONFIDENCE);
        let all = [other.clone(), exact.clone()];
        assert_eq!(
            best_match("Dune", Some("Frank Herbert"), &all, MIN_CONFIDENCE),
            Some(&exact)
        );
        assert_eq!(best_match("Unknown", None, &all, MIN_CONFIDENCE), None);
    }

    #[test]
    fn only_blanks_are_filled() {
        let mut c = cand("Dune", &["Frank Herbert"], Some(1965));
        c.cover_url = Some("https://c/x.jpg".into());
        assert_eq!(
            fill_from(&c, false, false, false),
            Fill {
                author: Some("Frank Herbert".into()),
                year: Some(1965),
                cover_url: Some("https://c/x.jpg".into())
            }
        );
        assert_eq!(fill_from(&c, true, true, true), Fill::default());
        assert_eq!(fill_from(&c, true, false, true).year, Some(1965));
    }

    // --- the job, against a local stand-in for the services ---------------------

    /// A cover image (over the size of a placeholder pixel).
    fn image() -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G'];
        v.extend((0..2000).map(|i| (i % 251) as u8));
        v
    }

    /// Serves Open Library, its covers, and Google Books for a few titles.
    async fn stub(ol_hits: Arc<std::sync::Mutex<Vec<String>>>) -> String {
        let hits = ol_hits.clone();
        let app = Router::new()
            .route(
                "/search.json",
                get(move |Query(q): Query<HashMap<String, String>>| {
                    let hits = hits.clone();
                    async move {
                        let title = q.get("title").cloned().unwrap_or_default();
                        hits.lock().unwrap().push(format!("ol:{title}"));
                        let docs = match title.as_str() {
                            "Dune" => json!([{"title": "Dune", "author_name": ["Frank Herbert"], "first_publish_year": 1965, "cover_i": 7}]),
                            "Gaia Only" => json!([]),
                            _ => json!([]),
                        };
                        Json(json!({"docs": docs}))
                    }
                }),
            )
            .route(
                "/b/id/{file}",
                get(|| async { ([(header::CONTENT_TYPE, "image/jpeg")], image()) }),
            )
            .route(
                "/volumes",
                get({
                    let hits = ol_hits.clone();
                    move |Query(q): Query<HashMap<String, String>>| {
                        let hits = hits.clone();
                        async move {
                            let query = q.get("q").cloned().unwrap_or_default();
                            hits.lock().unwrap().push(format!("gb:{query}"));
                            if query.contains("Gaia Only") {
                                Json(json!({"items": [{"volumeInfo": {
                                    "title": "Gaia Only", "authors": ["G. Writer"], "publishedDate": "2011",
                                    "imageLinks": {"thumbnail": "http://127.0.0.1/never-fetched-over-https"}}}]}))
                                .into_response()
                            } else {
                                (StatusCode::OK, Json(json!({"totalItems": 0}))).into_response()
                            }
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    async fn db_with_books(
        books: &[(&str, Option<&str>, i64)],
    ) -> (tempfile::TempDir, SqlitePool, Vec<i64>) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("t.db")).await.unwrap();
        sqlx::query("INSERT INTO audiobook_roots (id, path, name) VALUES (1, '/b', 'b')")
            .execute(&pool)
            .await
            .unwrap();
        let mut ids = Vec::new();
        for (i, (title, author, edited)) in books.iter().enumerate() {
            let id: i64 = sqlx::query(
                "INSERT INTO audiobooks (root_id, path, title, author, added_at, meta_edited) VALUES (1, ?, ?, ?, 0, ?) RETURNING id",
            )
            .bind(format!("/b/{i}"))
            .bind(title)
            .bind(author)
            .bind(edited)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
            ids.push(id);
        }
        (dir, pool, ids)
    }

    async fn run(pool: &SqlitePool, base: &str, only: Option<i64>) -> EnrichReport {
        let jobs = JobStore::new();
        let job = jobs
            .create(kahawai_core::JobKind::EnrichBooks, "t".into(), None)
            .await;
        jobs.set_status(&job.id, JobStatus::Running).await;
        let client = BookClient::with_bases(base, base, base, Duration::from_millis(1));
        enrich_books(pool, &client, only, &jobs, &job.id, |_, _| {})
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn blanks_are_filled_from_open_library_then_google_and_nothing_else_changes() {
        let hits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let base = stub(hits.clone()).await;
        // Dune has an author; Gaia Only has none and Open Library does not know it;
        // the hand-edited book must be left alone; the last matches nothing.
        let (_d, pool, ids) = db_with_books(&[
            ("Dune", Some("Frank Herbert"), 0),
            ("Gaia Only", None, 0),
            ("Edited", None, 1),
            ("Nothing Known", None, 0),
        ])
        .await;
        let r = run(&pool, &base, None).await;
        assert_eq!((r.looked_up, r.matched, r.not_found), (3, 2, 1), "{r:?}");
        assert_eq!(r.covers, 1, "Dune's cover came from Open Library");

        let row = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query("SELECT author, year, cover_hash IS NOT NULL AS cover, enriched_at FROM audiobooks WHERE id = ?")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        let dune = row(ids[0]).await;
        assert_eq!(
            dune.get::<Option<String>, _>("author").as_deref(),
            Some("Frank Herbert")
        );
        assert_eq!(dune.get::<Option<i64>, _>("year"), Some(1965));
        assert!(dune.get::<bool, _>("cover"));
        let gaia = row(ids[1]).await;
        assert_eq!(
            gaia.get::<Option<String>, _>("author").as_deref(),
            Some("G. Writer"),
            "Google Books filled the author"
        );
        assert_eq!(gaia.get::<Option<i64>, _>("year"), Some(2011));
        let edited = row(ids[2]).await;
        assert!(
            edited.get::<Option<i64>, _>("enriched_at").is_none(),
            "a hand-edited book is never looked up"
        );
        let none = row(ids[3]).await;
        assert!(
            none.get::<Option<i64>, _>("enriched_at").is_some(),
            "asked once, not again"
        );
        assert!(none.get::<Option<String>, _>("author").is_none());

        // A second run asks about nothing: every book was asked.
        let before = hits.lock().unwrap().len();
        let again = run(&pool, &base, None).await;
        assert_eq!(again.looked_up, 0);
        assert_eq!(
            hits.lock().unwrap().len(),
            before,
            "no request for a book already asked about"
        );
        // ...unless that one book is asked for by name.
        let one = run(&pool, &base, Some(ids[3])).await;
        assert_eq!(one.looked_up, 1);
    }

    #[tokio::test]
    async fn an_unreachable_service_stops_the_job_without_charging_the_books() {
        let (_d, pool, ids) = db_with_books(&[("Dune", None, 0), ("Other", None, 0)]).await;
        // Nothing listens here.
        let r = run(&pool, "http://127.0.0.1:1", None).await;
        assert!(r.offline.is_some(), "{r:?}");
        assert_eq!(r.looked_up, 0);
        let asked: i64 =
            sqlx::query("SELECT COUNT(*) FROM audiobooks WHERE enriched_at IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(asked, 0, "they will be tried again next time");
        let _ = ids;
    }

    #[tokio::test]
    async fn a_cancelled_job_stops_between_books() {
        let hits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let base = stub(hits).await;
        let (_d, pool, _ids) = db_with_books(&[("Dune", None, 0), ("Gaia Only", None, 0)]).await;
        let jobs = JobStore::new();
        let job = jobs
            .create(kahawai_core::JobKind::EnrichBooks, "t".into(), None)
            .await;
        jobs.set_status(&job.id, JobStatus::Running).await;
        jobs.transition(&job.id, &[JobStatus::Running], JobStatus::Cancelled)
            .await;
        let client = BookClient::with_bases(&base, &base, &base, Duration::from_millis(1));
        let r = enrich_books(&pool, &client, None, &jobs, &job.id, |_, _| {})
            .await
            .unwrap();
        assert!(r.cancelled);
        assert_eq!(r.looked_up, 0);
    }
}

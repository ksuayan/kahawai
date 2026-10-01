//! MusicBrainz and Cover Art Archive client for metadata enrichment
//! (docs/v1/kahawai-metadata-enrichment-spec.md).
//!
//! MusicBrainz's rules (musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting):
//! on average one request per second per IP, a meaningful User-Agent with
//! contact details, and HTTP 503 when a client goes over. So this client:
//!
//! - spaces every request at least [`MIN_INTERVAL`] apart plus random jitter,
//!   through one lock, so concurrent callers can't exceed the rate;
//! - answers from `mb_cache` first: a request already made is never repeated;
//! - on 503/429 backs off (honoring Retry-After) and retries a few times.
//!
//! Cover Art Archive has no stated rate limit; covers are fetched one at a
//! time as albums are matched.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kahawai_core::MusicError;
use serde_json::Value;
use sqlx::{sqlite::SqlitePool, Row};
use tokio::{sync::Mutex, time::Instant};

use crate::db;
use crate::normalize::group_key;

/// Identifies Kahawai to MusicBrainz, as their rules require.
pub const USER_AGENT: &str = concat!(
    "Kahawai/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/ksuayan/kahawai )"
);
/// At least this long between MusicBrainz requests (their limit is 1/s).
pub const MIN_INTERVAL: Duration = Duration::from_millis(1100);
const MAX_JITTER: Duration = Duration::from_millis(250);
/// Retries after 503/429 before MusicBrainz counts as unreachable.
const MAX_RETRIES: u32 = 5;
/// Retries after a network error (no connection, DNS, timeout).
const MAX_NETWORK_RETRIES: u32 = 2;
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Why a lookup failed. The difference matters: an outage must not count
/// against the album (it would be given up on for nothing), a bad answer
/// about one album should.
#[derive(Debug)]
pub enum MbError {
    /// MusicBrainz can't be reached: no internet connection, DNS, timeout,
    /// or the service is down or still refusing (5xx) after the retries.
    Unreachable(String),
    /// This request failed for its own reasons (a 4xx, an unreadable reply).
    Failed(String),
    /// Our own database: not MusicBrainz's fault, stops the job.
    Db(MusicError),
}

impl std::fmt::Display for MbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MbError::Unreachable(m) | MbError::Failed(m) => f.write_str(m),
            MbError::Db(e) => write!(f, "{e}"),
        }
    }
}

pub struct MbClient {
    http: reqwest::Client,
    mb_base: String,
    caa_base: String,
    pool: SqlitePool,
    next_slot: Mutex<Instant>,
    min_interval: Duration,
}

/// One MusicBrainz release that a search returned.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub mbid: String,
    pub title: String,
    pub artist: String,
    /// "1959-08-17", "1959" or empty.
    pub date: String,
    pub track_count: Option<u32>,
    /// MusicBrainz's own relevance score, 0-100.
    pub score: u32,
}

/// What we know about an album, to match it against candidates.
#[derive(Debug, Clone)]
pub struct AlbumQuery {
    pub title: String,
    pub artist: Option<String>,
    pub track_count: u32,
}

impl MbClient {
    pub fn new(pool: SqlitePool) -> Self {
        Self::with_bases(
            pool,
            "https://musicbrainz.org/ws/2",
            "https://coverartarchive.org",
            MIN_INTERVAL,
        )
    }

    /// Test seam: other hosts (a local stub) and interval.
    pub fn with_bases(
        pool: SqlitePool,
        mb_base: &str,
        caa_base: &str,
        min_interval: Duration,
    ) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("static client config is valid");
        Self {
            http,
            mb_base: mb_base.trim_end_matches('/').to_string(),
            caa_base: caa_base.trim_end_matches('/').to_string(),
            pool,
            next_slot: Mutex::new(Instant::now()),
            min_interval,
        }
    }

    /// Wait for this request's turn. The lock is held while waiting, so
    /// callers line up one interval apart.
    async fn wait_turn(&self) {
        let mut next = self.next_slot.lock().await;
        let now = Instant::now();
        if *next > now {
            tokio::time::sleep_until(*next).await;
        }
        *next = Instant::now() + self.min_interval + jitter(MAX_JITTER);
    }

    /// GET a MusicBrainz JSON document, from the cache when it was fetched
    /// before. `Ok(None)` for 404.
    pub async fn get_json(&self, url: &reqwest::Url) -> Result<Option<Value>, MbError> {
        let key = blake3::hash(url.as_str().as_bytes()).to_hex().to_string();
        if let Some(r) = sqlx::query("SELECT response_json FROM mb_cache WHERE query_hash = ?")
            .bind(&key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| MbError::Db(db::cvt(e)))?
        {
            let text: String = r.get(0);
            return serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| MbError::Failed(format!("cached MusicBrainz response: {e}")));
        }
        let (mut attempt, mut network_attempt) = (0, 0);
        loop {
            self.wait_turn().await;
            let res = match self
                .http
                .get(url.clone())
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await
            {
                Ok(res) => res,
                Err(e) => {
                    network_attempt += 1;
                    if network_attempt > MAX_NETWORK_RETRIES {
                        return Err(MbError::Unreachable(format!(
                            "can't reach MusicBrainz: {e}"
                        )));
                    }
                    self.push_back(self.backoff(network_attempt)).await;
                    continue;
                }
            };
            let status = res.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            {
                attempt += 1;
                if attempt > MAX_RETRIES {
                    return Err(MbError::Unreachable(format!(
                        "MusicBrainz still busy ({status}) after {MAX_RETRIES} retries"
                    )));
                }
                let wait = retry_after(&res).unwrap_or_else(|| self.backoff(attempt));
                tracing::warn!(%status, attempt, wait_ms = wait.as_millis() as u64, "MusicBrainz asked us to slow down");
                self.push_back(wait.min(MAX_BACKOFF)).await;
                continue;
            }
            if status.is_server_error() {
                return Err(MbError::Unreachable(format!(
                    "MusicBrainz is down (HTTP {status})"
                )));
            }
            if !status.is_success() {
                return Err(MbError::Failed(format!("MusicBrainz: HTTP {status}")));
            }
            let text = res
                .text()
                .await
                .map_err(|e| MbError::Unreachable(format!("MusicBrainz reply cut off: {e}")))?;
            let value: Value = serde_json::from_str(&text)
                .map_err(|e| MbError::Failed(format!("MusicBrainz response: {e}")))?;
            sqlx::query("INSERT OR REPLACE INTO mb_cache (query_hash, response_json, fetched_at) VALUES (?, ?, ?)")
                .bind(&key)
                .bind(&text)
                .bind(unix_now())
                .execute(&self.pool)
                .await
                .map_err(|e| MbError::Db(db::cvt(e)))?;
            return Ok(Some(value));
        }
    }

    /// 2, 4, 8, 16, 32 request intervals (about 2 s to 35 s), capped.
    fn backoff(&self, attempt: u32) -> Duration {
        (self.min_interval * 2u32.saturating_pow(attempt)).min(MAX_BACKOFF)
    }

    /// Delay the next request by at least `wait` (a backoff shared by all
    /// callers, since the limit is per IP).
    async fn push_back(&self, wait: Duration) {
        let mut next = self.next_slot.lock().await;
        let later = Instant::now() + wait + jitter(MAX_JITTER);
        if later > *next {
            *next = later;
        }
    }

    /// Search releases for an album. One request, cached.
    pub async fn search_releases(&self, q: &AlbumQuery) -> Result<Vec<Candidate>, MbError> {
        let mut query = format!("release:\"{}\"", lucene_phrase(&q.title));
        if let Some(a) = q.artist.as_deref().filter(|a| !a.trim().is_empty()) {
            query.push_str(&format!(" AND artist:\"{}\"", lucene_phrase(a)));
        }
        let url = reqwest::Url::parse_with_params(
            &format!("{}/release", self.mb_base),
            &[("query", query.as_str()), ("fmt", "json"), ("limit", "10")],
        )
        .map_err(|e| MbError::Failed(format!("MusicBrainz URL: {e}")))?;
        let Some(doc) = self.get_json(&url).await? else {
            return Ok(Vec::new());
        };
        Ok(parse_candidates(&doc))
    }

    /// The release's front cover from Cover Art Archive (500 px), or `None`
    /// when it has none.
    pub async fn front_cover(&self, mbid: &str) -> Result<Option<Vec<u8>>, MusicError> {
        let url = format!("{}/release/{mbid}/front-500", self.caa_base);
        let res = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| MusicError::Http(format!("Cover Art Archive: {e}")))?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !res.status().is_success() {
            return Err(MusicError::Http(format!(
                "Cover Art Archive: HTTP {}",
                res.status()
            )));
        }
        let bytes = res
            .bytes()
            .await
            .map_err(|e| MusicError::Http(format!("Cover Art Archive: {e}")))?;
        Ok(Some(bytes.to_vec()))
    }
}

/// Release candidates from a MusicBrainz search response.
pub fn parse_candidates(doc: &Value) -> Vec<Candidate> {
    let Some(releases) = doc.get("releases").and_then(Value::as_array) else {
        return Vec::new();
    };
    releases
        .iter()
        .filter_map(|r| {
            let artist = r
                .get("artist-credit")
                .and_then(Value::as_array)
                .map(|credits| {
                    credits
                        .iter()
                        .map(|c| {
                            let name = c.get("name").and_then(Value::as_str).unwrap_or("");
                            let join = c.get("joinphrase").and_then(Value::as_str).unwrap_or("");
                            format!("{name}{join}")
                        })
                        .collect::<String>()
                })
                .unwrap_or_default();
            Some(Candidate {
                mbid: r.get("id")?.as_str()?.to_string(),
                title: r.get("title")?.as_str()?.to_string(),
                artist,
                date: r
                    .get("date")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                track_count: r
                    .get("track-count")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok()),
                score: r
                    .get("score")
                    .and_then(Value::as_u64)
                    .map(|s| s.min(100) as u32)
                    .unwrap_or(0),
            })
        })
        .collect()
}

/// How sure we are that `c` is `q`, 0.0-1.0: title and artist similarity
/// (case and spacing ignored), track-count agreement, and MusicBrainz's own
/// score. A deluxe edition or another pressing usually loses on track count.
pub fn confidence(q: &AlbumQuery, c: &Candidate) -> f32 {
    let title = similarity(&group_key(&q.title), &group_key(&c.title));
    let artist = match q.artist.as_deref() {
        Some(a) if !a.trim().is_empty() => similarity(&group_key(a), &group_key(&c.artist)),
        // No artist to compare: neither evidence for nor against.
        _ => 0.5,
    };
    let count = match c.track_count {
        Some(n) if n > 0 && q.track_count > 0 => {
            let (a, b) = (n.min(q.track_count) as f32, n.max(q.track_count) as f32);
            a / b
        }
        _ => 0.5,
    };
    0.35 * title + 0.25 * artist + 0.25 * count + 0.15 * (c.score as f32 / 100.0)
}

/// The best candidate at or above `min_confidence`, with its confidence.
pub fn best_match(
    q: &AlbumQuery,
    candidates: &[Candidate],
    min_confidence: f32,
) -> Option<(Candidate, f32)> {
    candidates
        .iter()
        .map(|c| (c, confidence(q, c)))
        .filter(|(_, conf)| *conf >= min_confidence)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, conf)| (c.clone(), conf))
}

/// 1.0 for equal strings, falling toward 0.0 with edit distance.
pub fn similarity(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    1.0 - prev[b.len()] as f32 / longest as f32
}

/// Text for a Lucene phrase query: backslashes and quotes escaped.
fn lucene_phrase(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn retry_after(res: &reqwest::Response) -> Option<Duration> {
    res.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|s| Duration::from_secs(s).min(MAX_BACKOFF))
}

/// A random-enough delay in `[0, max)`, so installs don't fire in lockstep.
fn jitter(max: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as u64;
    let max_ms = max.as_millis().max(1) as u64;
    Duration::from_millis(nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15) % max_ms)
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A local stand-in for MusicBrainz and Cover Art Archive, for tests.
#[cfg(test)]
pub(crate) mod stub {
    use std::sync::{Arc, Mutex};

    use axum::{
        extract::{Path, Query, State},
        http::{header, StatusCode},
        response::{IntoResponse, Response},
        routing::get,
        Router,
    };
    use serde_json::{json, Value};
    use tokio::time::Instant;

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Mode {
        Ok,
        /// 503 with Retry-After: 0 (MusicBrainz throttling us).
        Busy,
        /// 503 once, then normal.
        BusyOnce,
        /// 500: the service is down.
        Down,
        /// 400: a bad request.
        Bad,
    }

    #[derive(Clone)]
    pub struct Stub {
        pub hits: Arc<Mutex<Vec<(Instant, String, String)>>>,
        pub mode: Arc<Mutex<Mode>>,
        /// Releases returned for every search.
        pub releases: Arc<Mutex<Value>>,
        pub cover_hits: Arc<Mutex<u32>>,
    }

    pub const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];

    impl Stub {
        pub fn search_hits(&self) -> usize {
            self.hits.lock().unwrap().len()
        }
    }

    pub fn release(
        id: &str,
        title: &str,
        artist: &str,
        date: &str,
        tracks: u32,
        score: u32,
    ) -> Value {
        json!({
            "id": id, "score": score, "title": title, "date": date, "track-count": tracks,
            "artist-credit": [{ "name": artist, "joinphrase": "" }]
        })
    }

    async fn search(
        State(s): State<Stub>,
        headers: axum::http::HeaderMap,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> Response {
        let ua = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        s.hits.lock().unwrap().push((
            Instant::now(),
            ua,
            q.get("query").cloned().unwrap_or_default(),
        ));
        let mode = *s.mode.lock().unwrap();
        match mode {
            Mode::Busy => (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "0")],
            )
                .into_response(),
            Mode::BusyOnce => {
                *s.mode.lock().unwrap() = Mode::Ok;
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::RETRY_AFTER, "0")],
                )
                    .into_response()
            }
            Mode::Down => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            Mode::Bad => StatusCode::BAD_REQUEST.into_response(),
            Mode::Ok => {
                let releases = s.releases.lock().unwrap().clone();
                axum::Json(json!({ "releases": releases })).into_response()
            }
        }
    }

    async fn cover(State(s): State<Stub>, Path(mbid): Path<String>) -> Response {
        *s.cover_hits.lock().unwrap() += 1;
        if mbid.starts_with("nocover") {
            StatusCode::NOT_FOUND.into_response()
        } else {
            ([(header::CONTENT_TYPE, "image/jpeg")], JPEG).into_response()
        }
    }

    /// Start the stub; returns it and its base URL.
    pub async fn start(releases: Value) -> (Stub, String) {
        let stub = Stub {
            hits: Arc::default(),
            mode: Arc::new(Mutex::new(Mode::Ok)),
            releases: Arc::new(Mutex::new(releases)),
            cover_hits: Arc::default(),
        };
        let app = Router::new()
            .route("/ws/2/release", get(search))
            .route("/caa/release/{mbid}/front-500", get(cover))
            .with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (stub, format!("http://{addr}"))
    }

    /// A client pointed at the stub, with a short interval.
    pub fn client(pool: sqlx::SqlitePool, base: &str, interval_ms: u64) -> super::MbClient {
        super::MbClient::with_bases(
            pool,
            &format!("{base}/ws/2"),
            &format!("{base}/caa"),
            std::time::Duration::from_millis(interval_ms),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::stub::{self, Mode};
    use super::*;

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("t.db")).await.unwrap();
        (dir, pool)
    }

    fn query(title: &str, artist: &str, tracks: u32) -> AlbumQuery {
        AlbumQuery {
            title: title.into(),
            artist: Some(artist.into()),
            track_count: tracks,
        }
    }

    #[test]
    fn identifies_itself_and_stays_under_one_request_a_second() {
        assert!(USER_AGENT.starts_with("Kahawai/"));
        assert!(USER_AGENT.contains("( https://github.com/ksuayan/kahawai )"));
        assert!(MIN_INTERVAL >= Duration::from_secs(1));
    }

    #[test]
    fn parses_search_results() {
        let doc = serde_json::json!({ "releases": [
            { "id": "r1", "score": 100, "title": "Time Out", "date": "1959-12-14", "track-count": 7,
              "artist-credit": [ { "name": "Dave Brubeck", "joinphrase": " & " }, { "name": "Paul Desmond" } ] },
            { "title": "missing id is skipped" }
        ]});
        let c = parse_candidates(&doc);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].artist, "Dave Brubeck & Paul Desmond");
        assert_eq!((c[0].track_count, c[0].score), (Some(7), 100));
    }

    #[test]
    fn confidence_rewards_matching_title_artist_and_track_count() {
        let q = query("Time Out", "The Dave Brubeck Quartet", 7);
        let exact = Candidate {
            mbid: "a".into(),
            title: "Time Out".into(),
            artist: "the dave brubeck quartet".into(),
            date: "1959".into(),
            track_count: Some(7),
            score: 100,
        };
        assert!(confidence(&q, &exact) > 0.99, "{}", confidence(&q, &exact));
        let deluxe = Candidate {
            track_count: Some(14),
            ..exact.clone()
        };
        assert!(
            confidence(&q, &deluxe) < 0.9,
            "a 14-track edition isn't our 7-track album"
        );
        let other = Candidate {
            title: "Time Further Out".into(),
            ..exact.clone()
        };
        assert!(confidence(&q, &other) < confidence(&q, &exact));
        assert_eq!(
            best_match(&q, &[deluxe.clone(), exact.clone()], 0.9)
                .unwrap()
                .0,
            exact
        );
        assert!(
            best_match(&q, &[deluxe], 0.9).is_none(),
            "below the threshold: no guess"
        );
    }

    #[test]
    fn similarity_is_one_for_equal_and_falls_with_edits() {
        assert_eq!(similarity("time out", "time out"), 1.0);
        assert!(similarity("time out", "time outt") > 0.85);
        assert!(similarity("time out", "kind of blue") < 0.4);
    }

    /// The acceptance check: requests are spaced at least the interval
    /// apart, carry our User-Agent, and a repeated request is served from
    /// the cache without reaching the server.
    #[tokio::test]
    async fn spaces_requests_sends_the_user_agent_and_caches() {
        let (_d, pool) = pool().await;
        let (stub, base) = stub::start(serde_json::json!([])).await;
        let client = stub::client(pool, &base, 200);
        for t in ["A", "B", "C", "A", "B"] {
            client.search_releases(&query(t, "X", 1)).await.unwrap();
        }
        let hits = stub.hits.lock().unwrap().clone();
        assert_eq!(hits.len(), 3, "the repeats came from mb_cache");
        for w in hits.windows(2) {
            let gap = w[1].0 - w[0].0;
            assert!(gap >= Duration::from_millis(200), "requests {gap:?} apart");
        }
        assert!(hits.iter().all(|(_, ua, _)| ua == USER_AGENT));
        assert!(hits[0].2.contains("release:\"A\"") && hits[0].2.contains("artist:\"X\""));
    }

    #[tokio::test]
    async fn a_busy_server_is_retried_then_reported_unreachable() {
        let (_d, pool) = pool().await;
        let (stub, base) = stub::start(serde_json::json!([])).await;
        let client = stub::client(pool, &base, 10);
        *stub.mode.lock().unwrap() = Mode::BusyOnce;
        assert!(
            client.search_releases(&query("A", "X", 1)).await.is_ok(),
            "one 503, then fine"
        );
        *stub.mode.lock().unwrap() = Mode::Busy;
        let e = client
            .search_releases(&query("B", "X", 1))
            .await
            .unwrap_err();
        assert!(matches!(e, MbError::Unreachable(_)), "{e}");
        assert_eq!(stub.search_hits(), 2 + MAX_RETRIES as usize + 1);
    }

    #[tokio::test]
    async fn down_or_offline_is_unreachable_but_a_bad_request_is_not() {
        let (_d, pool) = pool().await;
        let (stub, base) = stub::start(serde_json::json!([])).await;
        let client = stub::client(pool.clone(), &base, 10);
        *stub.mode.lock().unwrap() = Mode::Down;
        assert!(matches!(
            client.search_releases(&query("A", "X", 1)).await,
            Err(MbError::Unreachable(_))
        ));
        *stub.mode.lock().unwrap() = Mode::Bad;
        assert!(matches!(
            client.search_releases(&query("B", "X", 1)).await,
            Err(MbError::Failed(_))
        ));
        // No internet: nothing listens on port 1.
        let offline = stub::client(pool, "http://127.0.0.1:1", 10);
        assert!(matches!(
            offline.search_releases(&query("C", "X", 1)).await,
            Err(MbError::Unreachable(_))
        ));
    }
}

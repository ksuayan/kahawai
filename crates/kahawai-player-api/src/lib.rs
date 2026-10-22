//! Async HTTP client for the music server API.
//! (Spec §3.3, §3.7.)
//!
//! The Tauri frontend talks to the server directly for browse/read APIs;
//! this crate is the Rust-side client used by the shell (e.g. resolving a
//! track id to full [`Track`] metadata before playback). All JSON types are
//! reused from `kahawai_core::api` — there is exactly one contract.
//!
//! No Tauri or platform imports: this crate is plain async Rust.

use kahawai_core::{
    api::{
        Album, Artist, CatalogDelta, CatalogSnapshot, Genre, Job, NewPlaylist, Page, Playlist,
        SetPlaylistTracks, StreamFormat, Track,
    },
    MusicError, ServerIdentity, KAHAWAI_SERVICE,
};

/// Options for [`Client::stream_url`]. Mirrors the server's `StreamQuery`.
#[derive(Debug, Clone, Default)]
pub struct StreamUrlOptions {
    pub format: Option<StreamFormat>,
    pub seek_ms: Option<u64>,
    pub next: Option<i64>,
}

/// Error from [`Client::login`]: distinguishes bad credentials (and
/// throttling) from transport failures so the UI can say the right thing.
#[derive(Debug)]
pub enum LoginError {
    /// 401: wrong username/password (or wrong/retired pairing token).
    Unauthorized,
    /// 429: too many attempts; retry after the given seconds.
    Throttled(u64),
    /// Anything else (network, server error, bad response shape).
    Other(MusicError),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::Unauthorized => write!(f, "invalid credentials"),
            LoginError::Throttled(secs) => {
                write!(f, "too many login attempts — try again in {secs}s")
            }
            LoginError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoginError {}

/// True when `err` is a `MusicError::Http` produced by this client for a
/// 401 response — i.e. the stored credential is wrong, expired, or retired
/// ("session expired — log in again" / "server token changed").
pub fn is_unauthorized(err: &MusicError) -> bool {
    matches!(err, MusicError::Http(m) if m.starts_with("server returned 401"))
}

/// Async client for one server base URL, e.g. `http://localhost:8080`.
///
/// Carries one credential: either the Phase 1 pairing token or (once user
/// accounts exist) a Phase 3 session token. The credential is sent as
/// `Authorization: Bearer` on every JSON request, and as `?access_token=`
/// on the URL-only surfaces (`<audio>`/`<img>` tags can't set headers —
/// see [`Client::stream_url`] / [`Client::artwork_url`]).
#[derive(Debug, Clone)]
pub struct Client {
    base: String,
    http: reqwest::Client,
    token: Option<String>,
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
            token: None,
        }
    }

    /// Attach the credential (pairing token or session token).
    pub fn set_token(&mut self, token: impl Into<String>) {
        self.token = Some(token.into());
    }

    /// Forget the credential (logout).
    pub fn clear_token(&mut self) {
        self.token = None;
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Add `Authorization: Bearer` when a credential is set.
    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, MusicError> {
        let resp = self
            .authed(self.http.get(self.url(path)))
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    /// The `?access_token=` suffix for URL-only surfaces, or "" when no
    /// credential is set. Percent-encoding the token keeps it one query
    /// value even though generated tokens are already URL-safe.
    fn token_param(&self, first: bool) -> String {
        match &self.token {
            Some(t) => format!(
                "{}access_token={}",
                if first { '?' } else { '&' },
                urlencode(t)
            ),
            None => String::new(),
        }
    }

    // -- auth (P0 §1.3, Phase 3 §3.2–§3.3) -----------------------------------

    /// `POST /api/login` → session token. Returns the token; the caller
    /// stores it (OS keychain) and hands it to [`Client::set_token`].
    pub async fn login(&self, username: &str, password: &str) -> Result<String, LoginError> {
        let body = serde_json::json!({ "username": username, "password": password });
        let resp = self
            .http
            .post(self.url("/api/login"))
            .json(&body)
            .send()
            .await
            .map_err(|e| LoginError::Other(reqwest_err(e)))?;
        match resp.status() {
            s if s.is_success() => {
                let v: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| LoginError::Other(reqwest_err(e)))?;
                v.get("token")
                    .and_then(|t| t.as_str())
                    .map(|t| t.to_string())
                    .ok_or_else(|| {
                        LoginError::Other(MusicError::Http("login: missing token".to_string()))
                    })
            }
            s if s == reqwest::StatusCode::UNAUTHORIZED => Err(LoginError::Unauthorized),
            s if s == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                let retry = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60);
                Err(LoginError::Throttled(retry))
            }
            _ => Err(LoginError::Other(http_status_error(resp).await)),
        }
    }

    /// `POST /api/logout`: deletes the presenting session server-side.
    pub async fn logout(&self) -> Result<(), MusicError> {
        let resp = self
            .authed(self.http.post(self.url("/api/logout")))
            .send()
            .await
            .map_err(reqwest_err)?;
        ok_or_error(resp).await
    }

    /// `GET /api/setup/status`: does the server still need its first admin?
    pub async fn setup_needed(&self) -> Result<bool, MusicError> {
        let v: serde_json::Value = self.get("/api/setup/status").await?;
        Ok(v.get("setup_needed")
            .and_then(|b| b.as_bool())
            .unwrap_or(false))
    }

    /// `POST /api/setup`: create the first admin with the pairing token.
    pub async fn create_first_admin(
        &self,
        username: &str,
        password: &str,
        pairing_token: &str,
    ) -> Result<(), MusicError> {
        let body = serde_json::json!({
            "username": username,
            "password": password,
            "pairing_token": pairing_token,
        });
        let resp = self
            .http
            .post(self.url("/api/setup"))
            .json(&body)
            .send()
            .await
            .map_err(reqwest_err)?;
        ok_or_error(resp).await
    }

    // -- health --------------------------------------------------------

    pub async fn health(&self) -> Result<serde_json::Value, MusicError> {
        self.get("/api/health").await
    }

    /// `GET /api/identity`: which Kahawai server this is. Errors when the
    /// address answers but isn't a Kahawai server.
    pub async fn identity(&self) -> Result<ServerIdentity, MusicError> {
        let id: ServerIdentity = self.get("/api/identity").await?;
        if id.service != KAHAWAI_SERVICE {
            return Err(MusicError::Http(format!(
                "{} is not a Kahawai server (it says it is \"{}\")",
                self.base, id.service
            )));
        }
        Ok(id)
    }

    // -- browse (S2) -----------------------------------------------------

    pub async fn albums(&self, page: u64, per_page: u64) -> Result<Page<Album>, MusicError> {
        let resp = self
            .authed(self.http.get(self.url("/api/albums")))
            .query(&[("page", page), ("per_page", per_page)])
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    pub async fn album(&self, id: i64) -> Result<Album, MusicError> {
        self.get(&format!("/api/albums/{id}")).await
    }

    pub async fn artists(&self) -> Result<Vec<Artist>, MusicError> {
        self.get("/api/artists").await
    }

    pub async fn artist(&self, id: i64) -> Result<Artist, MusicError> {
        self.get(&format!("/api/artists/{id}")).await
    }

    pub async fn track(&self, id: i64) -> Result<Track, MusicError> {
        self.get(&format!("/api/tracks/{id}")).await
    }

    pub async fn search(&self, q: &str) -> Result<Vec<Track>, MusicError> {
        let resp = self
            .authed(self.http.get(self.url("/api/search")))
            .query(&[("q", q)])
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    // -- catalog cache -------------------------------------------------------

    /// The whole catalog with its revision (`GET /api/catalog`).
    pub async fn catalog(&self) -> Result<CatalogSnapshot, MusicError> {
        self.get("/api/catalog").await
    }

    /// What changed after revision `since` of database `catalog_id`.
    pub async fn catalog_delta(
        &self,
        since: i64,
        catalog_id: &str,
    ) -> Result<CatalogDelta, MusicError> {
        let resp = self
            .http
            .get(self.url("/api/catalog/delta"))
            .query(&[
                ("since", since.to_string()),
                ("catalog_id", catalog_id.to_string()),
            ])
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    // -- genres -------------------------------------------------------------

    /// Canonical genres with track counts, most tracks first.
    pub async fn genres(&self) -> Result<Vec<Genre>, MusicError> {
        self.get("/api/genres").await
    }

    /// One page of a genre's tracks.
    pub async fn genre_tracks(
        &self,
        name: &str,
        page: u64,
        per_page: u64,
    ) -> Result<Page<Track>, MusicError> {
        let mut url = reqwest::Url::parse(&self.url("/api/genres"))
            .map_err(|e| MusicError::Http(format!("bad server URL: {e}")))?;
        // Path-segment encoding: "R&B", "Children's", "Drum & Bass".
        url.path_segments_mut()
            .map_err(|_| MusicError::Http("bad server URL".to_string()))?
            .push(name)
            .push("tracks");
        let resp = self
            .http
            .get(url)
            .query(&[("page", page), ("per_page", per_page)])
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    // -- playlists (S7, S11) -----------------------------------------------

    pub async fn playlists(&self) -> Result<Vec<Playlist>, MusicError> {
        self.get("/api/playlists").await
    }

    pub async fn create_playlist(&self, body: &NewPlaylist) -> Result<Playlist, MusicError> {
        let resp = self
            .authed(self.http.post(self.url("/api/playlists")))
            .json(body)
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    pub async fn delete_playlist(&self, id: i64) -> Result<(), MusicError> {
        let resp = self
            .authed(self.http.delete(self.url(&format!("/api/playlists/{id}"))))
            .send()
            .await
            .map_err(reqwest_err)?;
        ok_or_error(resp).await
    }

    pub async fn set_playlist_tracks(
        &self,
        id: i64,
        body: &SetPlaylistTracks,
    ) -> Result<Playlist, MusicError> {
        let resp = self
            .authed(
                self.http
                    .put(self.url(&format!("/api/playlists/{id}/tracks"))),
            )
            .json(body)
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    /// Import a playlist file that already lives on the server.
    pub async fn import_playlist(
        &self,
        name: Option<&str>,
        path: &str,
    ) -> Result<serde_json::Value, MusicError> {
        let body = serde_json::json!({ "name": name, "path": path });
        let resp = self
            .authed(self.http.post(self.url("/api/playlists/import")))
            .json(&body)
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    // -- artwork -----------------------------------------------------------

    /// Direct URL for an `<img>` tag; no request is made. Carries
    /// `?access_token=` when a credential is set — `<img>` tags can't
    /// send an `Authorization` header (spec §1.3).
    pub fn artwork_url(&self, hash: &str) -> String {
        format!(
            "{}{}",
            self.url(&format!("/api/artwork/{hash}")),
            self.token_param(true)
        )
    }

    // -- jobs & scan (S9) ----------------------------------------------------

    pub async fn jobs(&self) -> Result<Vec<Job>, MusicError> {
        self.get("/api/jobs").await
    }

    pub async fn job(&self, id: &str) -> Result<Job, MusicError> {
        self.get(&format!("/api/jobs/{id}")).await
    }

    /// `POST /api/scan` → 202 with the job payload.
    pub async fn trigger_scan(&self) -> Result<serde_json::Value, MusicError> {
        let resp = self
            .authed(self.http.post(self.url("/api/scan")))
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    // -- streaming (S3, S12, S13, S8) ------------------------------------------

    /// Build a `/stream/:id` URL. Pure string building — no request.
    /// The frontend uses these directly in `<audio>` tags or fetch calls.
    pub fn stream_url(&self, id: i64, opts: &StreamUrlOptions) -> String {
        let mut url = format!("{}/stream/{id}", self.base);
        let mut first = true;
        let mut push = |k: &str, v: String| {
            url.push(if first { '?' } else { '&' });
            first = false;
            url.push_str(k);
            url.push('=');
            url.push_str(&v);
        };
        if let Some(f) = opts.format {
            // StreamFormat serializes snake_case: passthrough|flac|opus|mp3|dop.
            let s = serde_json::to_string(&f).expect("StreamFormat serializes");
            push("format", s.trim_matches('"').to_string());
        }
        if let Some(ms) = opts.seek_ms {
            push("seek_ms", ms.to_string());
        }
        if let Some(n) = opts.next {
            push("next", n.to_string());
        }
        // URL-only surface (`<audio>` tags can't set headers): the token
        // rides as a query param (spec §1.3 documents the tradeoff).
        if self.token.is_some() {
            push("access_token", urlencode(self.token.as_deref().unwrap()));
        }
        url
    }
}

fn reqwest_err(e: reqwest::Error) -> MusicError {
    MusicError::Http(e.to_string())
}

async fn http_status_error(resp: reqwest::Response) -> MusicError {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    MusicError::Http(format!("server returned {status}: {body}"))
}

/// Minimal percent-encoding for query values. Generated tokens are
/// base64url (already safe), so this is belt-and-braces.
fn urlencode(s: &str) -> String {
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

async fn json_or_error<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, MusicError> {
    let status = resp.status();
    if status.is_success() {
        resp.json().await.map_err(reqwest_err)
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(MusicError::Http(format!(
            "server returned {status}: {body}"
        )))
    }
}

async fn ok_or_error(resp: reqwest::Response) -> Result<(), MusicError> {
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(MusicError::Http(format!(
            "server returned {status}: {body}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_url_carries_token_param() {
        let mut c = Client::new("http://localhost:8080");
        let plain = c.stream_url(7, &StreamUrlOptions::default());
        assert_eq!(plain, "http://localhost:8080/stream/7");
        c.set_token("tok123");
        let authed = c.stream_url(7, &StreamUrlOptions::default());
        assert_eq!(authed, "http://localhost:8080/stream/7?access_token=tok123");
        // Combines with existing params via &.
        let opts = StreamUrlOptions {
            seek_ms: Some(5000),
            ..Default::default()
        };
        let with_seek = c.stream_url(7, &opts);
        assert!(with_seek.contains("seek_ms=5000&access_token=tok123"));
        c.clear_token();
        assert_eq!(
            c.stream_url(7, &StreamUrlOptions::default()),
            "http://localhost:8080/stream/7"
        );
    }

    #[test]
    fn artwork_url_carries_token_param() {
        let mut c = Client::new("http://localhost:8080/");
        assert_eq!(
            c.artwork_url("abc"),
            "http://localhost:8080/api/artwork/abc"
        );
        c.set_token("tok123");
        assert_eq!(
            c.artwork_url("abc"),
            "http://localhost:8080/api/artwork/abc?access_token=tok123"
        );
    }

    #[test]
    fn unauthorized_detection() {
        let e = MusicError::Http(
            "server returned 401 Unauthorized: {\"error\":\"unauthorized\"}".to_string(),
        );
        assert!(is_unauthorized(&e));
        assert!(!is_unauthorized(&MusicError::Http(
            "server returned 200 OK".to_string()
        )));
        assert!(!is_unauthorized(&MusicError::NotFound("x".to_string())));
    }

    #[test]
    fn login_error_display() {
        assert_eq!(LoginError::Unauthorized.to_string(), "invalid credentials");
        assert!(LoginError::Throttled(30).to_string().contains("30s"));
    }
}

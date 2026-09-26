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
        Album, Artist, Job, NewPlaylist, Page, Playlist, SetPlaylistTracks, StreamFormat, Track,
    },
    MusicError,
};

/// Options for [`Client::stream_url`]. Mirrors the server's `StreamQuery`.
#[derive(Debug, Clone, Default)]
pub struct StreamUrlOptions {
    pub format: Option<StreamFormat>,
    pub seek_ms: Option<u64>,
    pub next: Option<i64>,
}

/// Async client for one server base URL, e.g. `http://localhost:8080`.
#[derive(Debug, Clone)]
pub struct Client {
    base: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, MusicError> {
        let resp = self
            .http
            .get(self.url(path))
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    // -- health --------------------------------------------------------

    pub async fn health(&self) -> Result<serde_json::Value, MusicError> {
        self.get("/api/health").await
    }

    // -- browse (S2) -----------------------------------------------------

    pub async fn albums(&self, page: u64, per_page: u64) -> Result<Page<Album>, MusicError> {
        let resp = self
            .http
            .get(self.url("/api/albums"))
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
            .http
            .get(self.url("/api/search"))
            .query(&[("q", q)])
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
            .http
            .post(self.url("/api/playlists"))
            .json(body)
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    pub async fn delete_playlist(&self, id: i64) -> Result<(), MusicError> {
        let resp = self
            .http
            .delete(self.url(&format!("/api/playlists/{id}")))
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
            .http
            .put(self.url(&format!("/api/playlists/{id}/tracks")))
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
            .http
            .post(self.url("/api/playlists/import"))
            .json(&body)
            .send()
            .await
            .map_err(reqwest_err)?;
        json_or_error(resp).await
    }

    // -- artwork -----------------------------------------------------------

    /// Direct URL for an `<img>` tag; no request is made.
    pub fn artwork_url(&self, hash: &str) -> String {
        self.url(&format!("/api/artwork/{hash}"))
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
            .http
            .post(self.url("/api/scan"))
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
        url
    }
}

fn reqwest_err(e: reqwest::Error) -> MusicError {
    MusicError::Http(e.to_string())
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

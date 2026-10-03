//! Listening to podcasts (docs/v2/kahawai-podcast-spec.md, D3's streaming and
//! D4): episodes play through `/stream/:id` under their own track ids
//! ([`kahawai_core::podcast_track_id`]), from the downloaded file or straight
//! from the podcast's host; positions, sessions and the 97% played rule are the
//! audiobooks'.

use std::time::Duration;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use kahawai_core::MusicError;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use tokio_stream::wrappers::ReceiverStream;

use crate::api::{ApiError, TRANSCODE_CHAIN_HEADER};
use crate::audiobooks as ab;
use crate::audiobooks_api::Session;
use crate::db::cvt;
use crate::podcast_api::{episode_from_row, episodes_from_rows, Episode, EPISODE_SELECT};
use crate::podcast_dl;
use crate::podcasts::{self, FeedRow};
use crate::AppState;

type ApiResult<T> = Result<T, ApiError>;

/// What `/stream` reports produced the bytes of an episode.
const EPISODE_CHAIN: &str = "podcast->passthrough";

fn mime_for(enclosure_type: Option<&str>, path_or_url: &str) -> &'static str {
    match podcast_dl::extension_for(enclosure_type, path_or_url) {
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        _ => "audio/mpeg",
    }
}

/// `/stream/:id` for an episode: the downloaded file with byte ranges, or the
/// podcast's own file passed through (its Range answered by the host), so an
/// episode plays and seeks whether or not it is downloaded. `head` answers
/// without fetching anything from the host.
pub(crate) async fn stream_episode(
    s: &AppState,
    episode_id: i64,
    headers: &HeaderMap,
    head: bool,
) -> ApiResult<Response> {
    let row = sqlx::query(
        "SELECT file_path, enclosure_url, enclosure_type FROM podcast_episodes WHERE id = ?",
    )
    .bind(episode_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(cvt)?
    .ok_or_else(|| MusicError::NotFound(format!("episode {episode_id}")))?;
    let file_path: Option<String> = row.get(0);
    let url: String = row.get(1);
    let kind: Option<String> = row.get(2);

    let local = match file_path {
        Some(p) => tokio::fs::File::open(&p).await.ok().map(|f| (f, p)),
        None => None,
    };
    let mut res = match local {
        Some((file, path)) => {
            crate::api::serve_ranged(file, headers, mime_for(kind.as_deref(), &path)).await?
        }
        None if head => {
            let mut r = StatusCode::OK.into_response();
            r.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static(mime_for(kind.as_deref(), &url)),
            );
            r.headers_mut()
                .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            r
        }
        None => proxy(&url, kind.as_deref(), headers).await?,
    };
    res.headers_mut().insert(
        TRANSCODE_CHAIN_HEADER,
        HeaderValue::from_static(EPISODE_CHAIN),
    );
    Ok(res)
}

/// Pass the podcast host's file through, with the client's Range. Only a
/// connect timeout and a per-read timeout: an hour-long episode must not be cut
/// off by a whole-request limit.
async fn proxy(url: &str, kind: Option<&str>, headers: &HeaderMap) -> ApiResult<Response> {
    let client = reqwest::Client::builder()
        .user_agent(crate::musicbrainz::USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| MusicError::Http(e.to_string()))?;
    let mut req = client.get(url);
    if let Some(r) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        req = req.header(header::RANGE.as_str(), r);
    }
    let mut upstream = req
        .send()
        .await
        .map_err(|e| MusicError::Http(format!("couldn't reach the podcast's host: {e}")))?;
    let status = upstream.status();
    if status.as_u16() == 416 {
        return Err(MusicError::BadRange.into());
    }
    if !status.is_success() {
        return Err(MusicError::Http(format!(
            "the podcast's host answered HTTP {}",
            status.as_u16()
        ))
        .into());
    }
    let pass = |name: header::HeaderName, upstream: &reqwest::Response| {
        upstream
            .headers()
            .get(name.as_str())
            .and_then(|v| HeaderValue::from_bytes(v.as_bytes()).ok())
    };
    let content_type = pass(header::CONTENT_TYPE, &upstream)
        .filter(|v| v.to_str().is_ok_and(|t| t.starts_with("audio/")))
        .unwrap_or_else(|| HeaderValue::from_static(mime_for(kind, url)));
    let length = pass(header::CONTENT_LENGTH, &upstream);
    let range = pass(header::CONTENT_RANGE, &upstream);
    let ranges = pass(header::ACCEPT_RANGES, &upstream);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(8);
    tokio::spawn(async move {
        loop {
            match upstream.chunk().await {
                Ok(Some(chunk)) => {
                    if tx.send(Ok(chunk)).await.is_err() {
                        return; // the player went away (seek, skip, stop)
                    }
                }
                Ok(None) => return,
                Err(e) => {
                    let _ = tx.send(Err(std::io::Error::other(e.to_string()))).await;
                    return;
                }
            }
        }
    });
    let mut res = Response::new(Body::from_stream(ReceiverStream::new(rx)));
    *res.status_mut() = if status.as_u16() == 206 {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, content_type);
    if let Some(v) = length {
        h.insert(header::CONTENT_LENGTH, v);
    }
    if let Some(v) = range {
        h.insert(header::CONTENT_RANGE, v);
    }
    if let Some(v) = ranges {
        h.insert(header::ACCEPT_RANGES, v);
    }
    Ok(res)
}

// --- one episode ----------------------------------------------------------------

#[derive(Serialize)]
pub struct EpisodeDetail {
    #[serde(flatten)]
    pub episode: Episode,
    pub feed: FeedRow,
}

async fn episode(pool: &SqlitePool, id: i64) -> Result<Episode, MusicError> {
    let row = sqlx::query(&format!("{EPISODE_SELECT} WHERE e.id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("episode {id}")))?;
    Ok(episode_from_row(&row).await)
}

/// `GET /api/podcasts/episodes/{id}`: the episode, where you are in it, and its show.
pub async fn get_episode(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<EpisodeDetail>> {
    let episode = episode(&s.pool, id).await?;
    let feed = podcasts::get_feed(&s.pool, episode.feed_id).await?;
    Ok(Json(EpisodeDetail { episode, feed }))
}

#[derive(Deserialize)]
pub struct PositionBody {
    pub offset_ms: i64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct PositionOut {
    pub offset_ms: i64,
    pub updated_at: i64,
    pub played: bool,
}

/// `PUT /api/podcasts/episodes/{id}/position {offset_ms}`: where the listener is.
/// The Player sends it every 10 s while playing and on pause, stop, seek and close.
pub async fn put_position(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(b): Json<PositionBody>,
) -> ApiResult<Json<PositionOut>> {
    Ok(Json(
        save_position(&s.pool, id, b.offset_ms, podcasts::now_ms()).await?,
    ))
}

/// Save a position: sessions are derived like the audiobooks' (a gap over 15
/// minutes or a new day starts another), and reaching 97% marks it played.
/// Rewinding does not mark it unplayed again; that is the listener's call.
pub async fn save_position(
    pool: &SqlitePool,
    id: i64,
    offset_ms: i64,
    now: i64,
) -> Result<PositionOut, MusicError> {
    let mut tx = pool.begin().await.map_err(cvt)?;
    let ep = sqlx::query("SELECT duration_ms, played_at FROM podcast_episodes WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("episode {id}")))?;
    let duration: i64 = ep.get::<Option<i64>, _>(0).unwrap_or(0);
    let played_at: Option<i64> = ep.get(1);
    let offset = if duration > 0 {
        offset_ms.clamp(0, duration)
    } else {
        offset_ms.max(0)
    };
    let previous: Option<i64> =
        sqlx::query("SELECT offset_ms FROM podcast_positions WHERE episode_id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(cvt)?
            .map(|r| r.get(0));
    sqlx::query(
        "INSERT INTO podcast_positions (episode_id, offset_ms, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(episode_id) DO UPDATE SET offset_ms = excluded.offset_ms, updated_at = excluded.updated_at",
    )
    .bind(id)
    .bind(offset)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(cvt)?;

    let last = sqlx::query(
        "SELECT id, ended_at FROM podcast_sessions WHERE episode_id = ? ORDER BY ended_at DESC, id DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(cvt)?
    .map(|r| (r.get::<i64, _>(0), r.get::<i64, _>(1)));
    match last {
        Some((sid, ended)) if !ab::starts_new_session(ended, now) => {
            sqlx::query("UPDATE podcast_sessions SET ended_at = ?, end_offset_ms = ? WHERE id = ?")
                .bind(now)
                .bind(offset)
                .bind(sid)
                .execute(&mut *tx)
                .await
                .map_err(cvt)?;
        }
        _ => {
            // A save that does not move after a long silence is the app
            // closing, not listening: it opens nothing.
            if previous != Some(offset) {
                sqlx::query(
                    "INSERT INTO podcast_sessions (episode_id, started_at, ended_at, start_offset_ms, end_offset_ms)
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(id)
                .bind(now)
                .bind(now)
                .bind(previous.unwrap_or(offset))
                .bind(offset)
                .execute(&mut *tx)
                .await
                .map_err(cvt)?;
            }
        }
    }

    let played = played_at.is_some() || ab::is_finished(offset, duration);
    if played && played_at.is_none() {
        sqlx::query("UPDATE podcast_episodes SET played_at = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(cvt)?;
    }
    tx.commit().await.map_err(cvt)?;
    Ok(PositionOut {
        offset_ms: offset,
        updated_at: now,
        played,
    })
}

/// `GET /api/podcasts/episodes/{id}/history`: listening sessions, newest first.
pub async fn history(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<Session>>> {
    episode(&s.pool, id).await?;
    let rows = sqlx::query(
        "SELECT id, started_at, ended_at, start_offset_ms, end_offset_ms FROM podcast_sessions
         WHERE episode_id = ? ORDER BY started_at DESC, id DESC",
    )
    .bind(id)
    .fetch_all(&s.pool)
    .await
    .map_err(cvt)?;
    Ok(Json(
        rows.iter()
            .map(|r| {
                let (a, b): (i64, i64) = (r.get("start_offset_ms"), r.get("end_offset_ms"));
                Session {
                    id: r.get("id"),
                    started_at: r.get("started_at"),
                    ended_at: r.get("ended_at"),
                    start_offset_ms: a,
                    end_offset_ms: b,
                    listened_ms: (b - a).max(0),
                }
            })
            .collect(),
    ))
}

/// `GET /api/podcasts/in-progress`: started, unplayed episodes, most recent first.
pub async fn in_progress(State(s): State<AppState>) -> ApiResult<Json<Vec<Episode>>> {
    let rows = sqlx::query(&format!(
        "{EPISODE_SELECT}
         WHERE p.offset_ms > 0 AND e.played_at IS NULL
         ORDER BY p.updated_at DESC LIMIT 20"
    ))
    .fetch_all(&s.pool)
    .await
    .map_err(cvt)?;
    Ok(Json(episodes_from_rows(&rows).await))
}

/// `GET /api/podcasts/downloads`: every downloaded episode with its file size,
/// newest download first.
pub async fn downloads(State(s): State<AppState>) -> ApiResult<Json<Vec<Episode>>> {
    let rows = sqlx::query(&format!(
        "{EPISODE_SELECT}
         WHERE e.file_path IS NOT NULL
         ORDER BY e.downloaded_at DESC, e.id DESC"
    ))
    .fetch_all(&s.pool)
    .await
    .map_err(cvt)?;
    Ok(Json(episodes_from_rows(&rows).await))
}

//! `GET /`: a page listing the library's playlists and albums with links
//! for VLC and other players (see `export.rs`). Opening the server's bare
//! address used to be a 404 — the first thing someone tries in a browser,
//! or in VLC's Open Network.
//!
//! Links are built from the address the page was opened with. When that
//! address only works on the server's own machine (`0.0.0.0`, `localhost`,
//! loopback), the links use the server's network address instead, and the
//! page says so.

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, Uri},
    response::Response,
};
use kahawai_core::MusicError;
use serde::Deserialize;
use sqlx::Row;
use std::fmt::Write as _;

use crate::export::{is_local_only_host, lan_base, request_host, split_host, xml_escape as esc};
use crate::{api::ApiError, db, AppState};

/// Albums per page.
const PER_PAGE: i64 = 100;

#[derive(Debug, Deserialize)]
pub struct HomeQuery {
    /// Album filter: title or artist contains this.
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub page: Option<i64>,
}

struct Item {
    id: i64,
    name: String,
    detail: String,
    tracks: i64,
}

/// `LIKE` pattern for "contains `q`", with `%`, `_` and `\` escaped.
fn contains_pattern(q: &str) -> String {
    let mut p = String::from("%");
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            p.push('\\');
        }
        p.push(c);
    }
    p.push('%');
    p
}

pub async fn home(
    State(s): State<AppState>,
    Query(q): Query<HomeQuery>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, ApiError> {
    let host = request_host(&headers, &uri)?;
    let (name, port) = split_host(&host);
    // Where the links point, and a note when that isn't where the page was
    // opened.
    let (base, note) = if is_local_only_host(name) {
        match lan_base(&s, port) {
            Some(lan) => (
                lan.clone(),
                Some(format!(
                    "You opened this page as <code>{}</code>, which only works on the \
                     server's own computer. The links below use its network address, \
                     <a href=\"{lan}/\">{lan}</a>, so they work from other devices too.",
                    esc(&host)
                )),
            ),
            None => (
                format!("http://{host}"),
                Some(format!(
                    "You opened this page as <code>{}</code>, which only works on the \
                     server's own computer, and its network address couldn't be found. \
                     Open the page with that address instead.",
                    esc(&host)
                )),
            ),
        }
    } else {
        (format!("http://{host}"), None)
    };

    let playlists: Vec<Item> = sqlx::query(
        "SELECT p.id, p.name, (SELECT COUNT(*) FROM playlist_tracks pt \
         WHERE pt.playlist_id = p.id) AS n FROM playlists p ORDER BY p.name COLLATE NOCASE, p.id",
    )
    .fetch_all(&s.pool)
    .await
    .map_err(db::cvt)?
    .iter()
    .map(|r| Item {
        id: r.get("id"),
        name: r.get("name"),
        detail: String::new(),
        tracks: r.get("n"),
    })
    .collect();

    // Albums with at least one playable track, like the album list.
    let filter = q.q.trim();
    let pattern = contains_pattern(filter);
    let present = "a.id IN (SELECT album_id FROM tracks WHERE missing = 0 \
                   AND duplicate_of IS NULL AND album_id IS NOT NULL)";
    let matches = "(?1 = '' OR a.title LIKE ?2 ESCAPE '\\' OR a.artist LIKE ?2 ESCAPE '\\')";
    let total: i64 = sqlx::query(&format!(
        "SELECT COUNT(*) FROM albums a WHERE {present} AND {matches}"
    ))
    .bind(filter)
    .bind(&pattern)
    .fetch_one(&s.pool)
    .await
    .map_err(db::cvt)?
    .get(0);
    let pages = ((total + PER_PAGE - 1) / PER_PAGE).max(1);
    let page = q.page.unwrap_or(1).clamp(1, pages);
    let albums: Vec<Item> = sqlx::query(&format!(
        "SELECT a.id, a.title, a.artist, a.year, \
         (SELECT COUNT(*) FROM tracks t WHERE t.album_id = a.id AND t.missing = 0 \
          AND t.duplicate_of IS NULL) AS n \
         FROM albums a WHERE {present} AND {matches} \
         ORDER BY COALESCE(a.sort_artist, a.artist) COLLATE NOCASE, \
                  COALESCE(a.sort_title, a.title) COLLATE NOCASE, a.id \
         LIMIT ?3 OFFSET ?4"
    ))
    .bind(filter)
    .bind(&pattern)
    .bind(PER_PAGE)
    .bind((page - 1) * PER_PAGE)
    .fetch_all(&s.pool)
    .await
    .map_err(db::cvt)?
    .iter()
    .map(|r| {
        let artist: Option<String> = r.get("artist");
        let year: Option<i64> = r.get("year");
        let detail = match (artist.filter(|a| !a.trim().is_empty()), year) {
            (Some(a), Some(y)) => format!("{a} · {y}"),
            (Some(a), None) => a,
            (None, Some(y)) => y.to_string(),
            (None, None) => String::new(),
        };
        Item {
            id: r.get("id"),
            name: r.get("title"),
            detail,
            tracks: r.get("n"),
        }
    })
    .collect();

    let body = render(
        &base,
        note.as_deref(),
        &playlists,
        &albums,
        filter,
        page,
        pages,
        total,
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(body))
        .map_err(|e| ApiError::from(MusicError::Http(e.to_string())))
}

/// `?q=…&page=…` for an album-list link.
fn list_query(filter: &str, page: i64) -> String {
    let mut out = String::from("?");
    if !filter.is_empty() {
        out.push_str("q=");
        for b in filter.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                out.push(b as char);
            } else {
                let _ = write!(out, "%{b:02X}");
            }
        }
        out.push('&');
    }
    let _ = write!(out, "page={page}");
    out
}

fn rows(out: &mut String, base: &str, kind: &str, items: &[Item]) {
    out.push_str("<ul class=\"rows\">\n");
    for it in items {
        let m3u = format!("{base}/api/{kind}/{}/export?format=m3u", it.id);
        let xspf = format!("{base}/api/{kind}/{}/export?format=xspf", it.id);
        let tracks = if it.tracks == 1 {
            "1 track".to_string()
        } else {
            format!("{} tracks", it.tracks)
        };
        let detail = if it.detail.is_empty() {
            tracks
        } else {
            format!("{} · {tracks}", it.detail)
        };
        let _ = writeln!(
            out,
            "<li><div class=\"what\"><span class=\"name\">{}</span>\
             <span class=\"detail\">{}</span></div>\
             <code class=\"link\">{}</code>\
             <span class=\"formats\"><a href=\"{}\">M3U</a> <a href=\"{}\">XSPF</a></span></li>",
            esc(&it.name),
            esc(&detail),
            esc(&m3u),
            esc(&m3u),
            esc(&xspf),
        );
    }
    out.push_str("</ul>\n");
}

#[allow(clippy::too_many_arguments)]
fn render(
    base: &str,
    note: Option<&str>,
    playlists: &[Item],
    albums: &[Item],
    filter: &str,
    page: i64,
    pages: i64,
    total: i64,
) -> String {
    let mut out = String::with_capacity(32 * 1024 + albums.len() * 400);
    out.push_str(concat!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n",
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
        "<title>Kahawai Server</title>\n<style>\n",
        include_str!("home.css"),
        "</style></head><body><main>\n",
        "<h1>Kahawai Server</h1>\n",
        "<p class=\"lede\">To play a playlist or album in VLC, choose <b>Media → Open Network \
         Stream…</b> (on a Mac, <b>File → Open Network…</b>) and paste its link. \
         M3U and XSPF download the same playlist as a file; XSPF adds cover art.</p>\n",
    ));
    if let Some(note) = note {
        let _ = writeln!(out, "<p class=\"note\">{note}</p>");
    }

    out.push_str("<h2>Playlists</h2>\n");
    if playlists.is_empty() {
        out.push_str("<p class=\"empty\">No playlists yet. Make one in Kahawai Player.</p>\n");
    } else {
        rows(&mut out, base, "playlists", playlists);
    }

    let _ = writeln!(
        out,
        "<h2>Albums</h2>\n<form class=\"search\" method=\"get\" action=\"/\">\
         <input type=\"search\" name=\"q\" value=\"{}\" placeholder=\"Album or artist\" \
         aria-label=\"Find an album or artist\"> <button type=\"submit\">Find</button></form>",
        esc(filter)
    );
    if albums.is_empty() {
        let msg = if filter.is_empty() {
            "No albums yet. Scan a music folder from the Kahawai Server app.".to_string()
        } else {
            format!("No album or artist matches “{}”.", esc(filter))
        };
        let _ = writeln!(out, "<p class=\"empty\">{msg}</p>");
    } else {
        rows(&mut out, base, "albums", albums);
        if pages > 1 {
            out.push_str("<nav class=\"pages\">");
            if page > 1 {
                let _ = write!(
                    out,
                    "<a href=\"/{}\">← Previous</a> ",
                    esc(&list_query(filter, page - 1))
                );
            }
            let _ = write!(out, "<span>Page {page} of {pages} · {total} albums</span>");
            if page < pages {
                let _ = write!(
                    out,
                    " <a href=\"/{}\">Next →</a>",
                    esc(&list_query(filter, page + 1))
                );
            }
            out.push_str("</nav>\n");
        }
    }
    out.push_str("</main></body></html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns_escape_wildcards() {
        assert_eq!(contains_pattern("50% off_x\\"), "%50\\% off\\_x\\\\%");
        assert_eq!(contains_pattern(""), "%%");
    }

    #[test]
    fn list_links_encode_the_filter() {
        assert_eq!(list_query("", 2), "?page=2");
        assert_eq!(list_query("Café & co", 3), "?q=Caf%C3%A9%20%26%20co&page=3");
    }
}

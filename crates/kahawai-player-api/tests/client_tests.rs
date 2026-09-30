//! Hermetic tests for [`kahawai_player_api::Client`] against a tiny in-process
//! Axum server. No network beyond loopback.

use std::net::SocketAddr;

use axum::{extract::Path, routing::get, Json, Router};
use kahawai_core::{
    api::{Album, Page, StreamFormat, Track},
    format::AudioFormat,
};
use kahawai_player_api::{Client, StreamUrlOptions};

fn track(id: i64) -> Track {
    Track {
        id,
        path: format!("/m/{id}.flac"),
        hash: Some("h".into()),
        format: AudioFormat::Flac,
        sample_rate: Some(44100),
        bit_depth: Some(16),
        channels: Some(2),
        duration_ms: Some(180_000),
        bitrate: None,
        title: Some(format!("T{id}")),
        album: None,
        artist: None,
        album_id: None,
        track_no: None,
        disc_no: None,
        genre: None,
        year: None,
        missing: false,
        decodable: true,
        mqa: false,
        original_sample_rate: None,
    }
}

fn album(id: i64) -> Album {
    Album {
        id,
        title: format!("A{id}"),
        artist: Some("Artist".into()),
        year: Some(2024),
        artwork_hash: None,
        track_ids: vec![1, 2],
        track_count: 2,
        ..Default::default()
    }
}

async fn test_server() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route(
            "/api/health",
            get(|| async { Json(serde_json::json!({"ok": true})) }),
        )
        .route(
            "/api/albums",
            get(|| async {
                Json(Page {
                    items: vec![album(1), album(2)],
                    page: 1,
                    per_page: 50,
                    total: 2,
                })
            }),
        )
        .route(
            "/api/albums/{id}",
            get(|Path(id): Path<i64>| async move { Json(album(id)) }),
        )
        .route(
            "/api/tracks/{id}",
            get(|Path(id): Path<i64>| async move { Json(track(id)) }),
        )
        .route(
            "/api/playlists",
            get(|| async { Json(Vec::<kahawai_core::api::Playlist>::new()) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn health_albums_track_round_trip() {
    let (base, _srv) = test_server().await;
    let c = Client::new(&base);
    assert_eq!(c.base_url(), base);

    let h = c.health().await.unwrap();
    assert_eq!(h["ok"], true);

    let page = c.albums(1, 50).await.unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.items[0].title, "A1");

    let a = c.album(7).await.unwrap();
    assert_eq!(a.id, 7);

    let t = c.track(42).await.unwrap();
    assert_eq!(t.id, 42);
    assert_eq!(t.title.as_deref(), Some("T42"));

    let pl = c.playlists().await.unwrap();
    assert!(pl.is_empty());
}

#[tokio::test]
async fn not_found_surfaces_as_http_error() {
    let (base, _srv) = test_server().await;
    let c = Client::new(&base);
    let err = c.artist(999).await.unwrap_err();
    assert!(err.to_string().contains("404"), "{err}");
}

#[tokio::test]
async fn stream_url_builds_query_params() {
    let c = Client::new("http://localhost:8080/");
    assert_eq!(
        c.stream_url(5, &StreamUrlOptions::default()),
        "http://localhost:8080/stream/5"
    );
    let url = c.stream_url(
        5,
        &StreamUrlOptions {
            format: Some(StreamFormat::Flac),
            seek_ms: Some(61000),
            next: Some(6),
        },
    );
    assert_eq!(
        url,
        "http://localhost:8080/stream/5?format=flac&seek_ms=61000&next=6"
    );
    // Passthrough and DoP serialize in snake_case per the API contract.
    let url = c.stream_url(
        5,
        &StreamUrlOptions {
            format: Some(StreamFormat::Passthrough),
            ..Default::default()
        },
    );
    assert!(url.contains("format=passthrough"), "{url}");
}

#[tokio::test]
async fn artwork_url_is_stable() {
    let c = Client::new("http://localhost:8080");
    assert_eq!(
        c.artwork_url("abc123"),
        "http://localhost:8080/api/artwork/abc123"
    );
}

import type { Album, PlayerState, PodcastEpisode, Track } from "../types";

let n = 0;

export function makeTrack(over: Partial<Track> = {}): Track {
  n += 1;
  return {
    id: n,
    path: `/music/${n}.flac`,
    hash: `hash${n}`,
    format: "flac",
    sample_rate: 44100,
    bit_depth: 16,
    channels: 2,
    duration_ms: 200_000,
    bitrate: 900,
    title: `Track ${n}`,
    album: "Album",
    artist: "Artist",
    album_id: 1,
    track_no: n,
    disc_no: 1,
    genre: null,
    year: 2020,
    missing: false,
    decodable: true,
    mqa: false,
    original_sample_rate: null,
    ...over,
  } as Track;
}

export function makeAlbum(over: Partial<Album> = {}): Album {
  n += 1;
  return {
    id: n,
    title: `Album ${n}`,
    artist: "Artist",
    year: 2020,
    artwork_hash: `art${n}`,
    track_ids: [],
    track_count: 0,
    ...over,
  } as Album;
}

export function makeState(over: Partial<PlayerState> = {}): PlayerState {
  const track = over.track === undefined ? makeTrack() : over.track;
  return {
    status: "stopped",
    track,
    queue_ids: track ? [track.id] : [],
    queue_index: track ? 0 : null,
    position_ms: 0,
    duration_ms: track?.duration_ms ?? null,
    format: null,
    chain: null,
    output_path: "pcm-shared",
    volume: 1,
    error: null,
    repeat: "off",
    shuffle: false,
    ...over,
  };
}

/** Mock `fetch` with a router: pass `{ "/api/x": body | (init)=>Response }`. */
export function mockFetch(routes: Record<string, unknown | ((url: string, init?: RequestInit) => Response)>) {
  const calls: { url: string; init?: RequestInit }[] = [];
  const fn = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = String(input);
    calls.push({ url, init });
    const path = url.replace(/^https?:\/\/[^/]+/, "");
    const key = Object.keys(routes).find((k) => path === k || path.startsWith(k));
    if (!key) return new Response(JSON.stringify({ error: `no route ${path}` }), { status: 404 });
    const r = routes[key];
    if (typeof r === "function") return (r as (u: string, i?: RequestInit) => Response)(url, init);
    return new Response(JSON.stringify(r), { status: 200, headers: { "content-type": "application/json" } });
  };
  (globalThis as unknown as { fetch: typeof fn }).fetch = fn;
  return calls;
}

/** A podcast episode, 10 minutes long, not started. */
export function makeEpisode(over: Partial<PodcastEpisode> = {}): PodcastEpisode {
  return {
    id: 1,
    feed_id: 3,
    guid: "g1",
    title: "Episode one",
    description_html: null,
    published_at: Date.UTC(2026, 2, 2),
    duration_ms: 600_000,
    enclosure_url: "https://cdn.example/one.mp3",
    enclosure_type: "audio/mpeg",
    enclosure_bytes: 10_000_000,
    image_url: null,
    season: null,
    episode: null,
    link: null,
    downloaded: false,
    file_bytes: null,
    played_at: null,
    dropped_from_feed: false,
    position_ms: 0,
    position_updated_at: null,
    feed_title: "The Show",
    feed_image_url: null,
    ...over,
  };
}

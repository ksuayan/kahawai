/**
 * Pure podcast rules: the track an episode plays as, its format, where to
 * start it, how it reads in a list, and the show notes made safe to show.
 */
import type { PodcastEpisode, Track, TrackFormat } from "../types";

/** Episodes play under ids from here up (the server's PODCAST_TRACK_ID_BASE, 2^40). */
export const PODCAST_TRACK_ID_BASE = 2 ** 40;

export const episodeTrackId = (episodeId: number): number => PODCAST_TRACK_ID_BASE + episodeId;

/** The episode a track id stands for, if it is one. */
export function episodeOfTrack(trackId: number | null | undefined): number | null {
  return trackId != null && trackId >= PODCAST_TRACK_ID_BASE ? trackId - PODCAST_TRACK_ID_BASE : null;
}

export const isEpisodeTrack = (trackId: number | null | undefined): boolean => episodeOfTrack(trackId) !== null;

/** The format from the enclosure's type, else its address. */
export function episodeFormat(type: string | null | undefined, url: string): TrackFormat {
  const t = (type ?? "").toLowerCase();
  const ext = url.split(/[?#]/)[0]!.split(".").pop()?.toLowerCase() ?? "";
  if (t.includes("mp4") || t.includes("m4a") || ext === "m4a" || ext === "mp4") return "m4a";
  if (t.includes("aac") || ext === "aac") return "aac";
  if (t.includes("opus") || ext === "opus") return "opus";
  if (t.includes("ogg") || ext === "ogg" || ext === "oga") return "ogg_vorbis";
  if (t.includes("flac") || ext === "flac") return "flac";
  if (t.includes("wav") || ext === "wav") return "wav";
  return "mp3";
}

/** The queue item an episode plays as: the server streams it, downloaded or not. */
export function episodeTrack(ep: PodcastEpisode): Track {
  return {
    id: episodeTrackId(ep.id),
    path: ep.enclosure_url,
    format: episodeFormat(ep.enclosure_type, ep.enclosure_url),
    title: ep.title,
    artist: ep.feed_title,
    album: ep.feed_title,
    album_id: null,
    genre: "Podcast",
    duration_ms: ep.duration_ms,
    missing: false,
    decodable: true,
  };
}

/** Where to start: the saved place, unless it is played or that close to the end. */
export function startOffset(ep: Pick<PodcastEpisode, "position_ms" | "duration_ms" | "played_at">): number {
  if (ep.played_at != null) return 0;
  const d = ep.duration_ms ?? 0;
  if (d > 0 && ep.position_ms >= d * 0.97) return 0;
  return Math.max(0, ep.position_ms);
}

/** 0 to 1, how far in. */
export function episodeProgress(ep: Pick<PodcastEpisode, "position_ms" | "duration_ms">): number {
  const d = ep.duration_ms ?? 0;
  return d > 0 ? Math.min(1, Math.max(0, ep.position_ms / d)) : 0;
}

/** "Mar 2, 2026" (and "Mar 2" this year). */
export function episodeDate(ms: number | null | undefined, now = Date.now(), locale?: string): string {
  if (ms == null) return "";
  const d = new Date(ms);
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  return d.toLocaleDateString(locale, sameYear ? { month: "short", day: "numeric" } : { year: "numeric", month: "short", day: "numeric" });
}

/** "12.3 MB". */
export function sizeText(bytes: number | null | undefined): string {
  if (bytes == null) return "";
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

const ALLOWED = new Set(["P", "BR", "A", "UL", "OL", "LI", "STRONG", "B", "EM", "I", "U", "BLOCKQUOTE", "H1", "H2", "H3", "H4", "H5", "H6", "CODE", "PRE", "HR", "SPAN", "DIV"]);
const DROP_WITH_CONTENT = new Set(["SCRIPT", "STYLE", "IFRAME", "OBJECT", "EMBED", "NOSCRIPT", "TEMPLATE", "FORM", "SVG", "MATH", "HEAD", "TITLE", "LINK", "META"]);

/** Notes that contain a real tag (not just a "<" in the text). */
const LOOKS_LIKE_HTML = /<\/?(p|br|a|b|i|u|em|strong|ul|ol|li|div|span|h[1-6]|blockquote|code|pre|hr|img|script|style|iframe|table|font)\b[^>]*>|&[a-z]+;|&#\d+;/i;

/** A link the notes may keep: web and mail addresses only. */
export function safeHref(href: string | null | undefined): string | null {
  const h = (href ?? "").trim();
  return /^(https?:|mailto:)/i.test(h) ? h : null;
}

/**
 * Show notes as safe HTML: only text formatting, lists and links survive;
 * scripts, styles, frames, forms, images (which would call the podcast's
 * servers from the app) and every attribute but a link's web or mail address
 * are removed. Plain text (no tags) keeps its line breaks.
 */
export function sanitizeNotes(html: string | null | undefined): string {
  const src = (html ?? "").trim();
  if (!src) return "";
  const doc = new DOMParser().parseFromString(LOOKS_LIKE_HTML.test(src) ? src : escapeText(src).replace(/\n/g, "<br>"), "text/html");
  const out = document.createElement("div");
  for (const node of Array.from(doc.body.childNodes)) copyClean(node, out);
  return out.innerHTML;
}

function escapeText(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function copyClean(node: Node, into: HTMLElement): void {
  if (node.nodeType === 3) {
    into.appendChild(document.createTextNode(node.textContent ?? ""));
    return;
  }
  if (node.nodeType !== 1) return;
  const el = node as Element;
  const tag = el.tagName.toUpperCase();
  if (DROP_WITH_CONTENT.has(tag)) return;
  if (!ALLOWED.has(tag)) {
    // An unknown or unsafe wrapper (img, table, font…): keep what is inside.
    for (const c of Array.from(el.childNodes)) copyClean(c, into);
    return;
  }
  const copy = document.createElement(tag.toLowerCase());
  if (tag === "A") {
    const href = safeHref(el.getAttribute("href"));
    if (href) {
      copy.setAttribute("href", href);
      copy.setAttribute("data-external", "");
    }
  }
  for (const c of Array.from(el.childNodes)) copyClean(c, copy);
  into.appendChild(copy);
}

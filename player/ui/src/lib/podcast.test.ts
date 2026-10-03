import { describe, expect, it } from "vitest";
import { makeEpisode } from "../test/fixtures";
import {
  PODCAST_TRACK_ID_BASE,
  episodeFormat,
  episodeOfTrack,
  episodeProgress,
  episodeTrack,
  episodeTrackId,
  safeHref,
  sanitizeNotes,
  sizeText,
  startOffset,
} from "./podcast";

describe("podcast episodes as tracks", () => {
  it("play under ids from 2^40 up, the same as the server's", () => {
    expect(PODCAST_TRACK_ID_BASE).toBe(1_099_511_627_776);
    expect(episodeOfTrack(episodeTrackId(42))).toBe(42);
    expect(episodeOfTrack(123)).toBeNull();
    expect(episodeOfTrack(-4)).toBeNull();
    expect(episodeOfTrack(null)).toBeNull();
  });

  it("take their format from the enclosure type, then the address", () => {
    expect(episodeFormat("audio/mpeg", "x")).toBe("mp3");
    expect(episodeFormat("audio/x-m4a", "x")).toBe("m4a");
    expect(episodeFormat(null, "https://h/e.m4a?t=1")).toBe("m4a");
    expect(episodeFormat("audio/ogg", "x")).toBe("ogg_vorbis");
    expect(episodeFormat(null, "https://h/e")).toBe("mp3");
  });

  it("carry the episode and the show", () => {
    const t = episodeTrack(makeEpisode({ id: 7 }));
    expect(t).toMatchObject({ id: episodeTrackId(7), title: "Episode one", artist: "The Show", format: "mp3", decodable: true, duration_ms: 600_000 });
  });

  it("start where you left off, unless played or at the very end", () => {
    expect(startOffset(makeEpisode({ position_ms: 120_000 }))).toBe(120_000);
    expect(startOffset(makeEpisode({ position_ms: 590_000 }))).toBe(0);
    expect(startOffset(makeEpisode({ position_ms: 120_000, played_at: 5 }))).toBe(0);
    expect(episodeProgress(makeEpisode({ position_ms: 150_000 }))).toBe(0.25);
    expect(episodeProgress(makeEpisode({ duration_ms: null, position_ms: 9 }))).toBe(0);
  });

  it("show sizes plainly", () => {
    expect(sizeText(2_500_000)).toBe("2.4 MB");
    expect(sizeText(900)).toBe("1 KB");
    expect(sizeText(null)).toBe("");
  });
});

describe("show notes", () => {
  it("keep formatting and web links, and drop scripts, styles, images, handlers and other schemes", () => {
    const html = sanitizeNotes(
      `<p onclick="x()">Hello <b>there</b> <a href="https://show.example/ep" target="_blank" style="color:red">site</a>
       <a href="javascript:alert(1)">bad</a> <a href="file:///etc/passwd">file</a></p>
       <script>alert(1)</script><style>p{}</style><img src="about:blank#tracker">
       <iframe src="about:blank"></iframe><ul><li>one</li></ul>`,
    );
    expect(html).toContain("<p>Hello <b>there</b> <a href=\"https://show.example/ep\" data-external=\"\">site</a>");
    expect(html).toContain("<a>bad</a>");
    expect(html).toContain("<a>file</a>");
    expect(html).toContain("<ul><li>one</li></ul>");
    for (const gone of ["script", "style", "img", "iframe", "onclick", "target", "javascript", "tracker"]) {
      expect(html).not.toContain(gone);
    }
  });

  it("keep plain text's line breaks, escaped", () => {
    expect(sanitizeNotes("Line one\nLine <two>")).toBe("Line one<br>Line &lt;two&gt;");
    expect(sanitizeNotes(null)).toBe("");
  });

  it("allow only web and mail links", () => {
    expect(safeHref(" https://a.example ")).toBe("https://a.example");
    expect(safeHref("mailto:x@y.z")).toBe("mailto:x@y.z");
    expect(safeHref("JavaScript:x")).toBeNull();
    expect(safeHref("/relative")).toBeNull();
  });
});

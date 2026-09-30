import { describe, expect, it } from "vitest";
import { makeAlbum, makeTrack } from "../test/fixtures";
import { sortAlbums, sortTracks } from "./sorting";

const a = makeAlbum({ id: 1, title: "Kind of Blue", artist: "Miles Davis", year: 1959 });
const b = makeAlbum({ id: 2, title: "Abbey Road", artist: "The Beatles", sort_artist: "Beatles, The", year: 1969 });
const c = makeAlbum({ id: 3, title: "Unknown Date", artist: "Anonymous", year: null });
const d = makeAlbum({ id: 4, title: "Bitches Brew", artist: "Miles Davis", year: 1970 });
const ids = (xs: { id: number }[]) => xs.map((x) => x.id);

describe("sortAlbums", () => {
  it("by artist, using sort keys, then year", () => {
    expect(ids(sortAlbums([a, b, c, d], "artist-asc"))).toEqual([3, 2, 1, 4]);
    expect(ids(sortAlbums([a, b, c, d], "artist-desc"))).toEqual([1, 4, 2, 3]);
  });

  it("by album title", () => {
    expect(ids(sortAlbums([a, b, c, d], "title-asc"))).toEqual([2, 4, 1, 3]);
    expect(ids(sortAlbums([a, b, c, d], "title-desc"))).toEqual([3, 1, 4, 2]);
  });

  it("by release year, unknown years last both ways", () => {
    expect(ids(sortAlbums([a, b, c, d], "year-asc"))).toEqual([1, 2, 4, 3]);
    expect(ids(sortAlbums([a, b, c, d], "year-desc"))).toEqual([4, 2, 1, 3]);
  });

  it("doesn't reorder its input", () => {
    const input = [a, b];
    sortAlbums(input, "title-asc");
    expect(ids(input)).toEqual([1, 2]);
  });
});

describe("sortTracks", () => {
  const t1 = makeTrack({ id: 1, artist: "Miles Davis", album: "Kind of Blue", album_id: 1, track_no: 2, year: 1959 });
  const t2 = makeTrack({ id: 2, artist: "Miles Davis", album: "Kind of Blue", album_id: 1, track_no: 1, year: 1959 });
  const t3 = makeTrack({ id: 3, artist: "Bill Evans", album: "Waltz for Debby", album_id: 2, track_no: 1, year: 1961 });
  const t4 = makeTrack({ id: 4, artist: "Anon", album: "Tapes", album_id: 3, track_no: 1, year: null });
  const all = [t1, t2, t3, t4];

  it("keeps the view's own order by default", () => {
    expect(ids(sortTracks(all, "default", (t) => t))).toEqual([1, 2, 3, 4]);
  });

  it("keeps albums in play order within each order", () => {
    expect(ids(sortTracks(all, "artist-asc", (t) => t))).toEqual([4, 3, 2, 1]);
    expect(ids(sortTracks(all, "artist-desc", (t) => t))).toEqual([2, 1, 3, 4]);
    expect(ids(sortTracks(all, "title-asc", (t) => t))).toEqual([2, 1, 4, 3]);
    expect(ids(sortTracks(all, "year-desc", (t) => t))).toEqual([3, 2, 1, 4]);
    expect(ids(sortTracks(all, "year-asc", (t) => t))).toEqual([2, 1, 3, 4]);
  });

  it("sorts wrappers by the track inside", () => {
    const entries = all.map((t, i) => ({ t, i }));
    expect(sortTracks(entries, "artist-asc", (e) => e.t).map((e) => e.i)).toEqual([3, 2, 1, 0]);
  });
});

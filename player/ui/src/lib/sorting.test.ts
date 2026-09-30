import { describe, expect, it } from "vitest";
import { makeAlbum } from "../test/fixtures";
import { sortAlbums } from "./sorting";

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

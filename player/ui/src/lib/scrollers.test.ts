import { describe, expect, it } from "vitest";
import { viewBoxClass } from "./scrollers";

describe("view containers", () => {
  it("a virtualized view gets a bounded flex column; the rest scroll", () => {
    for (const v of ["albums", "artists", "queue", "search", "audiobooks", "album", "genre", "playlist"]) {
      expect(viewBoxClass(v)).toBe("flex flex-col overflow-hidden");
    }
    for (const v of ["settings", "playlists", "genres", "artist", "audiobook"]) {
      expect(viewBoxClass(v)).toBe("overflow-y-auto");
    }
  });
});

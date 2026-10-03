import { describe, expect, it } from "vitest";
import { sortStations } from "./radioSort";

const s = (name: string, codec: string | null, bitrate: number | null) => ({ name, codec, bitrate });
const list = [s("Zeta FM", "MP3", 128), s("alpha radio", "AAC", 64), s("Beta", null, null), s("Gamma", "AAC", 320), s("Delta", "MP3", 320)];
const names = (l: { name: string }[]) => l.map((x) => x.name);

describe("sorting stations", () => {
  it("as listed keeps the order", () => {
    expect(names(sortStations(list, "listed"))).toEqual(["Zeta FM", "alpha radio", "Beta", "Gamma", "Delta"]);
  });
  it("by name ignores case", () => {
    expect(names(sortStations(list, "name"))).toEqual(["alpha radio", "Beta", "Delta", "Gamma", "Zeta FM"]);
  });
  it("by bandwidth puts the highest bitrate first and unknown last", () => {
    expect(names(sortStations(list, "bandwidth"))).toEqual(["Delta", "Gamma", "Zeta FM", "alpha radio", "Beta"]);
  });
  it("by format groups codecs, best bitrate first in each, unknown last", () => {
    expect(names(sortStations(list, "format"))).toEqual(["Gamma", "alpha radio", "Delta", "Zeta FM", "Beta"]);
  });
  it("never changes the list it was given", () => {
    const copy = [...list];
    sortStations(list, "name");
    expect(list).toEqual(copy);
  });
});

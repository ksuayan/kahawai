import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useScrollMemoryStore } from "./scrollMemory";

describe("scroll memory", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("starts at the top and remembers each view separately", () => {
    const m = useScrollMemoryStore();
    expect(m.get("albums")).toBe(0);
    m.set("albums", 1234.6);
    m.set("artists", 50);
    expect(m.get("albums")).toBe(1235);
    expect(m.get("artists")).toBe(50);
  });

  it("forgets a view scrolled back to the top and ignores junk offsets", () => {
    const m = useScrollMemoryStore();
    m.set("albums", 900);
    m.set("albums", 0);
    expect(m.get("albums")).toBe(0);
    m.set("albums", 900);
    m.set("albums", Number.NaN);
    expect(m.get("albums")).toBe(0);
    m.set("albums", -20);
    expect(m.get("albums")).toBe(0);
  });
});

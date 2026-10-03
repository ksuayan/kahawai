import { describe, expect, it } from "vitest";

/** Load the polyfills against a stand-in global that lacks the built-ins. */
describe("polyfills for old WebViews", () => {
  it("leave existing built-ins alone", async () => {
    const before = Array.prototype.at;
    await import("./polyfills");
    expect(Array.prototype.at).toBe(before);
  });

  it("supply what is missing, with the standard behaviour", async () => {
    const saved = { hasOwn: Object.hasOwn, at: Array.prototype.at, toSorted: (Array.prototype as unknown as { toSorted?: unknown }).toSorted };
    // @ts-expect-error simulate an old WebView
    delete Object.hasOwn;
    // @ts-expect-error simulate an old WebView
    delete Array.prototype.at;
    delete (Array.prototype as unknown as { toSorted?: unknown }).toSorted;
    try {
      const { vi } = await import("vitest");
      vi.resetModules();
      await import("./polyfills");
      expect(Object.hasOwn({ a: 1 }, "a")).toBe(true);
      expect(Object.hasOwn(Object.create({ a: 1 }), "a")).toBe(false);
      expect([1, 2, 3].at(-1)).toBe(3);
      expect([1, 2, 3].at(5)).toBeUndefined();
      expect("abc".at(-1)).toBe("c");
      const list = [3, 1, 2];
      expect((list as unknown as { toSorted(): number[] }).toSorted()).toEqual([1, 2, 3]);
      expect(list).toEqual([3, 1, 2]);
      expect(Object.keys(Array.prototype)).not.toContain("at"); // not enumerable
    } finally {
      Object.defineProperty(Object, "hasOwn", { value: saved.hasOwn, writable: true, configurable: true });
      Object.defineProperty(Array.prototype, "at", { value: saved.at, writable: true, configurable: true });
      if (saved.toSorted) Object.defineProperty(Array.prototype, "toSorted", { value: saved.toSorted, writable: true, configurable: true });
    }
  });
});

import { describe, expect, it } from "vitest";
import { joinParts } from "./format";

describe("joinParts", () => {
  it("joins with a middot by default", () => {
    expect(joinParts(["a", "b"])).toBe("a · b");
  });
  it("skips null, undefined, empty string, and false", () => {
    expect(joinParts(["a", null, undefined, "", false, "b"])).toBe("a · b");
  });
  it("takes a custom separator", () => {
    expect(joinParts(["a", "b"], " — ")).toBe("a — b");
  });
  it("returns an empty string when nothing is defined", () => {
    expect(joinParts([null, undefined])).toBe("");
  });
});

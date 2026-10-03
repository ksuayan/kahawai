import postcss from "postcss";
import { describe, expect, it } from "vitest";
import legacyTransforms from "./legacyTransforms";

const run = (css: string) => postcss([legacyTransforms()]).process(css, { from: undefined }).css;
const COMPOSITE =
  "translate(var(--tw-translate-x, 0), var(--tw-translate-y, 0)) rotate(var(--tw-rotate, 0)) scale(var(--tw-scale-x, 1), var(--tw-scale-y, 1))";

describe("legacy transforms (Chromium < 104)", () => {
  it("turns Tailwind's translate into a transform, replacing it", () => {
    const out = run(".t{--tw-translate-x:18px;translate:var(--tw-translate-x) var(--tw-translate-y)}");
    expect(out).toBe(`.t{--tw-translate-x:18px;transform:${COMPOSITE}}`);
  });

  it("carries a literal translate over through the variables", () => {
    expect(run(".t{translate:-50% -50%}")).toBe(
      `.t{--tw-translate-x:-50%;--tw-translate-y:-50%;transform:${COMPOSITE}}`,
    );
  });

  it("does rotate and scale the same way, so they compose with translate", () => {
    expect(run(".r{rotate:45deg}")).toBe(`.r{--tw-rotate:45deg;transform:${COMPOSITE}}`);
    expect(run(".s{scale:var(--tw-scale-x) var(--tw-scale-y)}")).toBe(`.s{transform:${COMPOSITE}}`);
  });

  it("keeps none as none, and leaves transform and transition-property alone", () => {
    expect(run(".n{translate:none}")).toBe(".n{transform:none}");
    const other = ".o{transform:rotate(1deg);transition-property:translate,scale}";
    expect(run(other)).toBe(other);
  });
});

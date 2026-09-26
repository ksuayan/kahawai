import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/** Guardrails for guidelines/Visual-House-Style-Guide.md. */
const root = join(__dirname);
function vueFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? vueFiles(p) : p.endsWith(".vue") ? [p] : [];
  });
}
const sources = vueFiles(root).map((p) => ({ p: p.replace(root, "src"), s: readFileSync(p, "utf8") }));
const offenders = (re: RegExp) => sources.filter(({ s }) => re.test(s)).map(({ p }) => p);

describe("house style", () => {
  it("uses only two font weights (400/600): no medium, bold or italics in components", () => {
    expect(offenders(/\bfont-(medium|bold|extrabold|black|light|thin)\b|\bitalic\b/)).toEqual([]);
  });
  it("uses only the three radii (rounded-sm/md/lg) besides pills", () => {
    expect(offenders(/\brounded-(xl|2xl|3xl)\b|\brounded-\[/)).toEqual([]);
  });
  it("references colour tokens, never raw hex, in components", () => {
    expect(offenders(/(?:text|bg|border|fill|stroke)-\[#[0-9a-fA-F]{3,8}\]/)).toEqual([]);
  });
  it("has one shadow token for floating layers, no ad hoc shadows", () => {
    expect(offenders(/shadow-\[(?!inset)/)).toEqual([]);
  });
  it("bundles only the Plex weights the guide allows", () => {
    const main = readFileSync(join(root, "main.ts"), "utf8");
    expect(main).not.toMatch(/ibm-plex-(sans|serif)\/latin(-ext)?-(500|700)/);
    expect(main).toMatch(/ibm-plex-serif\/latin-400/);
  });
});

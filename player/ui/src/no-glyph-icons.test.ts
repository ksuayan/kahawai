import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Icons are Lucide SVGs. This keeps emoji and text-glyph icons (▶ ✕ ⚙ …)
// from creeping back into component templates.
const root = join(__dirname);
function vueFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? vueFiles(join(dir, e.name)) : e.name.endsWith(".vue") ? [join(dir, e.name)] : [],
  );
}

/** Glyphs that used to stand in for icons. Ordinary punctuation (… → ·) is fine. */
const GLYPH_ICONS = /[▶▷⏸⏹⏮⏭✕✖✎⚙☰⋯⋮⟳▲▼▾▸▴‹›＋♫⚠]/u;
const EMOJI = /\p{Extended_Pictographic}/u;

describe("templates use SVG icons, not emoji or glyph characters", () => {
  for (const file of vueFiles(root)) {
    const name = file.slice(root.length + 1);
    it(name, () => {
      const src = readFileSync(file, "utf8");
      const template = src.slice(src.indexOf("<template>"));
      const stripped = template.replace(/<!--[\s\S]*?-->/g, "");
      expect(stripped.match(EMOJI)?.[0]).toBeUndefined();
      expect(stripped.match(GLYPH_ICONS)?.[0]).toBeUndefined();
    });
  }
});

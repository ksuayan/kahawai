import type { Declaration, Plugin } from "postcss";

/**
 * Build-time only (vite.config.ts, Android legacy builds): Tailwind 4 moves,
 * turns and scales with the individual `translate`, `rotate` and `scale`
 * properties, which Chromium only has from 104. The HiBy R4's WebView is 91,
 * so there a switch's thumb never slid and centred dialogs sat off-centre.
 *
 * Each of those declarations is replaced by one `transform` built from
 * Tailwind's variables, the way Tailwind 3 did it, so classes that combine
 * (a translate and a rotate) still compose. Replaced, not added alongside:
 * a newer WebView would otherwise apply both and move things twice.
 */
const COMPOSITE =
  "translate(var(--tw-translate-x, 0), var(--tw-translate-y, 0)) rotate(var(--tw-rotate, 0)) " +
  "scale(var(--tw-scale-x, 1), var(--tw-scale-y, 1))";

/** `a b` (Tailwind's two-value form) or `a`, split at top-level spaces. */
function pair(value: string): [string, string] {
  const parts: string[] = [];
  let depth = 0;
  let cur = "";
  for (const ch of value.trim()) {
    if (ch === "(") depth++;
    if (ch === ")") depth--;
    if (ch === " " && depth === 0) {
      if (cur) parts.push(cur);
      cur = "";
    } else cur += ch;
  }
  if (cur) parts.push(cur);
  return [parts[0] ?? "0", parts[1] ?? parts[0] ?? "0"];
}

function replace(decl: Declaration, vars: Record<string, string>): void {
  if (decl.value.trim() === "none") {
    decl.replaceWith(decl.clone({ prop: "transform", value: "none" }));
    return;
  }
  for (const [name, value] of Object.entries(vars)) decl.cloneBefore({ prop: name, value });
  decl.replaceWith(decl.clone({ prop: "transform", value: COMPOSITE }));
}

export default function legacyTransforms(): Plugin {
  return {
    postcssPlugin: "kahawai-legacy-transforms",
    Declaration: {
      translate(decl) {
        const [x, y] = pair(decl.value);
        // Tailwind's own form already lives in the variables.
        replace(decl, x === "var(--tw-translate-x)" ? {} : { "--tw-translate-x": x, "--tw-translate-y": y });
      },
      rotate(decl) {
        replace(decl, { "--tw-rotate": decl.value.trim() });
      },
      scale(decl) {
        const [x, y] = pair(decl.value);
        replace(decl, x === "var(--tw-scale-x)" ? {} : { "--tw-scale-x": x, "--tw-scale-y": y });
      },
    },
  };
}

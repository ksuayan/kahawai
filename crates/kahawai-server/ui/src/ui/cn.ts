/** Join class fragments, dropping falsy ones. Later fragments win only by
 *  source order, so variants must not define conflicting utilities. */
export type ClassValue = string | false | null | undefined | ClassValue[] | Record<string, unknown>;

export function cn(...parts: ClassValue[]): string {
  const out: string[] = [];
  for (const p of parts) {
    if (!p) continue;
    if (typeof p === "string") out.push(p);
    else if (Array.isArray(p)) {
      const inner = cn(...p);
      if (inner) out.push(inner);
    } else {
      for (const [k, v] of Object.entries(p)) if (v) out.push(k);
    }
  }
  return out.join(" ");
}

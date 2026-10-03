/** How the Radio screen orders the stations it shows. "listed" keeps the order they came in (your favorites' own order, or the directory's). */
export type RadioSort = "listed" | "name" | "bandwidth" | "format";

export const RADIO_SORTS: { value: RadioSort; label: string }[] = [
  { value: "listed", label: "As listed" },
  { value: "name", label: "Name" },
  { value: "bandwidth", label: "Bandwidth" },
  { value: "format", label: "Format" },
];

export const isRadioSort = (v: unknown): v is RadioSort => RADIO_SORTS.some((s) => s.value === v);

interface Sortable {
  name: string;
  bitrate: number | null;
  codec: string | null;
}

const byName = (a: Sortable, b: Sortable) => a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true });

/**
 * A sorted copy: by name (A to Z), by bandwidth (highest bitrate first, unknown
 * last), or by format (codec A to Z, then highest bitrate). Ties go by name.
 */
export function sortStations<T extends Sortable>(list: T[], by: RadioSort): T[] {
  if (by === "listed") return list;
  const out = [...list];
  if (by === "name") return out.sort(byName);
  if (by === "bandwidth") return out.sort((a, b) => (b.bitrate ?? -1) - (a.bitrate ?? -1) || byName(a, b));
  return out.sort((a, b) => {
    const ca = (a.codec ?? "").trim().toUpperCase();
    const cb = (b.codec ?? "").trim().toUpperCase();
    if (ca !== cb) return !ca ? 1 : !cb ? -1 : ca.localeCompare(cb);
    return (b.bitrate ?? -1) - (a.bitrate ?? -1) || byName(a, b);
  });
}

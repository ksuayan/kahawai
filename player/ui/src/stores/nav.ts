import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { uiGet, uiSet } from "../lib/uiState";

export type ViewName =
  | "albums"
  | "artists"
  | "genres"
  | "playlists"
  | "audiobooks"
  | "podcasts"
  | "radio"
  | "search"
  | "queue"
  | "settings";

/** What each section is called (the sidebar, and the first crumb of the breadcrumb). */
export const SECTION_LABELS: Record<ViewName, string> = {
  albums: "Albums",
  artists: "Artists",
  genres: "Genres",
  playlists: "Playlists",
  audiobooks: "Audiobooks",
  podcasts: "Podcasts",
  radio: "Radio",
  search: "Search",
  queue: "Queue",
  settings: "Settings",
};

export interface NavState {
  name:
    | ViewName
    | "album"
    | "artist"
    | "playlist"
    | "genre"
    | "audiobook"
    | "podcast"
    | "episode"
    | "podcastdownloads"
    | "nowplaying";
  id?: number;
  /** The genre a "genre" view shows (genres are named, not numbered). */
  genre?: string;
}

/** One step of the breadcrumb: a view, and what its page called itself. */
export interface Crumb extends NavState {
  label?: string;
}

const KEY = "kahawai.nav";
const SECTIONS = Object.keys(SECTION_LABELS) as ViewName[];
const BY_ID = ["album", "artist", "playlist", "audiobook", "podcast", "episode"];
/** Pages under a section that take no id. */
const PLAIN_PAGES = ["nowplaying", "podcastdownloads"];
/** The most steps a trail keeps (its section, then the latest pages). */
export const MAX_TRAIL = 6;

/** The section a page belongs to when there is no trail to say otherwise. */
const HOME: Record<Exclude<NavState["name"], ViewName>, ViewName> = {
  album: "albums",
  artist: "artists",
  genre: "genres",
  playlist: "playlists",
  audiobook: "audiobooks",
  podcast: "podcasts",
  episode: "podcasts",
  podcastdownloads: "podcasts",
  nowplaying: "albums",
};

export function isSection(name: NavState["name"]): name is ViewName {
  return (SECTIONS as string[]).includes(name);
}

/** The same page (labels aside). */
export function samePage(a: NavState, b: NavState): boolean {
  return a.name === b.name && (a.id ?? null) === (b.id ?? null) && (a.genre ?? null) === (b.genre ?? null);
}

/** What a crumb shows before its page has named it. */
export function crumbLabel(c: Crumb): string {
  if (isSection(c.name)) return SECTION_LABELS[c.name];
  if (c.label) return c.label;
  if (c.name === "genre" && c.genre) return c.genre;
  if (c.name === "nowplaying") return "Now Playing";
  if (c.name === "podcastdownloads") return "Downloads";
  return c.name.charAt(0).toUpperCase() + c.name.slice(1);
}

/** A saved view or crumb, if it still makes sense. */
function valid(v: unknown): Crumb | null {
  if (!v || typeof v !== "object") return null;
  const c = v as Partial<Crumb>;
  if (typeof c.name !== "string") return null;
  const label = typeof c.label === "string" && c.label ? { label: c.label } : {};
  if (isSection(c.name) || PLAIN_PAGES.includes(c.name)) return { name: c.name, ...label };
  if (BY_ID.includes(c.name) && Number.isInteger(c.id)) return { name: c.name, id: c.id, ...label };
  if (c.name === "genre" && typeof c.genre === "string" && c.genre) return { name: "genre", genre: c.genre, ...label };
  return null;
}

/** The trail for a page reached with no history: its section, then it. */
function freshTrail(v: NavState): Crumb[] {
  return isSection(v.name) ? [{ name: v.name }] : [{ name: HOME[v.name] }, { ...v }];
}

/** The view and trail the app was on when it quit, if they still make sense. */
function lastState(): { view: NavState; trail: Crumb[] } {
  try {
    const saved = JSON.parse(uiGet(KEY) ?? "null") as (Partial<NavState> & { trail?: unknown }) | null;
    const view = valid(saved);
    if (view) {
      const { label: _label, ...page } = view;
      void _label;
      const trail = Array.isArray(saved?.trail) ? saved.trail.map(valid) : [];
      const ok =
        trail.length > 0 &&
        trail.length <= MAX_TRAIL &&
        trail.every((c): c is Crumb => c !== null) &&
        isSection(trail[0]!.name) &&
        samePage(trail[trail.length - 1]!, page);
      return { view: page, trail: ok ? (trail as Crumb[]) : freshTrail(page) };
    }
  } catch {
    // Unreadable: start on Albums.
  }
  return { view: { name: "albums" }, trail: [{ name: "albums" }] };
}

/**
 * Minimal router: the app is a single Tauri window with view state. Besides
 * the view, it keeps the trail that led there (the breadcrumb): a section
 * starts a trail, a page opened from it is a step further, going to a page
 * already in the trail goes back to it, and a page of the same kind as the
 * last step (another album) takes its place. Both are saved on every change,
 * and the app reopens on them.
 */
export const useNavStore = defineStore("nav", () => {
  const start = lastState();
  const view = ref<NavState>(start.view);
  const trail = ref<Crumb[]>(start.trail);

  /** The section the trail starts from: what the sidebar highlights. */
  const section = computed<ViewName>(() => {
    const first = trail.value[0]?.name;
    if (first && isSection(first)) return first;
    return isSection(view.value.name) ? view.value.name : HOME[view.value.name];
  });

  function save(): void {
    uiSet(KEY, JSON.stringify({ ...view.value, trail: trail.value }));
  }

  function go(name: NavState["name"], id?: number, genre?: string): void {
    const target: NavState = genre === undefined ? { name, id } : { name, id, genre };
    view.value = target;
    if (isSection(name)) {
      trail.value = [{ name }];
    } else {
      const at = trail.value.findIndex((c) => samePage(c, target));
      if (at >= 0) {
        trail.value = trail.value.slice(0, at + 1);
      } else {
        const next = trail.value.length ? [...trail.value] : freshTrail(target).slice(0, 1);
        if (next.length > 1 && next[next.length - 1]!.name === name) next.pop(); // a sibling
        next.push({ ...target });
        // Too deep: keep the section and the latest steps.
        trail.value = next.length > MAX_TRAIL ? [next[0]!, ...next.slice(next.length - MAX_TRAIL + 1)] : next;
      }
    }
    save();
  }

  /** Go back to a step of the breadcrumb. */
  function goToCrumb(index: number): void {
    const c = trail.value[index];
    if (!c) return;
    trail.value = trail.value.slice(0, index + 1);
    view.value = c.genre === undefined ? { name: c.name, id: c.id } : { name: c.name, id: c.id, genre: c.genre };
    save();
  }

  /** A page names itself ("Kind of Blue") once it knows its title. */
  function setLabel(page: NavState, label: string): void {
    let changed = false;
    for (const c of trail.value) {
      if (samePage(c, page) && c.label !== label) {
        c.label = label;
        changed = true;
      }
    }
    if (changed) save();
  }

  return { view, trail, section, go, goToCrumb, setLabel };
});

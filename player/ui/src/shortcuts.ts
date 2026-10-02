// Global keyboard shortcuts, kept separate from App.vue so they can be tested.
//
// Reka UI widgets own their keys: a focused slider uses the arrows, a select
// uses arrows and typeahead letters, a menu uses arrows/Enter, a dialog owns
// everything inside it. The global handler must stay out of their way, or one
// key press would both move the slider AND seek/change volume.

import type { NavState } from "./stores/nav";

export interface ShortcutActions {
  toggle(): void;
  seekBy(deltaMs: number): void;
  volumeBy(delta: number): void;
  next(): void;
  prev(): void;
  go(view: NavState["name"]): void;
  /** Audiobook skip, in the book's own seconds: -1 back, 1 forward. Optional; keys j and l. */
  skip?(direction: 1 | -1): void;
  /** Analog warmth A/B: listen to slot A, slot B, or switch. Optional. */
  ab?(which: "a" | "b" | "toggle"): void;
}

const TYPING_TAGS = new Set(["INPUT", "TEXTAREA", "SELECT"]);

/** Roles whose widgets consume arrow/letter keys themselves. */
const OWNER_SELECTOR = [
  '[role="slider"]',
  '[role="combobox"]',
  '[role="listbox"]',
  '[role="option"]',
  '[role="menu"]',
  '[role="menuitem"]',
  '[role="dialog"]',
  '[role="alertdialog"]',
].join(",");

/** True when the event target is somewhere a global shortcut must not fire. */
export function ownsKeyboard(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || !("tagName" in el)) return false;
  if (TYPING_TAGS.has(el.tagName) || el.isContentEditable) return true;
  return typeof el.closest === "function" && el.closest(OWNER_SELECTOR) !== null;
}

/** Space activates a focused button/switch natively; don't also toggle play. */
function activatesNatively(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || !("tagName" in el)) return false;
  return el.tagName === "BUTTON" || el.getAttribute?.("role") === "switch";
}

const VIEW_KEYS: Record<string, NavState["name"]> = {
  "1": "albums",
  "2": "artists",
  "3": "playlists",
  "4": "search",
  "5": "queue",
  "6": "settings",
  "7": "audiobooks",
  f: "search",
  g: "genres",
};

export const SEEK_STEP_MS = 10_000;
export const VOLUME_STEP = 0.05;

/** Returns true when the key was handled (and default-prevented if needed). */
export function handleShortcut(e: KeyboardEvent, a: ShortcutActions): boolean {
  if (e.metaKey || e.ctrlKey || e.altKey) return false;

  if (ownsKeyboard(e.target)) {
    if (e.key === "Escape" && (e.target as HTMLElement).tagName === "INPUT") {
      (e.target as HTMLElement).blur();
      return true;
    }
    return false;
  }

  switch (e.key) {
    case " ":
      if (activatesNatively(e.target)) return false;
      e.preventDefault();
      a.toggle();
      return true;
    case "ArrowRight":
      e.preventDefault();
      a.seekBy(SEEK_STEP_MS);
      return true;
    case "ArrowLeft":
      e.preventDefault();
      a.seekBy(-SEEK_STEP_MS);
      return true;
    case "ArrowUp":
      e.preventDefault();
      a.volumeBy(VOLUME_STEP);
      return true;
    case "ArrowDown":
      e.preventDefault();
      a.volumeBy(-VOLUME_STEP);
      return true;
    case "n":
    case "N":
      a.next();
      return true;
    case "p":
    case "P":
      a.prev();
      return true;
    case "j":
    case "J":
      if (!a.skip) return false;
      a.skip(-1);
      return true;
    case "l":
    case "L":
      if (!a.skip) return false;
      a.skip(1);
      return true;
    case "a":
    case "A":
      if (!a.ab) return false;
      a.ab("a");
      return true;
    case "b":
    case "B":
      if (!a.ab) return false;
      a.ab("b");
      return true;
    case "x":
    case "X":
      if (!a.ab) return false;
      a.ab("toggle");
      return true;
    default: {
      const view = VIEW_KEYS[e.key.toLowerCase()];
      if (view) {
        a.go(view);
        return true;
      }
      return false;
    }
  }
}

/**
 * Views that scroll inside a virtualized list or grid of their own. Their
 * container must be a height-bounded flex column that does not scroll itself:
 * otherwise the list grows to its full content height, the virtualizer thinks
 * every row is on screen and renders them all (on a phone with a large
 * library, thousands of cards: the WebView runs out of memory). Every other
 * view scrolls its container normally. Used by the desktop <main> (App.vue)
 * and the phone shell, so they cannot drift apart.
 */
export const OWN_SCROLLER = new Set(["albums", "artists", "genre", "album", "playlist", "search", "queue", "audiobooks"]);

/** The classes for the box that holds view `name`. */
export function viewBoxClass(name: string): string {
  return OWN_SCROLLER.has(name) ? "flex flex-col overflow-hidden" : "overflow-y-auto";
}

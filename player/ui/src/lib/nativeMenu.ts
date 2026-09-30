/**
 * WebKit's own context menu (Reload, Inspect Element, …) is a development
 * tool, not part of the app. Unless `allowed()` (a development build, or
 * Settings → Developer tools), right-clicking where the app has no menu of
 * its own shows nothing. Text keeps its menu: in a text field, or with text
 * selected, it offers Copy and Paste.
 *
 * The app's own menus (the item menu on rows and cards) handle the event on
 * their element first and call preventDefault themselves; this listener
 * only decides what's left.
 */
export function installNativeMenuGuard(allowed: () => boolean, target: Document = document): () => void {
  function onContextMenu(e: Event): void {
    if (e.defaultPrevented || allowed()) return;
    const el = e.target instanceof Element ? e.target : null;
    const editable = el?.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"]');
    const selected = (target.getSelection?.()?.toString() ?? "").length > 0;
    if (!editable && !selected) e.preventDefault();
  }
  target.addEventListener("contextmenu", onContextMenu);
  return () => target.removeEventListener("contextmenu", onContextMenu);
}

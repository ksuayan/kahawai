import { useNavStore } from "../stores/nav";

/**
 * Android's Back button. The page keeps no browser history (the nav store's
 * breadcrumb trail is the history), so the WebView alone would close the app
 * on the first press, whatever was open. MainActivity asks the page first,
 * through `window.kahawaiBack()`, and only lets Android have the press (the
 * app goes to the background, playback carries on) when this says no.
 *
 * In order: close the top sheet, dialog or menu (every Reka layer closes on
 * Escape); else go back one step of the breadcrumb; else not handled.
 */
export function handleBack(): boolean {
  if (document.querySelector("[data-dismissable-layer]")) {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    return true;
  }
  const nav = useNavStore();
  if (nav.trail.length > 1) {
    nav.goToCrumb(nav.trail.length - 2);
    return true;
  }
  return false;
}

/** Expose [`handleBack`] to the Android shell. Needs Pinia installed. */
export function installBackHandler(): void {
  (window as unknown as { kahawaiBack?: () => boolean }).kahawaiBack = handleBack;
}

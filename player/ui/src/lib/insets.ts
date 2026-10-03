/**
 * Safe-area insets from the Android app (player/src-tauri/android-overlay's
 * MainActivity), for WebViews that report every env(safe-area-inset-*) as 0
 * while the app draws under the system bars (the HiBy R4's Chromium 91).
 *
 * The native side exposes `window.KahawaiInsets.get()` (JSON, CSS pixels) and
 * fires `kahawai-insets` when the bars change. The values go into the same
 * --sat/--sab/--sal/--sar variables style.css derives from env(), as inline
 * styles on <html>, so everything that pads by them follows. Elsewhere (the
 * desktop, a browser) there is no bridge and nothing changes.
 */
interface Bridge {
  get(): string;
}

type Sides = { top: number; right: number; bottom: number; left: number };

function bridge(): Bridge | null {
  const b = (window as unknown as { KahawaiInsets?: Bridge }).KahawaiInsets;
  return b && typeof b.get === "function" ? b : null;
}

/** Read the bridge and set the variables. Returns what was applied, or null. */
export function applyNativeInsets(root: HTMLElement = document.documentElement): Sides | null {
  const b = bridge();
  if (!b) return null;
  let s: Partial<Sides>;
  try {
    s = JSON.parse(b.get()) as Partial<Sides>;
  } catch {
    return null;
  }
  const px = (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.round(v) : 0);
  const sides: Sides = { top: px(s.top), right: px(s.right), bottom: px(s.bottom), left: px(s.left) };
  // Never less than what the WebView itself reports, where it does.
  root.style.setProperty("--sat", `max(env(safe-area-inset-top, 0px), ${sides.top}px)`);
  root.style.setProperty("--sar", `max(env(safe-area-inset-right, 0px), ${sides.right}px)`);
  root.style.setProperty("--sab", `max(env(safe-area-inset-bottom, 0px), ${sides.bottom}px)`);
  root.style.setProperty("--sal", `max(env(safe-area-inset-left, 0px), ${sides.left}px)`);
  return sides;
}

/** Apply now and whenever the app says the bars changed. */
export function watchNativeInsets(): void {
  if (!bridge()) return;
  applyNativeInsets();
  window.addEventListener("kahawai-insets", () => applyNativeInsets());
  window.addEventListener("resize", () => applyNativeInsets());
}

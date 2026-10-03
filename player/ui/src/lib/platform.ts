import { inTauri } from "../tauri";

export type PlatformName = "android" | "ios" | "macos" | "windows" | "linux" | "web";

/**
 * Behavioral platform detection. Layout never branches on this — layout
 * uses useBreakpoint (width). Use this for behavior only (haptics,
 * share sheets, etc.).
 */
export function platformName(): PlatformName {
  if (typeof navigator === "undefined") return "web";
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes("android")) return "android";
  // iOS webviews: iPhone/iPad/iPod tokens (iPadOS 13+ reports Macintosh, so
  // also check touch points when the UA claims to be a Mac).
  if (/iphone|ipad|ipod/.test(ua)) return "ios";
  if (ua.includes("macintosh") && typeof navigator.maxTouchPoints === "number" && navigator.maxTouchPoints > 1) {
    return "ios";
  }
  if (ua.includes("mac")) return "macos";
  if (ua.includes("win")) return "windows";
  if (ua.includes("linux")) return "linux";
  // Inside Tauri but UA unrecognized: best-effort fallback.
  return inTauri() ? "android" : "web";
}

/** True when running inside the Tauri Android or iOS shell. */
export function isMobileApp(): boolean {
  const p = platformName();
  return inTauri() && (p === "android" || p === "ios");
}

import { describe, expect, it, vi, afterEach } from "vitest";
import { isMobileApp, platformName } from "./platform";

const realUA = navigator.userAgent;
const realMaxTouch = navigator.maxTouchPoints;

function setUA(ua: string, maxTouchPoints = 0) {
  Object.defineProperty(navigator, "userAgent", { value: ua, configurable: true });
  Object.defineProperty(navigator, "maxTouchPoints", { value: maxTouchPoints, configurable: true });
}

afterEach(() => {
  setUA(realUA, realMaxTouch);
  vi.restoreAllMocks();
});

describe("platformName", () => {
  it("detects Android", () => {
    setUA("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36");
    expect(platformName()).toBe("android");
  });

  it("detects iPhone", () => {
    setUA("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15");
    expect(platformName()).toBe("ios");
  });

  it("detects iPadOS masquerading as Macintosh via touch points", () => {
    setUA("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) AppleWebKit/605.1.15", 5);
    expect(platformName()).toBe("ios");
  });

  it("detects desktop macOS", () => {
    setUA("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15", 0);
    expect(platformName()).toBe("macos");
  });

  it("detects Windows and Linux", () => {
    setUA("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36");
    expect(platformName()).toBe("windows");
    setUA("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36");
    expect(platformName()).toBe("linux");
  });
});

describe("isMobileApp", () => {
  it("is false outside Tauri even on a mobile UA", () => {
    setUA("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36");
    expect(isMobileApp()).toBe(false);
  });
});

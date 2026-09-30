import { describe, expect, it } from "vitest";
import { tauri } from "./test/tauri-mock";

describe("splash page", () => {
  it("shows the build's name and version, and reports once the artwork is on screen", async () => {
    document.body.innerHTML = '<p id="name"></p><p id="version"></p>';
    window.history.replaceState(null, "", "/splash.html?name=Kahawai%20Player&version=0.1.0");
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    await import("./splash");
    expect(document.getElementById("name")!.textContent).toBe("Kahawai Player");
    expect(document.getElementById("version")!.textContent).toBe("Version 0.1.0");
    await new Promise((r) => setTimeout(r, 100)); // decode, then two frames
    expect(tauri.callsTo("splash_shown")).toHaveLength(1);
  });
});

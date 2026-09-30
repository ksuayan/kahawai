import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { tauri } from "../test/tauri-mock";
import { useDeveloperStore } from "./developer";

beforeEach(() => {
  setActivePinia(createPinia());
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
});

describe("developer tools", () => {
  it("off in a release build: WebKit's menu is not allowed", async () => {
    tauri.on("get_developer_tools", { enabled: false, inspector: false, dev_build: false });
    const d = useDeveloperStore();
    d.devBuild = false; // the test runner is a dev build
    await d.load();
    expect(d.nativeMenuAllowed()).toBe(false);
  });

  it("turning it on allows the menu at once and saves the choice; the Inspector follows on restart", async () => {
    tauri
      .on("get_developer_tools", { enabled: false, inspector: false, dev_build: false })
      .on("set_developer_tools", (a?: Record<string, unknown>) => ({ enabled: a!.enabled, inspector: false, dev_build: false }));
    const d = useDeveloperStore();
    d.devBuild = false;
    await d.load();
    await d.set(true);
    expect(tauri.callsTo("set_developer_tools")).toEqual([{ enabled: true }]);
    expect(d.nativeMenuAllowed()).toBe(true);
    expect([d.enabled, d.inspector]).toEqual([true, false]);
  });
});

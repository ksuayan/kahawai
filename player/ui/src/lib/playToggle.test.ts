import { Pause, Play } from "lucide-vue-next";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises } from "@vue/test-utils";
import { makeState, makeTrack } from "../test/fixtures";
import { tauri } from "../test/tauri-mock";
import { usePlayerStore } from "../stores/player";
import { usePlayToggle } from "./playToggle";

const mine = makeTrack({ id: 1, album_id: 10 });
const other = makeTrack({ id: 2, album_id: 20 });

async function playerWith(status: "playing" | "paused" | "stopped", track = mine) {
  setActivePinia(createPinia());
  tauri.reset();
  tauri.on("get_state", makeState({ status, track: status === "stopped" ? null : track }));
  await usePlayerStore().init();
}
const button = (start = vi.fn()) => ({ start, b: usePlayToggle((t) => t.album_id === 10, start) });

beforeEach(() => tauri.reset());

describe("usePlayToggle", () => {
  it("says Pause, with the pause icon, while its own track plays, and pauses", async () => {
    await playerWith("playing");
    const { b, start } = button();
    expect(b.playing).toBe(true);
    expect(b.label).toBe("Pause");
    expect(b.icon).toBe(Pause);
    b.press();
    await flushPromises();
    expect(tauri.callsTo("pause")).toHaveLength(1);
    expect(start).not.toHaveBeenCalled();
  });

  it("says Play while its own track is paused, and carries on from there", async () => {
    await playerWith("paused");
    const { b, start } = button();
    expect(b.label).toBe("Play");
    expect(b.icon).toBe(Play);
    b.press();
    await flushPromises();
    expect(tauri.callsTo("resume")).toHaveLength(1);
    expect(start).not.toHaveBeenCalled();
  });

  it("says Play and starts its own thing while something else plays, or nothing does", async () => {
    for (const [status, track] of [["playing", other], ["stopped", mine]] as const) {
      await playerWith(status, track);
      const { b, start } = button();
      expect(b.label).toBe("Play");
      b.press();
      await flushPromises();
      expect(start).toHaveBeenCalledOnce();
      expect(tauri.callsTo("pause")).toHaveLength(0);
    }
  });

  it("follows the player: Pause appears when its track starts playing", async () => {
    await playerWith("stopped");
    const { b } = button();
    expect(b.label).toBe("Play");
    tauri.emit("player-state", makeState({ status: "playing", track: mine }));
    await flushPromises();
    expect(b.label).toBe("Pause");
  });
});

import { describe, expect, it } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, settle } from "./test/helpers";
import App from "./App.vue";

describe("App", () => {
  it("shows the wizard when no usable config exists", async () => {
    tauri.on("setup_get_state", { config_path: "/cfg/config.toml", config_exists: false });
    const { wrapper } = mountApp(App);
    await settle();
    expect(wrapper.text()).toContain("Welcome to Kahawai Server");
  });

  it("shows the status view when a config already exists", async () => {
    tauri
      .on("setup_get_state", {
        config_path: "/cfg/config.toml",
        config_exists: true,
        config: {
          music_dirs: ["/music/a"],
          bind: "0.0.0.0:8080",
          db_path: "/data/music.db",
          preferred_ladder: ["passthrough", "flac"],
          dsd_story: "pcm",
          scan_on_startup: false,
        },
      })
      .on("setup_server_status", { running: true, bind: "0.0.0.0:8080" });
    const { wrapper } = mountApp(App);
    await settle();
    expect(wrapper.text()).toContain("Server running");
    expect(wrapper.text()).toContain("0.0.0.0:8080");
  });

  it("quits the server from the status view", async () => {
    tauri
      .on("setup_get_state", {
        config_path: "/cfg/config.toml",
        config_exists: true,
        config: {
          music_dirs: ["/music/a"],
          bind: "0.0.0.0:8080",
          db_path: "/data/music.db",
          preferred_ladder: ["passthrough", "flac"],
          dsd_story: "pcm",
          scan_on_startup: false,
        },
      })
      .on("setup_server_status", { running: true, bind: "0.0.0.0:8080" })
      .on("setup_quit", undefined);
    const { wrapper } = mountApp(App);
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Quit App")!.trigger("click");
    expect(tauri.callsTo("setup_quit")).toHaveLength(1);
  });
});

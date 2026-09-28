import { describe, expect, it } from "vitest";
import { mountApp, settle } from "../test/helpers";
import StatusView from "./StatusView.vue";
import { useSetupStore } from "../stores/setup";

function boot() {
  return mountApp(StatusView);
}

describe("StatusView: tabs", () => {
  it("shows the Status tab by default", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    await settle();
    expect(wrapper.text()).toContain("Server running");
    expect(wrapper.text()).not.toContain("Music folders");
  });

  it("switches to Settings and back", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    await settle();

    await wrapper.findAll("button").find((b) => b.text() === "settings")!.trigger("click");
    await settle();
    expect(wrapper.text()).toContain("Music folders");
    expect(wrapper.text()).not.toContain("Server running");

    await wrapper.findAll("button").find((b) => b.text() === "status")!.trigger("click");
    await settle();
    expect(wrapper.text()).toContain("Server running");
  });
});

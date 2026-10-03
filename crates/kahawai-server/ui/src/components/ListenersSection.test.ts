import { describe, expect, it } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, settle } from "../test/helpers";
import ListenersSection from "./ListenersSection.vue";

const listeners = [
  { id: 0, name: "Default", books_started: 3 },
  { id: 2, name: "Sam", books_started: 1 },
];

describe("ListenersSection", () => {
  it("lists the listeners; Default cannot be removed", async () => {
    tauri.on("setup_audiobook_listeners", listeners);
    const { wrapper } = mountApp(ListenersSection);
    await settle();
    const rows = wrapper.findAll('[data-testid="listener-row"]');
    expect(rows.map((r) => r.text())).toEqual(["Default3 books started", "Sam1 book started"]);
    expect(rows[0]!.find('[data-testid="remove-listener"]').exists()).toBe(false);
    expect(rows[1]!.find('[data-testid="remove-listener"]').exists()).toBe(true);
  });

  it("adds and removes through the server", async () => {
    tauri
      .on("setup_audiobook_listeners", listeners)
      .on("setup_add_audiobook_listener", { id: 3, name: "Alex", books_started: 0 })
      .on("setup_remove_audiobook_listener", undefined);
    const { wrapper } = mountApp(ListenersSection);
    await settle();
    await wrapper.get('[data-testid="new-listener"] input, input[data-testid="new-listener"]').setValue("Alex");
    await wrapper.get('[data-testid="add-listener"]').trigger("click");
    await settle();
    expect(tauri.callsTo("setup_add_audiobook_listener")).toEqual([{ name: "Alex" }]);
    await wrapper.get('[data-testid="remove-listener"]').trigger("click");
    await settle();
    expect(tauri.callsTo("setup_remove_audiobook_listener")).toEqual([{ id: 2 }]);
  });
});

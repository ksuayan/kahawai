import { describe, expect, it } from "vitest";
import { $$, mountApp, settle } from "../test/helpers";
import { useToastsStore } from "../stores/toasts";
import ToastHost from "./ToastHost.vue";

const shown = () => $$("[data-kind]");

describe("ToastHost", () => {
  it("renders nothing without toasts", async () => {
    mountApp(ToastHost);
    await settle();
    expect(shown()).toHaveLength(0);
  });

  it("shows each toast's title and detail with its kind", async () => {
    mountApp(ToastHost);
    const t = useToastsStore();
    t.push("success", "Playlist created", { ttl: 0 });
    t.push("error", "Scan failed", { detail: "disk full\nsecond line", ttl: 0 });
    await settle();
    expect(shown().map((e) => e.dataset.kind)).toEqual(["success", "error"]);
    expect(shown()[0].textContent).toContain("Playlist created");
    expect(shown()[1].textContent).toContain("Scan failed");
    expect(shown()[1].textContent).toContain("disk full");
  });

  it("colours each kind differently", async () => {
    mountApp(ToastHost);
    const t = useToastsStore();
    for (const k of ["info", "success", "error", "progress"] as const) t.push(k, k, { ttl: 0 });
    await settle();
    const cls = shown().map((e) => e.className.split(" ").find((c) => c.startsWith("border-l-") && !c.includes("[")));
    expect(cls).toEqual(["border-l-accent", "border-l-ok", "border-l-danger", "border-l-warn"]);
  });

  it("shows a progress bar for progress toasts and follows updates", async () => {
    mountApp(ToastHost);
    const t = useToastsStore();
    const id = t.push("progress", "Scanning", { progress: 0.25 });
    await settle();
    const bar = () => document.body.querySelector<HTMLElement>('[role="progressbar"]')!;
    expect(bar().getAttribute("aria-valuenow")).toBe("25");
    expect(bar().style.width).toBe("25%");
    t.update(id, { progress: 0.8 });
    await settle();
    expect(bar().getAttribute("aria-valuenow")).toBe("80");
  });

  it("has no progress bar for a plain toast", async () => {
    mountApp(ToastHost);
    useToastsStore().push("info", "Hello", { ttl: 0 });
    await settle();
    expect(document.body.querySelector('[role="progressbar"]')).toBeNull();
  });

  it("dismisses a toast from its close button", async () => {
    mountApp(ToastHost);
    const t = useToastsStore();
    t.push("info", "A", { ttl: 0 });
    t.push("info", "B", { ttl: 0 });
    await settle();
    const close = document.body.querySelector<HTMLElement>('[aria-label="Dismiss"]')!;
    close.click();
    await settle();
    expect(t.toasts.map((x) => x.title)).toEqual(["B"]);
    expect(shown()).toHaveLength(1);
  });

  it("does not auto-close on its own: the store owns timing (sticky stays)", async () => {
    mountApp(ToastHost);
    const t = useToastsStore();
    t.push("progress", "Long job", { ttl: 0 });
    await settle();
    await new Promise((r) => setTimeout(r, 50));
    expect(t.toasts).toHaveLength(1);
  });
});

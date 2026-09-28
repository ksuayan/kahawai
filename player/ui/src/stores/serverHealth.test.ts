import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { setBaseUrl } from "../api";
import { latestEventSource } from "../test/eventsource-mock";
import { useServerHealthStore } from "./serverHealth";
import { useToastsStore } from "./toasts";

const MIN = 60_000;

beforeEach(() => {
  setActivePinia(createPinia());
  setBaseUrl("http://server:8080");
  vi.useFakeTimers();
});

function connect(): void {
  latestEventSource()!.emit("open");
}

function drop(): void {
  latestEventSource()!.emit("error");
}

describe("serverHealth store", () => {
  it("does nothing before the first successful connection", () => {
    const health = useServerHealthStore();
    health.init();
    drop(); // never connected — not a "disconnect"
    expect(useToastsStore().toasts).toHaveLength(0);
  });

  it("toasts immediately on disconnect, then escalates at 3, 8, and 23 minutes", async () => {
    const health = useServerHealthStore();
    health.init();
    const toasts = useToastsStore();

    connect();
    drop();
    expect(toasts.toasts.map((t) => t.title)).toEqual(["Lost connection to the server"]);

    await vi.advanceTimersByTimeAsync(3 * MIN);
    expect(toasts.toasts.map((t) => t.title)).toEqual(["Still trying to reach the server…"]);

    await vi.advanceTimersByTimeAsync(5 * MIN);
    expect(toasts.toasts.map((t) => t.title)).toEqual([
      "Server has been unreachable for a while — still trying…",
    ]);

    await vi.advanceTimersByTimeAsync(15 * MIN);
    expect(toasts.toasts).toHaveLength(1);
    expect(toasts.toasts[0]).toMatchObject({
      kind: "error",
      title: "Server is unreachable. Will keep trying quietly in the background.",
    });

    // No further escalation after the final tier.
    await vi.advanceTimersByTimeAsync(60 * MIN);
    expect(toasts.toasts).toHaveLength(1);
  });

  it("clears the toast and stops escalating once reconnected", async () => {
    const health = useServerHealthStore();
    health.init();
    const toasts = useToastsStore();

    connect();
    drop();
    await vi.advanceTimersByTimeAsync(3 * MIN);
    expect(toasts.toasts).toHaveLength(1);

    connect();
    expect(toasts.toasts).toHaveLength(0);

    // The pre-reconnect timers must not still be armed.
    await vi.advanceTimersByTimeAsync(30 * MIN);
    expect(toasts.toasts).toHaveLength(0);
  });

  it("only reacts once per drop, not once per repeated failed retry", () => {
    const health = useServerHealthStore();
    health.init();
    connect();
    drop();
    drop();
    drop();
    expect(useToastsStore().toasts).toHaveLength(1);
  });

  it("shows a distinct notice for a graceful server shutdown", () => {
    const health = useServerHealthStore();
    health.init();
    connect();
    latestEventSource()!.emit("server-shutting-down");
    expect(useToastsStore().toasts.map((t) => t.title)).toContain("Server is shutting down");
  });

  it("stops reacting after the returned unsubscribe is called", () => {
    const health = useServerHealthStore();
    const stop = health.init();
    connect();
    stop();
    drop();
    expect(useToastsStore().toasts).toHaveLength(0);
  });
});

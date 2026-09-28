import { defineStore } from "pinia";
import { onServerConnected, onServerDisconnected, onServerShuttingDown } from "../api";
import { useToastsStore } from "./toasts";

/** Escalation ladder, each entry the delay *since the previous tier* (not
 *  cumulative): 3 min, then another 5 (8 min total), then another 15 (23 min
 *  total) before settling into a final notice and no longer escalating.
 *  The connection itself keeps quietly retrying the whole time (api.ts), so
 *  recovery is picked up immediately regardless of how far up the ladder
 *  the toast has climbed. */
const ESCALATION_STEPS: { afterMs: number; title: string }[] = [
  { afterMs: 3 * 60_000, title: "Still trying to reach the server…" },
  { afterMs: 5 * 60_000, title: "Server has been unreachable for a while — still trying…" },
  { afterMs: 15 * 60_000, title: "Server is unreachable. Will keep trying quietly in the background." },
];

/**
 * Turns the server-connection events from api.ts into user-facing toasts:
 * an immediate notice on disconnect, an escalating one at each step above
 * while still down, and silence again once reconnected.
 */
export const useServerHealthStore = defineStore("serverHealth", () => {
  let down = false;
  let toastId: number | null = null;
  let timers: number[] = [];

  function clearTimers(): void {
    timers.forEach((t) => window.clearTimeout(t));
    timers = [];
  }

  function replaceToast(kind: "info" | "error", title: string): void {
    const toasts = useToastsStore();
    if (toastId != null) toasts.dismiss(toastId);
    toastId = toasts.push(kind, title, { ttl: 0 }); // sticky until reconnect or the next tier
  }

  function scheduleEscalation(): void {
    let elapsed = 0;
    for (const [i, step] of ESCALATION_STEPS.entries()) {
      elapsed += step.afterMs;
      const isFinal = i === ESCALATION_STEPS.length - 1;
      timers.push(
        window.setTimeout(() => replaceToast(isFinal ? "error" : "info", step.title), elapsed),
      );
    }
  }

  function handleDisconnected(): void {
    if (down) return; // already handling this drop
    down = true;
    replaceToast("info", "Lost connection to the server");
    scheduleEscalation();
  }

  function handleConnected(): void {
    down = false;
    clearTimers();
    if (toastId != null) {
      useToastsStore().dismiss(toastId);
      toastId = null;
    }
  }

  function handleShuttingDown(): void {
    useToastsStore().push("info", "Server is shutting down");
  }

  /** Call once on app launch. Returns an unsubscribe function. */
  function init(): () => void {
    const stopDisconnected = onServerDisconnected(handleDisconnected);
    const stopConnected = onServerConnected(handleConnected);
    const stopShuttingDown = onServerShuttingDown(handleShuttingDown);
    return () => {
      stopDisconnected();
      stopConnected();
      stopShuttingDown();
      clearTimers();
    };
  }

  return { init };
});

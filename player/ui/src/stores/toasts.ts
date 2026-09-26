import { defineStore } from "pinia";
import { ref } from "vue";

export type ToastKind = "info" | "success" | "error" | "progress";

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  detail?: string | null;
  /** 0..1 for progress toasts. */
  progress?: number | null;
  /** Auto-dismiss after this many ms (0 = sticky). */
  ttl: number;
}

let nextId = 1;

/**
 * Small toast host for transient job/scan/import notifications.
 * The jobs store drives start/progress/completion/failure toasts;
 * components can push their own for e.g. playlist import results.
 */
export const useToastsStore = defineStore("toasts", () => {
  const toasts = ref<Toast[]>([]);

  function push(
    kind: ToastKind,
    title: string,
    opts: { detail?: string | null; progress?: number | null; ttl?: number } = {},
  ): number {
    const id = nextId++;
    const ttl = opts.ttl ?? (kind === "error" ? 12000 : kind === "progress" ? 0 : 6000);
    toasts.value.push({
      id,
      kind,
      title,
      detail: opts.detail ?? null,
      progress: opts.progress ?? null,
      ttl,
    });
    if (ttl > 0) {
      window.setTimeout(() => dismiss(id), ttl);
    }
    // Cap the stack so a flapping job can't fill the screen.
    while (toasts.value.length > 6) toasts.value.shift();
    return id;
  }

  function update(
    id: number,
    patch: Partial<Pick<Toast, "title" | "detail" | "progress" | "kind">>,
  ): void {
    const t = toasts.value.find((x) => x.id === id);
    if (t) Object.assign(t, patch);
  }

  function dismiss(id: number): void {
    toasts.value = toasts.value.filter((t) => t.id !== id);
  }

  return { toasts, push, update, dismiss };
});

// A controllable stand-in for Tauri's IPC, used by every test that touches
// `src/tauri.ts`. `invoke` resolves handlers registered with `tauri.on`;
// unregistered commands resolve `undefined` (the app treats that as a soft
// failure, exactly like running outside the Tauri webview).

export type Handler = (args?: Record<string, unknown>) => unknown;
type Listener = (e: { payload: unknown }) => void;

class TauriMock {
  handlers = new Map<string, Handler>();
  calls: { cmd: string; args?: Record<string, unknown> }[] = [];
  listeners = new Map<string, Set<Listener>>();
  /** Make `listen()` reject, as when the `core:event` capability is missing. */
  listenFails = false;

  reset(): void {
    this.handlers.clear();
    this.calls = [];
    this.listeners.clear();
    this.listenFails = false;
  }

  /** Register a command result (value or function of the args). */
  on(cmd: string, result: unknown | Handler): this {
    this.handlers.set(cmd, typeof result === "function" ? (result as Handler) : () => result);
    return this;
  }

  async invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
    this.calls.push({ cmd, args });
    const h = this.handlers.get(cmd);
    return h ? h(args) : undefined;
  }

  /** Args of every call to `cmd`, in order. */
  callsTo(cmd: string): (Record<string, unknown> | undefined)[] {
    return this.calls.filter((c) => c.cmd === cmd).map((c) => c.args);
  }

  async listen(event: string, cb: Listener): Promise<() => void> {
    if (this.listenFails) throw new Error("event.listen not allowed");
    if (!this.listeners.has(event)) this.listeners.set(event, new Set());
    this.listeners.get(event)!.add(cb);
    return () => this.listeners.get(event)?.delete(cb);
  }

  /** Deliver an event to subscribers, like the shell's `app.emit`. */
  emit(event: string, payload: unknown): void {
    this.listeners.get(event)?.forEach((cb) => cb({ payload }));
  }
}

export const tauri = new TauriMock();

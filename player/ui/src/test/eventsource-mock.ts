// Controllable stand-in for the browser's `EventSource`, used to test the
// catalog-updated SSE subscription (`api.ts`) without a real connection.
// happy-dom does not implement `EventSource` at all, so this is installed
// globally in `test/setup.ts`, not opt-in per test.

type Listener = (e: { data: string }) => void;

export class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  closed = false;
  private listeners = new Map<string, Set<Listener>>();

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(type: string, cb: Listener): void {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(cb);
  }

  removeEventListener(type: string, cb: Listener): void {
    this.listeners.get(type)?.delete(cb);
  }

  close(): void {
    this.closed = true;
  }

  /** Deliver an event to subscribers, like the server's SSE stream would. */
  emit(type: string, data = ""): void {
    this.listeners.get(type)?.forEach((cb) => cb({ data }));
  }
}

export function resetEventSourceMock(): void {
  MockEventSource.instances = [];
}

/** The most recently constructed instance — `setBaseUrl` closes the old one
 *  and opens a new one on every call, so this is always the live one. */
export function latestEventSource(): MockEventSource | undefined {
  return MockEventSource.instances.at(-1);
}

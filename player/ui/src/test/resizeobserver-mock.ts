// Controllable stand-in for `ResizeObserver`, used to test components that
// resize-observe an element appearing on a `v-else`/`v-if` branch (e.g.
// AlbumsView's virtualized grid) — regression coverage for a real bug: a
// one-shot `observe()` call in `onMounted` silently does nothing if the
// element isn't there yet (still loading), and nothing ever retries once it
// appears. The default no-op stub can't catch that class of bug since it
// never records *what* was observed or *when*.

type Callback = ResizeObserverCallback;

export class ResizeObserverStub {
  static instances: ResizeObserverStub[] = [];
  observed = new Set<Element>();

  constructor(private callback: Callback) {
    ResizeObserverStub.instances.push(this);
  }

  observe(el: Element): void {
    this.observed.add(el);
  }

  unobserve(el: Element): void {
    this.observed.delete(el);
  }

  disconnect(): void {
    this.observed.clear();
  }

  /** Fire a synthetic resize, like the browser would on a real layout change. */
  trigger(el: Element, width: number, height = 0): void {
    if (!this.observed.has(el)) return;
    this.callback(
      [{ contentRect: { width, height } } as ResizeObserverEntry],
      this as unknown as ResizeObserver,
    );
  }
}

export function resetResizeObserverMock(): void {
  ResizeObserverStub.instances = [];
}

/** The most recently constructed observer — components create at most one. */
export function latestResizeObserver(): ResizeObserverStub | undefined {
  return ResizeObserverStub.instances.at(-1);
}

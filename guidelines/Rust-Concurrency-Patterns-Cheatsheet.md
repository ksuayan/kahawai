# Rust Concurrency Patterns — Cheatsheet for Non-Rust Developers

Rust's headline promise: **data races are compile errors, not runtime surprises.** Two marker traits do the work — `Send` (safe to move to another thread) and `Sync` (safe to share between threads). If your type isn't both where it needs to be, the code doesn't compile. Everything below is a pattern for getting work across threads *within* that rule.

---

## Decision table — "I need to…"

| I need to… | Reach for | Crate |
|---|---|---|
| Run N independent jobs and collect results | `thread::scope` + `join`, or channels | `std` |
| Share read-mostly state across threads | `Arc<RwLock<T>>` | `std` |
| Share mutable state, simple | `Arc<Mutex<T>>` | `std` |
| A counter or flag shared across threads | `AtomicUsize` / `AtomicBool` | `std` (`core::sync::atomic`) |
| Fan work out / stream results back | channels | `std::sync::mpsc`, `crossbeam-channel`, `tokio::sync::mpsc` |
| Parallelize a loop over data (map/filter/fold) | parallel iterators | `rayon` |
| Run thousands of concurrent I/O tasks (server, streams) | async tasks | `tokio` |
| Call blocking/CPU code from async code | `spawn_blocking` | `tokio` |
| One-time global init (config, regex) | `OnceLock` / `LazyLock` | `std` |
| Wait until several things finish | `join!`, `FuturesUnordered`, barriers | `futures`, `tokio`, `std` |
| React to whichever event arrives first | `select!` | `tokio` |

---

## 1. OS threads — `std::thread`

```rust
let handle = thread::spawn(|| {
    // owns what it captures (must be 'static + Send)
    expensive_work()
});
let result = handle.join().unwrap(); // join = wait + get result (or panic)
```

- **Best at:** coarse job parallelism, long-lived workers, bridging into non-async code.
- Threads are OS threads (1:1). Spawning thousands is the wrong tool — that's what async is for.
- `join()` returns `Result`; a panicked thread surfaces as `Err` — decide whether to propagate or ignore.

### Scoped threads — borrow instead of `Arc`

```rust
let mut data = vec![1, 2, 3];
thread::scope(|s| {
    s.spawn(|| data.push(4)); // can borrow stack data — no 'static needed
}); // scope joins everything; data usable again here
```

- **Best at:** fork-join parallelism over local data. No `Arc` ceremony, no lifetime hacks. Prefer this over manual `Arc` cloning when the threads don't outlive the current function.

---

## 2. Message passing — channels

**Don't communicate by sharing memory; share memory by communicating.**

```rust
let (tx, rx) = mpsc::channel();       // std: multi-producer, single-consumer
tx.send(work).unwrap();
let item = rx.recv().unwrap();         // blocks
```

| Channel | Flavor | Use when |
|---|---|---|
| `std::sync::mpsc` | unbounded, blocking `recv` | simple pipelines, std-only |
| `crossbeam-channel` | bounded/unbounded, `select!`, `try_recv` | need backpressure, timeouts, or MPMC |
| `tokio::sync::mpsc` | async `send().await` / `recv().await` | inside async code |
| `tokio::sync::broadcast` / `watch` | one-to-many | config reload, shutdown signals |

- **Best at:** pipelines, worker pools, clean shutdown (drop the sender → receivers see disconnect).
- **Bounded vs unbounded:** bounded channels apply *backpressure* — a fast producer blocks instead of growing memory forever. Prefer bounded when the producer can outrun the consumer.
- **Actor pattern:** a struct owned by one thread/task + an `mpsc` inbox is a complete actor. No locks, no shared state — often the simplest correct design.

---

## 3. Shared state — `Arc<Mutex<T>>` / `Arc<RwLock<T>>`

```rust
let state = Arc::new(Mutex::new(Cache::new()));
let s2 = Arc::clone(&state);
thread::spawn(move || { s2.lock().unwrap().insert(k, v); });
```

- `Arc` = thread-safe reference counting (like `shared_ptr`). `Mutex` = exclusive access. Together: shared *mutable* state.
- **`RwLock`:** many readers *or* one writer. Best when reads dominate (config, caches). Writers can starve under heavy read load — know your ratio.
- **Lock granularity:** hold the guard for the shortest time possible. Clone data out, drop the guard, then compute.
- **Poisoning:** if a thread panics while holding a `Mutex`, the lock is *poisoned* — `lock()` returns `Err`. That's usually `.unwrap()`-able in apps (the data is fine; only the panic flag is set), but handle it deliberately in libraries.

### ⚠️ The async trap

**Never hold a `std::sync::Mutex` guard across `.await`.** The thread blocks while the task yields → executor thread starvation, or worse. In async code use `tokio::sync::Mutex` (lock is itself async) — or better, restructure so the lock isn't held across the await at all.

---

## 4. Atomics — lock-free counters and flags

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
let hits = AtomicUsize::new(0);
hits.fetch_add(1, Ordering::Relaxed);
if done.load(Ordering::Acquire) { … }
```

- **Best at:** counters, progress reporting, shutdown flags, sequence numbers. Cheapest synchronization there is.
- **Orderings, the 10-second version:**
  - `Relaxed` — just the atomic op, no ordering promises. Fine for pure counters where nothing else depends on the value.
  - `Acquire` (load) / `Release` (store) — the standard pair: a `Release` store *publishes* prior writes; an `Acquire` load *observes* them. Use for flags guarding other data.
  - `SeqCst` — total global order, strongest and slowest. Default when unsure; optimize down only with a reason.
- Atomics are not a substitute for a mutex around *compound* state. "Check then act" on two atomics is still a race.

---

## 5. Rayon — data parallelism

```rust
use rayon::prelude::*;
let total: u64 = pixels.par_iter().map(process).sum(); // parallel iterator
```

- **Best at:** CPU-bound loops over collections — image processing, encoding, batch transforms, search. Drop-in `.par_iter()` on existing iterator chains; work-stealing pool balances the load.
- **Not for:** I/O (it will happily block its thread pool on your network call), or tiny workloads where pool overhead dominates.
- Requires `Send` item types — the compiler enforces thread safety of your closure for free.
- Tune with `rayon::ThreadPoolBuilder` (thread count) for machines where the default (one thread per core) is wrong — e.g. hyperthreaded boxes doing memory-bound work.
- **Don't call rayon from inside async tasks** without care: blocking the executor's threads on CPU pools compounds badly. `spawn_blocking` first.

---

## 6. Tokio — async I/O at scale

```rust
#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("0.0.0.0:8080").await.unwrap();
    loop {
        let (socket, _) = listener.accept().await.unwrap();
        tokio::spawn(async move { handle(socket).await }); // cheap task, not a thread
    }
}
```

- **Best at:** network servers, many concurrent I/O-bound tasks, timers, streaming. Thousands of tasks on a handful of OS threads.
- Tasks are cooperative: an `.await` yields; code *between* awaits never interleaves. That's what makes "no locks needed" true *within* a task — but shared state across tasks still needs `Arc<Mutex>` (the tokio one) or channels.
- **CPU-bound work in async:** `tokio::task::spawn_blocking` moves it to a dedicated blocking pool. Rule of thumb: nothing that takes >~100µs without awaiting should run directly on an async task.
- `tokio::select!` — wait on multiple futures, act on the first ready. The cancellation-safety rules matter: the *losing* branches are dropped, so futures must be safe to drop mid-await (most are; document when not).
- `tokio::sync` mirrors std's primitives for async: `Mutex`, `RwLock`, `mpsc`, `oneshot`, `watch`, `Semaphore`, `Notify`.

### When *not* to use Tokio

No async I/O workload → no Tokio. A CLI that scans files in parallel wants `thread::scope` or rayon, not a runtime. Async is a tool for *waiting efficiently*, not a general speedup.

---

## 7. `futures` crate — combinators

```rust
use futures::future::{join, join_all};
let (a, b) = join(fetch_a(), fetch_b()).await;   // both concurrently
let results = join_all(items.map(process)).await; // N concurrently
```

- **Best at:** combining a *known set* of futures without a runtime — works on any executor, including embedded/wasm.
- `FuturesUnordered` — a stream of futures completing in arrival order; the workhorse for "spawn N, process as they finish."

---

## 8. One-time init — `OnceLock` / `LazyLock`

```rust
static CONFIG: LazyLock<Config> = LazyLock::new(|| load_config());
```

- **Best at:** global config, compiled regexes, lookup tables. Initialized exactly once, thread-safely, on first use. Replaces `lazy_static`.

---

## 9. Rarer tools (know they exist)

| Tool | For |
|---|---|
| `Condvar` | Classic wait/signal with a `Mutex` (producer waits until buffer non-empty) |
| `Barrier` | N threads rendezvous before any proceeds (phased computation) |
| `threadpool` crate | Explicit bounded pool when rayon/​tokio don't fit |
| `crossbeam` (epoch, deque) | Lock-free data structures; you almost certainly don't need these directly |
| `parking_lot` | Faster `Mutex`/`RwLock` (smaller, no poisoning). Drop-in upgrade if lock overhead ever shows in profiles |

---

## Gotchas for developers coming from other languages

1. **The borrow checker *is* the data-race detector.** Fighting it usually means the design has shared mutable state that should be a channel or an owned handoff instead.
2. **`Send`/`Sync` errors name the culprit.** "Future is not Send" after adding `.await` → something non-thread-safe (e.g. `Rc`, `Cell`) is held across the await. Swap for `Arc`/`Mutex` or scope it tighter.
3. **Deadlocks are still your fault.** Lock ordering (always acquire A before B), no locks across await, no calling back into a lock holder — the compiler can't save you here.
4. **Unbounded channels are memory leaks with extra steps.** If the producer can outrun the consumer, bound the channel and let backpressure do its job.
5. **Threads are 1:1 with OS threads; tasks are not.** 10k threads = pain. 10k tokio tasks = Tuesday.
6. **`clone()` on `Arc` is cheap; cloning the *data* is not.** `Arc::clone(&x)` bumps a counter. `x.lock().clone()` copies the world. Know which one you wrote.
7. **Measure before reaching for atomics/lock-free.** A `Mutex` uncontended costs ~20ns. Complexity is the real expense — start boring.

import { flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeTrack, mockFetch } from "../test/fixtures";
import { useJobsStore } from "./jobs";
import { useToastsStore } from "./toasts";

const job = (over: Record<string, unknown> = {}) => ({
  id: "job-1", kind: "scan", label: "startup scan", status: "running", progress: 0.2, message: null, payload: null, ...over,
});
const res = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });

/** A /api/jobs route whose answer the test can change between polls. */
function jobsFeed(initial: unknown[]) {
  let current = initial;
  const calls = mockFetch({ "/api/jobs": (_u: string, init?: RequestInit) => (init?.method === "POST" ? res(current[0]) : res(current)) });
  return { calls, set: (v: unknown[]) => (current = v), polls: () => calls.filter((c) => c.url.endsWith("/api/jobs") && !c.init?.method).length };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout"] });
});
afterEach(() => vi.useRealTimers());

describe("jobs store: polling", () => {
  it("stays silent for historical finished jobs and stops polling", async () => {
    const feed = jobsFeed([job({ status: "done", progress: 1 })]);
    const jobs = useJobsStore();
    const toasts = useToastsStore();
    jobs.init();
    await flushPromises();
    expect(jobs.jobs).toHaveLength(1);
    expect(jobs.hasActive).toBe(false);
    expect(toasts.toasts).toHaveLength(0);
    const before = feed.polls();
    await vi.advanceTimersByTimeAsync(3000);
    expect(feed.polls()).toBe(before); // idle: no more polling
  });

  it("announces an already-running job with a progress toast, then follows its progress", async () => {
    const feed = jobsFeed([job({ progress: 0.2 })]);
    const jobs = useJobsStore();
    const toasts = useToastsStore();
    jobs.init();
    await flushPromises();
    expect(jobs.hasActive).toBe(true);
    expect(toasts.toasts).toHaveLength(1);
    expect(toasts.toasts[0]).toMatchObject({ kind: "progress", title: "Library scan started", progress: 0.2 });

    feed.set([job({ progress: 0.65 })]);
    await vi.advanceTimersByTimeAsync(600);
    expect(toasts.toasts).toHaveLength(1); // updated in place, not duplicated
    expect(toasts.toasts[0].progress).toBe(0.65);
  });

  it("swaps the progress toast for a success toast when the job finishes, then stops polling", async () => {
    const feed = jobsFeed([job()]);
    const jobs = useJobsStore();
    const toasts = useToastsStore();
    jobs.init();
    await flushPromises();
    feed.set([job({ status: "done", progress: 1, message: "scan complete: 12 files seen" })]);
    await vi.advanceTimersByTimeAsync(600);
    expect(toasts.toasts.map((t) => [t.kind, t.title])).toEqual([["success", "Library scan finished"]]);
    expect(toasts.toasts[0].detail).toBe("scan complete: 12 files seen");
    const after = feed.polls();
    await vi.advanceTimersByTimeAsync(3000);
    expect(feed.polls()).toBe(after);
    expect(jobs.hasActive).toBe(false);
  });

  it("reports a failed job with the server's message verbatim", async () => {
    const feed = jobsFeed([job({ id: "x", kind: "extract_iso", label: "Extract SACD ISO: a" })]);
    const jobs = useJobsStore();
    const toasts = useToastsStore();
    jobs.init();
    await flushPromises();
    feed.set([job({ id: "x", kind: "extract_iso", label: "Extract SACD ISO: a", status: "failed", message: "sacd_extract integration not yet implemented" })]);
    await vi.advanceTimersByTimeAsync(600);
    expect(toasts.toasts.at(-1)).toMatchObject({ kind: "error", title: "Extract SACD ISO: a failed", detail: "sacd_extract integration not yet implemented" });
    expect(toasts.toasts.some((t) => t.kind === "progress")).toBe(false);
  });

  it("a failed job without a message still says something", async () => {
    const feed = jobsFeed([job()]);
    const jobs = useJobsStore();
    const toasts = useToastsStore();
    jobs.init();
    await flushPromises();
    feed.set([job({ status: "failed", message: null })]);
    await vi.advanceTimersByTimeAsync(600);
    expect(toasts.toasts.at(-1)?.detail).toBe("Job failed");
  });

  it("records a polling failure without throwing and recovers", async () => {
    const jobs = useJobsStore();
    mockFetch({});
    jobs.init();
    await flushPromises();
    expect(jobs.lastError).toBeTruthy();
    jobsFeed([job({ status: "done", progress: 1 })]);
    await jobs.refresh();
    expect(jobs.lastError).toBeNull();
    expect(jobs.jobs).toHaveLength(1);
  });
});

describe("jobs store: starting work", () => {
  it("startScan: a 409 means a scan is already running, which is information, not an error", async () => {
    mockFetch({
      "/api/scan": () => res({ error: "scan already running" }, 409),
      "/api/jobs": () => res([job()]),
    });
    const jobs = useJobsStore();
    const toasts = useToastsStore();
    await jobs.startScan();
    await flushPromises();
    expect(toasts.toasts.find((t) => t.title === "Scan already running")).toMatchObject({ kind: "info" });
    expect(toasts.toasts.some((t) => t.kind === "error")).toBe(false);
    expect(jobs.hasActive).toBe(true); // it starts following the running scan
  });

  it("startScan: any other failure is an error toast with the reason", async () => {
    mockFetch({ "/api/scan": () => res({ error: "db locked" }, 500) });
    const toasts = useToastsStore();
    await useJobsStore().startScan();
    expect(toasts.toasts.at(-1)).toMatchObject({ kind: "error", title: "Could not start scan", detail: "db locked" });
  });

  it("startScan: success shows progress and follows the job", async () => {
    const started = job({ id: "scan-9", progress: 0, status: "queued" });
    mockFetch({
      "/api/scan": () => res(started, 202),
      "/api/jobs": () => res([{ ...started, status: "running", progress: 0.5 }]),
    });
    const jobs = useJobsStore();
    const toasts = useToastsStore();
    await jobs.startScan();
    await flushPromises();
    expect(toasts.toasts[0]).toMatchObject({ kind: "progress", title: "Library scan started" });
    expect(jobs.jobs[0].progress).toBe(0.5);
  });

  it("extractIso posts the file path and follows the job", async () => {
    const created = job({ id: "iso-1", kind: "extract_iso", label: "Extract SACD ISO: Album", status: "queued", progress: 0 });
    const calls = mockFetch({
      "/api/jobs": (_u: string, init?: RequestInit) => (init?.method === "POST" ? res(created) : res([created])),
    });
    const track = makeTrack({ title: "Album", path: "/music/a.iso", format: "sacd_iso" });
    await useJobsStore().extractIso(track);
    await flushPromises();
    const post = calls.find((c) => c.init?.method === "POST")!;
    expect(JSON.parse(String(post.init!.body))).toEqual({ kind: "extract_iso", label: "Extract SACD ISO: Album", path: "/music/a.iso" });
    expect(useToastsStore().toasts[0]).toMatchObject({ kind: "progress", title: "Extract SACD ISO: Album started" });
  });

  it("extractIso reports a rejection", async () => {
    mockFetch({ "/api/jobs": () => res({ error: "not an ISO" }, 400) });
    const toasts = useToastsStore();
    await useJobsStore().extractIso(makeTrack());
    expect(toasts.toasts.at(-1)).toMatchObject({ kind: "error", title: "Could not start ISO extraction", detail: "not an ISO" });
  });
});

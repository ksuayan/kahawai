import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { ApiError, createJob, fetchJobs, triggerScan } from "../api";
import { isJobActive, trackTitle, type JobInfo, type JobStatus, type Track } from "../types";
import { useLibraryStore } from "./library";
import { useToastsStore } from "./toasts";

function jobTitle(j: JobInfo, verb: "started" | "running" | "finished" | "failed"): string {
  const base = j.kind === "scan" ? "Library scan" : j.label || "Job";
  return `${base} ${verb}`;
}

interface SeenJob {
  status: JobStatus;
  toastId: number | null;
}

/**
 * Server long-running jobs (library scans, SACD ISO extraction).
 * Polls GET /api/jobs at ~2 Hz, but only while at least one job is
 * active — no polling when the list is idle or empty.
 */
export const useJobsStore = defineStore("jobs", () => {
  const jobs = ref<JobInfo[]>([]);
  const lastError = ref<string | null>(null);

  const toasts = useToastsStore();
  const library = useLibraryStore();
  const seen = new Map<string, SeenJob>();

  let timer: number | undefined;

  const activeJobs = computed(() => jobs.value.filter(isJobActive));
  const hasActive = computed(() => activeJobs.value.length > 0);

  function stopPolling(): void {
    window.clearInterval(timer);
    timer = undefined;
  }

  function ensurePolling(): void {
    void poll();
    if (timer === undefined) {
      timer = window.setInterval(() => void poll(), 500);
    }
  }

  async function poll(): Promise<void> {
    let list: JobInfo[];
    try {
      list = await fetchJobs();
    } catch (e) {
      lastError.value = e instanceof Error ? e.message : String(e);
      return;
    }
    lastError.value = null;
    jobs.value = list;

    for (const j of list) {
      const prev = seen.get(j.id);
      if (!prev) {
        // Historical done/failed jobs stay silent; only active ones
        // announce themselves.
        if (isJobActive(j)) {
          const toastId = toasts.push("progress", jobTitle(j, "started"), {
            progress: j.progress,
          });
          seen.set(j.id, { status: j.status, toastId });
        } else {
          seen.set(j.id, { status: j.status, toastId: null });
        }
        continue;
      }
      if (prev.status === j.status && !isJobActive(j)) continue;
      if (isJobActive(j)) {
        if (prev.toastId != null) {
          toasts.update(prev.toastId, { progress: j.progress });
        } else {
          prev.toastId = toasts.push("progress", jobTitle(j, "started"), {
            progress: j.progress,
          });
        }
      } else if (j.status === "done") {
        if (prev.toastId != null) toasts.dismiss(prev.toastId);
        toasts.push("success", jobTitle(j, "finished"), { detail: j.message });
        prev.toastId = null;
        // A finished scan changed the catalog: reload albums and artists.
        if (j.kind === "scan") void library.loadAll();
      } else if (j.status === "failed") {
        if (prev.toastId != null) toasts.dismiss(prev.toastId);
        // The server's message is the honest failure reason — verbatim.
        toasts.push("error", jobTitle(j, "failed"), {
          detail: j.message ?? "Job failed",
        });
        prev.toastId = null;
      } else if (j.status === "paused" || j.status === "cancelled") {
        if (prev.toastId != null) toasts.dismiss(prev.toastId);
        // A lookup pauses itself when MusicBrainz is unreachable; the
        // message says why and that nothing was lost.
        if (j.status === "paused" && j.message) {
          toasts.push("info", `${j.label || "Job"} paused`, { detail: j.message });
        }
        prev.toastId = null;
      }
      prev.status = j.status;
    }

    if (!list.some(isJobActive)) stopPolling();
  }

  /** POST /api/scan. A 409 means a scan is already running — that's a
   *  state, not a failure, so it becomes an info toast. */
  async function startScan(): Promise<void> {
    try {
      const job = await triggerScan();
      seen.set(job.id, {
        status: job.status,
        toastId: toasts.push("progress", jobTitle(job, "started"), {
          progress: job.progress,
        }),
      });
      ensurePolling();
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        toasts.push("info", "Scan already running");
        ensurePolling();
        return;
      }
      toasts.push("error", "Could not start scan", {
        detail: e instanceof Error ? e.message : String(e),
      });
    }
  }

  /** SACD ISO "Extract to DSF" (POST /api/jobs). The v1 server validates
   *  the ISO path and then fails the job with its honest
   *  "sacd_extract integration not yet implemented" message, which the
   *  poller surfaces verbatim via the failure toast. */
  async function extractIso(track: Track): Promise<void> {
    try {
      const job = await createJob({
        kind: "extract_iso",
        label: `Extract SACD ISO: ${trackTitle(track)}`,
        path: track.path,
      });
      seen.set(job.id, {
        status: job.status,
        toastId: toasts.push("progress", jobTitle(job, "started"), {
          progress: job.progress,
        }),
      });
      ensurePolling();
    } catch (e) {
      toasts.push("error", "Could not start ISO extraction", {
        detail: e instanceof Error ? e.message : String(e),
      });
    }
  }

  /** One-shot refresh for the Settings jobs section; does not toast. */
  async function refresh(): Promise<void> {
    try {
      jobs.value = await fetchJobs();
      lastError.value = null;
    } catch (e) {
      lastError.value = e instanceof Error ? e.message : String(e);
    }
  }

  /** Begin background polling if anything is active (call on launch).
   *  The first poll starts the timer; poll() itself stops it again when no
   *  active jobs remain, so an already-running scan/extraction keeps
   *  updating until it finishes. */
  function init(): void {
    ensurePolling();
  }

  return {
    jobs,
    lastError,
    activeJobs,
    hasActive,
    init,
    refresh,
    startScan,
    extractIso,
  };
});

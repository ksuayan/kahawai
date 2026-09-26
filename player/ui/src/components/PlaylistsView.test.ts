import { describe, expect, it } from "vitest";
import { mockFetch } from "../test/fixtures";
import { $$, bodyOf, dialog, mountApp, settle, typeInto } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import { usePlaylistsStore } from "../stores/playlists";
import { useToastsStore } from "../stores/toasts";
import PlaylistsView from "./PlaylistsView.vue";

const lists = [
  { id: 1, name: "Road trip", track_ids: [1, 2, 3] },
  { id: 2, name: "Focus", track_ids: [] },
];
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });

async function mountView(items = lists) {
  const { wrapper } = mountApp(PlaylistsView, {}, {}, () => {
    const store = usePlaylistsStore();
    store.items = items.map((p) => ({ ...p }));
    store.loaded = true;
  });
  await settle();
  return wrapper;
}
const rows = (w: Awaited<ReturnType<typeof mountView>>) => w.findAll('[data-testid="playlist-row"]');
const btn = (w: Awaited<ReturnType<typeof mountView>>, text: string | RegExp) =>
  w.findAll("button").find((b) => (typeof text === "string" ? b.text() === text : text.test(b.text())))!;
const dlgBtn = (label: string) => $$("[role=dialog] button").find((b) => b.textContent?.trim() === label)!;

describe("PlaylistsView", () => {
  it("lists playlists with their track counts", async () => {
    const w = await mountView();
    expect(w.text()).toContain("2 playlists");
    expect(rows(w).map((r) => r.text().replace(/\s+/g, " "))[0]).toContain("Road trip");
    expect(rows(w)[0].text()).toContain("3 tracks");
    expect(rows(w)[1].text()).toContain("0 tracks");
  });

  it("shows an empty state that explains the options", async () => {
    const w = await mountView([]);
    expect(w.text()).toContain("No playlists yet");
  });

  it("opens a playlist when its name is clicked", async () => {
    const w = await mountView();
    await rows(w)[1].get("button").trigger("click");
    expect(useNavStore().view).toEqual({ name: "playlist", id: 2 });
  });

  describe("create", () => {
    it("creates from the inline form, then opens the new playlist", async () => {
      const calls = mockFetch({ "/api/playlists": () => json({ id: 9, name: "New one", track_ids: [] }) });
      const w = await mountView();
      await btn(w, /New playlist/).trigger("click");
      const input = w.get('input[aria-label="Playlist name"]').element as HTMLInputElement;
      expect(btn(w, "Create").attributes("disabled")).toBeDefined();
      await typeInto(input, "  New one ");
      expect(btn(w, "Create").attributes("disabled")).toBeUndefined();
      await btn(w, "Create").trigger("click");
      await settle();
      expect(bodyOf(calls.find((c) => c.init?.method === "POST")!.init)).toMatchObject({ name: "New one" });
      expect(useNavStore().view).toEqual({ name: "playlist", id: 9 });
    });

    it("submits on Enter and cancels on Escape", async () => {
      mockFetch({ "/api/playlists": () => json({ id: 9, name: "X", track_ids: [] }) });
      const w = await mountView();
      await btn(w, /New playlist/).trigger("click");
      const input = w.get('input[aria-label="Playlist name"]');
      await input.trigger("keydown", { key: "Escape" });
      expect(w.find('input[aria-label="Playlist name"]').exists()).toBe(false);

      await btn(w, /New playlist/).trigger("click");
      await typeInto(w.get('input[aria-label="Playlist name"]').element as HTMLInputElement, "X");
      await w.get('input[aria-label="Playlist name"]').trigger("keydown", { key: "Enter" });
      await settle();
      expect(useNavStore().view.name).toBe("playlist");
    });

    it("shows the server's error and stays on the page when creation fails", async () => {
      mockFetch({ "/api/playlists": () => json({ error: "name taken" }, 409) });
      const w = await mountView();
      await btn(w, /New playlist/).trigger("click");
      await typeInto(w.get('input[aria-label="Playlist name"]').element as HTMLInputElement, "Dup");
      await btn(w, "Create").trigger("click");
      await settle();
      expect(w.get('[role="alert"]').text()).toBe("name taken");
      expect(useNavStore().view.name).toBe("albums");
    });
  });

  describe("rename", () => {
    it("renames inline (Enter) with a PATCH", async () => {
      const calls = mockFetch({ "/api/playlists/2": () => json({ id: 2, name: "Deep focus", track_ids: [] }) });
      const w = await mountView();
      await rows(w)[1].get('button[aria-label="Rename playlist"]').trigger("click");
      const input = rows(w)[1].get("input").element as HTMLInputElement;
      expect(input.value).toBe("Focus");
      await typeInto(input, "Deep focus");
      await rows(w)[1].get("input").trigger("keydown", { key: "Enter" });
      await settle();
      expect(calls.find((c) => c.init?.method === "PATCH")!.url).toContain("/api/playlists/2");
      expect(bodyOf(calls.find((c) => c.init?.method === "PATCH")!.init)).toEqual({ name: "Deep focus" });
      expect(usePlaylistsStore().items[1].name).toBe("Deep focus");
    });

    it("rolls the name back and toasts when the rename fails", async () => {
      mockFetch({ "/api/playlists/2": () => json({ error: "read-only" }, 500) });
      const w = await mountView();
      await rows(w)[1].get('button[aria-label="Rename playlist"]').trigger("click");
      await typeInto(rows(w)[1].get("input").element as HTMLInputElement, "Changed");
      await rows(w)[1].get("input").trigger("keydown", { key: "Enter" });
      await settle();
      expect(usePlaylistsStore().items[1].name).toBe("Focus");
      expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "error", title: "Rename failed" });
    });

    it("Escape abandons the rename", async () => {
      const calls = mockFetch({});
      const w = await mountView();
      await rows(w)[0].get('button[aria-label="Rename playlist"]').trigger("click");
      await rows(w)[0].get("input").trigger("keydown", { key: "Escape" });
      expect(rows(w)[0].find("input").exists()).toBe(false);
      expect(calls).toHaveLength(0);
    });
  });

  describe("delete (confirm dialog instead of window.confirm)", () => {
    it("asks first, naming the playlist, and deletes only on confirm", async () => {
      const calls = mockFetch({ "/api/playlists/1": () => new Response(null, { status: 204 }) });
      const w = await mountView();
      await rows(w)[0].get('button[aria-label="Delete playlist"]').trigger("click");
      await settle();
      expect(dialog()!.textContent).toContain("Delete playlist?");
      expect(dialog()!.textContent).toContain("Road trip");
      expect(calls).toHaveLength(0);
      dlgBtn("Delete").click();
      await settle();
      expect(calls.some((c) => c.init?.method === "DELETE" && c.url.endsWith("/api/playlists/1"))).toBe(true);
      expect(usePlaylistsStore().items.map((p) => p.id)).toEqual([2]);
    });

    it("keeps the playlist when cancelled", async () => {
      const calls = mockFetch({});
      const w = await mountView();
      await rows(w)[0].get('button[aria-label="Delete playlist"]').trigger("click");
      await settle();
      dlgBtn("Cancel").click();
      await settle();
      expect(calls).toHaveLength(0);
      expect(usePlaylistsStore().items).toHaveLength(2);
      expect(dialog()).toBeNull();
    });

    it("shows the error if the server refuses", async () => {
      mockFetch({ "/api/playlists/1": () => json({ error: "in use" }, 500) });
      const w = await mountView();
      await rows(w)[0].get('button[aria-label="Delete playlist"]').trigger("click");
      await settle();
      dlgBtn("Delete").click();
      await settle();
      expect(w.get('[role="alert"]').text()).toBe("in use");
      expect(usePlaylistsStore().items).toHaveLength(2);
    });
  });

  describe("M3U import", () => {
    async function pickFile(w: Awaited<ReturnType<typeof mountView>>, name: string) {
      const input = w.get('input[type="file"]').element as HTMLInputElement;
      Object.defineProperty(input, "files", { value: [new File(["#EXTM3U\n"], name)], configurable: true });
      input.dispatchEvent(new Event("change", { bubbles: true }));
      await settle();
    }

    it("rejects a non-M3U file before uploading", async () => {
      const calls = mockFetch({});
      const w = await mountView();
      await pickFile(w, "notes.txt");
      expect(calls).toHaveLength(0);
      expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "error", title: "Not an M3U file" });
    });

    it("uploads, then reports what matched and what did not", async () => {
      const calls = mockFetch({
        "/api/playlists/import": () => json({ playlist_id: 5, matched: 2, unmatched: ["/old/a.flac", "/old/b.flac"] }),
        "/api/playlists/5": () => json({ id: 5, name: "mix", track_ids: [1, 2] }),
      });
      const w = await mountView();
      await pickFile(w, "mix.m3u8");
      expect(calls[0].init?.method).toBe("POST");
      expect(calls[0].init?.body).toBeInstanceOf(FormData);
      expect(dialog()!.textContent).toContain("Imported “mix”");
      expect(dialog()!.textContent).toContain("Matched 2 tracks.");
      expect(dialog()!.textContent).toContain("2 entries did not match the library");
      expect($$('[data-testid="unmatched"] li').map((l) => l.textContent)).toEqual(["/old/a.flac", "/old/b.flac"]);
    });

    it("reports a fully matched import and singular wording", async () => {
      mockFetch({
        "/api/playlists/import": () => json({ playlist_id: 5, matched: 1, unmatched: [] }),
        "/api/playlists/5": () => json({ id: 5, name: "one", track_ids: [1] }),
      });
      const w = await mountView();
      await pickFile(w, "one.m3u");
      expect(dialog()!.textContent).toContain("Matched 1 track.");
      expect(dialog()!.textContent).toContain("Every entry matched the library.");
    });

    it("opens the imported playlist from the dialog", async () => {
      mockFetch({
        "/api/playlists/import": () => json({ playlist_id: 5, matched: 1, unmatched: [] }),
        "/api/playlists/5": () => json({ id: 5, name: "one", track_ids: [1] }),
      });
      const w = await mountView();
      await pickFile(w, "one.m3u");
      dlgBtn("Open playlist").click();
      await settle();
      expect(useNavStore().view).toEqual({ name: "playlist", id: 5 });
      expect(usePlaylistsStore().lastImport).toBeNull();
      expect(dialog()).toBeNull();
    });

    it("closes the dialog with Close or Escape, and surfaces upload failures", async () => {
      mockFetch({ "/api/playlists/import": () => json({ error: "bad m3u" }, 400) });
      const w = await mountView();
      await pickFile(w, "bad.m3u");
      expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "error", title: "Playlist import failed", detail: "bad m3u" });
      expect(dialog()).toBeNull();
    });
  });
});

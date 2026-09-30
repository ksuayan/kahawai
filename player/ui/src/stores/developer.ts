import { defineStore } from "pinia";
import { ref } from "vue";
import { getDeveloperTools, inTauri, setDeveloperTools } from "../tauri";

/**
 * Settings → Developer tools. Off by default in a release build: no WebKit
 * menu (Reload, Inspect Element) where the app has no menu of its own, and
 * no Web Inspector. On, WebKit's menu comes back at once and the Inspector
 * after a restart. A development build always has both.
 */
export const useDeveloperStore = defineStore("developer", () => {
  /** The saved choice. */
  const enabled = ref(false);
  /** This window has the Web Inspector. */
  const inspector = ref(false);
  /** A development build (vite dev server, or a debug shell). */
  const devBuild = ref(import.meta.env.DEV);

  async function load(): Promise<void> {
    if (!inTauri()) return;
    const d = await getDeveloperTools();
    if (!d) return;
    enabled.value = d.enabled;
    inspector.value = d.inspector;
    devBuild.value = devBuild.value || d.dev_build;
  }

  async function set(on: boolean): Promise<void> {
    enabled.value = on;
    const d = await setDeveloperTools(on);
    if (d) inspector.value = d.inspector;
  }

  /** WebKit's own menu may show (everywhere, not just in text). */
  const nativeMenuAllowed = () => devBuild.value || enabled.value;

  return { enabled, inspector, devBuild, load, set, nativeMenuAllowed };
});

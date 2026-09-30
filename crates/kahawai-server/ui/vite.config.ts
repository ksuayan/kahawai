import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";

// The player's test-only helpers (tauri mock, mountApp) have no vue/pinia
// dependency of their own, so they're safe to share via this alias. Vue
// *components* are not shared this way — see src/ui/ (a small local copy):
// aliasing a .vue file that lives under the player's own node_modules tree
// resolves its `vue`/`pinia` imports to the player's copies, a second
// instance of each, which breaks Pinia's active-instance check and Vue's
// component context.
const playerSrc = fileURLToPath(new URL("../../../player/ui/src", import.meta.url));

// The app version shown in About: the bundle version the shell is built with.
const tauriConf = JSON.parse(readFileSync(new URL("../tauri.conf.json", import.meta.url), "utf8")) as { version?: string };
const appVersion = tauriConf.version ?? "dev";

// Tauri v2 expects the dev server on a fixed port; the player uses 1420.
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  define: { __APP_VERSION__: JSON.stringify(appVersion) },
  clearScreen: false,
  resolve: {
    alias: { "@pw": playerSrc },
  },
  server: {
    port: 1421,
    strictPort: true,
    // The repository, not just this UI: the About dialog's License tab
    // imports the top-level LICENSE (and the server UI shares the player's
    // styles). Files outside the repository stay off-limits.
    fs: { allow: [fileURLToPath(new URL("../../..", import.meta.url))] },
  },
  build: {
    target: "esnext",
    // Two pages: the app, and the splash window shown while it starts.
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        splash: fileURLToPath(new URL("./splash.html", import.meta.url)),
      },
    },
  },
  test: {
    environment: "happy-dom",
    setupFiles: ["src/test/setup.ts"],
    include: ["src/**/*.test.ts"],
    css: false,
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,vue}"],
      exclude: ["src/**/*.test.ts", "src/test/**", "src/main.ts", "src/vite-env.d.ts"],
      reporter: ["text-summary", "text"],
    },
  },
});

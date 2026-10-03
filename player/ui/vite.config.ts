import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
import cascadeLayers from "@csstools/postcss-cascade-layers";
import legacyTransforms from "./src/build/legacyTransforms";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// The app version shown in About: the bundle version the shell is built with.
const tauriConf = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8")) as { version?: string };
const appVersion = tauriConf.version ?? "dev";

// Android: devices such as the HiBy R4 ship an old system WebView (Chromium 91
// on Android 12) that the device cannot update. Tailwind 4 puts every rule in
// cascade layers (`@layer`, Chromium 99+), which that WebView throws away,
// leaving the app unstyled. KAHAWAI_LEGACY_WEBVIEW=1 (set by
// scripts/build-android.sh and start-dev-android.sh) flattens the layers with
// the PostCSS polyfill, which keeps the same cascade by adjusting specificity.
// It also lacks the individual translate / rotate / scale properties
// (Chromium 104+) Tailwind 4 moves things with, so those become one
// `transform` (src/build/legacyTransforms.ts): switch thumbs slide, dialogs
// centre. Tailwind already writes plain fallbacks for its color-mix() colors.
// The desktop build is left as it is.
const legacyWebView = process.env.KAHAWAI_LEGACY_WEBVIEW === "1";

// Tauri v2 expects the dev server on port 1420.
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  define: { __APP_VERSION__: JSON.stringify(appVersion) },
  css: { postcss: { plugins: legacyWebView ? [cascadeLayers(), legacyTransforms()] : [] } },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // The repository, not just this UI: the About dialog's License tab
    // imports the top-level LICENSE (and the server UI shares the player's
    // styles). Files outside the repository stay off-limits.
    fs: { allow: [fileURLToPath(new URL("../..", import.meta.url))] },
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
      exclude: ["src/**/*.test.ts", "src/test/**", "src/main.ts", "src/vite-env.d.ts", "src/types.ts"],
      reporter: ["text-summary", "text"],
    },
  },
});

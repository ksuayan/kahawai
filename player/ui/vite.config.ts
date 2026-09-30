import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// The app version shown in About: the bundle version the shell is built with.
const tauriConf = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8")) as { version?: string };
const appVersion = tauriConf.version ?? "dev";

// Tauri v2 expects the dev server on port 1420.
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  define: { __APP_VERSION__: JSON.stringify(appVersion) },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
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

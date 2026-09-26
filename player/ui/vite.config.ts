import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";

// Tauri v2 expects the dev server on port 1420.
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "esnext",
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

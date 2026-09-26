import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri v2 expects the dev server on port 1420.
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "esnext",
  },
});

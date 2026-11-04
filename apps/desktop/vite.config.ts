import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Tauri dev server contract: fixed port, no screen clearing, TAURI_* env exposed.
// TAURI_DEV_HOST is set by `tauri dev` for mobile targets.
const host = process.env.TAURI_DEV_HOST;
const debug = Boolean(process.env.TAURI_ENV_DEBUG);

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
  server: {
    port: 1420,
    strictPort: true,
    host: host ?? false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // WebView2 on Windows, WebKit elsewhere. Tailwind 4 needs Safari 16.4 / Chrome 111 (and the
    // UI uses color-mix() and container queries), so nothing older is supported anyway.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome111" : "safari16.4",
    minify: debug ? false : undefined,
    sourcemap: debug,
    chunkSizeWarningLimit: 1200,
  },
});

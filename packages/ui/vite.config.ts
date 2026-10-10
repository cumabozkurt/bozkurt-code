import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

// `vite` / `vite preview` run the playground (index.html; add ?mock for the demo backend).
// The playground talks to a real bozkurtd through a same-origin proxy (`/api/v1`, incl. the
// /events WebSocket), so the daemon's CORS allow-list does not matter here. Point it at another
// daemon with BOZKURT_API_TARGET=http://127.0.0.1:17878.
// `vite build` builds the library bundle into dist/ (React and Tauri APIs stay external).
const target = process.env.BOZKURT_API_TARGET || "http://127.0.0.1:7878";

export default defineConfig(({ command }) => ({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: { "/api/v1": { target, ws: true, changeOrigin: true } },
  },
  preview: {
    proxy: { "/api/v1": { target, ws: true, changeOrigin: true } },
  },
  build:
    command === "build"
      ? {
          lib: { entry: resolve(__dirname, "src/index.ts"), formats: ["es"], fileName: "index", cssFileName: "style" },
          rollupOptions: { external: ["react", "react-dom", "react/jsx-runtime", /^@tauri-apps\//] },
          sourcemap: true,
        }
      : undefined,
}));

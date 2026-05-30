import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(__dirname, "./src") } },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "dist" },
  test: {
    environment: "jsdom",
    globals: true,
    alias: {
      "@tauri-apps/api/core": path.resolve(__dirname, "src/__mocks__/tauri-core.ts"),
      "@tauri-apps/api/event": path.resolve(__dirname, "src/__mocks__/tauri-event.ts"),
      "@tauri-apps/api/webviewWindow": path.resolve(__dirname, "src/__mocks__/tauri-webview.ts"),
    },
  },
});

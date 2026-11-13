/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The dev server proxies the API to a locally running `hetzner-monitor serve`
// (default http://127.0.0.1:8080, override with HM_DEV_API).
const api = process.env.HM_DEV_API ?? "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    assetsDir: "assets",
    // No inline assets: the CSP allows only same-origin files (ADR-0015).
    assetsInlineLimit: 0,
    sourcemap: false,
  },
  server: {
    proxy: { "/api": api },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    css: false,
  },
});

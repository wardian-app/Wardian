import fs from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { viteDevServerHeaders } from "./src/config/viteDevServerHeaders";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;
const workspaceRoot = process.cwd();
const realWorkspaceRoot = fs.realpathSync.native(workspaceRoot);

// https://vite.dev/config/
export default defineConfig(async () => ({
  root: workspaceRoot,
  plugins: [react(), tailwindcss()],
  resolve: {
    preserveSymlinks: true,
    alias: {
      "@xterm/headless": "@xterm/headless/lib-headless/xterm-headless.js",
    },
  },

  // esbuild miscompiles xterm.js 6.0.0's `requestMode` (nested `const i`
  // shadowing the outer webpack require in the pre-minified UMD bundle),
  // producing a runtime `ReferenceError: i is not defined` when a provider
  // sends a DECRQM sequence (e.g. OpenCode's `CSI ? 2027 $ p` on startup,
  // or anything that hits the parser after). Terser handles the shadowing
  // correctly.
  build: {
    minify: "terser",
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    headers: viteDevServerHeaders,
    fs: {
      allow: [workspaceRoot, realWorkspaceRoot],
    },
    hmr: host
      ? {
        protocol: "ws",
        host,
        port: 1421,
      }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri` and gemini local artifacts
      ignored: ["**/src-tauri/**", "**/.learnings/**", "**/.gemini/**", "**/tmp/**"],
    },
  },
}));

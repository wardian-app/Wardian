import fs from "node:fs";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { viteDevServerHeaders } from "./src/config/viteDevServerHeaders";
import { viteWatchIgnored } from "./src/config/viteWatchIgnored";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;
const workspaceRoot = process.cwd();
const realWorkspaceRoot = fs.realpathSync.native(workspaceRoot);

function emitFilesRendererWorkers(): Plugin {
  const publicId = "virtual:wardian-files-renderer-workers";
  const resolvedId = `\0${publicId}`;
  return {
    name: "wardian-files-renderer-workers",
    apply: "build",
    resolveId(id) {
      return id === publicId ? resolvedId : undefined;
    },
    load(id) {
      if (id !== resolvedId) return undefined;
      return [
        'import MonacoEditorWorker from "monaco-editor/esm/vs/editor/editor.worker.js?worker";',
        'import PdfJsWorker from "pdfjs-dist/build/pdf.worker.min.mjs?worker";',
        "export const filesRendererWorkers = { MonacoEditorWorker, PdfJsWorker };",
      ].join("\n");
    },
    transformIndexHtml: {
      order: "pre",
      handler() {
        return [{
          tag: "script",
          attrs: { type: "module" },
          children: [
            `import { filesRendererWorkers } from "${publicId}";`,
            "void filesRendererWorkers;",
          ].join("\n"),
          injectTo: "head",
        }];
      },
    },
  };
}

// https://vite.dev/config/
export default defineConfig(async () => ({
  root: workspaceRoot,
  plugins: [react(), tailwindcss(), emitFilesRendererWorkers()],
  resolve: {
    preserveSymlinks: true,
    alias: {
      "@xterm/headless": "@xterm/headless/lib-headless/xterm-headless.js",
    },
  },

  // Align the dev-server dependency optimizer with the `build.target` below.
  // esbuild 0.28 no longer lowers destructuring for Vite's legacy default
  // optimizeDeps target (chrome87/es2020/...), so prebundling deps that use
  // destructuring (konva, d3, zustand, xyflow, ...) fails when the dev server
  // starts. Wardian's Tauri webviews are modern enough for ES2022.
  optimizeDeps: {
    esbuildOptions: {
      target: "es2022",
    },
  },

  // esbuild miscompiles xterm.js 6.0.0's `requestMode` (nested `const i`
  // shadowing the outer webpack require in the pre-minified UMD bundle),
  // producing a runtime `ReferenceError: i is not defined` when a provider
  // sends a DECRQM sequence (e.g. OpenCode's `CSI ? 2027 $ p` on startup,
  // or anything that hits the parser after). Terser handles the shadowing
  // correctly.
  build: {
    // esbuild 0.28 no longer lowers destructuring for Vite's legacy default
    // target; Wardian's Tauri webviews are modern enough for ES2022 output.
    target: "es2022",
    minify: "terser",
    rollupOptions: {
      output: {
        manualChunks(id) {
          const normalized = id.replace(/\\/g, "/");
          if (!normalized.includes("/node_modules/")) {
            return undefined;
          }
          if (normalized.includes("/node_modules/@xterm/addon-webgl/")) {
            return "vendor-terminal-webgl";
          }
          if (normalized.includes("/node_modules/@xterm/addon-")) {
            return "vendor-terminal-addons";
          }
          if (normalized.includes("/node_modules/@xterm/headless/")) {
            return "vendor-terminal-headless";
          }
          if (normalized.includes("/node_modules/@xterm/xterm/")) {
            return "vendor-terminal-core";
          }
          if (
            normalized.includes("/node_modules/@xyflow/react/") ||
            normalized.includes("/node_modules/graphology/") ||
            normalized.includes("/node_modules/sigma/")
          ) {
            return "vendor-graph";
          }
          if (
            normalized.includes("/node_modules/react/") ||
            normalized.includes("/node_modules/react-dom/") ||
            normalized.includes("/node_modules/scheduler/")
          ) {
            return "vendor-react";
          }
          if (normalized.includes("/node_modules/lucide-react/")) {
            return "vendor-icons";
          }
          if (normalized.includes("/node_modules/qrcode/")) {
            return "vendor-qrcode";
          }
          return "vendor";
        },
      },
    },
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
      // 3. tell Vite to ignore backend/runtime state that can churn during agent sessions
      ignored: [...viteWatchIgnored],
    },
  },
}));

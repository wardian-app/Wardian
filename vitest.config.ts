/// <reference types="vitest" />
import fs from "node:fs";
import path from "node:path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

const workspaceRoot = process.cwd();
const realWorkspaceRoot = fs.realpathSync.native(workspaceRoot);

export default defineConfig({
  root: workspaceRoot,
  plugins: [react()],
  resolve: {
    preserveSymlinks: true,
  },
  server: {
    fs: {
      allow: [workspaceRoot, realWorkspaceRoot],
    },
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: [path.join(workspaceRoot, "src/test/setup.ts")],
    include: ["src/**/*.test.{ts,tsx}"],
    css: false,
  },
});

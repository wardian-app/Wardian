import { defineConfig } from "@playwright/test";
import * as path from "path";
import * as os from "os";

const testHome = path.join(os.tmpdir(), "wardian-e2e-test-home");
const e2ePort = Number.parseInt(process.env.WARDIAN_E2E_PORT ?? "1420", 10);
const e2eHost = process.env.WARDIAN_E2E_HOST ?? "127.0.0.1";
const baseURL = process.env.WARDIAN_E2E_BASE_URL ?? `http://${e2eHost}:${e2ePort}`;
const reuseExistingServer = process.env.WARDIAN_E2E_REUSE_SERVER !== "0";
const parsedViewportWidth = Number.parseInt(process.env.WARDIAN_E2E_VIEWPORT_WIDTH ?? "1920", 10);
const parsedViewportHeight = Number.parseInt(process.env.WARDIAN_E2E_VIEWPORT_HEIGHT ?? "1080", 10);
const e2eViewport = {
  width: Number.isFinite(parsedViewportWidth) && parsedViewportWidth > 0 ? parsedViewportWidth : 1920,
  height: Number.isFinite(parsedViewportHeight) && parsedViewportHeight > 0 ? parsedViewportHeight : 1080,
};

export function createE2eConfig(options: { workbench?: boolean } = {}) {
  const workbench = options.workbench === true;
  return defineConfig({
    testDir: "./tests",
    workers: 1,
    timeout: 60_000,
    retries: 1,
    reporter: [["html", { open: "never" }]],
    outputDir: "./test-results",

    use: {
      baseURL,
      viewport: e2eViewport,
      trace: "retain-on-failure",
      screenshot: "only-on-failure",
    },

    webServer: {
      command: `npm run vite -- --host ${e2eHost} --port ${e2ePort} --strictPort`,
      url: baseURL,
      timeout: 180_000,
      reuseExistingServer: workbench ? false : reuseExistingServer,
      env: {
        WARDIAN_HOME: testHome,
        ...(workbench ? { VITE_WARDIAN_WORKBENCH: "1" } : {}),
      },
    },

    projects: [
      {
        name: "smoke",
        testMatch: "*.spec.ts",
      },
    ],
  });
}

export default createE2eConfig();

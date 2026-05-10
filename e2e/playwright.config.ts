import { defineConfig } from "@playwright/test";
import * as path from "path";
import * as os from "os";

const testHome = path.join(os.tmpdir(), "wardian-e2e-test-home");

export default defineConfig({
  testDir: "./tests",
  workers: 1,
  timeout: 60_000,
  retries: 1,
  reporter: [["html", { open: "never" }]],
  outputDir: "./test-results",

  use: {
    baseURL: "http://localhost:1420",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },

  webServer: {
    command: "npm run vite",
    url: "http://localhost:1420",
    timeout: 180_000,
    reuseExistingServer: true,
    env: {
      WARDIAN_HOME: testHome,
    },
  },

  projects: [
    {
      name: "smoke",
      testMatch: "*.spec.ts",
    },
  ],
});

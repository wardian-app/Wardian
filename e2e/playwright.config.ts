import { defineConfig } from "@playwright/test";
import * as path from "path";
import * as os from "os";

const testHome = path.join(os.tmpdir(), "wardian-e2e-test-home");

export default defineConfig({
  testDir: "./tests",
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
    command: "npm run tauri dev",
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
      testMatch: /^(?!screenshots).*\.spec\.ts$/,
    },
    {
      name: "screenshots",
      testMatch: "screenshots.spec.ts",
      use: {
        screenshot: "on",
        video: "off",
        // Pass screenshot output dir via env so the spec can use it
        // without hardcoding a path.
      },
    },
  ],
});

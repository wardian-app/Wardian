import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

import {
  assertNativePreflight,
  createNativeHarness,
  formatAppShellTimeoutMessage,
  nativeAppBuildArgs,
} from "../lib/harness.mjs";

test("native preflight reports missing tauri-driver clearly", () => {
  assert.throws(
    () =>
      assertNativePreflight({
        appPath: "D:/Development/Wardian/target/debug/Wardian.exe",
        platform: "win32",
        tauriDriverPath: null,
        nativeDriverPath: "C:/WebDriver/msedgedriver.exe",
      }),
    /tauri-driver was not found on PATH/,
  );
});

test("native preflight reports missing native driver clearly", () => {
  assert.throws(
    () =>
      assertNativePreflight({
        appPath: "D:/Development/Wardian/target/debug/Wardian.exe",
        platform: "win32",
        tauriDriverPath: "C:/Users/test/.cargo/bin/tauri-driver.exe",
        nativeDriverPath: null,
      }),
    /No native WebDriver binary was found/,
  );
});

test("native preflight accepts macOS when required drivers are configured", () => {
  assert.doesNotThrow(() =>
    assertNativePreflight({
      appPath: "/Applications/Wardian.app/Contents/MacOS/Wardian",
      platform: "darwin",
      tauriDriverPath: "/Users/test/.cargo/bin/tauri-driver",
      nativeDriverPath: "/usr/local/bin/chromedriver",
    }),
  );
});

test("native harness reads watch mode settings from the environment", async () => {
  const previousWatch = process.env.WARDIAN_E2E_WATCH;
  const previousDelay = process.env.WARDIAN_E2E_STEP_DELAY_MS;
  process.env.WARDIAN_E2E_WATCH = "1";
  process.env.WARDIAN_E2E_STEP_DELAY_MS = "25";

  try {
    const harness = await createNativeHarness();

    assert.equal(harness.watchMode, true);
    assert.equal(harness.watchStepDelayMs, 25);
  } finally {
    if (previousWatch === undefined) {
      delete process.env.WARDIAN_E2E_WATCH;
    } else {
      process.env.WARDIAN_E2E_WATCH = previousWatch;
    }
    if (previousDelay === undefined) {
      delete process.env.WARDIAN_E2E_STEP_DELAY_MS;
    } else {
      process.env.WARDIAN_E2E_STEP_DELAY_MS = previousDelay;
    }
  }
});

test("native app build args include explicit Cargo features from environment", () => {
  const previousFeatures = process.env.WARDIAN_NATIVE_BUILD_FEATURES;
  process.env.WARDIAN_NATIVE_BUILD_FEATURES = "terminal-trace";

  try {
    assert.deepEqual(nativeAppBuildArgs(), [
      "run",
      "tauri",
      "--",
      "build",
      "--debug",
      "--no-bundle",
      "--features",
      "terminal-trace",
    ]);
  } finally {
    if (previousFeatures === undefined) {
      delete process.env.WARDIAN_NATIVE_BUILD_FEATURES;
    } else {
      process.env.WARDIAN_NATIVE_BUILD_FEATURES = previousFeatures;
    }
  }
});

test("native harness resolves debug app from cargo metadata target directory", async () => {
  const harness = await createNativeHarness();
  const metadata = spawnSync("cargo", ["metadata", "--format-version=1", "--no-deps"], {
    cwd: harness.repoRoot,
    encoding: "utf8",
    shell: process.platform === "win32",
  });
  assert.equal(metadata.status, 0, metadata.stderr);
  const targetDirectory = JSON.parse(metadata.stdout).target_directory;
  const exe = process.platform === "win32" ? "Wardian.exe" : "Wardian";
  const sharedDebugApp = path.join(targetDirectory, "debug", exe);

  if (!fs.existsSync(sharedDebugApp)) {
    return;
  }

  assert.equal(harness.appPath, sharedDebugApp);
});

test("native app shell timeout explains dev server connection failures", () => {
  const message = formatAppShellTimeoutMessage({
    timeoutMs: 20000,
    currentUrl: "http://localhost:1420/",
    title: "localhost",
    bodyText: "This site can't be reached. localhost refused to connect.",
  });

  assert.match(message, /Vite dev server/);
  assert.match(message, /npm run vite/);
  assert.match(message, /npm run tauri -- build --debug --no-bundle/);
});

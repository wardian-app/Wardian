import test from "node:test";
import assert from "node:assert/strict";

import {
  assertNativePreflight,
  createNativeHarness,
  formatAppShellTimeoutMessage,
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

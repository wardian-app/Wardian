import test from "node:test";
import assert from "node:assert/strict";

import { assertNativePreflight } from "../lib/harness.mjs";

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

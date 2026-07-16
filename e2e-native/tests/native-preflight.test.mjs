import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

import {
  assertNativePreflight,
  createNativeHarness,
  formatAppShellTimeoutMessage,
  isRetryableNativeSessionStartError,
  nativeAppBuildArgs,
  prepareIsolatedHome,
  startNativeSession,
} from "../lib/harness.mjs";

test("native session startup retries transient WebDriver transport failures", () => {
  assert.equal(
    isRetryableNativeSessionStartError(new Error("ECONNRESET socket hang up")),
    true,
  );
  assert.equal(
    isRetryableNativeSessionStartError(new Error("tcp connect error: target machine actively refused it")),
    true,
  );
  assert.equal(isRetryableNativeSessionStartError(new Error("application assertion failed")), false);
});

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

test("native harness ignores ambient production WARDIAN_HOME", async () => {
  const previousHome = process.env.WARDIAN_HOME;
  const previousNativeHome = process.env.WARDIAN_E2E_NATIVE_HOME;
  process.env.WARDIAN_HOME = path.join(os.tmpdir(), `wardian-production-home-${process.pid}`);
  delete process.env.WARDIAN_E2E_NATIVE_HOME;

  try {
    const harness = await createNativeHarness();

    assert.notEqual(harness.isolatedHome, process.env.WARDIAN_HOME);
    assert.match(path.basename(harness.isolatedHome), /^wardian-e2e-native/);
  } finally {
    if (previousHome === undefined) {
      delete process.env.WARDIAN_HOME;
    } else {
      process.env.WARDIAN_HOME = previousHome;
    }
    if (previousNativeHome === undefined) {
      delete process.env.WARDIAN_E2E_NATIVE_HOME;
    } else {
      process.env.WARDIAN_E2E_NATIVE_HOME = previousNativeHome;
    }
  }
});

test("native harness refuses to delete an unsafe isolated home path", () => {
  const unsafeHome = path.join(process.cwd(), ".tmp", `unsafe-native-home-${process.pid}`);
  const sentinel = path.join(unsafeHome, "sentinel.txt");
  fs.mkdirSync(unsafeHome, { recursive: true });
  fs.writeFileSync(sentinel, "do not delete", "utf8");

  try {
    assert.throws(
      () => prepareIsolatedHome({ isolatedHome: unsafeHome }),
      /Refusing to reset unsafe native E2E home/,
    );
    assert.equal(fs.readFileSync(sentinel, "utf8"), "do not delete");
  } finally {
    fs.rmSync(unsafeHome, { recursive: true, force: true });
  }
});

test("native session infrastructure failures make the test process fail by default", async () => {
  const previousAllowSkip = process.env.WARDIAN_E2E_ALLOW_INFRA_SKIP;
  const previousExitCode = process.exitCode;
  delete process.env.WARDIAN_E2E_ALLOW_INFRA_SKIP;
  process.exitCode = undefined;

  try {
    await assert.rejects(
      () =>
        startNativeSession({
          repoRoot: process.cwd(),
          appPath: "D:/Development/Wardian/target/debug/Wardian.exe",
          platform: "win32",
          tauriDriverPath: null,
          nativeDriverPath: "C:/WebDriver/msedgedriver.exe",
        }),
      /tauri-driver was not found on PATH/,
    );
    assert.equal(process.exitCode, 1);
  } finally {
    if (previousAllowSkip === undefined) {
      delete process.env.WARDIAN_E2E_ALLOW_INFRA_SKIP;
    } else {
      process.env.WARDIAN_E2E_ALLOW_INFRA_SKIP = previousAllowSkip;
    }
    process.exitCode = previousExitCode;
  }
});

test("native session infrastructure failures can be explicitly skipped for local runs", async () => {
  const previousAllowSkip = process.env.WARDIAN_E2E_ALLOW_INFRA_SKIP;
  const previousExitCode = process.exitCode;
  process.env.WARDIAN_E2E_ALLOW_INFRA_SKIP = "1";
  process.exitCode = undefined;

  try {
    await assert.rejects(
      () =>
        startNativeSession({
          repoRoot: process.cwd(),
          appPath: "D:/Development/Wardian/target/debug/Wardian.exe",
          platform: "win32",
          tauriDriverPath: null,
          nativeDriverPath: "C:/WebDriver/msedgedriver.exe",
        }),
      /tauri-driver was not found on PATH/,
    );
    assert.equal(process.exitCode, undefined);
  } finally {
    if (previousAllowSkip === undefined) {
      delete process.env.WARDIAN_E2E_ALLOW_INFRA_SKIP;
    } else {
      process.env.WARDIAN_E2E_ALLOW_INFRA_SKIP = previousAllowSkip;
    }
    process.exitCode = previousExitCode;
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
      "--config",
      JSON.stringify({
        build: {
          beforeBuildCommand: "npm run build && npm run stage-cli:dev",
        },
      }),
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

// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

import {
  assertNativePreflight,
  createNativeHarness,
  ensureNativeAppBuilt,
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

test("native harness rejects a missing explicit app before driver startup", async () => {
  const previousApp = process.env.WARDIAN_NATIVE_APP;
  process.env.WARDIAN_NATIVE_APP = path.join(os.tmpdir(), `missing-wardian-app-${process.pid}.exe`);

  try {
    await assert.rejects(
      () => createNativeHarness(),
      (error) => error?.code === "EXPLICIT_APP_MISSING" && /WARDIAN_NATIVE_APP does not exist/.test(error.message),
    );
  } finally {
    if (previousApp === undefined) {
      delete process.env.WARDIAN_NATIVE_APP;
    } else {
      process.env.WARDIAN_NATIVE_APP = previousApp;
    }
  }
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

test("native harness registers the legacy TestClass fixture", () => {
  const safeHome = path.join(
    process.cwd(),
    ".tmp",
    "e2e-native",
    `fixture-class-${process.pid}`,
  );

  try {
    prepareIsolatedHome({ isolatedHome: safeHome });
    const classes = JSON.parse(
      fs.readFileSync(path.join(safeHome, "custom_classes.json"), "utf8"),
    );
    assert.deepEqual(classes, [
      {
        name: "TestClass",
        description: "Native test fixture class",
        is_default: false,
      },
    ]);
  } finally {
    fs.rmSync(safeHome, { recursive: true, force: true });
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

test("Windows native build transports config and features without npm environment", {
  skip: process.platform !== "win32",
}, () => {
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-native-build & argv "));
  const cliDirectory = path.join(fixture, "node_modules", "@tauri-apps", "cli");
  const cli = path.join(cliDirectory, "tauri.js");
  const output = path.join(fixture, "argv.json");
  const keys = ["npm_execpath", "WARDIAN_NATIVE_BUILD_FEATURES", "WARDIAN_NATIVE_APP"];
  const previous = keys.map((key) => process.env[key]);
  const previousExitCode = process.exitCode;
  try {
    fs.mkdirSync(cliDirectory, { recursive: true });
    fs.writeFileSync(path.join(fixture, "package.json"), JSON.stringify({
      scripts: { tauri: "node node_modules/@tauri-apps/cli/tauri.js" },
    }));
    fs.writeFileSync(cli, 'require("node:fs").writeFileSync("argv.json", JSON.stringify(process.argv.slice(2)));');
    delete process.env.npm_execpath;
    process.env.WARDIAN_NATIVE_BUILD_FEATURES = "terminal-trace native-test";
    // Existing file satisfies post-build discovery; this test never starts an app.
    process.env.WARDIAN_NATIVE_APP = cli;

    ensureNativeAppBuilt({ repoRoot: fixture });

    assert.deepEqual(JSON.parse(fs.readFileSync(output, "utf8")), [
      "build", "--debug", "--no-bundle", "--config",
      JSON.stringify({ build: { beforeBuildCommand: "npm run build && npm run stage-cli:dev" } }),
      "--features", "terminal-trace native-test",
    ]);
  } finally {
    keys.forEach((key, index) => {
      if (previous[index] === undefined) delete process.env[key];
      else process.env[key] = previous[index];
    });
    process.exitCode = previousExitCode;
    fs.rmSync(fixture, { recursive: true, force: true });
  }
});

test("native app build entry refreshes a cold harness from its built output", () => {
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-native-cold-output-"));
  const output = path.join(fixture, "Wardian.exe");
  let buildCalls = 0;
  try {
    const harness = { repoRoot: fixture, appPath: null };
    ensureNativeAppBuilt(harness, {
      buildInvocation: { command: process.execPath, args: ["-e", ""] },
      spawnSyncImpl: (command, args, options) => {
        buildCalls += 1;
        assert.equal(command, process.execPath);
        assert.deepEqual(args, ["-e", ""]);
        assert.equal(options.cwd, fixture);
        fs.writeFileSync(output, "fresh native output", "utf8");
        return { status: 0 };
      },
      resolveAppPathImpl: () => (fs.existsSync(output) ? output : null),
    });

    assert.equal(buildCalls, 1);
    assert.equal(harness.appPath, output);
  } finally {
    fs.rmSync(fixture, { recursive: true, force: true });
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

test("native consumers build before asserting an app path", () => {
  const testsRoot = path.join(process.cwd(), "e2e-native", "tests");
  const consumerFiles = fs.readdirSync(testsRoot)
    .filter((fileName) => fileName.endsWith("native.test.mjs"));

  for (const fileName of consumerFiles) {
    const source = fs.readFileSync(path.join(testsRoot, fileName), "utf8");
    let assertion;
    const assertions = /assert\.ok\(harness\.appPath\)/g;
    while ((assertion = assertions.exec(source)) !== null) {
      const testStart = source.lastIndexOf("test(", assertion.index);
      const precedingBuild = source.lastIndexOf("ensureNativeAppBuilt(harness)", assertion.index);
      assert.notEqual(
        testStart,
        -1,
        `${fileName} app-path assertion is outside a test block`,
      );
      assert.ok(
        precedingBuild > testStart,
        `${fileName} asserts harness.appPath before ensuring the native app is built in its test`,
      );
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

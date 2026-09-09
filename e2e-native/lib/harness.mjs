import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import net from "node:net";
import { spawn, spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import readline from "node:readline/promises";
import { fileURLToPath } from "node:url";
import { Builder, By, Capabilities, until } from "selenium-webdriver";
import {
  resolveBuiltCliPath,
  resolveExistingCliPath,
  resolveNativeAppArtifact,
} from "./native-artifact-resolution.mjs";

import { allocateSessionPorts, assertPortOwnedBy, portIsFree } from "./sessionPorts.mjs";
import { FROZEN_BIN_DIR, freezeArtifact, freezeRunArtifacts } from "./frozenArtifacts.mjs";
import {
  NATIVE_E2E_HOME_ENV,
  HOME_LOCK_DIRECTORY,
  acquireHomeLock,
  defaultNativeE2EHome,
  nativeRunId,
  releaseHomeLock,
} from "./sessionHome.mjs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");
const DEFAULT_WATCH_STEP_DELAY_MS = 750;

export { nativeRunId, releaseHomeLock };

/**
 * Identity of a binary this run consumed.
 *
 * The app is resolved from the configured Cargo target, which other work can
 * rebuild. Recording size and mtime makes a mid-run swap visible in evidence
 * instead of silently changing what was tested.
 */
function describeArtifact(artifactPath) {
  if (!artifactPath) {
    return null;
  }
  try {
    const stats = fs.statSync(artifactPath);
    return {
      path: artifactPath,
      bytes: stats.size,
      modifiedAt: stats.mtime.toISOString(),
    };
  } catch {
    return { path: artifactPath, bytes: null, modifiedAt: null };
  }
}

function existingPath(candidates) {
  for (const candidate of candidates) {
    if (candidate && fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
}

function resolveAppPath() {
  try {
    return resolveNativeAppArtifact({ repoRoot, env: process.env }).path;
  } catch (error) {
    if (error?.code === "APP_ARTIFACT_MISSING") {
      return null;
    }
    throw error;
  }
}

function splitPathEntries() {
  return (process.env.PATH || "")
    .split(path.delimiter)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function resolveCommand(nameCandidates) {
  for (const dir of splitPathEntries()) {
    for (const name of nameCandidates) {
      const candidate = path.join(dir, name);
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }
  return null;
}

function resolveNativeDriverPath() {
  if (process.env.WARDIAN_NATIVE_WEBDRIVER && fs.existsSync(process.env.WARDIAN_NATIVE_WEBDRIVER)) {
    return process.env.WARDIAN_NATIVE_WEBDRIVER;
  }

  if (process.platform === "win32") {
    return existingPath([
      path.join(repoRoot, "tools", "e2e-native", "msedgedriver.exe"),
      path.join(repoRoot, "tools", "e2e-native", "chromedriver.exe"),
      path.join(repoRoot, "msedgedriver.exe"),
      path.join(repoRoot, "chromedriver.exe"),
      resolveCommand(["msedgedriver.exe", "chromedriver.exe"]),
    ]);
  }

  return resolveCommand(["chromedriver", "geckodriver"]);
}

function resolveTauriDriverPath() {
  if (process.env.TAURI_DRIVER && fs.existsSync(process.env.TAURI_DRIVER)) {
    return process.env.TAURI_DRIVER;
  }

  return resolveCommand(["tauri-driver.exe", "tauri-driver"]);
}

/**
 * The CLI binary as built into the shared target.
 *
 * Returns null before it has been built. Callers that build it themselves can
 * freeze it afterwards with `freezeRunArtifacts`.
 */
export function resolveSharedCliPath() {
  return resolveExistingCliPath({ repoRoot, env: process.env });
}

/**
 * Resolve a just-built CLI from Cargo's configured target and freeze it into
 * this run's private home before a caller starts using it.
 */
export function freezeBuiltCliForRun(harness) {
  const sourcePath = resolveBuiltCliPath({
    repoRoot: harness.repoRoot,
    env: process.env,
  });
  const frozen = freezeArtifact(
    sourcePath,
    path.join(harness.isolatedHome, FROZEN_BIN_DIR),
  );
  if (!frozen) {
    throw new Error(`wardian-cli could not be frozen from ${sourcePath}.`);
  }
  harness.sharedCliPath = sourcePath;
  harness.cliPath = frozen.path;
  harness.cliArtifact = frozen;
  return frozen.path;
}

function resolveIsolatedHome(runId) {
  return process.env[NATIVE_E2E_HOME_ENV] || defaultNativeE2EHome(runId);
}

/** True when this run generated its own home rather than being handed one. */
function ownsGeneratedHome() {
  return !process.env[NATIVE_E2E_HOME_ENV];
}

function isPathInside(candidate, parent) {
  const relative = path.relative(parent, candidate);
  return relative === "" || (!!relative && !relative.startsWith("..") && !path.isAbsolute(relative));
}

function isSafeNativeE2EHome(homePath) {
  const resolvedHome = path.resolve(homePath || "");
  const root = path.parse(resolvedHome).root;
  if (!homePath || resolvedHome === root || resolvedHome === repoRoot) {
    return false;
  }

  const tempRoot = path.resolve(os.tmpdir());
  const repoNativeTempRoot = path.resolve(repoRoot, ".tmp", "e2e-native");
  const basename = path.basename(resolvedHome).toLowerCase();
  const namedNativeE2EHome =
    basename === "wardian-e2e-native-home" || basename.startsWith("wardian-e2e-native-");

  return (
    isPathInside(resolvedHome, repoNativeTempRoot) ||
    (isPathInside(resolvedHome, tempRoot) && namedNativeE2EHome)
  );
}

function readBooleanEnv(name) {
  const value = process.env[name];
  if (!value) {
    return false;
  }
  return ["1", "true", "yes", "on"].includes(value.trim().toLowerCase());
}

function nativeInfrastructureError(error) {
  if (!readBooleanEnv("WARDIAN_E2E_ALLOW_INFRA_SKIP")) {
    process.exitCode = 1;
  }
  return error instanceof Error ? error : new Error(String(error));
}

function readPositiveIntegerEnv(name, fallback) {
  const value = Number.parseInt(process.env[name] || "", 10);
  return Number.isFinite(value) && value >= 0 ? value : fallback;
}

function compactText(value, maxLength = 1200) {
  const normalized = String(value || "").replace(/\s+/g, " ").trim();
  if (normalized.length <= maxLength) {
    return normalized;
  }
  return `${normalized.slice(0, maxLength)}...`;
}

export async function createNativeHarness() {
  const watchMode = readBooleanEnv("WARDIAN_E2E_WATCH");
  const runId = process.env.WARDIAN_E2E_RUN_ID || nativeRunId();
  const appPath = resolveAppPath();
  return {
    runId,
    repoRoot,
    appPath,
    // Attribution before freezing; `prepareIsolatedHome` replaces this with the
    // run-private copy it actually executes.
    appArtifact: describeArtifact(appPath),
    // Where the CLI is built by consumers that use the shared target. Frozen
    // per run alongside the app so a concurrent rebuild cannot replace it
    // mid-run either.
    sharedCliPath: resolveSharedCliPath(),
    ownsGeneratedHome: ownsGeneratedHome(),
    isolatedHome: resolveIsolatedHome(runId),
    tauriDriverPath: resolveTauriDriverPath(),
    nativeDriverPath: resolveNativeDriverPath(),
    platform: process.platform,
    watchMode,
    watchStepDelayMs: watchMode
      ? readPositiveIntegerEnv("WARDIAN_E2E_STEP_DELAY_MS", DEFAULT_WATCH_STEP_DELAY_MS)
      : 0,
  };
}

export function assertNativePreflight(harness) {
  if (!harness.appPath) {
    throw new Error(
      "Wardian app binary not found. Build it first or set WARDIAN_NATIVE_APP."
    );
  }

  if (!harness.tauriDriverPath) {
    throw new Error(
      "tauri-driver was not found on PATH. Install it or set TAURI_DRIVER."
    );
  }

  if (!harness.nativeDriverPath) {
    throw new Error(
      "No native WebDriver binary was found. Install msedgedriver/chromedriver or set WARDIAN_NATIVE_WEBDRIVER."
    );
  }
}

function nativeBuildInvocation(root) {
  const args = nativeAppBuildArgs();
  if (process.platform === "win32") {
    // cmd.exe reparses the inline JSON and treats its && as a command separator.
    // Launch the installed CLI with Node so paths and config stay literal argv.
    const require = createRequire(path.join(root, "package.json"));
    return {
      command: process.execPath,
      args: [require.resolve("@tauri-apps/cli/tauri.js"), ...args.slice(3)],
    };
  }

  return {
    command: "npm",
    args,
  };
}

export function nativeAppBuildArgs() {
  const debugBuildConfig = JSON.stringify({
    build: {
      beforeBuildCommand: "npm run build && npm run stage-cli:dev",
    },
  });
  const args = [
    "run",
    "tauri",
    "--",
    "build",
    "--debug",
    "--no-bundle",
    "--config",
    debugBuildConfig,
  ];
  const features = (process.env.WARDIAN_NATIVE_BUILD_FEATURES || "").trim();
  if (features) {
    args.push("--features", features);
  }
  return args;
}

export function ensureNativeAppBuilt(
  harness,
  {
    buildInvocation = nativeBuildInvocation(harness.repoRoot),
    spawnSyncImpl = spawnSync,
    resolveAppPathImpl = resolveAppPath,
  } = {},
) {
  const build = spawnSyncImpl(
    buildInvocation.command,
    buildInvocation.args,
    {
      cwd: harness.repoRoot,
      stdio: "inherit",
    },
  );

  if (build.status !== 0) {
    throw nativeInfrastructureError(
      new Error(`Failed to build Wardian native app (exit ${build.status ?? 1}).`),
    );
  }

  const refreshedAppPath = resolveAppPathImpl();
  if (!refreshedAppPath) {
    throw nativeInfrastructureError(
      new Error(
        "Wardian app binary was not found after build. Set WARDIAN_NATIVE_APP if your output path is non-standard.",
      ),
    );
  }

  harness.appPath = refreshedAppPath;
}

export function prepareIsolatedHome(harness) {
  // Claim by run id, not pid. The runner claims the home and then spawns this
  // process, so a pid comparison would see the live parent as a foreign holder
  // and make the run refuse its own home.
  harness.homeLock = acquireHomeLock({
    home: harness.isolatedHome,
    runId: harness.runId,
  }).lock;
  if (!isSafeNativeE2EHome(harness.isolatedHome)) {
    throw new Error(
      `Refusing to reset unsafe native E2E home: ${harness.isolatedHome}. ` +
        `Use ${NATIVE_E2E_HOME_ENV} with a wardian-e2e-native-* path under ${os.tmpdir()} ` +
        `or a path under ${path.join(repoRoot, ".tmp", "e2e-native")}.`,
    );
  }

  let lastError = null;
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      for (const entry of fs.readdirSync(harness.isolatedHome, { withFileTypes: true })) {
        if (entry.name === HOME_LOCK_DIRECTORY) {
          continue;
        }
        fs.rmSync(path.join(harness.isolatedHome, entry.name), {
          recursive: true,
          force: true,
          maxRetries: 10,
          retryDelay: 100,
        });
      }
      lastError = null;
      break;
    } catch (error) {
      lastError = error;
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 150);
    }
  }
  if (lastError) {
    throw lastError;
  }
  fs.mkdirSync(harness.isolatedHome, { recursive: true });
  // The exclusive lock and its metadata were preserved throughout reset, so
  // there is no check-then-delete window in which another run can claim this
  // home while its contents are being removed.

  // Take a private copy of every binary this run executes. The normal build
  // target is shared between worktrees, so another build can replace the app or
  // the CLI while a session is live. Recording a hash of the shared path only
  // says what the run started with; copying is what stops the bytes changing
  // underneath it. This runs on the ordinary path, so no caller has to opt in.
  harness.frozenArtifacts = freezeRunArtifacts({
    home: harness.isolatedHome,
    appPath: harness.appPath,
    cliPath: harness.sharedCliPath,
  });
  if (harness.frozenArtifacts.app) {
    harness.appPath = harness.frozenArtifacts.app.path;
    harness.appArtifact = harness.frozenArtifacts.app;
  }
  if (harness.frozenArtifacts.cli) {
    harness.cliPath = harness.frozenArtifacts.cli.path;
  }

  // Native fixtures historically use TestClass. Keep that fixture explicitly
  // registered now that spawn_agent validates classes at the command boundary.
  fs.writeFileSync(
    path.join(harness.isolatedHome, "custom_classes.json"),
    JSON.stringify([
      {
        name: "TestClass",
        description: "Native test fixture class",
        is_default: false,
      },
    ]),
    "utf8",
  );
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function terminateChildProcess(child, signal = "SIGTERM", timeoutMs = 5000) {
  if (!child || child.exitCode !== null) {
    return;
  }

  await new Promise((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      clearTimeout(killTimer);
      clearTimeout(doneTimer);
      resolve();
    };
    const killTimer = setTimeout(() => {
      if (child.exitCode === null) {
        child.kill("SIGKILL");
      }
    }, Math.max(500, Math.floor(timeoutMs / 2)));
    const doneTimer = setTimeout(finish, timeoutMs);
    child.once("exit", finish);
    child.kill(signal);
  });
}

export async function watchStep(harness, label) {
  if (!harness.watchMode) {
    return;
  }

  console.log(`[native-watch] ${label}`);
  if (harness.watchStepDelayMs > 0) {
    await sleep(harness.watchStepDelayMs);
  }
}

function waitForPort({ port, host = "127.0.0.1", timeoutMs = 15000, processRef, logs }) {
  const startedAt = Date.now();

  return new Promise((resolve, reject) => {
    const tryConnect = () => {
      if (processRef.exitCode !== null) {
        reject(
          new Error(
            `tauri-driver exited before becoming ready (exit ${processRef.exitCode}).\n` +
              `--- tauri-driver stdout ---\n${logs().stdout}\n` +
              `--- tauri-driver stderr ---\n${logs().stderr}`
          )
        );
        return;
      }

      const socket = net.createConnection({ host, port });
      socket.once("connect", () => {
        socket.end();
        resolve();
      });
      socket.once("error", () => {
        socket.destroy();
        if (Date.now() - startedAt >= timeoutMs) {
          reject(
            new Error(
              `Timed out waiting for tauri-driver on ${host}:${port}.\n` +
                `--- tauri-driver stdout ---\n${logs().stdout}\n` +
                `--- tauri-driver stderr ---\n${logs().stderr}`
            )
          );
          return;
        }
        setTimeout(tryConnect, 250);
      });
    };

    tryConnect();
  });
}

async function startNativeSessionAttempt(harness) {
  const { driverPort, nativePort } = await allocateSessionPorts();
  harness.driverPort = driverPort;
  harness.nativeDriverPort = nativePort;

  for (const [label, port] of [["driver", driverPort], ["native driver", nativePort]]) {
    if (!(await portIsFree({ port }))) {
      throw new Error(
        `Refusing to start: port ${port} chosen for the ${label} is already in use by another ` +
          `process. A listener this run did not start must never be treated as its endpoint.`,
      );
    }
  }

  const tauriDriverArgs = ["--port", String(driverPort), "--native-port", String(nativePort)];
  if (harness.nativeDriverPath) {
    tauriDriverArgs.push("--native-driver", harness.nativeDriverPath);
  }

  const tauriDriver = spawn(harness.tauriDriverPath, tauriDriverArgs, {
    cwd: harness.repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      WARDIAN_HOME: harness.isolatedHome,
      WARDIAN_E2E_NATIVE_HOME: harness.isolatedHome,
    },
  });

  let stderr = "";
  let stdout = "";
  tauriDriver.stdout.on("data", (chunk) => {
    stdout += chunk.toString();
  });
  tauriDriver.stderr.on("data", (chunk) => {
    stderr += chunk.toString();
  });

  const logs = () => ({ stdout, stderr });

  try {
    await waitForPort({
      port: driverPort,
      processRef: tauriDriver,
      logs,
    });
    // The connect above can succeed against a listener that appeared while the
    // driver was dying. Re-check the child so an exited driver is never
    // reported as a live endpoint.
    if (tauriDriver.exitCode !== null) {
      throw new Error(
        `tauri-driver exited (${tauriDriver.exitCode}) while port ${driverPort} became reachable; ` +
          `refusing to use an endpoint this run does not own.\n` +
          `--- tauri-driver stdout ---\n${logs().stdout}\n` +
          `--- tauri-driver stderr ---\n${logs().stderr}`,
      );
    }
    // Neither of the checks above proves the listener is ours, only that the
    // port was free earlier and our child is alive. Establish actual ownership.
    harness.driverPortOwnership = assertPortOwnedBy({
      port: driverPort,
      processRef: tauriDriver,
    });
    // The native-driver endpoint is a second externally supplied socket. It
    // has the same free-check race as the WebDriver endpoint, so prove its
    // listener belongs to this tauri-driver tree before Selenium can use it.
    await waitForPort({
      port: nativePort,
      processRef: tauriDriver,
      logs,
    });
    harness.nativeDriverPortOwnership = assertPortOwnedBy({
      port: nativePort,
      processRef: tauriDriver,
    });
  } catch (error) {
    await terminateChildProcess(tauriDriver);
    throw error;
  }

  const capabilities = new Capabilities();
  capabilities.setBrowserName("wry");
  capabilities.set("tauri:options", {
    application: harness.appPath,
  });

  try {
    const driver = await new Builder()
      .withCapabilities(capabilities)
      .usingServer(`http://127.0.0.1:${driverPort}/`)
      .build();

    return {
      driver,
      tauriDriver,
      async close() {
        if (
          harness.watchMode &&
          process.env.WARDIAN_E2E_WATCH_KEEP_OPEN !== "0" &&
          process.stdin.isTTY
        ) {
          const rl = readline.createInterface({
            input: process.stdin,
            output: process.stdout,
          });
          try {
            await rl.question("[native-watch] Press Enter to close the Wardian test window...");
          } finally {
            rl.close();
          }
        }

        try {
          await driver.quit();
        } finally {
          await terminateChildProcess(tauriDriver);
        }
      },
      logs,
    };
  } catch (error) {
    await terminateChildProcess(tauriDriver);
    throw new Error(
      `Failed to start native Tauri session: ${error}\n--- tauri-driver stdout ---\n${stdout}\n--- tauri-driver stderr ---\n${stderr}`,
    );
  }
}

export function isRetryableNativeSessionStartError(error) {
  const text = String(error instanceof Error ? error.message : error).toLowerCase();
  return (
    text.includes("sessionnotcreatederror") ||
    text.includes("chrome not reachable") ||
    text.includes("microsoft edge failed to start") ||
    text.includes("can not listen to address") ||
    text.includes("econnreset") ||
    text.includes("tcp connect error") ||
    text.includes("timed out waiting for tauri-driver")
  );
}

export async function startNativeSession(harness) {
  try {
    assertNativePreflight(harness);
  } catch (error) {
    throw nativeInfrastructureError(error);
  }

  const maxAttempts = Math.max(1, readPositiveIntegerEnv("WARDIAN_NATIVE_SESSION_START_ATTEMPTS", 2));
  let lastError = null;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      return await startNativeSessionAttempt(harness);
    } catch (error) {
      lastError = error;
      if (attempt >= maxAttempts || !isRetryableNativeSessionStartError(error)) {
        throw nativeInfrastructureError(error);
      }
      await sleep(750 * attempt);
    }
  }

  throw nativeInfrastructureError(lastError ?? new Error("Failed to start native Tauri session."));
}

export function formatAppShellTimeoutMessage({
  timeoutMs,
  currentUrl = "",
  title = "",
  bodyText = "",
}) {
  const details = [
    `Timed out after ${timeoutMs}ms waiting for [data-testid="app-shell"].`,
    `url: ${currentUrl || "<unknown>"}`,
    `title: ${title || "<unknown>"}`,
  ];

  const lowerUrl = currentUrl.toLowerCase();
  const lowerBody = bodyText.toLowerCase();
  if (
    lowerUrl.includes("localhost:1420") ||
    lowerBody.includes("localhost refused to connect") ||
    lowerBody.includes("this site can't be reached") ||
    lowerBody.includes("this site can’t be reached")
  ) {
    details.push(
      "The native WebView appears to be loading the Vite dev server, but the dev server is not reachable.",
      "Start it with `npm run vite`, or rebuild the debug app with `npm run tauri -- build --debug --no-bundle` before using the fast native runner.",
    );
  }

  const compactBody = compactText(bodyText);
  if (compactBody) {
    details.push(`body: ${compactBody}`);
  }

  return details.join("\n");
}

async function readAppShellDiagnostics(driver) {
  try {
    return await driver.executeScript(() => ({
      currentUrl: window.location.href,
      title: document.title,
      bodyText: document.body?.innerText || "",
    }));
  } catch (error) {
    return {
      currentUrl: "",
      title: "",
      bodyText: `Unable to read WebView diagnostics: ${String(error)}`,
    };
  }
}

export async function waitForAppShell(driver, timeoutMs = 15000) {
  try {
    const shell = await driver.wait(
      until.elementLocated(By.css('[data-testid="app-shell"]')),
      timeoutMs,
    );
    await driver.wait(until.elementIsVisible(shell), timeoutMs);
    return shell;
  } catch (error) {
    const diagnostics = await readAppShellDiagnostics(driver);
    throw new Error(`${formatAppShellTimeoutMessage({ timeoutMs, ...diagnostics })}\n${error}`);
  }
}

/**
 * Invokes a Tauri command through the native WebView without converting a
 * structured backend rejection into the unhelpful string "[object Object]".
 */
export async function invokeTauriResult(driver, command, args = {}) {
  return driver.executeAsyncScript((commandName, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(commandName, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({
        ok: false,
        error: error && typeof error === "object"
          ? error
          : { message: String(error) },
      }),
    );
  }, command, args);
}

/** Invoke a Tauri command through the native WebView and fail on rejection. */
export async function invokeTauri(driver, command, args = {}) {
  const result = await invokeTauriResult(driver, command, args);
  if (!result?.ok) {
    const detail = result?.error?.message ?? JSON.stringify(result?.error ?? null);
    throw new Error(`${command} failed: ${detail}`);
  }
  return result.value;
}

/**
 * Starts a native Tauri event capture owned by the current WebView. The caller
 * must release it with `stopTauriEventCapture` before closing the session.
 */
export async function startTauriEventCapture(driver, eventName) {
  const captureId = `native-event-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  const result = await driver.executeAsyncScript((name, id, done) => {
    const captures = window.__WARDIAN_NATIVE_EVENT_CAPTURES__ ??= {};
    const capture = { events: [], callbackId: null, eventId: null };
    captures[id] = capture;
    capture.callbackId = window.__TAURI_INTERNALS__.transformCallback((event) => {
      capture.events.push(event?.payload ?? event);
    });
    window.__TAURI_INTERNALS__.invoke("plugin:event|listen", {
      event: name,
      target: { kind: "Any" },
      handler: capture.callbackId,
    }).then(
      (eventId) => {
        capture.eventId = eventId;
        done({ ok: true });
      },
      (error) => {
        delete captures[id];
        done({ ok: false, error: String(error) });
      },
    );
  }, eventName, captureId);
  if (!result?.ok) {
    throw new Error(`Failed to listen for ${eventName}: ${result?.error}`);
  }
  return { captureId, eventName };
}

/** Returns the payloads observed by a native event capture. */
export async function readTauriEventCapture(driver, capture) {
  return driver.executeScript((id) => (
    window.__WARDIAN_NATIVE_EVENT_CAPTURES__?.[id]?.events ?? []
  ), capture.captureId);
}

/** Waits until a matching payload is observed without mocking the event bus. */
export async function waitForTauriEvent(
  driver,
  capture,
  predicate,
  timeoutMs = 10_000,
) {
  const startedAt = Date.now();
  let events = [];
  while (Date.now() - startedAt < timeoutMs) {
    events = await readTauriEventCapture(driver, capture);
    const match = events.find(predicate);
    if (match) return match;
    await sleep(25);
  }
  throw new Error(
    `Timed out waiting for ${capture.eventName}. Observed: ${JSON.stringify(events)}`,
  );
}

/** Releases a native event capture and its WebView callback. */
export async function stopTauriEventCapture(driver, capture) {
  const result = await driver.executeAsyncScript((name, id, done) => {
    const captures = window.__WARDIAN_NATIVE_EVENT_CAPTURES__;
    const current = captures?.[id];
    if (!current) {
      done({ ok: true });
      return;
    }
    const finish = () => {
      window.__TAURI_INTERNALS__.unregisterCallback(current.callbackId);
      delete captures[id];
      done({ ok: true });
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__?.unregisterListener(name, current.eventId);
    window.__TAURI_INTERNALS__.invoke("plugin:event|unlisten", {
      event: name,
      eventId: current.eventId,
    }).then(finish, (error) => done({ ok: false, error: String(error) }));
  }, capture.eventName, capture.captureId);
  if (!result?.ok) {
    throw new Error(`Failed to unlisten from ${capture.eventName}: ${result?.error}`);
  }
}

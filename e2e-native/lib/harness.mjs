import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import net from "node:net";
import { spawn, spawnSync } from "node:child_process";
import readline from "node:readline/promises";
import { fileURLToPath } from "node:url";
import { Builder, By, Capabilities, until } from "selenium-webdriver";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");
const DEFAULT_WATCH_STEP_DELAY_MS = 750;

function existingPath(candidates) {
  for (const candidate of candidates) {
    if (candidate && fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
}

function resolveAppPath() {
  if (process.env.WARDIAN_NATIVE_APP && fs.existsSync(process.env.WARDIAN_NATIVE_APP)) {
    return process.env.WARDIAN_NATIVE_APP;
  }

  if (process.platform === "win32") {
    return existingPath([
      path.join(repoRoot, "target", "debug", "Wardian.exe"),
      path.join(repoRoot, "target", "release", "Wardian.exe"),
      path.join(repoRoot, "src-tauri", "target", "debug", "Wardian.exe"),
      path.join(repoRoot, "src-tauri", "target", "release", "Wardian.exe"),
    ]);
  }

  if (process.platform === "darwin") {
    return existingPath([
      path.join(
        repoRoot,
        "target",
        "debug",
        "bundle",
        "macos",
        "Wardian.app",
        "Contents",
        "MacOS",
        "Wardian",
      ),
      path.join(
        repoRoot,
        "target",
        "release",
        "bundle",
        "macos",
        "Wardian.app",
        "Contents",
        "MacOS",
        "Wardian",
      ),
      path.join(
        repoRoot,
        "src-tauri",
        "target",
        "debug",
        "bundle",
        "macos",
        "Wardian.app",
        "Contents",
        "MacOS",
        "Wardian",
      ),
      path.join(
        repoRoot,
        "src-tauri",
        "target",
        "release",
        "bundle",
        "macos",
        "Wardian.app",
        "Contents",
        "MacOS",
        "Wardian",
      ),
    ]);
  }

  return existingPath([
    path.join(repoRoot, "target", "debug", "Wardian"),
    path.join(repoRoot, "target", "release", "Wardian"),
    path.join(repoRoot, "target", "debug", "wardian"),
    path.join(repoRoot, "target", "release", "wardian"),
    path.join(repoRoot, "src-tauri", "target", "debug", "Wardian"),
    path.join(repoRoot, "src-tauri", "target", "release", "Wardian"),
    path.join(repoRoot, "src-tauri", "target", "debug", "wardian"),
    path.join(repoRoot, "src-tauri", "target", "release", "wardian"),
  ]);
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

function resolveIsolatedHome() {
  return process.env.WARDIAN_HOME || path.join(os.tmpdir(), "wardian-e2e-native-home");
}

function readBooleanEnv(name) {
  const value = process.env[name];
  if (!value) {
    return false;
  }
  return ["1", "true", "yes", "on"].includes(value.trim().toLowerCase());
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
  return {
    repoRoot,
    appPath: resolveAppPath(),
    isolatedHome: resolveIsolatedHome(),
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

function npmCommand() {
  return process.platform === "win32" ? "npm.cmd" : "npm";
}

export function ensureNativeAppBuilt(harness) {
  const build = spawnSync(
    npmCommand(),
    ["run", "tauri", "--", "build", "--debug", "--no-bundle"],
    {
      cwd: harness.repoRoot,
      stdio: "inherit",
      shell: process.platform === "win32",
    },
  );

  if (build.status !== 0) {
    throw new Error(`Failed to build Wardian native app (exit ${build.status ?? 1}).`);
  }

  const refreshedAppPath = resolveAppPath();
  if (!refreshedAppPath) {
    throw new Error(
      "Wardian app binary was not found after build. Set WARDIAN_NATIVE_APP if your output path is non-standard."
    );
  }

  harness.appPath = refreshedAppPath;
}

export function prepareIsolatedHome(harness) {
  let lastError = null;
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      fs.rmSync(harness.isolatedHome, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
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
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
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

export async function startNativeSession(harness) {
  assertNativePreflight(harness);

  const tauriDriverArgs = [];
  if (harness.nativeDriverPath) {
    tauriDriverArgs.push("--native-driver", harness.nativeDriverPath);
  }

  const tauriDriver = spawn(harness.tauriDriverPath, tauriDriverArgs, {
    cwd: harness.repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      WARDIAN_HOME: harness.isolatedHome,
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

  await waitForPort({
    port: 4444,
    processRef: tauriDriver,
    logs,
  });

  const capabilities = new Capabilities();
  capabilities.setBrowserName("wry");
  capabilities.set("tauri:options", {
    application: harness.appPath,
  });

  try {
    const driver = await new Builder()
      .withCapabilities(capabilities)
      .usingServer("http://127.0.0.1:4444/")
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
          tauriDriver.kill();
        }
      },
      logs,
    };
  } catch (error) {
    tauriDriver.kill();
    throw new Error(
      `Failed to start native Tauri session: ${error}\n--- tauri-driver stdout ---\n${stdout}\n--- tauri-driver stderr ---\n${stderr}`
    );
  }
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

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import net from "node:net";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { Builder, By, Capabilities, until } from "selenium-webdriver";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");

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
      path.join(repoRoot, "src-tauri", "target", "debug", "Wardian.exe"),
      path.join(repoRoot, "src-tauri", "target", "release", "Wardian.exe"),
    ]);
  }

  if (process.platform === "darwin") {
    return existingPath([
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

export async function createNativeHarness() {
  return {
    repoRoot,
    appPath: resolveAppPath(),
    isolatedHome: resolveIsolatedHome(),
    tauriDriverPath: resolveTauriDriverPath(),
    nativeDriverPath: resolveNativeDriverPath(),
    platform: process.platform,
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
  fs.rmSync(harness.isolatedHome, { recursive: true, force: true });
  fs.mkdirSync(harness.isolatedHome, { recursive: true });
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

export async function waitForAppShell(driver, timeoutMs = 15000) {
  const shell = await driver.wait(
    until.elementLocated(By.css('[data-testid="app-shell"]')),
    timeoutMs,
  );
  await driver.wait(until.elementIsVisible(shell), timeoutMs);
  return shell;
}

#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");
const nativeToolsDir = path.join(repoRoot, "tools", "e2e-native");

function splitPathEntries(env = process.env) {
  return (env.PATH || "")
    .split(path.delimiter)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

export function parseArgs(argv) {
  const options = {
    skipTauriDriver: false,
    skipNativeDriver: false,
    help: false,
  };

  for (const arg of argv) {
    switch (arg) {
      case "--skip-tauri-driver":
        options.skipTauriDriver = true;
        break;
      case "--skip-native-driver":
      case "--skip-webdriver":
      case "--skip-edge-driver":
        options.skipNativeDriver = true;
        break;
      case "-h":
      case "--help":
        options.help = true;
        break;
      default:
        throw new Error(`Unknown option: ${arg}`);
    }
  }

  return options;
}

function commandNames(command) {
  if (process.platform === "win32" && !/\.(exe|cmd|bat|ps1)$/i.test(command)) {
    return [`${command}.exe`, `${command}.cmd`, `${command}.bat`, command];
  }
  return [command];
}

export function resolveCommand(command, env = process.env) {
  for (const dir of splitPathEntries(env)) {
    for (const name of commandNames(command)) {
      const candidate = path.join(dir, name);
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }
  return null;
}

export function nativeDriverCandidates(platform = process.platform) {
  if (platform === "win32") {
    return [
      path.join(nativeToolsDir, "msedgedriver.exe"),
      path.join(nativeToolsDir, "chromedriver.exe"),
      "msedgedriver",
      "chromedriver",
    ];
  }

  if (platform === "darwin") {
    return ["chromedriver", "geckodriver"];
  }

  return ["chromedriver", "geckodriver"];
}

function existingDriver(candidates) {
  for (const candidate of candidates) {
    if (path.isAbsolute(candidate) && fs.existsSync(candidate)) {
      return candidate;
    }

    if (!path.isAbsolute(candidate)) {
      const command = resolveCommand(candidate);
      if (command) {
        return command;
      }
    }
  }
  return null;
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd || repoRoot,
    stdio: "inherit",
    shell: process.platform === "win32",
    env: process.env,
  });

  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed with exit ${result.status ?? 1}`);
  }
}

function ensureCargo() {
  const cargo = resolveCommand("cargo");
  if (!cargo) {
    throw new Error("Required command 'cargo' was not found on PATH.");
  }
  return cargo;
}

function ensureTauriDriver(options) {
  if (options.skipTauriDriver) {
    console.log("Skipping tauri-driver setup.");
    return;
  }

  const tauriDriver = resolveCommand("tauri-driver");
  if (tauriDriver) {
    console.log(`tauri-driver already installed at ${tauriDriver}`);
    return;
  }

  console.log("Installing tauri-driver...");
  run("cargo", ["install", "tauri-driver", "--locked"]);
}

function ensureWindowsEdgeDriver() {
  fs.mkdirSync(nativeToolsDir, { recursive: true });

  const existing = existingDriver(nativeDriverCandidates("win32"));
  if (existing) {
    console.log(`Native WebDriver already available at ${existing}`);
    return;
  }

  let tool = resolveCommand("msedgedriver-tool");
  if (!tool) {
    console.log("Installing msedgedriver-tool...");
    run("cargo", ["install", "--git", "https://github.com/chippers/msedgedriver-tool"]);
    tool = resolveCommand("msedgedriver-tool");
  }

  if (!tool) {
    throw new Error("msedgedriver-tool was not found after installation.");
  }

  console.log(`Downloading matching msedgedriver into ${nativeToolsDir}...`);
  run(tool, [], { cwd: nativeToolsDir });
}

export function nativeDriverGuidance(platform = process.platform) {
  if (platform === "win32") {
    return "Install Microsoft Edge WebDriver or ChromeDriver, or set WARDIAN_NATIVE_WEBDRIVER.";
  }

  if (platform === "darwin") {
    return "Install chromedriver or geckodriver and ensure it is on PATH, or set WARDIAN_NATIVE_WEBDRIVER.";
  }

  return "Install chromedriver or geckodriver with your OS package manager and ensure it is on PATH, or set WARDIAN_NATIVE_WEBDRIVER.";
}

function ensureNativeDriver(options) {
  if (options.skipNativeDriver) {
    console.log("Skipping native WebDriver setup.");
    return;
  }

  if (process.env.WARDIAN_NATIVE_WEBDRIVER && fs.existsSync(process.env.WARDIAN_NATIVE_WEBDRIVER)) {
    console.log(`Using WARDIAN_NATIVE_WEBDRIVER=${process.env.WARDIAN_NATIVE_WEBDRIVER}`);
    return;
  }

  const existing = existingDriver(nativeDriverCandidates());
  if (existing) {
    console.log(`Native WebDriver already available at ${existing}`);
    return;
  }

  if (process.platform === "win32") {
    ensureWindowsEdgeDriver();
    return;
  }

  console.warn(`Native WebDriver was not found. ${nativeDriverGuidance()}`);
}

function printHelp() {
  console.log(`Usage: node scripts/setup-native-e2e.mjs [options]

Options:
  --skip-tauri-driver       Do not install/check tauri-driver.
  --skip-native-driver      Do not install/check native WebDriver.
  --skip-webdriver          Alias for --skip-native-driver.
  --skip-edge-driver        Backward-compatible alias for --skip-native-driver.
  -h, --help                Show this help.
`);
}

export function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) {
    printHelp();
    return;
  }

  ensureCargo();
  ensureTauriDriver(options);
  ensureNativeDriver(options);

  console.log("");
  console.log("Native E2E prerequisites are prepared.");
  console.log("Recommended next steps:");
  console.log("  npm run test:e2e:native");
  console.log("  WARDIAN_E2E_REAL_OPENCODE=1 npm run test:e2e:native");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}

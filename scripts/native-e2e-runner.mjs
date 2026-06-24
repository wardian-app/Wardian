import { spawn } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const NODE_TEST_ARGS = ["--test", "--test-concurrency=1"];
const DEFAULT_NATIVE_E2E_HOME = path.join(os.tmpdir(), "wardian-e2e-native-home");
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

export function createNativeE2eRunPlans({ requestedTargets, defaultTargets }) {
  const targets = requestedTargets.length > 0 ? requestedTargets : defaultTargets;
  return targets.map((target) => ({
    command: process.execPath,
    args: [...NODE_TEST_ARGS, target],
  }));
}

function runChild(plan, env) {
  return new Promise((resolve) => {
    const child = spawn(plan.command, plan.args, {
      stdio: "inherit",
      env,
    });

    child.on("exit", (code, signal) => {
      resolve({ code: code ?? 1, signal });
    });
  });
}

function isPathInside(parent, candidate) {
  const relative = path.relative(path.resolve(parent), path.resolve(candidate));
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function isSafeNativeE2eHome(nativeHome) {
  const resolvedHome = path.resolve(nativeHome);
  return (
    (isPathInside(os.tmpdir(), resolvedHome) && path.basename(resolvedHome).startsWith("wardian-e2e-native")) ||
    isPathInside(path.join(repoRoot, ".tmp", "e2e-native"), resolvedHome)
  );
}

function powerShellSingleQuoted(value) {
  return `'${String(value).replace(/'/g, "''")}'`;
}

export function createWindowsNativeE2eCleanupPlan({
  platform = process.platform,
  env = process.env,
} = {}) {
  if (platform !== "win32") {
    return null;
  }

  const nativeHome = env.WARDIAN_E2E_NATIVE_HOME || DEFAULT_NATIVE_E2E_HOME;
  if (!isSafeNativeE2eHome(nativeHome)) {
    return null;
  }

  const command =
    `$homePath = ${powerShellSingleQuoted(path.resolve(nativeHome))}; ` +
    "$escaped = [Regex]::Escape($homePath); " +
    "$escapedSlash = [Regex]::Escape($homePath.Replace('\\', '/')); " +
    "Get-CimInstance Win32_Process | " +
    "Where-Object { $_.CommandLine -and ($_.CommandLine -match $escaped -or $_.CommandLine -match $escapedSlash) -and $_.ProcessId -ne $PID } | " +
    "ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }";

  return {
    command: "powershell.exe",
    args: ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", command],
  };
}

async function runCleanupPlan(plan, env) {
  if (!plan) {
    return;
  }

  await runChild(plan, env);
}

export async function runNativeE2eTargets({
  requestedTargets,
  defaultTargets,
  env = process.env,
}) {
  const plans = createNativeE2eRunPlans({ requestedTargets, defaultTargets });
  const cleanupPlan = createWindowsNativeE2eCleanupPlan({ env });

  for (const plan of plans) {
    await runCleanupPlan(cleanupPlan, env);
    const result = await runChild(plan, env);
    await runCleanupPlan(cleanupPlan, env);
    if (result.signal) {
      process.kill(process.pid, result.signal);
      return 1;
    }
    if (result.code !== 0) {
      return result.code;
    }
  }

  return 0;
}

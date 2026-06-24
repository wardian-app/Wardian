import test from "node:test";
import assert from "node:assert/strict";
import os from "node:os";
import path from "node:path";

import {
  createNativeE2eRunPlans,
  createWindowsNativeE2eCleanupPlan,
} from "../../scripts/native-e2e-runner.mjs";

test("native e2e runner isolates each test target in a separate node process", () => {
  const plans = createNativeE2eRunPlans({
    requestedTargets: [],
    defaultTargets: [
      "e2e-native/tests/alpha.test.mjs",
      "e2e-native/tests/beta.test.mjs",
    ],
  });

  assert.deepEqual(plans, [
    {
      command: process.execPath,
      args: ["--test", "--test-concurrency=1", "e2e-native/tests/alpha.test.mjs"],
    },
    {
      command: process.execPath,
      args: ["--test", "--test-concurrency=1", "e2e-native/tests/beta.test.mjs"],
    },
  ]);
});

test("native e2e runner preserves explicitly requested target ordering", () => {
  const plans = createNativeE2eRunPlans({
    requestedTargets: [
      "e2e-native/tests/worktree-cli-native.test.mjs",
      "e2e-native/tests/cli-shared-state-native.test.mjs",
    ],
    defaultTargets: ["e2e-native/tests/alpha.test.mjs"],
  });

  assert.deepEqual(
    plans.map((plan) => plan.args.at(-1)),
    [
      "e2e-native/tests/worktree-cli-native.test.mjs",
      "e2e-native/tests/cli-shared-state-native.test.mjs",
    ],
  );
});

test("native e2e runner builds a Windows cleanup plan scoped to the isolated home", () => {
  const nativeHome = path.join(os.tmpdir(), "wardian-e2e-native-home");
  const plan = createWindowsNativeE2eCleanupPlan({
    platform: "win32",
    env: {
      WARDIAN_E2E_NATIVE_HOME: nativeHome,
    },
  });

  assert.deepEqual(plan, {
    command: "powershell.exe",
    args: [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-Command",
      `$homePath = '${nativeHome.replace(/'/g, "''")}'; ` +
        "$escaped = [Regex]::Escape($homePath); " +
        "$escapedSlash = [Regex]::Escape($homePath.Replace('\\', '/')); " +
        "Get-CimInstance Win32_Process | " +
        "Where-Object { $_.CommandLine -and ($_.CommandLine -match $escaped -or $_.CommandLine -match $escapedSlash) -and $_.ProcessId -ne $PID } | " +
        "ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }",
    ],
  });
});

test("native e2e runner skips cleanup for non-native-e2e homes", () => {
  assert.equal(
    createWindowsNativeE2eCleanupPlan({
      platform: "win32",
      env: { WARDIAN_E2E_NATIVE_HOME: "C:\\Users\\test\\.wardian" },
    }),
    null,
  );
  assert.equal(
    createWindowsNativeE2eCleanupPlan({
      platform: "linux",
      env: { WARDIAN_E2E_NATIVE_HOME: "/tmp/wardian-e2e-native-home" },
    }),
    null,
  );
});

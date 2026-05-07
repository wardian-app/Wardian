import { spawn } from "node:child_process";

const testTargets = process.argv.slice(2);
const args = ["--test", "--test-concurrency=1"];

if (testTargets.length > 0) {
  args.push(...testTargets);
} else {
  args.push("e2e-native/tests");
}

console.log("[native-watch] Running native E2E in visible watch mode.");
console.log("[native-watch] Rebuild first if the app window shows a localhost:1420 connection failure.");

const child = spawn(process.execPath, args, {
  stdio: "inherit",
  env: {
    ...process.env,
    WARDIAN_NATIVE_SKIP_BUILD: "1",
    WARDIAN_E2E_WATCH: "1",
    WARDIAN_E2E_STEP_DELAY_MS: process.env.WARDIAN_E2E_STEP_DELAY_MS || "750",
    WARDIAN_E2E_WATCH_KEEP_OPEN: process.env.WARDIAN_E2E_WATCH_KEEP_OPEN || "1",
  },
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});

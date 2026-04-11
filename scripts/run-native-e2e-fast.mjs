import { spawn } from "node:child_process";

const testTargets = process.argv.slice(2);
const args = ["--test", "--test-concurrency=1"];

if (testTargets.length > 0) {
  args.push(...testTargets);
} else {
  args.push("e2e-native/tests");
}

const child = spawn(process.execPath, args, {
  stdio: "inherit",
  env: {
    ...process.env,
    WARDIAN_NATIVE_SKIP_BUILD: "1",
  },
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});

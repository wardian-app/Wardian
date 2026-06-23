import { spawn } from "node:child_process";
import { nativeE2eTestTargets } from "./native-e2e-targets.mjs";

const testTargets = process.argv.slice(2);
const args = ["--test", "--test-concurrency=1"];

if (testTargets.length > 0) {
  args.push(...testTargets);
} else {
  args.push(...nativeE2eTestTargets());
}

const child = spawn(process.execPath, args, {
  stdio: "inherit",
  env: process.env,
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});

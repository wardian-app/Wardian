import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";

const requestedTargets = process.argv.slice(2);
const testTargets =
  requestedTargets.length > 0 ? expandTargets(requestedTargets) : expandTargets(["e2e-native/tests"]);

const child = spawn(process.execPath, ["--test", "--test-concurrency=1", ...testTargets], {
  stdio: "inherit",
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});

function expandTargets(targets) {
  return targets.flatMap((target) => {
    const stat = fs.existsSync(target) ? fs.statSync(target) : null;
    if (!stat?.isDirectory()) {
      return [target];
    }

    return fs
      .readdirSync(target, { withFileTypes: true })
      .flatMap((entry) => expandTargets([path.join(target, entry.name)]))
      .filter((file) => file.endsWith(".test.mjs"))
      .sort();
  });
}

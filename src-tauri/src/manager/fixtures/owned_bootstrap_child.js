import fs from "node:fs";
import { spawn } from "node:child_process";

const mode = process.argv[2];
const threadId = "thread-controlled-bootstrap-fixture";

if (process.env.WARDIAN_BOOTSTRAP_TEST_PID_FILE && mode !== "leader-exited") {
  fs.writeFileSync(process.env.WARDIAN_BOOTSTRAP_TEST_PID_FILE, String(process.pid));
}

const started = `${JSON.stringify({ type: "thread.started", thread_id: threadId })}\n`;

if (mode === "stderr-flood") {
  process.stderr.write(Buffer.alloc(1024 * 1024, 120), () => {
    process.stdout.write(started);
  });
} else if (mode === "early-marker") {
  process.stdout.write(started);
  process.stdout.write(`${JSON.stringify({ type: "turn.completed" })}\n`);
  setInterval(() => {}, 1000);
} else if (mode === "silent") {
  setInterval(() => {}, 1000);
} else if (mode === "leader-exited") {
  fs.writeFileSync(process.env.WARDIAN_BOOTSTRAP_TEST_LEADER, String(process.pid));
  const child = spawn(process.execPath, [process.argv[1], "silent"], {
    stdio: "inherit",
    env: process.env,
    detached: true,
  });
  child.on("error", error => {
    fs.writeFileSync(process.env.WARDIAN_BOOTSTRAP_TEST_FAILURE, String(error));
    process.exitCode = 65;
  });
  child.unref();
} else if (mode === "capture-config") {
  fs.writeFileSync(process.env.WARDIAN_BOOTSTRAP_TEST_CAPTURE, JSON.stringify({
    config: fs.readFileSync(`${process.env.CODEX_HOME}/config.toml`, "utf8"),
    auth: fs.readFileSync(`${process.env.CODEX_HOME}/auth.json`, "utf8"),
  }));
  process.stdout.write(started);
} else {
  process.exitCode = 64;
}

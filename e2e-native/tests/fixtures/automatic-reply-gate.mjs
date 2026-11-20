import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { randomUUID } from "node:crypto";
import assert from "node:assert/strict";

/** Hold one worker operation until release, without publishing any message. */
export async function waitForReplyRelease({ readyPath, releasePath, deadlineAt, pollMs = 50 }) {
  assert.ok(Number.isFinite(deadlineAt) && deadlineAt > Date.now(), "The original gate deadline expired");
  assert.ok(Number.isSafeInteger(pollMs) && pollMs > 0, "Invalid gate interval");
  try {
    await fs.access(releasePath);
    throw new Error("The gate was released before the worker became ready");
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  await fs.writeFile(readyPath, "ready\n", { flag: "wx" });
  while (Date.now() < deadlineAt) {
    try {
      if ((await fs.readFile(releasePath, "utf8")).trim() === "release") {
        await fs.writeFile(readyPath + ".consumed", "consumed\n", { flag: "wx" });
        return "WORKER_ONLY_" + randomUUID();
      }
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    await new Promise((resolve) => setTimeout(resolve, Math.min(pollMs, Math.max(1, deadlineAt - Date.now()))));
  }
  throw new Error("The original worker gate deadline expired");
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  assert.equal(process.argv[2], "--deadline", "The fixture requires the granted work deadline");
  assert.match(process.argv[3] ?? "", /(Z|[+-]\d{2}:\d{2})$/u, "The gate requires an explicit UTC offset");
  const deadlineAt = Date.parse(process.argv[3] ?? "");
  const root = path.dirname(fileURLToPath(import.meta.url));
  const marker = await waitForReplyRelease({
    readyPath: path.join(root, "ready"),
    releasePath: path.resolve(root, "..", "..", "..", "control", "reply-release"),
    deadlineAt,
  });
  process.stdout.write(marker + "\n");
}

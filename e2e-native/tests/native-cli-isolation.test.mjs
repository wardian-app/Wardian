// @tier nightly — Deterministic source contract for run-private CLI callers.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

const REPO_ROOT = path.resolve(import.meta.dirname, "../..");
const CLI_CONSUMERS = [
  "agent-memory-native.test.mjs",
  "antigravity-native.test.mjs",
  "artifact-presentation-native.test.mjs",
  "automation-completion-native.test.mjs",
  "browser-default-address-native.test.mjs",
  "browser-surface-native.test.mjs",
  "cli-shared-state-native.test.mjs",
  "provider-advanced-config-native.test.mjs",
  "provider-delivery-real-native.test.mjs",
  "topology-cli-native.test.mjs",
  "worktree-cli-native.test.mjs",
].map((name) => path.join(REPO_ROOT, "e2e-native", "tests", name));

test("native CLI consumers use the shared resolver and run-private freeze glue", () => {
  for (const filePath of CLI_CONSUMERS) {
    const source = fs.readFileSync(filePath, "utf8");
    assert.match(source, /freezeBuiltCliForRun/, filePath);
    assert.doesNotMatch(source, /resolveBuiltCliPath/, filePath);
    assert.doesNotMatch(source, /path\.join\(harness\.repoRoot, ["']target["']/, filePath);
  }
});

test("shared CLI probe delegates to effective Cargo target only", () => {
  const source = fs.readFileSync(path.join(REPO_ROOT, "e2e-native", "lib", "harness.mjs"), "utf8");
  assert.match(source, /return resolveExistingCliPath\(\{ repoRoot, env: process\.env \}\);/);
  assert.doesNotMatch(source, /path\.join\(repoRoot, ["']target["']/);
});

// @tier nightly — Exercises the exported harness without starting a native app.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { prepareIsolatedHome } from "../lib/harness.mjs";
import { acquireHomeLock, HOME_LOCK_FILE, readHomeLock, releaseHomeLock } from "../lib/sessionHome.mjs";

const repoRoot = fileURLToPath(new URL("../../", import.meta.url));

function fixture(t, valid = false) {
  const parent = path.join(repoRoot, ".tmp", valid ? "e2e-native" : "home-preflight-tests");
  fs.mkdirSync(parent, { recursive: true });
  const root = fs.mkdtempSync(path.join(parent, "preflight-"));
  t.after(() => {
    const relative = path.relative(parent, path.resolve(root));
    assert.ok(relative && !path.isAbsolute(relative) && !relative.startsWith(".."));
    fs.rmSync(root, { recursive: true, force: true });
  });
  return root;
}

test("unsafe absent home is rejected before creating a directory or lock", (t) => {
  const isolatedHome = path.join(fixture(t), "absent-home");
  const harness = { isolatedHome, runId: "unsafe-absent" };
  assert.throws(() => prepareIsolatedHome(harness), /Refusing to reset unsafe native E2E home/);
  assert.equal(fs.existsSync(isolatedHome), false);
  assert.equal(harness.homeLock, undefined);
});

test("unsafe existing home is rejected without changing its contents", (t) => {
  const isolatedHome = fixture(t);
  const sentinel = path.join(isolatedHome, "sentinel.txt");
  fs.writeFileSync(sentinel, "retain these bytes");
  const harness = { isolatedHome, runId: "unsafe-existing" };
  assert.throws(() => prepareIsolatedHome(harness), /Refusing to reset unsafe native E2E home/);
  assert.deepEqual(fs.readdirSync(isolatedHome), ["sentinel.txt"]);
  assert.equal(fs.readFileSync(sentinel, "utf8"), "retain these bytes");
  assert.equal(harness.homeLock, undefined);
});

test("unsafe home rejection preserves an existing stale lock exactly", (t) => {
  const isolatedHome = fixture(t);
  const lockFile = path.join(isolatedHome, HOME_LOCK_FILE);
  fs.mkdirSync(path.dirname(lockFile));
  const original = '{"runId":"old-run","pid":-1,"startedAt":"retained"}\n';
  fs.writeFileSync(lockFile, original);
  assert.throws(() => prepareIsolatedHome({ isolatedHome, runId: "new-run" }), /Refusing to reset unsafe native E2E home/);
  assert.equal(fs.readFileSync(lockFile, "utf8"), original);
});

test("valid home reset preserves the same-run exclusive claim", (t) => {
  const isolatedHome = fixture(t, true);
  const runId = "valid-preflight";
  const original = acquireHomeLock({ home: isolatedHome, runId }).lock;
  fs.writeFileSync(path.join(isolatedHome, "old-state.txt"), "discard fixture state");
  const harness = { isolatedHome, runId };
  prepareIsolatedHome(harness);
  assert.deepEqual(harness.homeLock, original);
  assert.deepEqual(readHomeLock(isolatedHome), original);
  assert.equal(fs.existsSync(path.join(isolatedHome, "old-state.txt")), false);
  assert.equal(JSON.parse(fs.readFileSync(path.join(isolatedHome, "custom_classes.json"), "utf8"))[0].name, "TestClass");
  releaseHomeLock({ home: isolatedHome, runId });
  assert.equal(readHomeLock(isolatedHome), null);
});

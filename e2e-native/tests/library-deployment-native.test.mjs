import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
  watchStep,
} from "../lib/harness.mjs";

/**
 * Native E2E coverage for the library redesign's junction-based deployment
 * engine (SDD Task 17, step 2). These claims cannot be proven at the browser
 * E2E layer, which has no real Tauri backend behind it (see
 * e2e/tests/library-redesign.spec.ts): a deploy must create a real
 * junction/reparse point on Windows, undeploy must remove it without
 * touching the library source, and rename must relink an existing
 * deployment to the renamed source.
 *
 * Runs through the app's real invoke bridge (window.__TAURI_INTERNALS__),
 * exactly like e2e-native/tests/cli-shared-state-native.test.mjs, against a
 * real Wardian home seeded directly on disk (no CLI needed here — the
 * library CLI does not exist yet, see the SDD Task 17 brief).
 */

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

/** Seeds a single library skill source under `home` for deployment tests. */
function seedSkillSource(home, relPath, content) {
  const dir = path.join(home, "library", "skills", ...relPath.split("/"));
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "SKILL.md"), content);
  return dir;
}

async function invokeCommand(driver, command, args) {
  const result = await driver.executeAsyncScript(
    (command, args, done) => {
      window.__TAURI_INTERNALS__.invoke(command, args).then(
        (value) => done({ ok: true, value }),
        (error) => done({ ok: false, error: String(error) }),
      );
    },
    command,
    args,
  );
  assert.equal(result.ok, true, `${command} failed: ${result.error}`);
  return result.value;
}

/**
 * A junction is not reported as a symlink by Node's `fs.lstatSync`
 * (`isSymbolicLink()` is false on Windows for IO_REPARSE_TAG_MOUNT_POINT),
 * but it IS a reparse point, and only reparse points support `readlink`.
 * A plain directory throws `EINVAL` here; a junction resolves to its
 * target. This is the cross-platform-safe way to assert "this is a real
 * link, not a copy" without depending on a Windows-only attribute API.
 */
function assertIsDirectoryLink(target) {
  assert.doesNotThrow(() => fs.readlinkSync(target), `expected ${target} to be a directory link (junction/symlink)`);
}

test("library skill deployment: deploy creates a live junction, undeploy leaves the source, rename relinks, delete cleans up", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);

  // Seed the library source directly on the isolated WARDIAN_HOME the app
  // will read — no CLI round trip needed for this.
  const sourceDir = seedSkillSource(harness.isolatedHome, "planner", "one");
  const deployedPath = path.join(
    harness.isolatedHome,
    "classes",
    "Architect",
    ".agents",
    "skills",
    "planner",
  );
  const renamedSourceDir = path.join(harness.isolatedHome, "library", "skills", "planner-v2");
  const renamedDeployedPath = path.join(
    harness.isolatedHome,
    "classes",
    "Architect",
    ".agents",
    "skills",
    "planner-v2",
  );

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  t.after(async () => {
    await session.close();
  });

  await waitForAppShell(session.driver, 20000);
  await watchStep(harness, "Wardian app shell is ready for library deployment smoke");

  // --- Deploy: creates a real junction/reparse point, not a copy. ---
  await invokeCommand(session.driver, "set_skill_deployments", {
    sourcePath: "planner",
    targets: [{ target_type: "class", target_id: "Architect" }],
  });

  assert.doesNotThrow(() => fs.statSync(deployedPath), "deployed target must resolve through the link");
  assertIsDirectoryLink(deployedPath);
  assert.deepEqual(fs.readdirSync(deployedPath).sort(), ["SKILL.md"]);

  // Live-link proof: editing the source is visible through the deployed
  // target without redeploying.
  fs.writeFileSync(path.join(sourceDir, "SKILL.md"), "two");
  assert.equal(
    fs.readFileSync(path.join(deployedPath, "SKILL.md"), "utf8"),
    "two",
    "editing the library source must be visible through the deployed junction",
  );

  // --- Undeploy: removes the link without touching the source. ---
  await invokeCommand(session.driver, "set_skill_deployments", {
    sourcePath: "planner",
    targets: [],
  });

  assert.equal(fs.existsSync(deployedPath), false, "undeploy must remove the deployed target");
  assert.equal(
    fs.readFileSync(path.join(sourceDir, "SKILL.md"), "utf8"),
    "two",
    "undeploy must not touch the library source",
  );

  // Redeploy so the rename step has a live target to relink.
  await invokeCommand(session.driver, "set_skill_deployments", {
    sourcePath: "planner",
    targets: [{ target_type: "class", target_id: "Architect" }],
  });
  assert.doesNotThrow(() => fs.statSync(deployedPath));

  // --- Rename: the junction follows the renamed source. ---
  await invokeCommand(session.driver, "rename_library_entry", {
    section: "skills",
    fromPath: "planner",
    toPath: "planner-v2",
  });

  assert.equal(fs.existsSync(path.join(harness.isolatedHome, "library", "skills", "planner")), false, "old source path must be gone after rename");
  assert.doesNotThrow(() => fs.statSync(renamedSourceDir), "renamed source must exist");
  assert.equal(fs.existsSync(deployedPath), false, "old deployed path must be gone after rename");
  assert.doesNotThrow(() => fs.statSync(renamedDeployedPath), "renamed deployed target must resolve through the relinked junction");
  assertIsDirectoryLink(renamedDeployedPath);

  fs.writeFileSync(path.join(renamedSourceDir, "SKILL.md"), "three");
  assert.equal(
    fs.readFileSync(path.join(renamedDeployedPath, "SKILL.md"), "utf8"),
    "three",
    "the relinked junction must still be live after rename",
  );

  // --- Delete: source and every deployment are cleaned up. ---
  await invokeCommand(session.driver, "delete_library_entry", {
    section: "skills",
    path: "planner-v2",
  });

  assert.equal(fs.existsSync(renamedSourceDir), false, "delete must remove the library source");
  assert.equal(fs.existsSync(renamedDeployedPath), false, "delete must remove the deployed target");
});

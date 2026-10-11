// @tier nightly — Runs on the nightly schedule; too slow or too broad for every nightly pull request.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { FROZEN_BIN_DIR, freezeArtifact, freezeRunArtifacts } from "../lib/frozenArtifacts.mjs";
import { resolveExistingCliPath } from "../lib/native-artifact-resolution.mjs";

function scratch(label) {
  const dir = path.join(os.tmpdir(), `wardian-e2e-native-frozen-${label}-${process.pid}`);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

/**
 * The point of freezing: the shared build target can be rewritten by another
 * worktree while a session is live. Recording a hash only attributes what the
 * run started with, so the run executes its own copy instead.
 */
test("a rebuild of the shared source cannot change what the run executes", () => {
  const sharedTarget = scratch("source");
  const home = scratch("home");
  try {
    const shared = path.join(sharedTarget, "Wardian.exe");
    fs.writeFileSync(shared, "original build");

    const frozen = freezeArtifact(shared, path.join(home, FROZEN_BIN_DIR));
    assert.ok(frozen, "freezing an existing binary returns its identity");
    assert.equal(frozen.source, shared);
    assert.notEqual(frozen.path, shared, "the run must execute a copy, not the shared path");

    // Another worktree rebuilds the shared target mid-run.
    fs.writeFileSync(shared, "a completely different build");

    assert.equal(fs.readFileSync(frozen.path, "utf8"), "original build");
    assert.equal(frozen.bytes, "original build".length);
  } finally {
    fs.rmSync(sharedTarget, { recursive: true, force: true });
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("sidecar libraries travel with the executable", () => {
  const sharedTarget = scratch("sidecar");
  const home = scratch("sidecar-home");
  try {
    fs.writeFileSync(path.join(sharedTarget, "Wardian.exe"), "app");
    fs.writeFileSync(path.join(sharedTarget, "wardian_app_lib.dll"), "lib");
    fs.writeFileSync(path.join(sharedTarget, "notes.txt"), "not a binary");

    const frozen = freezeArtifact(path.join(sharedTarget, "Wardian.exe"), path.join(home, FROZEN_BIN_DIR));
    const frozenDir = path.dirname(frozen.path);

    // Copying the executable alone would produce something that cannot start.
    assert.equal(fs.existsSync(path.join(frozenDir, "wardian_app_lib.dll")), true);
    assert.equal(fs.existsSync(path.join(frozenDir, "notes.txt")), false, "only link libraries travel");
  } finally {
    fs.rmSync(sharedTarget, { recursive: true, force: true });
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("Tauri runtime payload directories travel with the executable", () => {
  const sharedTarget = scratch("runtime-payload");
  const home = scratch("runtime-payload-home");
  try {
    fs.writeFileSync(path.join(sharedTarget, "Wardian.exe"), "app");
    fs.mkdirSync(path.join(sharedTarget, "conpty", "x64"), { recursive: true });
    fs.mkdirSync(path.join(sharedTarget, "resources", "bin"), { recursive: true });
    fs.writeFileSync(path.join(sharedTarget, "conpty", "x64", "conpty.dll"), "conpty");
    fs.writeFileSync(path.join(sharedTarget, "conpty", "x64", "OpenConsole.exe"), "openconsole");
    fs.writeFileSync(path.join(sharedTarget, "resources", "bin", "wardian-cli.exe"), "cli");

    const frozen = freezeArtifact(path.join(sharedTarget, "Wardian.exe"), path.join(home, FROZEN_BIN_DIR));
    const frozenDir = path.dirname(frozen.path);

    assert.equal(
      fs.readFileSync(path.join(frozenDir, "conpty", "x64", "conpty.dll"), "utf8"),
      "conpty",
    );
    assert.equal(
      fs.readFileSync(path.join(frozenDir, "conpty", "x64", "OpenConsole.exe"), "utf8"),
      "openconsole",
    );
    assert.equal(
      fs.readFileSync(path.join(frozenDir, "resources", "bin", "wardian-cli.exe"), "utf8"),
      "cli",
    );
  } finally {
    fs.rmSync(sharedTarget, { recursive: true, force: true });
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("a later CLI freeze cannot replace runtime payloads already in use", () => {
  const firstBuild = scratch("runtime-payload-first");
  const laterBuild = scratch("runtime-payload-later");
  const home = scratch("runtime-payload-existing");
  try {
    for (const build of [firstBuild, laterBuild]) {
      fs.writeFileSync(path.join(build, "Wardian.exe"), "app");
      fs.writeFileSync(path.join(build, "wardian-cli.exe"), "cli");
      fs.mkdirSync(path.join(build, "resources", "bin"), { recursive: true });
    }
    fs.writeFileSync(path.join(firstBuild, "resources", "bin", "wardian-cli.exe"), "payload-v1");
    fs.writeFileSync(path.join(laterBuild, "resources", "bin", "wardian-cli.exe"), "payload-v2-from-other-build");
    fs.writeFileSync(path.join(laterBuild, "resources", "bin", "new-helper.exe"), "new payload");

    const frozenDir = path.join(home, FROZEN_BIN_DIR);
    freezeArtifact(path.join(firstBuild, "Wardian.exe"), frozenDir);
    freezeArtifact(path.join(laterBuild, "wardian-cli.exe"), frozenDir);

    assert.equal(
      fs.readFileSync(path.join(frozenDir, "resources", "bin", "wardian-cli.exe"), "utf8"),
      "payload-v1",
      "later freezes must not replace payload bytes already used by the run",
    );
    assert.equal(
      fs.readFileSync(path.join(frozenDir, "resources", "bin", "new-helper.exe"), "utf8"),
      "new payload",
      "later freezes may fill missing payload files",
    );
  } finally {
    for (const dir of [firstBuild, laterBuild, home]) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }
});

test("Unicode runtime paths survive both freeze orders without replacing existing payloads", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-frozen-unicode-"));
  try {
    const builds = ["app", "cli"].map((label) => path.join(root, label));
    const payloads = [
      path.join("conpty", "x64", "conpty.dll"),
      path.join("resources", "bin", "wardian-cli.exe"),
      path.join("resources", "nested-é中", "payload-é中.txt"),
    ];
    for (const [index, build] of builds.entries()) {
      fs.mkdirSync(build, { recursive: true });
      fs.writeFileSync(path.join(build, index === 0 ? "Wardian.exe" : "wardian-cli.exe"), `binary-${index}`);
      for (const payload of payloads) {
        fs.mkdirSync(path.dirname(path.join(build, payload)), { recursive: true });
        fs.writeFileSync(path.join(build, payload), `payload-${index}`);
      }
      fs.writeFileSync(path.join(build, "resources", `only-${index}-é中.txt`), `unique-${index}`);
    }

    for (const order of [[0, 1], [1, 0]]) {
      const frozenDir = path.join(root, `home-é中-${order[0]}`, FROZEN_BIN_DIR);
      for (const index of order) {
        const name = index === 0 ? "Wardian.exe" : "wardian-cli.exe";
        freezeArtifact(path.join(builds[index], name), frozenDir);
        assert.equal(fs.readFileSync(path.join(frozenDir, name), "utf8"), `binary-${index}`);
        for (const payload of payloads) {
          assert.equal(
            fs.readFileSync(path.join(frozenDir, payload), "utf8"),
            `payload-${order[0]}`,
            "each freeze must use the exact Unicode path and preserve the first payload bytes",
          );
        }
        assert.equal(
          fs.readFileSync(path.join(frozenDir, "resources", `only-${index}-é中.txt`), "utf8"),
          `unique-${index}`,
          "the second freeze must still add missing files",
        );
      }
    }
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("each run freezes into its own home, so runs cannot share a binary", () => {
  const sharedTarget = scratch("shared");
  const homeA = scratch("run-a");
  const homeB = scratch("run-b");
  try {
    const app = path.join(sharedTarget, "Wardian.exe");
    const cli = path.join(sharedTarget, "wardian-cli.exe");
    fs.writeFileSync(app, "app build");
    fs.writeFileSync(cli, "cli build");
    fs.mkdirSync(path.join(sharedTarget, "resources", "bin"), { recursive: true });
    fs.writeFileSync(path.join(sharedTarget, "resources", "bin", "wardian-cli.exe"), "cli build");

    const a = freezeRunArtifacts({ home: homeA, appPath: app, cliPath: cli, platform: "win32" });
    const b = freezeRunArtifacts({ home: homeB, appPath: app, cliPath: cli, platform: "win32" });

    assert.notEqual(a.app.path, b.app.path);
    assert.notEqual(a.cli.path, b.cli.path);
    assert.equal(a.app.sha256, b.app.sha256, "same source yields the same identity");
    assert.equal(path.dirname(a.app.path), path.join(homeA, FROZEN_BIN_DIR));

    // Each app-backed run carries its matching packaged CLI.
    assert.equal(fs.readFileSync(a.cli.path, "utf8"), "cli build");
  } finally {
    for (const dir of [sharedTarget, homeA, homeB]) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }
});

test("an explicit paired freeze imports only the app runtime and checks the frozen CLI", () => {
  const root = scratch("paired-inputs");
  try {
    const appDir = path.join(root, "app");
    const cliDir = path.join(root, "cli");
    const home = path.join(root, "home");
    const app = path.join(appDir, "Wardian.exe");
    const cli = path.join(cliDir, "wardian-cli.exe");
    const packaged = path.join(appDir, "resources", "bin", "wardian-cli.exe");
    fs.mkdirSync(path.dirname(packaged), { recursive: true });
    fs.mkdirSync(path.join(cliDir, "resources"), { recursive: true });
    fs.writeFileSync(app, "paired app");
    fs.writeFileSync(cli, "paired cli");
    fs.writeFileSync(packaged, "paired cli");
    fs.writeFileSync(path.join(appDir, "own.dll"), "app library");
    fs.writeFileSync(path.join(cliDir, "foreign.dll"), "unqualified CLI library");
    fs.writeFileSync(path.join(cliDir, "resources", "foreign.txt"), "unqualified CLI resource");
    const frozen = freezeRunArtifacts({ home, appPath: app, cliPath: cli, pairedCli: true, platform: "win32" });
    assert.equal(fs.readFileSync(frozen.cli.path, "utf8"), "paired cli");
    assert.equal(fs.readFileSync(path.join(frozen.dir, "own.dll"), "utf8"), "app library");
    assert.equal(fs.existsSync(path.join(frozen.dir, "foreign.dll")), false);
    assert.equal(fs.existsSync(path.join(frozen.dir, "resources", "foreign.txt")), false);

    // A newly matching source pair must not conceal an older private payload.
    // The second validation checks what will actually execute, after copying.
    fs.writeFileSync(cli, "changed pair");
    fs.writeFileSync(packaged, "changed pair");
    assert.throws(() => freezeRunArtifacts({ home, appPath: app, cliPath: cli, pairedCli: true, platform: "win32" }),
      (error) => error.code === "PAIRED_CLI_MISMATCH");
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test("default app-backed freeze uses the packaged CLI after a standalone debug build", () => {
  const root = scratch("default-pair");
  try {
    const target = path.join(root, "target");
    const appDir = path.join(target, "debug");
    const app = path.join(appDir, "Wardian.exe");
    const debugCli = path.join(appDir, "wardian-cli.exe");
    const packaged = path.join(appDir, "resources", "bin", "wardian-cli.exe");
    fs.mkdirSync(path.dirname(packaged), { recursive: true });
    fs.mkdirSync(path.join(target, "release"));
    fs.writeFileSync(app, "debug app");
    fs.writeFileSync(debugCli, "later standalone debug CLI");
    fs.writeFileSync(path.join(target, "release", "wardian-cli.exe"), "packaged release CLI");
    fs.writeFileSync(packaged, "packaged release CLI");
    const selected = resolveExistingCliPath({ repoRoot: root, env: {}, platform: "win32",
      spawnSyncImpl: () => ({ status: 0, stdout: JSON.stringify({ target_directory: target }), stderr: "" }),
    });
    assert.equal(selected, debugCli, "Standalone Cargo selection retains its debug preference");
    const frozen = freezeRunArtifacts({ home: path.join(root, "app-home"), appPath: app, cliPath: selected, platform: "win32" });
    assert.equal(frozen.cli.source, fs.realpathSync(packaged));
    assert.equal(fs.readFileSync(frozen.cli.path, "utf8"), "packaged release CLI");
    assert.equal(fs.readFileSync(path.join(frozen.dir, "resources", "bin", "wardian-cli.exe"), "utf8"), "packaged release CLI");
    fs.writeFileSync(packaged, "a subsequent app build");
    assert.equal(fs.readFileSync(frozen.cli.path, "utf8"), "packaged release CLI", "Owned bytes survive later staging");
    const standalone = freezeRunArtifacts({ home: path.join(root, "cli-home"), cliPath: selected });
    assert.equal(standalone.app, null);
    assert.equal(standalone.cli.source, debugCli);
    assert.equal(fs.readFileSync(standalone.cli.path, "utf8"), "later standalone debug CLI");
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test("default app-backed freeze rejects missing, nonregular and conflicting CLI packages before copying", () => {
  for (const condition of ["missing", "directory", "broken-link", "conflicting"]) {
    const root = scratch(`default-pair-${condition}`);
    try {
      const appDir = path.join(root, "app");
      const app = path.join(appDir, "Wardian.exe");
      const direct = path.join(appDir, "bin", "wardian-cli.exe");
      const nested = path.join(appDir, "resources", "bin", "wardian-cli.exe");
      const cargo = path.join(root, "wardian-cli.exe");
      const home = path.join(root, "home");
      fs.mkdirSync(appDir);
      fs.writeFileSync(app, "app");
      fs.writeFileSync(cargo, "available Cargo CLI is not a fallback");
      if (condition !== "missing") {
        fs.mkdirSync(path.dirname(direct), { recursive: true });
        fs.mkdirSync(path.dirname(nested), { recursive: true });
        fs.writeFileSync(nested, "packaged CLI");
        if (condition === "directory") fs.mkdirSync(direct);
        else if (condition === "broken-link") fs.symlinkSync(path.join(root, "missing-cli.exe"), direct, "file");
        else fs.writeFileSync(direct, "conflicting packaged CLI");
      }
      const expected = condition === "missing" || condition === "broken-link" ? "PAIRED_CLI_PACKAGED_MISSING"
        : condition === "directory" ? "PAIRED_CLI_PACKAGED_NOT_FILE" : "PAIRED_CLI_PACKAGED_AMBIGUOUS";
      assert.throws(() => freezeRunArtifacts({ home, appPath: app, cliPath: cargo, platform: "win32" }),
        (error) => error.code === expected);
      assert.equal(fs.existsSync(home), false, "Invalid input fails before the run's binaries or runtime are copied");
    } finally { fs.rmSync(root, { recursive: true, force: true }); }
  }
});

test("default pair validation rejects stale copied resources", () => {
  const root = scratch("default-pair-stale-copy");
  try {
    const app = path.join(root, "app", "Wardian.exe");
    const packaged = path.join(root, "app", "resources", "bin", "wardian-cli.exe");
    const home = path.join(root, "home");
    fs.mkdirSync(path.dirname(packaged), { recursive: true });
    fs.writeFileSync(app, "app");
    fs.writeFileSync(packaged, "original packaged CLI");
    freezeRunArtifacts({ home, appPath: app, platform: "win32" });
    fs.writeFileSync(packaged, "newly staged CLI");
    assert.throws(() => freezeRunArtifacts({ home, appPath: app, platform: "win32" }),
      (error) => error.code === "PAIRED_CLI_MISMATCH");
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test("a paired app-file symlink freezes canonical runtime rather than alias-directory files", () => {
  const root = scratch("paired-file-link");
  try {
    const appDir = path.join(root, "real app");
    const aliasDir = path.join(root, "alias");
    const app = path.join(appDir, "Wardian.exe");
    const alias = path.join(aliasDir, "Wardian.exe");
    const cli = path.join(root, "selected", "wardian-cli.exe");
    fs.mkdirSync(path.join(appDir, "resources", "bin"), { recursive: true });
    fs.mkdirSync(path.join(appDir, "conpty"));
    fs.mkdirSync(aliasDir);
    fs.mkdirSync(path.dirname(cli));
    fs.writeFileSync(app, "canonical app");
    fs.writeFileSync(cli, "paired cli");
    fs.writeFileSync(path.join(appDir, "resources", "bin", "wardian-cli.exe"), "paired cli");
    fs.writeFileSync(path.join(appDir, "own.dll"), "canonical library");
    fs.writeFileSync(path.join(appDir, "conpty", "own.dll"), "canonical ConPTY");
    fs.writeFileSync(path.join(aliasDir, "foreign.dll"), "unrelated alias library");
    fs.symlinkSync(app, alias, "file");
    const frozen = freezeRunArtifacts({ home: path.join(root, "home"), appPath: alias,
      cliPath: cli, pairedCli: true, platform: "win32" });
    assert.equal(frozen.app.source, fs.realpathSync(app));
    assert.equal(fs.readFileSync(path.join(frozen.dir, "own.dll"), "utf8"), "canonical library");
    assert.equal(fs.readFileSync(path.join(frozen.dir, "conpty", "own.dll"), "utf8"), "canonical ConPTY");
    assert.equal(fs.existsSync(path.join(frozen.dir, "foreign.dll")), false);
    assert.equal(fs.readFileSync(frozen.cli.path, "utf8"), "paired cli");
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test("explicit and default POSIX pairs retain the app loader's resource layout in the owned home", () => {
  for (const [platform, pairedCli] of [["linux", true], ["linux", false], ["darwin", true], ["darwin", false]]) {
    const root = scratch(`paired-${platform}-${pairedCli}`);
    try {
      const appDir = platform === "darwin" ? path.join(root, "Wardian.app", "Contents", "MacOS")
        : path.join(root, "usr", "bin");
      const resources = platform === "darwin" ? path.join(appDir, "..", "Resources")
        : path.join(appDir, "..", "lib", "Wardian");
      const cli = path.join(root, "selected", "wardian-cli");
      const app = path.join(appDir, "Wardian");
      const home = path.join(root, "home");
      fs.mkdirSync(appDir, { recursive: true });
      fs.mkdirSync(path.join(resources, "bin"), { recursive: true });
      fs.mkdirSync(path.dirname(cli), { recursive: true });
      fs.writeFileSync(app, "paired app");
      fs.writeFileSync(cli, "paired cli");
      fs.writeFileSync(path.join(resources, "bin", "wardian-cli"), "paired cli");
      const frozen = freezeRunArtifacts({ home, appPath: app, cliPath: cli, pairedCli, platform });
      const resourceDest = platform === "darwin" ? path.join(home, "Resources") : path.join(home, "lib", "Wardian");
      assert.equal(fs.readFileSync(path.join(resourceDest, "bin", "wardian-cli"), "utf8"), "paired cli");
      assert.equal(fs.readFileSync(frozen.cli.path, "utf8"), "paired cli");
    } finally { fs.rmSync(root, { recursive: true, force: true }); }
  }
});

test("a missing binary freezes to nothing rather than throwing", () => {
  const home = scratch("missing");
  try {
    assert.equal(freezeArtifact(null, path.join(home, FROZEN_BIN_DIR)), null);
    assert.equal(freezeArtifact(path.join(home, "absent.exe"), path.join(home, FROZEN_BIN_DIR)), null);
    const run = freezeRunArtifacts({ home, appPath: null, cliPath: null });
    assert.equal(run.app, null);
    assert.equal(run.cli, null);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

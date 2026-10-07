// @tier nightly — Deterministic native artifact and dependency preflight tests.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import {
  NativeArtifactResolutionError,
  assertNativeNodeDependencies,
  commandName,
  nativeNodeDependencyDiagnostics,
  resolveBuiltCliPath,
  resolveExistingCliPath,
  resolveExplicitNativeApp,
  resolveNativeAppArtifact,
  resolvePackageEntry,
  resolveCargoTargetDirectory,
} from "../lib/native-artifact-resolution.mjs";

function fixtureRoot(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `wardian-${label}-`));
}

function removeFixture(root) {
  fs.rmSync(root, { recursive: true, force: true });
}

function metadataResult(targetDirectory) {
  return {
    status: 0,
    stdout: JSON.stringify({ target_directory: targetDirectory }),
    stderr: "",
  };
}

function pairedFixture(root, platform = "win32") {
  const appDir = platform === "darwin"
    ? path.join(root, "Wardian.app", "Contents", "MacOS")
    : platform === "linux" ? path.join(root, "usr", "bin") : path.join(root, "app with spaces");
  const resources = platform === "darwin" ? path.join(appDir, "..", "Resources")
    : platform === "linux" ? path.join(appDir, "..", "lib", "Wardian") : appDir;
  const packagedCli = path.join(resources, "resources", "bin", commandName("wardian-cli", platform));
  const cli = path.join(root, "selected cli", commandName("wardian-cli", platform));
  const app = path.join(appDir, commandName("Wardian", platform));
  fs.mkdirSync(path.dirname(packagedCli), { recursive: true });
  fs.mkdirSync(path.dirname(cli), { recursive: true });
  fs.mkdirSync(appDir, { recursive: true });
  fs.writeFileSync(app, "paired app");
  fs.writeFileSync(packagedCli, "paired cli");
  fs.writeFileSync(cli, "paired cli");
  return { app, cli, packagedCli, env: {
    WARDIAN_NATIVE_APP: app, WARDIAN_NATIVE_CLI: cli, WARDIAN_NATIVE_SKIP_BUILD: "1",
  } };
}

test("Cargo target resolution normalizes relative, absolute, and configured targets", () => {
  const root = fixtureRoot("target-resolution");
  try {
    const relative = resolveCargoTargetDirectory({
      repoRoot: root,
      env: { CARGO_TARGET_DIR: "ignored-by-metadata-probe" },
      spawnSyncImpl: (_command, _args, options) => {
        assert.equal(options.cwd, path.resolve(root));
        assert.equal(options.env.CARGO_TARGET_DIR, "ignored-by-metadata-probe");
        return metadataResult("configured target with spaces");
      },
    });
    assert.equal(relative, path.join(root, "configured target with spaces"));

    const absoluteTarget = path.join(root, "absolute target with spaces");
    const absolute = resolveCargoTargetDirectory({
      repoRoot: root,
      spawnSyncImpl: () => metadataResult(absoluteTarget),
    });
    assert.equal(absolute, absoluteTarget);
  } finally {
    removeFixture(root);
  }
});

test("built CLI resolution uses Cargo metadata instead of a stale local candidate", () => {
  const root = fixtureRoot("stale-cli");
  const effectiveTarget = path.join(root, "configured target with spaces");
  const expected = path.join(effectiveTarget, "debug", commandName("wardian-cli", "win32"));
  try {
    const resolved = resolveBuiltCliPath({
      repoRoot: root,
      platform: "win32",
      spawnSyncImpl: () => metadataResult(effectiveTarget),
      existsSyncImpl: (candidate) => candidate === expected || candidate === path.join(root, "target", "debug", "wardian-cli.exe"),
    });
    assert.equal(resolved, expected);
  } finally {
    removeFixture(root);
  }
});

test("built CLI resolution fails with a classified missing-artifact error", () => {
  const root = fixtureRoot("missing-cli");
  try {
    assert.throws(
      () => resolveBuiltCliPath({
        repoRoot: root,
        spawnSyncImpl: () => metadataResult("target"),
        existsSyncImpl: () => false,
      }),
      (error) => error instanceof NativeArtifactResolutionError && error.code === "CLI_ARTIFACT_MISSING",
    );
  } finally {
    removeFixture(root);
  }
});

test("existing CLI resolution stays nullable and never falls back to repo-local target", () => {
  const root = fixtureRoot("existing-cli");
  const effectiveTarget = path.join(root, "configured target");
  const expected = path.join(effectiveTarget, "debug", commandName("wardian-cli", "win32"));
  try {
    assert.equal(
      resolveExistingCliPath({
        repoRoot: root,
        platform: "win32",
        spawnSyncImpl: () => metadataResult(effectiveTarget),
        existsSyncImpl: (candidate) => candidate === expected,
      }),
      expected,
    );
    assert.equal(
      resolveExistingCliPath({
        repoRoot: root,
        platform: "win32",
        spawnSyncImpl: () => metadataResult(effectiveTarget),
        existsSyncImpl: (candidate) => candidate === path.join(root, "target", "debug", "wardian-cli.exe"),
      }),
      null,
    );
  } finally {
    removeFixture(root);
  }
});

test("explicit paired CLI bypasses Cargo metadata and a stale debug CLI", () => {
  const root = fixtureRoot("paired-cli");
  try {
    const pair = pairedFixture(root);
    const staleTarget = path.join(root, "stale target");
    const staleCli = path.join(staleTarget, "debug", "wardian-cli.exe");
    fs.mkdirSync(path.dirname(staleCli), { recursive: true });
    fs.writeFileSync(staleCli, "stale debug cli");
    fs.mkdirSync(path.join(staleTarget, "release"));
    fs.writeFileSync(path.join(staleTarget, "release", "wardian-cli.exe"), "paired cli");
    let metadataCalls = 0;
    const selected = resolveExistingCliPath({ repoRoot: root, platform: "win32",
      env: { ...pair.env, WARDIAN_NATIVE_CLI: path.relative(root, pair.cli) },
      spawnSyncImpl: () => { metadataCalls++; return metadataResult(staleTarget); },
    });
    assert.equal(selected, fs.realpathSync(pair.cli));
    assert.equal(metadataCalls, 0);
    assert.equal(resolveExistingCliPath({ repoRoot: root, platform: "win32", env: {},
      spawnSyncImpl: () => metadataResult(staleTarget) }), staleCli);
  } finally { removeFixture(root); }
});

test("explicit CLI validates the paired app's supported POSIX resource layouts", () => {
  for (const platform of ["linux", "darwin"]) {
    const root = fixtureRoot(`paired-cli-${platform}`);
    try {
      const pair = pairedFixture(root, platform);
      assert.equal(resolveExistingCliPath({ repoRoot: root, platform, env: pair.env,
        spawnSyncImpl: () => { throw new Error("Explicit pairing must not probe Cargo"); },
      }), fs.realpathSync(pair.cli));
    } finally { removeFixture(root); }
  }
});

test("invalid explicit CLI inputs fail before metadata", () => {
  const cases = [
    ["empty", (pair) => ({ ...pair.env, WARDIAN_NATIVE_CLI: " " }), "EXPLICIT_CLI_EMPTY"],
    ["missing", (pair) => ({ ...pair.env, WARDIAN_NATIVE_CLI: `${pair.cli}.missing` }), "EXPLICIT_CLI_MISSING"],
    ["directory", (pair) => ({ ...pair.env, WARDIAN_NATIVE_CLI: path.dirname(pair.cli) }), "EXPLICIT_CLI_NOT_FILE"],
    ["app required", (pair) => ({ ...pair.env, WARDIAN_NATIVE_APP: undefined }), "EXPLICIT_APP_EMPTY"],
    ["app key required", (pair) => { const env = { ...pair.env }; delete env.WARDIAN_NATIVE_APP; return env; }, "EXPLICIT_CLI_APP_REQUIRED"],
    ["basename", (pair) => {
      const renamed = path.join(path.dirname(pair.cli), "other.exe");
      fs.copyFileSync(pair.cli, renamed);
      return { ...pair.env, WARDIAN_NATIVE_CLI: renamed };
    }, "EXPLICIT_CLI_NAME"],
    ["skip build required", (pair) => ({ ...pair.env, WARDIAN_NATIVE_SKIP_BUILD: "0" }), "EXPLICIT_CLI_SKIP_BUILD_REQUIRED"],
    ["mismatch", (pair) => { fs.writeFileSync(pair.cli, "foreign cli"); return pair.env; }, "PAIRED_CLI_MISMATCH"],
    ["packaged missing", (pair) => { fs.unlinkSync(pair.packagedCli); return pair.env; }, "PAIRED_CLI_PACKAGED_MISSING"],
  ];
  for (const [name, configure, code] of cases) {
    const root = fixtureRoot("invalid-paired-cli");
    try {
      const pair = pairedFixture(root);
      assert.throws(() => resolveExistingCliPath({ repoRoot: root, platform: "win32", env: configure(pair),
        spawnSyncImpl: () => { throw new Error("Invalid pairing must not probe Cargo"); },
      }), (error) => error instanceof NativeArtifactResolutionError && error.code === code, name);
    } finally { removeFixture(root); }
  }
});

test("explicit paired CLI accepts Windows local-drive verbatim paths", { skip: process.platform !== "win32" }, () => {
  const root = fixtureRoot("verbatim-paired-cli");
  try {
    const pair = pairedFixture(root);
    assert.equal(resolveExistingCliPath({ repoRoot: root, platform: "win32", env: {
      ...pair.env, WARDIAN_NATIVE_APP: path.toNamespacedPath(pair.app), WARDIAN_NATIVE_CLI: path.toNamespacedPath(pair.cli),
    }, spawnSyncImpl: () => { throw new Error("Explicit pairing must not probe Cargo"); } }), fs.realpathSync(pair.cli));
  } finally { removeFixture(root); }
});

test("explicit app paths are deterministic for relative and absolute values", () => {
  const root = fixtureRoot("explicit-app");
  const appPath = path.join(root, "artifact directory with spaces", "Wardian-test.exe");
  fs.mkdirSync(path.dirname(appPath), { recursive: true });
  fs.writeFileSync(appPath, "fixture", "utf8");
  try {
    const relativeValue = path.relative(root, appPath);
    assert.equal(
      resolveExplicitNativeApp({
        repoRoot: root,
        env: { WARDIAN_NATIVE_APP: relativeValue },
      }),
      appPath,
    );
    assert.equal(
      resolveExplicitNativeApp({
        repoRoot: root,
        env: { WARDIAN_NATIVE_APP: appPath },
      }),
      appPath,
    );
    assert.equal(resolveExplicitNativeApp({ repoRoot: root, env: {} }), null);
  } finally {
    removeFixture(root);
  }
});

test("invalid explicit app paths fail closed before provider setup", () => {
  const root = fixtureRoot("invalid-app");
  const directory = path.join(root, "not-an-app");
  fs.mkdirSync(directory);
  try {
    assert.throws(
      () => resolveExplicitNativeApp({ repoRoot: root, env: { WARDIAN_NATIVE_APP: "missing.exe" } }),
      (error) => error.code === "EXPLICIT_APP_MISSING",
    );
    assert.throws(
      () => resolveExplicitNativeApp({ repoRoot: root, env: { WARDIAN_NATIVE_APP: "not-an-app" } }),
      (error) => error.code === "EXPLICIT_APP_NOT_FILE",
    );
    assert.throws(
      () => resolveExplicitNativeApp({ repoRoot: root, env: { WARDIAN_NATIVE_APP: " " } }),
      (error) => error.code === "EXPLICIT_APP_EMPTY",
    );
  } finally {
    removeFixture(root);
  }
});

test("automatic app resolution is bound to Cargo's effective target", () => {
  const root = fixtureRoot("auto-app");
  const target = path.join(root, "target with spaces");
  const appPath = path.join(target, "debug", "Wardian.exe");
  try {
    const resolved = resolveNativeAppArtifact({
      repoRoot: root,
      platform: "win32",
      env: {},
      spawnSyncImpl: () => metadataResult(target),
      statSyncImpl: (candidate) => {
        if (candidate === appPath) return { isFile: () => true };
        throw new Error("missing fixture path");
      },
    });
    assert.deepEqual(resolved, { path: appPath, profile: "debug", source: "cargo-target" });
  } finally {
    removeFixture(root);
  }
});

test("missing native Node packages produce actionable preflight diagnostics", () => {
  const packagePaths = {
    "@tauri-apps/cli/tauri.js": "C:/fixture/node_modules/@tauri-apps/cli/tauri.js",
    "selenium-webdriver": "C:/fixture/node_modules/selenium-webdriver/index.js",
  };
  const diagnostics = nativeNodeDependencyDiagnostics({
    repoRoot: "C:/fixture",
    requireResolve: (packageName) => {
      if (packagePaths[packageName]) return packagePaths[packageName];
      throw new Error("not installed");
    },
  });
  assert.equal(diagnostics.ok, true);
  assert.deepEqual(diagnostics.resolved, packagePaths);
  assert.deepEqual(nativeNodeDependencyDiagnostics({
    repoRoot: "C:/fixture",
    requireResolve: (packageName) => {
      if (packageName === "selenium-webdriver") return packagePaths[packageName];
      throw new Error("not installed");
    },
  }).missing.map(({ packageName, code }) => ({ packageName, code })), [
    { packageName: "@tauri-apps/cli/tauri.js", code: "NODE_PACKAGE_MISSING" },
  ]);
  assert.throws(
    () => assertNativeNodeDependencies({
      repoRoot: "C:/fixture",
      requireResolve: () => { throw new Error("not installed"); },
    }),
    (error) => error.code === "NODE_DEPENDENCIES_MISSING" && /@tauri-apps\/cli/.test(error.message),
  );
  assert.equal(
    resolvePackageEntry({
      repoRoot: "C:/fixture",
      packageName: "selenium-webdriver",
      requireResolve: (packageName) => packagePaths[packageName],
    }),
    packagePaths["selenium-webdriver"],
  );
});

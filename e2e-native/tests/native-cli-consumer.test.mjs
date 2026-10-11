// @tier ci — Pure file/consumer contracts; no compiler, app or provider starts.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import * as nativeHarness from "../lib/harness.mjs";
import { commandName } from "../lib/native-artifact-resolution.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

/** Run the actual setup declaration without evaluating the file's native tests. */
function consumers() {
  return fs.readdirSync(testDir).filter((name) => name.endsWith(".test.mjs"))
    .flatMap((name) => {
      const source = fs.readFileSync(path.join(testDir, name), "utf8");
      const module = ts.createSourceFile(name, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
      const entry = module.statements.find((node) => ts.isFunctionDeclaration(node)
        && node.name?.text === "buildCli");
      if (!entry) return [];
      return [{ name, body: entry.getText(module), module }];
    });
}

function frozenPair(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-native-cli-consumer-"));
  t.after(() => {
    assert.equal(fs.realpathSync(path.dirname(root)), fs.realpathSync(os.tmpdir()));
    assert.ok(path.basename(root).startsWith("wardian-native-cli-consumer-"));
    fs.rmSync(root, { recursive: true, force: true });
  });
  const appDir = process.platform === "darwin" ? path.join(root, "Wardian.app/Contents/MacOS")
    : process.platform === "linux" ? path.join(root, "usr/bin") : root;
  const resources = process.platform === "darwin" ? path.join(root, "Wardian.app/Contents/Resources")
    : process.platform === "linux" ? path.join(root, "usr/lib/Wardian") : root;
  fs.mkdirSync(appDir, { recursive: true });
  fs.mkdirSync(path.join(resources, "resources/bin"), { recursive: true });
  const appPath = path.join(appDir, commandName("Wardian", process.platform));
  const cliPath = path.join(appDir, commandName("wardian-cli", process.platform));
  const packaged = path.join(resources, "resources/bin", commandName("wardian-cli", process.platform));
  fs.writeFileSync(appPath, "app fixture; never executed");
  fs.writeFileSync(cliPath, "matching CLI fixture; never executed");
  fs.copyFileSync(cliPath, packaged);
  return { appPath, cliPath, packaged, repoRoot: root, isolatedHome: root, platform: process.platform };
}

function invoke(entry, harness, skipNativeBuild, allowBuild = false) {
  const calls = [];
  const processCall = (program) => {
    calls.push(program);
    assert.ok(allowBuild, `${entry.name} unexpectedly entered ${program} for a frozen CLI`);
    return { status: 0, stdout: "", stderr: "" };
  };
  const context = {
    harness, skipNativeBuild, assert,
    prebuiltCliForRun: nativeHarness.prebuiltCliForRun,
    spawnSync: processCall, runProcess: processCall,
    freezeBuiltCliForRun: () => {
      calls.push("freeze");
      assert.ok(allowBuild, `${entry.name} unexpectedly replaced its frozen CLI`);
      return "newly-built-cli";
    },
  };
  const result = vm.runInNewContext(`${entry.body}\nbuildCli(harness);`, context);
  return { result, calls };
}

for (const entry of consumers()) {
  for (const mode of [{ pairedCli: true, skip: false }, { pairedCli: false, skip: true },
    { pairedCli: true, skip: true }]) {
    test(`${entry.name} consumes its frozen CLI with paired=${mode.pairedCli}, skip=${mode.skip}`, (t) => {
      const harness = { ...frozenPair(t), pairedCli: mode.pairedCli };
      const before = fs.readFileSync(harness.cliPath);
      const observed = invoke(entry, harness, mode.skip);
      assert.equal(observed.result, fs.realpathSync(harness.cliPath));
      assert.deepEqual(observed.calls, []);
      assert.deepEqual(fs.readFileSync(harness.cliPath), before);
    });
  }

  test(`${entry.name} rejects a missing frozen CLI before compiler entry`, (t) => {
    const harness = { ...frozenPair(t), pairedCli: true, cliPath: null };
    assert.throws(() => invoke(entry, harness, true), (error) =>
      error.message === "A prebuilt run requires the harness's frozen CLI.");
  });

  test(`${entry.name} rejects mismatched packaged bytes before compiler entry`, (t) => {
    const harness = { ...frozenPair(t), pairedCli: true };
    fs.writeFileSync(harness.packaged, "different staged CLI");
    assert.throws(() => invoke(entry, harness, true), (error) => error.code === "PAIRED_CLI_MISMATCH");
  });

  test(`${entry.name} retains the ordinary build-and-freeze route`, (t) => {
    const harness = { ...frozenPair(t), pairedCli: false };
    const observed = invoke(entry, harness, false, true);
    assert.equal(observed.result, "newly-built-cli");
    assert.deepEqual(observed.calls, ["cargo", "freeze"]);
  });
}

test("every maintained CLI setup resolves the prebuilt gate from its harness import", () => {
  const entries = consumers();
  assert.equal(entries.length, 10, "inspect the native consumer inventory when adding a CLI setup");
  for (const entry of entries) {
    const bound = entry.module.statements.some((node) => ts.isImportDeclaration(node)
      && node.moduleSpecifier.text === "../lib/harness.mjs"
      && node.importClause?.namedBindings && ts.isNamedImports(node.importClause.namedBindings)
      && node.importClause.namedBindings.elements.some((name) => name.name.text === "prebuiltCliForRun"));
    assert.ok(bound, `${entry.name} must bind the maintained prebuilt gate`);
  }
});

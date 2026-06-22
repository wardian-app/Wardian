import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";

import { resolveCliSourcePath } from "../../scripts/stage-cli.mjs";

test("stage-cli resolves source from cargo metadata target directory", () => {
  const root = "D:/repo/Wardian";
  const targetDirectory = "D:/shared/cargo-target";

  assert.equal(
    resolveCliSourcePath({ root, targetDirectory, target: "", profile: "release", exe: "wardian-cli.exe" }),
    path.join(targetDirectory, "release", "wardian-cli.exe"),
  );
});

test("stage-cli resolves explicit target triples under cargo metadata target directory", () => {
  const root = "D:/repo/Wardian";
  const targetDirectory = "D:/shared/cargo-target";

  assert.equal(
    resolveCliSourcePath({
      root,
      targetDirectory,
      target: "x86_64-pc-windows-msvc",
      profile: "release",
      exe: "wardian-cli.exe",
    }),
    path.join(targetDirectory, "x86_64-pc-windows-msvc", "release", "wardian-cli.exe"),
  );
});

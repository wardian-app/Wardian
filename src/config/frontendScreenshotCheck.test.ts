import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { describe, expect, test } from "vitest";

function git(cwd: string, args: string[]) {
  execFileSync("git", args, { cwd, stdio: "pipe" });
}

describe("frontend screenshot check", () => {
  test("does not require screenshot evidence for dependency manifest-only changes", () => {
    const repo = mkdtempSync(join(tmpdir(), "wardian-screenshot-check-"));

    git(repo, ["init", "-b", "main"]);
    git(repo, ["config", "user.email", "test@example.com"]);
    git(repo, ["config", "user.name", "Test User"]);

    writeFileSync(join(repo, "package.json"), "{}\n");
    writeFileSync(join(repo, "package-lock.json"), "{}\n");
    git(repo, ["add", "."]);
    git(repo, ["commit", "-m", "initial"]);
    git(repo, ["branch", "base"]);

    writeFileSync(join(repo, "package.json"), '{ "dependencies": {} }\n');
    writeFileSync(join(repo, "package-lock.json"), '{ "lockfileVersion": 3 }\n');
    git(repo, ["add", "."]);
    git(repo, ["commit", "-m", "update dependencies"]);

    const script = resolve("scripts/verify-frontend-screenshot.mjs");
    const result = spawnSync(process.execPath, [script, "base", "HEAD"], {
      cwd: repo,
      encoding: "utf8",
      env: { ...process.env, PR_BODY: "" },
    });

    expect(result.status).toBe(0);
    expect(result.stdout).toContain("No frontend changes detected");
  });
});

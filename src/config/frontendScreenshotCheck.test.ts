import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
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

  test("does not require screenshot evidence for test-only source changes", () => {
    const repo = mkdtempSync(join(tmpdir(), "wardian-screenshot-check-"));

    git(repo, ["init", "-b", "main"]);
    git(repo, ["config", "user.email", "test@example.com"]);
    git(repo, ["config", "user.name", "Test User"]);

    writeFileSync(join(repo, "README.md"), "# Test repo\n");
    git(repo, ["add", "."]);
    git(repo, ["commit", "-m", "initial"]);
    git(repo, ["branch", "base"]);

    const testPath = join(repo, "src", "config");
    mkdirSync(testPath, { recursive: true });
    writeFileSync(join(testPath, "frontendScreenshotCheck.test.ts"), "test('example', () => {});\n");
    git(repo, ["add", "."]);
    git(repo, ["commit", "-m", "add test"]);

    const script = resolve("scripts/verify-frontend-screenshot.mjs");
    const result = spawnSync(process.execPath, [script, "base", "HEAD"], {
      cwd: repo,
      encoding: "utf8",
      env: { ...process.env, PR_BODY: "" },
    });

    expect(result.status).toBe(0);
    expect(result.stdout).toContain("No frontend changes detected");
  });

  test("does not require screenshot evidence for type and terminal input utility changes", () => {
    const repo = mkdtempSync(join(tmpdir(), "wardian-screenshot-check-"));

    git(repo, ["init", "-b", "main"]);
    git(repo, ["config", "user.email", "test@example.com"]);
    git(repo, ["config", "user.name", "Test User"]);

    writeFileSync(join(repo, "README.md"), "# Test repo\n");
    git(repo, ["add", "."]);
    git(repo, ["commit", "-m", "initial"]);
    git(repo, ["branch", "base"]);

    const typesPath = join(repo, "src", "types");
    const utilsPath = join(repo, "src", "utils");
    mkdirSync(typesPath, { recursive: true });
    mkdirSync(utilsPath, { recursive: true });
    writeFileSync(join(typesPath, "index.ts"), "export interface DeliveryDetail {}\n");
    writeFileSync(join(utilsPath, "terminalInput.ts"), "export function submit() {}\n");
    git(repo, ["add", "."]);
    git(repo, ["commit", "-m", "add non-visual frontend contracts"]);

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

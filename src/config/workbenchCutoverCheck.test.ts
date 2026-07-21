import { spawnSync } from "node:child_process";
import path from "node:path";
import { describe, expect, it } from "vitest";

const repoRoot = process.cwd();
const scriptPath = path.join(repoRoot, "scripts", "verify-workbench-cutover.mjs");

function runVerifier(args: string[]) {
  return spawnSync(process.execPath, [scriptPath, ...args], {
    cwd: repoRoot,
    encoding: "utf8",
  });
}

describe("workbench cutover verifier", () => {
  it("freezes all 25 audited paths with final dispositions and documented exceptions", () => {
    const result = runVerifier(["--describe-audit", "--json"]);

    expect(result.status, result.stderr).toBe(0);
    const description = JSON.parse(result.stdout) as {
      baseline_revision: string;
      expected_entries: number;
      counts: Record<string, number>;
      entries: Array<{ path: string; disposition: string; reason: string }>;
      allowlist: Array<{ path: string; rule_ids: string[]; reason: string }>;
    };
    expect(description.baseline_revision).toBe("d53842dc");
    expect(description.expected_entries).toBe(25);
    expect(description.entries).toHaveLength(25);
    expect(new Set(description.entries.map((entry) => entry.path)).size).toBe(25);
    expect(description.counts).toEqual({
      migrated: 22,
      removed: 1,
      "intentionally-unrelated": 2,
    });
    expect(description.entries.every((entry) => entry.reason.trim().length > 0)).toBe(true);
    expect(description.entries.filter((entry) => entry.disposition === "intentionally-unrelated")
      .map((entry) => entry.path)).toEqual([
      "src/features/remote/RemoteMobileApp.test.tsx",
      "src/features/settings/SettingsModal.test.tsx",
    ]);
    expect(description.allowlist.length).toBeGreaterThanOrEqual(2);
    expect(description.allowlist.every(
      (entry) => entry.reason.trim().length > 0 && entry.rule_ids.length > 0,
    )).toBe(true);
  });

  it("self-tests every forbidden rule and accepts semantic workbench navigation", () => {
    const result = runVerifier(["--self-test"]);

    expect(result.status, result.stderr).toBe(0);
    expect(JSON.parse(result.stdout)).toEqual(expect.objectContaining({
      self_test: "passed",
      audit_entries: 25,
      rules: [
        "legacy-titlebar-selector",
        "legacy-role-button-selector",
        "legacy-xpath-button-selector",
        "legacy-desktop-navigation-symbol",
        "direct-desktop-surface-launch-click",
      ],
      errors: [],
    }));
  });

  it("fails closed for an invalid scan root", () => {
    const result = runVerifier(["--root", "relative-workbench-root"]);

    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("--root requires an absolute path");
  });

  it("proves the tracked tree has completed every frozen audit disposition", () => {
    const result = runVerifier(["--json"]);
    const report = JSON.parse(result.stdout) as {
      passed: boolean;
      roots: string[];
      audit: { expected_entries: number; verified_entries: number };
      allowlisted_matches: Array<{ path: string; reason: string }>;
      violations: Array<{ path: string; rule_id: string }>;
    };

    expect(result.status, result.stderr || JSON.stringify(report.violations, null, 2)).toBe(0);
    expect(report.passed).toBe(true);
    expect(report.roots).toEqual(["src", "e2e", "e2e-native", "scripts"]);
    expect(report.audit).toMatchObject({ expected_entries: 25, verified_entries: 25 });
    expect(report.allowlisted_matches.length).toBeGreaterThan(0);
    expect(report.allowlisted_matches.every((entry) => entry.reason.trim().length > 0)).toBe(true);
    expect(report.violations).toEqual([]);
  });
});

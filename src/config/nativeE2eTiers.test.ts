import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";

/**
 * Tier declarations are the contract between a native test and the jobs that
 * run it. CI selects `ci`, the nightly automation selects `nightly`, and nothing
 * runs `manual`.
 *
 * `ciAutomation.test.ts` used to assert the file names the automation listed
 * by hand. That assertion lives here now, against the declarations themselves,
 * so a test that changes tier is caught by the same check that proves the tier
 * exists at all.
 */

const TEST_DIR = path.join("e2e-native", "tests");
const TIERS = ["ci", "nightly", "manual"] as const;

function nativeTests(): Array<{ file: string; tier: string | null }> {
  return readdirSync(TEST_DIR)
    .filter((entry) => entry.endsWith(".test.mjs"))
    .sort()
    .map((entry) => {
      const source = readFileSync(path.join(TEST_DIR, entry), "utf8");
      const match = /^\s*\/\/\s*@tier\s+([a-z]+)/m.exec(source);
      return { file: entry, tier: match ? match[1] : null };
    });
}

describe("native E2E tiers", () => {
  it("declares a known tier on every native test", () => {
    const undeclared = nativeTests().filter(
      (test) => test.tier === null || !TIERS.includes(test.tier as (typeof TIERS)[number]),
    );
    expect(undeclared.map((test) => test.file)).toEqual([]);
  });

  it("keeps the per-PR tier to the suite CI can afford", () => {
    const ci = nativeTests().filter((test) => test.tier === "ci").map((test) => test.file);
    expect(ci).toEqual([
      "canonical-messaging.test.mjs",
      "remote-gateway-native.test.mjs",
      "terminal-presentation-broker-native.test.mjs",
      "workbench-persistence-native.test.mjs",
      "workbench-runtime-lifecycle-native.test.mjs",
    ]);
  });

  it("reserves the manual tier for tests needing a real provider", () => {
    // A `manual` test runs in no job at all, so the reason has to be visible
    // in the file: it reads a WARDIAN_E2E_REAL_* switch or drives a real
    // provider binary.
    for (const test of nativeTests().filter((entry) => entry.tier === "manual")) {
      const source = readFileSync(path.join(TEST_DIR, test.file), "utf8");
      expect(source, `${test.file} is @tier manual but names no real-provider gate`)
        .toMatch(/WARDIAN_E2E_REAL|real[-_]provider|REAL_OPENCODE/i);
    }
  });

  it("leaves no tier empty of the tests its job expects", () => {
    const byTier = new Map<string, number>(TIERS.map((tier) => [tier, 0]));
    for (const test of nativeTests()) {
      if (test.tier && byTier.has(test.tier)) byTier.set(test.tier, byTier.get(test.tier)! + 1);
    }
    // `ci` and `nightly` both have an automation pointed at them; an empty tier
    // would make that job silently pass with nothing run.
    expect(byTier.get("ci")).toBeGreaterThan(0);
    expect(byTier.get("nightly")).toBeGreaterThan(0);
  });
});

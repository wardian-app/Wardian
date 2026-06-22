import { readdirSync } from "node:fs";
import path from "node:path";

export function nativeE2eTestTargets() {
  const testDir = path.join("e2e-native", "tests");
  return readdirSync(testDir)
    .filter((entry) => entry.endsWith(".test.mjs"))
    .sort()
    .map((entry) => path.join(testDir, entry));
}

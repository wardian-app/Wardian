/**
 * Mock agent fixture for browser E2E tests.
 *
 * CURRENT LIMITATION: The spawn form only exposes Claude/Codex/Gemini/Antigravity/OpenCode
 * as provider options. The mock provider is not selectable via UI, so tests
 * that require an actively-running mock agent are @native-only.
 *
 * To unlock browser E2E agent-state tests, two things are needed:
 *   1. Add "mock" to the provider dropdown in SpawnAgentPanel.tsx (dev/test builds only).
 *   2. Expose WARDIAN_MOCK_SCENARIO via the spawn config so it flows through
 *      to the provider process environment.
 *
 * Until then, use `npm run test:e2e:native` with:
 *   WARDIAN_MOCK_SCENARIO=basic npm run test:e2e:native
 *
 * What this fixture DOES provide today:
 *   - seededHome(): returns a temp WARDIAN_HOME path pre-seeded with projects.json
 *     (useful for tests that read state but don't need a live agent)
 */

import * as os from "os";
import * as path from "path";
import * as fs from "fs";

export interface SeededHome {
  wardianHome: string;
  cleanup: () => void;
}

/**
 * Creates an isolated WARDIAN_HOME directory seeded with a mock project config.
 * The spawned Tauri process must be pointed at this home via WARDIAN_HOME env.
 *
 * Note: browser E2E tests start the app via webServer which reads WARDIAN_HOME
 * from the playwright.config.ts env block — this fixture is most useful for
 * native E2E tests that control the process environment directly.
 */
export function seededHome(scenario: string = "basic"): SeededHome {
  const wardianHome = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-e2e-"));

  const agentsDir = path.join(wardianHome, "agents");
  fs.mkdirSync(agentsDir, { recursive: true });

  const projectsJson = {
    projects: [
      {
        name: "e2e-test-project",
        agents: [
          {
            name: "mock-agent",
            provider: "mock",
            workspace: agentsDir,
            mock_scenario: scenario,
          },
        ],
      },
    ],
  };

  fs.writeFileSync(
    path.join(wardianHome, "projects.json"),
    JSON.stringify(projectsJson, null, 2)
  );

  return {
    wardianHome,
    cleanup: () => fs.rmSync(wardianHome, { recursive: true, force: true }),
  };
}

/**
 * Seeds a topology.json file in the given WARDIAN_HOME directory.
 * Ensures canonical pair ordering (lexicographic) as required by the Rust model.
 *
 * Like seededHome, this is most useful for native E2E tests where the backend
 * actually reads WARDIAN_HOME from disk. Browser E2E specs mock the Tauri IPC
 * layer instead, so they exercise this helper only via its on-disk output
 * (see the serialization test in e2e/tests/graph-topology.spec.ts).
 *
 * @param home The WARDIAN_HOME directory path
 * @param edges Array of [agentId1, agentId2] pairs to create manual edges
 * @param ignoredPairs Array of [agentId1, agentId2] pairs to ignore (optional)
 */
export function seedTopology(
  home: string,
  edges: Array<[string, string]>,
  ignoredPairs: Array<[string, string]> = [],
) {
  const canonicalizeEdges = (pairs: Array<[string, string]>) =>
    pairs.map(([a, b]) => {
      const [x, y] = a < b ? [a, b] : [b, a];
      return { a: x, b: y };
    });

  const topology = {
    version: 1,
    edges: canonicalizeEdges(edges).map((pair) => ({
      ...pair,
      created_at: new Date().toISOString(),
    })),
    ignored_pairs: canonicalizeEdges(ignoredPairs),
  };

  fs.writeFileSync(
    path.join(home, "topology.json"),
    JSON.stringify(topology, null, 2)
  );
}

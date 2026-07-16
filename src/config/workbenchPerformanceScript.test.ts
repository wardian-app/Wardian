import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { createCoreWorkbenchSurfaceRegistry } from "../features/workbench/coreSurfaceRegistry";
import { validateWorkbenchDocument } from "../features/workbench/workbenchModel";
import type { WorkbenchDocumentV1 } from "../types";

const repoRoot = process.cwd();
const scriptPath = path.join(repoRoot, "scripts", "measure-workbench-performance.mjs");
const fixturePath = path.join(repoRoot, "scripts", "fixtures", "workbench-performance-v1.json");
const refusal = "Refusing to benchmark without an explicit isolated WARDIAN_HOME.";

function runScript(args: string[], wardianHome?: string) {
  const env = { ...process.env };
  if (wardianHome === undefined) delete env.WARDIAN_HOME;
  else env.WARDIAN_HOME = wardianHome;
  return spawnSync(process.execPath, [scriptPath, ...args], {
    cwd: repoRoot,
    env,
    encoding: "utf8",
  });
}

describe("workbench performance script", () => {
  it.each([
    ["unset", undefined],
    ["empty", ""],
    ["relative", "wardian-workbench-performance-relative"],
    ["production default", path.join(os.homedir(), ".wardian")],
  ])("fails closed for %s WARDIAN_HOME", (_label, wardianHome) => {
    const result = runScript(["--validate-home-only"], wardianHome);

    expect(result.status).not.toBe(0);
    expect(result.stderr.trim()).toBe(refusal);
  });

  it("accepts an explicit absolute workspace-local performance home", () => {
    const isolatedHome = path.join(
      repoRoot,
      ".tmp",
      "workbench-performance",
      `vitest-${process.pid}`,
    );
    const result = runScript(["--validate-home-only"], isolatedHome);

    expect(result.status).toBe(0);
    expect(path.normalize(result.stdout.trim())).toBe(path.normalize(isolatedHome));
  });

  it("ships the deterministic production fixture with exact populations", () => {
    const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8")) as {
      scenario: string;
      benchmark: { heavy_surface_hidden_grace_ms: number };
      workbench: {
        groups: Record<string, { surface_ids: string[] }>;
        surfaces: Record<string, { surface_type: string }>;
      };
      agents: Array<{ session_id: string }>;
      terminal_presentations: Array<{ mode: "owner" | "mirror"; session_id: string }>;
    };
    const surfaces = Object.values(fixture.workbench.surfaces);
    const referencedSurfaceIds = Object.values(fixture.workbench.groups)
      .flatMap((group) => group.surface_ids);

    expect(fixture.scenario).toBe("production-workbench-performance-v1");
    expect(fixture.benchmark.heavy_surface_hidden_grace_ms).toBe(250);
    expect(Object.keys(fixture.workbench.groups)).toHaveLength(4);
    expect(surfaces).toHaveLength(20);
    expect(new Set(referencedSurfaceIds).size).toBe(20);
    expect(fixture.agents).toHaveLength(20);
    expect(new Set(fixture.agents.map((agent) => agent.session_id)).size).toBe(20);
    expect([...new Set(surfaces.map((surface) => surface.surface_type))]).toEqual(
      expect.arrayContaining(["agents-overview", "graph", "garden", "queue", "library", "workflows"]),
    );
    expect(fixture.terminal_presentations.filter((entry) => entry.mode === "owner")).toHaveLength(1);
    expect(fixture.terminal_presentations.filter((entry) => entry.mode === "mirror")).toHaveLength(3);
    expect(new Set(fixture.terminal_presentations.map((entry) => entry.session_id)).size).toBe(1);

    const singletonTypes = [
      "agents-overview", "dashboard", "queue", "graph", "garden", "library", "workflows",
    ];
    for (const type of singletonTypes) {
      expect(surfaces.filter((surface) => surface.surface_type === type)).toHaveLength(1);
    }
  });

  it("passes the canonical document validator and every registered surface state contract", () => {
    const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8")) as {
      workbench: WorkbenchDocumentV1;
    };
    expect(validateWorkbenchDocument(fixture.workbench)).toMatchObject({ valid: true });

    const registry = createCoreWorkbenchSurfaceRegistry();
    for (const surface of Object.values(fixture.workbench.surfaces)) {
      const resolved = registry.resolve_surface(surface);
      expect(resolved.missing_surface_type).toBeUndefined();
      expect(resolved.restore_result, surface.surface_id).toMatchObject({ ok: true });
    }
  });

  it("self-tests every exact gate and rejects absent observations", () => {
    const result = runScript(["--self-test"]);

    expect(result.status).toBe(0);
    const output = JSON.parse(result.stdout) as {
      self_test: string;
      gates: Record<string, { limit: number; unit: string }>;
    };
    expect(output.self_test).toBe("passed");
    expect(output.gates).toEqual({
      restore_p95_ms: { limit: 1500, unit: "ms" },
      tab_switch_p95_ms: { limit: 100, unit: "ms" },
      group_focus_p95_ms: { limit: 75, unit: "ms" },
      terminal_output_commit_p95_ms: { limit: 50, unit: "ms" },
      stream_gap_count: { limit: 0, unit: "gaps" },
      overview_settle_p95_ms: { limit: 300, unit: "ms" },
      heavy_surface_resume_p95_ms: { limit: 500, unit: "ms" },
      react_commit_max_ms: { limit: 50, unit: "ms" },
      bundle_delta_gzip_bytes: { limit: 250 * 1024, unit: "bytes" },
      xterm_renderer_peak: { limit: 24, unit: "renderers" },
      webgl_context_peak: { limit: 12, unit: "contexts" },
    });
  });

  it("uses a built production preview and in-page lifecycle instrumentation", () => {
    const source = fs.readFileSync(scriptPath, "utf8");

    expect(source).toContain("await buildProductionRuntime(home, fixture)");
    expect(source).toContain("const server = await preview(");
    expect(source).not.toContain("createServer");
    expect(source).toContain("runtime.terminal_burst_started_at = performance.now()");
    expect(source).toContain("waitForHeavyRendererReleased");
    expect(source).toContain("runtime.webgl_live = liveWebglContexts.size");
  });
});

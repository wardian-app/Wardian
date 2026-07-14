import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import type {
  ClosedSurfaceV1,
  WorkbenchDocumentV1,
  WorkbenchNodeV1,
  WorkbenchSurfaceV1,
} from "../../types";
import {
  applyWorkbenchCommand,
  createDefaultWorkbenchDocument,
  groupsAreWorkbenchAdjacent,
  validateWorkbenchDocument,
  type WorkbenchCommand,
} from "./workbenchModel";
import {
  cloneDocument,
  countTrackedSurfaces,
  createSeededRandom,
  inspectWorkbenchDocument,
  makeDeepWorkbenchDocument,
  makeSingleGroupDocument,
  makeSurface,
  pick,
} from "./workbenchTestUtils";

function acceptedDocument(result: ReturnType<typeof applyWorkbenchCommand>) {
  expect(result.accepted).toBe(true);
  if (!result.accepted) throw new Error(JSON.stringify(result.errors));
  return result.document;
}

function assertCanonicalInvariants(document: WorkbenchDocumentV1): void {
  const validation = validateWorkbenchDocument(document);
  expect(validation.valid).toBe(true);
  const inspection = inspectWorkbenchDocument(document);
  expect(new Set(inspection.group_references).size).toBe(inspection.group_references.length);
  expect([...inspection.group_references].sort()).toEqual(Object.keys(document.groups).sort());
  expect(new Set(inspection.split_node_ids).size).toBe(inspection.split_node_ids.length);
  expect(new Set(inspection.open_surface_references).size).toBe(
    inspection.open_surface_references.length,
  );
  expect([...inspection.open_surface_references].sort()).toEqual(
    Object.keys(document.surfaces).sort(),
  );
  expect(inspection.max_depth).toBeLessThanOrEqual(64);
  expect(document.groups[document.active_group_id]).toBeDefined();
  for (const group of Object.values(document.groups)) {
    if (group.surface_ids.length === 0) {
      expect(group.active_surface_id).toBeNull();
    } else {
      expect(group.surface_ids).toContain(group.active_surface_id);
    }
  }
  const visitRatios = (node: WorkbenchNodeV1): void => {
    if (node.kind === "group") return;
    expect(node.ratio).toBeGreaterThanOrEqual(0.1);
    expect(node.ratio).toBeLessThanOrEqual(0.9);
    visitRatios(node.first);
    visitRatios(node.second);
  };
  visitRatios(document.root);
  expect(document.recently_closed.length).toBeLessThanOrEqual(20);
}

function surfaceLineage(surface: WorkbenchSurfaceV1): string {
  const state = surface.state;
  if (typeof state === "object" && state !== null && !Array.isArray(state)) {
    const lineage = (state as Record<string, unknown>).lineage;
    if (typeof lineage === "string") return lineage;
  }
  return `legacy:${surface.surface_id}`;
}

function trackedSurfaceLineages(document: WorkbenchDocumentV1): string[] {
  return [
    ...Object.values(document.surfaces).map(surfaceLineage),
    ...document.recently_closed.map((closed) => surfaceLineage(closed.surface)),
  ];
}

function removeOne(values: string[], value: string): void {
  const index = values.indexOf(value);
  expect(index).toBeGreaterThanOrEqual(0);
  if (index >= 0) values.splice(index, 1);
}

function makeDeepEdgeAdjacentDocument(depth: number): {
  document: WorkbenchDocumentV1;
  deep_group_id: string;
  neighbor_group_id: string;
} {
  let document = createDefaultWorkbenchDocument();
  document = acceptedDocument(applyWorkbenchCommand(document, {
    type: "split_group",
    group_id: "group-1",
    new_group_id: "group-neighbor",
    node_id: "split-root",
    direction: "horizontal",
    placement: "after",
  }));
  let deepGroupId = "group-1";
  for (let level = 0; level < depth; level += 1) {
    const newGroupId = `group-deep-${level}`;
    const nodeId = `split-deep-${level}`;
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: deepGroupId,
      new_group_id: newGroupId,
      node_id: nodeId,
      direction: "vertical",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "set_split_ratio",
      node_id: nodeId,
      ratio: 0.9,
    }));
    deepGroupId = newGroupId;
  }
  return {
    document,
    deep_group_id: deepGroupId,
    neighbor_group_id: "group-neighbor",
  };
}

function randomWorkbenchCommand(
  document: WorkbenchDocumentV1,
  seed: number,
  step: number,
  random: () => number,
): WorkbenchCommand {
  const groupIds = Object.keys(document.groups);
  const surfaceIds = Object.keys(document.surfaces);
  const splitIds = inspectWorkbenchDocument(document).split_node_ids;
  const commandKind = Math.floor(random() * 12);
  const groupId = pick(groupIds, random) ?? "missing-group";
  const surfaceId = pick(surfaceIds, random) ?? "__missing-surface__";

  switch (commandKind) {
    case 0: {
      const target = document.groups[groupId];
      const useDuplicate = surfaceIds.length > 0 && (surfaceIds.length > 40 || random() < 0.08);
      return {
        type: "open_surface",
        surface: makeSurface(
          useDuplicate ? surfaceId : `surface-${seed}-${step}`,
          {
            surface_type: step % 3 === 0 ? "unknown-random-contribution" : "test-surface",
            state_schema_version: step % 9,
            state: {
              lineage: `lineage-${seed}-${step}`,
              seed,
              step,
              label: `Habitat-${step} 🌿`,
            },
          },
        ),
        group_id: random() < 0.08 ? "missing-group" : groupId,
        index: random() < 0.08 ? -1 : target?.surface_ids.length ?? 0,
      };
    }
    case 1:
      return {
        type: "focus_surface",
        surface_id: random() < 0.12 ? "__missing-surface__" : surfaceId,
      };
    case 2:
      return {
        type: "close_surface",
        surface_id: random() < 0.12 ? "__missing-surface__" : surfaceId,
      };
    case 3:
      return { type: "reopen_closed_surface" };
    case 4:
      return {
        type: "split_group",
        group_id: random() < 0.08 ? "missing-group" : groupId,
        new_group_id: groupIds.length < 8 ? `group-${seed}-${step}` : groupId,
        node_id: `split-${seed}-${step}`,
        direction: random() < 0.5 ? "horizontal" : "vertical",
        placement: random() < 0.5 ? "before" : "after",
      };
    case 5: {
      const target = document.groups[groupId];
      return {
        type: "move_surface",
        surface_id: random() < 0.1 ? "__missing-surface__" : surfaceId,
        group_id: random() < 0.1 ? "missing-group" : groupId,
        index: random() < 0.1 ? -1 : Math.floor(random() * ((target?.surface_ids.length ?? 0) + 1)),
      };
    }
    case 6: {
      const group = document.groups[groupId];
      const activeSurface = group?.surface_ids.length
        ? pick(group.surface_ids, random) ?? null
        : null;
      return {
        type: "set_active_surface",
        group_id: random() < 0.08 ? "missing-group" : groupId,
        surface_id: random() < 0.08 ? "__missing-surface__" : activeSurface,
      };
    }
    case 7:
      return {
        type: "set_split_ratio",
        node_id: random() < 0.1 ? "missing-split" : pick(splitIds, random) ?? "missing-split",
        ratio: random() < 0.05 ? Number.NaN : (random() * 1.4) - 0.2,
      };
    case 8:
      return {
        type: "close_group",
        group_id: random() < 0.1 ? "missing-group" : groupId,
      };
    case 9: {
      const source = pick(groupIds, random) ?? "missing-group";
      const targets = groupIds.filter((candidate) => candidate !== source);
      return {
        type: "join_group",
        source_group_id: source,
        target_group_id: random() < 0.1
          ? source
          : pick(targets, random) ?? "missing-group",
      };
    }
    case 10: {
      const current = document.surfaces[surfaceId];
      return {
        type: "update_surface_state",
        surface_id: random() < 0.1 ? "__missing-surface__" : surfaceId,
        state_schema_version: step % 11,
        state: {
          lineage: current ? surfaceLineage(current) : `missing-${seed}-${step}`,
          seed,
          step,
          opaque: [true, null, `value-${step}`],
        },
      };
    }
    default:
      return {
        type: "update_shell",
        patch: step % 2 === 0
          ? { bottom_terminal_open: random() < 0.5 }
          : { right_sidebar_width: 200 + Math.floor(random() * 300) },
      };
  }
}

describe("workbench model", () => {
  it("creates one empty active group so Home is derived rather than persisted", () => {
    const document = createDefaultWorkbenchDocument();

    expect(document.root).toEqual({ kind: "group", group_id: "group-1" });
    expect(document.groups).toEqual({
      "group-1": {
        group_id: "group-1",
        surface_ids: [],
        active_surface_id: null,
      },
    });
    expect(document.surfaces).toEqual({});
    expect(document.revision).toBe(0);
    expect(Object.keys(document.surfaces)).not.toContain("home");
  });

  it("returns the exact validated document without normalization", () => {
    const document = makeSingleGroupDocument([makeSurface("surface-1")]);
    const result = validateWorkbenchDocument(document);

    expect(result.valid).toBe(true);
    if (result.valid) expect(result.document).toBe(document);
  });

  it("rejects duplicate tree/group and tab/surface references", () => {
    const duplicateGroup = makeSingleGroupDocument();
    duplicateGroup.root = {
      kind: "split",
      node_id: "split-1",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-1" },
      second: { kind: "group", group_id: "group-1" },
    };

    const duplicateSurface = makeSingleGroupDocument([makeSurface("surface-1")]);
    duplicateSurface.groups["group-1"].surface_ids.push("surface-1");

    expect(validateWorkbenchDocument(duplicateGroup).valid).toBe(false);
    expect(validateWorkbenchDocument(duplicateSurface).valid).toBe(false);
  });

  it("rejects missing/orphan references and invalid active IDs", () => {
    const missingSurface = makeSingleGroupDocument([makeSurface("surface-1")]);
    delete missingSurface.surfaces["surface-1"];

    const orphanSurface = makeSingleGroupDocument();
    orphanSurface.surfaces["orphan"] = makeSurface("orphan");

    const missingActive = makeSingleGroupDocument([makeSurface("surface-1")]);
    missingActive.groups["group-1"].active_surface_id = "missing";

    const orphanGroup = makeSingleGroupDocument();
    orphanGroup.groups["orphan"] = {
      group_id: "orphan",
      surface_ids: [],
      active_surface_id: null,
    };

    expect(validateWorkbenchDocument(missingSurface).valid).toBe(false);
    expect(validateWorkbenchDocument(orphanSurface).valid).toBe(false);
    expect(validateWorkbenchDocument(missingActive).valid).toBe(false);
    expect(validateWorkbenchDocument(orphanGroup).valid).toBe(false);
  });

  it("rejects invalid ratios, duplicate split IDs, and cycles", () => {
    const invalidRatio = makeDeepWorkbenchDocument(2);
    if (invalidRatio.root.kind === "split") invalidRatio.root.ratio = 0.09;

    const duplicateSplit = makeDeepWorkbenchDocument(3);
    if (
      duplicateSplit.root.kind === "split" &&
      duplicateSplit.root.second.kind === "split"
    ) {
      duplicateSplit.root.second.node_id = duplicateSplit.root.node_id;
    }

    const cyclic = makeDeepWorkbenchDocument(2);
    const root = cyclic.root as Extract<WorkbenchNodeV1, { kind: "split" }>;
    root.second = root;

    expect(validateWorkbenchDocument(invalidRatio).valid).toBe(false);
    expect(validateWorkbenchDocument(duplicateSplit).valid).toBe(false);
    expect(validateWorkbenchDocument(cyclic).valid).toBe(false);
  });

  it("accepts tree depth 64 and rejects depth 65", () => {
    expect(validateWorkbenchDocument(makeDeepWorkbenchDocument(64)).valid).toBe(true);
    expect(validateWorkbenchDocument(makeDeepWorkbenchDocument(65)).valid).toBe(false);
  });

  it("opens and focuses surfaces immutably", () => {
    const original = createDefaultWorkbenchDocument();
    const first = makeSurface("surface-1", {
      surface_type: "unknown-plugin",
      state_schema_version: 7,
      state: { opaque: ["kept", { value: 1 }] },
    });
    const opened = acceptedDocument(applyWorkbenchCommand(original, {
      type: "open_surface",
      surface: first,
    }));
    const withSecond = acceptedDocument(applyWorkbenchCommand(opened, {
      type: "open_surface",
      surface: makeSurface("surface-2"),
      index: 0,
    }));
    const focused = acceptedDocument(applyWorkbenchCommand(withSecond, {
      type: "focus_surface",
      surface_id: "surface-1",
    }));

    expect(original.groups["group-1"].surface_ids).toEqual([]);
    expect(withSecond.groups["group-1"].surface_ids).toEqual(["surface-2", "surface-1"]);
    expect(focused.active_group_id).toBe("group-1");
    expect(focused.groups["group-1"].active_surface_id).toBe("surface-1");
    expect(focused.surfaces["surface-1"].state).toBe(first.state);
    expect(focused.revision).toBe(0);
    expect(focused.saved_at).toBe(original.saved_at);
  });

  it("rejects invalid open/focus commands with the exact original object", () => {
    const document = makeSingleGroupDocument([makeSurface("surface-1")]);
    const duplicate = applyWorkbenchCommand(document, {
      type: "open_surface",
      surface: makeSurface("surface-1"),
    });
    const badIndex = applyWorkbenchCommand(document, {
      type: "open_surface",
      surface: makeSurface("surface-2"),
      index: 3,
    });
    const missingFocus = applyWorkbenchCommand(document, {
      type: "focus_surface",
      surface_id: "missing",
    });

    expect(duplicate.accepted).toBe(false);
    expect(duplicate.document).toBe(document);
    expect(badIndex.document).toBe(document);
    expect(missingFocus.document).toBe(document);
  });

  it("closes in tab order and reopens the newest snapshot at its prior index", () => {
    let document = makeSingleGroupDocument([
      makeSurface("surface-1"),
      makeSurface("surface-2"),
      makeSurface("surface-3"),
    ]);
    document.groups["group-1"].active_surface_id = "surface-2";

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "close_surface",
      surface_id: "surface-2",
    }));
    expect(document.groups["group-1"].surface_ids).toEqual(["surface-1", "surface-3"]);
    expect(document.groups["group-1"].active_surface_id).toBe("surface-3");
    expect(document.recently_closed[0]).toMatchObject({
      previous_group_id: "group-1",
      previous_index: 1,
      surface: { surface_id: "surface-2" },
    });

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "reopen_closed_surface",
    }));
    expect(document.groups["group-1"].surface_ids).toEqual([
      "surface-1",
      "surface-2",
      "surface-3",
    ]);
    expect(document.groups["group-1"].active_surface_id).toBe("surface-2");
    expect(document.recently_closed).toEqual([]);
  });

  it("caps recently closed at 20 and deterministically resolves reopen ID collisions", () => {
    const surfaces = Array.from({ length: 22 }, (_, index) => makeSurface(`surface-${index}`));
    let document = makeSingleGroupDocument(surfaces);
    for (const surface of surfaces) {
      document = acceptedDocument(applyWorkbenchCommand(document, {
        type: "close_surface",
        surface_id: surface.surface_id,
      }));
    }
    expect(document.recently_closed).toHaveLength(20);
    expect(document.recently_closed[0].surface.surface_id).toBe("surface-21");
    expect(document.recently_closed.some((entry) => entry.surface.surface_id === "surface-0")).toBe(false);

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "open_surface",
      surface: makeSurface("surface-21", { state: { replacement: true } }),
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "reopen_closed_surface",
    }));
    expect(document.groups["group-1"].surface_ids).toContain("surface-21-reopened");
    expect(document.surfaces["surface-21-reopened"].state).toEqual({ label: "surface-21" });
  });

  it("splits a group with deterministic placement and a derived empty-group Home", () => {
    const document = makeSingleGroupDocument([makeSurface("surface-1")]);
    const split = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "vertical",
      placement: "before",
    }));

    expect(split.root).toEqual({
      kind: "split",
      node_id: "split-1",
      direction: "vertical",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-2" },
      second: { kind: "group", group_id: "group-1" },
    });
    expect(split.groups["group-2"]).toEqual({
      group_id: "group-2",
      surface_ids: [],
      active_surface_id: null,
    });
    expect(split.active_group_id).toBe("group-2");
    expect(document.root).toEqual({ kind: "group", group_id: "group-1" });

    const duplicate = applyWorkbenchCommand(split, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-2",
      direction: "horizontal",
      placement: "after",
    });
    expect(duplicate.accepted).toBe(false);
    expect(duplicate.document).toBe(split);
  });

  it("moves surfaces within and between groups without losing their records", () => {
    let document = makeSingleGroupDocument([
      makeSurface("surface-1"),
      makeSurface("surface-2"),
      makeSurface("surface-3"),
    ]);
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "move_surface",
      surface_id: "surface-1",
      group_id: "group-1",
      index: 3,
    }));
    expect(document.groups["group-1"].surface_ids).toEqual([
      "surface-2",
      "surface-3",
      "surface-1",
    ]);

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    }));
    const surfaceRecord = document.surfaces["surface-2"];
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "move_surface",
      surface_id: "surface-2",
      group_id: "group-2",
      index: 0,
    }));
    expect(document.groups["group-1"].surface_ids).toEqual(["surface-3", "surface-1"]);
    expect(document.groups["group-2"].surface_ids).toEqual(["surface-2"]);
    expect(document.groups["group-2"].active_surface_id).toBe("surface-2");
    expect(document.active_group_id).toBe("group-2");
    expect(document.surfaces["surface-2"]).toBe(surfaceRecord);

    const rejected = applyWorkbenchCommand(document, {
      type: "move_surface",
      surface_id: "surface-2",
      group_id: "group-1",
      index: 99,
    });
    expect(rejected.accepted).toBe(false);
    expect(rejected.document).toBe(document);
  });

  it("collapses an empty source group when its sole surface moves to a sibling", () => {
    let document = makeSingleGroupDocument([makeSurface("surface-1")]);
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    }));
    const surfaceRecord = document.surfaces["surface-1"];

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "move_surface",
      surface_id: "surface-1",
      group_id: "group-2",
      index: 0,
    }));

    expect(document.root).toEqual({ kind: "group", group_id: "group-2" });
    expect(document.groups["group-1"]).toBeUndefined();
    expect(document.groups["group-2"]).toEqual({
      group_id: "group-2",
      surface_ids: ["surface-1"],
      active_surface_id: "surface-1",
    });
    expect(document.active_group_id).toBe("group-2");
    expect(document.surfaces["surface-1"]).toBe(surfaceRecord);
  });

  it("collapses a nested empty source without changing unaffected topology or ratios", () => {
    let document = makeSingleGroupDocument([makeSurface("surface-1")]);
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-root",
      direction: "horizontal",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "set_split_ratio",
      node_id: "split-root",
      ratio: 0.3,
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-3",
      node_id: "split-nested",
      direction: "vertical",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "set_split_ratio",
      node_id: "split-nested",
      ratio: 0.7,
    }));

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "move_surface",
      surface_id: "surface-1",
      group_id: "group-2",
      index: 0,
    }));

    expect(document.root).toEqual({
      kind: "split",
      node_id: "split-root",
      direction: "horizontal",
      ratio: 0.3,
      first: { kind: "group", group_id: "group-3" },
      second: { kind: "group", group_id: "group-2" },
    });
    expect(document.groups["group-1"]).toBeUndefined();
    expect(document.groups["group-3"]).toEqual({
      group_id: "group-3",
      surface_ids: [],
      active_surface_id: null,
    });
    expect(Object.keys(document.surfaces)).toEqual(["surface-1"]);
    expect(document.active_group_id).toBe("group-2");
  });

  it("never collapses a source group during same-group reorder", () => {
    let document = makeSingleGroupDocument([
      makeSurface("surface-1"),
      makeSurface("surface-2"),
    ]);
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    }));
    const root = document.root;

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "move_surface",
      surface_id: "surface-1",
      group_id: "group-1",
      index: 2,
    }));

    expect(document.root).toBe(root);
    expect(document.groups["group-1"].surface_ids).toEqual(["surface-2", "surface-1"]);
    expect(document.groups["group-2"]).toBeDefined();
  });

  it("retains a multi-tab source group after a cross-group move", () => {
    let document = makeSingleGroupDocument([
      makeSurface("surface-1"),
      makeSurface("surface-2"),
    ]);
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    }));
    const root = document.root;

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "move_surface",
      surface_id: "surface-1",
      group_id: "group-2",
      index: 0,
    }));

    expect(document.root).toBe(root);
    expect(document.groups["group-1"].surface_ids).toEqual(["surface-2"]);
    expect(document.groups["group-2"].surface_ids).toEqual(["surface-1"]);
    expect(Object.keys(document.surfaces).sort()).toEqual(["surface-1", "surface-2"]);
  });

  it("preserves the final group when moving its sole surface", () => {
    const original = makeSingleGroupDocument([makeSurface("surface-1")]);

    const document = acceptedDocument(applyWorkbenchCommand(original, {
      type: "move_surface",
      surface_id: "surface-1",
      group_id: "group-1",
      index: 1,
    }));

    expect(document.root).toEqual({ kind: "group", group_id: "group-1" });
    expect(Object.keys(document.groups)).toEqual(["group-1"]);
    expect(document.groups["group-1"]).toEqual({
      group_id: "group-1",
      surface_ids: ["surface-1"],
      active_surface_id: "surface-1",
    });
  });

  it("enforces active-tab membership and clamps only finite split ratios", () => {
    let document = makeSingleGroupDocument([makeSurface("surface-1")]);
    const nullActive = applyWorkbenchCommand(document, {
      type: "set_active_surface",
      group_id: "group-1",
      surface_id: null,
    });
    expect(nullActive.accepted).toBe(false);
    expect(nullActive.document).toBe(document);

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "set_active_surface",
      group_id: "group-2",
      surface_id: null,
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "set_split_ratio",
      node_id: "split-1",
      ratio: 0.01,
    }));
    expect(document.root.kind === "split" && document.root.ratio).toBe(0.1);
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "set_split_ratio",
      node_id: "split-1",
      ratio: 4,
    }));
    expect(document.root.kind === "split" && document.root.ratio).toBe(0.9);

    const nonFinite = applyWorkbenchCommand(document, {
      type: "set_split_ratio",
      node_id: "split-1",
      ratio: Number.NaN,
    });
    expect(nonFinite.accepted).toBe(false);
    expect(nonFinite.document).toBe(document);
  });

  it("closes a group transactionally, collapses its parent, and activates the sibling's leftmost group", () => {
    let document = makeSingleGroupDocument([
      makeSurface("surface-1"),
      makeSurface("surface-2"),
    ]);
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-2",
      new_group_id: "group-3",
      node_id: "split-2",
      direction: "vertical",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "close_group",
      group_id: "group-1",
    }));

    expect(document.root).toEqual({
      kind: "split",
      node_id: "split-2",
      direction: "vertical",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-2" },
      second: { kind: "group", group_id: "group-3" },
    });
    expect(document.active_group_id).toBe("group-2");
    expect(document.groups["group-1"]).toBeUndefined();
    expect(Object.keys(document.surfaces)).toEqual([]);
    expect(document.recently_closed.map((entry) => entry.surface.surface_id)).toEqual([
      "surface-2",
      "surface-1",
    ]);
  });

  it("retains the only group empty on close and never redistributes its tabs", () => {
    const original = makeSingleGroupDocument([
      makeSurface("surface-1"),
      makeSurface("surface-2"),
    ]);
    const closed = acceptedDocument(applyWorkbenchCommand(original, {
      type: "close_group",
      group_id: "group-1",
    }));

    expect(closed.root).toEqual({ kind: "group", group_id: "group-1" });
    expect(closed.groups["group-1"].surface_ids).toEqual([]);
    expect(closed.groups["group-1"].active_surface_id).toBeNull();
    expect(closed.recently_closed.map((entry) => entry.surface.surface_id)).toEqual([
      "surface-2",
      "surface-1",
    ]);
    expect(original.groups["group-1"].surface_ids).toEqual(["surface-1", "surface-2"]);
  });

  it("activates the removed group's sibling subtree rather than the whole tree's leftmost group", () => {
    let document = createDefaultWorkbenchDocument();
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-2",
      new_group_id: "group-3",
      node_id: "split-2",
      direction: "horizontal",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "close_group",
      group_id: "group-3",
    }));

    expect(document.active_group_id).toBe("group-2");
    expect(document.root).toEqual({
      kind: "split",
      node_id: "split-1",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-1" },
      second: { kind: "group", group_id: "group-2" },
    });
  });

  it("joins a source group into a target in order without closing surfaces", () => {
    let document = makeSingleGroupDocument([
      makeSurface("surface-1"),
      makeSurface("surface-2"),
    ]);
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "open_surface",
      surface: makeSurface("surface-3"),
      group_id: "group-2",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "open_surface",
      surface: makeSurface("surface-4"),
      group_id: "group-2",
    }));
    const joined = acceptedDocument(applyWorkbenchCommand(document, {
      type: "join_group",
      source_group_id: "group-2",
      target_group_id: "group-1",
    }));

    expect(joined.root).toEqual({ kind: "group", group_id: "group-1" });
    expect(joined.groups["group-1"].surface_ids).toEqual([
      "surface-1",
      "surface-2",
      "surface-3",
      "surface-4",
    ]);
    expect(joined.groups["group-1"].active_surface_id).toBe("surface-4");
    expect(joined.recently_closed).toEqual([]);
    expect(Object.keys(joined.surfaces)).toHaveLength(4);

    const sameGroup = applyWorkbenchCommand(joined, {
      type: "join_group",
      source_group_id: "group-1",
      target_group_id: "group-1",
    });
    expect(sameGroup.accepted).toBe(false);
    expect(sameGroup.document).toBe(joined);
  });

  it("rejects joining groups that are not spatially adjacent", () => {
    let document = createDefaultWorkbenchDocument();
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    }));
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "split_group",
      group_id: "group-2",
      new_group_id: "group-3",
      node_id: "split-2",
      direction: "horizontal",
      placement: "after",
    }));

    const nonAdjacent = applyWorkbenchCommand(document, {
      type: "join_group",
      source_group_id: "group-1",
      target_group_id: "group-3",
    });
    expect(nonAdjacent.accepted).toBe(false);
    expect(nonAdjacent.document).toBe(document);

    expect(applyWorkbenchCommand(document, {
      type: "join_group",
      source_group_id: "group-1",
      target_group_id: "group-2",
    }).accepted).toBe(true);

    expect(groupsAreWorkbenchAdjacent(document.root, "group-1", "group-2")).toBe(true);
    expect(groupsAreWorkbenchAdjacent(document.root, "group-1", "group-3")).toBe(false);
  });

  it("updates opaque surface state and applies only the six shell fields", () => {
    let document = makeSingleGroupDocument([makeSurface("surface-1")]);
    const opaqueState = { plugin_data: { nested: [1, "two", true, null] } };
    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "update_surface_state",
      surface_id: "surface-1",
      state_schema_version: 19,
      state: opaqueState,
    }));
    expect(document.surfaces["surface-1"].state_schema_version).toBe(19);
    expect(document.surfaces["surface-1"].state).toBe(opaqueState);

    document = acceptedDocument(applyWorkbenchCommand(document, {
      type: "update_shell",
      patch: {
        left_sidebar_collapsed: true,
        bottom_terminal_height: 444,
      },
    }));
    expect(document.shell.left_sidebar_collapsed).toBe(true);
    expect(document.shell.bottom_terminal_height).toBe(444);

    const unknownPatch = applyWorkbenchCommand(document, {
      type: "update_shell",
      patch: { settings_open: true },
    } as unknown as Parameters<typeof applyWorkbenchCommand>[1]);
    const nonFinite = applyWorkbenchCommand(document, {
      type: "update_shell",
      patch: { left_sidebar_width: Number.POSITIVE_INFINITY },
    });
    expect(unknownPatch.accepted).toBe(false);
    expect(unknownPatch.document).toBe(document);
    expect(nonFinite.accepted).toBe(false);
    expect(nonFinite.document).toBe(document);
  });

  it("enforces surface state limits using serialized UTF-8 bytes", () => {
    const asciiAtLimit = makeSingleGroupDocument([
      makeSurface("surface-1", { state: "a".repeat((64 * 1024) - 2) }),
    ]);
    const asciiOverLimit = makeSingleGroupDocument([
      makeSurface("surface-1", { state: "a".repeat((64 * 1024) - 1) }),
    ]);
    const multibyteOverLimit = makeSingleGroupDocument([
      makeSurface("surface-1", { state: "😀".repeat(16_384) }),
    ]);

    expect(validateWorkbenchDocument(asciiAtLimit).valid).toBe(true);
    expect(validateWorkbenchDocument(asciiOverLimit).valid).toBe(false);
    expect(validateWorkbenchDocument(multibyteOverLimit).valid).toBe(false);
  });

  it("enforces the 2 MiB document limit while each surface remains under 64 KiB", () => {
    const surfaces = Array.from({ length: 36 }, (_, index) => makeSurface(
      `surface-${index}`,
      { state: "x".repeat(60_000) },
    ));
    expect(validateWorkbenchDocument(makeSingleGroupDocument(surfaces)).valid).toBe(false);
  });

  it("rejects non-JSON opaque state, exact-shape violations, and more than 20 closed snapshots", () => {
    const undefinedState = makeSingleGroupDocument([
      makeSurface("surface-1", { state: { missing: undefined } }),
    ]);
    const extraDocumentField = {
      ...createDefaultWorkbenchDocument(),
      zoomed_group_id: "group-1",
    };
    const tooManyClosed = createDefaultWorkbenchDocument();
    tooManyClosed.recently_closed = Array.from({ length: 21 }, (_, index) => ({
      surface: makeSurface(`closed-${index}`),
      previous_group_id: "group-1",
      previous_index: index,
    }));
    const sparseClosed = createDefaultWorkbenchDocument();
    sparseClosed.recently_closed = new Array<ClosedSurfaceV1>(1);

    expect(validateWorkbenchDocument(undefinedState).valid).toBe(false);
    expect(validateWorkbenchDocument(extraDocumentField).valid).toBe(false);
    expect(validateWorkbenchDocument(tooManyClosed).valid).toBe(false);
    expect(validateWorkbenchDocument(sparseClosed).valid).toBe(false);
  });

  it("returns invalid rather than throwing for pathologically deep opaque JSON", () => {
    let state: unknown = null;
    for (let depth = 0; depth < 5_000; depth += 1) state = { next: state };
    const document = makeSingleGroupDocument([makeSurface("surface-1", { state })]);

    expect(() => validateWorkbenchDocument(document)).not.toThrow();
    expect(validateWorkbenchDocument(document).valid).toBe(false);
  });

  it("validates both pre-state and post-state and preserves original identity on rejection", () => {
    const corrupt = makeSingleGroupDocument([makeSurface("surface-1")]);
    corrupt.groups["group-1"].surface_ids.push("surface-1");
    const preRejected = applyWorkbenchCommand(corrupt, {
      type: "focus_surface",
      surface_id: "surface-1",
    });
    expect(preRejected.accepted).toBe(false);
    expect(preRejected.document).toBe(corrupt);

    const valid = makeSingleGroupDocument([makeSurface("surface-1")]);
    const before = structuredClone(valid);
    const postRejected = applyWorkbenchCommand(valid, {
      type: "update_surface_state",
      surface_id: "surface-1",
      state_schema_version: 2,
      state: "😀".repeat(16_384),
    });
    expect(postRejected.accepted).toBe(false);
    expect(postRejected.document).toBe(valid);
    expect(valid).toEqual(before);
  });

  it("parses the shared Rust/TypeScript fixture into the exact V1 DTO", () => {
    const fixturePath = resolve(
      process.cwd(),
      "crates/wardian-core/tests/fixtures/workbench-v1.json",
    );
    const parsed: unknown = JSON.parse(readFileSync(fixturePath, "utf8"));
    const result = validateWorkbenchDocument(parsed);

    expect(result.valid).toBe(true);
    if (!result.valid) throw new Error(JSON.stringify(result.errors));
    const exactDocument: WorkbenchDocumentV1 = result.document;
    expect(exactDocument.schema_version).toBe(1);
    expect(exactDocument.surfaces["surface-missing-plugin"].state).toEqual({
      query: "status:open",
      columns: ["name", "status"],
      unicode_label: "Habitat 🌿",
    });
  });

  it("requires saved_at to be a canonical finite UTC millisecond timestamp", () => {
    const validTimestamps = [
      "0000-01-01T00:00:00.000Z",
      "1970-01-01T00:00:00.000Z",
      "2026-07-10T12:34:56.789Z",
      "9999-12-31T23:59:59.999Z",
    ];
    for (const saved_at of validTimestamps) {
      expect(validateWorkbenchDocument({
        ...createDefaultWorkbenchDocument(),
        saved_at,
      }).valid).toBe(true);
    }

    const invalidTimestamps = [
      "",
      "not-a-timestamp",
      "2026-02-30T00:00:00.000Z",
      "2026-07-10T12:34:56Z",
      "2026-07-10T12:34:56.789+00:00",
      "2026-07-10t12:34:56.789z",
      "+010000-01-01T00:00:00.000Z",
    ];
    for (const saved_at of invalidTimestamps) {
      expect(validateWorkbenchDocument({
        ...createDefaultWorkbenchDocument(),
        saved_at,
      }).valid).toBe(false);
    }
  });

  it("rejects custom prototypes throughout the canonical DTO graph", () => {
    const documents = Array.from({ length: 6 }, () => makeSingleGroupDocument([
      makeSurface("surface-1"),
    ]));
    Object.setPrototypeOf(documents[0], { custom: true });
    Object.setPrototypeOf(documents[1].root, { custom: true });
    Object.setPrototypeOf(documents[2].groups, { custom: true });
    Object.setPrototypeOf(documents[3].groups["group-1"], { custom: true });
    Object.setPrototypeOf(documents[4].surfaces["surface-1"], { custom: true });
    Object.setPrototypeOf(documents[5].shell, { custom: true });

    for (const document of documents) {
      expect(validateWorkbenchDocument(document).valid).toBe(false);
    }
  });

  it("rejects symbol, non-enumerable, accessor, and extra array properties without invoking getters", () => {
    const symbolDocument = createDefaultWorkbenchDocument();
    Object.defineProperty(symbolDocument.shell, Symbol("hidden"), {
      value: true,
      enumerable: true,
    });

    const nonEnumerableDocument = createDefaultWorkbenchDocument();
    Object.defineProperty(nonEnumerableDocument.groups["group-1"], "hidden", {
      value: true,
      enumerable: false,
    });

    let rootGetterInvoked = false;
    const accessorDocument = createDefaultWorkbenchDocument();
    Object.defineProperty(accessorDocument, "saved_at", {
      enumerable: true,
      get() {
        rootGetterInvoked = true;
        throw new Error("saved_at getter must not run");
      },
    });

    const extraArrayDocument = makeSingleGroupDocument([makeSurface("surface-1")]);
    Object.defineProperty(extraArrayDocument.groups["group-1"].surface_ids, "extra", {
      value: true,
      enumerable: true,
    });

    let arrayGetterInvoked = false;
    const accessorArrayDocument = makeSingleGroupDocument([makeSurface("surface-1")]);
    Object.defineProperty(accessorArrayDocument.groups["group-1"].surface_ids, "0", {
      enumerable: true,
      get() {
        arrayGetterInvoked = true;
        throw new Error("array getter must not run");
      },
    });

    expect(validateWorkbenchDocument(symbolDocument).valid).toBe(false);
    expect(validateWorkbenchDocument(nonEnumerableDocument).valid).toBe(false);
    expect(() => validateWorkbenchDocument(accessorDocument)).not.toThrow();
    expect(validateWorkbenchDocument(accessorDocument).valid).toBe(false);
    expect(rootGetterInvoked).toBe(false);
    expect(validateWorkbenchDocument(extraArrayDocument).valid).toBe(false);
    expect(() => validateWorkbenchDocument(accessorArrayDocument)).not.toThrow();
    expect(validateWorkbenchDocument(accessorArrayDocument).valid).toBe(false);
    expect(arrayGetterInvoked).toBe(false);
  });

  it("validates opaque state as getter-safe canonical JSON while allowing null-prototype records", () => {
    const nullPrototypeState = Object.assign(Object.create(null) as Record<string, unknown>, {
      label: "plain-null-prototype",
      nested: [1, true, null],
    });
    expect(validateWorkbenchDocument(makeSingleGroupDocument([
      makeSurface("surface-1", { state: nullPrototypeState }),
    ])).valid).toBe(true);

    let stateGetterInvoked = false;
    const accessorState = Object.create(null) as Record<string, unknown>;
    Object.defineProperty(accessorState, "secret", {
      enumerable: true,
      get() {
        stateGetterInvoked = true;
        throw new Error("state getter must not run");
      },
    });
    const accessorStateDocument = makeSingleGroupDocument([
      makeSurface("surface-1", { state: accessorState }),
    ]);
    expect(() => validateWorkbenchDocument(accessorStateDocument)).not.toThrow();
    expect(validateWorkbenchDocument(accessorStateDocument).valid).toBe(false);
    expect(stateGetterInvoked).toBe(false);

    const symbolState = { visible: true };
    Object.defineProperty(symbolState, Symbol("hidden"), { value: true, enumerable: true });
    expect(validateWorkbenchDocument(makeSingleGroupDocument([
      makeSurface("surface-1", { state: symbolState }),
    ])).valid).toBe(false);
  });

  it("keeps positive deep shared edges adjacent at extreme ratios without accepting corners", () => {
    for (const depth of [1, 30, 62]) {
      const { document, deep_group_id, neighbor_group_id } = makeDeepEdgeAdjacentDocument(depth);
      expect(applyWorkbenchCommand(document, {
        type: "join_group",
        source_group_id: deep_group_id,
        target_group_id: neighbor_group_id,
      }).accepted, `depth=${depth}`).toBe(true);
    }

    let cornerDocument = createDefaultWorkbenchDocument();
    cornerDocument = acceptedDocument(applyWorkbenchCommand(cornerDocument, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-horizontal",
      direction: "horizontal",
      placement: "after",
    }));
    cornerDocument = acceptedDocument(applyWorkbenchCommand(cornerDocument, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-3",
      node_id: "split-left-vertical",
      direction: "vertical",
      placement: "after",
    }));
    cornerDocument = acceptedDocument(applyWorkbenchCommand(cornerDocument, {
      type: "split_group",
      group_id: "group-2",
      new_group_id: "group-4",
      node_id: "split-right-vertical",
      direction: "vertical",
      placement: "after",
    }));
    expect(applyWorkbenchCommand(cornerDocument, {
      type: "join_group",
      source_group_id: "group-1",
      target_group_id: "group-4",
    }).accepted).toBe(false);
    expect(applyWorkbenchCommand(cornerDocument, {
      type: "join_group",
      source_group_id: "group-1",
      target_group_id: "group-2",
    }).accepted).toBe(true);
  });

  it("preserves invariants across 10,000 fixed-seed randomized commands", () => {
    const seeds = [1, 42, 0x5eedc0de, 20_260_710] as const;
    const operationsPerSeed = 2_500;

    for (const seed of seeds) {
      const random = createSeededRandom(seed);
      let document = createDefaultWorkbenchDocument();
      for (let step = 0; step < operationsPerSeed; step += 1) {
        const command = randomWorkbenchCommand(document, seed, step, random);
        const original = document;
        const originalSnapshot = cloneDocument(original);
        const trackedBefore = countTrackedSurfaces(original);
        const expectedLineages = trackedSurfaceLineages(original);
        const closedBefore = original.recently_closed.length;
        const closingGroupSize = command.type === "close_group"
          ? original.groups[command.group_id]?.surface_ids.length ?? 0
          : 0;
        const result = applyWorkbenchCommand(original, command);

        expect(original).toEqual(originalSnapshot);
        if (!result.accepted) {
          expect(result.document).toBe(original);
          continue;
        }

        expect(result.document).not.toBe(original);
        document = result.document;
        assertCanonicalInvariants(document);

        let expectedTracked = trackedBefore;
        if (command.type === "open_surface") expectedTracked += 1;
        if (command.type === "open_surface") {
          expectedLineages.push(surfaceLineage(command.surface));
        }
        if (command.type === "close_surface" && closedBefore === 20) expectedTracked -= 1;
        if (command.type === "close_surface" && closedBefore === 20) {
          removeOne(expectedLineages, surfaceLineage(original.recently_closed[19].surface));
        }
        if (command.type === "close_group") {
          expectedTracked -= Math.max(0, closedBefore + closingGroupSize - 20);
          const simulatedClosed = [...original.recently_closed];
          for (const closingSurfaceId of original.groups[command.group_id].surface_ids) {
            simulatedClosed.unshift({
              surface: original.surfaces[closingSurfaceId],
              previous_group_id: command.group_id,
              previous_index: 0,
            });
            if (simulatedClosed.length > 20) {
              const evicted = simulatedClosed.pop();
              if (evicted) removeOne(expectedLineages, surfaceLineage(evicted.surface));
            }
          }
        }
        expect(countTrackedSurfaces(document)).toBe(expectedTracked);
        expect(
          trackedSurfaceLineages(document).sort(),
          `seed=${seed} step=${step} command=${JSON.stringify(command)}`,
        ).toEqual(expectedLineages.sort());
      }
    }
  }, 30_000);
});

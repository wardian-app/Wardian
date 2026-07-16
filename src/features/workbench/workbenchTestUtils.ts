import type {
  WorkbenchDocumentV1,
  WorkbenchGroupV1,
  WorkbenchNodeV1,
  WorkbenchSurfaceV1,
} from "../../types";

export const DEFAULT_TEST_SHELL = {
  left_sidebar_collapsed: false,
  left_sidebar_width: 240,
  right_sidebar_collapsed: false,
  right_sidebar_width: 240,
  bottom_terminal_open: false,
  bottom_terminal_height: 360,
} as const;

export function makeSurface(
  surface_id: string,
  overrides: Partial<Omit<WorkbenchSurfaceV1, "surface_id">> = {},
): WorkbenchSurfaceV1 {
  return {
    surface_id,
    surface_type: "test-surface",
    state_schema_version: 1,
    state: { label: surface_id },
    ...overrides,
  };
}

export function makeSingleGroupDocument(
  surfaces: WorkbenchSurfaceV1[] = [],
  group_id = "group-1",
): WorkbenchDocumentV1 {
  const surfaceRecords = Object.fromEntries(
    surfaces.map((surface) => [surface.surface_id, surface]),
  );
  const surfaceIds = surfaces.map((surface) => surface.surface_id);

  return {
    schema_version: 1,
    revision: 0,
    saved_at: "1970-01-01T00:00:00.000Z",
    root: { kind: "group", group_id },
    groups: {
      [group_id]: {
        group_id,
        surface_ids: surfaceIds,
        active_surface_id: surfaceIds.length > 0 ? surfaceIds[surfaceIds.length - 1] : null,
      },
    },
    surfaces: surfaceRecords,
    active_group_id: group_id,
    recently_closed: [],
    shell: { ...DEFAULT_TEST_SHELL },
  };
}

export function makeDeepWorkbenchDocument(depth: number): WorkbenchDocumentV1 {
  if (!Number.isInteger(depth) || depth < 1) {
    throw new Error("depth must be a positive integer");
  }

  const groups: Record<string, WorkbenchGroupV1> = {};
  const makeGroup = (index: number): WorkbenchNodeV1 => {
    const group_id = `group-${index}`;
    groups[group_id] = {
      group_id,
      surface_ids: [],
      active_surface_id: null,
    };
    return { kind: "group", group_id };
  };

  let root = makeGroup(depth);
  for (let level = depth - 1; level >= 1; level -= 1) {
    root = {
      kind: "split",
      node_id: `split-${level}`,
      direction: level % 2 === 0 ? "vertical" : "horizontal",
      ratio: 0.5,
      first: makeGroup(level),
      second: root,
    };
  }

  return {
    ...makeSingleGroupDocument(),
    root,
    groups,
    active_group_id: "group-1",
  };
}

export type WorkbenchInspection = {
  group_references: string[];
  split_node_ids: string[];
  open_surface_references: string[];
  max_depth: number;
};

export function inspectWorkbenchDocument(
  document: WorkbenchDocumentV1,
): WorkbenchInspection {
  const group_references: string[] = [];
  const split_node_ids: string[] = [];
  let max_depth = 0;

  const visit = (node: WorkbenchNodeV1, depth: number): void => {
    max_depth = Math.max(max_depth, depth);
    if (node.kind === "group") {
      group_references.push(node.group_id);
      return;
    }
    split_node_ids.push(node.node_id);
    visit(node.first, depth + 1);
    visit(node.second, depth + 1);
  };

  visit(document.root, 1);

  return {
    group_references,
    split_node_ids,
    open_surface_references: Object.values(document.groups).flatMap(
      (group) => group.surface_ids,
    ),
    max_depth,
  };
}

export function createSeededRandom(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

export function pick<T>(values: readonly T[], random: () => number): T | undefined {
  if (values.length === 0) return undefined;
  return values[Math.floor(random() * values.length)];
}

export function cloneDocument(document: WorkbenchDocumentV1): WorkbenchDocumentV1 {
  return structuredClone(document);
}

export function countTrackedSurfaces(document: WorkbenchDocumentV1): number {
  return Object.keys(document.surfaces).length + document.recently_closed.length;
}

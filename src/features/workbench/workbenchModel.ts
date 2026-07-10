import type {
  WorkbenchCommandResult,
  WorkbenchDocumentV1,
  WorkbenchNodeV1,
  WorkbenchShellV1,
  WorkbenchSurfaceV1,
  WorkbenchValidationError,
  WorkbenchValidationResult,
} from "../../types";

export const MAX_WORKBENCH_DOCUMENT_BYTES = 2 * 1024 * 1024;
export const MAX_WORKBENCH_SURFACE_STATE_BYTES = 64 * 1024;
export const MAX_WORKBENCH_TREE_DEPTH = 64;
export const MAX_RECENTLY_CLOSED_SURFACES = 20;

export type WorkbenchCommand =
  | { type: "open_surface"; surface: WorkbenchSurfaceV1; group_id?: string; index?: number }
  | { type: "focus_surface"; surface_id: string }
  | { type: "close_surface"; surface_id: string }
  | { type: "reopen_closed_surface" }
  | {
      type: "split_group";
      group_id: string;
      new_group_id: string;
      node_id: string;
      direction: "horizontal" | "vertical";
      placement: "before" | "after";
    }
  | { type: "move_surface"; surface_id: string; group_id: string; index: number }
  | { type: "set_active_surface"; group_id: string; surface_id: string | null }
  | { type: "set_split_ratio"; node_id: string; ratio: number }
  | { type: "close_group"; group_id: string }
  | { type: "join_group"; source_group_id: string; target_group_id: string }
  | {
      type: "update_surface_state";
      surface_id: string;
      state_schema_version: number;
      state: unknown;
    }
  | { type: "update_shell"; patch: Partial<WorkbenchShellV1> };

const DEFAULT_SHELL: WorkbenchShellV1 = {
  left_sidebar_collapsed: false,
  left_sidebar_width: 240,
  right_sidebar_collapsed: false,
  right_sidebar_width: 240,
  bottom_terminal_open: false,
  bottom_terminal_height: 360,
};

function validationFailure(path: string, message: string): WorkbenchValidationResult {
  return { valid: false, errors: [{ path, message }] };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasOwn(value: object, key: PropertyKey): boolean {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function hasExactKeys(
  value: Record<string, unknown>,
  required: readonly string[],
  optional: readonly string[] = [],
): boolean {
  const allowed = new Set([...required, ...optional]);
  const keys = Object.keys(value);
  return required.every((key) => hasOwn(value, key)) && keys.every((key) => allowed.has(key));
}

function addError(errors: WorkbenchValidationError[], path: string, message: string): void {
  errors.push({ path, message });
}

function jsonByteLength(value: unknown): number | null {
  try {
    const json = JSON.stringify(value);
    return json === undefined ? null : new TextEncoder().encode(json).byteLength;
  } catch {
    return null;
  }
}

function validateJsonValue(
  value: unknown,
  path: string,
  errors: WorkbenchValidationError[],
  ancestors = new WeakSet<object>(),
): void {
  if (value === null || typeof value === "string" || typeof value === "boolean") return;
  if (typeof value === "number") {
    if (!Number.isFinite(value)) addError(errors, path, "must contain only finite JSON numbers");
    return;
  }
  if (typeof value !== "object") {
    addError(errors, path, "must be JSON-compatible");
    return;
  }
  if (ancestors.has(value)) {
    addError(errors, path, "must not contain cycles");
    return;
  }

  ancestors.add(value);
  if (Array.isArray(value)) {
    for (let index = 0; index < value.length; index += 1) {
      if (!hasOwn(value, index)) {
        addError(errors, `${path}[${index}]`, "must not be a sparse array entry");
      } else {
        validateJsonValue(value[index], `${path}[${index}]`, errors, ancestors);
      }
    }
  } else {
    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) {
      addError(errors, path, "must contain only plain JSON objects");
    } else {
      for (const [key, child] of Object.entries(value)) {
        validateJsonValue(child, `${path}.${key}`, errors, ancestors);
      }
    }
  }
  ancestors.delete(value);
}

function validateSurface(
  value: unknown,
  path: string,
  errors: WorkbenchValidationError[],
): string | null {
  if (!isRecord(value)) {
    addError(errors, path, "must be an object");
    return null;
  }
  if (!hasExactKeys(
    value,
    ["surface_id", "surface_type", "state_schema_version", "state"],
    ["resource_key"],
  )) {
    addError(errors, path, "must contain only V1 surface fields");
  }
  const surfaceId = value.surface_id;
  if (typeof surfaceId !== "string" || surfaceId.length === 0) {
    addError(errors, `${path}.surface_id`, "must be a non-empty string");
  }
  if (typeof value.surface_type !== "string" || value.surface_type.length === 0) {
    addError(errors, `${path}.surface_type`, "must be a non-empty string");
  }
  if (
    hasOwn(value, "resource_key") &&
    typeof value.resource_key !== "string"
  ) {
    addError(errors, `${path}.resource_key`, "must be a string when present");
  }
  if (!Number.isSafeInteger(value.state_schema_version) || (value.state_schema_version as number) < 0) {
    addError(errors, `${path}.state_schema_version`, "must be a non-negative safe integer");
  }
  try {
    validateJsonValue(value.state, `${path}.state`, errors);
  } catch {
    addError(errors, `${path}.state`, "is nested too deeply to validate safely");
  }
  const stateBytes = jsonByteLength(value.state);
  if (stateBytes === null) {
    addError(errors, `${path}.state`, "must be serializable JSON");
  } else if (stateBytes > MAX_WORKBENCH_SURFACE_STATE_BYTES) {
    addError(errors, `${path}.state`, "exceeds the 64 KiB UTF-8 limit");
  }
  return typeof surfaceId === "string" ? surfaceId : null;
}

function validateNode(
  value: unknown,
  depth: number,
  groupReferences: string[],
  splitIds: Set<string>,
  errors: WorkbenchValidationError[],
  ancestors: WeakSet<object>,
  path = "$.root",
): void {
  if (depth > MAX_WORKBENCH_TREE_DEPTH) {
    addError(errors, path, "exceeds the 64-node tree depth limit");
    return;
  }
  if (!isRecord(value)) {
    addError(errors, path, "must be a workbench node object");
    return;
  }
  if (ancestors.has(value)) {
    addError(errors, path, "split tree must be acyclic");
    return;
  }
  ancestors.add(value);

  if (value.kind === "group") {
    if (!hasExactKeys(value, ["kind", "group_id"])) {
      addError(errors, path, "must contain only V1 group-node fields");
    }
    if (typeof value.group_id !== "string" || value.group_id.length === 0) {
      addError(errors, `${path}.group_id`, "must be a non-empty string");
    } else {
      groupReferences.push(value.group_id);
    }
  } else if (value.kind === "split") {
    if (!hasExactKeys(value, ["kind", "node_id", "direction", "ratio", "first", "second"])) {
      addError(errors, path, "must contain only V1 split-node fields");
    }
    if (typeof value.node_id !== "string" || value.node_id.length === 0) {
      addError(errors, `${path}.node_id`, "must be a non-empty string");
    } else if (splitIds.has(value.node_id)) {
      addError(errors, `${path}.node_id`, "must be unique");
    } else {
      splitIds.add(value.node_id);
    }
    if (value.direction !== "horizontal" && value.direction !== "vertical") {
      addError(errors, `${path}.direction`, "must be horizontal or vertical");
    }
    if (
      typeof value.ratio !== "number" ||
      !Number.isFinite(value.ratio) ||
      value.ratio < 0.1 ||
      value.ratio > 0.9
    ) {
      addError(errors, `${path}.ratio`, "must be a finite number in 0.1..0.9");
    }
    validateNode(value.first, depth + 1, groupReferences, splitIds, errors, ancestors, `${path}.first`);
    validateNode(value.second, depth + 1, groupReferences, splitIds, errors, ancestors, `${path}.second`);
  } else {
    addError(errors, `${path}.kind`, "must be group or split");
  }

  ancestors.delete(value);
}

const DOCUMENT_KEYS = [
  "schema_version",
  "revision",
  "saved_at",
  "root",
  "groups",
  "surfaces",
  "active_group_id",
  "recently_closed",
  "shell",
] as const;

const SHELL_KEYS = [
  "left_sidebar_collapsed",
  "left_sidebar_width",
  "right_sidebar_collapsed",
  "right_sidebar_width",
  "bottom_terminal_open",
  "bottom_terminal_height",
] as const;

/** Creates the deterministic revision-zero document; empty-group Home is derived by the renderer. */
export function createDefaultWorkbenchDocument(): WorkbenchDocumentV1 {
  return {
    schema_version: 1,
    revision: 0,
    saved_at: "1970-01-01T00:00:00.000Z",
    root: { kind: "group", group_id: "group-1" },
    groups: {
      "group-1": {
        group_id: "group-1",
        surface_ids: [],
        active_surface_id: null,
      },
    },
    surfaces: {},
    active_group_id: "group-1",
    recently_closed: [],
    shell: { ...DEFAULT_SHELL },
  };
}

/** Validates an untrusted value without normalizing or cloning it. */
export function validateWorkbenchDocument(value: unknown): WorkbenchValidationResult {
  if (!isRecord(value)) return validationFailure("$", "must be an object");
  const errors: WorkbenchValidationError[] = [];
  if (!hasExactKeys(value, DOCUMENT_KEYS)) {
    addError(errors, "$", "must contain exactly the V1 document fields");
  }
  if (value.schema_version !== 1) addError(errors, "$.schema_version", "must equal 1");
  if (!Number.isSafeInteger(value.revision) || (value.revision as number) < 0) {
    addError(errors, "$.revision", "must be a non-negative safe integer");
  }
  if (typeof value.saved_at !== "string") addError(errors, "$.saved_at", "must be a string");

  const groupReferences: string[] = [];
  const splitIds = new Set<string>();
  validateNode(value.root, 1, groupReferences, splitIds, errors, new WeakSet<object>());

  const openSurfaceReferences: string[] = [];
  if (!isRecord(value.groups)) {
    addError(errors, "$.groups", "must be a record");
  } else {
    for (const [groupKey, groupValue] of Object.entries(value.groups)) {
      const path = `$.groups.${groupKey}`;
      if (!isRecord(groupValue)) {
        addError(errors, path, "must be an object");
        continue;
      }
      if (!hasExactKeys(groupValue, ["group_id", "surface_ids", "active_surface_id"])) {
        addError(errors, path, "must contain exactly the V1 group fields");
      }
      if (groupValue.group_id !== groupKey) {
        addError(errors, `${path}.group_id`, "must match its record key");
      }
      if (!Array.isArray(groupValue.surface_ids)) {
        addError(errors, `${path}.surface_ids`, "must be an array");
        continue;
      }
      const localIds = new Set<string>();
      for (const [index, surfaceId] of groupValue.surface_ids.entries()) {
        if (typeof surfaceId !== "string" || surfaceId.length === 0) {
          addError(errors, `${path}.surface_ids[${index}]`, "must be a non-empty string");
        } else {
          if (localIds.has(surfaceId)) {
            addError(errors, `${path}.surface_ids[${index}]`, "must not be duplicated in a group");
          }
          localIds.add(surfaceId);
          openSurfaceReferences.push(surfaceId);
        }
      }
      if (groupValue.surface_ids.length === 0) {
        if (groupValue.active_surface_id !== null) {
          addError(errors, `${path}.active_surface_id`, "must be null for an empty group");
        }
      } else if (
        typeof groupValue.active_surface_id !== "string" ||
        !localIds.has(groupValue.active_surface_id)
      ) {
        addError(errors, `${path}.active_surface_id`, "must reference a tab in the group");
      }
    }

    const referenceCounts = new Map<string, number>();
    for (const groupId of groupReferences) {
      referenceCounts.set(groupId, (referenceCounts.get(groupId) ?? 0) + 1);
    }
    for (const groupId of Object.keys(value.groups)) {
      if (referenceCounts.get(groupId) !== 1) {
        addError(errors, `$.groups.${groupId}`, "must be referenced exactly once by the tree");
      }
    }
    for (const groupId of groupReferences) {
      if (!hasOwn(value.groups, groupId)) {
        addError(errors, "$.root", `references missing group ${groupId}`);
      }
    }
  }

  if (!isRecord(value.surfaces)) {
    addError(errors, "$.surfaces", "must be a record");
  } else {
    for (const [surfaceKey, surfaceValue] of Object.entries(value.surfaces)) {
      const surfaceId = validateSurface(surfaceValue, `$.surfaces.${surfaceKey}`, errors);
      if (surfaceId !== surfaceKey) {
        addError(errors, `$.surfaces.${surfaceKey}.surface_id`, "must match its record key");
      }
    }
    const referenceCounts = new Map<string, number>();
    for (const surfaceId of openSurfaceReferences) {
      referenceCounts.set(surfaceId, (referenceCounts.get(surfaceId) ?? 0) + 1);
    }
    for (const surfaceId of Object.keys(value.surfaces)) {
      if (referenceCounts.get(surfaceId) !== 1) {
        addError(errors, `$.surfaces.${surfaceId}`, "must be referenced exactly once by a group");
      }
    }
    for (const surfaceId of openSurfaceReferences) {
      if (!hasOwn(value.surfaces, surfaceId)) {
        addError(errors, "$.groups", `references missing surface ${surfaceId}`);
      }
    }
  }

  if (
    typeof value.active_group_id !== "string" ||
    !isRecord(value.groups) ||
    !hasOwn(value.groups, value.active_group_id) ||
    !groupReferences.includes(value.active_group_id)
  ) {
    addError(errors, "$.active_group_id", "must reference a tree group");
  }

  if (!Array.isArray(value.recently_closed)) {
    addError(errors, "$.recently_closed", "must be an array");
  } else {
    if (value.recently_closed.length > MAX_RECENTLY_CLOSED_SURFACES) {
      addError(errors, "$.recently_closed", "must contain at most 20 surfaces");
    }
    for (let index = 0; index < value.recently_closed.length; index += 1) {
      const path = `$.recently_closed[${index}]`;
      if (!hasOwn(value.recently_closed, index)) {
        addError(errors, path, "must not be a sparse array entry");
        continue;
      }
      const closed = value.recently_closed[index];
      if (!isRecord(closed)) {
        addError(errors, path, "must be an object");
        continue;
      }
      if (!hasExactKeys(closed, ["surface", "previous_group_id", "previous_index"])) {
        addError(errors, path, "must contain exactly the V1 closed-surface fields");
      }
      validateSurface(closed.surface, `${path}.surface`, errors);
      if (typeof closed.previous_group_id !== "string" || closed.previous_group_id.length === 0) {
        addError(errors, `${path}.previous_group_id`, "must be a non-empty string");
      }
      if (!Number.isSafeInteger(closed.previous_index) || (closed.previous_index as number) < 0) {
        addError(errors, `${path}.previous_index`, "must be a non-negative safe integer");
      }
    }
  }

  if (!isRecord(value.shell) || !hasExactKeys(value.shell, SHELL_KEYS)) {
    addError(errors, "$.shell", "must contain exactly the V1 shell fields");
  } else {
    for (const key of [
      "left_sidebar_collapsed",
      "right_sidebar_collapsed",
      "bottom_terminal_open",
    ] as const) {
      if (typeof value.shell[key] !== "boolean") {
        addError(errors, `$.shell.${key}`, "must be a boolean");
      }
    }
    for (const key of [
      "left_sidebar_width",
      "right_sidebar_width",
      "bottom_terminal_height",
    ] as const) {
      const size = value.shell[key];
      if (typeof size !== "number" || !Number.isFinite(size) || size < 0) {
        addError(errors, `$.shell.${key}`, "must be a finite non-negative number");
      }
    }
  }

  const documentBytes = jsonByteLength(value);
  if (documentBytes === null) {
    addError(errors, "$", "must be serializable JSON");
  } else if (documentBytes > MAX_WORKBENCH_DOCUMENT_BYTES) {
    addError(errors, "$", "exceeds the 2 MiB UTF-8 document limit");
  }

  return errors.length === 0
    ? { valid: true, document: value as WorkbenchDocumentV1 }
    : { valid: false, errors };
}

function rejected(
  document: WorkbenchDocumentV1,
  errors: WorkbenchValidationError[],
): WorkbenchCommandResult {
  return { accepted: false, document, errors };
}

function commandRejected(
  document: WorkbenchDocumentV1,
  message: string,
  path = "$.command",
): WorkbenchCommandResult {
  return rejected(document, [{ path, message }]);
}

function acceptedCandidate(
  original: WorkbenchDocumentV1,
  candidate: WorkbenchDocumentV1,
): WorkbenchCommandResult {
  const validation = validateWorkbenchDocument(candidate);
  return validation.valid
    ? { accepted: true, document: candidate }
    : rejected(original, validation.errors);
}

function groupContainingSurface(
  document: WorkbenchDocumentV1,
  surfaceId: string,
): { groupId: string; index: number } | null {
  for (const [groupId, group] of Object.entries(document.groups)) {
    const index = group.surface_ids.indexOf(surfaceId);
    if (index >= 0) return { groupId, index };
  }
  return null;
}

function nextActiveSurface(surfaceIds: string[], removedIndex: number): string | null {
  if (surfaceIds.length === 0) return null;
  return surfaceIds[Math.min(removedIndex, surfaceIds.length - 1)];
}

function reopenedSurfaceId(document: WorkbenchDocumentV1, closedId: string): string {
  const base = `${closedId}-reopened`;
  if (!hasOwn(document.surfaces, base)) return base;
  let suffix = 2;
  while (hasOwn(document.surfaces, `${base}-${suffix}`)) suffix += 1;
  return `${base}-${suffix}`;
}

function splitNodeExists(node: WorkbenchNodeV1, nodeId: string): boolean {
  if (node.kind === "group") return false;
  return node.node_id === nodeId || splitNodeExists(node.first, nodeId) || splitNodeExists(node.second, nodeId);
}

function replaceGroupLeaf(
  node: WorkbenchNodeV1,
  groupId: string,
  replacement: WorkbenchNodeV1,
): { node: WorkbenchNodeV1; replaced: boolean } {
  if (node.kind === "group") {
    return node.group_id === groupId
      ? { node: replacement, replaced: true }
      : { node, replaced: false };
  }
  const first = replaceGroupLeaf(node.first, groupId, replacement);
  if (first.replaced) return { node: { ...node, first: first.node }, replaced: true };
  const second = replaceGroupLeaf(node.second, groupId, replacement);
  return second.replaced
    ? { node: { ...node, second: second.node }, replaced: true }
    : { node, replaced: false };
}

function replaceSplitRatio(
  node: WorkbenchNodeV1,
  nodeId: string,
  ratio: number,
): { node: WorkbenchNodeV1; replaced: boolean } {
  if (node.kind === "group") return { node, replaced: false };
  if (node.node_id === nodeId) return { node: { ...node, ratio }, replaced: true };
  const first = replaceSplitRatio(node.first, nodeId, ratio);
  if (first.replaced) return { node: { ...node, first: first.node }, replaced: true };
  const second = replaceSplitRatio(node.second, nodeId, ratio);
  return second.replaced
    ? { node: { ...node, second: second.node }, replaced: true }
    : { node, replaced: false };
}

function removeGroupLeaf(
  node: WorkbenchNodeV1,
  groupId: string,
): { node: WorkbenchNodeV1 | null; removed: boolean } {
  if (node.kind === "group") {
    return node.group_id === groupId
      ? { node: null, removed: true }
      : { node, removed: false };
  }
  const first = removeGroupLeaf(node.first, groupId);
  if (first.removed) {
    return first.node === null
      ? { node: node.second, removed: true }
      : { node: { ...node, first: first.node }, removed: true };
  }
  const second = removeGroupLeaf(node.second, groupId);
  if (second.removed) {
    return second.node === null
      ? { node: node.first, removed: true }
      : { node: { ...node, second: second.node }, removed: true };
  }
  return { node, removed: false };
}

function leftmostGroupId(node: WorkbenchNodeV1): string {
  return node.kind === "group" ? node.group_id : leftmostGroupId(node.first);
}

function siblingSubtreeForGroup(
  node: WorkbenchNodeV1,
  groupId: string,
): WorkbenchNodeV1 | null {
  if (node.kind === "group") return null;
  if (node.first.kind === "group" && node.first.group_id === groupId) return node.second;
  if (node.second.kind === "group" && node.second.group_id === groupId) return node.first;
  return siblingSubtreeForGroup(node.first, groupId)
    ?? siblingSubtreeForGroup(node.second, groupId);
}

type WorkbenchRectangle = {
  left: number;
  top: number;
  right: number;
  bottom: number;
};

function collectGroupRectangles(
  node: WorkbenchNodeV1,
  rectangle: WorkbenchRectangle,
  rectangles: Map<string, WorkbenchRectangle>,
): void {
  if (node.kind === "group") {
    rectangles.set(node.group_id, rectangle);
    return;
  }
  if (node.direction === "horizontal") {
    const boundary = rectangle.left + ((rectangle.right - rectangle.left) * node.ratio);
    collectGroupRectangles(node.first, { ...rectangle, right: boundary }, rectangles);
    collectGroupRectangles(node.second, { ...rectangle, left: boundary }, rectangles);
    return;
  }
  const boundary = rectangle.top + ((rectangle.bottom - rectangle.top) * node.ratio);
  collectGroupRectangles(node.first, { ...rectangle, bottom: boundary }, rectangles);
  collectGroupRectangles(node.second, { ...rectangle, top: boundary }, rectangles);
}

function groupsAreAdjacent(root: WorkbenchNodeV1, firstId: string, secondId: string): boolean {
  const rectangles = new Map<string, WorkbenchRectangle>();
  collectGroupRectangles(root, { left: 0, top: 0, right: 1, bottom: 1 }, rectangles);
  const first = rectangles.get(firstId);
  const second = rectangles.get(secondId);
  if (!first || !second) return false;
  const epsilon = 1e-10;
  const verticalOverlap = Math.min(first.bottom, second.bottom) - Math.max(first.top, second.top);
  const horizontalOverlap = Math.min(first.right, second.right) - Math.max(first.left, second.left);
  const touchesVertically = Math.abs(first.right - second.left) <= epsilon
    || Math.abs(second.right - first.left) <= epsilon;
  const touchesHorizontally = Math.abs(first.bottom - second.top) <= epsilon
    || Math.abs(second.bottom - first.top) <= epsilon;
  return (touchesVertically && verticalOverlap > epsilon)
    || (touchesHorizontally && horizontalOverlap > epsilon);
}

function closeGroupSurfaces(
  document: WorkbenchDocumentV1,
  groupId: string,
): { surfaces: Record<string, WorkbenchSurfaceV1>; recentlyClosed: WorkbenchDocumentV1["recently_closed"] } {
  const group = document.groups[groupId];
  const surfaces = { ...document.surfaces };
  let recentlyClosed = [...document.recently_closed];
  group.surface_ids.forEach((surfaceId, index) => {
    const surface = document.surfaces[surfaceId];
    delete surfaces[surfaceId];
    recentlyClosed = [
      { surface, previous_group_id: groupId, previous_index: index },
      ...recentlyClosed,
    ].slice(0, MAX_RECENTLY_CLOSED_SURFACES);
  });
  return { surfaces, recentlyClosed };
}

/** Applies a workbench command without mutating the input document. */
export function applyWorkbenchCommand(
  document: WorkbenchDocumentV1,
  command: WorkbenchCommand,
): WorkbenchCommandResult {
  const validation = validateWorkbenchDocument(document);
  if (!validation.valid) return rejected(document, validation.errors);

  switch (command.type) {
    case "open_surface": {
      const groupId = command.group_id ?? document.active_group_id;
      const group = document.groups[groupId];
      if (!group) return commandRejected(document, "target group does not exist", "$.command.group_id");
      if (hasOwn(document.surfaces, command.surface.surface_id)) {
        return commandRejected(document, "surface_id is already open", "$.command.surface.surface_id");
      }
      const index = command.index ?? group.surface_ids.length;
      if (!Number.isInteger(index) || index < 0 || index > group.surface_ids.length) {
        return commandRejected(document, "index is outside the target group", "$.command.index");
      }
      const surfaceIds = [...group.surface_ids];
      surfaceIds.splice(index, 0, command.surface.surface_id);
      return acceptedCandidate(document, {
        ...document,
        groups: {
          ...document.groups,
          [groupId]: {
            ...group,
            surface_ids: surfaceIds,
            active_surface_id: command.surface.surface_id,
          },
        },
        surfaces: {
          ...document.surfaces,
          [command.surface.surface_id]: command.surface,
        },
        active_group_id: groupId,
      });
    }

    case "focus_surface": {
      const location = groupContainingSurface(document, command.surface_id);
      if (!location) {
        return commandRejected(document, "surface does not exist", "$.command.surface_id");
      }
      const group = document.groups[location.groupId];
      return acceptedCandidate(document, {
        ...document,
        groups: {
          ...document.groups,
          [location.groupId]: { ...group, active_surface_id: command.surface_id },
        },
        active_group_id: location.groupId,
      });
    }

    case "close_surface": {
      const location = groupContainingSurface(document, command.surface_id);
      const surface = document.surfaces[command.surface_id];
      if (!location || !surface) {
        return commandRejected(document, "surface does not exist", "$.command.surface_id");
      }
      const group = document.groups[location.groupId];
      const surfaceIds = group.surface_ids.filter((surfaceId) => surfaceId !== command.surface_id);
      const surfaces = { ...document.surfaces };
      delete surfaces[command.surface_id];
      const activeSurfaceId = group.active_surface_id === command.surface_id
        ? nextActiveSurface(surfaceIds, location.index)
        : group.active_surface_id;
      return acceptedCandidate(document, {
        ...document,
        groups: {
          ...document.groups,
          [location.groupId]: {
            ...group,
            surface_ids: surfaceIds,
            active_surface_id: activeSurfaceId,
          },
        },
        surfaces,
        recently_closed: [
          {
            surface,
            previous_group_id: location.groupId,
            previous_index: location.index,
          },
          ...document.recently_closed,
        ].slice(0, MAX_RECENTLY_CLOSED_SURFACES),
      });
    }

    case "reopen_closed_surface": {
      const closed = document.recently_closed[0];
      if (!closed) return commandRejected(document, "there is no recently closed surface");
      const groupId = hasOwn(document.groups, closed.previous_group_id)
        ? closed.previous_group_id
        : document.active_group_id;
      const group = document.groups[groupId];
      const surfaceId = hasOwn(document.surfaces, closed.surface.surface_id)
        ? reopenedSurfaceId(document, closed.surface.surface_id)
        : closed.surface.surface_id;
      const surface = surfaceId === closed.surface.surface_id
        ? closed.surface
        : { ...closed.surface, surface_id: surfaceId };
      const index = Math.min(closed.previous_index, group.surface_ids.length);
      const surfaceIds = [...group.surface_ids];
      surfaceIds.splice(index, 0, surfaceId);
      return acceptedCandidate(document, {
        ...document,
        groups: {
          ...document.groups,
          [groupId]: {
            ...group,
            surface_ids: surfaceIds,
            active_surface_id: surfaceId,
          },
        },
        surfaces: { ...document.surfaces, [surfaceId]: surface },
        active_group_id: groupId,
        recently_closed: document.recently_closed.slice(1),
      });
    }

    case "split_group": {
      if (!hasOwn(document.groups, command.group_id)) {
        return commandRejected(document, "group does not exist", "$.command.group_id");
      }
      if (hasOwn(document.groups, command.new_group_id)) {
        return commandRejected(document, "new_group_id already exists", "$.command.new_group_id");
      }
      if (splitNodeExists(document.root, command.node_id)) {
        return commandRejected(document, "node_id already exists", "$.command.node_id");
      }
      if (
        command.new_group_id.length === 0 ||
        command.node_id.length === 0 ||
        (command.direction !== "horizontal" && command.direction !== "vertical") ||
        (command.placement !== "before" && command.placement !== "after")
      ) {
        return commandRejected(document, "split command fields are invalid");
      }
      const existingLeaf: WorkbenchNodeV1 = { kind: "group", group_id: command.group_id };
      const newLeaf: WorkbenchNodeV1 = { kind: "group", group_id: command.new_group_id };
      const split: WorkbenchNodeV1 = {
        kind: "split",
        node_id: command.node_id,
        direction: command.direction,
        ratio: 0.5,
        first: command.placement === "before" ? newLeaf : existingLeaf,
        second: command.placement === "before" ? existingLeaf : newLeaf,
      };
      const replaced = replaceGroupLeaf(document.root, command.group_id, split);
      if (!replaced.replaced) return commandRejected(document, "group is not present in the tree");
      return acceptedCandidate(document, {
        ...document,
        root: replaced.node,
        groups: {
          ...document.groups,
          [command.new_group_id]: {
            group_id: command.new_group_id,
            surface_ids: [],
            active_surface_id: null,
          },
        },
        active_group_id: command.new_group_id,
      });
    }

    case "move_surface": {
      const location = groupContainingSurface(document, command.surface_id);
      const target = document.groups[command.group_id];
      if (!location) return commandRejected(document, "surface does not exist", "$.command.surface_id");
      if (!target) return commandRejected(document, "target group does not exist", "$.command.group_id");
      if (!Number.isInteger(command.index) || command.index < 0 || command.index > target.surface_ids.length) {
        return commandRejected(document, "index is outside the target group", "$.command.index");
      }
      const source = document.groups[location.groupId];
      if (location.groupId === command.group_id) {
        const surfaceIds = source.surface_ids.filter((surfaceId) => surfaceId !== command.surface_id);
        surfaceIds.splice(Math.min(command.index, surfaceIds.length), 0, command.surface_id);
        return acceptedCandidate(document, {
          ...document,
          groups: {
            ...document.groups,
            [location.groupId]: {
              ...source,
              surface_ids: surfaceIds,
              active_surface_id: command.surface_id,
            },
          },
          active_group_id: location.groupId,
        });
      }

      const sourceSurfaceIds = source.surface_ids.filter((surfaceId) => surfaceId !== command.surface_id);
      const targetSurfaceIds = [...target.surface_ids];
      targetSurfaceIds.splice(command.index, 0, command.surface_id);
      return acceptedCandidate(document, {
        ...document,
        groups: {
          ...document.groups,
          [location.groupId]: {
            ...source,
            surface_ids: sourceSurfaceIds,
            active_surface_id: source.active_surface_id === command.surface_id
              ? nextActiveSurface(sourceSurfaceIds, location.index)
              : source.active_surface_id,
          },
          [command.group_id]: {
            ...target,
            surface_ids: targetSurfaceIds,
            active_surface_id: command.surface_id,
          },
        },
        active_group_id: command.group_id,
      });
    }

    case "set_active_surface": {
      const group = document.groups[command.group_id];
      if (!group) return commandRejected(document, "group does not exist", "$.command.group_id");
      if (
        (command.surface_id === null && group.surface_ids.length > 0) ||
        (command.surface_id !== null && !group.surface_ids.includes(command.surface_id))
      ) {
        return commandRejected(document, "active surface must belong to the group", "$.command.surface_id");
      }
      return acceptedCandidate(document, {
        ...document,
        groups: {
          ...document.groups,
          [command.group_id]: { ...group, active_surface_id: command.surface_id },
        },
        active_group_id: command.group_id,
      });
    }

    case "set_split_ratio": {
      if (!Number.isFinite(command.ratio)) {
        return commandRejected(document, "ratio must be finite", "$.command.ratio");
      }
      const ratio = Math.max(0.1, Math.min(0.9, command.ratio));
      const replaced = replaceSplitRatio(document.root, command.node_id, ratio);
      if (!replaced.replaced) return commandRejected(document, "split node does not exist", "$.command.node_id");
      return acceptedCandidate(document, { ...document, root: replaced.node });
    }

    case "close_group": {
      const group = document.groups[command.group_id];
      if (!group) return commandRejected(document, "group does not exist", "$.command.group_id");
      const closed = closeGroupSurfaces(document, command.group_id);
      if (Object.keys(document.groups).length === 1) {
        return acceptedCandidate(document, {
          ...document,
          groups: {
            ...document.groups,
            [command.group_id]: {
              ...group,
              surface_ids: [],
              active_surface_id: null,
            },
          },
          surfaces: closed.surfaces,
          recently_closed: closed.recentlyClosed,
        });
      }
      const siblingSubtree = siblingSubtreeForGroup(document.root, command.group_id);
      if (!siblingSubtree) {
        return commandRejected(document, "group has no sibling subtree");
      }
      const removed = removeGroupLeaf(document.root, command.group_id);
      if (!removed.removed || removed.node === null) {
        return commandRejected(document, "group is not present in the tree");
      }
      const groups = { ...document.groups };
      delete groups[command.group_id];
      return acceptedCandidate(document, {
        ...document,
        root: removed.node,
        groups,
        surfaces: closed.surfaces,
        active_group_id: leftmostGroupId(siblingSubtree),
        recently_closed: closed.recentlyClosed,
      });
    }

    case "join_group": {
      if (command.source_group_id === command.target_group_id) {
        return commandRejected(document, "source and target groups must differ");
      }
      const source = document.groups[command.source_group_id];
      const target = document.groups[command.target_group_id];
      if (!source) return commandRejected(document, "source group does not exist", "$.command.source_group_id");
      if (!target) return commandRejected(document, "target group does not exist", "$.command.target_group_id");
      if (!groupsAreAdjacent(document.root, command.source_group_id, command.target_group_id)) {
        return commandRejected(document, "source and target groups must be adjacent");
      }
      const removed = removeGroupLeaf(document.root, command.source_group_id);
      if (!removed.removed || removed.node === null) {
        return commandRejected(document, "source group is not present in the tree");
      }
      const groups = { ...document.groups };
      delete groups[command.source_group_id];
      groups[command.target_group_id] = {
        ...target,
        surface_ids: [...target.surface_ids, ...source.surface_ids],
        active_surface_id: source.active_surface_id ?? target.active_surface_id,
      };
      return acceptedCandidate(document, {
        ...document,
        root: removed.node,
        groups,
        active_group_id: command.target_group_id,
      });
    }

    case "update_surface_state": {
      const surface = document.surfaces[command.surface_id];
      if (!surface) return commandRejected(document, "surface does not exist", "$.command.surface_id");
      return acceptedCandidate(document, {
        ...document,
        surfaces: {
          ...document.surfaces,
          [command.surface_id]: {
            ...surface,
            state_schema_version: command.state_schema_version,
            state: command.state,
          },
        },
      });
    }

    case "update_shell": {
      if (!isRecord(command.patch)) {
        return commandRejected(document, "shell patch must be an object", "$.command.patch");
      }
      return acceptedCandidate(document, {
        ...document,
        shell: { ...document.shell, ...command.patch },
      });
    }

    default:
      return commandRejected(document, "unknown command type");
  }
}

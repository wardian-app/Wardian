import type {
  FilesComparisonBaseline,
  FilesSurfaceStateV1,
  FilesSurfaceStateV2,
  SurfaceRestoreResult,
  WorkbenchSurfaceV1,
} from "../../types";
import type { WorkbenchCommand } from "../workbench/workbenchModel";

const V1_KEYS = Object.freeze([
  "mode",
  "optional_checkpoint_id",
  "resource_kind",
  "review_drawer_open",
  "selected_version_id",
  "transient_preview",
] as const);

const V2_KEYS = Object.freeze([
  "comparison_baseline",
  "comparison_layout_preference",
  "comparison_open",
  "optional_checkpoint_id",
  "presentation",
  "resource_kind",
  "review_drawer_open",
  "selected_version_id",
  "transient_preview",
] as const);

export type FilesPresentationCapabilities = {
  default_presentation: "rendered" | "editor";
  rendered: boolean;
  editor: boolean;
  baseline_available: boolean;
};

export type FilesComparisonContentKind = "text" | "binary";
export type FilesEffectiveComparisonLayout =
  | "side_by_side"
  | "unified"
  | "stacked";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(value: Record<string, unknown>, expected: readonly string[]): boolean {
  const keys = Object.keys(value).sort();
  return keys.length === expected.length && keys.every((key, index) => key === expected[index]);
}

function nullableNonEmptyString(value: unknown): value is string | null {
  return value === null || (typeof value === "string" && value.trim().length > 0);
}

function validBaseline(value: unknown): value is FilesComparisonBaseline | null {
  if (value === null) return true;
  if (!isRecord(value) || typeof value.kind !== "string") return false;
  switch (value.kind) {
    case "saved_file":
      return hasExactKeys(value, ["kind"]);
    case "prompt_checkpoint":
      return hasExactKeys(value, ["checkpoint_id", "kind"])
        && nullableNonEmptyString(value.checkpoint_id)
        && value.checkpoint_id !== null;
    case "presented_version":
    case "previous_presented_version":
      return hasExactKeys(value, ["kind", "version_id"])
        && nullableNonEmptyString(value.version_id)
        && value.version_id !== null;
    default:
      return false;
  }
}

export function isFilesSurfaceStateV1(value: unknown): value is FilesSurfaceStateV1 {
  if (!isRecord(value) || !hasExactKeys(value, V1_KEYS)) return false;
  return (value.resource_kind === "file" || value.resource_kind === "artifact")
    && (value.mode === "preview" || value.mode === "changes" || value.mode === "draft")
    && typeof value.transient_preview === "boolean"
    && typeof value.review_drawer_open === "boolean"
    && nullableNonEmptyString(value.selected_version_id)
    && nullableNonEmptyString(value.optional_checkpoint_id);
}

export function isFilesSurfaceStateV2(value: unknown): value is FilesSurfaceStateV2 {
  if (!isRecord(value) || !hasExactKeys(value, V2_KEYS)) return false;
  const baselineValid = validBaseline(value.comparison_baseline);
  return (value.resource_kind === "file" || value.resource_kind === "artifact")
    && typeof value.transient_preview === "boolean"
    && (value.presentation === "rendered" || value.presentation === "editor")
    && typeof value.comparison_open === "boolean"
    && (
      value.comparison_layout_preference === "auto"
      || value.comparison_layout_preference === "unified"
      || value.comparison_layout_preference === "side_by_side"
    )
    && baselineValid
    && (value.comparison_open !== true || value.comparison_baseline !== null)
    && typeof value.review_drawer_open === "boolean"
    && nullableNonEmptyString(value.selected_version_id)
    && nullableNonEmptyString(value.optional_checkpoint_id);
}

function legacyBaseline(state: FilesSurfaceStateV1): FilesComparisonBaseline | null {
  if (state.optional_checkpoint_id !== null) {
    return { kind: "prompt_checkpoint", checkpoint_id: state.optional_checkpoint_id };
  }
  if (state.resource_kind === "artifact" && state.selected_version_id !== null) {
    return { kind: "presented_version", version_id: state.selected_version_id };
  }
  return null;
}

/** Migrates the byte-free V1 intent before descriptor/baseline normalization. */
export function migrateFilesSurfaceStateV1(state: FilesSurfaceStateV1): FilesSurfaceStateV2 {
  const comparisonBaseline = legacyBaseline(state);
  return {
    resource_kind: state.resource_kind,
    transient_preview: state.transient_preview,
    presentation: state.mode === "preview" ? "rendered" : "editor",
    comparison_open: state.mode === "changes" && comparisonBaseline !== null,
    comparison_layout_preference: "auto",
    comparison_baseline: comparisonBaseline,
    review_drawer_open: state.review_drawer_open,
    selected_version_id: state.selected_version_id,
    optional_checkpoint_id: state.optional_checkpoint_id,
  };
}

export function restoreFilesSurfaceState(
  value: unknown,
  version: number,
): SurfaceRestoreResult<FilesSurfaceStateV2> {
  if (version === 1) {
    return isFilesSurfaceStateV1(value)
      ? { ok: true, state: migrateFilesSurfaceStateV1(value) }
      : { ok: false, error: "files state is malformed" };
  }
  if (version === 2) {
    return isFilesSurfaceStateV2(value)
      ? { ok: true, state: { ...value } }
      : { ok: false, error: "files state is malformed" };
  }
  return { ok: false, error: `unsupported files state version ${version}` };
}

/** Idempotent schema-only migration; descriptor normalization happens after resource discovery. */
export function filesSurfaceMigrationCommands(
  document: {
    readonly surfaces: Readonly<Record<string, Readonly<WorkbenchSurfaceV1>>>;
  },
): WorkbenchCommand[] {
  const commands: WorkbenchCommand[] = [];
  for (const surface of Object.values(document.surfaces)) {
    if (surface.surface_type !== "files" || surface.state_schema_version !== 1) continue;
    const restored = restoreFilesSurfaceState(surface.state, 1);
    if (!restored.ok) continue;
    commands.push({
      type: "update_surface_state",
      surface_id: surface.surface_id,
      state_schema_version: 2,
      state: restored.state,
    });
  }
  return commands;
}

/** Resolves persisted presentation intent after renderer and baseline discovery. */
export function normalizeFilesSurfaceState(
  state: FilesSurfaceStateV2,
  capabilities: FilesPresentationCapabilities,
): FilesSurfaceStateV2 {
  let presentation = state.presentation;
  if (presentation === "editor" && !capabilities.editor) {
    presentation = capabilities.rendered ? "rendered" : capabilities.default_presentation;
  } else if (presentation === "rendered" && !capabilities.rendered) {
    presentation = capabilities.editor ? "editor" : capabilities.default_presentation;
  }
  const baselineAvailable = state.comparison_baseline !== null && capabilities.baseline_available;
  return {
    ...state,
    presentation,
    comparison_open: state.comparison_open && baselineAvailable,
    comparison_baseline: baselineAvailable ? state.comparison_baseline : null,
  };
}

export function resolveFilesComparisonLayout(
  preference: FilesSurfaceStateV2["comparison_layout_preference"],
  contentWidth: number,
  contentKind: FilesComparisonContentKind,
): FilesEffectiveComparisonLayout {
  if (contentKind === "binary") {
    if (preference === "unified") return "stacked";
    if (preference === "side_by_side" && contentWidth >= 560) return "side_by_side";
    return preference === "auto" && contentWidth >= 720 ? "side_by_side" : "stacked";
  }
  if (preference === "side_by_side" && contentWidth >= 560) return "side_by_side";
  if (preference === "unified") return "unified";
  return contentWidth >= 720 ? "side_by_side" : "unified";
}

/** Compatibility projection removed with the legacy Files mode bar. */
export function legacyFilesMode(state: FilesSurfaceStateV2): FilesSurfaceStateV1["mode"] {
  if (state.comparison_open) return "changes";
  return state.presentation === "rendered" ? "preview" : "draft";
}

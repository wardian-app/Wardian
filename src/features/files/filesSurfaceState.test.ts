import { describe, expect, it } from "vitest";

import type { FilesSurfaceStateV1, FilesSurfaceStateV2 } from "../../types";
import {
  migrateFilesSurfaceStateV1,
  filesSurfaceMigrationCommands,
  normalizeFilesSurfaceState,
  resolveFilesComparisonLayout,
  restoreFilesSurfaceState,
} from "./filesSurfaceState";
import { makeSingleGroupDocument, makeSurface } from "../workbench/workbenchTestUtils";

const v1 = (mode: FilesSurfaceStateV1["mode"]): FilesSurfaceStateV1 => ({
  resource_kind: "file",
  mode,
  transient_preview: true,
  review_drawer_open: true,
  selected_version_id: null,
  optional_checkpoint_id: null,
});

const v2 = (): FilesSurfaceStateV2 => ({
  resource_kind: "file",
  transient_preview: true,
  presentation: "editor",
  comparison_open: false,
  comparison_layout_preference: "auto",
  comparison_baseline: { kind: "saved_file" },
  review_drawer_open: false,
  selected_version_id: null,
  optional_checkpoint_id: null,
});

describe("Files surface state V2", () => {
  it("strictly round-trips V2 and rejects malformed or extra fields", () => {
    expect(restoreFilesSurfaceState(v2(), 2)).toEqual({ ok: true, state: v2() });
    expect(restoreFilesSurfaceState({ ...v2(), extra: true }, 2)).toEqual({
      ok: false,
      error: "files state is malformed",
    });
    expect(restoreFilesSurfaceState({
      ...v2(),
      comparison_baseline: { kind: "prompt_checkpoint", checkpoint_id: "" },
    }, 2)).toEqual({ ok: false, error: "files state is malformed" });
    expect(restoreFilesSurfaceState({ ...v2(), presentation: "preview" }, 2)).toEqual({
      ok: false,
      error: "files state is malformed",
    });
    expect(restoreFilesSurfaceState({
      ...v2(),
      comparison_open: true,
      comparison_baseline: null,
    }, 2)).toEqual({ ok: false, error: "files state is malformed" });
  });

  it("migrates preview, changes, and byte-free draft intent deterministically", () => {
    expect(migrateFilesSurfaceStateV1(v1("preview"))).toMatchObject({
      presentation: "rendered",
      comparison_open: false,
      comparison_baseline: null,
      comparison_layout_preference: "auto",
    });
    expect(migrateFilesSurfaceStateV1({
      ...v1("changes"),
      optional_checkpoint_id: "checkpoint-1",
    })).toMatchObject({
      presentation: "editor",
      comparison_open: true,
      comparison_baseline: { kind: "prompt_checkpoint", checkpoint_id: "checkpoint-1" },
    });
    expect(migrateFilesSurfaceStateV1({
      ...v1("changes"),
      resource_kind: "artifact",
      selected_version_id: "version-1",
    })).toMatchObject({
      presentation: "editor",
      comparison_open: true,
      comparison_baseline: { kind: "presented_version", version_id: "version-1" },
    });
    expect(migrateFilesSurfaceStateV1(v1("changes"))).toMatchObject({
      presentation: "editor",
      comparison_open: false,
      comparison_baseline: null,
    });
    expect(migrateFilesSurfaceStateV1(v1("draft"))).toEqual({
      resource_kind: "file",
      transient_preview: true,
      presentation: "editor",
      comparison_open: false,
      comparison_layout_preference: "auto",
      comparison_baseline: null,
      review_drawer_open: true,
      selected_version_id: null,
      optional_checkpoint_id: null,
    });
    expect(restoreFilesSurfaceState({ ...v1("draft"), buffer: "forbidden" }, 1))
      .toEqual({ ok: false, error: "files state is malformed" });
  });

  it("normalizes presentation and unavailable baselines after discovery", () => {
    expect(normalizeFilesSurfaceState(
      { ...v2(), presentation: "rendered" },
      { default_presentation: "editor", rendered: false, editor: true, baseline_available: true },
    )).toMatchObject({ presentation: "editor" });
    expect(normalizeFilesSurfaceState(
      { ...v2(), presentation: "editor" },
      { default_presentation: "rendered", rendered: true, editor: false, baseline_available: true },
    )).toMatchObject({ presentation: "rendered" });
    expect(normalizeFilesSurfaceState(
      { ...v2(), comparison_open: true },
      { default_presentation: "editor", rendered: false, editor: true, baseline_available: false },
    )).toMatchObject({ comparison_open: false, comparison_baseline: null });
  });

  it("preserves layout preference while degrading the effective comparison layout", () => {
    expect(resolveFilesComparisonLayout("side_by_side", 900, "text"))
      .toBe("side_by_side");
    expect(resolveFilesComparisonLayout("side_by_side", 650, "text"))
      .toBe("side_by_side");
    expect(resolveFilesComparisonLayout("side_by_side", 500, "text"))
      .toBe("unified");
    expect(resolveFilesComparisonLayout("auto", 650, "binary"))
      .toBe("stacked");
    expect(v2().comparison_layout_preference).toBe("auto");
  });

  it("creates idempotent Workbench commands for every persisted V1 Files surface", () => {
    const document = makeSingleGroupDocument([
      makeSurface("legacy", { surface_type: "files", state: v1("draft") }),
      makeSurface("current", {
        surface_type: "files",
        state_schema_version: 2,
        state: v2(),
      }),
      makeSurface("other", { surface_type: "library", state: {} }),
    ]);
    expect(filesSurfaceMigrationCommands(document)).toEqual([{
      type: "update_surface_state",
      surface_id: "legacy",
      state_schema_version: 2,
      state: migrateFilesSurfaceStateV1(v1("draft")),
    }]);
    document.surfaces.legacy = {
      ...document.surfaces.legacy,
      state_schema_version: 2,
      state: migrateFilesSurfaceStateV1(v1("draft")),
    };
    expect(filesSurfaceMigrationCommands(document)).toEqual([]);
  });
});

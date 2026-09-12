import { useState, type ComponentProps } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { ChangeReviewFileEntry } from "../../types";
import { GardenWorkspaceInterior } from "./GardenWorkspaceInterior";
import { buildTerrainPaint } from "./terrainPaint";
import type { TerrainChangeEntry } from "./useTerrainChanges";
import type { GardenTimeLens } from "./gardenNavigation";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const file = (path: string, turn: number, evidence: "attributed" | "inferred" = "attributed"): ChangeReviewFileEntry => ({
  path, change_kind: "modified", old_path: null, insertions: 1, deletions: 0,
  evidence, agent_ids: evidence === "attributed" ? ["a"] : [],
  turn_indices: evidence === "attributed" ? [turn] : [], binary: false, truncated: false, reviewed: false,
});
function props(): ComponentProps<typeof GardenWorkspaceInterior> {
  const files = [file("new.ts", 20), file("middle.ts", 10), file("old.ts", 1), file("unknown.ts", 0, "inferred")];
  const entries = new Map<string, TerrainChangeEntry>(files.map((entry) => [`/work/${entry.path}`, { entry, root: "/work", baselineRef: "base" }]));
  entries.set("/other/foreign.ts", { entry: file("foreign.ts", 20), root: "/other", baselineRef: "base" });
  return { path: "/work", entries, paint: buildTerrainPaint([{ root: "/work", entries: files, toTurnIndex: 20 }]),
    lens: "recent", selectedKey: null, onSelect: vi.fn(), onEnter: vi.fn(), onLensChange: vi.fn() };
}
beforeEach(() => { vi.mocked(invoke).mockReset(); });

describe("GardenWorkspaceInterior file activity", () => {
  it("offers scoped turn ranges and applies parent-controlled changes to the visible files", () => {
    const input = props();
    function ControlledWorkspace() {
      const [lens, setLens] = useState<GardenTimeLens>("recent");
      return <GardenWorkspaceInterior {...input} lens={lens} onLensChange={(next) => { input.onLensChange?.(next); setLens(next); }} />;
    }
    render(<ControlledWorkspace />);
    const control = within(screen.getByRole("group", { name: "File activity" })).getByRole("combobox", { name: "Turn range" });
    expect(within(control).getAllByRole("option").map((option) => option.textContent)).toEqual(["Latest 2 turns", "Latest 16 turns", "All compared changes"]);
    expect(control).toHaveValue("recent");
    expect(screen.getByText("middle.ts")).toBeVisible();
    expect(screen.queryByText("old.ts")).not.toBeInTheDocument();
    expect(screen.queryByText("foreign.ts")).not.toBeInTheDocument();
    fireEvent.change(control, { target: { value: "now" } });
    expect(input.onLensChange).toHaveBeenLastCalledWith("now");
    expect(screen.getByText("new.ts")).toBeVisible();
    expect(screen.queryByText("middle.ts")).not.toBeInTheDocument();
    expect(screen.getByText("unknown.ts")).toBeVisible();
    expect(screen.getByText(/inferred · recency uncertain/)).toBeVisible();
    fireEvent.change(control, { target: { value: "branch" } });
    expect(input.onLensChange).toHaveBeenLastCalledWith("branch");
    expect(screen.getByText("old.ts")).toBeVisible();
    fireEvent.change(control, { target: { value: "recent" } });
    expect(input.onLensChange).toHaveBeenLastCalledWith("recent");
    expect(screen.queryByText("old.ts")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("describes the actual comparison baseline independently of the selected turn range", () => {
    const input = props();
    const view = render(<GardenWorkspaceInterior {...input} baseline="head" lens="branch" />);
    expect(screen.getByText("Compared with HEAD (uncommitted changes). Changes with uncertain recency are included.")).toBeVisible();
    view.rerender(<GardenWorkspaceInterior {...input} baseline="branch_point" lens="now" />);
    expect(screen.getByText("Compared with the branch point. Changes with uncertain recency are included.")).toBeVisible();
    expect(screen.queryByText(/HEAD \(uncommitted changes\)/)).not.toBeInTheDocument();
  });

  it("disables turn filtering during full-tree browsing and restores the selected range afterward", async () => {
    vi.mocked(invoke).mockResolvedValue({ nodes: [{ name: "unchanged.ts", path: "/work/unchanged.ts", is_dir: false, extension: "ts" }], truncated: false });
    const input = props();
    render(<GardenWorkspaceInterior {...input} />);
    const range = screen.getByRole("combobox", { name: "Turn range" });
    fireEvent.click(screen.getByRole("checkbox", { name: "Show full tree" }));
    expect(range).toBeDisabled();
    expect(range).toHaveValue("recent");
    expect(await screen.findByText("unchanged.ts")).toBeVisible();
    expect(invoke).toHaveBeenCalledWith("get_directory_tree", { path: "/work", offset: 0 });
    expect(screen.getByText("Showing all folder contents; the turn range applies when full-tree browsing is off.")).toBeVisible();
    expect(input.onLensChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("checkbox", { name: "Show full tree" }));
    expect(range).toBeEnabled();
    expect(range).toHaveValue("recent");
    expect(screen.queryByText("unchanged.ts")).not.toBeInTheDocument();
    expect(screen.getByText("middle.ts")).toBeVisible();
  });
});

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { WorkbenchConflictDialog } from "./WorkbenchConflictDialog";

describe("WorkbenchConflictDialog", () => {
  it("offers explicit disk, replacement, and export resolutions", () => {
    const useDisk = vi.fn();
    const replaceDisk = vi.fn();
    const exportLocal = vi.fn();
    render(
      <WorkbenchConflictDialog
        mode="revision_conflict"
        on_use_disk={useDisk}
        on_replace_disk={replaceDisk}
        on_export_local={exportLocal}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Use Disk" }));
    fireEvent.click(screen.getByRole("button", { name: "Replace Disk" }));
    fireEvent.click(screen.getByRole("button", { name: "Export Local JSON" }));
    expect(useDisk).toHaveBeenCalledOnce();
    expect(replaceDisk).toHaveBeenCalledOnce();
    expect(exportLocal).toHaveBeenCalledOnce();
  });

  it("makes a future-schema conflict export-only", () => {
    render(
      <WorkbenchConflictDialog
        mode="future_schema"
        on_use_disk={vi.fn()}
        on_replace_disk={vi.fn()}
        on_export_local={vi.fn()}
      />,
    );

    expect(screen.queryByRole("button", { name: "Use Disk" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Replace Disk" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export Local JSON" })).toBeInTheDocument();
  });

  it("disables every resolution while one choice is loading", () => {
    render(
      <WorkbenchConflictDialog
        mode="revision_conflict"
        resolving={true}
        on_use_disk={vi.fn()}
        on_replace_disk={vi.fn()}
        on_export_local={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "Use Disk" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Replace Disk" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Export Local JSON" })).toBeDisabled();
  });
});

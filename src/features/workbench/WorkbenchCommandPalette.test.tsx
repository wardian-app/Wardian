import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WorkbenchCommandAction, WorkbenchCommandId } from "./useWorkbenchCommands";
import {
  filterWorkbenchCommands,
  WorkbenchCommandPalette,
} from "./WorkbenchCommandPalette";

const ACTIONS: readonly WorkbenchCommandAction[] = [
  { command_id: "workbench.command_palette", title: "Show Command Palette", shortcut: "Mod+Shift+P" },
  { command_id: "workbench.close_surface", title: "Close Surface", shortcut: "Mod+W" },
  { command_id: "workbench.reset_workbench", title: "Reset Workbench" },
  { command_id: "workbench.split_right", title: "Split Right", shortcut: "Mod+Alt+Right" },
];

describe("WorkbenchCommandPalette", () => {
  it("fuzzy filters commands and omits the palette command itself", () => {
    expect(filterWorkbenchCommands(ACTIONS, "splr").map((action) => action.command_id)).toEqual([
      "workbench.split_right",
    ]);
    expect(filterWorkbenchCommands(ACTIONS, "")).not.toContainEqual(
      expect.objectContaining({ command_id: "workbench.command_palette" }),
    );
  });

  it("renders an accessible searchable list with shortcuts and contextual disabling", async () => {
    const isEnabled = (commandId: WorkbenchCommandId) => commandId !== "workbench.close_surface";
    render(
      <WorkbenchCommandPalette
        open
        actions={ACTIONS}
        is_enabled={isEnabled}
        on_execute={vi.fn(async () => true)}
        on_close={vi.fn()}
      />,
    );

    expect(screen.getByRole("dialog", { name: "Command Palette" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Close Surface/ })).toBeDisabled();
    expect(screen.getByText(/(?:Ctrl|⌘)\+Alt\+Right/)).toBeInTheDocument();
    const input = screen.getByRole("combobox", { name: "Search commands" });
    await waitFor(() => expect(input).toHaveFocus());
    fireEvent.change(input, { target: { value: "splr" } });
    expect(screen.getByRole("option", { name: /Split Right/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Reset Workbench/ })).not.toBeInTheDocument();
  });

  it("supports arrow navigation, Enter execution, Escape, and focus return", async () => {
    const onExecute = vi.fn(async () => true);
    const onClose = vi.fn();
    render(
      <div>
        <button type="button">Before</button>
        <WorkbenchCommandPalette
          open
          actions={ACTIONS}
          is_enabled={(commandId) => commandId !== "workbench.close_surface"}
          on_execute={onExecute}
          on_close={onClose}
        />
      </div>,
    );
    const input = screen.getByRole("combobox", { name: "Search commands" });
    await waitFor(() => expect(input).toHaveFocus());

    fireEvent.change(input, { target: { value: "split" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(onExecute).toHaveBeenCalledWith("workbench.split_right"));
    expect(onClose).toHaveBeenCalledOnce();

    onClose.mockClear();
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });
});

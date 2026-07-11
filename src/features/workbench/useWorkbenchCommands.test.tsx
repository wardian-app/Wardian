import { act, fireEvent, render, screen } from "@testing-library/react";
import { useEffect, useRef } from "react";
import { describe, expect, it, vi } from "vitest";

import type { WorkbenchDocumentV1 } from "../../types";
import { createWorkbenchNavigationService } from "./navigationService";
import { applyWorkbenchCommand, type WorkbenchCommand } from "./workbenchModel";
import { makeSingleGroupDocument, makeSurface } from "./workbenchTestUtils";
import { createWorkbenchStore, type WorkbenchStore } from "./useWorkbenchStore";
import {
  useWorkbenchCommands,
  type WorkbenchCommandRouter,
} from "./useWorkbenchCommands";
import { createCoreWorkbenchSurfaceRegistry } from "./OpenSurfaceDialog";

function apply(document: WorkbenchDocumentV1, command: WorkbenchCommand): WorkbenchDocumentV1 {
  const result = applyWorkbenchCommand(document, command);
  if (!result.accepted) throw new Error(result.errors.map((error) => error.message).join(", "));
  return result.document;
}

function makeCommandDocument(): WorkbenchDocumentV1 {
  let document = makeSingleGroupDocument([
    makeSurface("surface-1", { surface_type: "agents-overview" }),
    makeSurface("surface-2", { surface_type: "dashboard" }),
    makeSurface("surface-3", { surface_type: "queue" }),
  ]);
  document = apply(document, {
    type: "split_group",
    group_id: "group-1",
    new_group_id: "group-2",
    node_id: "split-1",
    direction: "horizontal",
    placement: "after",
  });
  return apply(document, {
    type: "move_surface",
    surface_id: "surface-2",
    group_id: "group-2",
    index: 0,
  });
}

type HarnessProps = {
  store: WorkbenchStore;
  on_router: (router: WorkbenchCommandRouter) => void;
  on_quick_open?: () => void;
  on_command_palette?: () => void;
  on_focus_left_dock?: () => void;
  on_focus_right_dock?: () => void;
};

function CommandHarness({ store, on_router, ...callbacks }: HarnessProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const registry = useRef(createCoreWorkbenchSurfaceRegistry()).current;
  const navigation = useRef(createWorkbenchNavigationService({ registry, store })).current;
  const id = useRef(0);
  const router = useWorkbenchCommands({
    store,
    navigation,
    root_ref: rootRef,
    create_id: (kind) => `${kind}-command-${++id.current}`,
    ...callbacks,
  });
  useEffect(() => on_router(router), [on_router, router]);
  const document = store.getState().document;
  return (
    <div ref={rootRef} data-testid="command-root">
      {Object.values(document.groups).map((group) => (
        <section key={group.group_id} data-testid="workbench-group" data-group-id={group.group_id}>
          {group.surface_ids.map((surfaceId) => (
            <button
              key={surfaceId}
              role="tab"
              data-surface-id={surfaceId}
              data-surface-type={document.surfaces[surfaceId].surface_type}
            >
              {surfaceId}
            </button>
          ))}
        </section>
      ))}
      <input aria-label="Editable" />
      <button data-terminal-shortcuts="terminal">Terminal target</button>
    </div>
  );
}

describe("useWorkbenchCommands", () => {
  it("captures deterministic F6/tab traversal while suppressing editable and terminal shortcuts", async () => {
    const store = createWorkbenchStore({ initial_document: makeCommandDocument() });
    const onRouter = vi.fn();
    render(<CommandHarness store={store} on_router={onRouter} />);

    const activeTab = screen.getByRole("tab", { name: "surface-2" });
    activeTab.focus();
    fireEvent.keyDown(activeTab, { key: "F6" });
    expect(store.getState().document.active_group_id).toBe("group-1");
    expect(screen.getByRole("tab", { name: "surface-3" })).toHaveFocus();

    fireEvent.keyDown(screen.getByRole("tab", { name: "surface-3" }), {
      key: "]",
      ctrlKey: true,
    });
    expect(store.getState().document.groups["group-1"].active_surface_id).toBe("surface-1");

    const editable = screen.getByRole("textbox", { name: "Editable" });
    fireEvent.keyDown(editable, { key: "]", ctrlKey: true });
    expect(store.getState().document.groups["group-1"].active_surface_id).toBe("surface-1");

    const terminal = screen.getByRole("button", { name: "Terminal target" });
    fireEvent.keyDown(terminal, { key: "w", ctrlKey: true });
    expect(store.getState().document.surfaces["surface-1"]).toBeDefined();
  });

  it("executes split, move, close/reopen, reset, zoom, launcher, palette, and dock actions", async () => {
    const store = createWorkbenchStore({ initial_document: makeCommandDocument() });
    let router: WorkbenchCommandRouter | null = null;
    const onQuickOpen = vi.fn();
    const onCommandPalette = vi.fn();
    const onLeftDock = vi.fn();
    const onRightDock = vi.fn();
    render(
      <CommandHarness
        store={store}
        on_router={(next) => { router = next; }}
        on_quick_open={onQuickOpen}
        on_command_palette={onCommandPalette}
        on_focus_left_dock={onLeftDock}
        on_focus_right_dock={onRightDock}
      />,
    );
    expect(router).not.toBeNull();
    const commandRouter = router as unknown as WorkbenchCommandRouter;

    await act(() => commandRouter.execute("workbench.split_right"));
    expect(Object.keys(store.getState().document.groups)).toHaveLength(3);
    expect(store.getState().document.root.kind).toBe("split");

    act(() => {
      store.getState().apply_commands([{
        type: "set_active_surface",
        group_id: "group-2",
        surface_id: "surface-2",
      }]);
    });
    await act(() => commandRouter.execute("workbench.move_tab_previous_group"));
    expect(store.getState().document.groups["group-2"].surface_ids).toHaveLength(0);

    await act(() => commandRouter.execute("workbench.close_surface"));
    expect(store.getState().document.recently_closed).toHaveLength(1);
    await act(() => commandRouter.execute("workbench.reopen_closed_surface"));
    expect(store.getState().document.recently_closed).toHaveLength(0);

    await act(() => commandRouter.execute("workbench.toggle_group_zoom"));
    expect(store.getState().zoomed_group_id).not.toBeNull();
    await act(() => commandRouter.execute("workbench.open_surface"));
    expect(store.getState().launcher_open).toBe(true);
    await act(() => commandRouter.execute("workbench.quick_open"));
    await act(() => commandRouter.execute("workbench.command_palette"));
    await act(() => commandRouter.execute("workbench.focus_left_dock"));
    await act(() => commandRouter.execute("workbench.focus_right_dock"));
    expect(onQuickOpen).toHaveBeenCalledOnce();
    expect(onCommandPalette).toHaveBeenCalledOnce();
    expect(onLeftDock).toHaveBeenCalledOnce();
    expect(onRightDock).toHaveBeenCalledOnce();

    await act(() => commandRouter.execute("workbench.reset_workbench"));
    expect(Object.keys(store.getState().document.surfaces)).toHaveLength(0);
  });

  it("routes keyboard Quick Open and command palette actions", () => {
    const store = createWorkbenchStore({ initial_document: makeCommandDocument() });
    const onQuickOpen = vi.fn();
    const onCommandPalette = vi.fn();
    render(
      <CommandHarness
        store={store}
        on_router={() => {}}
        on_quick_open={onQuickOpen}
        on_command_palette={onCommandPalette}
      />,
    );

    fireEvent.keyDown(screen.getByTestId("command-root"), { key: "p", ctrlKey: true });
    fireEvent.keyDown(screen.getByTestId("command-root"), {
      key: "p",
      ctrlKey: true,
      shiftKey: true,
    });
    expect(onQuickOpen).toHaveBeenCalledOnce();
    expect(onCommandPalette).toHaveBeenCalledOnce();
  });
});

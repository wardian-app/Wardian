import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WorkbenchDocumentV1 } from "../../types";
import { applyWorkbenchCommand, type WorkbenchCommand } from "../../features/workbench/workbenchModel";
import { makeSingleGroupDocument, makeSurface } from "../../features/workbench/workbenchTestUtils";
import {
  canSplitWorkbenchPane,
  deriveWorkbenchSplitRatios,
  dispatchWorkbenchAdapterCommand,
  DockviewLayoutAdapter,
  handleWorkbenchDockviewGroupDrag,
  handleWorkbenchDockviewOverlayAdmission,
  isCanonicalWorkbenchGroupDestination,
  normalizedWorkbenchGeometry,
  planDockviewGroupPlacements,
  projectWorkbenchGroupSizes,
  routeWorkbenchDockviewDrop,
  shouldRecoverUnexpectedPanelRemoval,
  workbenchDockviewDropOverlayModel,
  workbenchGroupOwnsWindowChrome,
  workbenchGroupTouchesLeftEdge,
  workbenchGroupTouchesTopEdge,
  workbenchPaneTargets,
  workbenchPaneSplitAdmission,
  workbenchSplitRatioCommands,
  WORKBENCH_PANE_MINIMUM_HEIGHT,
  WORKBENCH_PANE_MINIMUM_WIDTH,
} from "./DockviewLayoutAdapter";
import {
  DockviewApi,
  type DockviewGroupPanel,
  type GroupDragEvent,
  type DockviewWillDropEvent,
  type DockviewWillShowOverlayLocationEvent,
} from "dockview-react";

function apply(document: WorkbenchDocumentV1, command: WorkbenchCommand): WorkbenchDocumentV1 {
  const result = applyWorkbenchCommand(document, command);
  if (!result.accepted) throw new Error(result.errors.map((error) => error.message).join(", "));
  return result.document;
}

function makeTwoGroupDocument(): WorkbenchDocumentV1 {
  const first = makeSurface("surface-1", { surface_type: "agents-overview" });
  const second = makeSurface("surface-2", {
    surface_type: "agent-session",
    resource_key: "agent-7",
  });
  let document = makeSingleGroupDocument([first, second]);
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
    surface_id: second.surface_id,
    group_id: "group-2",
    index: 0,
  });
}

function makeMixedDepthThreeDocument(): WorkbenchDocumentV1 {
  let document = makeSingleGroupDocument([
    makeSurface("surface-1", { surface_type: "agents-overview" }),
  ]);
  document = apply(document, {
    type: "split_group",
    group_id: "group-1",
    new_group_id: "group-2",
    node_id: "split-root",
    direction: "horizontal",
    placement: "after",
  });
  document = apply(document, {
    type: "split_group",
    group_id: "group-1",
    new_group_id: "group-3",
    node_id: "split-left",
    direction: "vertical",
    placement: "after",
  });
  return apply(document, {
    type: "split_group",
    group_id: "group-2",
    new_group_id: "group-4",
    node_id: "split-right",
    direction: "vertical",
    placement: "after",
  });
}

describe("DockviewLayoutAdapter", () => {
  it("admits edge splits only when both half-panes meet the Wardian minimum", () => {
    const horizontalBoundary = WORKBENCH_PANE_MINIMUM_WIDTH * 2;
    const verticalBoundary = WORKBENCH_PANE_MINIMUM_HEIGHT * 2;

    expect(canSplitWorkbenchPane(
      { left: 0, top: 0, width: horizontalBoundary - 1, height: verticalBoundary },
      "left",
    )).toBe(false);
    expect(canSplitWorkbenchPane(
      { left: 0, top: 0, width: horizontalBoundary, height: verticalBoundary },
      "right",
    )).toBe(true);
    expect(canSplitWorkbenchPane(
      { left: 0, top: 0, width: horizontalBoundary, height: verticalBoundary - 1 },
      "top",
    )).toBe(false);
    expect(canSplitWorkbenchPane(
      { left: 0, top: 0, width: horizontalBoundary, height: verticalBoundary },
      "bottom",
    )).toBe(true);
    expect(canSplitWorkbenchPane(
      { left: 0, top: 0, width: 0, height: 0 },
      "center",
    )).toBe(true);
    expect(canSplitWorkbenchPane(undefined, "right")).toBe(true);
    expect(workbenchPaneSplitAdmission(undefined, "right")).toBe("unmeasured");
  });

  it("suppresses only edge overlays that the live destination cannot hold", () => {
    const preventNarrow = vi.fn();
    handleWorkbenchDockviewOverlayAdmission({
      position: "left",
      preventDefault: preventNarrow,
      group: {
        api: { boundingBox: { left: 0, top: 0, width: 199, height: 400 } },
      },
    } as unknown as DockviewWillShowOverlayLocationEvent);
    expect(preventNarrow).toHaveBeenCalledOnce();

    const preventViable = vi.fn();
    handleWorkbenchDockviewOverlayAdmission({
      position: "right",
      preventDefault: preventViable,
      group: {
        api: { boundingBox: { left: 0, top: 0, width: 200, height: 400 } },
      },
    } as unknown as DockviewWillShowOverlayLocationEvent);
    expect(preventViable).not.toHaveBeenCalled();

    const preventCenter = vi.fn();
    handleWorkbenchDockviewOverlayAdmission({
      position: "center",
      preventDefault: preventCenter,
      group: undefined,
    } as unknown as DockviewWillShowOverlayLocationEvent);
    expect(preventCenter).not.toHaveBeenCalled();

    const preventUnmeasured = vi.fn();
    handleWorkbenchDockviewOverlayAdmission({
      position: "bottom",
      preventDefault: preventUnmeasured,
      group: undefined,
    } as unknown as DockviewWillShowOverlayLocationEvent);
    expect(preventUnmeasured).not.toHaveBeenCalled();
  });

  it("reserves top-edge group drag handles for native window dragging", () => {
    const root = makeMixedDepthThreeDocument().root;
    const topPreventDefault = vi.fn();
    const lowerPreventDefault = vi.fn();

    handleWorkbenchDockviewGroupDrag({
      group: { id: "group-1" },
      nativeEvent: { preventDefault: topPreventDefault },
    } as unknown as GroupDragEvent, root, null);
    handleWorkbenchDockviewGroupDrag({
      group: { id: "group-3" },
      nativeEvent: { preventDefault: lowerPreventDefault },
    } as unknown as GroupDragEvent, root, null);

    expect(topPreventDefault).toHaveBeenCalledOnce();
    expect(lowerPreventDefault).not.toHaveBeenCalled();
  });

  it("subscribes overlay admission with the ready API and disposes it on teardown", async () => {
    type OverlayListener = Parameters<DockviewApi["onWillShowOverlay"]>[0];
    const overlayDescriptor = Object.getOwnPropertyDescriptor(
      DockviewApi.prototype,
      "onWillShowOverlay",
    );
    if (!overlayDescriptor?.get) throw new Error("expected DockviewApi overlay event getter");
    let overlayListener: OverlayListener | undefined;
    const dispose = vi.fn();
    Object.defineProperty(DockviewApi.prototype, "onWillShowOverlay", {
      ...overlayDescriptor,
      get(this: DockviewApi) {
        const subscribe = overlayDescriptor.get?.call(this) as DockviewApi["onWillShowOverlay"];
        return ((listener: OverlayListener) => {
          overlayListener = listener;
          const disposable = subscribe(listener);
          return {
            dispose: () => {
              dispose();
              disposable.dispose();
            },
          };
        }) as DockviewApi["onWillShowOverlay"];
      },
    });

    try {
      const view = render(<DockviewLayoutAdapter document={makeTwoGroupDocument()} />);
      await waitFor(() => expect(overlayListener).toBeDefined());
      const preventDefault = vi.fn();
      overlayListener?.({
        position: "bottom",
        preventDefault,
        group: {
          api: { boundingBox: { left: 0, top: 0, width: 400, height: 199 } },
        },
      } as unknown as Parameters<OverlayListener>[0]);
      expect(preventDefault).toHaveBeenCalledOnce();
      view.unmount();
      expect(dispose).toHaveBeenCalledOnce();
    } finally {
      Object.defineProperty(DockviewApi.prototype, "onWillShowOverlay", overlayDescriptor);
    }
  });

  it("projects exact Wardian IDs and DOM metadata without a renderer serialization source", async () => {
    render(<DockviewLayoutAdapter document={makeTwoGroupDocument()} />);

    await waitFor(() => expect(screen.getAllByTestId("workbench-group")).toHaveLength(2));
    expect(screen.getAllByRole("tab").map((tab) => tab.dataset.surfaceId)).toEqual([
      "surface-1",
      "surface-2",
    ]);
    expect(document.querySelector('[data-group-id="group-1"]')).not.toBeNull();
    expect(document.querySelector('[data-group-id="group-2"]')).not.toBeNull();
    expect(document.querySelector('[data-surface-id="surface-2"][data-resource-key="agent-7"]')).not.toBeNull();
    expect(document.querySelector('[data-layout-source="wardian-model"]')).not.toBeNull();
    expect(document.querySelector('[data-layout-source="dockview-json"]')).toBeNull();
    const tab = screen.getByRole("tab", { name: /agents overview/i });
    expect(tab.id).toMatch(/^dv-tab-/);
    const controlledId = tab.getAttribute("aria-controls");
    expect(controlledId).toMatch(/^dv-tabpanel-/);
    const controlledPanel = controlledId ? document.getElementById(controlledId) : null;
    expect(controlledPanel).toHaveAttribute("role", "tabpanel");
    expect(controlledPanel).toHaveAttribute("aria-labelledby", tab.id);
    expect(document.getElementById("workbench-panel-surface-1")).toHaveAttribute(
      "data-surface-id",
      "surface-1",
    );
  });

  it("creates every canonical Dockview group with Wardian pane constraints", async () => {
    const addGroup = DockviewApi.prototype.addGroup;
    const addGroupSpy = vi.spyOn(DockviewApi.prototype, "addGroup")
      .mockImplementation(function addConstrainedGroup(
        this: DockviewApi,
        options?: Parameters<DockviewApi["addGroup"]>[0],
      ) {
        return addGroup.call(this, options);
      });
    try {
      render(<DockviewLayoutAdapter document={makeTwoGroupDocument()} />);
      await waitFor(() => expect(screen.getAllByTestId("workbench-group")).toHaveLength(2));
      expect(addGroupSpy).toHaveBeenCalledTimes(2);
      for (const [options] of addGroupSpy.mock.calls) {
        expect(options).toMatchObject({
          hideHeader: false,
          headerPosition: "top",
          constraints: {
            minimumWidth: WORKBENCH_PANE_MINIMUM_WIDTH,
            minimumHeight: WORKBENCH_PANE_MINIMUM_HEIGHT,
          },
        });
      }
    } finally {
      addGroupSpy.mockRestore();
    }
  });

  it("reprojects a retained canonical group's tab strip as visible top chrome", async () => {
    const documentModel = makeSingleGroupDocument([
      makeSurface("surface-1", { surface_type: "agents-overview" }),
    ]);
    const addGroup = DockviewApi.prototype.addGroup;
    let projectedGroup: DockviewGroupPanel | undefined;
    const addGroupSpy = vi.spyOn(DockviewApi.prototype, "addGroup")
      .mockImplementation(function captureGroup(
        this: DockviewApi,
        options?: Parameters<DockviewApi["addGroup"]>[0],
      ) {
        const group = addGroup.call(this, options);
        if (group.id === "group-1") projectedGroup = group;
        return group;
      });

    try {
      const view = render(<DockviewLayoutAdapter document={documentModel} />);
      await screen.findByRole("tab", { name: /agents overview/i });
      if (!projectedGroup) throw new Error("expected canonical Dockview group");

      act(() => {
        projectedGroup!.header.hidden = true;
        projectedGroup!.api.setHeaderPosition("bottom");
      });
      expect(projectedGroup.header.hidden).toBe(true);
      expect(projectedGroup.api.getHeaderPosition()).toBe("bottom");

      view.rerender(
        <DockviewLayoutAdapter document={structuredClone(documentModel)} />,
      );

      await waitFor(() => expect(projectedGroup?.header.hidden).toBe(false));
      expect(projectedGroup.api.getHeaderPosition()).toBe("top");
      expect(screen.getByRole("tab", { name: /agents overview/i })).toBeVisible();
    } finally {
      addGroupSpy.mockRestore();
    }
  });

  it("recreates a canonical empty group after Dockview removes its final panel", async () => {
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    const initial = makeSingleGroupDocument([surface]);
    const closed = apply(initial, { type: "close_surface", surface_id: surface.surface_id });
    const renderHome = (groupId: string) => (
      <div data-testid="canonical-empty-home" data-group-id={groupId}>
        <h2>Choose a surface</h2>
      </div>
    );
    const { rerender } = render(
      <DockviewLayoutAdapter document={initial} render_home={renderHome} />,
    );

    await screen.findByRole("tab", { name: /agents overview/i });
    rerender(<DockviewLayoutAdapter document={closed} render_home={renderHome} />);

    await waitFor(() => expect(screen.getAllByTestId("workbench-group")).toHaveLength(1));
    const group = screen.getByTestId("workbench-group");
    expect(group).toHaveAttribute("data-group-id", "group-1");
    expect(within(group).queryAllByRole("tab")).toHaveLength(0);
    expect(within(group).getByRole("heading", { name: "Choose a surface" })).toBeVisible();
    expect(within(group).getByRole("button", { name: "Open Surface" })).toBeVisible();
    expect(within(group).getByRole("button", { name: "Pane actions" })).toBeVisible();
  });

  it("retains a re-added canonical empty group before Dockview republishes it", async () => {
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    const initial = makeSingleGroupDocument([surface]);
    const closed = apply(initial, { type: "close_surface", surface_id: surface.surface_id });
    const view = render(<DockviewLayoutAdapter document={initial} />);
    await screen.findByRole("tab", { name: /agents overview/i });

    const groupsGetter = Object.getOwnPropertyDescriptor(DockviewApi.prototype, "groups")?.get;
    if (!groupsGetter) throw new Error("expected DockviewApi groups getter");
    const addGroup = DockviewApi.prototype.addGroup;
    let delayEmptyGroupPublication = false;
    const groupsSpy = vi.spyOn(DockviewApi.prototype, "groups", "get")
      .mockImplementation(function groupsWithDelayedEmptyGroup(this: DockviewApi) {
        const groups = groupsGetter.call(this);
        return delayEmptyGroupPublication
          ? groups.filter((group: DockviewGroupPanel) => group.id !== "group-1")
          : groups;
      });
    const addGroupSpy = vi.spyOn(DockviewApi.prototype, "addGroup")
      .mockImplementation(function addDelayedEmptyGroup(
        this: DockviewApi,
        options?: Parameters<DockviewApi["addGroup"]>[0],
      ) {
        const group = addGroup.call(this, options);
        if (group.id === "group-1") delayEmptyGroupPublication = true;
        return group;
      });

    try {
      view.rerender(<DockviewLayoutAdapter document={closed} />);
      await waitFor(() => expect(screen.getByTestId("workbench-group"))
        .toHaveAttribute("data-group-id", "group-1"));
      const group = screen.getByTestId("workbench-group");
      expect(group).toHaveAttribute("data-active", "true");
      expect(within(group).getByTestId("workbench-empty-group")).toBeVisible();
    } finally {
      addGroupSpy.mockRestore();
      groupsSpy.mockRestore();
    }
  });

  it("recovers when the canonical root replaces the sole-tab source group", async () => {
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    const initial = makeSingleGroupDocument([surface]);
    const replacement: WorkbenchDocumentV1 = {
      ...initial,
      root: { kind: "group", group_id: "group-2" },
      groups: {
        "group-2": {
          ...initial.groups["group-1"],
          group_id: "group-2",
        },
      },
      active_group_id: "group-2",
    };
    const onCommand = vi.fn((_command: WorkbenchCommand) => true);
    const groupsGetter = Object.getOwnPropertyDescriptor(DockviewApi.prototype, "groups")?.get;
    if (!groupsGetter) throw new Error("expected DockviewApi groups getter");
    let delayReplacementPublication = false;
    let delayedReads = 0;
    const groupsSpy = vi.spyOn(DockviewApi.prototype, "groups", "get")
      .mockImplementation(function groupsWithDelayedReplacement(this: DockviewApi) {
        const groups = groupsGetter.call(this);
        if (!delayReplacementPublication || delayedReads >= 2) return groups;
        delayedReads += 1;
        return groups.filter((group: DockviewGroupPanel) => group.id !== "group-2");
      });
    const view = render(
      <DockviewLayoutAdapter document={initial} on_command={onCommand} />,
    );

    await screen.findByRole("tab", { name: /agents overview/i });
    onCommand.mockClear();
    delayReplacementPublication = true;
    try {
      expect(() => view.rerender(
        <DockviewLayoutAdapter document={replacement} on_command={onCommand} />,
      )).not.toThrow();
    } finally {
      groupsSpy.mockRestore();
    }

    await waitFor(() => expect(screen.getByTestId("workbench-group"))
      .toHaveAttribute("data-group-id", "group-2"));
    expect(screen.getByRole("tab", { name: /agents overview/i })).toBeVisible();
    expect(onCommand.mock.calls.every(([command]) => (
      !("group_id" in command) || command.group_id === "group-2"
    ))).toBe(true);
  });

  it("bounds recovery when a canonical group remains transiently unavailable", async () => {
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    const initial = makeSingleGroupDocument([surface]);
    const replacement: WorkbenchDocumentV1 = {
      ...initial,
      root: { kind: "group", group_id: "group-2" },
      groups: {
        "group-2": {
          ...initial.groups["group-1"],
          group_id: "group-2",
        },
      },
      active_group_id: "group-2",
    };
    const view = render(<DockviewLayoutAdapter document={initial} />);
    await screen.findByRole("tab", { name: /agents overview/i });

    const groupsGetter = Object.getOwnPropertyDescriptor(DockviewApi.prototype, "groups")?.get;
    if (!groupsGetter) throw new Error("expected DockviewApi groups getter");
    const groupsSpy = vi.spyOn(DockviewApi.prototype, "groups", "get")
      .mockImplementation(function groupsWithoutReplacement(this: DockviewApi) {
        return groupsGetter.call(this)
          .filter((group: DockviewGroupPanel) => group.id !== "group-2");
      });
    const addGroupSpy = vi.spyOn(DockviewApi.prototype, "addGroup")
      .mockImplementation(() => undefined as unknown as ReturnType<DockviewApi["addGroup"]>);
    const consoleErrorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);

    try {
      expect(() => view.rerender(<DockviewLayoutAdapter document={replacement} />)).not.toThrow();
      await waitFor(() => expect(consoleErrorSpy).toHaveBeenCalledOnce());
      await new Promise((resolve) => window.setTimeout(resolve, 10));
      expect(addGroupSpy).toHaveBeenCalledTimes(2);
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        "Dockview group projection deferred",
        { group_id: "group-2", surface_ids: ["surface-1"] },
      );
    } finally {
      consoleErrorSpy.mockRestore();
      addGroupSpy.mockRestore();
      groupsSpy.mockRestore();
    }
  });

  it("recognizes only groups in the current canonical document as destinations", () => {
    const documentModel = makeTwoGroupDocument();
    expect(isCanonicalWorkbenchGroupDestination(documentModel, "group-2")).toBe(true);
    expect(isCanonicalWorkbenchGroupDestination(documentModel, "stale-group")).toBe(false);
  });

  it("ignores stale user move and activation callbacks", async () => {
    type MoveListener = Parameters<DockviewApi["onDidMovePanel"]>[0];
    type ActivePanelListener = Parameters<DockviewApi["onDidActivePanelChange"]>[0];
    type ActiveGroupListener = Parameters<DockviewApi["onDidActiveGroupChange"]>[0];
    const moveDescriptor = Object.getOwnPropertyDescriptor(
      DockviewApi.prototype,
      "onDidMovePanel",
    );
    const activePanelDescriptor = Object.getOwnPropertyDescriptor(
      DockviewApi.prototype,
      "onDidActivePanelChange",
    );
    const activeGroupDescriptor = Object.getOwnPropertyDescriptor(
      DockviewApi.prototype,
      "onDidActiveGroupChange",
    );
    if (!moveDescriptor?.get || !activePanelDescriptor?.get || !activeGroupDescriptor?.get) {
      throw new Error("expected DockviewApi event getters");
    }
    let moveListener: MoveListener | undefined;
    let activePanelListener: ActivePanelListener | undefined;
    let activeGroupListener: ActiveGroupListener | undefined;
    Object.defineProperty(DockviewApi.prototype, "onDidMovePanel", {
      ...moveDescriptor,
      get(this: DockviewApi) {
        const subscribe = moveDescriptor.get?.call(this) as DockviewApi["onDidMovePanel"];
        return ((listener: MoveListener) => {
          moveListener = listener;
          return subscribe(listener);
        }) as DockviewApi["onDidMovePanel"];
      },
    });
    Object.defineProperty(DockviewApi.prototype, "onDidActivePanelChange", {
      ...activePanelDescriptor,
      get(this: DockviewApi) {
        const subscribe = activePanelDescriptor.get?.call(this) as
          DockviewApi["onDidActivePanelChange"];
        return ((listener: ActivePanelListener) => {
          activePanelListener = listener;
          return subscribe(listener);
        }) as DockviewApi["onDidActivePanelChange"];
      },
    });
    Object.defineProperty(DockviewApi.prototype, "onDidActiveGroupChange", {
      ...activeGroupDescriptor,
      get(this: DockviewApi) {
        const subscribe = activeGroupDescriptor.get?.call(this) as
          DockviewApi["onDidActiveGroupChange"];
        return ((listener: ActiveGroupListener) => {
          activeGroupListener = listener;
          return subscribe(listener);
        }) as DockviewApi["onDidActiveGroupChange"];
      },
    });
    const onCommand = vi.fn((_command: WorkbenchCommand) => true);

    try {
      render(<DockviewLayoutAdapter document={makeTwoGroupDocument()} on_command={onCommand} />);
      await waitFor(() => expect(moveListener).toBeDefined());
      if (!moveListener || !activePanelListener || !activeGroupListener) {
        throw new Error("expected adapter event listeners");
      }
      onCommand.mockClear();
      const stalePanel = {
        id: "surface-1",
        group: {
          id: "stale-group",
          panels: [{ id: "surface-1" }],
          activePanel: { id: "surface-1" },
        },
      };

      moveListener({ panel: stalePanel } as Parameters<MoveListener>[0]);
      activePanelListener({
        origin: "user",
        panel: stalePanel,
      } as Parameters<ActivePanelListener>[0]);
      activeGroupListener(stalePanel.group as Parameters<ActiveGroupListener>[0]);
      await Promise.resolve();

      expect(onCommand).not.toHaveBeenCalled();
    } finally {
      Object.defineProperty(DockviewApi.prototype, "onDidActiveGroupChange", activeGroupDescriptor);
      Object.defineProperty(DockviewApi.prototype, "onDidActivePanelChange", activePanelDescriptor);
      Object.defineProperty(DockviewApi.prototype, "onDidMovePanel", moveDescriptor);
    }
  });

  it("requests recovery only for canonical panels removed outside projection", () => {
    const documentModel = makeTwoGroupDocument();
    expect(shouldRecoverUnexpectedPanelRemoval(documentModel, "surface-1", false)).toBe(true);
    expect(shouldRecoverUnexpectedPanelRemoval(documentModel, "surface-1", true)).toBe(false);
    expect(shouldRecoverUnexpectedPanelRemoval(documentModel, "missing", false)).toBe(false);
  });

  it("requests one self-heal only when the canonical writer explicitly rejects", () => {
    const rejected = vi.fn(() => false);
    const accepted = vi.fn(() => true);
    const recover = vi.fn(() => true);
    const diagnostic = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const command = { type: "focus_surface", surface_id: "surface-1" } as const;

    expect(dispatchWorkbenchAdapterCommand(command, rejected, recover)).toBe(false);
    expect(recover).toHaveBeenCalledOnce();
    expect(diagnostic).toHaveBeenCalledWith("Workbench canonical command rejected", { command });
    recover.mockClear();
    diagnostic.mockClear();
    expect(dispatchWorkbenchAdapterCommand(command, accepted, recover)).toBe(true);
    expect(dispatchWorkbenchAdapterCommand(command, undefined, recover)).toBe(false);
    expect(recover).toHaveBeenCalledOnce();
    expect(diagnostic).not.toHaveBeenCalled();
    diagnostic.mockRestore();
  });

  it("bounds rejection diagnostics through the adapter and re-arms its recovery cycle", async () => {
    type MoveListener = Parameters<DockviewApi["onDidMovePanel"]>[0];
    const moveDescriptor = Object.getOwnPropertyDescriptor(
      DockviewApi.prototype,
      "onDidMovePanel",
    );
    if (!moveDescriptor?.get) throw new Error("expected DockviewApi move event getter");
    let moveListener: MoveListener | undefined;
    Object.defineProperty(DockviewApi.prototype, "onDidMovePanel", {
      ...moveDescriptor,
      get(this: DockviewApi) {
        const subscribe = moveDescriptor.get?.call(this) as DockviewApi["onDidMovePanel"];
        return ((listener: MoveListener) => {
          moveListener = listener;
          return subscribe(listener);
        }) as DockviewApi["onDidMovePanel"];
      },
    });
    const rejected = vi.fn(() => false);
    const diagnostic = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const event = {
      panel: {
        id: "surface-1",
        group: {
          id: "group-2",
          panels: [{ id: "surface-1" }],
        },
      },
    } as Parameters<MoveListener>[0];

    try {
      render(<DockviewLayoutAdapter document={makeTwoGroupDocument()} on_command={rejected} />);
      await waitFor(() => expect(moveListener).toBeDefined());
      if (!moveListener) throw new Error("expected adapter move listener");
      diagnostic.mockClear();

      act(() => {
        moveListener?.(event);
        moveListener?.(event);
      });
      expect(diagnostic).toHaveBeenCalledOnce();
      expect(diagnostic).toHaveBeenCalledWith("Workbench canonical command rejected", {
        command: {
          type: "move_surface",
          surface_id: "surface-1",
          group_id: "group-2",
          index: 0,
        },
      });

      await act(async () => Promise.resolve());
      act(() => moveListener?.(event));
      expect(diagnostic).toHaveBeenCalledTimes(2);
    } finally {
      diagnostic.mockRestore();
      Object.defineProperty(DockviewApi.prototype, "onDidMovePanel", moveDescriptor);
    }
  });

  it("seeds both sides before subdividing mixed depth-three canonical trees", () => {
    expect(planDockviewGroupPlacements(makeMixedDepthThreeDocument().root)).toEqual([
      { group_id: "group-1" },
      { group_id: "group-2", reference_group_id: "group-1", direction: "right" },
      { group_id: "group-3", reference_group_id: "group-1", direction: "below" },
      { group_id: "group-4", reference_group_id: "group-2", direction: "below" },
    ]);
  });

  it("limits native window dragging to groups that touch the top edge", () => {
    const root = makeMixedDepthThreeDocument().root;

    expect(workbenchGroupTouchesTopEdge(root, "group-1")).toBe(true);
    expect(workbenchGroupTouchesTopEdge(root, "group-2")).toBe(true);
    expect(workbenchGroupTouchesTopEdge(root, "group-3")).toBe(false);
    expect(workbenchGroupTouchesTopEdge(root, "group-4")).toBe(false);
    expect(workbenchGroupTouchesLeftEdge(root, "group-1")).toBe(true);
    expect(workbenchGroupTouchesLeftEdge(root, "group-3")).toBe(true);
    expect(workbenchGroupTouchesLeftEdge(root, "group-2")).toBe(false);
    expect(workbenchGroupTouchesLeftEdge(root, "group-4")).toBe(false);
    expect(workbenchGroupOwnsWindowChrome(root, "group-3", null)).toBe(false);
    expect(workbenchGroupOwnsWindowChrome(root, "group-3", "group-3")).toBe(true);
  });

  it("promotes a zoomed lower pane to the draggable window chrome", async () => {
    const documentModel = makeTwoGroupDocument();
    if (documentModel.root.kind !== "split") throw new Error("expected split root");
    documentModel.root.direction = "vertical";
    const { rerender } = render(<DockviewLayoutAdapter document={documentModel} />);

    await waitFor(() => expect(screen.getAllByTestId("workbench-group")).toHaveLength(2));
    const lowerGroup = document.querySelector<HTMLElement>('[data-group-id="group-2"]');
    const lowerEmptyHeader = lowerGroup?.querySelector<HTMLElement>(".dv-void-container");
    expect(lowerEmptyHeader).not.toHaveAttribute("data-tauri-drag-region");

    rerender(
      <DockviewLayoutAdapter
        document={documentModel}
        zoomed_group_id="group-2"
      />,
    );

    await waitFor(() => expect(lowerEmptyHeader).toHaveAttribute("data-tauri-drag-region"));
  });

  it("projects canonical nested ratios through public group setSize calls", () => {
    const documentModel = makeMixedDepthThreeDocument();
    if (documentModel.root.kind !== "split") throw new Error("expected split root");
    documentModel.root.ratio = 0.6;
    if (documentModel.root.first.kind !== "split") throw new Error("expected left split");
    documentModel.root.first.ratio = 0.25;
    if (documentModel.root.second.kind !== "split") throw new Error("expected right split");
    documentModel.root.second.ratio = 0.75;

    expect(normalizedWorkbenchGeometry(documentModel.root).groups).toEqual({
      "group-1": { left: 0, top: 0, width: 0.6, height: 0.25 },
      "group-3": { left: 0, top: 0.25, width: 0.6, height: 0.75 },
      "group-2": { left: 0.6, top: 0, width: 0.4, height: 0.75 },
      "group-4": { left: 0.6, top: 0.75, width: 0.4, height: 0.25 },
    });

    const setSizes = new Map<string, ReturnType<typeof vi.fn>>();
    const groups = Object.keys(documentModel.groups).map((id) => {
      const setSize = vi.fn();
      setSizes.set(id, setSize);
      return { id, api: { setSize } };
    });
    const fakeApi = { width: 1000, height: 800, groups } as unknown as DockviewApi;

    projectWorkbenchGroupSizes(fakeApi, documentModel.root);

    expect(setSizes.get("group-1")).toHaveBeenCalledWith({ width: 600, height: 200 });
    expect(setSizes.get("group-3")).toHaveBeenCalledWith({ width: 600, height: 600 });
    expect(setSizes.get("group-2")).toHaveBeenCalledWith({ width: 400, height: 600 });
    expect(setSizes.get("group-4")).toHaveBeenCalledWith({ width: 400, height: 200 });
  });

  it("derives canonical split ratios from descendant public bounding boxes", () => {
    const root = makeMixedDepthThreeDocument().root;
    expect(deriveWorkbenchSplitRatios(root, {
      "group-1": { left: 0, top: 0, width: 600, height: 200 },
      "group-3": { left: 0, top: 200, width: 600, height: 600 },
      "group-2": { left: 600, top: 0, width: 400, height: 600 },
      "group-4": { left: 600, top: 600, width: 400, height: 200 },
    })).toEqual({
      "split-root": 0.6,
      "split-left": 0.25,
      "split-right": 0.75,
    });
  });

  it("emits ratio feedback only outside the adapter epsilon", () => {
    const root = makeMixedDepthThreeDocument().root;
    const rectangles = {
      "group-1": { left: 0, top: 0, width: 600, height: 200 },
      "group-3": { left: 0, top: 200, width: 600, height: 600 },
      "group-2": { left: 600, top: 0, width: 400, height: 600 },
      "group-4": { left: 600, top: 600, width: 400, height: 200 },
    };

    expect(workbenchSplitRatioCommands(root, rectangles)).toEqual([
      { type: "set_split_ratio", node_id: "split-root", ratio: 0.6 },
      { type: "set_split_ratio", node_id: "split-left", ratio: 0.25 },
      { type: "set_split_ratio", node_id: "split-right", ratio: 0.75 },
    ]);

    const nearlyEqual = makeTwoGroupDocument().root;
    expect(workbenchSplitRatioCommands(nearlyEqual, {
      "group-1": { left: 0, top: 0, width: 502, height: 800 },
      "group-2": { left: 502, top: 0, width: 498, height: 800 },
    })).toEqual([]);
  });

  it("keeps center/tab drops in Dockview and routes edge drops to canonical handling", () => {
    const documentModel = makeTwoGroupDocument();
    const onSurfaceDrop = vi.fn();
    const centerPreventDefault = vi.fn();
    const makeDrop = (
      position: "center" | "left",
      preventDefault: () => void,
      bounds = { left: 0, top: 0, width: 400, height: 400 },
    ): DockviewWillDropEvent => ({
      position,
      preventDefault,
      getData: () => ({ panelId: "surface-1" }),
      panel: undefined,
      group: { id: "group-2", api: { boundingBox: bounds } },
    }) as unknown as DockviewWillDropEvent;

    routeWorkbenchDockviewDrop(makeDrop("center", centerPreventDefault), documentModel, onSurfaceDrop);
    expect(centerPreventDefault).not.toHaveBeenCalled();
    expect(onSurfaceDrop).not.toHaveBeenCalled();

    const orphanCenterPreventDefault = vi.fn();
    routeWorkbenchDockviewDrop({
      position: "center",
      preventDefault: orphanCenterPreventDefault,
      getData: () => ({ panelId: "surface-1" }),
      panel: undefined,
      group: undefined,
    } as unknown as DockviewWillDropEvent, documentModel, onSurfaceDrop);
    expect(orphanCenterPreventDefault).toHaveBeenCalledOnce();

    const edgePreventDefault = vi.fn();
    routeWorkbenchDockviewDrop(makeDrop("left", edgePreventDefault), documentModel, onSurfaceDrop);
    expect(edgePreventDefault).toHaveBeenCalledOnce();
    expect(onSurfaceDrop).toHaveBeenCalledWith("surface-1", "group-2", "left");

    const impossibleEdgePreventDefault = vi.fn();
    routeWorkbenchDockviewDrop(
      makeDrop("left", impossibleEdgePreventDefault, {
        left: 0,
        top: 0,
        width: (WORKBENCH_PANE_MINIMUM_WIDTH * 2) - 1,
        height: WORKBENCH_PANE_MINIMUM_HEIGHT * 2,
      }),
      documentModel,
      onSurfaceDrop,
    );
    expect(impossibleEdgePreventDefault).toHaveBeenCalledOnce();
    expect(onSurfaceDrop).toHaveBeenCalledOnce();

    const unmeasuredEdgePreventDefault = vi.fn();
    routeWorkbenchDockviewDrop({
      position: "right",
      preventDefault: unmeasuredEdgePreventDefault,
      getData: () => ({ panelId: "surface-1" }),
      panel: undefined,
      group: { id: "group-2", api: { boundingBox: undefined } },
    } as unknown as DockviewWillDropEvent, documentModel, onSurfaceDrop);
    expect(unmeasuredEdgePreventDefault).toHaveBeenCalledOnce();
    expect(onSurfaceDrop).toHaveBeenLastCalledWith("surface-1", "group-2", "right");
    expect(onSurfaceDrop).toHaveBeenCalledTimes(2);

    const groupPreventDefault = vi.fn();
    routeWorkbenchDockviewDrop({
      position: "center",
      preventDefault: groupPreventDefault,
      getData: () => ({ panelId: null }),
      panel: undefined,
      group: { id: "group-2" },
    } as unknown as DockviewWillDropEvent, documentModel, onSurfaceDrop);
    expect(groupPreventDefault).toHaveBeenCalledOnce();

    const staleCenterPreventDefault = vi.fn();
    routeWorkbenchDockviewDrop({
      ...makeDrop("center", staleCenterPreventDefault),
      group: { id: "stale-group" },
    } as DockviewWillDropEvent, documentModel, onSurfaceDrop);
    expect(staleCenterPreventDefault).toHaveBeenCalledOnce();

    const staleEdgePreventDefault = vi.fn();
    routeWorkbenchDockviewDrop({
      ...makeDrop("left", staleEdgePreventDefault),
      group: { id: "stale-group" },
    } as DockviewWillDropEvent, documentModel, onSurfaceDrop);
    expect(staleEdgePreventDefault).toHaveBeenCalledOnce();
    expect(onSurfaceDrop).toHaveBeenCalledTimes(2);

    const soleTabSelfEdgePreventDefault = vi.fn();
    routeWorkbenchDockviewDrop({
      position: "right",
      preventDefault: soleTabSelfEdgePreventDefault,
      getData: () => ({ panelId: "surface-2" }),
      panel: undefined,
      group: {
        id: "group-2",
        api: { boundingBox: { left: 0, top: 0, width: 400, height: 400 } },
      },
    } as unknown as DockviewWillDropEvent, documentModel, onSurfaceDrop);
    expect(soleTabSelfEdgePreventDefault).toHaveBeenCalledOnce();
    expect(onSurfaceDrop).toHaveBeenCalledTimes(2);
  });

  it("uses accurate half-pane overlays except for a sole-tab self target", () => {
    const expected = {
      size: { type: "percentage", value: 50 },
      activationSize: { type: "percentage", value: 20 },
      smallWidthBoundary: WORKBENCH_PANE_MINIMUM_WIDTH * 2,
      smallHeightBoundary: WORKBENCH_PANE_MINIMUM_HEIGHT * 2,
    };

    expect(workbenchDockviewDropOverlayModel({ location: "content" })).toEqual(expected);
    expect(workbenchDockviewDropOverlayModel({ location: "header_space" })).toEqual(expected);
    expect(workbenchDockviewDropOverlayModel({ location: "tab" })).toEqual(expected);
    expect(workbenchDockviewDropOverlayModel({ location: "edge" })).toBeUndefined();

    const documentModel = makeTwoGroupDocument();
    expect(workbenchDockviewDropOverlayModel(
      { location: "content" },
      documentModel,
      { surface_id: "surface-2", source_group_id: "group-2", pointer_id: 7 },
      "group-2",
    )).toEqual({
      ...expected,
      activationSize: { type: "percentage", value: 0 },
    });
    expect(workbenchDockviewDropOverlayModel(
      { location: "content" },
      documentModel,
      { surface_id: "surface-2", source_group_id: "group-2", pointer_id: 7 },
      "group-1",
    )).toEqual(expected);
  });

  it("preserves a keyed panel renderer while the canonical model moves it", async () => {
    const initial = makeTwoGroupDocument();
    const onCommand = vi.fn((_command: WorkbenchCommand) => true);
    const renderSurface = (surface: { surface_id: string }) => (
      <div data-testid={`renderer-${surface.surface_id}`} />
    );
    const view = render(
      <DockviewLayoutAdapter
        document={initial}
        render_surface={renderSurface}
        on_command={onCommand}
      />,
    );
    const renderer = await screen.findByTestId("renderer-surface-1");
    onCommand.mockClear();
    const moved = apply(initial, {
      type: "move_surface",
      surface_id: "surface-1",
      group_id: "group-2",
      index: 1,
    });

    view.rerender(
      <DockviewLayoutAdapter
        document={moved}
        render_surface={renderSurface}
        on_command={onCommand}
      />,
    );

    await waitFor(() => expect(screen.getByTestId("renderer-surface-1")).toBe(renderer));
    expect(onCommand).not.toHaveBeenCalled();
  });

  it("reports canonical active-tab and zoom visibility to retained renderers", async () => {
    const first = makeSurface("surface-1", { surface_type: "graph" });
    const second = makeSurface("surface-2", { surface_type: "garden" });
    const initial = makeSingleGroupDocument([first, second]);
    const renderSurface = vi.fn((surface: { surface_id: string }, lifecycle?: { visible: boolean }) => (
      <div data-testid={`visibility-${surface.surface_id}`}>
        {lifecycle?.visible ? "visible" : "hidden"}
      </div>
    ));
    const view = render(
      <DockviewLayoutAdapter
        document={initial}
        render_surface={renderSurface}
        renderer_policy={() => "always"}
      />,
    );

    expect(await screen.findByTestId("visibility-surface-1")).toHaveTextContent("hidden");
    expect(screen.getByTestId("visibility-surface-2")).toHaveTextContent("visible");

    const activated = apply(initial, {
      type: "set_active_surface",
      group_id: "group-1",
      surface_id: "surface-1",
    });
    view.rerender(
      <DockviewLayoutAdapter
        document={activated}
        render_surface={renderSurface}
        renderer_policy={() => "always"}
      />,
    );

    await waitFor(() => expect(screen.getByTestId("visibility-surface-1")).toHaveTextContent("visible"));
    expect(screen.getByTestId("visibility-surface-2")).toHaveTextContent("hidden");

    view.rerender(
      <DockviewLayoutAdapter
        document={activated}
        zoomed_group_id="other-group"
        render_surface={renderSurface}
        renderer_policy={() => "always"}
      />,
    );
    await waitFor(() => expect(screen.getByTestId("visibility-surface-1")).toHaveTextContent("hidden"));
  });

  it("repairs structural topology without remounting keyed panel renderers", async () => {
    const initial = makeMixedDepthThreeDocument();
    const renderSurface = (surface: { surface_id: string }) => (
      <div data-testid={`structural-renderer-${surface.surface_id}`} />
    );
    const view = render(
      <DockviewLayoutAdapter document={initial} render_surface={renderSurface} />,
    );
    const renderer = await screen.findByTestId("structural-renderer-surface-1");
    const changed = apply(initial, { type: "close_group", group_id: "group-3" });

    view.rerender(
      <DockviewLayoutAdapter document={changed} render_surface={renderSurface} />,
    );

    await waitFor(() => expect(screen.getAllByTestId("workbench-group")).toHaveLength(3));
    expect(screen.getByTestId("structural-renderer-surface-1")).toBe(renderer);
  });

  it("renders one active group in safe mode without rewriting the supplied document", () => {
    const documentModel = makeTwoGroupDocument();
    const before = structuredClone(documentModel);

    render(<DockviewLayoutAdapter document={documentModel} safe_mode />);

    expect(screen.getAllByTestId("workbench-group")).toHaveLength(1);
    expect(screen.getByTestId("workbench-group")).toHaveAttribute("data-group-id", "group-2");
    expect(screen.getByTestId("workbench-group")).toHaveAttribute("tabindex", "-1");
    for (const tab of screen.getAllByRole("tab")) {
      const controlledId = tab.getAttribute("aria-controls");
      const controlledPanel = controlledId ? document.getElementById(controlledId) : null;
      expect(controlledPanel).toHaveAttribute("role", "tabpanel");
      expect(controlledPanel).toHaveAttribute("aria-labelledby", tab.id);
    }
    expect(documentModel).toEqual(before);
    expect(documentModel.root.kind).toBe("split");
  });

  it("does not feed model-owned initial activation back into the canonical writer", async () => {
    const onCommand = vi.fn();
    render(<DockviewLayoutAdapter document={makeTwoGroupDocument()} on_command={onCommand} />);

    await waitFor(() => expect(screen.getAllByTestId("workbench-group")).toHaveLength(2));
    await Promise.resolve();
    expect(onCommand).not.toHaveBeenCalled();
  });

  it("emits canonical tab/group commands and keyboard-adjustable separator ratios", async () => {
    const onCommand = vi.fn();
    const onOpenSurface = vi.fn();
    render(
      <DockviewLayoutAdapter
        document={makeTwoGroupDocument()}
        on_command={onCommand}
        on_open_surface={onOpenSurface}
      />,
    );

    const separator = await screen.findByRole("separator", { name: "Resize split split-1" });
    expect(separator).toHaveAttribute("aria-orientation", "vertical");
    expect(separator).toHaveAttribute("aria-valuemin", "10");
    expect(separator).toHaveAttribute("aria-valuemax", "90");
    expect(separator).toHaveAttribute("aria-valuenow", "50");
    expect(separator).toHaveAttribute("data-split-direction", "horizontal");
    fireEvent.keyDown(separator, { key: "ArrowRight" });
    expect(onCommand).toHaveBeenCalledWith({
      type: "set_split_ratio",
      node_id: "split-1",
      ratio: 0.55,
    });

    fireEvent.click(screen.getAllByRole("button", { name: "Open Surface" })[0]);
    expect(onOpenSurface).toHaveBeenCalledWith("group-1");

    fireEvent.pointerDown(screen.getByRole("tab", { name: /agents overview/i }));
    await waitFor(() => expect(onCommand).toHaveBeenCalledWith({
      type: "set_active_surface",
      group_id: "group-1",
      surface_id: "surface-1",
    }));
  });

  it("defers rather than drops user activation during canonical projection", async () => {
    const onCommand = vi.fn();
    render(
      <DockviewLayoutAdapter
        document={makeTwoGroupDocument()}
        on_command={onCommand}
      />,
    );

    fireEvent.pointerDown(screen.getByRole("tab", { name: /agents overview/i }));

    await waitFor(() => expect(onCommand).toHaveBeenCalledWith({
      type: "set_active_surface",
      group_id: "group-1",
      surface_id: "surface-1",
    }));
  });

  it.each(["Delete", "Backspace"])(
    "routes %s on a focused tab through the guarded close boundary",
    async (key) => {
      const onCloseSurface = vi.fn();
      render(
        <DockviewLayoutAdapter
          document={makeTwoGroupDocument()}
          on_close_surface={onCloseSurface}
        />,
      );

      const tab = await screen.findByRole("tab", { name: /agents overview/i });
      tab.focus();
      expect(tab).toHaveFocus();
      expect(fireEvent.keyDown(tab, { key })).toBe(false);

      expect(onCloseSurface).toHaveBeenCalledWith("surface-1");
      expect(screen.getByRole("tab", { name: /agents overview/i })).toBeInTheDocument();
      expect(screen.getByRole("tab", { name: /agent session/i })).toBeInTheDocument();
    },
  );

  it("keeps tab close chrome out of the tab stop model and focuses the successor after close", async () => {
    const first = makeSurface("surface-1", { surface_type: "agents-overview" });
    const second = makeSurface("surface-2", { surface_type: "library" });
    let documentModel = makeSingleGroupDocument([first, second]);
    let view: ReturnType<typeof render>;
    const onCloseSurface = (surfaceId: string): void => {
      documentModel = apply(documentModel, { type: "close_surface", surface_id: surfaceId });
      view.rerender(
        <DockviewLayoutAdapter
          document={documentModel}
          safe_mode
          on_close_surface={onCloseSurface}
        />,
      );
    };
    view = render(
      <DockviewLayoutAdapter
        document={documentModel}
        safe_mode
        on_close_surface={onCloseSurface}
      />,
    );

    const activeTab = screen.getByRole("tab", { name: "Library" });
    expect(activeTab).toHaveAttribute("aria-selected", "true");
    expect(within(activeTab).queryByRole("button")).not.toBeInTheDocument();
    const closeAction = activeTab.querySelector<HTMLElement>("[data-tab-close]");
    expect(closeAction).not.toHaveAttribute("tabindex");
    if (!closeAction) throw new Error("expected pointer close affordance");
    fireEvent.click(closeAction);

    await waitFor(() => expect(screen.getByRole("tab", { name: "Agents Overview" })).toHaveFocus());
  });

  it("focuses an emptied pane after its active tab closes", async () => {
    let documentModel = makeSingleGroupDocument([
      makeSurface("surface-1", { surface_type: "agents-overview" }),
    ]);
    let view: ReturnType<typeof render>;
    const onCloseSurface = (surfaceId: string): void => {
      documentModel = apply(documentModel, { type: "close_surface", surface_id: surfaceId });
      view.rerender(
        <DockviewLayoutAdapter
          document={documentModel}
          safe_mode
          on_close_surface={onCloseSurface}
        />,
      );
    };
    view = render(
      <DockviewLayoutAdapter
        document={documentModel}
        safe_mode
        on_close_surface={onCloseSurface}
      />,
    );

    const activeTab = screen.getByRole("tab", { name: "Agents Overview" });
    const closeAction = activeTab.querySelector<HTMLElement>("[data-tab-close]");
    if (!closeAction) throw new Error("expected pointer close affordance");
    fireEvent.click(closeAction);

    await waitFor(() => expect(screen.getByTestId("workbench-group")).toHaveFocus());
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  });

  it("focuses the canonical surviving active tab after closing a collapsing pane", async () => {
    let documentModel = makeTwoGroupDocument();
    let view: ReturnType<typeof render>;
    const onCloseSurface = (surfaceId: string): void => {
      documentModel = apply(documentModel, { type: "close_surface", surface_id: surfaceId });
      view.rerender(
        <DockviewLayoutAdapter
          document={documentModel}
          safe_mode
          on_close_surface={onCloseSurface}
        />,
      );
    };
    view = render(
      <DockviewLayoutAdapter
        document={documentModel}
        safe_mode
        on_close_surface={onCloseSurface}
      />,
    );

    const closeAction = screen.getByRole("tab", { name: "Agent Session" })
      .querySelector<HTMLElement>("[data-tab-close]");
    if (!closeAction) throw new Error("expected pointer close affordance");
    fireEvent.click(closeAction);

    await waitFor(() => expect(screen.getByRole("tab", { name: "Agents Overview" })).toHaveFocus());
    expect(screen.getByTestId("workbench-group")).toHaveAttribute("data-group-id", "group-1");
  });

  it("routes tab context actions through close, edge-split, and canonical move boundaries", async () => {
    const onCloseSurface = vi.fn();
    const onSurfaceDrop = vi.fn();
    const onCommand = vi.fn(() => true);
    render(
      <DockviewLayoutAdapter
        document={makeTwoGroupDocument()}
        on_close_surface={onCloseSurface}
        on_surface_drop={onSurfaceDrop}
        on_command={onCommand}
      />,
    );

    const tab = await screen.findByRole("tab", { name: /agents overview/i });
    fireEvent.contextMenu(tab, { clientX: 80, clientY: 40 });
    let menu = screen.getByRole("menu", { name: "Agents Overview tab actions" });
    expect(within(menu).getByRole("menuitem", { name: "Close tab" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Split tab right" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Split tab down" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Move to next pane" })).toBeInTheDocument();
    expect(menu).not.toHaveTextContent("group-");

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("menu", { name: "Agents Overview tab actions" })).not.toBeInTheDocument();
    await waitFor(() => expect(tab).toHaveFocus());

    fireEvent.contextMenu(tab, { clientX: 80, clientY: 40 });
    menu = screen.getByRole("menu", { name: "Agents Overview tab actions" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Split tab right" }));
    expect(onSurfaceDrop).toHaveBeenCalledWith("surface-1", "group-1", "right");
    await waitFor(() => expect(tab).toHaveFocus());

    fireEvent.contextMenu(tab, { clientX: 80, clientY: 40 });
    menu = screen.getByRole("menu", { name: "Agents Overview tab actions" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Move to next pane" }));
    expect(onCommand).toHaveBeenCalledWith({
      type: "move_surface",
      surface_id: "surface-1",
      group_id: "group-2",
      index: 1,
    });

    fireEvent.contextMenu(tab, { clientX: 80, clientY: 40 });
    menu = screen.getByRole("menu", { name: "Agents Overview tab actions" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Close tab" }));
    expect(onCloseSurface).toHaveBeenCalledWith("surface-1");
  });

  it("keeps pane chrome compact and puts layout commands in its contextual menu", async () => {
    const onToggleZoom = vi.fn();
    const onSplitGroup = vi.fn();
    const onCloseGroup = vi.fn();
    render(
      <DockviewLayoutAdapter
        document={makeTwoGroupDocument()}
        zoomed_group_id="group-1"
        on_toggle_zoom={onToggleZoom}
        on_split_group={onSplitGroup}
        on_close_group={onCloseGroup}
      />,
    );

    await waitFor(() => expect(screen.getAllByTestId("workbench-group")).toHaveLength(2));
    const group = document.querySelector<HTMLElement>('[data-group-id="group-1"]');
    if (!group) throw new Error("expected first workbench group");
    expect(within(group).getAllByRole("button").map((button) => button.getAttribute("aria-label")))
      .toEqual(["Open Surface", "Pane actions"]);

    const tabs = group.querySelector(".dv-tabs-container");
    const afterTabs = group.querySelector(".dv-left-actions-container");
    const farEdge = group.querySelector(".dv-right-actions-container");
    expect(tabs).not.toBeNull();
    expect(afterTabs).toContainElement(within(group).getByRole("button", { name: "Open Surface" }));
    expect(farEdge).toContainElement(within(group).getByRole("button", { name: "Pane actions" }));
    expect(group.querySelector(".dv-void-container")).toHaveAttribute("data-tauri-drag-region");
    expect(within(group).getByRole("button", { name: "Open Surface" }))
      .not.toHaveAttribute("data-tauri-drag-region");
    expect(within(group).getByRole("button", { name: "Pane actions" }))
      .not.toHaveAttribute("data-tauri-drag-region");

    const paneActions = within(group).getByRole("button", { name: "Pane actions" });
    fireEvent.click(paneActions);
    let menu = screen.getByRole("menu", { name: "Pane actions" });
    expect(within(menu).getByRole("menuitem", { name: "Restore pane" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Split pane right" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Split pane down" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Close pane" })).toBeInTheDocument();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Restore pane" }));
    expect(onToggleZoom).toHaveBeenCalledWith("group-1");
    await waitFor(() => expect(paneActions).toHaveFocus());

    fireEvent.click(within(group).getByRole("button", { name: "Pane actions" }));
    menu = screen.getByRole("menu", { name: "Pane actions" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Split pane down" }));
    expect(onSplitGroup).toHaveBeenCalledWith("group-1", "vertical");

    fireEvent.click(within(group).getByRole("button", { name: "Pane actions" }));
    menu = screen.getByRole("menu", { name: "Pane actions" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Close pane" }));
    expect(onCloseGroup).toHaveBeenCalledWith("group-1");
  });

  it("reprojects canonical activation after an explicit writer rejection", async () => {
    const first = makeSurface("surface-1", { surface_type: "agents-overview" });
    const second = makeSurface("surface-2", { surface_type: "library" });
    const onCommand = vi.fn(() => false);
    render(
      <DockviewLayoutAdapter
        document={makeSingleGroupDocument([first, second])}
        on_command={onCommand}
      />,
    );

    const firstTab = await screen.findByRole("tab", { name: /agents overview/i });
    const secondTab = screen.getByRole("tab", { name: /library/i });
    expect(secondTab).toHaveAttribute("aria-selected", "true");
    fireEvent.pointerDown(firstTab);

    await waitFor(() => expect(onCommand).toHaveBeenCalledWith({
      type: "set_active_surface",
      group_id: "group-1",
      surface_id: "surface-1",
    }));
    await waitFor(() => expect(secondTab).toHaveAttribute("aria-selected", "true"));
    expect(firstTab).toHaveAttribute("aria-selected", "false");
  });

  it("offers contextual previous/next pane actions without exposing internal group IDs", async () => {
    const documentModel = makeMixedDepthThreeDocument();
    const onJoinGroup = vi.fn();
    render(
      <DockviewLayoutAdapter
        document={documentModel}
        on_join_group={onJoinGroup}
      />,
    );

    await waitFor(() => expect(screen.getAllByTestId("workbench-group")).toHaveLength(4));
    expect(workbenchPaneTargets(documentModel.root, "group-1")).toEqual([
      { group_id: "group-3", position: "next" },
    ]);
    const firstGroup = document.querySelector<HTMLElement>('[data-group-id="group-1"]');
    if (!firstGroup) throw new Error("expected first workbench group");
    fireEvent.click(within(firstGroup).getByRole("button", { name: "Pane actions" }));
    const paneMenu = screen.getByRole("menu", { name: "Pane actions" });
    expect(within(paneMenu).getByRole("menuitem", { name: "Merge into next pane" }))
      .toBeInTheDocument();
    expect(paneMenu).not.toHaveTextContent("group-");
    fireEvent.click(within(paneMenu).getByRole("menuitem", { name: "Merge into next pane" }));
    expect(onJoinGroup).toHaveBeenCalledWith("group-1", "group-3");

    const separators = screen.getAllByRole("separator");
    expect(separators).toHaveLength(3);
    expect(separators.map((separator) => separator.getAttribute("data-split-node-id"))).toEqual([
      "split-root",
      "split-left",
      "split-right",
    ]);
    expect(screen.queryAllByRole("slider")).toHaveLength(0);
    expect(separators[0].style.left).toBe("50%");
    expect(separators[1].style.top).toBe("50%");
    expect(separators[2].style.top).toBe("50%");
  });

  it("feeds empty-group activation back through Dockview's active-group event", async () => {
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    let documentModel = makeSingleGroupDocument([surface]);
    documentModel = apply(documentModel, {
      type: "split_group",
      group_id: "group-1",
      new_group_id: "group-2",
      node_id: "split-1",
      direction: "horizontal",
      placement: "after",
    });
    documentModel = apply(documentModel, {
      type: "set_active_surface",
      group_id: "group-1",
      surface_id: "surface-1",
    });
    const onCommand = vi.fn();
    render(<DockviewLayoutAdapter document={documentModel} on_command={onCommand} />);

    const emptyGroup = await screen.findByTestId("workbench-empty-group");
    expect(emptyGroup).toHaveAttribute("data-group-id", "group-2");
    fireEvent.pointerDown(emptyGroup);

    await waitFor(() => expect(onCommand).toHaveBeenCalledWith({
      type: "set_active_surface",
      group_id: "group-2",
      surface_id: null,
    }));
  });
});

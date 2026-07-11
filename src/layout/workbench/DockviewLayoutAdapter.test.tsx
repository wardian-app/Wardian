import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WorkbenchDocumentV1 } from "../../types";
import { applyWorkbenchCommand, type WorkbenchCommand } from "../../features/workbench/workbenchModel";
import { makeSingleGroupDocument, makeSurface } from "../../features/workbench/workbenchTestUtils";
import {
  deriveWorkbenchSplitRatios,
  dispatchWorkbenchAdapterCommand,
  DockviewLayoutAdapter,
  normalizedWorkbenchGeometry,
  planDockviewGroupPlacements,
  projectWorkbenchGroupSizes,
  routeWorkbenchDockviewDrop,
  shouldRecoverUnexpectedPanelRemoval,
  workbenchPaneTargets,
  workbenchSplitRatioCommands,
} from "./DockviewLayoutAdapter";
import type { DockviewApi, DockviewWillDropEvent } from "dockview-react";

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

  it("requests recovery only for canonical panels removed outside projection", () => {
    const documentModel = makeTwoGroupDocument();
    expect(shouldRecoverUnexpectedPanelRemoval(documentModel, "surface-1", false)).toBe(true);
    expect(shouldRecoverUnexpectedPanelRemoval(documentModel, "surface-1", true)).toBe(false);
    expect(shouldRecoverUnexpectedPanelRemoval(documentModel, "missing", false)).toBe(false);
  });

  it("requests one self-heal only when the canonical writer explicitly rejects", () => {
    const rejected = vi.fn(() => false);
    const accepted = vi.fn(() => true);
    const recover = vi.fn();
    const command = { type: "focus_surface", surface_id: "surface-1" } as const;

    expect(dispatchWorkbenchAdapterCommand(command, rejected, recover)).toBe(false);
    expect(recover).toHaveBeenCalledOnce();
    recover.mockClear();
    expect(dispatchWorkbenchAdapterCommand(command, accepted, recover)).toBe(true);
    expect(dispatchWorkbenchAdapterCommand(command, undefined, recover)).toBe(false);
    expect(recover).toHaveBeenCalledOnce();
  });

  it("seeds both sides before subdividing mixed depth-three canonical trees", () => {
    expect(planDockviewGroupPlacements(makeMixedDepthThreeDocument().root)).toEqual([
      { group_id: "group-1" },
      { group_id: "group-2", reference_group_id: "group-1", direction: "right" },
      { group_id: "group-3", reference_group_id: "group-1", direction: "below" },
      { group_id: "group-4", reference_group_id: "group-2", direction: "below" },
    ]);
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
    const onSurfaceDrop = vi.fn();
    const centerPreventDefault = vi.fn();
    const makeDrop = (
      position: "center" | "left",
      preventDefault: () => void,
    ): DockviewWillDropEvent => ({
      position,
      preventDefault,
      getData: () => ({ panelId: "surface-1" }),
      panel: undefined,
      group: { id: "group-2" },
    }) as unknown as DockviewWillDropEvent;

    routeWorkbenchDockviewDrop(makeDrop("center", centerPreventDefault), onSurfaceDrop);
    expect(centerPreventDefault).not.toHaveBeenCalled();
    expect(onSurfaceDrop).not.toHaveBeenCalled();

    const orphanCenterPreventDefault = vi.fn();
    routeWorkbenchDockviewDrop({
      position: "center",
      preventDefault: orphanCenterPreventDefault,
      getData: () => ({ panelId: "surface-1" }),
      panel: undefined,
      group: undefined,
    } as unknown as DockviewWillDropEvent, onSurfaceDrop);
    expect(orphanCenterPreventDefault).toHaveBeenCalledOnce();

    const edgePreventDefault = vi.fn();
    routeWorkbenchDockviewDrop(makeDrop("left", edgePreventDefault), onSurfaceDrop);
    expect(edgePreventDefault).toHaveBeenCalledOnce();
    expect(onSurfaceDrop).toHaveBeenCalledWith("surface-1", "group-2", "left");

    const groupPreventDefault = vi.fn();
    routeWorkbenchDockviewDrop({
      position: "center",
      preventDefault: groupPreventDefault,
      getData: () => ({ panelId: null }),
      panel: undefined,
      group: { id: "group-2" },
    } as unknown as DockviewWillDropEvent, onSurfaceDrop);
    expect(groupPreventDefault).toHaveBeenCalledOnce();
  });

  it("preserves a keyed panel renderer while the canonical model moves it", async () => {
    const initial = makeTwoGroupDocument();
    const renderSurface = (surface: { surface_id: string }) => (
      <div data-testid={`renderer-${surface.surface_id}`} />
    );
    const view = render(
      <DockviewLayoutAdapter document={initial} render_surface={renderSurface} />,
    );
    const renderer = await screen.findByTestId("renderer-surface-1");
    const moved = apply(initial, {
      type: "move_surface",
      surface_id: "surface-1",
      group_id: "group-2",
      index: 1,
    });

    view.rerender(
      <DockviewLayoutAdapter document={moved} render_surface={renderSurface} />,
    );

    await waitFor(() => expect(screen.getByTestId("renderer-surface-1")).toBe(renderer));
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

    fireEvent.click(within(menu).getByRole("menuitem", { name: "Split tab right" }));
    expect(onSurfaceDrop).toHaveBeenCalledWith("surface-1", "group-1", "right");

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
      .toEqual(["Close Agents Overview", "Open Surface", "Pane actions"]);

    fireEvent.click(within(group).getByRole("button", { name: "Pane actions" }));
    let menu = screen.getByRole("menu", { name: "Pane actions" });
    expect(within(menu).getByRole("menuitem", { name: "Restore pane" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Split pane right" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Split pane down" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Close pane" })).toBeInTheDocument();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Restore pane" }));
    expect(onToggleZoom).toHaveBeenCalledWith("group-1");

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

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { createCoreWorkbenchSurfaceRegistry } from "../../features/workbench/coreSurfaceRegistry";
import type { WorkbenchNavigationService } from "../../features/workbench/navigationService";
import { createWorkbenchStore } from "../../features/workbench/useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "../../features/workbench/workbenchTestUtils";
import {
  canSplitWorkbenchGroup,
  WorkbenchHost,
  workbenchEdgeDropCommands,
} from "./WorkbenchHost";

function makeNavigation(overrides: Partial<WorkbenchNavigationService> = {}) {
  return {
    open: vi.fn(),
    open_to_side: vi.fn(),
    focus: vi.fn(),
    close: vi.fn(),
    close_group: vi.fn(),
    reset_workbench: vi.fn(),
    ...overrides,
  } as unknown as WorkbenchNavigationService;
}

function makeTwoPaneDocument() {
  const left = makeSurface("surface-left", { surface_type: "dashboard" });
  const right = makeSurface("surface-right", { surface_type: "queue" });
  const document = makeSingleGroupDocument([left]);
  return {
    ...document,
    root: {
      kind: "split" as const,
      node_id: "split-1",
      direction: "horizontal" as const,
      ratio: 0.5,
      first: { kind: "group" as const, group_id: "group-1" },
      second: { kind: "group" as const, group_id: "group-2" },
    },
    groups: {
      ...document.groups,
      "group-2": {
        group_id: "group-2",
        surface_ids: [right.surface_id],
        active_surface_id: right.surface_id,
      },
    },
    surfaces: { ...document.surfaces, [right.surface_id]: right },
  };
}

describe("WorkbenchHost", () => {
  it("keeps programmatic splits enabled while pane geometry is not yet measurable", () => {
    expect(canSplitWorkbenchGroup(null, "group-1", "horizontal")).toBe(true);
    const root = document.createElement("div");
    expect(canSplitWorkbenchGroup(root, "group-1", "vertical")).toBe(true);
  });

  it("renders the registry presentation icon when its token differs from the surface type", async () => {
    const registry = createCoreWorkbenchSurfaceRegistry();
    registry.register({
      type: "extension-tool",
      icon: "graph",
      title: () => "Extension Tool",
      render_policy: "recreate_from_state",
      open_policy: "singleton",
      runtime_policy: "view_only",
      close_policy: "close_view",
      state_schema_version: 1,
      max_state_bytes: 1024,
      default_state: () => ({}),
      serialize_state: (state) => state,
      restore_state: (_value, version) => version === 1
        ? { ok: true, state: {} }
        : { ok: false, error: "unsupported version" },
      commands: [],
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("extension-1", { surface_type: "extension-tool", state: {} }),
      ]),
    });

    render(<WorkbenchHost store={store} registry={registry} navigation={makeNavigation()} />);

    const tab = await screen.findByRole("tab", { name: "Extension Tool" });
    expect(tab.querySelector('[data-surface-icon="graph"]')).toHaveClass("lucide-network");
  });

  it("reacts to generic presentation invalidation and releases its subscription", async () => {
    let title = "Before descriptor";
    let icon = "graph";
    const listeners = new Set<() => void>();
    const registry = createCoreWorkbenchSurfaceRegistry();
    registry.register({
      type: "reactive-extension",
      icon: "dashboard",
      title: () => title,
      presentation_icon: () => icon,
      presentation_subscribe: (listener) => {
        listeners.add(listener);
        return () => { listeners.delete(listener); };
      },
      render_policy: "recreate_from_state",
      open_policy: "singleton",
      runtime_policy: "view_only",
      close_policy: "close_view",
      state_schema_version: 1,
      max_state_bytes: 1024,
      default_state: () => ({}),
      serialize_state: (state) => state,
      restore_state: (_value, version) => version === 1
        ? { ok: true, state: {} }
        : { ok: false, error: "unsupported version" },
      commands: [],
    });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("reactive-1", { surface_type: "reactive-extension", state: {} }),
      ]),
    });

    const view = render(
      <WorkbenchHost store={store} registry={registry} navigation={makeNavigation()} />,
    );
    const before = await screen.findByRole("tab", { name: "Before descriptor" });
    expect(before.querySelector('[data-surface-icon="graph"]')).toBeInTheDocument();
    expect(listeners).toHaveLength(1);

    act(() => {
      title = "After descriptor";
      icon = "queue";
      for (const listener of listeners) listener();
    });
    const after = await screen.findByRole("tab", { name: "After descriptor" });
    expect(after.querySelector('[data-surface-icon="queue"]')).toBeInTheDocument();

    view.unmount();
    expect(listeners).toHaveLength(0);
  });

  it("opens an inline New Tab by default and replaces it in place after selection", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });
    const createId = vi.fn(() => "new-tab-1");

    render(<WorkbenchHost store={store} create_id={createId} />);
    const addButton = await screen.findByRole("button", { name: "Open Surface" });
    fireEvent.click(addButton);

    const newTab = await screen.findByRole("tab", { name: "New Tab" });
    expect(newTab).toHaveAttribute("aria-selected", "true");
    expect(newTab.querySelector('[data-surface-icon="new-tab"]')).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Choose a surface" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Choose a surface" })).not.toBeInTheDocument();
    expect(store.getState().document.groups["group-1"].surface_ids)
      .toEqual(["surface-1", "new-tab-1"]);

    fireEvent.click(screen.getByRole("button", { name: /Agents:/i }));

    expect(store.getState().document.groups["group-1"].surface_ids)
      .toEqual(["surface-1", "new-tab-1"]);
    expect(store.getState().document.surfaces["new-tab-1"]).toMatchObject({
      surface_id: "new-tab-1",
      surface_type: "agents-overview",
    });
    expect(store.getState().document.recently_closed).toEqual([]);
  });

  it("carries the inline placeholder through Browse all and replaces it from search", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });

    render(<WorkbenchHost store={store} create_id={() => "new-tab-1"} />);
    const addButton = await screen.findByRole("button", { name: "Open Surface" });
    fireEvent.click(addButton);
    fireEvent.click(screen.getByRole("button", { name: "Browse all surfaces" }));

    const searchable = screen.getByRole("dialog", { name: "Open Surface" });
    const queueOption = searchable.querySelector<HTMLElement>('[data-surface-type="queue"]');
    if (!queueOption) throw new Error("Queue surface option missing");
    fireEvent.click(queueOption);

    expect(store.getState().document.groups["group-1"].surface_ids)
      .toEqual(["surface-1", "new-tab-1"]);
    expect(store.getState().document.surfaces["new-tab-1"].surface_type).toBe("queue");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Open Surface" }))
      .not.toBeInTheDocument());
  });

  it("consumes the inline placeholder when Browse all reopens a recently closed surface", async () => {
    const dashboard = makeSurface("surface-1", { surface_type: "dashboard" });
    const closedQueue = makeSurface("queue-closed", { surface_type: "queue" });
    const initial = makeSingleGroupDocument([dashboard]);
    initial.recently_closed = [{
      surface: closedQueue,
      previous_group_id: "group-1",
      previous_index: 1,
    }];
    const store = createWorkbenchStore({ initial_document: initial });

    render(<WorkbenchHost store={store} create_id={() => "new-tab-1"} />);
    fireEvent.click(await screen.findByRole("button", { name: "Open Surface" }));
    fireEvent.click(screen.getByRole("button", { name: "Browse all surfaces" }));
    fireEvent.click(screen.getByRole("option", { name: "Reopen Queue" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Open Surface" }))
      .not.toBeInTheDocument());
    expect(store.getState().document.groups["group-1"].surface_ids)
      .toEqual(["surface-1", "queue-closed"]);
    expect(store.getState().document.surfaces["new-tab-1"]).toBeUndefined();
    expect(store.getState().document.surfaces["queue-closed"]).toEqual(closedQueue);
    expect(store.getState().document.recently_closed).toEqual([]);
  });

  it("opens the searchable list directly when the new tab preference is palette", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });

    render(
      <WorkbenchHost
        store={store}
        navigation={makeNavigation()}
        new_tab_action="palette"
      />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Open Surface" }));

    expect(screen.getByRole("dialog", { name: "Open Surface" })).toBeInTheDocument();
    expect(store.getState().document.groups["group-1"].surface_ids).toEqual(["surface-1"]);
  });

  it("keeps keyboard Quick Open searchable when the new tab preference is home", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });

    render(
      <WorkbenchHost
        store={store}
        navigation={makeNavigation()}
        new_tab_action="home"
      />,
    );
    fireEvent.keyDown(document.body, { key: "p", ctrlKey: true });

    expect(await screen.findByRole("dialog", { name: "Open Surface" })).toBeInTheDocument();
    expect(store.getState().document.groups["group-1"].surface_ids).toEqual(["surface-1"]);
  });

  it("supplies measured pane admission to owned navigation for Open to Side", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });
    const createId = vi.fn(() => "must-not-be-created");

    render(
      <WorkbenchHost
        store={store}
        create_id={createId}
        new_tab_action="palette"
      />,
    );
    const group = await screen.findByTestId("workbench-group");
    vi.spyOn(group, "getBoundingClientRect").mockReturnValue({
      left: 0,
      top: 0,
      right: 199,
      bottom: 500,
      x: 0,
      y: 0,
      width: 199,
      height: 500,
      toJSON: () => ({}),
    });
    fireEvent.click(within(group).getByRole("button", { name: "Open Surface" }));
    fireEvent.click(screen.getByRole("option", { name: "Queue" }), { ctrlKey: true });

    expect(Object.keys(store.getState().document.groups)).toEqual(["group-1"]);
    expect(store.getState().document.groups["group-1"].surface_ids).toEqual(["surface-1"]);
    expect(createId).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Open Surface" })).toBeInTheDocument();
  });

  it("opens and replaces an inline New Tab in the inactive pane whose plus was clicked", async () => {
    const store = createWorkbenchStore({ initial_document: makeTwoPaneDocument() });

    render(<WorkbenchHost store={store} create_id={() => "new-tab-right"} />);
    const rightGroup = (await screen.findAllByTestId("workbench-group"))
      .find((group) => group.dataset.groupId === "group-2");
    if (!rightGroup) throw new Error("right group missing");
    fireEvent.click(within(rightGroup).getByRole("button", { name: "Open Surface" }));
    fireEvent.click(screen.getByRole("button", { name: /Agents:/i }));

    expect(store.getState().document.groups["group-2"].surface_ids)
      .toEqual(["surface-right", "new-tab-right"]);
    expect(store.getState().document.surfaces["new-tab-right"].surface_type)
      .toBe("agents-overview");
  });

  it("keeps Browse all bound to the inactive pane whose plus was clicked", async () => {
    const store = createWorkbenchStore({ initial_document: makeTwoPaneDocument() });

    render(<WorkbenchHost store={store} create_id={() => "new-tab-right"} />);
    const rightGroup = (await screen.findAllByTestId("workbench-group"))
      .find((group) => group.dataset.groupId === "group-2");
    if (!rightGroup) throw new Error("right group missing");
    fireEvent.click(within(rightGroup).getByRole("button", { name: "Open Surface" }));
    fireEvent.click(screen.getByRole("button", { name: "Browse all surfaces" }));
    fireEvent.click(screen.getByRole("option", { name: "Agents" }));

    expect(store.getState().document.groups["group-2"].surface_ids)
      .toEqual(["surface-right", "new-tab-right"]);
    expect(store.getState().document.surfaces["new-tab-right"].surface_type)
      .toBe("agents-overview");
  });

  it("routes tab close keys through the async navigation guard", async () => {
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument([surface]) });
    const close = vi.fn(async () => "cancel" as const);
    const navigation = {
      open: vi.fn(),
      open_to_side: vi.fn(),
      focus: vi.fn(),
      close,
      close_group: vi.fn(),
      reset_workbench: vi.fn(),
    } as unknown as WorkbenchNavigationService;

    render(<WorkbenchHost store={store} navigation={navigation} />);
    const tab = await screen.findByRole("tab", { name: /agents-overview/i });
    tab.focus();
    fireEvent.keyDown(tab, { key: "Delete" });

    await waitFor(() => expect(close).toHaveBeenCalledWith("surface-1"));
    expect(store.getState().document.surfaces["surface-1"]).toBeDefined();
  });

  it("rejects pane-menu, tab-menu, and keyboard splits against the same narrow pane", async () => {
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    const store = createWorkbenchStore({ initial_document: makeSingleGroupDocument([surface]) });
    render(<WorkbenchHost store={store} />);
    const group = await screen.findByTestId("workbench-group");
    const groupBounds = vi.spyOn(group, "getBoundingClientRect").mockReturnValue({
      left: 0,
      top: 0,
      right: 199,
      bottom: 199,
      x: 0,
      y: 0,
      width: 199,
      height: 199,
      toJSON: () => ({}),
    });

    fireEvent.click(within(group).getByRole("button", { name: "Pane actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Split pane right" }));
    expect(Object.keys(store.getState().document.groups)).toHaveLength(1);

    const tab = within(group).getByRole("tab", { name: /agents-overview/i });
    fireEvent.contextMenu(tab, { clientX: 40, clientY: 20 });
    fireEvent.click(screen.getByRole("menuitem", { name: "Split tab down" }));
    expect(Object.keys(store.getState().document.groups)).toHaveLength(1);

    fireEvent.keyDown(tab, { key: "ArrowRight", ctrlKey: true, altKey: true });
    expect(Object.keys(store.getState().document.groups)).toHaveLength(1);

    groupBounds.mockReturnValue({
      left: 0,
      top: 0,
      right: 200,
      bottom: 200,
      x: 0,
      y: 0,
      width: 200,
      height: 200,
      toJSON: () => ({}),
    });
    fireEvent.keyDown(tab, { key: "ArrowRight", ctrlKey: true, altKey: true });
    await waitFor(() => expect(Object.keys(store.getState().document.groups)).toHaveLength(2));
  });

  it("keeps group zoom runtime-only", () => {
    const initialDocument = makeSingleGroupDocument();
    const store = createWorkbenchStore({ initial_document: initialDocument });
    const documentBefore = store.getState().document;

    render(<WorkbenchHost store={store} />);
    fireEvent.click(screen.getByRole("button", { name: "Pane actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Zoom pane" }));

    expect(store.getState().zoomed_group_id).toBe("group-1");
    expect(store.getState().document).toBe(documentBefore);
    expect(screen.getByTestId("workbench-host")).toHaveAttribute(
      "data-zoomed-group-id",
      "group-1",
    );
  });

  it("exposes guarded actions through a shortcut-opened command palette, not a permanent bar", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "agents-overview" }),
      ]),
    });
    const resetWorkbench = vi.fn(async () => "allow" as const);
    const navigation = {
      open: vi.fn(),
      open_to_side: vi.fn(),
      focus: vi.fn(),
      close: vi.fn(),
      close_group: vi.fn(),
      reset_workbench: resetWorkbench,
    } as unknown as WorkbenchNavigationService;

    render(<WorkbenchHost store={store} navigation={navigation} />);
    expect(screen.queryByRole("navigation", { name: "Workbench commands" })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByTestId("workbench-host"), {
      key: "p",
      ctrlKey: true,
      shiftKey: true,
    });
    expect(await screen.findByRole("dialog", { name: "Command Palette" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "Reset Workbench" }));

    await waitFor(() => expect(resetWorkbench).toHaveBeenCalledOnce());
  });

  it("makes every surface inert while a durable reset is pending", () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "agents-overview" }),
      ]),
      durable_token: "opaque-zero",
    });
    render(<WorkbenchHost store={store} />);

    act(() => {
      expect(store.getState().begin_pending_reset(
        "reset-request",
        store.getState().transaction_version,
      )).toBe(true);
    });
    expect(screen.getByTestId("workbench-host")).toHaveAttribute("inert", "");
    expect(screen.getByTestId("workbench-host")).toHaveAttribute("aria-busy", "true");

    act(() => {
      expect(store.getState().fail_pending_save("reset-request", "cancelled")).toBe(true);
    });
    expect(screen.getByTestId("workbench-host")).not.toHaveAttribute("inert");
    expect(screen.getByTestId("workbench-host")).toHaveAttribute("aria-busy", "false");
  });

  it("retains suspended surfaces but recreates state-backed surfaces when hidden", async () => {
    const graph = makeSurface("surface-graph", { surface_type: "graph" });
    const dashboard = makeSurface("surface-dashboard", { surface_type: "dashboard" });
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([graph, dashboard]),
    });

    render(
      <WorkbenchHost
        store={store}
        render_surface={(surface, lifecycle) => (
          <div data-testid={`surface-${surface.surface_id}`}>
            {lifecycle?.visible ? "visible" : "hidden"}
          </div>
        )}
      />,
    );

    expect(await screen.findByTestId("surface-surface-graph")).toHaveTextContent("hidden");
    expect(screen.getByTestId("surface-surface-dashboard")).toHaveTextContent("visible");

    store.getState().apply_commands([{
      type: "set_active_surface",
      group_id: "group-1",
      surface_id: "surface-graph",
    }]);

    await waitFor(() => expect(screen.getByTestId("surface-surface-graph")).toHaveTextContent("visible"));
    await waitFor(() => expect(screen.queryByTestId("surface-surface-dashboard")).not.toBeInTheDocument());
  });

  it("translates edge drops into one canonical split-and-move transaction", () => {
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    const initial = makeSingleGroupDocument([surface]);
    let nextId = 0;
    const commands = workbenchEdgeDropCommands(
      "surface-1",
      "group-1",
      "left",
      (kind) => `${kind}-new-${++nextId}`,
    );

    const store = createWorkbenchStore({ initial_document: initial });
    const applied = store.getState().apply_commands(commands);
    if (!applied.accepted) throw new Error(applied.errors[0]?.message ?? "command rejected");
    const result = store.getState().document;

    expect(commands).toEqual([
      {
        type: "split_group",
        group_id: "group-1",
        new_group_id: "group-new-1",
        node_id: "node-new-2",
        direction: "horizontal",
        placement: "before",
      },
      {
        type: "move_surface",
        surface_id: "surface-1",
        group_id: "group-new-1",
        index: 0,
      },
    ]);
    expect(result.root).toEqual({ kind: "group", group_id: "group-new-1" });
    expect(result.groups).toEqual({
      "group-new-1": {
        group_id: "group-new-1",
        surface_ids: ["surface-1"],
        active_surface_id: "surface-1",
      },
    });
  });
});

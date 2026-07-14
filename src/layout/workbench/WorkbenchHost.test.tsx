import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WorkbenchNavigationService } from "../../features/workbench/navigationService";
import { createWorkbenchStore } from "../../features/workbench/useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "../../features/workbench/workbenchTestUtils";
import { WorkbenchHost, workbenchEdgeDropCommands } from "./WorkbenchHost";

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
  it("opens the visual chooser by default and creates a surface only after selection", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });
    const navigation = makeNavigation();

    render(<WorkbenchHost store={store} navigation={navigation} />);
    const addButton = await screen.findByRole("button", { name: "Open Surface" });
    addButton.focus();
    fireEvent.click(addButton);

    expect(screen.getByRole("dialog", { name: "Choose a surface" })).toBeInTheDocument();
    expect(navigation.open).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByRole("button", { name: /Agents:/i })).toHaveFocus());

    fireEvent.click(screen.getByRole("button", { name: /Agents:/i }));

    expect(navigation.open).toHaveBeenCalledWith({
      surface_type: "agents-overview",
      group_id: "group-1",
    });
    expect(screen.queryByRole("dialog", { name: "Choose a surface" })).not.toBeInTheDocument();
    await waitFor(() => expect(addButton).toHaveFocus());
  });

  it("switches Browse all surfaces from the visual chooser to the searchable list", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });

    render(<WorkbenchHost store={store} navigation={makeNavigation()} />);
    const addButton = await screen.findByRole("button", { name: "Open Surface" });
    addButton.focus();
    fireEvent.click(addButton);
    fireEvent.click(screen.getByRole("button", { name: "Browse all surfaces" }));

    expect(screen.queryByRole("dialog", { name: "Choose a surface" })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Open Surface" })).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Open Surface" }), { key: "Escape" });
    await waitFor(() => expect(addButton).toHaveFocus());
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
    expect(screen.queryByRole("dialog", { name: "Choose a surface" })).not.toBeInTheDocument();
  });

  it("closes the visual chooser on Escape and restores focus", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });

    render(<WorkbenchHost store={store} navigation={makeNavigation()} />);
    const addButton = await screen.findByRole("button", { name: "Open Surface" });
    addButton.focus();
    fireEvent.click(addButton);
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Choose a surface" }), { key: "Escape" });

    expect(screen.queryByRole("dialog", { name: "Choose a surface" })).not.toBeInTheDocument();
    await waitFor(() => expect(addButton).toHaveFocus());
  });

  it("closes the visual chooser from its backdrop", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });

    render(<WorkbenchHost store={store} navigation={makeNavigation()} />);
    const addButton = await screen.findByRole("button", { name: "Open Surface" });
    addButton.focus();
    fireEvent.click(addButton);
    const dialog = screen.getByRole("dialog", { name: "Choose a surface" });
    const backdrop = dialog.parentElement;
    if (!backdrop) throw new Error("visual chooser backdrop missing");
    fireEvent.mouseDown(backdrop);

    expect(screen.queryByRole("dialog", { name: "Choose a surface" })).not.toBeInTheDocument();
    await waitFor(() => expect(addButton).toHaveFocus());
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
    expect(screen.queryByRole("dialog", { name: "Choose a surface" })).not.toBeInTheDocument();
  });

  it.each([
    { key: "p", ctrlKey: true, shiftKey: false, label: "Quick Open" },
    { key: "p", metaKey: true, shiftKey: false, label: "Quick Open with Command" },
    { key: "o", ctrlKey: true, shiftKey: true, label: "Open Surface" },
  ])("switches an open visual chooser to the searchable list for $label", async (shortcut) => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });

    render(<WorkbenchHost store={store} navigation={makeNavigation()} />);
    fireEvent.click(await screen.findByRole("button", { name: "Open Surface" }));
    const visualDialog = screen.getByRole("dialog", { name: "Choose a surface" });

    fireEvent.keyDown(visualDialog, shortcut);

    expect(screen.queryByRole("dialog", { name: "Choose a surface" })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Open Surface" })).toBeInTheDocument();
  });

  it("traps tab focus and blocks workbench shortcuts behind the visual chooser", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });
    const navigation = makeNavigation();

    render(<WorkbenchHost store={store} navigation={navigation} />);
    const addButton = await screen.findByRole("button", { name: "Open Surface" });
    addButton.focus();
    fireEvent.click(addButton);
    const dialog = screen.getByRole("dialog", { name: "Choose a surface" });
    const first = screen.getByRole("button", { name: /Agents:/i });
    const last = screen.getByRole("button", { name: "Browse all surfaces" });

    last.focus();
    fireEvent.keyDown(last, { key: "Tab" });
    expect(first).toHaveFocus();
    fireEvent.keyDown(first, { key: "Tab", shiftKey: true });
    expect(last).toHaveFocus();

    first.focus();
    fireEvent.keyDown(dialog, { key: "w", ctrlKey: true });
    fireEvent.keyDown(dialog, { key: "F6" });

    expect(navigation.close).not.toHaveBeenCalled();
    expect(store.getState().document.surfaces["surface-1"]).toBeDefined();
    expect(first).toHaveFocus();
  });

  it("restores direct Quick Open focus to its invocation after a prior plus session", async () => {
    const store = createWorkbenchStore({
      initial_document: makeSingleGroupDocument([
        makeSurface("surface-1", { surface_type: "dashboard" }),
      ]),
    });

    render(
      <>
        <button type="button">Quick invocation</button>
        <WorkbenchHost store={store} navigation={makeNavigation()} />
      </>,
    );
    const addButton = await screen.findByRole("button", { name: "Open Surface" });
    addButton.focus();
    fireEvent.click(addButton);
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Choose a surface" }), { key: "Escape" });
    await waitFor(() => expect(addButton).toHaveFocus());

    const invocation = screen.getByRole("button", { name: "Quick invocation" });
    invocation.focus();
    fireEvent.keyDown(invocation, { key: "p", ctrlKey: true });
    fireEvent.keyDown(await screen.findByRole("dialog", { name: "Open Surface" }), { key: "Escape" });

    await waitFor(() => expect(invocation).toHaveFocus());
    expect(addButton).not.toHaveFocus();
  });

  it("opens a visual selection in the inactive pane whose plus was clicked", async () => {
    const store = createWorkbenchStore({ initial_document: makeTwoPaneDocument() });
    const navigation = makeNavigation();

    render(<WorkbenchHost store={store} navigation={navigation} />);
    const rightGroup = (await screen.findAllByTestId("workbench-group"))
      .find((group) => group.dataset.groupId === "group-2");
    if (!rightGroup) throw new Error("right group missing");
    fireEvent.click(within(rightGroup).getByRole("button", { name: "Open Surface" }));
    fireEvent.click(screen.getByRole("button", { name: /Agents:/i }));

    expect(navigation.open).toHaveBeenCalledWith({
      surface_type: "agents-overview",
      group_id: "group-2",
    });
  });

  it("keeps Browse all bound to the inactive pane whose plus was clicked", async () => {
    const store = createWorkbenchStore({ initial_document: makeTwoPaneDocument() });
    const navigation = makeNavigation();

    render(<WorkbenchHost store={store} navigation={navigation} />);
    const rightGroup = (await screen.findAllByTestId("workbench-group"))
      .find((group) => group.dataset.groupId === "group-2");
    if (!rightGroup) throw new Error("right group missing");
    fireEvent.click(within(rightGroup).getByRole("button", { name: "Open Surface" }));
    fireEvent.click(screen.getByRole("button", { name: "Browse all surfaces" }));
    fireEvent.click(screen.getByRole("option", { name: "Agents" }));

    expect(navigation.open).toHaveBeenCalledWith({
      surface_type: "agents-overview",
      group_id: "group-2",
    });
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

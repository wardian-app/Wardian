import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { createWorkbenchNavigationService } from "./navigationService";
import { createWorkbenchStore } from "./useWorkbenchStore";
import { makeSurface } from "./workbenchTestUtils";
import {
  OpenSurfaceDialog,
  createCoreWorkbenchSurfaceRegistry,
} from "./OpenSurfaceDialog";

function createNavigationFixture() {
  const store = createWorkbenchStore();
  const registry = createCoreWorkbenchSurfaceRegistry();
  let id = 0;
  const navigation = createWorkbenchNavigationService({
    registry,
    store,
    create_id: (kind) => `${kind}-${++id}`,
  });
  return { navigation, registry, store };
}

describe("OpenSurfaceDialog", () => {
  it("lists only actionable surfaces instead of rendering unavailable controls", () => {
    const fixture = createNavigationFixture();
    render(
      <OpenSurfaceDialog
        open
        group_id="group-1"
        navigation={fixture.navigation}
        registry={fixture.registry}
        on_close={() => {}}
      />,
    );

    expect(screen.getByRole("dialog", { name: "Open Surface" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Open a surface" })).toBeInTheDocument();
    for (const surfaceType of [
      "agents-overview",
      "dashboard",
      "queue",
      "graph",
      "garden",
      "library",
      "workflows",
    ]) {
      expect(document.querySelector(`[role="option"][data-surface-type="${surfaceType}"]`)).not.toBeNull();
    }
    expect(document.querySelector('[role="option"][data-surface-type="agent-session"]'))
      .toBeNull();
    expect(document.querySelector('[role="option"][data-surface-type="file-editor"]'))
      .toBeNull();
    expect(document.querySelector('[role="option"][data-surface-type="browser"]'))
      .toBeNull();
  });

  it("filters and keyboard-navigates one compact option list", () => {
    const fixture = createNavigationFixture();
    render(
      <OpenSurfaceDialog
        open
        group_id="group-1"
        navigation={fixture.navigation}
        registry={fixture.registry}
        on_close={() => {}}
      />,
    );

    const input = screen.getByRole("combobox", { name: "Open a surface" });
    const listbox = screen.getByRole("listbox", { name: "Available surfaces" });
    const options = within(listbox).getAllByRole("option");
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(options[1]).toHaveAttribute("aria-selected", "true");
    fireEvent.change(input, { target: { value: "graph" } });
    expect(within(listbox).getAllByRole("option")).toHaveLength(1);
    expect(within(listbox).getByRole("option", { name: "Graph" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.queryByRole("button", { name: /to Side/ })).toBeNull();
  });

  it("keeps singleton focus authoritative for repeated Open to Side", () => {
    const fixture = createNavigationFixture();
    const onClose = vi.fn();
    const view = render(
      <OpenSurfaceDialog
        open
        group_id="group-1"
        navigation={fixture.navigation}
        registry={fixture.registry}
        on_close={onClose}
      />,
    );

    fireEvent.click(screen.getByRole("option", { name: "Agents" }));
    expect(Object.values(fixture.store.getState().document.surfaces)).toHaveLength(1);
    expect(onClose).toHaveBeenCalledOnce();

    view.rerender(
      <OpenSurfaceDialog
        open
        group_id="group-1"
        navigation={fixture.navigation}
        registry={fixture.registry}
        on_close={onClose}
      />,
    );
    fireEvent.click(screen.getByRole("option", { name: "Agents" }));
    expect(Object.values(fixture.store.getState().document.surfaces)).toHaveLength(1);

    view.rerender(
      <OpenSurfaceDialog
        open
        group_id="group-1"
        navigation={fixture.navigation}
        registry={fixture.registry}
        on_close={onClose}
      />,
    );
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Open a surface" }), {
      key: "Enter",
      ctrlKey: true,
    });
    expect(Object.values(fixture.store.getState().document.surfaces)).toHaveLength(1);
    expect(Object.keys(fixture.store.getState().document.groups)).toHaveLength(1);
  });

  it("enables Agent Session with a resource and exposes recent reopen", () => {
    const fixture = createNavigationFixture();
    const onReopen = vi.fn();
    render(
      <OpenSurfaceDialog
        open
        group_id="group-1"
        resource_key="agent-7"
        navigation={fixture.navigation}
        registry={fixture.registry}
        recently_closed={[{
          surface: makeSurface("closed-queue", { surface_type: "queue" }),
          previous_group_id: "group-1",
          previous_index: 0,
        }]}
        on_reopen_closed={onReopen}
        on_close={() => {}}
      />,
    );

    expect(document.querySelector('[role="option"][data-surface-type="agent-session"]'))
      .not.toBeNull();
    fireEvent.click(screen.getByRole("option", { name: "Agent Session" }));
    expect(Object.values(fixture.store.getState().document.surfaces)[0]).toMatchObject({
      surface_type: "agent-session",
      resource_key: "agent-7",
    });
    fireEvent.click(screen.getByRole("option", { name: "Reopen Queue" }));
    expect(onReopen).toHaveBeenCalledOnce();
  });

  it("rejects a programmatic Agent Session open without a resource", () => {
    const fixture = createNavigationFixture();

    expect(() => fixture.navigation.open({ surface_type: "agent-session" }))
      .toThrow("Agent Session requires a resource_key");
    expect(Object.values(fixture.store.getState().document.surfaces)).toHaveLength(0);
  });

  it("treats a whitespace-only Agent Session resource as unavailable", () => {
    const fixture = createNavigationFixture();
    render(
      <OpenSurfaceDialog
        open
        group_id="group-1"
        resource_key="   "
        navigation={fixture.navigation}
        registry={fixture.registry}
        on_close={() => {}}
      />,
    );

    expect(document.querySelector('[role="option"][data-surface-type="agent-session"]'))
      .toBeNull();
  });

  it("offers only the top recently closed entry because reopen is stack ordered", () => {
    const fixture = createNavigationFixture();
    render(
      <OpenSurfaceDialog
        open
        group_id="group-1"
        navigation={fixture.navigation}
        registry={fixture.registry}
        recently_closed={[
          {
            surface: makeSurface("closed-queue", { surface_type: "queue" }),
            previous_group_id: "group-1",
            previous_index: 0,
          },
          {
            surface: makeSurface("closed-dashboard", { surface_type: "dashboard" }),
            previous_group_id: "group-1",
            previous_index: 1,
          },
        ]}
        on_close={() => {}}
      />,
    );

    expect(screen.getByRole("option", { name: "Reopen Queue" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Reopen Dashboard" })).not.toBeInTheDocument();
  });

  it("restores focus to the launcher trigger after Escape", () => {
    const fixture = createNavigationFixture();
    const onClose = vi.fn();
    const trigger = document.createElement("button");
    trigger.textContent = "Trigger";
    document.body.append(trigger);
    trigger.focus();

    render(
      <OpenSurfaceDialog
        open
        group_id="group-1"
        navigation={fixture.navigation}
        registry={fixture.registry}
        on_close={onClose}
      />,
    );
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Open Surface" }), { key: "Escape" });

    expect(onClose).toHaveBeenCalledOnce();
    expect(trigger).toHaveFocus();
    trigger.remove();
  });

  it("traps Tab focus inside the searchable dialog", () => {
    const fixture = createNavigationFixture();
    render(
      <>
        <button type="button">Underlying terminal control</button>
        <OpenSurfaceDialog
          open
          group_id="group-1"
          navigation={fixture.navigation}
          registry={fixture.registry}
          on_close={() => {}}
        />
      </>,
    );

    const input = screen.getByRole("combobox", { name: "Open a surface" });
    const options = within(
      screen.getByRole("listbox", { name: "Available surfaces" }),
    ).getAllByRole("option");
    const last = options[options.length - 1];
    if (!last) throw new Error("searchable dialog has no final option");

    last.focus();
    expect(fireEvent.keyDown(last, { key: "Tab" })).toBe(false);
    expect(input).toHaveFocus();

    input.focus();
    expect(fireEvent.keyDown(input, { key: "Tab", shiftKey: true })).toBe(false);
    expect(last).toHaveFocus();
    expect(screen.getByRole("button", { name: "Underlying terminal control" }))
      .not.toHaveFocus();
  });
});

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
  it("groups core choices, gates Agent Session by resource, and reserves future types", () => {
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
    expect(screen.getByRole("heading", { name: "Core views" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Sessions" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Reserved" })).toBeInTheDocument();
    for (const surfaceType of [
      "agents-overview",
      "dashboard",
      "queue",
      "graph",
      "garden",
      "library",
      "workflows",
      "agent-session",
      "file-editor",
      "browser",
    ]) {
      expect(document.querySelector(`[role="option"][data-surface-type="${surfaceType}"]`)).not.toBeNull();
    }
    expect(document.querySelector('[role="option"][data-surface-type="agent-session"]'))
      .toHaveAttribute("aria-disabled", "true");
    expect(document.querySelector('[role="option"][data-surface-type="file-editor"]'))
      .toHaveAttribute("aria-disabled", "true");
    expect(document.querySelector('[role="option"][data-surface-type="browser"]'))
      .toHaveAttribute("aria-disabled", "true");
  });

  it("uses roving listbox focus and keeps Open to Side actions outside the composite", () => {
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

    const listbox = screen.getByRole("listbox", { name: "Core views" });
    const options = within(listbox).getAllByRole("option");
    expect(options[0]).toHaveAttribute("tabindex", "0");
    expect(options[1]).toHaveAttribute("tabindex", "-1");
    options[0].focus();
    fireEvent.keyDown(options[0], { key: "ArrowDown" });
    expect(options[1]).toHaveFocus();
    expect(options[0]).toHaveAttribute("tabindex", "-1");
    expect(options[1]).toHaveAttribute("tabindex", "0");
    fireEvent.keyDown(options[1], { key: "End" });
    expect(options[options.length - 1]).toHaveFocus();
    expect(within(listbox).queryByRole("button", { name: /to Side/ })).toBeNull();
    expect(screen.getByRole("group", { name: "Core views Open to Side" }))
      .toContainElement(screen.getByRole("button", { name: "Open Agents Overview to Side" }));
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

    fireEvent.click(screen.getByRole("option", { name: "Agents Overview" }));
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
    fireEvent.click(screen.getByRole("option", { name: "Agents Overview" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Open Agents Overview to Side" }));
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
      .toHaveAttribute("aria-disabled", "false");
    fireEvent.click(screen.getByRole("option", { name: "Agent Session" }));
    expect(Object.values(fixture.store.getState().document.surfaces)[0]).toMatchObject({
      surface_type: "agent-session",
      resource_key: "agent-7",
    });
    fireEvent.click(screen.getByRole("button", { name: "Reopen Queue" }));
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
      .toHaveAttribute("aria-disabled", "true");
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

    expect(screen.getByRole("button", { name: "Reopen Queue" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reopen Dashboard" })).not.toBeInTheDocument();
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
});

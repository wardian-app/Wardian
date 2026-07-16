import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { SurfaceRecoveryPlaceholder } from "./SurfaceRecoveryPlaceholder";
import { makeSurface } from "./workbenchTestUtils";

describe("SurfaceRecoveryPlaceholder", () => {
  it("exposes persisted identity and explicit recovery actions", async () => {
    const onRetry = vi.fn();
    const onReset = vi.fn();
    const onClose = vi.fn();
    const surface = makeSurface("opaque-1", {
      surface_type: "extension.notes",
      resource_key: "note-42",
      state_schema_version: 7,
      state: { opaque: true },
    });

    render(
      <SurfaceRecoveryPlaceholder
        surface={surface}
        error="The surface contribution is not registered."
        on_retry={onRetry}
        on_reset={onReset}
        on_close={onClose}
      />,
    );

    expect(screen.getByRole("heading", { name: "Surface unavailable" })).toBeVisible();
    expect(screen.getByText("extension.notes")).toBeVisible();
    expect(screen.getByText("note-42")).toBeVisible();
    expect(screen.getByText("opaque-1")).toBeVisible();
    expect(screen.getByText("The surface contribution is not registered.")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(onRetry).toHaveBeenCalledOnce());
    expect(screen.getByText(/recovery check completed/i)).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Reset Surface" }));
    await waitFor(() => expect(onReset).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });

  it("offers rebind only when the caller supplies meaningful resources", async () => {
    const onRebind = vi.fn();
    render(
      <SurfaceRecoveryPlaceholder
        surface={makeSurface("session-1", {
          surface_type: "agent-session",
          resource_key: "missing-agent",
        })}
        error="The stored Agent Session state is invalid."
        on_retry={vi.fn()}
        on_reset={vi.fn()}
        on_close={vi.fn()}
        rebind_options={[
          { resource_key: "agent-1", label: "First Agent" },
          { resource_key: "agent-2", label: "Second Agent" },
        ]}
        on_rebind={onRebind}
      />,
    );

    fireEvent.change(screen.getByLabelText("Rebind to"), { target: { value: "agent-2" } });
    fireEvent.click(screen.getByRole("button", { name: "Rebind" }));
    await waitFor(() => expect(onRebind).toHaveBeenCalledWith("agent-2"));
  });
});

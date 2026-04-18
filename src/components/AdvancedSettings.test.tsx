import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AdvancedSettings } from "./AdvancedSettings";

describe("AdvancedSettings", () => {
  it("shows the OpenCode agent field for the OpenCode provider", () => {
    const updateField = vi.fn();

    render(
      <AdvancedSettings
        config={{ provider: "opencode" }}
        updateField={updateField}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Advanced Settings" }));

    const agentInput = screen.getByLabelText("OpenCode Agent");
    fireEvent.change(agentInput, { target: { value: "build" } });

    expect(updateField).toHaveBeenCalledWith("opencode_agent", "build");
  });

  it("edits regular session persistence from advanced settings", () => {
    const updateField = vi.fn();

    render(
      <AdvancedSettings
        config={{ provider: "claude", session_persistence: "default" }}
        updateField={updateField}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Advanced Settings" }));
    fireEvent.change(screen.getByLabelText("Regular Session Resume"), {
      target: { value: "fresh" },
    });

    expect(updateField).toHaveBeenCalledWith("session_persistence", "fresh");
  });

  it("places regular session resume outside provider parameters", () => {
    const updateField = vi.fn();
    const { container } = render(
      <AdvancedSettings
        config={{ provider: "claude", session_persistence: "default" }}
        updateField={updateField}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Advanced Settings" }));

    const text = container.textContent ?? "";
    expect(text.indexOf("Regular Session Resume")).toBeGreaterThanOrEqual(0);
    expect(text.indexOf("Provider Parameters")).toBeGreaterThanOrEqual(0);
    expect(text.indexOf("Regular Session Resume")).toBeLessThan(
      text.indexOf("Provider Parameters"),
    );
  });
});

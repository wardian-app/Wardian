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

    expect(updateField).toHaveBeenCalledWith("provider_config", {
      type: "opencode",
      agent: "build",
    });
  });

  it("writes Codex sandbox settings to nested provider config", () => {
    const updateField = vi.fn();

    render(
      <AdvancedSettings
        config={{ provider: "codex", provider_config: { type: "codex" } }}
        updateField={updateField}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Advanced Settings" }));
    fireEvent.change(screen.getByLabelText("Sandbox Mode"), {
      target: { value: "workspace-write" },
    });

    expect(updateField).toHaveBeenCalledWith("provider_config", {
      type: "codex",
      sandbox_mode: "workspace-write",
    });
  });

  it("only renders controls for the selected provider", () => {
    const updateField = vi.fn();

    render(
      <AdvancedSettings
        config={{ provider: "claude", provider_config: { type: "claude" } }}
        updateField={updateField}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Advanced Settings" }));

    expect(screen.getByLabelText("Permission Mode")).toBeInTheDocument();
    expect(screen.queryByLabelText("Sandbox Mode")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("OpenCode Agent")).not.toBeInTheDocument();
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

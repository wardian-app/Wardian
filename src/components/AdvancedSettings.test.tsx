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
});

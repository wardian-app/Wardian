import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ConfigureAgentPanel } from "./ConfigureAgentPanel";
import type { AgentConfig } from "../../types";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn(),
}));

vi.mock("../library/ManageSkills", () => ({
  ManageSkills: () => <div data-testid="manage-skills" />,
}));

const invokeMock = vi.mocked(invoke);
const writeTextMock = vi.mocked(writeText);

const baseAgent: AgentConfig = {
  session_id: "agent-1",
  session_name: "Alpha",
  agent_class: "Coder",
  folder: "D:/Development/Wardian",
  is_off: true,
  provider: "codex",
  codex_sandbox_mode: "workspace-write",
};

const classes = [{ name: "Coder", description: "", is_default: true }];

describe("ConfigureAgentPanel", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    writeTextMock.mockReset();
    vi.spyOn(window, "alert").mockImplementation(() => {});
  });

  it("normalizes legacy flat provider fields before saving", async () => {
    const user = userEvent.setup();

    render(
      <ConfigureAgentPanel
        agentId="agent-1"
        agents={[baseAgent]}
        agentClasses={classes}
        telemetry={{}}
        onSaved={() => {}}
        onBackToSpawn={() => {}}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Advanced Settings" }));
    expect(screen.getByLabelText("Sandbox Mode")).toHaveValue("workspace-write");

    await user.click(screen.getByRole("button", { name: "Save Changes" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("update_agent_config", {
        newConfig: expect.objectContaining({
          provider: "codex",
          provider_config: expect.objectContaining({
            type: "codex",
            sandbox_mode: "workspace-write",
          }),
        }),
      });
    });
  });

  it("shows and copies the Wardian agent ID instead of the provider resume ID", async () => {
    const user = userEvent.setup();
    const agent = {
      ...baseAgent,
      resume_session: "provider-thread-1",
    };

    render(
      <ConfigureAgentPanel
        agentId="agent-1"
        agents={[agent]}
        agentClasses={classes}
        telemetry={{}}
        onSaved={() => {}}
        onBackToSpawn={() => {}}
      />,
    );

    expect(screen.getByText("Agent ID")).toBeInTheDocument();
    expect(screen.getByDisplayValue("agent-1")).toBeInTheDocument();
    expect(screen.queryByDisplayValue("provider-thread-1")).not.toBeInTheDocument();

    await user.click(screen.getAllByRole("button", { name: "Copy" })[0]);

    expect(writeTextMock).toHaveBeenCalledWith("agent-1");
  });
});

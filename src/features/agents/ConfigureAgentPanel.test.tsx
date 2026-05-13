import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
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
});

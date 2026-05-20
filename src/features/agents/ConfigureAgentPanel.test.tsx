import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ConfigureAgentPanel } from "./ConfigureAgentPanel";
import type { AgentConfig, ProviderReadiness, UserFacingProviderName } from "../../types";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn(),
}));

vi.mock("../library/ManageSkills", () => ({
  ManageSkills: () => <div data-testid="manage-skills" />,
}));

const invokeMock = vi.mocked(invoke);
const writeTextMock = vi.mocked(writeText);

const readiness = (
  provider: UserFacingProviderName,
  available: boolean,
): ProviderReadiness => ({
  provider,
  display_name: provider === "opencode" ? "OpenCode" : provider === "antigravity" ? "antigravity" : `${provider[0].toUpperCase()}${provider.slice(1)}`,
  available,
  executable: available ? provider : null,
  reason: available ? null : `The ${provider} command was not found.`,
});

const allProvidersReady: ProviderReadiness[] = [
  readiness("claude", true),
  readiness("codex", true),
  readiness("gemini", true),
  readiness("antigravity", true),
  readiness("opencode", true),
];

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

const mockConfigureInvokes = (providerReadiness = allProvidersReady) => {
  invokeMock.mockImplementation(async (command) => {
    if (command === "list_provider_readiness") return providerReadiness;
    if (command === "update_agent_config") return null;
    if (command === "resolve_system_include_directories") return [];
    return null;
  });
};

describe("ConfigureAgentPanel", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    writeTextMock.mockReset();
    vi.spyOn(window, "alert").mockImplementation(() => {});
    mockConfigureInvokes();
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

  it("labels unavailable providers and blocks saving stale unavailable provider state", async () => {
    const user = userEvent.setup();
    mockConfigureInvokes([
      readiness("claude", true),
      readiness("codex", false),
      readiness("gemini", true),
      readiness("opencode", true),
    ]);

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

    expect(await screen.findByRole("option", { name: "Codex - not installed" })).toBeDisabled();
    expect(screen.queryByText(/Only provider CLIs found on this machine are selectable/i)).not.toBeInTheDocument();
    const saveButton = screen.getByRole("button", { name: "Save Changes" });
    expect(saveButton).toBeDisabled();

    await user.click(saveButton);

    expect(invokeMock).not.toHaveBeenCalledWith("update_agent_config", expect.anything());
  });

  it("saves after switching an unavailable current provider to an available one", async () => {
    const user = userEvent.setup();
    mockConfigureInvokes([
      readiness("claude", true),
      readiness("codex", false),
      readiness("gemini", true),
      readiness("opencode", true),
    ]);

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

    await user.selectOptions(await screen.findByLabelText("Provider Engine"), "claude");
    await user.click(screen.getByRole("button", { name: "Save Changes" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("update_agent_config", {
        newConfig: expect.objectContaining({
          provider: "claude",
          provider_config: { type: "claude" },
        }),
      });
    });
  });

  it("keeps saving available when provider readiness cannot be checked", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_provider_readiness") throw new Error("readiness unavailable");
      if (command === "update_agent_config") return null;
      if (command === "resolve_system_include_directories") return [];
      return null;
    });
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

    expect(await screen.findByText("Unable to check provider readiness.")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Codex" })).toBeInTheDocument();
    const saveButton = screen.getByRole("button", { name: "Save Changes" });
    expect(saveButton).toBeEnabled();

    await user.click(saveButton);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("update_agent_config", {
        newConfig: expect.objectContaining({
          provider: "codex",
          provider_config: expect.objectContaining({ type: "codex" }),
        }),
      });
    });
  });
});

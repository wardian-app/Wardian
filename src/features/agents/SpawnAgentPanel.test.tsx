import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SpawnAgentPanel } from "./SpawnAgentPanel";
import { useOnboardingStore } from "../../store/useOnboardingStore";
import { useSettingsStore } from "../../store/useSettingsStore";
import type { ProviderReadiness, UserFacingProviderName } from "../../types";

const openMock = vi.mocked(open);
const invokeMock = vi.mocked(invoke);

const readiness = (
  provider: UserFacingProviderName,
  available: boolean,
): ProviderReadiness => ({
  provider,
  display_name: provider === "opencode" ? "OpenCode" : `${provider[0].toUpperCase()}${provider.slice(1)}`,
  available,
  executable: available ? provider : null,
  reason: available ? null : `The ${provider} command was not found.`,
});

const allProvidersReady: ProviderReadiness[] = [
  readiness("claude", true),
  readiness("codex", true),
  readiness("gemini", true),
  readiness("opencode", true),
];

const spawnResponse = {
  session_id: "agent-1",
  session_name: "Generalist-1",
  agent_class: "Generalist",
  folder: "",
  is_off: false,
};

const mockSpawnInvokes = (providerReadiness = allProvidersReady) => {
  invokeMock.mockImplementation(async (command) => {
    if (command === "list_provider_readiness") return providerReadiness;
    if (command === "validate_directory_path") return true;
    if (command === "spawn_agent") return spawnResponse;
    return null;
  });
};

describe("SpawnAgentPanel", () => {
  beforeEach(() => {
    openMock.mockReset();
    invokeMock.mockReset();
    useOnboardingStore.setState({
      dismissedHintIds: [],
      hintsLoaded: true,
    });
    useSettingsStore.setState({ default_provider: "auto" });
    mockSpawnInvokes();
  });

  it("lists OpenCode as a provider option", async () => {
    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    const providerSelect = screen.getByTestId("spawn-provider");
    expect(await screen.findByRole("option", { name: "OpenCode" })).toBeInTheDocument();
    expect(providerSelect).toHaveTextContent("OpenCode");
  });

  it("shows dismissible provider and first-run help before spawning", () => {
    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    expect(screen.getByText("First agent checklist")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /first-run guide/i })).toHaveAttribute(
      "href",
      "https://docs.wardian.org/guide/getting-started",
    );
    expect(screen.getByRole("link", { name: /provider runtimes/i })).toHaveAttribute(
      "href",
      "https://docs.wardian.org/providers",
    );
  });

  it("sets the workspace path from the native folder picker", async () => {
    openMock.mockResolvedValue("C:\\projects\\picked-app");
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Choose workspace folder" }));

    expect(openMock).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "Choose workspace folder",
    });
    expect(screen.getByTestId("spawn-workspace-path")).toHaveValue("C:\\projects\\picked-app");
  });

  it("keeps the current workspace path when the folder picker is cancelled", async () => {
    openMock.mockResolvedValue(null);
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    const workspacePath = screen.getByTestId("spawn-workspace-path");
    await user.type(workspacePath, "C:\\projects\\typed-app");
    await user.click(screen.getByRole("button", { name: "Choose workspace folder" }));

    expect(workspacePath).toHaveValue("C:\\projects\\typed-app");
  });

  it("submits a blank name so the backend can generate one", async () => {
    const user = userEvent.setup();
    const onSpawned = vi.fn();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={onSpawned}
      />,
    );

    await user.click(screen.getByTestId("spawn-submit"));

    expect(invokeMock).toHaveBeenCalledWith("spawn_agent", {
      req: expect.objectContaining({
        sessionName: "",
        agentClass: "Generalist",
      }),
    });
    expect(onSpawned).toHaveBeenCalled();
  });

  it("initializes the config override with a matching provider config", async () => {
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    await screen.findByRole("option", { name: "Codex" });
    await user.selectOptions(screen.getByTestId("spawn-provider"), "codex");
    await user.click(screen.getByTestId("spawn-submit"));

    expect(invokeMock).toHaveBeenCalledWith("spawn_agent", {
      req: expect.objectContaining({
        configOverride: expect.objectContaining({
          provider: "codex",
          provider_config: { type: "codex" },
        }),
      }),
    });
  });

  it("clears custom args when changing providers", async () => {
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    await screen.findByRole("option", { name: "OpenCode" });
    await user.click(screen.getByRole("button", { name: "Advanced Settings" }));
    await user.type(screen.getByLabelText("Custom Arguments"), "--old-provider-flag");
    await user.selectOptions(screen.getByTestId("spawn-provider"), "opencode");
    await user.click(screen.getByTestId("spawn-submit"));

    expect(invokeMock).toHaveBeenCalledWith("spawn_agent", {
      req: expect.objectContaining({
        configOverride: expect.objectContaining({
          provider: "opencode",
          provider_config: { type: "opencode" },
          custom_args: undefined,
        }),
      }),
    });
  });

  it("blocks explicit names with spaces", async () => {
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    await user.type(screen.getByTestId("spawn-agent-name"), " Coder ");
    await user.click(screen.getByTestId("spawn-submit"));

    expect(invokeMock).not.toHaveBeenCalledWith("spawn_agent", expect.anything());
    expect(screen.getByText(/Names must be alphanumeric/)).toBeInTheDocument();
  });

  it("disables missing providers and explains provider readiness", async () => {
    mockSpawnInvokes([
      readiness("claude", true),
      readiness("codex", false),
      readiness("gemini", false),
      readiness("opencode", true),
    ]);

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    expect(await screen.findByRole("option", { name: "Codex - not installed" })).toBeDisabled();
    expect(screen.getByRole("option", { name: "Gemini - not installed" })).toBeDisabled();
    expect(screen.queryByText(/Only provider CLIs found on this machine are selectable/i)).not.toBeInTheDocument();
  });

  it("falls back when the saved default provider is unavailable", async () => {
    useSettingsStore.setState({ default_provider: "codex" });
    mockSpawnInvokes([
      readiness("claude", true),
      readiness("codex", false),
      readiness("gemini", true),
      readiness("opencode", true),
    ]);

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId("spawn-provider")).toHaveValue("claude");
    });
    expect(await screen.findByText(/Default provider Codex is not installed\. Using Claude\./)).toBeInTheDocument();
  });

  it("disables Initialize when no provider CLI is available", async () => {
    mockSpawnInvokes([
      readiness("claude", false),
      readiness("codex", false),
      readiness("gemini", false),
      readiness("opencode", false),
    ]);
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    const submit = await screen.findByTestId("spawn-submit");
    expect(submit).toBeDisabled();
    await user.click(submit);

    expect(invokeMock).not.toHaveBeenCalledWith("spawn_agent", expect.anything());
  });

  it("submits the configured default provider when it is available", async () => {
    useSettingsStore.setState({ default_provider: "codex" });
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId("spawn-provider")).toHaveValue("codex");
    });
    await user.click(screen.getByTestId("spawn-submit"));

    expect(invokeMock).toHaveBeenCalledWith("spawn_agent", {
      req: expect.objectContaining({
        configOverride: expect.objectContaining({
          provider: "codex",
          provider_config: { type: "codex" },
        }),
      }),
    });
  });
});

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
  readiness("antigravity", true),
  readiness("opencode", true),
];

const spawnResponse = {
  session_id: "agent-1",
  session_name: "Generalist-alpha",
  agent_class: "Generalist",
  folder: "C:/workspace",
  is_off: false,
};

const mockSpawnInvokes = (providerReadiness = allProvidersReady) => {
  invokeMock.mockImplementation(async (command, args) => {
    if (command === "list_provider_readiness") return providerReadiness;
    if (command === "list_provider_model_catalog") {
      return {
        provider: "claude",
        version: "2.1.220",
        source: "provider_aliases",
        refresh_error: null,
        models: [],
      };
    }
    if (command === "validate_directory_path") return true;
    if (command === "get_generated_agent_name") {
      const agentClass = (args as { agentClass?: string } | undefined)?.agentClass ?? "Generalist";
      return `${agentClass}-alpha`;
    }
    if (command === "spawn_agent") {
      const sessionName = ((args as { req?: { sessionName?: string } } | undefined)?.req?.sessionName) ?? "";
      if (sessionName.includes(" ")) {
        throw new Error("Agent name may contain only letters, numbers, underscores, or hyphens; spaces are not allowed.");
      }
      return spawnResponse;
    }
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
    useSettingsStore.setState({
      default_provider: "auto",
      default_workspace: "C:/workspace",
      shell_settings_loaded: true,
    });
    mockSpawnInvokes();
  });

  it("shows portable guidance without treating the placeholder as a workspace", async () => {
    useSettingsStore.setState({ default_workspace: "" });
    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    const workspace = screen.getByTestId("spawn-workspace-path");
    expect(workspace).toHaveAttribute("placeholder", "Enter workspace path");
    expect(workspace).toHaveValue("");
    expect(screen.getByTestId("spawn-submit")).toBeDisabled();
    expect(invokeMock).not.toHaveBeenCalledWith("validate_directory_path", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("spawn_agent", expect.anything());
  });

  it.each(["/projects/my-app", "C:\\projects\\my-app"])(
    "validates and submits the entered workspace unchanged: %s",
    async (folder) => {
      useSettingsStore.setState({ default_workspace: "" });
      const user = userEvent.setup();
      const onSpawned = vi.fn();
      render(
        <SpawnAgentPanel
          agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
          onSpawned={onSpawned}
        />,
      );

      await user.type(screen.getByTestId("spawn-workspace-path"), folder);
      await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("validate_directory_path", { path: folder }));
      await user.click(screen.getByTestId("spawn-submit"));
      expect(invokeMock).toHaveBeenCalledWith("spawn_agent", {
        req: expect.objectContaining({ folder }),
      });
      expect(onSpawned).toHaveBeenCalledWith(spawnResponse);
      expect(screen.getByTestId("spawn-workspace-path")).toHaveValue("");
      expect(screen.getByTestId("spawn-workspace-path")).toHaveAttribute("placeholder", "Enter workspace path");
    },
  );

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

  it("lists Antigravity as a provider option", async () => {
    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    const providerSelect = screen.getByTestId("spawn-provider");
    expect(await screen.findByRole("option", { name: "Antigravity" })).toBeInTheDocument();
    expect(providerSelect).toHaveTextContent("Antigravity");
  });

  it("shows dismissible provider and first-run help before spawning", async () => {
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
    expect(screen.queryByText("Provider aliases follow the installed CLI’s current model mapping.")).not.toBeInTheDocument();
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

  it("prefills the default workspace and asks Rust for the class-specific generated name", async () => {
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[
          { name: "Generalist", description: "", is_default: true },
          { name: "Reviewer", description: "", is_default: true },
        ]}
        onSpawned={() => {}}
      />,
    );

    expect(screen.getByTestId("spawn-workspace-path")).toHaveValue("C:/workspace");
    expect(await screen.findByPlaceholderText("Generalist-alpha")).toBeInTheDocument();

    await user.selectOptions(screen.getByTestId("spawn-agent-class"), "Reviewer");

    expect(await screen.findByPlaceholderText("Reviewer-alpha")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("get_generated_agent_name", {
      agentClass: "Reviewer",
    });
  });

  it("disables spawning until a workspace is available", async () => {
    useSettingsStore.setState({ default_workspace: "" });

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    expect(await screen.findByTestId("spawn-submit")).toBeDisabled();
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
    await user.clear(workspacePath);
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
        sessionName: "Generalist-alpha",
        agentClass: "Generalist",
        folder: "C:/workspace",
      }),
    });
    expect(onSpawned).toHaveBeenCalledWith(spawnResponse);
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
    vi.spyOn(window, "alert").mockImplementation(() => {});

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    await user.type(screen.getByTestId("spawn-agent-name"), " Coder ");
    await user.click(screen.getByTestId("spawn-submit"));

    expect(invokeMock).toHaveBeenCalledWith("spawn_agent", expect.anything());
    await waitFor(() => {
      expect(screen.getByText(/spaces are not allowed/i)).toBeInTheDocument();
    });
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

  it("disables Spawn Agent when no provider CLI is available", async () => {
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

  it("keeps spawning available when provider readiness cannot be checked", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_provider_readiness") throw new Error("readiness unavailable");
      if (command === "validate_directory_path") return true;
      if (command === "spawn_agent") return spawnResponse;
      return null;
    });
    const user = userEvent.setup();
    const onSpawned = vi.fn();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={onSpawned}
      />,
    );

    expect(await screen.findByText("Unable to check provider readiness.")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Claude" })).toBeInTheDocument();
    const submit = screen.getByTestId("spawn-submit");
    expect(submit).toBeEnabled();

    await user.click(submit);

    expect(invokeMock).toHaveBeenCalledWith("spawn_agent", {
      req: expect.objectContaining({
        configOverride: expect.objectContaining({
          provider: "claude",
          provider_config: {
            type: "claude",
            permission_mode: "bypassPermissions",
          },
        }),
      }),
    });
    expect(onSpawned).toHaveBeenCalled();
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

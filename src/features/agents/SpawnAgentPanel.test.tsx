import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SpawnAgentPanel } from "./SpawnAgentPanel";

const openMock = vi.mocked(open);
const invokeMock = vi.mocked(invoke);

describe("SpawnAgentPanel", () => {
  beforeEach(() => {
    openMock.mockReset();
    invokeMock.mockReset();
  });

  it("lists OpenCode as a provider option", () => {
    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    const providerSelect = screen.getByTestId("spawn-provider");
    expect(screen.getByRole("option", { name: "OpenCode" })).toBeInTheDocument();
    expect(providerSelect).toHaveTextContent("OpenCode");
  });

  it("sets the workspace path from the native folder picker", async () => {
    openMock.mockResolvedValue("C:\\projects\\picked-app");
    invokeMock.mockResolvedValue(true);
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
    invokeMock.mockResolvedValue(true);
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
    invokeMock.mockResolvedValue({
      session_id: "agent-1",
      session_name: "Generalist-1",
      agent_class: "Generalist",
      folder: "",
      is_off: false,
    });
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
    invokeMock.mockResolvedValue({
      session_id: "agent-1",
      session_name: "Generalist-1",
      agent_class: "Generalist",
      folder: "",
      is_off: false,
    });
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

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
    invokeMock.mockResolvedValue({
      session_id: "agent-1",
      session_name: "Generalist-1",
      agent_class: "Generalist",
      folder: "",
      is_off: false,
    });
    const user = userEvent.setup();

    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

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
});

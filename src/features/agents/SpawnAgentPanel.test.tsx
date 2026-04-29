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
});

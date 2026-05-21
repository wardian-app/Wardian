import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsModal } from "./SettingsModal";
import { useAppUpdate } from "./useAppUpdate";
import { useSettingsStore } from "../../store/useSettingsStore";
import type { AppUpdateState } from "./useAppUpdate";

vi.mock("./useAppUpdate", () => ({
  useAppUpdate: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockWriteText = vi.mocked(writeText);
const mockUseAppUpdate = vi.mocked(useAppUpdate);

const appUpdateState = (overrides: Partial<AppUpdateState> = {}): AppUpdateState => ({
  currentVersion: "0.3.6",
  availableUpdate: null,
  status: "up-to-date",
  errorMessage: "",
  updatesEnabled: true,
  updateEligibilityReason: "",
  updateChannel: "stable",
  downloadedBytes: 0,
  contentLength: null,
  progressPercent: null,
  checkNow: vi.fn(),
  downloadAndInstall: vi.fn(),
  relaunchApp: vi.fn(),
  ...overrides,
});

describe("SettingsModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    mockUseAppUpdate.mockReturnValue(appUpdateState());
    useSettingsStore.setState({
      theme: "dark",
      autoPatchGemini: false,
      terminalFontSize: 14,
      terminalFontFamily: "",
      shell_id: "auto",
      custom_executable: "",
      custom_args: "",
      agent_session_persistence: "resume",
      default_provider: "auto",
      codex_runtime_policy: {
        sandbox_mode: "workspace-write",
        approval_policy: "on-request",
        full_auto: false,
      },
      available_shells: [],
      app_settings_loaded: true,
      shell_settings_loaded: true,
      shells_loaded: true,
    });

    mockInvoke.mockImplementation(async (command, args) => {
      switch (command) {
        case "save_app_settings":
          return (args as { settings?: unknown } | undefined)?.settings;
        case "save_shell_settings":
          return (args as { settings?: unknown } | undefined)?.settings;
        case "run_gemini_patch":
          return "ok";
        case "get_settings_folder_path":
          return "C:/Users/test/.wardian/settings";
        case "reveal_in_explorer":
          return null;
        default:
          return null;
      }
    });
  });

  it("renders Codex-style category navigation and concise setting details", () => {
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    const dialog = screen.getByRole("dialog", { name: "Settings" });
    expect(within(dialog).getByRole("button", { name: "General" })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Agent Runtime" }));
    expect(screen.getByText("Auto prefers Claude when available.")).toBeInTheDocument();
  });

  it("filters settings by search text across labels and details", () => {
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.change(screen.getByPlaceholderText("Search settings"), {
      target: { value: "codex" },
    });

    expect(screen.getByText("Sandbox")).toBeInTheDocument();
    expect(screen.queryByText("Theme")).not.toBeInTheDocument();
  });

  it("groups Codex-specific controls inside Agent Runtime", () => {
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Agent Runtime" }));

    const agentRuntime = screen.getByRole("region", { name: "Agent Runtime" });
    expect(within(agentRuntime).getByRole("heading", { name: "Codex" })).toBeInTheDocument();
    expect(within(agentRuntime).getByText("Sandbox")).toBeInTheDocument();
    expect(within(agentRuntime).getByText("Approval")).toBeInTheDocument();
    expect(within(agentRuntime).getByText("Autonomous mode")).toBeInTheDocument();
  });

  it("shows Codex autonomous mode as an explicit off/on selection", () => {
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Agent Runtime" }));

    expect(screen.getByLabelText("Codex autonomous mode")).toHaveValue("false");
    expect(screen.getByRole("option", { name: "On: bypass approvals and sandbox" })).toBeInTheDocument();
  });

  it("resets app settings to defaults through the backend app settings file", async () => {
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    fireEvent.click(screen.getByRole("button", { name: "Reset Theme" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_app_settings", {
        settings: expect.objectContaining({
          schema_version: 2,
          overrides: expect.objectContaining({
            theme: "system",
          }),
        }),
      });
    });
    expect(useSettingsStore.getState().theme).toBe("system");
  });

  it("saves runtime settings with backend validation", async () => {
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Agent Runtime" }));
    fireEvent.change(screen.getByLabelText("Default provider"), {
      target: { value: "codex" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save Agent Runtime" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_shell_settings", {
        settings: expect.objectContaining({
          schema_version: 2,
          overrides: expect.objectContaining({
            default_provider: "codex",
          }),
        }),
      });
    });
  });

  it("shows and saves custom shell details from the terminal category", async () => {
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Terminal" }));
    fireEvent.change(screen.getByLabelText("Shell / Interpreter"), {
      target: { value: "custom" },
    });
    fireEvent.change(screen.getByLabelText("Custom shell executable"), {
      target: { value: "C:/Tools/custom-shell.exe" },
    });
    fireEvent.change(screen.getByLabelText("Custom shell arguments"), {
      target: { value: "--login" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save Terminal" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_shell_settings", {
        settings: expect.objectContaining({
          schema_version: 2,
          overrides: expect.objectContaining({
            shell_id: "custom",
            custom_executable: "C:/Tools/custom-shell.exe",
            custom_args: "--login",
          }),
        }),
      });
    });
  });

  it("names the resolved default terminal choices", () => {
    useSettingsStore.setState({
      shell_id: "auto",
      terminalFontFamily: "",
      available_shells: [
        {
          id: "powershell",
          label: "PowerShell",
          executable: "powershell.exe",
          default_args: [],
        },
      ],
    });

    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Terminal" }));

    expect(screen.getByRole("option", { name: "Default (PowerShell)" })).toBeInTheDocument();
    expect(screen.getByText("Currently uses PowerShell.")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Default (Droid Sans Mono)" })).toBeInTheDocument();
    expect(screen.getByText("Currently uses Droid Sans Mono.")).toBeInTheDocument();
  });

  it("checks for updates from the general category", () => {
    const checkNow = vi.fn();
    mockUseAppUpdate.mockReturnValue(appUpdateState({ checkNow }));

    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));

    expect(checkNow).toHaveBeenCalledTimes(1);
  });

  it("opens and copies the settings folder from Advanced", async () => {
    mockWriteText.mockResolvedValue(undefined);
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
    fireEvent.click(screen.getByRole("button", { name: "Open settings folder" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("get_settings_folder_path");
      expect(mockInvoke).toHaveBeenCalledWith("reveal_in_explorer", {
        path: "C:/Users/test/.wardian/settings",
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "Copy settings folder path" }));

    await waitFor(() => {
      expect(mockWriteText).toHaveBeenCalledWith("C:/Users/test/.wardian/settings");
    });
  });
});

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsModal } from "./SettingsModal";
import { useAppUpdate } from "./useAppUpdate";
import { useSettingsStore } from "../../store/useSettingsStore";
import { normalizeQueuePreferences } from "../queue/queueFilters";
import { useQueueStore } from "../../store/useQueueStore";
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
      gridCardDisplayMode: "terminal",
      watchlistNewAgentPosition: "top",
      titlebarTelemetryVisible: true,
      externalEditor: "system",
      externalEditorCustomExecutable: "",
      explorerFileClickAction: "preview",
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
    useQueueStore.setState({
      items: [],
      _agentBuffers: {},
      _workflowLastOutput: {},
      preferences: normalizeQueuePreferences({}),
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

  it("exposes remote access settings from the settings navigation", async () => {
    mockInvoke.mockImplementation(async (command, args) => {
      switch (command) {
        case "load_remote_access_status":
          return "enabled";
        case "load_remote_gateway_config":
          return {
            schema_version: 1,
            enabled: true,
            canonical_origin: "https://wardian.tailnet.ts.net",
            loopback_host: "127.0.0.1",
            loopback_port: 41241,
            gateway_identity_public_key: "pub",
            gateway_identity_fingerprint: "fp",
          };
        case "list_remote_devices":
          return [];
        case "save_app_settings":
          return (args as { settings?: unknown } | undefined)?.settings;
        case "save_shell_settings":
          return (args as { settings?: unknown } | undefined)?.settings;
        default:
          return null;
      }
    });

    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Remote Access" }));

    expect(await screen.findByText("https://wardian.tailnet.ts.net")).toBeInTheDocument();
    expect(screen.getByText(/full remote control/i)).toBeInTheDocument();
  });

  it("exposes queue notification rules from the settings navigation", () => {
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Queue" }));
    fireEvent.click(screen.getByLabelText("Desktop alert for workflow failures"));
    fireEvent.click(screen.getByLabelText("Sound alert for action needed"));
    fireEvent.change(screen.getByLabelText("Sound alert volume"), {
      target: { value: "75" },
    });

    const { preferences } = useQueueStore.getState();
    expect(preferences.desktop_notifications.workflow_failed).toBe(true);
    expect(preferences.sound_notifications.action_needed).toBe(false);
    expect(preferences.sound_volume).toBe(0.75);
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

  it("loads and saves the Grid card display preference", async () => {
    useSettingsStore.setState({ gridCardDisplayMode: "chat" });
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Grid" }));

    const select = screen.getByLabelText("Grid card display");
    expect(select).toHaveValue("chat");

    fireEvent.change(select, { target: { value: "terminal" } });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_app_settings", {
        settings: expect.objectContaining({
          schema_version: 2,
          overrides: expect.objectContaining({
            grid_card_display_mode: "terminal",
          }),
        }),
      });
    });
    expect(useSettingsStore.getState().gridCardDisplayMode).toBe("terminal");
  });

  it("loads and saves the Watchlist new agent position preference", async () => {
    useSettingsStore.setState({ watchlistNewAgentPosition: "top" });
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Watchlist" }));

    const select = screen.getByLabelText("New agent position");
    expect(select).toHaveValue("top");

    fireEvent.change(select, { target: { value: "bottom" } });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_app_settings", {
        settings: expect.objectContaining({
          schema_version: 2,
          overrides: expect.objectContaining({
            watchlist_new_agent_position: "bottom",
          }),
        }),
      });
    });
    expect(useSettingsStore.getState().watchlistNewAgentPosition).toBe("bottom");
  });

  it("loads and saves Explorer opening preferences", async () => {
    useSettingsStore.setState({
      externalEditor: "system",
      externalEditorCustomExecutable: "",
      explorerFileClickAction: "preview",
    });
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Explorer" }));

    const select = screen.getByLabelText("External editor");
    expect(select).toHaveValue("system");
    expect(screen.getByRole("option", { name: "VS Code (code command)" })).toBeInTheDocument();

    fireEvent.change(select, { target: { value: "custom" } });
    fireEvent.change(screen.getByLabelText("Custom editor executable"), {
      target: { value: "C:/Tools/editor.exe" },
    });
    fireEvent.change(screen.getByLabelText("File click action"), {
      target: { value: "external" },
    });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_app_settings", {
        settings: expect.objectContaining({
          schema_version: 2,
          overrides: expect.objectContaining({
            external_editor: "custom",
            external_editor_custom_executable: "C:/Tools/editor.exe",
            explorer_file_click_action: "external",
          }),
        }),
      });
    });
    expect(useSettingsStore.getState().externalEditor).toBe("custom");
    expect(useSettingsStore.getState().explorerFileClickAction).toBe("external");
  });

  it("loads and saves the top bar telemetry preference", async () => {
    useSettingsStore.setState({ titlebarTelemetryVisible: true });
    render(<SettingsModal isOpen onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));

    const select = screen.getByLabelText("Top bar telemetry");
    expect(select).toHaveValue("show");

    fireEvent.change(select, { target: { value: "hide" } });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_app_settings", {
        settings: expect.objectContaining({
          schema_version: 2,
          overrides: expect.objectContaining({
            titlebar_telemetry_visible: false,
          }),
        }),
      });
    });
    expect(useSettingsStore.getState().titlebarTelemetryVisible).toBe(false);
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

  it("shows restarting state after update installation", () => {
    mockUseAppUpdate.mockReturnValue(appUpdateState({ status: "installed" }));

    render(<SettingsModal isOpen onClose={vi.fn()} />);

    expect(screen.getByText("Update installed. Restarting...")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restarting..." })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Restart" })).not.toBeInTheDocument();
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

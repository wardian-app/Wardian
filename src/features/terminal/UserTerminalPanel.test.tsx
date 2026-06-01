import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { UserTerminalPanel } from "./UserTerminalPanel";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const openMock = vi.fn();
const loadAddonMock = vi.fn();
const onDataMock = vi.fn((_handler: (data: string) => void) => ({ dispose: vi.fn() }));
const onBinaryMock = vi.fn((_handler: (data: string) => void) => ({ dispose: vi.fn() }));
const disposeMock = vi.fn();
const focusMock = vi.fn();
const attachCustomKeyEventHandlerMock = vi.fn();
const selectAllMock = vi.fn();
const writeMock = vi.fn();
const clearMock = vi.fn();
const refreshMock = vi.fn();

vi.mock("@xterm/xterm", () => ({
  Terminal: vi.fn().mockImplementation(function MockTerminal() {
    return {
      cols: 80,
      rows: 24,
      options: {},
      open: openMock,
      loadAddon: loadAddonMock,
      onData: onDataMock,
      onBinary: onBinaryMock,
      dispose: disposeMock,
      focus: focusMock,
      attachCustomKeyEventHandler: attachCustomKeyEventHandlerMock,
      selectAll: selectAllMock,
      write: writeMock,
      clear: clearMock,
      refresh: refreshMock,
    };
  }),
}));

const fitMock = vi.fn();

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: vi.fn().mockImplementation(function MockFitAddon() {
    return {
      fit: fitMock,
    };
  }),
}));

vi.mock("@xterm/addon-unicode11", () => ({
  Unicode11Addon: vi.fn().mockImplementation(function MockUnicode11Addon() {
    return {};
  }),
}));

vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

describe("UserTerminalPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockInvoke.mockImplementation((command) => {
      if (command === "ensure_user_terminal" || command === "restart_user_terminal") {
        return Promise.resolve("test-user-terminal-session");
      }
      return Promise.resolve(null);
    });
    mockListen.mockResolvedValue(() => {});
  });

  it("starts the user terminal when mounted", async () => {
    render(
      <UserTerminalPanel
        theme="dark"
        height={320}
        selectedWorkspace={null}
        onHeightChange={vi.fn()}
        onHide={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("ensure_user_terminal", expect.objectContaining({
        cols: expect.any(Number),
        rows: expect.any(Number),
      }));
    });

    expect(screen.getByRole("link", { name: /cli guide/i })).toHaveAttribute(
      "href",
      "https://docs.wardian.org/guide/cli",
    );
  });

  it("installs conservative terminal shortcuts", async () => {
    render(
      <UserTerminalPanel
        theme="dark"
        height={320}
        selectedWorkspace={null}
        onHeightChange={vi.fn()}
        onHide={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(attachCustomKeyEventHandlerMock).toHaveBeenCalledTimes(1);
    });
  });

  it("drains all queued PTY output when mounted", async () => {
    const outputChunks = ["first chunk", "second chunk", null];
    mockInvoke.mockImplementation((command) => {
      if (command === "ensure_user_terminal") {
        return Promise.resolve("test-user-terminal-session");
      }
      if (command === "read_user_terminal_pty") {
        return Promise.resolve(outputChunks.shift() ?? null);
      }
      return Promise.resolve(null);
    });

    render(
      <UserTerminalPanel
        theme="dark"
        height={320}
        selectedWorkspace={null}
        onHeightChange={vi.fn()}
        onHide={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(writeMock).toHaveBeenCalledWith("first chunk");
      expect(writeMock).toHaveBeenCalledWith("second chunk");
    });
  });

  it("disables workspace jump when no single workspace is selected", () => {
    render(
      <UserTerminalPanel
        theme="dark"
        height={320}
        selectedWorkspace={null}
        onHeightChange={vi.fn()}
        onHide={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: /move to/i })).toBeDisabled();
  });

  it("sends selected workspace to backend", async () => {
    render(
      <UserTerminalPanel
        theme="dark"
        height={320}
        selectedWorkspace="C:/project"
        onHeightChange={vi.fn()}
        onHide={vi.fn()}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: /move to/i }));

    expect(mockInvoke).toHaveBeenCalledWith("set_user_terminal_cwd", { path: "C:/project" });
  });

  it("forwards binary terminal input to backend", async () => {
    render(
      <UserTerminalPanel
        theme="dark"
        height={320}
        selectedWorkspace={null}
        onHeightChange={vi.fn()}
        onHide={vi.fn()}
      />,
    );

    await waitFor(() => expect(onBinaryMock).toHaveBeenCalled());

    const onBinary = onBinaryMock.mock.calls[0]?.[0] as ((data: string) => void) | undefined;
    expect(onBinary).toBeDefined();
    onBinary?.(String.fromCharCode(96, 97, 98));

    expect(mockInvoke).toHaveBeenCalledWith("send_binary_input_to_user_terminal", {
      input: [96, 97, 98],
    });
  });
});

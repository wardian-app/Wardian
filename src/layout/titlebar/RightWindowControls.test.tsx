import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { getCurrentWindow } from "@tauri-apps/api/window";

const mockGetCurrentWindow = vi.mocked(getCurrentWindow);

async function loadRightWindowControls(options?: { tauri?: boolean; maximized?: boolean }) {
  const resizeListeners: Array<() => void> = [];
  const unlisten = vi.fn();
  const appWindow = {
    isMaximized: vi.fn().mockResolvedValue(options?.maximized ?? false),
    onResized: vi.fn((listener: () => void) => {
      resizeListeners.push(listener);
      return Promise.resolve(unlisten);
    }),
    minimize: vi.fn(() => Promise.resolve()),
    toggleMaximize: vi.fn(() => Promise.resolve()),
    close: vi.fn(() => Promise.resolve()),
  };

  if (options?.tauri) {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    mockGetCurrentWindow.mockReturnValue(appWindow as unknown as ReturnType<typeof getCurrentWindow>);
  } else {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    mockGetCurrentWindow.mockClear();
  }

  vi.resetModules();
  const module = await import("./RightWindowControls");
  return { RightWindowControls: module.RightWindowControls, appWindow, resizeListeners, unlisten };
}

describe("RightWindowControls", () => {
  afterEach(() => {
    mockGetCurrentWindow.mockReset();
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it("toggles the agent roster titlebar control", async () => {
    const user = userEvent.setup();
    const setRightCollapsed = vi.fn();
    const { RightWindowControls } = await loadRightWindowControls();

    render(<RightWindowControls rightCollapsed={false} setRightCollapsed={setRightCollapsed} />);
    await user.click(screen.getByTitle("Hide Agent Roster"));

    expect(setRightCollapsed).toHaveBeenCalledWith(true);
  });

  it("renders fallback window controls outside Tauri", async () => {
    const { RightWindowControls } = await loadRightWindowControls();

    render(<RightWindowControls rightCollapsed setRightCollapsed={vi.fn()} />);

    expect(screen.getByTitle("Show Agent Roster")).toBeInTheDocument();
    expect(screen.getByTitle("Minimize")).toBeInTheDocument();
    expect(screen.getByTitle("Maximize")).toBeInTheDocument();
    expect(screen.getByTitle("Close")).toBeInTheDocument();
  });

  it("routes Tauri window controls and updates maximized state after resize", async () => {
    const user = userEvent.setup();
    const { RightWindowControls, appWindow, resizeListeners, unlisten } = await loadRightWindowControls({
      tauri: true,
    });

    const { unmount } = render(<RightWindowControls rightCollapsed setRightCollapsed={vi.fn()} />);

    expect(await screen.findByTitle("Maximize")).toBeInTheDocument();
    await user.click(screen.getByTitle("Minimize"));
    await user.click(screen.getByTitle("Maximize"));
    await user.click(screen.getByTitle("Close"));

    expect(appWindow.minimize).toHaveBeenCalledTimes(1);
    expect(appWindow.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(appWindow.close).toHaveBeenCalledTimes(1);

    appWindow.isMaximized.mockResolvedValue(true);
    resizeListeners[0]();
    expect(await screen.findByTitle("Restore Down")).toBeInTheDocument();

    unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });
});

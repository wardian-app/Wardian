import { describe, expect, it, vi } from "vitest";
import { installConservativeTerminalShortcuts, isMacPlatform } from "./terminalShortcuts";

function keyboardEvent(init: Partial<KeyboardEvent> = {}) {
  return {
    type: "keydown",
    key: "a",
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    preventDefault: vi.fn(),
    stopPropagation: vi.fn(),
    ...init,
  } as unknown as KeyboardEvent;
}

function shortcutHarness(platform: string) {
  const installed: { handler?: (event: KeyboardEvent) => boolean } = {};
  const terminal = {
    attachCustomKeyEventHandler: vi.fn((nextHandler: (event: KeyboardEvent) => boolean) => {
      installed.handler = nextHandler;
    }),
    selectAll: vi.fn(),
  };

  installConservativeTerminalShortcuts(terminal, { platform });

  const handler = installed.handler;
  if (!handler) {
    throw new Error("shortcut handler was not installed");
  }

  return { terminal, handler };
}

describe("terminalShortcuts", () => {
  it.each([
    ["MacIntel", true],
    ["MacPPC", true],
    ["Win32", false],
    ["Linux x86_64", false],
  ])("detects mac platform for %s", (platform, expected) => {
    expect(isMacPlatform(platform)).toBe(expected);
  });

  it("selects the terminal buffer for Cmd+A on macOS", () => {
    const { terminal, handler } = shortcutHarness("MacIntel");
    const event = keyboardEvent({ metaKey: true });

    const shouldProcess = handler(event);

    expect(shouldProcess).toBe(false);
    expect(terminal.selectAll).toHaveBeenCalledTimes(1);
    expect(event.preventDefault).toHaveBeenCalledTimes(1);
    expect(event.stopPropagation).toHaveBeenCalledTimes(1);
  });

  it("does not intercept Cmd+A keyup on macOS", () => {
    const { terminal, handler } = shortcutHarness("MacIntel");
    const event = keyboardEvent({ type: "keyup", metaKey: true });

    expect(handler(event)).toBe(true);
    expect(terminal.selectAll).not.toHaveBeenCalled();
  });

  it.each(["Win32", "Linux x86_64"])("passes Ctrl+A through on %s", (platform) => {
    const { terminal, handler } = shortcutHarness(platform);
    const event = keyboardEvent({ ctrlKey: true });

    expect(handler(event)).toBe(true);
    expect(terminal.selectAll).not.toHaveBeenCalled();
    expect(event.preventDefault).not.toHaveBeenCalled();
  });

  it.each(["Win32", "Linux x86_64"])("passes Ctrl+Shift+A through on %s", (platform) => {
    const { terminal, handler } = shortcutHarness(platform);
    const event = keyboardEvent({ ctrlKey: true, shiftKey: true });

    expect(handler(event)).toBe(true);
    expect(terminal.selectAll).not.toHaveBeenCalled();
    expect(event.preventDefault).not.toHaveBeenCalled();
  });

  it("passes Cmd+A through on non-mac platforms", () => {
    const { terminal, handler } = shortcutHarness("Win32");
    const event = keyboardEvent({ metaKey: true });

    expect(handler(event)).toBe(true);
    expect(terminal.selectAll).not.toHaveBeenCalled();
  });
});

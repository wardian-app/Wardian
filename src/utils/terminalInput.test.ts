import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { mockInvoke } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mockInvoke,
}));

import {
  flattenPromptForInjection,
  submitInputToAgent,
  submitInputToAgents,
} from "./terminalInput";

describe("terminalInput", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("flattens multiline prompts for injection", () => {
    expect(flattenPromptForInjection("Line one\nLine two\r\nLine three")).toBe(
      "Line one Line two Line three",
    );
  });

  it("submits agent input as text followed by a separate enter key", async () => {
    await submitInputToAgent("agent-1", "hello world");

    expect(mockInvoke).toHaveBeenNthCalledWith(1, "submit_prompt_to_agent", {
      sessionId: "agent-1",
      prompt: "hello world",
    });
  });

  it("submits the same input to multiple agents sequentially", async () => {
    await submitInputToAgents(["agent-1", "agent-2"], "ping");

    expect(mockInvoke).toHaveBeenNthCalledWith(1, "submit_prompt_to_agent", {
      sessionId: "agent-1",
      prompt: "ping",
    });
    expect(mockInvoke).toHaveBeenNthCalledWith(2, "submit_prompt_to_agent", {
      sessionId: "agent-2",
      prompt: "ping",
    });
  });
});

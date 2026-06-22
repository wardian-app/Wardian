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
    mockInvoke.mockResolvedValue({
      uuid: "agent-1",
      name: "Coder",
      provider: "codex",
      runtime_state: "live_pty_available",
      delivery_state: "submit_sent_unconfirmed",
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("flattens multiline prompts for injection", () => {
    expect(flattenPromptForInjection("Line one\nLine two\r\nLine three")).toBe(
      "Line one Line two Line three",
    );
  });

  it("returns delivery detail from submit_prompt_to_agent", async () => {
    const result = await submitInputToAgent("agent-1", "hello world");

    expect(result?.delivery_state).toBe("submit_sent_unconfirmed");
    expect(mockInvoke).toHaveBeenNthCalledWith(1, "submit_prompt_to_agent", {
      sessionId: "agent-1",
      prompt: "hello world",
    });
  });

  it("returns all delivery details for multi-agent submission", async () => {
    mockInvoke
      .mockResolvedValueOnce({
        uuid: "agent-1",
        name: "one",
        provider: "codex",
        runtime_state: "live_pty_available",
        delivery_state: "submit_sent_unconfirmed",
      })
      .mockResolvedValueOnce({
        uuid: "agent-2",
        name: "two",
        provider: "claude",
        runtime_state: "live_pty_available",
        delivery_state: "submit_sent_unconfirmed",
      });

    const results = await submitInputToAgents(["agent-1", "agent-2"], "ping");

    expect(results.map((result) => result.uuid)).toEqual(["agent-1", "agent-2"]);
    expect(mockInvoke).toHaveBeenNthCalledWith(1, "submit_prompt_to_agent", {
      sessionId: "agent-1",
      prompt: "ping",
    });
    expect(mockInvoke).toHaveBeenNthCalledWith(2, "submit_prompt_to_agent", {
      sessionId: "agent-2",
      prompt: "ping",
    });
  });

  it("does not use raw terminal input for structured prompt submission", async () => {
    await submitInputToAgent("agent-1", "structured prompt");

    expect(mockInvoke).not.toHaveBeenCalledWith("send_input_to_agent", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("send_binary_input_to_agent", expect.anything());
  });
});

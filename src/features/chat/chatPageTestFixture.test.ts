import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";
import type { AgentChatPage } from "../../types";
import { chatPageInvokeFixture } from "../../test/chatPageTestFixture";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("Chat page invoke fixture", () => {
  it("frames a chained successful retry after a rejected read", async () => {
    const adapter = chatPageInvokeFixture(vi.mocked(invoke));
    adapter.mockReset();
    const chained = adapter.mockRejectedValueOnce(new Error("transcript missing")).mockResolvedValueOnce([]);
    expect(chained).toBe(adapter);
    await expect(adapter<AgentChatPage>("load_agent_chat_page", { sessionId: "agent" })).rejects.toThrow("transcript missing");
    await expect(adapter<AgentChatPage>("load_agent_chat_page", { sessionId: "agent" })).resolves.toMatchObject({
      session_id: "agent", events: [], unchanged: false, reset: false,
    });
  });
});

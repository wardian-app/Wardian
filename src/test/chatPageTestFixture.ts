import { invoke } from "@tauri-apps/api/core";
import type { MockedFunction } from "vitest";
import type { AgentChatEvent, AgentChatPage } from "../types";

type InvokeMock = MockedFunction<typeof invoke>;
type Implementation = Parameters<InvokeMock["mockImplementation"]>[0];

/** Existing presentation fixtures are server datasets. Only the requested
 * page enters the component, through the actual page command and envelope. */
function frame(command: string, args: Parameters<typeof invoke>[1], value: unknown): unknown {
  if (command !== "load_agent_chat_page" || !Array.isArray(value)) return value;
  const rows = value as AgentChatEvent[];
  const request = args as Record<string, unknown> | undefined;
  const end = typeof request?.cursor === "string" ? Number(request.cursor) : rows.length;
  const start = Math.max(0, end - 80);
  let hash = 0;
  for (const character of JSON.stringify(rows)) hash = (Math.imul(hash, 31) + character.charCodeAt(0)) | 0;
  const revision = `fixture:${hash}`;
  const unchanged = request?.revision === revision;
  const page: AgentChatPage = {
    session_id: typeof request?.sessionId === "string" ? request.sessionId : "agent-1",
    conversation_id: "conversation", generation: "generation", source_epoch: null,
    revision, events: unchanged ? [] : rows.slice(start, end), next_before: start > 0 ? String(start) : null,
    unchanged, reset: false, progress: "ready", aliases: [], removed_ids: [], detail: null,
    bytes_read: 0, records_decoded: 0,
  };
  return page;
}

/** Keep the real invoke spy and adapt only this test file's legacy datasets. */
export function chatPageInvokeFixture(target: InvokeMock): InvokeMock {
  const wrap = (implementation: Implementation): Implementation => (...args) =>
    Promise.resolve(implementation(...args)).then((value) => frame(args[0], args[1], value));
  const proxy: InvokeMock = new Proxy(target, {
    get(mock, property, receiver) {
      if (property === "mockImplementation" || property === "mockImplementationOnce") {
        return (implementation: Implementation) => { mock[property](wrap(implementation)); return proxy; };
      }
      if (property === "mockRejectedValue" || property === "mockRejectedValueOnce") {
        return (reason: unknown) => { mock[property](reason); return proxy; };
      }
      if (property === "mockResolvedValue" || property === "mockResolvedValueOnce" || property === "mockReturnValue" || property === "mockReturnValueOnce") {
        return (value: unknown) => {
          const implementation: Implementation = () => Promise.resolve(value);
          if (property.endsWith("Once")) mock.mockImplementationOnce(wrap(implementation));
          else mock.mockImplementation(wrap(implementation));
          return proxy;
        };
      }
      return Reflect.get(mock, property, receiver);
    },
  });
  return proxy;
}

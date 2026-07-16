import { invoke } from "@tauri-apps/api/core";

/**
 * One-release desktop adapter for installations that do not expose terminal
 * session protocol v2 yet. New workbench presentations never select this path.
 */
export const terminalCompatibilityAdapter = {
  read(sessionId: string, options?: { max_bytes?: number; peek?: boolean }) {
    return invoke<string | null>(
      "read_agent_pty",
      options ? { sessionId, options } : { sessionId },
    );
  },

  sendText(sessionId: string, input: string) {
    return invoke<void>("send_input_to_agent", { sessionId, input });
  },

  sendBinary(sessionId: string, input: readonly number[]) {
    return invoke<void>("send_binary_input_to_agent", {
      sessionId,
      input: Array.from(input),
    });
  },

  resize(sessionId: string, cols: number, rows: number) {
    return invoke<void>("resize_agent_terminal", { sessionId, cols, rows });
  },
};

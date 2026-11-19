import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AgentChatEvent } from "../../types";
import { derivePresentedChatRows } from "../grid/workLogPresentation";
import { ChatTranscriptRow } from "./ChatTranscriptRows";
import { shouldShowChatEvent, toolPatchText } from "./chatPresentation";
import { withTurnChangeSummaries } from "./chatTurns";
import fixtures from "./chatToolProjectionFixture.json";
import { structuredEditFromEvent } from "./structuredEdit";

// The Rust regression normalizes each raw record through the production parser,
// then verifies these metadata fields and detail bytes through the real header/body helpers.
// This test consumes that same projected contract, never raw provider metadata.
describe("metadata-backed tool projection contract", () => {
  it("retains Codex custom exec input and pairs its completion after visibility filtering", async () => {
    const input = 'const result = await tools.exec_command({ cmd: "cat audit.txt" }); text(result);';
    const call: AgentChatEvent = {
      id: "exec-call", session_id: "agent-1", provider: "codex", kind: "tool_call", role: null,
      text: null, title: "exec", status: "running", turn_id: "turn-1", source: "provider_log", command: null,
      exit_code: null, path: null, language: null, created_at: null, sequence: 1,
      metadata: { raw_type: "custom_tool_call", tool_name: "exec", tool_input_text: input,
        call_id: "exec-1", chat_detail_ref: "exec-input:0" },
    };
    const result: AgentChatEvent = { ...call, id: "exec-result", kind: "tool_result", title: "Tool result",
      text: "Script completed", status: "succeeded", sequence: 2,
      metadata: { raw_type: "custom_tool_call_output", call_id: "exec-1" } };
    const rows = derivePresentedChatRows([call, result].filter(shouldShowChatEvent));
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ kind: "event", event: { id: call.id },
      entry: { merged_result_events: [{ id: result.id }] } });
    const load = vi.fn().mockResolvedValue({ event_id: call.id, text: input, next: null, complete: true });
    const view = render(<ChatTranscriptRow row={rows[0]} agentIsWorking={false}
      isSubmitting={false} onApprovalSubmit={vi.fn()} onLoadDetail={load} />);
    expect(screen.getByText("exec", { exact: true })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show full details" }));
    await waitFor(() => expect(load).toHaveBeenCalledWith("exec-input:0"));
    await waitFor(() => expect([...view.container.querySelectorAll("pre")].some((node) => node.textContent === input)).toBe(true));
    view.unmount();
  });

  for (const fixture of fixtures) {
    it(`presents ${fixture.provider} ${fixture.title} and opens the original input`, async () => {
      const event: AgentChatEvent = {
        id: "projected-tool", session_id: "agent-1", provider: fixture.provider, kind: "tool_call", role: null,
        text: null, title: fixture.title, status: "running", turn_id: null, source: null, command: null,
        exit_code: null, path: null, language: null, created_at: null, sequence: 1,
        metadata: { ...fixture.metadata, chat_detail_ref: "projected:0" },
      };
      const rows = derivePresentedChatRows([event]);
      expect(withTurnChangeSummaries(rows).find((row) => row.kind === "turn_change_summary")).toMatchObject({ files: [fixture.change] });
      if (fixture.title === "apply_patch") expect(toolPatchText(event)).toBe(fixture.detail_text);
      else expect(structuredEditFromEvent(event)?.kind).toBe(fixture.title === "Write" ? "write" : "edit");
      const load = vi.fn().mockResolvedValue({ event_id: event.id, text: fixture.detail_text, next: null, complete: true });
      const view = render(<ChatTranscriptRow row={{ kind: "event", event }} agentIsWorking={false}
        isSubmitting={false} onApprovalSubmit={vi.fn()} onLoadDetail={load} />);
      expect(screen.getByTestId(fixture.title === "apply_patch" ? "tool-diff-panel" : "tool-structured-edit")).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Show full details" }));
      await waitFor(() => expect(load).toHaveBeenCalledWith("projected:0"));
      await waitFor(() => expect([...view.container.querySelectorAll("pre")].some((node) => node.textContent === fixture.detail_text)).toBe(true));
      view.unmount();
    });
  }

  it("keeps full edit totals when only an unchanged prefix fits in the preview", () => {
    const event: AgentChatEvent = {
      id: "preview", session_id: "agent-1", provider: "claude", kind: "tool_call", role: null, text: null,
      title: "Edit", status: "running", turn_id: null, source: null, command: null, exit_code: null,
      path: null, language: null, created_at: null, sequence: 1,
      metadata: { tool_name: "Edit", tool_input: { file_path: "src/example.ts", old_string: "prefix", new_string: "prefix" },
        chat_tool_input_truncated: true, chat_edit_summary: { kind: "edit", added: 500, removed: 300 } },
    };
    expect(structuredEditFromEvent(event)).toMatchObject({ file_path: "src/example.ts", kind: "edit", added: 500, removed: 300, truncated: true });
  });
});

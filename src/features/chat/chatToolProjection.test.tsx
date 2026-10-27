import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AgentChatEvent } from "../../types";
import { derivePresentedChatRows } from "../grid/workLogPresentation";
import { ChatTranscriptRow } from "./ChatTranscriptRows";
import { toolPatchText } from "./chatPresentation";
import { withTurnChangeSummaries } from "./chatTurns";
import fixtures from "./chatToolProjectionFixture.json";
import { structuredEditFromEvent } from "./structuredEdit";

// The Rust regression normalizes each raw record through the production parser,
// then verifies these metadata fields and detail bytes through the real header/body helpers.
// This test consumes that same projected contract, never raw provider metadata.
describe("metadata-backed tool projection contract", () => {
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

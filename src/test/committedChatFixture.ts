import type { AgentChatEvent, AgentChatPage, ChatPresentationV1, ChatPresentedItem, ChatStepDetail } from "../types";

/** Entirely synthetic conversation for unit and browser presentation evidence. */
export function committedChatFixture() {
  const event = (id: string, values: Partial<AgentChatEvent>): AgentChatEvent => ({
    id, session_id: "synthetic-agent", provider: "codex", kind: "message", role: "assistant", text: null,
    title: null, status: null, turn_id: null, source: "synthetic", command: null, exit_code: null,
    path: null, language: null, created_at: null, sequence: null, metadata: { _wardian_canonical_row: true }, ...values,
  });
  const events = [event("prompt", { role: "user", text: "Check the sample configuration and run its tests." }),
    event("read", { kind: "tool_call", role: "tool", title: "read_file", text: '{"path":"sample.json"}' }),
    event("note", { text: "The configuration looks consistent. I will run the focused tests." }),
    event("test", { kind: "tool_call", role: "tool", title: "exec_command", command: "npm test" }),
    event("answer", { text: "The sample configuration is valid. **All 42 tests passed.**\n\nNo changes were needed." })];
  const item = (id: string, kind: ChatPresentedItem["kind"], index: number): ChatPresentedItem => ({
    item_key: `item:${id}`, turn_key: "turn:synthetic", order_key: `${index}`.padStart(4, "0"), kind,
    provenance: "canonical", evidence: { kind: "source", source_key: "synthetic-source", source_epoch: "synthetic-epoch", record_key: id },
    evidence_version: "v1", admission_id: null, submission_status: null, event_refs: [id], preview: null,
    step: kind === "step" ? { tool_name: id === "read" ? "Read" : "Shell", category: id === "read" ? "read" : "shell",
      description: id === "read" ? "Read sample.json" : "Run focused tests", status: "completed", summary: "Result available",
      invocation_ref: id, result_ref: null, approval_ref: null, detail_availability: "available", detail_ref: `step:${id}` } : null,
  });
  const presentation: ChatPresentationV1 = { turns: [{ turn_key: "turn:synthetic", order_key: "0000", membership_complete: true,
    status: "completed", prompt_item_keys: ["item:prompt"], answer_item_keys: ["item:answer"], references_complete: true,
    duration_ms: 12_000, reported_changes: null, step_count: 2, answer_recorded: true, failed_step_count: 0, counts_complete: true }],
    items: [item("prompt", "prompt", 0), item("read", "step", 1), item("note", "narration", 2), item("test", "step", 3), item("answer", "answer", 4)],
    state_event_refs: [], legacy_turns: [], detail: null };
  const page: AgentChatPage = { session_id: "synthetic-agent", conversation_id: "synthetic-conversation", generation: "synthetic-generation",
    source_epoch: "synthetic-epoch", revision: "synthetic-revision", presentation: { version: 1, payload: presentation }, events,
    next_before: null, unchanged: false, reset: false, progress: "ready", aliases: [], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0 };
  const detail: ChatStepDetail = { step_key: "item:read", revision: page.revision, text: '{"valid":true,"checks":3}', input_text: '{"path":"sample.json"}',
    input_truncated: false, result_summary: "3 checks", format: "json", pending: false, unavailable: false, complete: true, truncated_before: false, total_bytes: 25 };
  return { events, presentation, page, detail };
}

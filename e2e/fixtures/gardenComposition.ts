import type { Page } from "@playwright/test";
import { installWorkbenchIpcMock, makeWorkbenchDocument, makeWorkbenchSurface } from "./workbenchIpcMock";
import type { GardenMemoryRecord } from "../../src/features/garden/useGardenAgentContents";
import type { AutomationSchedule } from "../../src/types/automation";
import type { Blueprint } from "../../src/features/automations/builder/blueprintTypes";

export const GARDEN_ROOT = "/synthetic/garden";
export const GARDEN_AGENT = "garden-designer";
export const GARDEN_MEMORY = "memory-layout";
export const GARDEN_RUN = "run-design-review";
const timestamp = "2026-09-07T12:00:00.000Z";
const memory: GardenMemoryRecord = {
  memory_id: GARDEN_MEMORY, revision_id: "revision-2", revision: 2,
  agent_id: GARDEN_AGENT, workspace: GARDEN_ROOT, kind: "stable",
  text: "Keep the five agent regions in a stable layout.",
  evidence_excerpt: "Review confirmed that Memory stays beside Skills across agents.",
  evidence_hash: "synthetic-evidence-hash", status: "active",
  supersedes_revision_id: "revision-1", replaced_by_revision_id: null,
  created_at: timestamp, updated_at: timestamp, last_verified_at: timestamp,
  idempotency_key: null,
  sources: [{ source_type: "conversation", locator: "conversation-design:turn:4", primary: true }],
};
const blueprint: Blueprint = {
  schema: 1, id: "design-review", name: "Design review", nodes: [
    { id: "draft", type: "task", name: "Draft interface", fields: { agent: "role:author", prompt: "Draft the interface" } },
    { id: "review", type: "task", name: "Review evidence", fields: { agent: "role:reviewer", prompt: "Review the draft" } },
  ], edges: [{ from: "draft", to: "review", from_port: "out", to_port: "in" }],
};
const assignments: AutomationSchedule["assignments"] = {
  author: { target_type: "agent", agent_id: GARDEN_AGENT, conversation: "current" },
  reviewer: { target_type: "agent", agent_id: "garden-reviewer", conversation: "fresh_background" },
};
const schedule: AutomationSchedule = {
  id: "daily-design", blueprint_id: blueprint.id, name: "Daily design review",
  workspace: GARDEN_ROOT, input: {}, bindings: {}, assignments,
  schedule: { schedule_type: "daily", time_of_day: "09:00", active: true }, is_paused: false,
};

/** All data is synthetic. The shared bridge records calls and implements file resources and CAS persistence. */
export async function installGardenCompositionMock(page: Page, options: {
  memoryCount?: number;
  conversationCount?: number;
  historicalOneOffRunCount?: number;
  assignedAutomationCount?: number;
  scheduledRunStatus?: "running" | "failed";
  singleAgentAutomations?: boolean;
} = {}) {
  const scheduledRunStatus = options.scheduledRunStatus ?? "running";
  const scheduleAssignments = options.singleAgentAutomations ? {
    ...assignments,
    reviewer: { target_type: "agent" as const, agent_id: GARDEN_AGENT, conversation: "fresh_background" as const },
  } : assignments;
  const schedules = Array.from({ length: options.assignedAutomationCount ?? 1 }, (_, index) => ({
    ...schedule,
    assignments: scheduleAssignments,
    ...(index ? { id: `daily-design-${index + 1}`, name: `Design review ${index + 1}` } : {}),
  }));
  const recentTerminalRuns = Array.from({ length: options.historicalOneOffRunCount ?? 0 }, (_, index) => ({
    run_id: `historical-review-${index}`,
    blueprint_id: blueprint.id,
    schedule_id: null,
    status: index % 2 ? "failed" : "completed",
    node_count: 2,
    path: `/synthetic/runs/historical-review-${index}`,
    started_at: new Date(Date.now() - (index + 1) * 60_000).toISOString(),
    updated_at: new Date(Date.now() - index * 60_000).toISOString(),
  }));
  return installWorkbenchIpcMock(page, {
    load_result: { source: "primary", notice: null, durable_revision: 0, durable_token: "garden-token",
      document: makeWorkbenchDocument({ surfaces: [makeWorkbenchSurface("garden-main", "garden")],
        shell: { left_sidebar_collapsed: true, right_sidebar_collapsed: true } }),
    },
    agents: [
      { session_id: GARDEN_AGENT, session_name: "Moss Designer", agent_class: "Designer", folder: GARDEN_ROOT, provider: "claude", is_off: false, description: "Designs clear spatial interfaces." },
      { session_id: "garden-reviewer", session_name: "Fern Reviewer", agent_class: "Reviewer", folder: GARDEN_ROOT, provider: "claude", is_off: false },
    ],
    explorer_root: GARDEN_ROOT,
    files: [
      { path: `${GARDEN_ROOT}/src/cutaway.tsx`, content: "export const regions = ['Identity', 'Skills', 'Memory', 'Automations', 'Conversations'];" },
      { path: `${GARDEN_ROOT}/docs/evidence.md`, content: "# Review evidence\nThe agent layout is stable." },
      { path: `${GARDEN_ROOT}/README.md`, content: "Unchanged workspace introduction." },
    ],
    responses: {
      memory_list: options.memoryCount ? Array.from({ length: options.memoryCount }, (_, index) => index === 0 ? memory : {
        ...memory, memory_id: `dense-memory-${index}`, revision_id: `dense-revision-${index}`,
        text: `Memory ${index}: Preserve the agent geography and inspect the supporting evidence.`,
      }) : [memory, { ...memory, memory_id: "memory-current", kind: "current", workspace: null, text: "Collect narrow viewport evidence.", revision_id: "current-1", revision: 1 }],
      memory_get: memory,
      memory_history: [{ ...memory, revision: 1, revision_id: "revision-1", status: "superseded", text: "Keep agent regions stable." }, memory],
      list_conversations: { schema: 1, conversations: Array.from({ length: options.conversationCount ?? 1 }, (_, index) => ({ schema: 1, conversation_id: index ? `conversation-${index}` : "conversation-design", agent_id: GARDEN_AGENT, agent_name: "Moss Designer", agent_class: "Designer", workspace: GARDEN_ROOT, provider: "claude", provider_session_ids: [], started_at: new Date(Date.parse(timestamp) - index * 86_400_000).toISOString(), ended_at: index ? timestamp : null, status: index ? "closed" : "open", boundary_reason: "spawn", first_prompt_excerpt: "Inspect the cutaway", last_record_excerpt: index ? `Conversation ${index}: Review the recorded design evidence.` : "Draft ready for evidence review.", record_count: 8, turn_count: 4, has_turns: true, lifecycle_only: false, artifact_count: 2, path: `/synthetic/conversations/design-${index}` })) },
      get_library_index: { sections: { skills: { stubbed: false, tree: { name: "Root", path: "", children: [{ kind: "skill", entry_ref: "skills/interface-review", path: "interface-review", name: "Interface Review", description: "Inspect interface evidence", tags: [], is_starred: false, deployment_count: 1 }] } } }, deployments: { "skills/interface-review": [{ target_type: "agent", target_id: GARDEN_AGENT, linked: true }] }, orphans: [] },
      read_library_item: "# Interface Review\nCheck task flow, keyboard access, and evidence.",
      load_change_review_prefs: { schema: 1, baseline: "branch_point" },
      load_change_review: { git_available: true, workspace_root: GARDEN_ROOT, head_ref: "synthetic-head", skipped_turn_records: 0, summary: { schema: 1, baseline: "branch_point", baseline_ref: "synthetic-base", from_turn_index: null, to_turn_index: 4, computed_at: timestamp, truncated: false, baseline_diverged: false, files: [
        { path: "src/cutaway.tsx", old_path: null, change_kind: "modified", insertions: 18, deletions: 3, evidence: "attributed", agent_ids: [GARDEN_AGENT, "garden-reviewer"], turn_indices: [3, 4] },
        { path: "docs/evidence.md", old_path: null, change_kind: "added", insertions: 4, deletions: 0, evidence: "attributed", agent_ids: [GARDEN_AGENT], turn_indices: [4] },
      ] } },
      load_agent_reach: { schema: 1, agents: [], skipped_turn_records: 0 },
      automation_list_blueprints: { blueprints: [{ id: blueprint.id, path: "/synthetic/library/design-review.md" }], truncated: false, next_offset: null },
      automation_parse: { blueprint }, schedule_list: schedules,
      automation_list_runs: { runs: [
        { run_id: GARDEN_RUN, blueprint_id: blueprint.id, schedule_id: schedule.id, status: scheduledRunStatus, node_count: 2, path: `/synthetic/runs/${GARDEN_RUN}`, started_at: timestamp },
        ...recentTerminalRuns,
      ], truncated: false, next_offset: null },
      read_file_preview: JSON.stringify({ workspace: GARDEN_ROOT, schedule_id: schedule.id, assignments: scheduleAssignments }),
      automation_read_run: { blueprint, blueprint_path: "/synthetic/library/design-review.md", state: { run_id: GARDEN_RUN, blueprint_id: blueprint.id, status: scheduledRunStatus, nodes: { draft: "completed", review: scheduledRunStatus } }, events: [{ seq: 1, ts: timestamp, kind: "node_completed", node: "draft", output: { artifact: "cutaway-preview", region_count: 5 } }, scheduledRunStatus === "failed" ? { seq: 2, ts: timestamp, kind: "node_failed", node: "review", error: "Synthetic review failure" } : { seq: 2, ts: timestamp, kind: "node_started", node: "review" }] },
    },
  });
}

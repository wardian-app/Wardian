import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useQueueStore } from "../../store/useQueueStore";
import { GardenAgentInterior } from "./GardenAgentInterior";
import { useGardenAgentContents, type GardenConversationEntry, type GardenMemoryRecord } from "./useGardenAgentContents";

vi.mock("./useGardenAgentContents", () => ({ useGardenAgentContents: vi.fn() }));
const agent = { session_id: "a", session_name: "Agent A", agent_class: "Builder", folder: "/workspace", is_off: false };
const loaded = <T,>(data: T) => ({ data, loading: false, stale: false, error: null });
function conversation(index: number): GardenConversationEntry {
  return {
    schema: 1, conversation_id: `conversation-${index}`, agent_id: "a", agent_name: "Agent A", agent_class: "Builder",
    workspace: "/workspace", provider: "codex", provider_session_ids: [], started_at: "2026-09-09T00:00:00Z",
    ended_at: null, status: "closed", boundary_reason: "clear", first_prompt_excerpt: null,
    last_record_excerpt: `Conversation ${index} has a long recorded excerpt that should only mount when its own disclosure is opened.`,
    record_count: 2, turn_count: index, has_turns: true, lifecycle_only: false, artifact_count: 0, path: "archive",
  };
}
function memory(index: number): GardenMemoryRecord {
  return {
    memory_id: `m${index}`, revision_id: `r${index}`, revision: 1, agent_id: "a",
    workspace: index % 2 ? "/workspace" : null, kind: index % 4 < 2 ? "stable" : "current",
    text: `Memory ${index} preserves its full accessible identity and canonical anchor.`, evidence_excerpt: "Evidence",
    evidence_hash: "hash", status: "active", supersedes_revision_id: null, replaced_by_revision_id: null,
    created_at: "now", updated_at: "now", last_verified_at: "now", idempotency_key: null, sources: [],
  };
}
function setup(memories: GardenMemoryRecord[], conversations: GardenConversationEntry[]) {
  vi.mocked(useGardenAgentContents).mockReturnValue({ memories: loaded(memories), conversations: loaded(conversations), refresh: vi.fn() });
  const callbacks = { onSelect: vi.fn(), onEnter: vi.fn(), onOpenAgent: vi.fn() };
  const view = render(<GardenAgentInterior agent={agent} status="idle" crown={[]} agents={[agent]} teams={[]} automations={[]} selectedKey="memory:m1" {...callbacks} />);
  return { ...view, ...callbacks };
}
beforeEach(() => {
  useQueueStore.setState({ items: [], loadItems: vi.fn().mockResolvedValue(undefined), inboxNotificationsTruncated: false });
});

describe("Garden collections", () => {
  it("uses singular counts for one loaded memory, scope memory and conversation", async () => {
    setup([memory(1)], [conversation(1)]);
    expect(screen.getByText("1 loaded memory")).toBeInTheDocument();
    expect(screen.getByText("1 memory")).toBeInTheDocument();
    expect(screen.getByText("1 loaded conversation")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("Loading Inbox…")).not.toBeInTheDocument());
  });
  it("exposes all 60 conversations and mounts only expanded excerpts, including entries beyond three", async () => {
    const conversations = Array.from({ length: 60 }, (_, index) => conversation(index + 1));
    const { container } = setup([], conversations);
    expect(screen.getByText("60 loaded conversations")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Conversations", { selector: "summary" }));
    expect(container.querySelectorAll(".garden-conversation-object")).toHaveLength(60);
    expect(container.querySelectorAll(".garden-conversation-detail")).toHaveLength(0);
    for (const index of [3, 59]) {
      const entry = conversations[index];
      const summary = screen.getByLabelText(`Recent session, ${entry.last_record_excerpt}, ${entry.started_at}, ${entry.conversation_id}`);
      expect(within(summary).getByText("2026-09-09")).toBeInTheDocument();
      fireEvent.click(summary);
      await waitFor(() => expect(screen.getByText(entry.last_record_excerpt!)).toBeInTheDocument());
      expect(container.querySelectorAll(".garden-conversation-detail")).toHaveLength(1);
      fireEvent.click(summary);
      await waitFor(() => expect(screen.queryByText(entry.last_record_excerpt!)).not.toBeInTheDocument());
    }
  });

  it.each([34, 300])("preserves all %i memory anchors in the existing scroll region", async (count) => {
    const memories = Array.from({ length: count }, (_, index) => memory(index + 1));
    const { onSelect, onEnter } = setup(memories, []);
    const region = screen.getByLabelText("Memory contents");
    expect(region).toHaveAttribute("tabindex", "0");
    expect(within(region).getByText(`${count} loaded memories`)).toBeInTheDocument();
    const anchorNodes = region.querySelectorAll<HTMLElement>('[data-garden-ref^="memory:"]');
    expect(anchorNodes).toHaveLength(count);
    const anchors = new Map<string, HTMLButtonElement>();
    for (const anchor of anchorNodes) {
      expect(anchor).toBeInstanceOf(HTMLButtonElement);
      expect(anchor).toHaveRole("button");
      const ref = anchor.dataset.gardenRef;
      if (ref) anchors.set(ref, anchor as HTMLButtonElement);
    }
    expect(anchors.size).toBe(count);
    if (count === 34) expect(screen.queryByRole("searchbox")).not.toBeInTheDocument();
    for (const entry of memories) {
      const anchor = anchors.get(`memory:${entry.memory_id}`);
      expect(anchor).toBeDefined();
      expect(anchor!).toHaveAccessibleName(`${entry.text} Revision 1`);
    }
    const last = anchors.get(`memory:m${count}`);
    expect(last).toBeDefined();
    expect(last).toBeInstanceOf(HTMLButtonElement);
    fireEvent.click(last!);
    expect(onSelect).toHaveBeenCalledWith({ kind: "memory", id: `m${count}` });
    fireEvent.keyDown(last!, { key: "Enter" });
    expect(onEnter).toHaveBeenCalledWith({ kind: "memory", id: `m${count}` });
    await waitFor(() => expect(screen.queryByText("Loading Inbox…")).not.toBeInTheDocument());
  });

  it("does not label unavailable collections as empty", () => {
    vi.mocked(useGardenAgentContents).mockReturnValue({
      memories: { data: null, loading: false, error: "Offline", stale: false },
      conversations: { data: null, loading: false, error: "Offline", stale: false }, refresh: vi.fn(),
    });
    render(<GardenAgentInterior agent={agent} status="idle" crown={[]} agents={[]} teams={[]} automations={[]} projectedWidth={100}
      onSelect={vi.fn()} onEnter={vi.fn()} onOpenAgent={vi.fn()} />);
    expect(screen.queryByText("0 loaded memories")).not.toBeInTheDocument();
    expect(screen.queryByText("0 loaded conversations")).not.toBeInTheDocument();
  });

  it("finds hundreds of memories without removing anchors or changing canonical selection", async () => {
    const memories = Array.from({ length: 300 }, (_, index) => memory(index + 1));
    memories[298].text = "Shared NEEDLE first match";
    memories[299].text = "Shared needle last match";
    const { container, onSelect, onEnter } = setup(memories, []);
    const input = screen.getByRole("searchbox", { name: "Find memory" });
    // Global role queries recompute accessible names/styles for all 300 memory
    // buttons. Scope controls to the search region and verify canonical result
    // buttons directly, retaining accessible-name and anchor assertions.
    const search = within(input.closest<HTMLElement>(".garden-memory-search")!);
    const first = container.querySelector<HTMLButtonElement>('[data-garden-ref="memory:m299"]')!;
    const last = container.querySelector<HTMLButtonElement>('[data-garden-ref="memory:m300"]')!;
    expect(first.tagName).toBe("BUTTON");
    expect(last.tagName).toBe("BUTTON");
    expect(first).toHaveAccessibleName("Shared NEEDLE first match Revision 1");
    expect(last).toHaveAccessibleName("Shared needle last match Revision 1");
    first.scrollIntoView = vi.fn();
    last.scrollIntoView = vi.fn();
    fireEvent.change(input, { target: { value: "  needle  " } });
    expect(screen.getByText("2 matching memories")).toBeInTheDocument();
    expect(container.querySelectorAll('[data-garden-ref^="memory:"]')).toHaveLength(300);
    expect(container.querySelector('[data-garden-ref="memory:m1"]')).toHaveAttribute("aria-pressed", "true");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(first).toHaveFocus();
    expect(first).toHaveAttribute("data-garden-ref", "memory:m299");
    expect(first.scrollIntoView).toHaveBeenCalledWith({ block: "nearest", inline: "nearest" });
    expect(screen.getByText("Match 1 of 2")).toBeInTheDocument();
    fireEvent.click(search.getByRole("button", { name: "Find next memory" }));
    expect(last).toHaveFocus();
    expect(screen.getByText("Match 2 of 2")).toBeInTheDocument();
    fireEvent.click(search.getByRole("button", { name: "Find next memory" }));
    expect(first).toHaveFocus();
    expect(onSelect).not.toHaveBeenCalled();
    expect(onEnter).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "missing text" } });
    expect(screen.getByText("0 matching memories")).toBeInTheDocument();
    expect(search.getByRole("button", { name: "Find next memory" })).toBeDisabled();
    fireEvent.change(input, { target: { value: "last match" } });
    expect(screen.getByText("1 matching memory")).toBeInTheDocument();
    fireEvent.change(input, { target: { value: "" } });
    expect(search.queryByRole("button", { name: "Find next memory" })).not.toBeInTheDocument();
    expect(screen.getByText("300 loaded memories")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("Loading Inbox…")).not.toBeInTheDocument());
  });

  it.each([48, 49])("offers search only above 48 loaded records (%i)", async (count) => {
    setup(Array.from({ length: count }, (_, index) => memory(index + 1)), []);
    expect(screen.queryAllByRole("searchbox", { name: "Find memory" })).toHaveLength(count > 48 ? 1 : 0);
    await waitFor(() => expect(screen.queryByText("Loading Inbox…")).not.toBeInTheDocument());
  });
});

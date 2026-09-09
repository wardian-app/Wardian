import { useEffect, useId, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { AgentConfig, QueueItem } from "../../types";
import { useQueueStore } from "../../store/useQueueStore";
import { normalizeAgentConfig } from "../agents/configUtils";
import type { AgentTeam } from "../../layout/watchlist/types";
import type { GardenEntityRef } from "./garden.types";
import type { GardenAutomationInput } from "./gardenProjection";
import type { GardenSkillGlyph } from "./skillGlyphs";
import { normalizeEntityPath } from "./entityRef";
import { agentMonogram } from "./agentMonogram";
import { automationRunStatusColor } from "../automations/run/statusLabels";
import type { SituatedAutomationInput } from "./automationProjection";
import { useGardenAgentContents, type GardenContentState, type GardenContentsCache, type GardenConversationEntry, type GardenMemoryRecord } from "./useGardenAgentContents";
import "./garden-agent-interior.css";

export interface GardenAgentInteriorProps {
  agent: AgentConfig;
  status: string;
  crown: GardenSkillGlyph[];
  agents: AgentConfig[];
  teams: AgentTeam[];
  automations: GardenAutomationInput[];
  selectedKey?: string | null;
  onSelect: (ref: GardenEntityRef) => void;
  onEnter: (ref: GardenEntityRef) => void;
  onOpenAgent: (id: string) => void;
  /** Projected cell width; distant organelles must not start canonical readers. */
  projectedWidth?: number;
  contentsCache?: GardenContentsCache;
}

function Region({ name, children, action, count }: { name: string; children: ReactNode; action?: ReactNode; count?: number }) {
  const id = useId();
  return <section className={`garden-agent-interior-region garden-agent-interior-${name.toLowerCase().replace(/ /g, "-")}`} aria-labelledby={id}>
    <h3 id={id}>{name}{count !== undefined && <span className="garden-region-count" aria-hidden="true">{count}</span>}</h3>
    <div className="garden-agent-interior-scroll" tabIndex={0} aria-label={`${name} contents`}>{children}</div>
    {action && <div className="garden-agent-interior-primary-action">{action}</div>}
  </section>;
}

/** Keep scanning text short; the complete source remains in an explicit disclosure. */
function concise(text: string, limit = 64): string {
  const line = text.replace(/\s+/g, " ").trim();
  if (line.length <= limit) return line;
  const boundary = line.lastIndexOf(" ", limit);
  return `${line.slice(0, boundary > limit / 2 ? boundary : limit)}…`;
}

function Excerpt({ text }: { text: string }) {
  return text.length > 48 ? <details className="garden-agent-interior-disclosure">
    <summary>{concise(text, 48)}</summary><p>{text}</p>
  </details> : <p>{text}</p>;
}

function ContentNotice({ state, label }: { state: GardenContentState<unknown>; label: string }) {
  return <>
    {state.loading && <p role="status">{state.stale ? `Refreshing ${label}…` : `Loading ${label}…`}</p>}
    {state.error && <p role="status">{label} unavailable: {state.error}</p>}
    {state.stale && <p className="garden-agent-interior-note">Showing the last loaded snapshot.</p>}
  </>;
}

/** Keep every archive entry discoverable; mount prose only for expanded objects. */
function ConversationObject({ conversation }: { conversation: GardenConversationEntry }) {
  const [open, setOpen] = useState(false);
  const label = conversation.status === "open" ? "Current session" : "Recent session";
  const excerpt = conversation.last_record_excerpt || conversation.first_prompt_excerpt || "No recorded excerpt.";
  return <details className="garden-agent-interior-conversation garden-conversation-object" onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary className="garden-conversation-summary" aria-label={`${label}, ${excerpt}, ${conversation.started_at}, ${conversation.conversation_id}`}>
      <i className="garden-conversation-mark" aria-hidden="true" />
      <strong>{label}</strong>
      <time className="garden-conversation-date" dateTime={conversation.started_at} title={conversation.started_at}>{conversation.started_at.slice(0, 10)}</time>
      <span className="garden-conversation-caption" title={excerpt}>{concise(excerpt, 38)}</span>
      <small className="garden-conversation-meta" title={`Status: ${conversation.status}`}>{conversation.turn_count} turns · {conversation.artifact_count} artifacts</small>
    </summary>
    {open && <div className="garden-conversation-detail"><p>{excerpt}</p><small>{conversation.started_at} · {conversation.status}</small></div>}
  </details>;
}

/** Find within the retained grid so searching never removes or repositions canonical anchors. */
function MemorySearch({ memories }: { memories: GardenMemoryRecord[] }) {
  const root = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState("");
  const [lastFound, setLastFound] = useState<string | null>(null);
  const needle = query.trim().toLocaleLowerCase();
  const matches = needle ? memories.filter((memory) => memory.text.toLocaleLowerCase().includes(needle)) : [];
  const index = matches.findIndex((memory) => memory.memory_id === lastFound);
  const findNext = () => {
    const match = matches[(index + 1) % matches.length];
    if (!match) return;
    const scroll = root.current?.closest(".garden-agent-interior-scroll");
    const button = Array.from(scroll?.querySelectorAll<HTMLButtonElement>("[data-garden-ref]") ?? [])
      .find((element) => element.dataset.gardenRef === `memory:${match.memory_id}`);
    if (!button) return;
    button.scrollIntoView({ block: "nearest", inline: "nearest" });
    button.focus({ preventScroll: true });
    setLastFound(match.memory_id);
  };
  return <div ref={root} className="garden-memory-search" onKeyDown={(event) => event.stopPropagation()}>
    <label className="garden-memory-search-label">Find memory
      <input type="search" className="garden-memory-search-input" value={query} placeholder="Search loaded memories"
        onChange={(event) => { setQuery(event.target.value); setLastFound(null); }}
        onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); findNext(); } }} />
    </label>
    {needle && <>
      <span className="garden-memory-search-status" role="status">{index >= 0 ? `Match ${index + 1} of ${matches.length}` : `${matches.length} matching ${matches.length === 1 ? "memory" : "memories"}`}</span>
      <button type="button" className="garden-agent-interior-action garden-memory-search-next" disabled={!matches.length} onClick={findNext}>Find next memory</button>
    </>}
  </div>;
}

/** Preview the recorded execution order; neutral nodes have no run evidence yet. */
function RoutineMark({ routine }: { routine: GardenAutomationInput }) {
  const situated = (value: GardenAutomationInput): value is SituatedAutomationInput => "stages" in value && Array.isArray(value.stages);
  const stages = situated(routine) ? routine.stages : [];
  const count = Math.min(6, stages.length || routine.nodeCount);
  return <svg viewBox="0 0 120 28" style={{ color: automationRunStatusColor(routine.runStatus) }}>
    {count > 1 && <path d="M12 14H108" />}
    {Array.from({ length: count }, (_, index) => <circle key={index}
      style={{ stroke: automationRunStatusColor(stages[index]?.status ?? "none") }}
      cx={count === 1 ? 60 : 12 + index * 96 / (count - 1)} cy="14" r="5" />)}
  </svg>;
}

function queueItemStatus(item: QueueItem): string {
  if (item.notification_status === "expired") return "Expired";
  if (item.approval_decision) return `Decision: ${item.approval_decision}`;
  if (item.provider_choice_pending) return "Sending response";
  if (item.provider_choice_sent) return "Response sent";
  if (item.notification_status === "awaiting_reply" || item.automation_approval) return "Awaiting approval";
  if (item.notification_status === "completed") return "Completed";
  if (item.type === "action_needed" || item.type === "approval_request") return "Action required";
  if (item.status === "failed") return "Failed";
  if (item.type === "agent_completed" || item.type === "automation_completed") return "Completed";
  return "Update";
}

/** Inbox attribution uses stable session identity, never a display name or blueprint guess. */
function AgentQueue({ agentId }: { agentId: string }) {
  const items = useQueueStore((state) => state.items);
  const loadItems = useQueueStore((state) => state.loadItems);
  const truncated = useQueueStore((state) => state.inboxNotificationsTruncated);
  const loadMore = useQueueStore((state) => state.loadMoreInboxNotifications);
  const loadingMore = useQueueStore((state) => state.loadingMoreInboxNotifications);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let active = true;
    void loadItems().finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [loadItems]);
  const attributed = items.filter((item) => item.agent_session_id === agentId && !item.dismissed)
    .sort((left, right) => right.timestamp - left.timestamp);
  return <div className="garden-agent-interior-queue">
    <h4>Inbox</h4>
    {loading && <p role="status">Loading Inbox…</p>}
    {attributed.map((item) => <article key={item.id} className="garden-agent-interior-conversation">
      <strong>{item.notification_title || queueItemStatus(item)}</strong>
      <Excerpt text={item.summary || item.proposed_action || item.error || "No summary recorded."} />
      <small>{queueItemStatus(item)} · {item.read ? "Read" : "Unread"}</small>
    </article>)}
    {!loading && !attributed.length && <p>No attributable items in the loaded Inbox.</p>}
    {truncated && <button type="button" className="garden-agent-interior-action" disabled={loadingMore} onClick={() => { void loadMore(); }}>
      {loadingMore ? "Loading older Inbox items…" : "Load older Inbox items"}
    </button>}
  </div>;
}

const PERMISSION_FIELDS = {
  permission_mode: "Permission mode", sandbox_mode: "Sandbox", approval_policy: "Approval policy",
  sandbox: "Sandbox enabled", yolo: "Bypass approvals", approval_mode: "Approval mode",
  dangerously_skip_permissions: "Skip permission checks", mode: "Operating mode", auto: "Automatic approvals",
  full_auto: "Full auto", project_trust: "Project trust", policy: "Policy files", admin_policy: "Admin policy files",
  strict_mcp_config: "Strict MCP configuration", offline: "Offline",
};
const TOOL_FIELDS = {
  tools: "Tools", allowed_tools: "Allowed tools", disallowed_tools: "Disallowed tools",
  exclude_tools: "Excluded tools", no_tools: "Disable tools", allowed_mcp_server_names: "Allowed MCP servers",
  extensions: "Extensions", search: "Web search",
};

function ConfigurationFields({ config, fields }: { config: Record<string, unknown>; fields: Record<string, string> }) {
  const rows = Object.entries(fields).flatMap(([key, label]) => {
    const value = config[key];
    if (value === undefined || value === null) return [];
    if (typeof value === "boolean") return [[label, value ? "Yes" : "No"]];
    if (typeof value === "string") return [[label, value || "Not specified"]];
    if (Array.isArray(value) && value.every((item) => typeof item === "string")) return [[label, value.join(", ") || "None explicitly configured"]];
    return [];
  });
  return rows.length ? <dl className="garden-agent-interior-configuration">{rows.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
    : <p>No explicit overrides; provider defaults apply.</p>;
}

/** Content only: the parent owns the membrane, camera, selection, and navigation. */
export function GardenAgentInterior({ agent, status, crown, agents, teams, automations, selectedKey, onSelect, onEnter, onOpenAgent, projectedWidth = 720, contentsCache }: GardenAgentInteriorProps) {
  const [reading, setReading] = useState(projectedWidth >= 360);
  useEffect(() => { setReading((previous) => projectedWidth < 2400 && projectedWidth >= (previous ? 280 : 360)); }, [projectedWidth]);
  const contents = useGardenAgentContents(agent, reading, contentsCache);
  const normalizedConfig = normalizeAgentConfig(agent);
  const providerConfig = (normalizedConfig.provider_config ?? {}) as Record<string, unknown>;
  const workspace = agent.git_worktree_folder || agent.folder;
  const workspaceId = normalizeEntityPath(workspace);
  const memberships = teams.filter((team) => team.agentIds.includes(agent.session_id));
  const routines = automations.filter((automation) => automation.agentIds?.includes(agent.session_id));
  const peers = agents.filter((peer) => peer.session_id !== agent.session_id && (
    (workspaceId !== null && normalizeEntityPath(peer.git_worktree_folder || peer.folder) === workspaceId)
    || memberships.some((team) => team.agentIds.includes(peer.session_id))
  ));

  const record = (ref: GardenEntityRef, title: string, detail?: ReactNode, glyph?: GardenSkillGlyph, mark?: ReactNode) => <div className={`garden-agent-interior-record garden-object-${ref.kind}${glyph ? " garden-agent-interior-skill" : ""}`} key={`${ref.kind}:${ref.id}`}>
    <button type="button" data-garden-ref={`${ref.kind}:${ref.id}`} className="garden-agent-interior-select" aria-label={typeof detail === "string" ? `${title} ${detail}` : undefined} title={typeof detail === "string" ? `${title} · ${detail}` : title} aria-pressed={selectedKey === `${ref.kind}:${ref.id}`} onClick={() => onSelect(ref)} onDoubleClick={(event) => { event.stopPropagation(); onEnter(ref); }}
      onKeyDown={(event) => {
        if (event.key === "Enter") { event.preventDefault(); event.stopPropagation(); onEnter(ref); }
        if (event.key === " ") event.stopPropagation();
      }}>
      {glyph && <i className="garden-agent-interior-glyph" aria-hidden="true" style={{ "--garden-skill-hue": glyph.hue } as CSSProperties}><b>{glyph.monogram}</b></i>}
      {!glyph && ref.kind !== "identity" && <i className={`garden-object-mark garden-object-mark-${ref.kind}`} aria-hidden="true">{mark ?? (ref.kind === "agent" ? agentMonogram(title) : ref.kind === "workspace" ? "⌁" : "")}</i>}
      <strong>{ref.kind === "memory" ? concise(title, 38) : title}</strong>{detail && <> <span>{detail}</span></>}
    </button>
  </div>;

  return <div className="garden-agent-interior" onClick={(event) => event.stopPropagation()} onDoubleClick={(event) => event.stopPropagation()} onPointerDown={(event) => event.stopPropagation()}>
    <Region name="Identity" action={<button type="button" className="garden-agent-interior-action" onClick={() => onOpenAgent(agent.session_id)}>Open agent session</button>}>
      <div className="garden-agent-interior-sigil" aria-hidden="true">{agentMonogram(agent.session_name)}</div>
      {record({ kind: "identity", id: agent.session_id }, agent.session_name,
        <>{agent.agent_class} · {agent.provider || "Provider unspecified"}{agent.model ? ` · ${agent.model}` : ""}<br />{status}</>)}
      <details className="garden-agent-interior-disclosure">
      <summary>Configured permissions</summary>
      {agent.description && <p>{agent.description}</p>}
      <ConfigurationFields config={providerConfig} fields={PERMISSION_FIELDS} />
      <p className="garden-agent-interior-note">Saved configuration; runtime application may require a restart.</p>
      </details>
    </Region>
    <Region name="Capabilities" count={crown.length} action={<details className="garden-agent-interior-disclosure">
      <summary>Configured tools</summary>
      <ConfigurationFields config={providerConfig} fields={TOOL_FIELDS} />
      {typeof providerConfig.mcp_config === "string" && providerConfig.mcp_config && <p>MCP configuration supplied.</p>}
    </details>}>
      <div className="garden-object-grid garden-skill-objects">{crown.length ? crown.map((skill) => record({ kind: "skill", id: skill.entryRef }, skill.label,
        `${skill.provenance === "class" ? "Class-inherited" : skill.provenance === "global" ? "Global" : "Direct"} · ${skill.copied ? "Copied; does not sync" : "Linked"}`, skill))
        : <p>No deployed skills in this projection.</p>}</div>
    </Region>
    <Region name="Memory" count={contents.memories.data?.length}>
      <ContentNotice state={contents.memories} label="Memory" />
      {contents.memories.data && <p className="garden-collection-count garden-memory-count">{contents.memories.data.length} loaded {contents.memories.data.length === 1 ? "memory" : "memories"}</p>}
      {contents.memories.data && contents.memories.data.length > 48 && <MemorySearch key={JSON.stringify([agent.session_id, workspaceId])} memories={contents.memories.data} />}
      {!contents.memories.data && !reading && <div className="garden-memory-dormant" aria-hidden="true"><i /><i /><i /></div>}
      {(["stable", "current"] as const).map((kind) => <div key={kind} className="garden-agent-interior-memory-kind">
        <h4>{kind === "stable" ? "Stable" : "Current"}</h4>
        {([false, true] as const).map((workspaceBound) => {
          const records = contents.memories.data?.filter((memory) => memory.kind === kind && (memory.workspace !== null) === workspaceBound) ?? [];
          return records.length > 0 && <div key={String(workspaceBound)} className="garden-agent-interior-scope">
            <h5>{workspaceBound ? "Workspace-bound" : "Agent-wide"}</h5>
            <span className="garden-collection-count garden-memory-scope-count">{records.length} {records.length === 1 ? "memory" : "memories"}</span>
            <div className="garden-object-grid garden-memory-objects">{records.map((memory) =>
              record({ kind: "memory", id: memory.memory_id }, memory.text, `Revision ${memory.revision}`)
            )}</div>
          </div>;
        })}
      </div>)}
      {contents.memories.data?.length === 0 && <p>No active memories in this scope.</p>}
      <button type="button" className="garden-agent-interior-action" onClick={contents.refresh}>Refresh contents</button>
    </Region>
    <Region name="Active work">
      <ContentNotice state={contents.conversations} label="Conversations" />
      {routines.map((routine) => record({ kind: "automation", id: routine.id }, routine.label,
        `${routine.runStatus === "none" ? "Assigned routine" : routine.runStatus} · ${routine.nodeCount} stages`, undefined, <RoutineMark routine={routine} />))}
      {contents.conversations.data && <p className="garden-collection-count garden-conversation-count">{contents.conversations.data.length} loaded {contents.conversations.data.length === 1 ? "conversation" : "conversations"}</p>}
      <details className="garden-agent-interior-disclosure garden-work-evidence"><summary>Sessions & Inbox</summary>
      <div className="garden-conversation-objects">{contents.conversations.data?.map((conversation) =>
        <ConversationObject key={`${agent.session_id}:${conversation.conversation_id}`} conversation={conversation} />
      )}</div>
      {contents.conversations.data?.length === 0 && <p>No recorded conversations available.</p>}
      {reading && <AgentQueue agentId={agent.session_id} />}
      </details>
    </Region>
    <Region name="Ports">
      {workspaceId && record({ kind: "workspace", id: workspaceId }, workspace, "Workspace")}
      {memberships.map((team) => <p key={team.id}>Team · {team.name}</p>)}
      {peers.map((peer) => record({ kind: "agent", id: peer.session_id }, peer.session_name,
        memberships.some((team) => team.agentIds.includes(peer.session_id)) ? "Shared team" : "Shared workspace"))}
      {!workspaceId && !memberships.length && !peers.length && <p>No workspace or team relationships available.</p>}
    </Region>
  </div>;
}

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentConfig } from "../../types";
import type { GardenEntityRef } from "./garden.types";
import type { GardenSkillGlyph } from "./skillGlyphs";
import type { TerrainChangeEntry } from "./useTerrainChanges";
import { readGardenMemory, readGardenMemoryHistory } from "./useGardenAgentContents";
import { useFileResource } from "../files/useFileResource";
import { fileResourceClient } from "../files/fileResourceClient";
import { MarkdownDocument } from "../files/renderers/MarkdownRenderer";
import "./garden-record.css";

function RecordDate({ value }: { value?: string }) {
  const date = value ? new Date(value) : null;
  return date && !Number.isNaN(date.getTime())
    ? <time dateTime={value} title={value}>{date.toLocaleString(undefined, { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}</time>
    : <>{value || "Not recorded"}</>;
}

/** Presentation only: referenced files and images do not acquire new read capabilities. */
function RecordMarkdown({ text }: { text: string }) {
  return <div className="garden-record-document"><MarkdownDocument text={text} components={{
    a: ({ children, href }) => <span className="garden-record-reference" title={href}>{children}</span>,
    img: ({ alt }) => <span>{alt || "Image reference — see full source"}</span>,
  }} /></div>;
}

function useRecordRead<T>(key: string, read: () => Promise<T>) {
  const [result, setResult] = useState<{ key: string; value?: T; error?: string }>({ key });
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let active = true;
    void read().then((value) => { if (active) setResult({ key, value }); })
      .catch((error: unknown) => { if (active) setResult({ key, error: String(error) }); });
    return () => { active = false; };
    // The canonical key is the request identity; inline readers must not reload on paint.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, retry]);
  return { ...(result.key === key ? result : { key }), retry: () => setRetry((value) => value + 1) };
}

function RecordText({ value, error, retry, markdown = false }: { value?: string; error?: string; retry: () => void; markdown?: boolean }) {
  if (error) return <div role="alert"><p>Record unavailable: {error}</p><button onClick={retry}>Retry</button></div>;
  if (value === undefined) return <p role="status">Loading record…</p>;
  if (!value) return <p className="garden-record-empty">This record is empty.</p>;
  return markdown ? <><RecordMarkdown text={value} /><details className="garden-record-disclosure"><summary>Full source</summary><pre className="garden-record-text">{value}</pre></details></>
    : <pre className="garden-record-text">{value}</pre>;
}

function MemoryRecord({ id }: { id: string }) {
  const result = useRecordRead(id, () => Promise.all([
    readGardenMemory(id),
    readGardenMemoryHistory(id),
  ]));
  const memory = result.value?.[0];
  return <>
    {!memory && <RecordText error={result.error} retry={result.retry} />}
    {memory && <>
      <p className="garden-record-meta">{memory.kind === "stable" ? "Stable" : "Current"} · {memory.status} · Revision {memory.revision}</p>
      <dl className="garden-record-facts"><dt>Scope</dt><dd>{memory.workspace ?? "Agent-wide"}</dd><dt>Last verified</dt><dd><RecordDate value={memory.last_verified_at} /></dd></dl>
      <RecordText value={memory.text} markdown retry={result.retry} />
      <details className="garden-record-disclosure"><summary>Evidence</summary><blockquote>{memory.evidence_excerpt || "No evidence excerpt recorded."}</blockquote></details>
      <details className="garden-record-disclosure"><summary>Sources ({memory.sources.length})</summary>{memory.sources.length ? memory.sources.map((source, index) => <p key={index}>{source.source_type} · {source.locator ?? "No locator recorded"}</p>) : <p>No sources recorded.</p>}</details>
      <details className="garden-record-disclosure"><summary>Revision history ({result.value?.[1].length ?? 0})</summary>{result.value?.[1].map((revision) => <section className="garden-record-revision" key={revision.revision_id}><h3>Revision {revision.revision}</h3><p className="garden-record-meta"><RecordDate value={revision.updated_at} /></p><RecordMarkdown text={revision.text} /><blockquote>{revision.evidence_excerpt}</blockquote></section>)}</details>
    </>}
  </>;
}

function SkillRecord({ id, glyph }: { id: string; glyph?: GardenSkillGlyph }) {
  const result = useRecordRead(id, () => invoke<string>("read_library_item", { section: "skills", path: id.replace(/^skills\//, "") }));
  return <><p className="garden-record-meta">{glyph?.provenance ?? "Library"} deployment · {glyph ? glyph.copied ? "Copied deployment" : "Linked deployment" : "Deployment not loaded"}</p><RecordText {...result} markdown /></>;
}

function FileRecord({ path, change }: { path: string; change?: TerrainChangeEntry }) {
  const resource = useFileResource({ path, agent_id: null, user_file_capability_id: null });
  const snapshot = resource.snapshot;
  const content = useRecordRead(`${path}:${snapshot?.revision ?? "loading"}`, async () => {
    if (!snapshot) return undefined;
    return (await fileResourceClient.readText(snapshot)).text;
  });
  return <>
    <p className="garden-path">{path}</p>
    {change && <details className="garden-record-disclosure"><summary>Change evidence</summary><dl className="garden-record-facts"><dt>Change</dt><dd>{change.entry.change_kind} · +{change.entry.insertions ?? 0} / −{change.entry.deletions ?? 0}</dd><dt>Evidence</dt><dd>{change.entry.evidence}</dd><dt>Agents</dt><dd>{change.entry.agent_ids.join(", ") || "No attributed agent"}</dd><dt>Baseline</dt><dd>{change.baselineRef ?? "Working tree"}</dd><dt>Turns</dt><dd>{change.entry.turn_indices.join(", ") || "Unknown"}</dd></dl></details>}
    <RecordText value={content.value} error={resource.error?.message ?? content.error} retry={() => { void resource.retry(); content.retry(); }} />
  </>;
}

export function GardenRecord({ target, agent, glyph, change, onOpenAgent, onOpenSkill, onOpenPath }: {
  target: GardenEntityRef;
  agent?: AgentConfig;
  glyph?: GardenSkillGlyph;
  change?: TerrainChangeEntry;
  onOpenAgent: (id: string) => void;
  onOpenSkill: (id: string) => void;
  onOpenPath: (id: string) => void;
}) {
  const heading = target.kind === "memory" ? "Memory" : target.kind === "skill" ? "Skill" : target.kind === "path" ? "File" : "Agent";
  return <article className="garden-record garden-record-detail" aria-label={`${target.kind} record`}>
    <header className="garden-record-heading"><h2>{heading}</h2></header>
    {target.kind === "memory" && <MemoryRecord id={target.id} />}
    {target.kind === "skill" && <><p className="garden-record-title">{glyph?.label ?? target.id}</p><SkillRecord id={target.id} glyph={glyph} /><button onClick={() => onOpenSkill(target.id)}>Open in Library</button></>}
    {target.kind === "path" && <><FileRecord path={target.id} change={change} /><button onClick={() => onOpenPath(target.id)}>Open file</button></>}
    {target.kind === "identity" && (agent ? <><p className="garden-record-title">{agent.session_name}</p><p>{agent.description || "No purpose recorded."}</p><dl className="garden-record-facts"><dt>Class</dt><dd>{agent.agent_class}</dd><dt>Provider</dt><dd>{agent.provider ?? "Default provider"}</dd><dt>Model</dt><dd>{agent.model ?? "Provider default"}</dd><dt>Workspace</dt><dd>{agent.git_worktree_folder ?? agent.folder}</dd></dl><details className="garden-record-disclosure"><summary>Instructions</summary><pre className="garden-record-text">{agent.append_system_prompt || "No additional system prompt configured."}</pre></details><button onClick={() => onOpenAgent(agent.session_id)}>Open agent session</button></> : <p>This agent is no longer in the current roster.</p>)}
  </article>;
}

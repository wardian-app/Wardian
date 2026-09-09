import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { GardenEntityRef } from "./garden.types";
import { activityChildren } from "./activityFrontier";
import type { GardenTimeLens } from "./gardenNavigation";
import type { TerrainChangeEntry } from "./useTerrainChanges";
import type { TerrainPaint } from "./terrainPaint";
import { basename } from "./terrain";
import type { DirectoryTreeResult } from "../explorer/FileTree";
import "./garden-workspace-interior.css";

interface Props {
  path: string;
  entries: ReadonlyMap<string, TerrainChangeEntry>;
  paint: ReadonlyMap<string, TerrainPaint>;
  lens: GardenTimeLens;
  baseline?: string;
  onLensChange?: (lens: GardenTimeLens) => void;
  selectedKey: string | null;
  onSelect: (ref: GardenEntityRef) => void;
  onEnter: (ref: GardenEntityRef) => void;
}

/** Activity ancestry is the default; full-tree browsing is explicit and paged. */
export function GardenWorkspaceInterior({ path, entries, paint, lens, baseline = "branch_point", onLensChange, selectedKey, onSelect, onEnter }: Props) {
  const [fullTree, setFullTree] = useState(false);
  const [listing, setListing] = useState<DirectoryTreeResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  useEffect(() => {
    if (!fullTree) return;
    let active = true;
    void invoke<DirectoryTreeResult>("get_directory_tree", { path, offset: page }).then((result) => {
      if (active) { setListing(result); setError(null); }
    }).catch((reason: unknown) => { if (active) setError(String(reason)); });
    return () => { active = false; };
  }, [path, page, fullTree]);
  const activity = activityChildren(path, entries, paint, lens);
  const children = fullTree && listing
    ? listing.nodes.map((node) => ({ path: node.path, isDirectory: node.is_dir, count: paint.get(node.path)?.count ?? 0, agents: [...(paint.get(node.path)?.agentIds ?? [])] }))
    : activity;
  return <section aria-label="Workspace activity" className="garden-workspace-interior">
    <div className="garden-interior-heading"><div><h2>Workspace</h2><p className="garden-path" title={path}>{path}</p></div>
      <label><input type="checkbox" checked={fullTree} onChange={(event) => { setFullTree(event.target.checked); setPage(0); }} /> Show full tree</label>
    </div>
    <fieldset className="garden-file-activity">
      <legend>File activity</legend>
      <label>Turn range <select value={lens} disabled={fullTree} onChange={(event) => onLensChange?.(event.target.value as GardenTimeLens)}>
        <option value="now">Latest 2 turns</option>
        <option value="recent">Latest 16 turns</option>
        <option value="branch">All compared changes</option>
      </select></label>
      <p>Compared with {baseline === "head" ? "HEAD (uncommitted changes)" : "the branch point"}. Changes with uncertain recency are included.</p>
      {fullTree && <p>Showing all folder contents; the turn range applies when full-tree browsing is off.</p>}
    </fieldset>
    {error && <p role="alert">Directory unavailable: {error}</p>}
    {fullTree && !listing && !error && <p role="status">Loading folder…</p>}
    {children.length === 0 && <p>No file activity in this turn range. Show the full tree to browse workspace contents.</p>}
    <div className="garden-activity-groups">{children.map((group) => {
      const ref: GardenEntityRef = { kind: group.isDirectory ? "workspace" : "path", id: group.path };
      const evidence = paint.get(group.path);
      return <button type="button" key={group.path} data-garden-ref={`${ref.kind}:${ref.id}`} className={`garden-organelle garden-workspace-tile garden-workspace-${group.isDirectory ? "directory" : "file"}`} title={group.path} aria-pressed={selectedKey === `${ref.kind}:${ref.id}`}
        onClick={() => onSelect(ref)} onDoubleClick={() => onEnter(ref)} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); onEnter(ref); } }}>
        <svg className="garden-workspace-mark" viewBox="0 0 56 44" aria-hidden="true" focusable="false">
          {group.isDirectory ? <><path className="garden-workspace-mark-back" d="M4 12V7h19l5 5h24v26H4Z" /><path d="M4 17h48l-4 21H8Z" /></>
            : <><path d="M13 3h21l9 9v29H13Z" /><path className="garden-workspace-mark-fold" d="M34 3v9h9M20 22h16M20 28h12" /></>}
        </svg>
        <span className="garden-workspace-tile-copy">
          <span className="garden-eyebrow">{group.isDirectory ? fullTree ? "Directory" : "Activity group" : "File"}</span>
          <strong>{basename(group.path)}</strong>
          <span className="garden-workspace-counts">{group.count} changed {group.count === 1 ? "file" : "files"} · {group.agents.length} {group.agents.length === 1 ? "collaborator" : "collaborators"}</span>
        </span>
        {evidence && <span className="garden-workspace-evidence" data-change={evidence.kind} data-evidence={evidence.evidence}>{evidence.kind} · {evidence.evidence}{evidence.evidence === "inferred" || evidence.recencyKnown === false ? " · recency uncertain" : ""}</span>}
      </button>;
    })}</div>
    {fullTree && listing?.next_offset != null && <button onClick={() => setPage(listing.next_offset ?? 0)}>Next folder page</button>}
  </section>;
}

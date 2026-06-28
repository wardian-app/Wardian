import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronsUp, CircleDot, Cloud, GitBranch, ListFilter, Maximize2, Minimize2 } from "lucide-react";
import type { GitCommitChangeEntry, GitLogEntry } from "../../types";

type GraphDensity = "detailed" | "tiny";
type GraphRefFilter = "auto" | "all" | "current" | "upstream";

interface GraphMetrics {
  rowHeight: number;
  swimlaneWidth: number;
  headRadius: number;
  mergeRadius: number;
  nodeRadius: number;
  innerRadius: number;
}

const GRAPH_DENSITY_METRICS: Record<GraphDensity, GraphMetrics> = {
  detailed: {
    rowHeight: 22,
    swimlaneWidth: 11,
    headRadius: 6,
    mergeRadius: 5,
    nodeRadius: 4,
    innerRadius: 2,
  },
  tiny: {
    rowHeight: 16,
    swimlaneWidth: 8,
    headRadius: 4.5,
    mergeRadius: 3.5,
    nodeRadius: 3,
    innerRadius: 1.5,
  },
};

const GRAPH_COLORS = [
  "var(--color-wardian-accent)",
  "var(--color-wardian-processing)",
  "var(--color-wardian-warning)",
  "var(--color-wardian-success)",
  "var(--color-wardian-error)",
];

interface GitHistoryGraphProps {
  entries: GitLogEntry[];
  branch: string;
  rootPath: string;
  upstream?: string | null;
}

interface Lane {
  hash: string;
  color: string;
}

interface HistoryGraphRow {
  entry: GitLogEntry;
  inputLanes: Lane[];
  outputLanes: Lane[];
  circleIndex: number;
  circleColor: string;
}

const shortHash = (hash: string) => hash.slice(0, 8);

const storageRoot = (rootPath: string) => rootPath.trim().replace(/\\/g, "/") || "unknown-root";

const storageKey = (rootPath: string, suffix: string) =>
  `wardian:source-control:history-graph:${storageRoot(rootPath)}:${suffix}`;

const loadDensity = (rootPath: string): GraphDensity => {
  if (typeof window === "undefined") return "detailed";
  const stored = window.localStorage.getItem(storageKey(rootPath, "density"));
  return stored === "tiny" || stored === "detailed" ? stored : "detailed";
};

const saveDensity = (rootPath: string, density: GraphDensity) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(storageKey(rootPath, "density"), density);
};

const loadRefFilter = (rootPath: string): GraphRefFilter => {
  if (typeof window === "undefined") return "auto";
  const stored = window.localStorage.getItem(storageKey(rootPath, "ref-filter"));
  return stored === "all" || stored === "current" || stored === "upstream" || stored === "auto" ? stored : "auto";
};

const saveRefFilter = (rootPath: string, filter: GraphRefFilter) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(storageKey(rootPath, "ref-filter"), filter);
};

const loadExpandedHashes = (rootPath: string) => {
  if (typeof window === "undefined") return new Set<string>();

  try {
    const stored = window.localStorage.getItem(storageKey(rootPath, "expanded"));
    if (!stored) return new Set<string>();
    const parsed: unknown = JSON.parse(stored);
    if (!Array.isArray(parsed)) return new Set<string>();
    return new Set(parsed.filter((value): value is string => typeof value === "string"));
  } catch {
    return new Set<string>();
  }
};

const saveExpandedHashes = (rootPath: string, hashes: Set<string>) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(storageKey(rootPath, "expanded"), JSON.stringify([...hashes]));
};

const formatDate = (date: string) => {
  const match = date.match(/^(\d{4}-\d{2}-\d{2})/);
  return match?.[1] ?? date;
};

const changeStatusClassName = (status: string) => {
  const base = "w-4 shrink-0 text-center text-[9px] font-mono leading-[14px]";
  if (status === "A" || status === "C") return `${base} text-[var(--color-wardian-success)]`;
  if (status === "D") return `${base} text-[var(--color-wardian-error)]`;
  if (status === "R") return `${base} text-[var(--color-wardian-processing)]`;
  return `${base} text-[var(--color-wardian-warning)]`;
};

const uniqueRefsForEntry = (entry: GitLogEntry, index: number, branch: string) => {
  const refs = [...(entry.refs ?? [])];
  if (index === 0 && !refs.includes("HEAD")) refs.unshift("HEAD");
  if (index === 0 && branch && !refs.includes(branch)) refs.push(branch);
  return Array.from(new Set(refs));
};

const refsMatchFilter = (
  entry: GitLogEntry,
  index: number,
  branch: string,
  upstream: string | null | undefined,
  filter: GraphRefFilter,
) => {
  if (filter === "all" || filter === "auto") return true;

  const refs = uniqueRefsForEntry(entry, index, branch);
  if (filter === "current") return refs.includes("HEAD") || (!!branch && refs.includes(branch));
  if (filter === "upstream") return !!upstream && refs.includes(upstream);

  return true;
};

const refClassName = (ref: string, branch: string, upstream?: string | null) => {
  const base = "px-1 py-0 rounded-[2px] text-[9px] font-semibold leading-[14px] max-w-[88px] truncate";
  if (ref === "HEAD" || ref === branch) {
    return `${base} bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_78%)] text-[var(--color-wardian-accent)]`;
  }
  if (upstream && ref === upstream) {
    return `${base} bg-[color-mix(in_srgb,var(--color-wardian-processing),transparent_78%)] text-[var(--color-wardian-processing)]`;
  }
  return `${base} bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)]`;
};

const buildRows = (entries: GitLogEntry[]): HistoryGraphRow[] => {
  let lanes: Lane[] = [];

  return entries.map((entry) => {
    const parents = entry.parent_hashes ?? [];
    const inputLanes = lanes.map((lane) => ({ ...lane }));
    const existingIndex = inputLanes.findIndex((lane) => lane.hash === entry.hash);
    const circleIndex = existingIndex >= 0 ? existingIndex : 0;
    const circleColor = inputLanes[circleIndex]?.color ?? GRAPH_COLORS[0];
    const retainedLanes = inputLanes.filter((lane) => lane.hash !== entry.hash);
    const parentLanes = parents.map((hash, parentIndex) => ({
      hash,
      color: parentIndex === 0 ? circleColor : GRAPH_COLORS[(parentIndex + circleIndex + 1) % GRAPH_COLORS.length],
    }));

    const outputLanes = [
      ...retainedLanes.slice(0, circleIndex),
      ...parentLanes,
      ...retainedLanes.slice(circleIndex),
    ];
    lanes = outputLanes;

    return {
      entry,
      inputLanes,
      outputLanes,
      circleIndex,
      circleColor,
    };
  });
};

const renderGraphPaths = (row: HistoryGraphRow, metrics: GraphMetrics) => {
  const paths = [];
  const maxLaneCount = Math.max(row.inputLanes.length, row.outputLanes.length, 1);

  for (let index = 0; index < maxLaneCount; index++) {
    const lane = row.inputLanes[index] ?? row.outputLanes[index];
    if (!lane) continue;
    const x = metrics.swimlaneWidth * (index + 1);
    paths.push(
      <path
        key={`lane-${index}-${lane.hash}`}
        d={`M ${x} 0 V ${metrics.rowHeight}`}
        fill="none"
        stroke={lane.color}
        strokeWidth="1"
        strokeLinecap="round"
        opacity={lane.hash === row.entry.hash ? 1 : 0.5}
      />,
    );
  }

  row.outputLanes.forEach((lane, index) => {
    if (index === row.circleIndex) return;
    const x = metrics.swimlaneWidth * (index + 1);
    const centerX = metrics.swimlaneWidth * (row.circleIndex + 1);
    paths.push(
      <path
        key={`parent-${index}-${lane.hash}`}
        d={`M ${centerX} ${metrics.rowHeight / 2} H ${x} V ${metrics.rowHeight}`}
        fill="none"
        stroke={lane.color}
        strokeWidth="1"
        strokeLinecap="round"
      />,
    );
  });

  return paths;
};

export function GitHistoryGraph({ entries, branch, rootPath, upstream }: GitHistoryGraphProps) {
  const [density, setDensity] = useState<GraphDensity>(() => loadDensity(rootPath));
  const [refFilter, setRefFilter] = useState<GraphRefFilter>(() => loadRefFilter(rootPath));
  const [expandedHashes, setExpandedHashes] = useState<Set<string>>(() => loadExpandedHashes(rootPath));
  const [changesByHash, setChangesByHash] = useState<Record<string, GitCommitChangeEntry[]>>({});
  const [loadingHashes, setLoadingHashes] = useState<Set<string>>(() => new Set());
  const [errorByHash, setErrorByHash] = useState<Record<string, string>>({});
  const visibleEntries = useMemo(
    () => entries.filter((entry, index) => refsMatchFilter(entry, index, branch, upstream, refFilter)),
    [branch, entries, refFilter, upstream],
  );
  const rows = useMemo(() => buildRows(visibleEntries), [visibleEntries]);
  const originalIndexByHash = useMemo(
    () => new Map(entries.map((entry, index) => [entry.hash, index])),
    [entries],
  );
  const metrics = GRAPH_DENSITY_METRICS[density];

  useEffect(() => {
    setDensity(loadDensity(rootPath));
    setRefFilter(loadRefFilter(rootPath));
    setExpandedHashes(loadExpandedHashes(rootPath));
    setChangesByHash({});
    setLoadingHashes(new Set());
    setErrorByHash({});
  }, [rootPath]);

  const loadCommitChanges = useCallback(
    async (entry: GitLogEntry) => {
      if (!rootPath || changesByHash[entry.hash] || loadingHashes.has(entry.hash)) return;

      setLoadingHashes((current) => new Set(current).add(entry.hash));
      setErrorByHash((current) => {
        const next = { ...current };
        delete next[entry.hash];
        return next;
      });

      try {
        const parentHash = entry.parent_hashes?.[0] ?? null;
        const changes = await invoke<GitCommitChangeEntry[]>("git_commit_changes", {
          cwd: rootPath,
          hash: entry.hash,
          parentHash,
        });
        setChangesByHash((current) => ({ ...current, [entry.hash]: changes }));
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setErrorByHash((current) => ({
          ...current,
          [entry.hash]: message.trim() || "Unable to load commit changes.",
        }));
      } finally {
        setLoadingHashes((current) => {
          const next = new Set(current);
          next.delete(entry.hash);
          return next;
        });
      }
    },
    [changesByHash, loadingHashes, rootPath],
  );

  useEffect(() => {
    rows.forEach((row) => {
      if (!expandedHashes.has(row.entry.hash)) return;
      if (changesByHash[row.entry.hash] || loadingHashes.has(row.entry.hash) || errorByHash[row.entry.hash]) return;
      void loadCommitChanges(row.entry);
    });
  }, [changesByHash, errorByHash, expandedHashes, loadCommitChanges, loadingHashes, rows]);

  const updateDensity = (nextDensity: GraphDensity) => {
    setDensity(nextDensity);
    saveDensity(rootPath, nextDensity);
  };

  const updateRefFilter = (nextFilter: GraphRefFilter) => {
    setRefFilter(nextFilter);
    saveRefFilter(rootPath, nextFilter);
  };

  const collapseAll = () => {
    const nextExpanded = new Set<string>();
    setExpandedHashes(nextExpanded);
    saveExpandedHashes(rootPath, nextExpanded);
  };

  const toggleRow = async (entry: GitLogEntry) => {
    const nextExpanded = new Set(expandedHashes);
    if (nextExpanded.has(entry.hash)) {
      nextExpanded.delete(entry.hash);
      setExpandedHashes(nextExpanded);
      saveExpandedHashes(rootPath, nextExpanded);
      return;
    }

    nextExpanded.add(entry.hash);
    setExpandedHashes(nextExpanded);
    saveExpandedHashes(rootPath, nextExpanded);
    await loadCommitChanges(entry);
  };

  return (
    <div className="flex flex-col" aria-label="Git history graph">
      <div
        className="flex items-center justify-end gap-1 px-1 py-1 border-b border-[var(--color-wardian-border-subtle)]"
        role="toolbar"
        aria-label="History graph controls"
      >
        <button
          type="button"
          aria-label="Use auto history refs"
          aria-pressed={refFilter === "auto"}
          title="Use auto history refs"
          onClick={() => updateRefFilter("auto")}
          className="h-6 w-6 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted aria-pressed:text-[var(--color-wardian-accent)]"
        >
          <CircleDot className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Show all history refs"
          aria-pressed={refFilter === "all"}
          title="Show all history refs"
          onClick={() => updateRefFilter("all")}
          className="h-6 w-6 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted aria-pressed:text-[var(--color-wardian-accent)]"
        >
          <ListFilter className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Filter history to current branch"
          aria-pressed={refFilter === "current"}
          title="Filter history to current branch"
          onClick={() => updateRefFilter("current")}
          className="h-6 w-6 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted aria-pressed:text-[var(--color-wardian-accent)]"
        >
          <GitBranch className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Filter history to upstream"
          aria-pressed={refFilter === "upstream"}
          title="Filter history to upstream"
          onClick={() => updateRefFilter("upstream")}
          className="h-6 w-6 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted aria-pressed:text-[var(--color-wardian-accent)] disabled:opacity-40"
          disabled={!upstream}
        >
          <Cloud className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        <span className="mx-1 h-4 w-px bg-[var(--color-wardian-border-subtle)]" aria-hidden="true" />
        <button
          type="button"
          aria-label="Use detailed history density"
          aria-pressed={density === "detailed"}
          title="Use detailed history density"
          onClick={() => updateDensity("detailed")}
          className="h-6 w-6 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted aria-pressed:text-[var(--color-wardian-accent)]"
        >
          <Maximize2 className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Use tiny history density"
          aria-pressed={density === "tiny"}
          title="Use tiny history density"
          onClick={() => updateDensity("tiny")}
          className="h-6 w-6 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted aria-pressed:text-[var(--color-wardian-accent)]"
        >
          <Minimize2 className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Collapse all history rows"
          title="Collapse all history rows"
          onClick={collapseAll}
          className="h-6 w-6 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted disabled:opacity-40"
          disabled={expandedHashes.size === 0}
        >
          <ChevronsUp className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
      </div>
      {rows.length === 0 && (
        <div role="status" className="px-2 py-2 text-[11px] text-[var(--color-wardian-text-muted)]">
          No commits match this history ref filter.
        </div>
      )}
      {rows.map((row, index) => {
        const refs = uniqueRefsForEntry(row.entry, originalIndexByHash.get(row.entry.hash) ?? index, branch);
        const maxLaneCount = Math.max(row.inputLanes.length, row.outputLanes.length, 1);
        const width = metrics.swimlaneWidth * (maxLaneCount + 1);
        const circleX = metrics.swimlaneWidth * (row.circleIndex + 1);
        const isHead = refs.includes("HEAD");
        const short = shortHash(row.entry.hash);
        const isExpanded = expandedHashes.has(row.entry.hash);
        const changes = changesByHash[row.entry.hash] ?? [];
        const isLoading = loadingHashes.has(row.entry.hash);
        const isTiny = density === "tiny";

        return (
          <div key={row.entry.hash}>
            <button
              type="button"
              data-testid={`history-graph-row-${short}`}
              aria-expanded={isExpanded}
              aria-label={`${isExpanded ? "Collapse" : "Expand"} ${row.entry.message}`}
              onClick={() => void toggleRow(row.entry)}
              className="group w-full flex items-center gap-2 px-1 hover:bg-wardian-card-bg-muted rounded cursor-default min-w-0 text-left"
              style={{ height: `${metrics.rowHeight}px` }}
            >
              <svg
                data-testid={`history-graph-svg-${short}`}
                width={width}
                height={metrics.rowHeight}
                viewBox={`0 0 ${width} ${metrics.rowHeight}`}
                className="shrink-0"
                aria-hidden="true"
              >
                {renderGraphPaths(row, metrics)}
                <circle
                  cx={circleX}
                  cy={metrics.rowHeight / 2}
                  r={
                    isHead
                      ? metrics.headRadius
                      : row.entry.parent_hashes && row.entry.parent_hashes.length > 1
                        ? metrics.mergeRadius
                        : metrics.nodeRadius
                  }
                  fill={isHead ? row.circleColor : "var(--color-wardian-card)"}
                  stroke={row.circleColor}
                  strokeWidth={isHead ? 2 : 1.5}
                />
                {isHead && (
                  <circle
                    cx={circleX}
                    cy={metrics.rowHeight / 2}
                    r={metrics.innerRadius}
                    fill="var(--color-wardian-card)"
                  />
                )}
              </svg>
              <div className="min-w-0 flex-1 flex items-center gap-1.5">
                <span className={`${isTiny ? "text-[10px] leading-[14px]" : "text-[11px] leading-[18px]"} text-primary truncate`}>
                  {row.entry.message}
                </span>
                {!isTiny && refs.length > 0 && (
                  <span className="min-w-0 flex items-center gap-1 overflow-hidden">
                    {refs.map((ref) => (
                      <span key={ref} className={refClassName(ref, branch, upstream)} title={ref}>
                        {ref}
                      </span>
                    ))}
                  </span>
                )}
              </div>
              {!isTiny && (
                <span className="hidden min-[360px]:inline text-[9px] text-[var(--color-wardian-text-muted)] truncate max-w-[96px]">
                  {row.entry.author} · {formatDate(row.entry.date)}
                </span>
              )}
              <span className="text-[9px] font-mono text-[var(--color-wardian-text-muted)] shrink-0 opacity-70 group-hover:opacity-100">
                {short}
              </span>
            </button>
            {isExpanded && (
              <div>
                {isLoading && (
                  <div className="flex items-center gap-2 px-1 text-[11px] text-[var(--color-wardian-text-muted)]" style={{ height: `${metrics.rowHeight}px` }}>
                    <GraphPlaceholder width={width} lanes={row.outputLanes} highlightIndex={row.circleIndex} metrics={metrics} />
                    Loading changes...
                  </div>
                )}
                {errorByHash[row.entry.hash] && (
                  <div className="flex items-center gap-2 px-1 text-[11px] text-[var(--color-wardian-error)]" style={{ height: `${metrics.rowHeight}px` }}>
                    <GraphPlaceholder width={width} lanes={row.outputLanes} highlightIndex={row.circleIndex} metrics={metrics} />
                    {errorByHash[row.entry.hash]}
                  </div>
                )}
                {changes.map((change) => (
                  <div
                    key={`${row.entry.hash}-${change.path}`}
                    data-testid={`history-graph-change-row-${short}-${change.path}`}
                    className="flex items-center gap-2 px-1 hover:bg-wardian-card-bg-muted rounded min-w-0"
                    style={{ height: `${metrics.rowHeight}px` }}
                  >
                    <GraphPlaceholder
                      testId={`history-graph-change-placeholder-${short}-${change.path}`}
                      width={width}
                      lanes={row.outputLanes}
                      highlightIndex={row.circleIndex}
                      metrics={metrics}
                    />
                    <span className={changeStatusClassName(change.status)}>{change.status}</span>
                    <span className="min-w-0 flex-1 truncate text-[11px] text-[var(--color-wardian-text-muted)]">
                      {change.path}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function GraphPlaceholder({
  lanes,
  width,
  highlightIndex,
  testId,
  metrics,
}: {
  lanes: Lane[];
  width: number;
  highlightIndex: number;
  testId?: string;
  metrics: GraphMetrics;
}) {
  return (
    <svg
      data-testid={testId}
      width={width}
      height={metrics.rowHeight}
      viewBox={`0 0 ${width} ${metrics.rowHeight}`}
      className="shrink-0"
      aria-hidden="true"
    >
      {lanes.map((lane, index) => (
        <path
          key={`${lane.hash}-${index}`}
          d={`M ${metrics.swimlaneWidth * (index + 1)} 0 V ${metrics.rowHeight}`}
          fill="none"
          stroke={lane.color}
          strokeWidth={index === highlightIndex ? 2 : 1}
          strokeLinecap="round"
          opacity={index === highlightIndex ? 0.9 : 0.45}
        />
      ))}
    </svg>
  );
}

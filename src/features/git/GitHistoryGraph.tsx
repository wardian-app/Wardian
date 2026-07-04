import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  ChevronDown,
  ChevronRight,
  ChevronsUp,
  Copy,
  Eye,
  FileText,
  GitBranch,
  List,
  ListTree,
  Target,
} from "lucide-react";
import type { GitCommitChangeEntry, GitLogEntry } from "../../types";
import { ContextMenu, type ContextMenuItem } from "../../components/ContextMenu";

export type GraphRefFilter = "auto" | "all" | "current" | "upstream" | `ref:${string}`;
type GraphChangeViewMode = "tree" | "list";
type GraphBadgeMode = "filter" | "all";

interface GraphMetrics {
  rowHeight: number;
  swimlaneWidth: number;
  headRadius: number;
  mergeRadius: number;
  nodeRadius: number;
  innerRadius: number;
}

const GRAPH_METRICS: GraphMetrics = {
  rowHeight: 22,
  swimlaneWidth: 11,
  headRadius: 6,
  mergeRadius: 5,
  nodeRadius: 4,
  innerRadius: 2,
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
  ahead?: number;
  behind?: number;
  selectedRefFilter?: GraphRefFilter;
  hasMoreHistory?: boolean;
  isLoadingMoreHistory?: boolean;
  onRefFilterChange?: (filter: GraphRefFilter) => void;
  onLoadMoreHistory?: () => void;
  onOpenHistoryFile?: (entry: GitLogEntry, change: GitCommitChangeEntry) => void;
  onViewHistoryChanges?: (entry: GitLogEntry) => void;
}

interface Lane {
  hash: string;
  color: string;
}

interface HistoryGraphRow {
  entry: HistoryGraphEntry;
  inputLanes: Lane[];
  outputLanes: Lane[];
  circleIndex: number;
  circleColor: string;
  kind: HistoryGraphRowKind;
}

type HistoryGraphRowKind = "commit" | "incoming-changes" | "outgoing-changes";

interface HistoryGraphEntry extends GitLogEntry {
  graphKind?: HistoryGraphRowKind;
  syntheticCount?: number;
}

interface ChangeTreeDirectory {
  type: "directory";
  name: string;
  path: string;
  children: ChangeTreeNode[];
}

interface ChangeTreeFile {
  type: "file";
  name: string;
  path: string;
  change: GitCommitChangeEntry;
}

type ChangeTreeNode = ChangeTreeDirectory | ChangeTreeFile;

const INCOMING_CHANGES_HASH = "__wardian_incoming_changes__";
const OUTGOING_CHANGES_HASH = "__wardian_outgoing_changes__";

const shortHash = (hash: string) => hash.slice(0, 8);

const graphRowId = (row: HistoryGraphRow) => {
  if (row.kind === "incoming-changes") return "incoming-changes";
  if (row.kind === "outgoing-changes") return "outgoing-changes";
  return shortHash(row.entry.hash);
};

const commitCountLabel = (count: number) => `${count} ${count === 1 ? "commit" : "commits"}`;

const storageRoot = (rootPath: string) => rootPath.trim().replace(/\\/g, "/") || "unknown-root";

const storageKey = (rootPath: string, suffix: string) =>
  `wardian:source-control:history-graph:${storageRoot(rootPath)}:${suffix}`;

export const loadRefFilter = (rootPath: string): GraphRefFilter => {
  if (typeof window === "undefined") return "auto";
  const stored = window.localStorage.getItem(storageKey(rootPath, "ref-filter"));
  if (stored?.startsWith("ref:") && stored.length > "ref:".length) return stored as GraphRefFilter;
  return stored === "all" || stored === "current" || stored === "upstream" || stored === "auto" ? stored : "auto";
};

export const saveRefFilter = (rootPath: string, filter: GraphRefFilter) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(storageKey(rootPath, "ref-filter"), filter);
};

const loadBadgeMode = (rootPath: string): GraphBadgeMode => {
  if (typeof window === "undefined") return "filter";
  const stored = window.localStorage.getItem(storageKey(rootPath, "badge-mode"));
  return stored === "all" || stored === "filter" ? stored : "filter";
};

const saveBadgeMode = (rootPath: string, mode: GraphBadgeMode) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(storageKey(rootPath, "badge-mode"), mode);
};

const loadChangeViewMode = (rootPath: string): GraphChangeViewMode => {
  if (typeof window === "undefined") return "tree";
  const stored = window.localStorage.getItem(storageKey(rootPath, "change-view-mode"));
  return stored === "list" || stored === "tree" ? stored : "tree";
};

const saveChangeViewMode = (rootPath: string, mode: GraphChangeViewMode) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(storageKey(rootPath, "change-view-mode"), mode);
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

const changeStatusClassName = (status: string) => {
  const base = "w-4 shrink-0 text-center text-[9px] font-mono leading-[14px]";
  if (status === "A" || status === "C") return `${base} text-[var(--color-wardian-success)]`;
  if (status === "D") return `${base} text-[var(--color-wardian-error)]`;
  if (status === "R") return `${base} text-[var(--color-wardian-processing)]`;
  return `${base} text-[var(--color-wardian-warning)]`;
};

const splitChangePath = (path: string) => path.replace(/\\/g, "/").split("/").filter(Boolean);

const buildChangeTree = (changes: GitCommitChangeEntry[]): ChangeTreeNode[] => {
  const root: ChangeTreeDirectory = { type: "directory", name: "", path: "", children: [] };
  const directories = new Map<string, ChangeTreeDirectory>([["", root]]);

  changes.forEach((change) => {
    const parts = splitChangePath(change.path);
    if (parts.length === 0) {
      root.children.push({ type: "file", name: change.path, path: change.path, change });
      return;
    }

    let current = root;
    parts.slice(0, -1).forEach((part, index) => {
      const path = parts.slice(0, index + 1).join("/");
      const existing = directories.get(path);
      if (existing) {
        current = existing;
        return;
      }

      const directory: ChangeTreeDirectory = { type: "directory", name: part, path, children: [] };
      directories.set(path, directory);
      current.children.push(directory);
      current = directory;
    });

    const name = parts[parts.length - 1] ?? change.path;
    current.children.push({ type: "file", name, path: parts.join("/"), change });
  });

  const sortNodes = (nodes: ChangeTreeNode[]): ChangeTreeNode[] =>
    [...nodes]
      .sort((left, right) => {
        if (left.type !== right.type) return left.type === "directory" ? -1 : 1;
        return left.name.localeCompare(right.name);
      })
      .map((node) => (node.type === "directory" ? { ...node, children: sortNodes(node.children) } : node));

  return sortNodes(root.children);
};

const uniqueRefsForEntry = (entry: GitLogEntry, index: number, branch: string) => {
  const refs = [...(entry.refs ?? [])];
  if (index === 0 && !refs.includes("HEAD")) refs.unshift("HEAD");
  if (index === 0 && branch && !refs.includes(branch)) refs.push(branch);
  return Array.from(new Set(refs));
};

const refsForBadgeMode = (
  refs: string[],
  branch: string,
  upstream: string | null | undefined,
  refFilter: GraphRefFilter,
  badgeMode: GraphBadgeMode,
) => {
  if (badgeMode === "all" || refFilter === "all") return refs;

  if (refFilter === "current") {
    return refs.filter((ref) => ref === "HEAD" || (!!branch && ref === branch));
  }

  if (refFilter === "upstream") {
    return refs.filter((ref) => !!upstream && ref === upstream);
  }

  if (refFilter.startsWith("ref:")) {
    const selectedRef = refFilter.slice("ref:".length);
    return refs.filter((ref) => ref === selectedRef);
  }

  return refs.filter((ref) => ref === "HEAD" || (!!branch && ref === branch) || (!!upstream && ref === upstream));
};

const meaningfulRowRefs = (refs: string[], branch: string) =>
  refs.filter((ref) => ref !== "HEAD" && ref !== branch);

const refsForRowBadges = (refs: string[], branch: string) =>
  refs.filter((ref) => ref !== "HEAD" && ref !== branch).slice(0, 1);

const historyGraphTooltipId = (rowId: string) => `history-graph-tooltip-${rowId}`;

const refFilterLabel = (filter: GraphRefFilter) => {
  if (filter === "all") return "All";
  if (filter === "current") return "Current Branch";
  if (filter === "upstream") return "Upstream";
  if (filter.startsWith("ref:")) return filter.slice("ref:".length);
  return "Auto";
};

const syntheticEntry = (
  kind: Exclude<HistoryGraphRowKind, "commit">,
  count: number,
  parentHash: string | undefined,
): HistoryGraphEntry => ({
  hash: kind === "incoming-changes" ? INCOMING_CHANGES_HASH : OUTGOING_CHANGES_HASH,
  graphKind: kind,
  syntheticCount: count,
  message: kind === "incoming-changes" ? "Incoming Changes" : "Outgoing Changes",
  author: commitCountLabel(count),
  date: "",
  parent_hashes: parentHash ? [parentHash] : [],
  refs: [],
});

const buildVisibleHistoryEntries = (
  entries: GitLogEntry[],
  branch: string,
  upstream: string | null | undefined,
  refFilter: GraphRefFilter,
  ahead: number,
  behind: number,
): HistoryGraphEntry[] => {
  const visibleEntries: HistoryGraphEntry[] = entries.map((entry) => ({ ...entry, graphKind: "commit" }));
  const headEntry = entries[0];
  const upstreamEntry = upstream
    ? entries.find((entry, index) => uniqueRefsForEntry(entry, index, branch).includes(upstream))
    : undefined;
  const showDivergenceNodes = refFilter === "auto" || refFilter === "all" || refFilter === "current";

  if (showDivergenceNodes && ahead > 0 && headEntry) {
    const headIndex = visibleEntries.findIndex((entry) => entry.hash === headEntry.hash);
    if (headIndex !== -1) {
      visibleEntries.splice(headIndex, 0, syntheticEntry("outgoing-changes", ahead, headEntry.hash));
    }
  }

  if (showDivergenceNodes && behind > 0 && upstreamEntry) {
    const upstreamIndex = visibleEntries.findIndex((entry) => entry.hash === upstreamEntry.hash);
    if (upstreamIndex !== -1) {
      visibleEntries.splice(upstreamIndex, 0, syntheticEntry("incoming-changes", behind, upstreamEntry.hash));
    }
  }

  return visibleEntries;
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

const buildRows = (entries: HistoryGraphEntry[]): HistoryGraphRow[] => {
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
      kind: entry.graphKind ?? "commit",
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

export function GitHistoryGraph({
  entries,
  branch,
  rootPath,
  upstream,
  ahead = 0,
  behind = 0,
  selectedRefFilter,
  hasMoreHistory = false,
  isLoadingMoreHistory = false,
  onRefFilterChange,
  onLoadMoreHistory,
  onOpenHistoryFile,
  onViewHistoryChanges,
}: GitHistoryGraphProps) {
  const [localRefFilter, setLocalRefFilter] = useState<GraphRefFilter>(() => loadRefFilter(rootPath));
  const [badgeMode, setBadgeMode] = useState<GraphBadgeMode>(() => loadBadgeMode(rootPath));
  const [changeViewMode, setChangeViewMode] = useState<GraphChangeViewMode>(() => loadChangeViewMode(rootPath));
  const [expandedHashes, setExpandedHashes] = useState<Set<string>>(() => loadExpandedHashes(rootPath));
  const [changesByHash, setChangesByHash] = useState<Record<string, GitCommitChangeEntry[]>>({});
  const [loadingHashes, setLoadingHashes] = useState<Set<string>>(() => new Set());
  const [errorByHash, setErrorByHash] = useState<Record<string, string>>({});
  const [collapsedChangeFolders, setCollapsedChangeFolders] = useState<Set<string>>(() => new Set());
  const [activeDetailRowId, setActiveDetailRowId] = useState<string | null>(null);
  const [historyContextMenu, setHistoryContextMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(
    null,
  );
  const rowRefs = useRef<Record<string, HTMLButtonElement | null>>({});
  const refFilter = selectedRefFilter ?? localRefFilter;
  const visibleEntries = useMemo(
    () => buildVisibleHistoryEntries(entries, branch, upstream, refFilter, ahead, behind),
    [ahead, behind, branch, entries, refFilter, upstream],
  );
  const rows = useMemo(() => buildRows(visibleEntries), [visibleEntries]);
  const currentHash = entries[0]?.hash;
  const currentRow = currentHash ? rows.find((row) => row.kind === "commit" && row.entry.hash === currentHash) : undefined;
  const originalIndexByHash = useMemo(
    () => new Map(entries.map((entry, index) => [entry.hash, index])),
    [entries],
  );
  const availableRefs = useMemo(
    () =>
      Array.from(
        new Set(entries.flatMap((entry, index) => uniqueRefsForEntry(entry, index, branch))),
      ).filter((ref) => ref !== "HEAD"),
    [branch, entries],
  );
  const metrics = GRAPH_METRICS;

  useEffect(() => {
    setLocalRefFilter(loadRefFilter(rootPath));
    setBadgeMode(loadBadgeMode(rootPath));
    setChangeViewMode(loadChangeViewMode(rootPath));
    setExpandedHashes(loadExpandedHashes(rootPath));
    setChangesByHash({});
    setLoadingHashes(new Set());
    setErrorByHash({});
    setCollapsedChangeFolders(new Set());
    setActiveDetailRowId(null);
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
      if (row.kind !== "commit") return;
      if (changesByHash[row.entry.hash] || loadingHashes.has(row.entry.hash) || errorByHash[row.entry.hash]) return;
      void loadCommitChanges(row.entry);
    });
  }, [changesByHash, errorByHash, expandedHashes, loadCommitChanges, loadingHashes, rows]);

  const updateRefFilter = (nextFilter: GraphRefFilter) => {
    setLocalRefFilter(nextFilter);
    saveRefFilter(rootPath, nextFilter);
    onRefFilterChange?.(nextFilter);
  };

  const updateBadgeMode = (nextMode: GraphBadgeMode) => {
    setBadgeMode(nextMode);
    saveBadgeMode(rootPath, nextMode);
  };

  const updateChangeViewMode = (nextMode: GraphChangeViewMode) => {
    setChangeViewMode(nextMode);
    saveChangeViewMode(rootPath, nextMode);
  };

  const collapseAll = () => {
    const nextExpanded = new Set<string>();
    setExpandedHashes(nextExpanded);
    saveExpandedHashes(rootPath, nextExpanded);
  };

  const revealCurrentHistoryItem = () => {
    if (!currentRow) return;

    const element = rowRefs.current[graphRowId(currentRow)];
    if (!element) return;

    element.scrollIntoView({ block: "center", inline: "nearest" });
    element.focus({ preventScroll: true });
  };

  const toggleChangeFolder = (hash: string, path: string) => {
    const key = `${hash}:${path}`;
    setCollapsedChangeFolders((current) => {
      const next = new Set(current);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
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

  const viewCommitChanges = async (entry: GitLogEntry) => {
    if (!expandedHashes.has(entry.hash)) {
      const nextExpanded = new Set(expandedHashes).add(entry.hash);
      setExpandedHashes(nextExpanded);
      saveExpandedHashes(rootPath, nextExpanded);
    }

    await loadCommitChanges(entry);
  };

  const viewHistoryItemChanges = (entry: GitLogEntry) => {
    if (onViewHistoryChanges) {
      onViewHistoryChanges(entry);
      return;
    }

    void viewCommitChanges(entry);
  };

  const copyToClipboard = (text: string) => {
    void navigator.clipboard?.writeText(text);
  };

  const openHistoryRefMenu = (event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();

    const refItems: ContextMenuItem[] = [
      {
        label: "Auto",
        icon: refFilter === "auto" ? <Check className="h-3.5 w-3.5" /> : <GitBranch className="h-3.5 w-3.5" />,
        onClick: () => updateRefFilter("auto"),
      },
      {
        label: "All",
        icon: refFilter === "all" ? <Check className="h-3.5 w-3.5" /> : <GitBranch className="h-3.5 w-3.5" />,
        onClick: () => updateRefFilter("all"),
      },
      {
        label: "Current Branch",
        icon: refFilter === "current" ? <Check className="h-3.5 w-3.5" /> : <GitBranch className="h-3.5 w-3.5" />,
        onClick: () => updateRefFilter("current"),
      },
    ];

    if (upstream) {
      refItems.push({
        label: "Upstream",
        icon: refFilter === "upstream" ? <Check className="h-3.5 w-3.5" /> : <GitBranch className="h-3.5 w-3.5" />,
        onClick: () => updateRefFilter("upstream"),
      });
    }

    setHistoryContextMenu({
      x: event.clientX,
      y: event.clientY,
      items: [
        ...refItems,
        ...(availableRefs.length > 0
          ? [
            { divider: true },
            ...availableRefs.map((ref) => ({
              label: ref,
              icon: refFilter === `ref:${ref}` ? <Check className="h-3.5 w-3.5" /> : <GitBranch className="h-3.5 w-3.5" />,
              onClick: () => updateRefFilter(`ref:${ref}`),
            })),
          ]
          : []),
        { divider: true },
        {
          label: badgeMode === "filter" ? "Show All Ref Badges" : "Show Filtered Ref Badges",
          icon: <GitBranch className="h-3.5 w-3.5" />,
          onClick: () => updateBadgeMode(badgeMode === "filter" ? "all" : "filter"),
        },
      ],
    });
  };

  const openHistoryContextMenu = (event: MouseEvent, row: HistoryGraphRow) => {
    if (row.kind !== "commit") return;

    event.preventDefault();
    event.stopPropagation();

    setHistoryContextMenu({
      x: event.clientX,
      y: event.clientY,
      items: [
        {
          label: "View Changes",
          icon: <Eye className="h-3.5 w-3.5" />,
          onClick: () => viewHistoryItemChanges(row.entry),
        },
        { divider: true },
        {
          label: "Copy Commit ID",
          icon: <Copy className="h-3.5 w-3.5" />,
          onClick: () => copyToClipboard(row.entry.hash),
        },
        {
          label: "Copy Commit Message",
          icon: <FileText className="h-3.5 w-3.5" />,
          onClick: () => copyToClipboard(row.entry.message),
        },
      ],
    });
  };

  const openHistoryChangeContextMenu = (
    event: MouseEvent,
    entry: GitLogEntry,
    change: GitCommitChangeEntry,
  ) => {
    if (!onOpenHistoryFile) return;

    event.preventDefault();
    event.stopPropagation();

    setHistoryContextMenu({
      x: event.clientX,
      y: event.clientY,
      items: [
        {
          label: "Open File",
          icon: <FileText className="h-3.5 w-3.5" />,
          onClick: () => onOpenHistoryFile(entry, change),
        },
      ],
    });
  };

  const renderChangeRow = (
    row: HistoryGraphRow,
    width: number,
    short: string,
    change: GitCommitChangeEntry,
    displayPath: string,
    depth = 0,
  ) => (
    <div
      key={`${row.entry.hash}-${change.path}`}
      data-testid={`history-graph-change-row-${short}-${change.path}`}
      className="group/history-change flex w-full items-center gap-1 rounded px-1 hover:bg-wardian-card-bg-muted"
      style={{ height: `${metrics.rowHeight}px` }}
      onContextMenu={(event) => openHistoryChangeContextMenu(event, row.entry, change)}
    >
      <button
        type="button"
        aria-label={`Open ${change.path} from ${short}`}
        disabled={!onOpenHistoryFile}
        onClick={() => onOpenHistoryFile?.(row.entry, change)}
        className="flex min-w-0 flex-1 items-center gap-2 rounded text-left disabled:cursor-default disabled:hover:bg-transparent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--color-wardian-accent)]"
      >
        <GraphPlaceholder
          testId={`history-graph-change-placeholder-${short}-${change.path}`}
          width={width}
          lanes={row.outputLanes}
          highlightIndex={row.circleIndex}
          metrics={metrics}
        />
        <span aria-hidden="true" style={{ width: `${depth * 12}px` }} className="shrink-0" />
        <span className={changeStatusClassName(change.status)}>{change.status}</span>
        <span
          className="min-w-0 flex-1 truncate text-[11px] text-[var(--color-wardian-text-muted)]"
          title={change.path}
        >
          {displayPath}
        </span>
      </button>
      {onOpenHistoryFile && (
        <button
          type="button"
          aria-label={`Open File for ${change.path} from ${short}`}
          title="Open File"
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            onOpenHistoryFile(row.entry, change);
          }}
          className="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded text-[var(--color-wardian-text-muted)] opacity-0 hover:bg-wardian-card-bg-muted hover:text-primary focus:opacity-100 focus:outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)] group-hover/history-change:opacity-100 group-focus-within/history-change:opacity-100"
        >
          <FileText className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
      )}
    </div>
  );

  const renderChangeTreeNodes = (
    row: HistoryGraphRow,
    width: number,
    short: string,
    nodes: ChangeTreeNode[],
    depth = 0,
  ): ReactNode =>
    nodes.map((node) => {
      if (node.type === "file") {
        return renderChangeRow(row, width, short, node.change, node.name, depth);
      }

      const key = `${row.entry.hash}:${node.path}`;
      const isOpen = !collapsedChangeFolders.has(key);
      return (
        <div key={`${row.entry.hash}-${node.path}`}>
          <div
            className="flex items-center gap-2 px-1 hover:bg-wardian-card-bg-muted rounded min-w-0"
            style={{ height: `${metrics.rowHeight}px` }}
          >
            <GraphPlaceholder
              width={width}
              lanes={row.outputLanes}
              highlightIndex={row.circleIndex}
              metrics={metrics}
            />
            <button
              type="button"
              aria-expanded={isOpen}
              onClick={() => toggleChangeFolder(row.entry.hash, node.path)}
              className="min-w-0 flex flex-1 items-center gap-1 rounded text-left text-[11px] text-[var(--color-wardian-text-muted)] hover:text-primary"
              style={{ paddingLeft: `${depth * 12}px` }}
              title={node.path}
            >
              <ChevronRight
                className={`h-3 w-3 shrink-0 transition-transform ${isOpen ? "rotate-90" : ""}`}
                aria-hidden="true"
              />
              <span className="truncate">{node.name}</span>
            </button>
          </div>
          {isOpen && renderChangeTreeNodes(row, width, short, node.children, depth + 1)}
        </div>
      );
    });

  const renderChangeViewControls = (row: HistoryGraphRow, width: number) => (
    <div
      className="flex items-center gap-2 px-1 text-[11px] text-[var(--color-wardian-text-muted)]"
      style={{ height: `${metrics.rowHeight}px` }}
    >
      <GraphPlaceholder width={width} lanes={row.outputLanes} highlightIndex={row.circleIndex} metrics={metrics} />
      <span className="min-w-0 flex-1 truncate">Changed files</span>
      <button
        type="button"
        aria-label="View history changes as tree"
        aria-pressed={changeViewMode === "tree"}
        title="View history changes as tree"
        onClick={() => updateChangeViewMode("tree")}
        className="h-5 w-5 shrink-0 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted aria-pressed:text-[var(--color-wardian-accent)]"
      >
        <ListTree className="h-3.5 w-3.5" aria-hidden="true" />
      </button>
      <button
        type="button"
        aria-label="View history changes as list"
        aria-pressed={changeViewMode === "list"}
        title="View history changes as list"
        onClick={() => updateChangeViewMode("list")}
        className="h-5 w-5 shrink-0 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted aria-pressed:text-[var(--color-wardian-accent)]"
      >
        <List className="h-3.5 w-3.5" aria-hidden="true" />
      </button>
    </div>
  );

  return (
    <div className="flex flex-col" aria-label="Git history graph">
      <div
        className="flex items-center justify-end gap-1 px-1 py-1 border-b border-[var(--color-wardian-border-subtle)]"
        role="toolbar"
        aria-label="History graph controls"
      >
        <button
          type="button"
          aria-label={`History refs: ${refFilterLabel(refFilter)}`}
          aria-haspopup="menu"
          title={`History refs: ${refFilterLabel(refFilter)}`}
          onClick={openHistoryRefMenu}
          className="h-6 max-w-[116px] inline-flex items-center justify-center gap-1 rounded px-1.5 text-[11px] text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted"
        >
          <GitBranch className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span className="truncate">{refFilterLabel(refFilter)}</span>
          <ChevronDown className="h-3 w-3 shrink-0 opacity-70" aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Go to Current History Item"
          title="Go to Current History Item"
          onClick={revealCurrentHistoryItem}
          className="h-6 w-6 inline-flex items-center justify-center rounded text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted disabled:opacity-40"
          disabled={!currentRow}
        >
          <Target className="h-3.5 w-3.5" aria-hidden="true" />
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
          No commits found for this history selection.
        </div>
      )}
      {rows.map((row, index) => {
        const refs = uniqueRefsForEntry(row.entry, originalIndexByHash.get(row.entry.hash) ?? index, branch);
        const badgeModeRefs = refsForBadgeMode(refs, branch, upstream, refFilter, badgeMode);
        const badgeRefs = badgeMode === "all"
          ? meaningfulRowRefs(badgeModeRefs, branch)
          : refsForRowBadges(badgeModeRefs, branch);
        const maxLaneCount = Math.max(row.inputLanes.length, row.outputLanes.length, 1);
        const width = metrics.swimlaneWidth * (maxLaneCount + 1);
        const circleX = metrics.swimlaneWidth * (row.circleIndex + 1);
        const isHead = refs.includes("HEAD");
        const rowId = graphRowId(row);
        const isSynthetic = row.kind !== "commit";
        const short = shortHash(row.entry.hash);
        const isExpanded = !isSynthetic && expandedHashes.has(row.entry.hash);
        const showDetails = !isSynthetic && activeDetailRowId === rowId;
        const changes = changesByHash[row.entry.hash] ?? [];
        const isLoading = loadingHashes.has(row.entry.hash);
        const isDivergenceNode = row.kind === "incoming-changes" || row.kind === "outgoing-changes";
        const parentSummary = row.entry.parent_hashes?.length
          ? row.entry.parent_hashes.map(shortHash).join(", ")
          : "None";
        const refsSummary = refs.length > 0 ? refs.join(", ") : "None";

        return (
          <div key={row.entry.hash} className="group relative">
            <button
              type="button"
              ref={(element) => {
                rowRefs.current[rowId] = element;
              }}
              data-testid={`history-graph-row-${rowId}`}
              aria-describedby={showDetails ? historyGraphTooltipId(rowId) : undefined}
              aria-expanded={isSynthetic ? undefined : isExpanded}
              aria-current={isHead ? "true" : undefined}
              aria-label={
                isSynthetic
                  ? `${row.entry.message}, ${commitCountLabel(row.entry.syntheticCount ?? 0)}`
                  : `${isExpanded ? "Collapse" : "Expand"} ${row.entry.message}`
              }
              onClick={() => {
                if (!isSynthetic) void toggleRow(row.entry);
              }}
              onContextMenu={(event) => openHistoryContextMenu(event, row)}
              onMouseEnter={() => {
                if (!isSynthetic) setActiveDetailRowId(rowId);
              }}
              onMouseLeave={() => {
                setActiveDetailRowId((current) => (current === rowId ? null : current));
              }}
              onFocus={() => {
                if (!isSynthetic) setActiveDetailRowId(rowId);
              }}
              onBlur={() => {
                setActiveDetailRowId((current) => (current === rowId ? null : current));
              }}
              className="w-full flex items-center gap-2 px-1 pr-7 hover:bg-wardian-card-bg-muted rounded cursor-default min-w-0 text-left"
              style={{ height: `${metrics.rowHeight}px` }}
            >
              <svg
                data-testid={`history-graph-svg-${rowId}`}
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
                    isHead || isDivergenceNode
                      ? metrics.headRadius
                      : row.entry.parent_hashes && row.entry.parent_hashes.length > 1
                        ? metrics.mergeRadius
                        : metrics.nodeRadius
                  }
                  fill={isHead || isDivergenceNode ? row.circleColor : "var(--color-wardian-card)"}
                  stroke={row.circleColor}
                  strokeWidth={isHead || isDivergenceNode ? 2 : 1.5}
                />
                {(isHead || isDivergenceNode) && (
                  <circle
                    cx={circleX}
                    cy={metrics.rowHeight / 2}
                    r={isDivergenceNode ? metrics.mergeRadius : metrics.innerRadius}
                    fill="var(--color-wardian-card)"
                    stroke={isDivergenceNode ? row.circleColor : undefined}
                    strokeWidth={isDivergenceNode ? 3 : undefined}
                  />
                )}
                {isDivergenceNode && (
                  <circle
                    cx={circleX}
                    cy={metrics.rowHeight / 2}
                    r={metrics.mergeRadius}
                    fill="none"
                    stroke={row.circleColor}
                    strokeWidth={1}
                    strokeDasharray="4,2"
                  />
                )}
              </svg>
              <div className="min-w-0 flex-1 flex items-center gap-1.5">
                <span className={`text-[11px] leading-[18px] ${isSynthetic ? "font-medium text-[var(--color-wardian-text-muted)]" : "text-primary"} truncate`}>
                  {row.entry.message}
                </span>
                {!isSynthetic && badgeRefs.length > 0 && (
                  <span className="min-w-0 max-w-[112px] flex items-center gap-1 overflow-hidden">
                    {badgeRefs.map((ref) => (
                      <span key={ref} className={refClassName(ref, branch, upstream)} title={ref}>
                        {ref}
                      </span>
                    ))}
                  </span>
                )}
              </div>
              {isSynthetic ? (
                <span className="text-[9px] text-[var(--color-wardian-text-muted)] shrink-0 opacity-80">
                  {commitCountLabel(row.entry.syntheticCount ?? 0)}
                </span>
              ) : null}
            </button>
            {!isSynthetic && (
              <button
                type="button"
                aria-label={`View Changes for ${row.entry.message}`}
                title="View Changes"
                onClick={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                  viewHistoryItemChanges(row.entry);
                }}
                className="absolute right-1 top-1/2 hidden h-5 w-5 -translate-y-1/2 items-center justify-center rounded text-[var(--color-wardian-text-muted)] opacity-0 hover:bg-wardian-card-bg-muted hover:text-primary focus:opacity-100 focus:outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)] group-hover:inline-flex group-hover:opacity-100 group-focus-within:inline-flex group-focus-within:opacity-100"
              >
                <Eye className="h-3.5 w-3.5" aria-hidden="true" />
              </button>
            )}
            {showDetails && (
              <div
                id={historyGraphTooltipId(rowId)}
                role="tooltip"
                className="pointer-events-none absolute left-8 top-full z-30 mt-1 w-[320px] max-w-[calc(100vw-2rem)] rounded border border-[var(--color-wardian-border)] bg-[var(--color-wardian-bg)] px-3 py-2 text-[11px] text-[var(--color-wardian-text)] shadow-2xl ring-1 ring-white/10"
              >
                <div className="mb-1 truncate font-medium text-primary">{row.entry.message}</div>
                <dl className="grid grid-cols-[56px_minmax(0,1fr)] gap-x-2 gap-y-1">
                  <dt className="text-[var(--color-wardian-text-muted)]">Commit</dt>
                  <dd className="min-w-0 truncate font-mono">{row.entry.hash}</dd>
                  <dt className="text-[var(--color-wardian-text-muted)]">Author</dt>
                  <dd className="min-w-0 truncate">{row.entry.author}</dd>
                  <dt className="text-[var(--color-wardian-text-muted)]">Date</dt>
                  <dd className="min-w-0 truncate">{row.entry.date}</dd>
                  <dt className="text-[var(--color-wardian-text-muted)]">Refs</dt>
                  <dd className="min-w-0 truncate">{refsSummary}</dd>
                  <dt className="text-[var(--color-wardian-text-muted)]">Parents</dt>
                  <dd className="min-w-0 truncate font-mono">{parentSummary}</dd>
                </dl>
              </div>
            )}
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
                {!isLoading && !errorByHash[row.entry.hash] && changes.length > 0 && renderChangeViewControls(row, width)}
                {changeViewMode === "tree"
                  ? renderChangeTreeNodes(row, width, short, buildChangeTree(changes))
                  : changes.map((change) => renderChangeRow(row, width, short, change, change.path))}
              </div>
            )}
          </div>
        );
      })}
      {hasMoreHistory && rows.length > 0 && (
        <button
          type="button"
          aria-label="Load more history commits"
          onClick={onLoadMoreHistory}
          disabled={isLoadingMoreHistory}
          className="w-full flex items-center gap-2 px-1 hover:bg-wardian-card-bg-muted rounded min-w-0 text-left text-[11px] text-[var(--color-wardian-text-muted)] disabled:opacity-60"
          style={{ height: `${metrics.rowHeight}px` }}
        >
          <GraphPlaceholder
            testId="history-graph-load-more-placeholder"
            width={metrics.swimlaneWidth * (Math.max(rows[rows.length - 1]?.outputLanes.length ?? 0, 1) + 1)}
            lanes={rows[rows.length - 1]?.outputLanes ?? []}
            highlightIndex={rows[rows.length - 1]?.circleIndex ?? 0}
            metrics={metrics}
          />
          <span className="truncate">{isLoadingMoreHistory ? "Loading More..." : "Load More..."}</span>
        </button>
      )}
      {historyContextMenu && (
        <ContextMenu
          x={historyContextMenu.x}
          y={historyContextMenu.y}
          items={historyContextMenu.items}
          onClose={() => setHistoryContextMenu(null)}
        />
      )}
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

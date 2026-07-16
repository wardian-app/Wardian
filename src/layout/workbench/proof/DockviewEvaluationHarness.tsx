import React, {
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createRoot, type Root } from "react-dom/client";
import {
  DockviewReact,
  type DockviewApi,
  type DockviewReadyEvent,
  type DockviewWillDropEvent,
  type IDockviewPanelHeaderProps,
  type IDockviewPanelProps,
} from "dockview-react";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import type { AgentConfig, AgentTelemetry } from "../../../types";
import type {
  AgentInteractions,
  AgentTeam,
} from "../../watchlist/types";
import "dockview-react/dist/styles/dockview.css";
import "@xterm/xterm/css/xterm.css";
import "./dockviewEvaluation.css";

export type DockviewProofSurfaceKind =
  | "terminal"
  | "graph"
  | "garden"
  | "synthetic";

export interface DockviewProofSurface {
  surface_id: string;
  title: string;
  kind: DockviewProofSurfaceKind;
  terminal_mode?: "owner" | "mirror";
}

export interface DockviewProofGroup {
  group_id: string;
  surface_ids: string[];
  active_surface_id: string | null;
}

export interface DockviewProofModel {
  schema_version: 1;
  active_group_id: string;
  groups: DockviewProofGroup[];
  surfaces: Record<string, DockviewProofSurface>;
}

export interface DockviewProofMetrics {
  started_at: string;
  react_commit_count: number;
  react_commit_duration_ms: number[];
  surface_mounts: Record<string, number>;
  surface_unmounts: Record<string, number>;
  visibility_changes: Record<string, number>;
  terminal_hosts: Record<string, string>;
  terminal_write_chars: Record<string, number>;
  terminal_webgl_loaded: number;
  terminal_webgl_failures: number;
  model_command_duration_ms: number[];
  adapter_move_events: number;
}

export interface DockviewProofRuntime {
  metrics: DockviewProofMetrics;
  getModel: () => DockviewProofModel;
  commands: {
    activateSurface: (surfaceId: string) => void;
    moveSurface: (surfaceId: string, targetGroupId: string) => void;
    toggleGroupZoom: (groupId: string) => void;
    splitGroup: () => void;
    closeSplitGroup: () => void;
    emitTerminalBurst: (lineCount?: number) => Promise<void>;
  };
}

declare global {
  interface Window {
    __WARDIAN_WORKBENCH_PROOF__?: DockviewProofRuntime;
  }
}

type ProofPanelParams = {
  surface_id: string;
  title: string;
  kind: DockviewProofSurfaceKind;
  terminal_mode?: "owner" | "mirror";
} & Record<string, unknown>;

interface DockviewEvaluationHarnessProps {
  initialModel?: DockviewProofModel;
  onModelChange?: (model: DockviewProofModel) => void;
}

const GROUP_IDS = [
  "proof-group-1",
  "proof-group-2",
  "proof-group-3",
  "proof-group-4",
] as const;
const SPLIT_GROUP_ID = "proof-group-split";
const SPLIT_SURFACE_ID = "synthetic-14";

const PROOF_SURFACES: DockviewProofSurface[] = [
  { surface_id: "terminal-owner", title: "Terminal Owner", kind: "terminal", terminal_mode: "owner" },
  { surface_id: "graph", title: "Graph", kind: "graph" },
  { surface_id: "terminal-mirror-1", title: "Terminal Mirror 1", kind: "terminal", terminal_mode: "mirror" },
  { surface_id: "garden", title: "Garden", kind: "garden" },
  { surface_id: "terminal-mirror-2", title: "Terminal Mirror 2", kind: "terminal", terminal_mode: "mirror" },
  { surface_id: "terminal-mirror-3", title: "Terminal Mirror 3", kind: "terminal", terminal_mode: "mirror" },
  ...Array.from({ length: 14 }, (_, index) => ({
    surface_id: `synthetic-${String(index + 1).padStart(2, "0")}`,
    title: `Synthetic ${String(index + 1).padStart(2, "0")}`,
    kind: "synthetic" as const,
  })),
];

const INITIAL_GROUP_SURFACES: Record<(typeof GROUP_IDS)[number], string[]> = {
  "proof-group-1": ["terminal-owner", "graph", "synthetic-01", "synthetic-02", "synthetic-03"],
  "proof-group-2": ["terminal-mirror-1", "garden", "synthetic-04", "synthetic-05", "synthetic-06"],
  "proof-group-3": ["terminal-mirror-2", "synthetic-07", "synthetic-08", "synthetic-09", "synthetic-10"],
  "proof-group-4": ["terminal-mirror-3", "synthetic-11", "synthetic-12", "synthetic-13", "synthetic-14"],
};

const PROOF_AGENTS: AgentConfig[] = [
  {
    session_id: "proof-agent-alpha",
    session_name: "Proof Alpha",
    agent_class: "Coder",
    folder: "<proof-workspace>/alpha",
    is_off: false,
    provider: "mock",
  },
  {
    session_id: "proof-agent-beta",
    session_name: "Proof Beta",
    agent_class: "Reviewer",
    folder: "<proof-workspace>/beta",
    is_off: false,
    provider: "mock",
  },
];

const PROOF_TELEMETRY: Record<string, AgentTelemetry> = Object.fromEntries(
  PROOF_AGENTS.map((agent, index) => [
    agent.session_id,
    {
      session_id: agent.session_id,
      cpu_usage: 3 + index,
      memory_mb: 128 + index * 32,
      uptime_seconds: 120 + index,
      query_count: 4 + index,
      init_timestamp: null,
      current_status: index === 0 ? "Processing..." : "Idle",
      log_path: null,
    },
  ]),
);

const PROOF_TEAMS: AgentTeam[] = [
  {
    id: "proof-team",
    name: "Proof Team",
    agentIds: PROOF_AGENTS.map((agent) => agent.session_id),
  },
];
const PROOF_INTERACTIONS: AgentInteractions = {};

const ProofGraphView = React.lazy(async () => {
  const module = await import("../../../views/GraphView");
  return { default: module.GraphView };
});
const ProofGardenView = React.lazy(async () => {
  const module = await import("../../../views/GardenView");
  return { default: module.GardenView };
});

let currentMetrics: DockviewProofMetrics | null = null;
let pendingReactCommitStartedAt: number | null = null;
const terminalInstances = new Map<string, Terminal>();

export function createDockviewProofModel(): DockviewProofModel {
  const surfaces = Object.fromEntries(
    PROOF_SURFACES.map((surface) => [surface.surface_id, { ...surface }]),
  );
  return {
    schema_version: 1,
    active_group_id: GROUP_IDS[0],
    groups: GROUP_IDS.map((groupId) => ({
      group_id: groupId,
      surface_ids: [...INITIAL_GROUP_SURFACES[groupId]],
      active_surface_id: INITIAL_GROUP_SURFACES[groupId][0],
    })),
    surfaces,
  };
}

export function serializeDockviewProofModel(model: DockviewProofModel): string {
  return JSON.stringify(model);
}

export function moveProofSurface(
  model: DockviewProofModel,
  surfaceId: string,
  targetGroupId: string,
  targetIndex?: number,
): DockviewProofModel {
  if (!model.surfaces[surfaceId]) {
    throw new Error(`Unknown proof surface: ${surfaceId}`);
  }
  if (!model.groups.some((group) => group.group_id === targetGroupId)) {
    throw new Error(`Unknown proof group: ${targetGroupId}`);
  }

  const groups = model.groups.map((group) => {
    const withoutSurface = group.surface_ids.filter((id) => id !== surfaceId);
    const nextActive = group.active_surface_id === surfaceId
      ? (withoutSurface[0] ?? null)
      : group.active_surface_id;
    return {
      ...group,
      surface_ids: withoutSurface,
      active_surface_id: nextActive,
    };
  });
  const target = groups.find((group) => group.group_id === targetGroupId);
  if (!target) throw new Error(`Unknown proof group: ${targetGroupId}`);
  const insertionIndex = Math.max(
    0,
    Math.min(targetIndex ?? target.surface_ids.length, target.surface_ids.length),
  );
  target.surface_ids.splice(insertionIndex, 0, surfaceId);
  target.active_surface_id = surfaceId;

  return {
    ...model,
    active_group_id: targetGroupId,
    groups,
  };
}

function cloneProofModel(model: DockviewProofModel): DockviewProofModel {
  return {
    ...model,
    groups: model.groups.map((group) => ({ ...group, surface_ids: [...group.surface_ids] })),
    surfaces: Object.fromEntries(
      Object.entries(model.surfaces).map(([id, surface]) => [id, { ...surface }]),
    ),
  };
}

function createProofMetrics(): DockviewProofMetrics {
  return {
    started_at: new Date().toISOString(),
    react_commit_count: 0,
    react_commit_duration_ms: [],
    surface_mounts: {},
    surface_unmounts: {},
    visibility_changes: {},
    terminal_hosts: {},
    terminal_write_chars: {},
    terminal_webgl_loaded: 0,
    terminal_webgl_failures: 0,
    model_command_duration_ms: [],
    adapter_move_events: 0,
  };
}

function proofMetrics(): DockviewProofMetrics {
  if (!currentMetrics) currentMetrics = createProofMetrics();
  return currentMetrics;
}

function recordModelCommand(startedAt: number): void {
  proofMetrics().model_command_duration_ms.push(performance.now() - startedAt);
}

function projectModelIntoDockview(api: DockviewApi, model: DockviewProofModel): void {
  const groups = new Map<string, ReturnType<DockviewApi["addGroup"]>>();
  model.groups.forEach((group, index) => {
    let dockviewGroup;
    if (index === 0) {
      dockviewGroup = api.addGroup({ id: group.group_id, direction: "right" });
    } else if (index === 1) {
      dockviewGroup = api.addGroup({
        id: group.group_id,
        referenceGroup: groups.get(model.groups[0].group_id),
        direction: "right",
      });
    } else if (index === 2) {
      dockviewGroup = api.addGroup({
        id: group.group_id,
        referenceGroup: groups.get(model.groups[0].group_id),
        direction: "below",
      });
    } else {
      dockviewGroup = api.addGroup({
        id: group.group_id,
        referenceGroup: groups.get(model.groups[index - 2].group_id),
        direction: "below",
      });
    }
    groups.set(group.group_id, dockviewGroup);
  });

  for (const group of model.groups) {
    const referenceGroup = groups.get(group.group_id);
    if (!referenceGroup) throw new Error(`Dockview group projection failed: ${group.group_id}`);
    for (const surfaceId of group.surface_ids) {
      const surface = model.surfaces[surfaceId];
      if (!surface) throw new Error(`Missing proof surface: ${surfaceId}`);
      api.addPanel<ProofPanelParams>({
        id: surface.surface_id,
        title: surface.title,
        component: "proof-surface",
        renderer: "always",
        inactive: true,
        position: { referenceGroup },
        params: {
          surface_id: surface.surface_id,
          title: surface.title,
          kind: surface.kind,
          terminal_mode: surface.terminal_mode,
        },
      });
    }
  }

  for (const group of model.groups) {
    if (group.active_surface_id) api.getPanel(group.active_surface_id)?.api.setActive();
  }
  const activeGroup = model.groups.find((group) => group.group_id === model.active_group_id);
  if (activeGroup?.active_surface_id) api.getPanel(activeGroup.active_surface_id)?.api.setActive();
}

function ProofTab({ api, params }: IDockviewPanelHeaderProps<ProofPanelParams>) {
  return (
    <span
      className="workbench-proof-tab"
      data-surface-id={params.surface_id}
      data-group-id={api.group.id}
    >
      {params.title}
    </span>
  );
}

function ProofSurfacePanel({ api, params }: IDockviewPanelProps<ProofPanelParams>) {
  const [isVisible, setIsVisible] = useState(api.isVisible);
  const mountCount = useRef(0);
  if (mountCount.current === 0) {
    const metrics = proofMetrics();
    metrics.surface_mounts[params.surface_id] = (metrics.surface_mounts[params.surface_id] ?? 0) + 1;
    mountCount.current = metrics.surface_mounts[params.surface_id];
  }

  useEffect(() => {
    const visibilityDisposable = api.onDidVisibilityChange((event) => {
      setIsVisible(event.isVisible);
      const metrics = proofMetrics();
      metrics.visibility_changes[params.surface_id] = (metrics.visibility_changes[params.surface_id] ?? 0) + 1;
    });
    return () => {
      visibilityDisposable.dispose();
      const metrics = proofMetrics();
      metrics.surface_unmounts[params.surface_id] = (metrics.surface_unmounts[params.surface_id] ?? 0) + 1;
    };
  }, [api, params.surface_id]);

  return (
    <section
      id={`proof-panel-${params.surface_id}`}
      data-testid={`proof-surface-${params.surface_id}`}
      data-surface-id={params.surface_id}
      data-surface-kind={params.kind}
      data-visible={String(isVisible)}
      data-mount-count={mountCount.current}
      className="workbench-proof-surface"
    >
      {params.kind === "terminal" && (
        <ProofTerminal
          surfaceId={params.surface_id}
          mode={params.terminal_mode ?? "mirror"}
        />
      )}
      {params.kind === "graph" && <ProofGraph />}
      {params.kind === "garden" && <ProofGarden />}
      {params.kind === "synthetic" && (
        <div className="workbench-proof-synthetic">
          <span>{params.title}</span>
          <small>Keyed synthetic renderer</small>
        </div>
      )}
    </section>
  );
}

function ProofTerminal({ surfaceId, mode }: { surfaceId: string; mode: "owner" | "mirror" }) {
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const terminalBackground = getComputedStyle(host)
      .getPropertyValue("--color-wardian-bg")
      .trim() || "transparent";
    const terminal = new Terminal({
      allowTransparency: terminalBackground === "transparent",
      allowProposedApi: false,
      convertEol: true,
      cursorBlink: mode === "owner",
      disableStdin: mode === "mirror",
      fontSize: 12,
      rows: 12,
      cols: 64,
      theme: {
        background: terminalBackground,
      },
    });
    terminal.open(host);
    terminal.write(`${mode === "owner" ? "owner" : "mirror"}:${surfaceId}\r\n`);
    terminalInstances.set(surfaceId, terminal);
    proofMetrics().terminal_hosts[surfaceId] = host.dataset.terminalHostId ?? surfaceId;

    let webglAddon: WebglAddon | null = null;
    try {
      webglAddon = new WebglAddon();
      terminal.loadAddon(webglAddon);
      proofMetrics().terminal_webgl_loaded += 1;
    } catch {
      webglAddon?.dispose();
      webglAddon = null;
      proofMetrics().terminal_webgl_failures += 1;
    }

    return () => {
      terminalInstances.delete(surfaceId);
      webglAddon?.dispose();
      terminal.dispose();
    };
  }, [mode, surfaceId]);

  return (
    <div className="workbench-proof-terminal-shell" data-terminal-mode={mode}>
      <div className="workbench-proof-terminal-badge">{mode}</div>
      <div
        ref={hostRef}
        className="workbench-proof-terminal-host"
        data-testid={`proof-terminal-host-${surfaceId}`}
        data-terminal-host-id={`host-${surfaceId}`}
        aria-label={`${mode} xterm host ${surfaceId}`}
      />
    </div>
  );
}

function ProofGraph() {
  if (typeof navigator !== "undefined" && /jsdom/i.test(navigator.userAgent)) {
    return <div data-testid="proof-graph-jsdom-placeholder">Graph renderer runs in browser proof</div>;
  }
  return (
    <div className="workbench-proof-heavy" data-testid="proof-graph-wrapper">
      <Suspense fallback={<div>Loading Graph renderer</div>}>
        <ProofGraphView
          filteredAgents={PROOF_AGENTS}
          allAgents={PROOF_AGENTS}
          telemetry={PROOF_TELEMETRY}
          terminalTitles={{}}
          currentThoughts={{}}
          selectedAgentIds={new Set()}
          offAgentIds={new Set()}
          watchlists={[]}
          activeList={null}
          teams={PROOF_TEAMS}
          interactions={PROOF_INTERACTIONS}
          onSelectionChange={() => {}}
          onOpenAgentInGrid={() => {}}
          onInitiateRename={() => {}}
          onQuery={() => {}}
          onPause={() => {}}
          onRestart={() => {}}
          onClear={() => {}}
          onClone={() => {}}
          onAddToList={() => {}}
          onRemoveFromList={() => {}}
          onAddAgentsToList={() => {}}
          onRemoveAgentsFromList={() => {}}
          onDelete={() => {}}
          onDeleteAgents={() => {}}
          deriveCurrentThought={() => ({ thought: "Proof renderer", status: "Idle" })}
        />
      </Suspense>
    </div>
  );
}

function ProofGarden() {
  if (typeof navigator !== "undefined" && /jsdom/i.test(navigator.userAgent)) {
    return <div data-testid="proof-garden-jsdom-placeholder">Garden renderer runs in browser proof</div>;
  }
  return (
    <div className="workbench-proof-heavy" data-testid="proof-garden-wrapper">
      <Suspense fallback={<div>Loading Garden renderer</div>}>
        <ProofGardenView
          filteredAgents={PROOF_AGENTS}
          telemetry={PROOF_TELEMETRY}
          teams={PROOF_TEAMS}
          activeList={null}
          interactions={PROOF_INTERACTIONS}
          selectedAgentIds={new Set()}
          offAgentIds={new Set()}
          onSelectionChange={() => {}}
          onOpenAgentInGrid={() => {}}
        />
      </Suspense>
    </div>
  );
}

const PROOF_COMPONENTS = { "proof-surface": ProofSurfacePanel };

export function DockviewEvaluationHarness({
  initialModel,
  onModelChange,
}: DockviewEvaluationHarnessProps = {}) {
  const initialModelRef = useRef(cloneProofModel(initialModel ?? createDockviewProofModel()));
  const [model, setModel] = useState(initialModelRef.current);
  const [ready, setReady] = useState(false);
  const [zoomedGroupId, setZoomedGroupId] = useState<string | null>(null);
  const modelRef = useRef(model);
  const apiRef = useRef<DockviewApi | null>(null);
  const expectedMovesRef = useRef(new Set<string>());
  const eventDisposablesRef = useRef<Array<{ dispose: () => void }>>([]);
  const runtimeMetricsRef = useRef<DockviewProofMetrics | null>(null);
  if (!runtimeMetricsRef.current) {
    runtimeMetricsRef.current = createProofMetrics();
    currentMetrics = runtimeMetricsRef.current;
    pendingReactCommitStartedAt = null;
    terminalInstances.clear();
  }

  const publishModel = useCallback((nextModel: DockviewProofModel) => {
    modelRef.current = nextModel;
    pendingReactCommitStartedAt = performance.now();
    setModel(nextModel);
    onModelChange?.(cloneProofModel(nextModel));
  }, [onModelChange]);

  useLayoutEffect(() => {
    if (pendingReactCommitStartedAt === null) return;
    const metrics = proofMetrics();
    metrics.react_commit_count += 1;
    metrics.react_commit_duration_ms.push(performance.now() - pendingReactCommitStartedAt);
    pendingReactCommitStartedAt = null;
  }, [model]);

  const activateSurface = useCallback((surfaceId: string) => {
    const startedAt = performance.now();
    const api = apiRef.current;
    const panel = api?.getPanel(surfaceId);
    if (!panel) throw new Error(`Dockview panel is unavailable: ${surfaceId}`);
    const groupId = panel.group.id;
    const groups = modelRef.current.groups.map((group) => ({
      ...group,
      active_surface_id: group.group_id === groupId ? surfaceId : group.active_surface_id,
    }));
    publishModel({ ...modelRef.current, groups, active_group_id: groupId });
    panel.api.setActive();
    recordModelCommand(startedAt);
  }, [publishModel]);

  const moveSurface = useCallback((surfaceId: string, targetGroupId: string) => {
    const startedAt = performance.now();
    const api = apiRef.current;
    const panel = api?.getPanel(surfaceId);
    const targetGroup = api?.groups.find((group) => group.id === targetGroupId);
    if (!panel || !targetGroup) {
      throw new Error(`Dockview move target is unavailable: ${surfaceId} -> ${targetGroupId}`);
    }
    const nextModel = moveProofSurface(modelRef.current, surfaceId, targetGroupId);
    publishModel(nextModel);
    expectedMovesRef.current.add(surfaceId);
    panel.api.moveTo({ group: targetGroup });
    recordModelCommand(startedAt);
  }, [publishModel]);

  const toggleGroupZoom = useCallback((groupId: string) => {
    const startedAt = performance.now();
    const group = apiRef.current?.getGroup(groupId);
    if (!group) throw new Error(`Dockview zoom group is unavailable: ${groupId}`);
    if (group.api.isMaximized()) {
      group.api.exitMaximized();
      setZoomedGroupId(null);
    } else {
      group.api.maximize();
      setZoomedGroupId(groupId);
    }
    recordModelCommand(startedAt);
  }, []);

  const splitGroup = useCallback(() => {
    if (modelRef.current.groups.some((group) => group.group_id === SPLIT_GROUP_ID)) return;
    const startedAt = performance.now();
    const api = apiRef.current;
    const referenceGroup = api?.groups.find((group) => group.id === GROUP_IDS[3]);
    const panel = api?.getPanel(SPLIT_SURFACE_ID);
    if (!api || !referenceGroup || !panel) throw new Error("Dockview split prerequisites are unavailable");
    const withoutSurface = moveProofSurface(modelRef.current, SPLIT_SURFACE_ID, GROUP_IDS[3]);
    const source = withoutSurface.groups.find((group) => group.group_id === GROUP_IDS[3]);
    if (!source) throw new Error("Proof split source group is unavailable");
    const sourceSurfaceIds = source.surface_ids.filter((surfaceId) => surfaceId !== SPLIT_SURFACE_ID);
    const nextModel: DockviewProofModel = {
      ...withoutSurface,
      active_group_id: SPLIT_GROUP_ID,
      groups: [
        ...withoutSurface.groups.map((group) => group.group_id === GROUP_IDS[3]
          ? { ...group, surface_ids: sourceSurfaceIds, active_surface_id: sourceSurfaceIds[0] ?? null }
          : group),
        { group_id: SPLIT_GROUP_ID, surface_ids: [SPLIT_SURFACE_ID], active_surface_id: SPLIT_SURFACE_ID },
      ],
    };
    const split = api.addGroup({
      id: SPLIT_GROUP_ID,
      referenceGroup,
      direction: "right",
    });
    publishModel(nextModel);
    expectedMovesRef.current.add(SPLIT_SURFACE_ID);
    panel.api.moveTo({ group: split });
    recordModelCommand(startedAt);
  }, [publishModel]);

  const closeSplitGroup = useCallback(() => {
    const splitModel = modelRef.current.groups.find((group) => group.group_id === SPLIT_GROUP_ID);
    if (!splitModel) return;
    const startedAt = performance.now();
    const api = apiRef.current;
    const split = api?.groups.find((group) => group.id === SPLIT_GROUP_ID);
    const fallback = api?.groups.find((group) => group.id === GROUP_IDS[3]);
    if (!api || !split || !fallback) throw new Error("Dockview close-group prerequisites are unavailable");
    let nextModel = modelRef.current;
    for (const surfaceId of splitModel.surface_ids) {
      nextModel = moveProofSurface(nextModel, surfaceId, GROUP_IDS[3]);
      const panel = api.getPanel(surfaceId);
      if (panel) {
        expectedMovesRef.current.add(surfaceId);
        panel.api.moveTo({ group: fallback });
      }
    }
    nextModel = {
      ...nextModel,
      active_group_id: GROUP_IDS[3],
      groups: nextModel.groups.filter((group) => group.group_id !== SPLIT_GROUP_ID),
    };
    publishModel(nextModel);
    api.removeGroup(split);
    recordModelCommand(startedAt);
  }, [publishModel]);

  const emitTerminalBurst = useCallback(async (lineCount = 200) => {
    const payload = Array.from(
      { length: lineCount },
      (_, index) => `proof-output-${String(index).padStart(4, "0")}\r\n`,
    ).join("");
    await Promise.all([...terminalInstances.entries()].map(([surfaceId, terminal]) => new Promise<void>((resolve) => {
      terminal.write(payload, () => {
        const metrics = proofMetrics();
        metrics.terminal_write_chars[surfaceId] = (metrics.terminal_write_chars[surfaceId] ?? 0) + payload.length;
        resolve();
      });
    })));
  }, []);

  const handleReady = useCallback((event: DockviewReadyEvent) => {
    apiRef.current = event.api;
    projectModelIntoDockview(event.api, modelRef.current);
    eventDisposablesRef.current.push(
      event.api.onDidMovePanel((moveEvent) => {
        proofMetrics().adapter_move_events += 1;
        if (expectedMovesRef.current.delete(moveEvent.panel.id)) return;
        const targetIndex = moveEvent.panel.group.panels.findIndex((panel) => panel.id === moveEvent.panel.id);
        publishModel(moveProofSurface(
          modelRef.current,
          moveEvent.panel.id,
          moveEvent.panel.group.id,
          targetIndex,
        ));
      }),
      event.api.onDidActivePanelChange(({ panel }) => {
        if (!panel) return;
        const groupId = panel.group.id;
        const currentGroup = modelRef.current.groups.find((group) => group.group_id === groupId);
        if (currentGroup?.active_surface_id === panel.id && modelRef.current.active_group_id === groupId) return;
        publishModel({
          ...modelRef.current,
          active_group_id: groupId,
          groups: modelRef.current.groups.map((group) => group.group_id === groupId
            ? { ...group, active_surface_id: panel.id }
            : group),
        });
      }),
    );
    setReady(true);
  }, [publishModel]);

  const handleWillDrop = useCallback((event: DockviewWillDropEvent) => {
    const transfer = event.getData();
    const panelId = transfer?.panelId ?? event.panel?.id;
    if (!panelId || !event.group) return;
    event.preventDefault();
    moveSurface(panelId, event.group.id);
  }, [moveSurface]);

  const focusNextGroupTab = useCallback((originGroupId?: string) => {
    const currentModel = modelRef.current;
    const currentIndex = currentModel.groups.findIndex(
      (group) => group.group_id === (originGroupId ?? currentModel.active_group_id),
    );
    const nextGroup = currentModel.groups[(currentIndex + 1) % currentModel.groups.length];
    const surfaceId = nextGroup?.active_surface_id ?? nextGroup?.surface_ids[0];
    if (!nextGroup || !surfaceId) throw new Error("Proof group traversal target is unavailable");
    window.requestAnimationFrame(() => {
      activateSurface(surfaceId);
      const descriptor = [...document.querySelectorAll<HTMLElement>(".workbench-proof-tab")].find(
        (tab) => tab.dataset.groupId === nextGroup.group_id && tab.dataset.surfaceId === surfaceId,
      );
      const tab = descriptor?.closest<HTMLElement>('[role="tab"]');
      if (!tab) throw new Error(`Proof group traversal tab is unavailable: ${nextGroup.group_id}/${surfaceId}`);
      tab.focus();
    });
  }, [activateSurface]);

  useEffect(() => {
    const handleGroupNavigation = (event: KeyboardEvent) => {
      if (event.key !== "F6" || !(event.target instanceof Element)) return;
      const proofRoot = document.querySelector('[data-testid="workbench-proof"]');
      if (!proofRoot?.contains(event.target)) return;
      const descriptor = event.target
        .closest('[role="tab"]')
        ?.querySelector<HTMLElement>("[data-group-id][data-surface-id]");
      event.preventDefault();
      event.stopImmediatePropagation();
      focusNextGroupTab(descriptor?.dataset.groupId);
    };
    window.addEventListener("keydown", handleGroupNavigation, true);
    return () => window.removeEventListener("keydown", handleGroupNavigation, true);
  }, [focusNextGroupTab]);

  useEffect(() => () => {
    eventDisposablesRef.current.forEach((disposable) => disposable.dispose());
    eventDisposablesRef.current = [];
  }, []);

  useEffect(() => {
    if (!ready || !runtimeMetricsRef.current) return;
    window.__WARDIAN_WORKBENCH_PROOF__ = {
      metrics: runtimeMetricsRef.current,
      getModel: () => cloneProofModel(modelRef.current),
      commands: {
        activateSurface,
        moveSurface,
        toggleGroupZoom,
        splitGroup,
        closeSplitGroup,
        emitTerminalBurst,
      },
    };
    return () => {
      delete window.__WARDIAN_WORKBENCH_PROOF__;
    };
  }, [
    activateSurface,
    closeSplitGroup,
    emitTerminalBurst,
    moveSurface,
    ready,
    splitGroup,
    toggleGroupZoom,
  ]);

  const modelJson = useMemo(() => serializeDockviewProofModel(model), [model]);

  return (
    <main
      data-testid="workbench-proof"
      data-layout-source="wardian-model"
      data-ready={String(ready)}
      data-zoomed-group-id={zoomedGroupId ?? "none"}
      className="workbench-proof dockview-theme-wardian"
      onKeyDown={(event) => {
        if (!event.altKey || !event.shiftKey) return;
        if (event.key === "ArrowRight") {
          event.preventDefault();
          moveSurface("terminal-owner", GROUP_IDS[1]);
        } else if (event.key.toLowerCase() === "z") {
          event.preventDefault();
          toggleGroupZoom(GROUP_IDS[0]);
        } else if (event.key.toLowerCase() === "s") {
          event.preventDefault();
          splitGroup();
        } else if (event.key.toLowerCase() === "c") {
          event.preventDefault();
          closeSplitGroup();
        }
      }}
    >
      <header className="workbench-proof-toolbar">
        <div>
          <strong>Dockview adapter proof</strong>
          <span>{model.groups.length} groups / {Object.keys(model.surfaces).length} tabs</span>
        </div>
        <nav aria-label="Workbench proof commands">
          <button type="button" onClick={() => moveSurface("terminal-owner", GROUP_IDS[1])}>
            Move terminal owner to group 2
          </button>
          <button type="button" onClick={() => toggleGroupZoom(GROUP_IDS[0])}>
            Toggle group 1 zoom
          </button>
          <button type="button" onClick={splitGroup}>Split group 4 right</button>
          <button type="button" onClick={closeSplitGroup}>Close split group</button>
        </nav>
      </header>
      <section className="workbench-proof-layout" aria-label="Dockview evaluation layout">
        <DockviewReact
          components={PROOF_COMPONENTS}
          defaultTabComponent={ProofTab}
          dndStrategy="pointer"
          keyboardNavigation
          onReady={handleReady}
          onWillDrop={handleWillDrop}
        />
      </section>
      <ul className="workbench-proof-model-summary" aria-hidden="true">
        {model.groups.map((group) => (
          <li
            key={group.group_id}
            data-testid={group.group_id}
            data-group-id={group.group_id}
            data-surface-count={group.surface_ids.length}
          >
            {group.group_id}: {group.surface_ids.join(",")}
          </li>
        ))}
      </ul>
      <output data-testid="proof-model" className="workbench-proof-model-json" aria-hidden="true">
        {modelJson}
      </output>
    </main>
  );
}

export function mountDockviewEvaluationHarness(target: HTMLElement): { unmount: () => void } {
  const root: Root = createRoot(target);
  root.render(<DockviewEvaluationHarness />);
  return { unmount: () => root.unmount() };
}

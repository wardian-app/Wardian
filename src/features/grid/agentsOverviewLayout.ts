export type AgentsOverviewMode = "auto" | "grid" | "single";
export type AgentsOverviewPresentationMode = "empty" | "grid" | "single";
export type AgentsOverviewCardMode = "terminal" | "chat";
export type AgentsOverviewOrientation = "portrait" | "balanced" | "landscape";

export interface AgentsOverviewLayoutAgent {
  id: string;
  cardMode: AgentsOverviewCardMode;
}

export interface AgentsOverviewContainerSize {
  width: number;
  height: number;
}

export interface AgentsOverviewCardFloor {
  width: number;
  height: number;
}

export interface AgentsOverviewGridCandidate {
  columns: number;
  rows: number;
  cardWidth: number;
  cardHeight: number;
  minimumCardArea: number;
  emptyCells: number;
  orientation: AgentsOverviewOrientation;
  meetsHardFloor: boolean;
  score: number;
}

export interface GenerateAgentsOverviewCandidatesInput {
  agents: readonly AgentsOverviewLayoutAgent[];
  containerSize: AgentsOverviewContainerSize;
  gap?: number;
}

export interface ResolveAgentsOverviewLayoutInput extends GenerateAgentsOverviewCandidatesInput {
  mode: AgentsOverviewMode;
  focusedAgentId?: string | null;
  /** Agent IDs ordered from most recently to least recently interacted. */
  recentAgentIds?: readonly string[];
  previousLayout?: AgentsOverviewLayoutResult | null;
}

export interface AgentsOverviewLayoutResult {
  requestedMode: AgentsOverviewMode;
  presentationMode: AgentsOverviewPresentationMode;
  orderedAgentIds: string[];
  focusedAgentId: string | null;
  visibleAgentIds: string[];
  columns: number;
  rows: number;
  cardWidth: number;
  cardHeight: number;
  contentWidth: number;
  contentHeight: number;
  requiresScroll: boolean;
  candidate: AgentsOverviewGridCandidate | null;
}

/** Minimum total card bounds, including header chrome, for a terminal renderer. */
export const TERMINAL_CARD_FLOOR: Readonly<AgentsOverviewCardFloor> = Object.freeze({
  width: 520,
  height: 280,
});

/** Minimum total card bounds, including header chrome, for a chat renderer. */
export const CHAT_CARD_FLOOR: Readonly<AgentsOverviewCardFloor> = Object.freeze({
  width: 360,
  height: 280,
});

export const AGENTS_OVERVIEW_CARD_CHROME_HEIGHT = 52;
export const AGENTS_OVERVIEW_RESIZE_DEBOUNCE_MS = 120;
export const AGENTS_OVERVIEW_SCORE_IMPROVEMENT_THRESHOLD = 0.1;
export const DEFAULT_AGENTS_OVERVIEW_GAP = 8;

const FLOAT_COMPARISON_EPSILON = 0.0001;

function finiteNonNegative(value: number): number {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}

function floorForMode(mode: AgentsOverviewCardMode): Readonly<AgentsOverviewCardFloor> {
  return mode === "terminal" ? TERMINAL_CARD_FLOOR : CHAT_CARD_FLOOR;
}

function stableAgents(agents: readonly AgentsOverviewLayoutAgent[]): AgentsOverviewLayoutAgent[] {
  const seen = new Set<string>();

  return agents.filter((agent) => {
    if (!agent.id || seen.has(agent.id)) return false;
    seen.add(agent.id);
    return true;
  });
}

function orientationFor(columns: number, rows: number): AgentsOverviewOrientation {
  if (columns > rows) return "landscape";
  if (columns < rows) return "portrait";
  return "balanced";
}

function requiredFloor(agents: readonly AgentsOverviewLayoutAgent[]): AgentsOverviewCardFloor {
  return agents.reduce<AgentsOverviewCardFloor>(
    (floor, agent) => {
      const agentFloor = floorForMode(agent.cardMode);
      return {
        width: Math.max(floor.width, agentFloor.width),
        height: Math.max(floor.height, agentFloor.height),
      };
    },
    { width: 0, height: 0 },
  );
}

function evaluateCandidate(
  agents: readonly AgentsOverviewLayoutAgent[],
  containerSize: AgentsOverviewContainerSize,
  columns: number,
  gap: number,
): AgentsOverviewGridCandidate {
  const safeColumns = Math.max(1, Math.min(agents.length, Math.floor(columns)));
  const rows = Math.ceil(agents.length / safeColumns);
  const width = finiteNonNegative(containerSize.width);
  const height = finiteNonNegative(containerSize.height);
  const safeGap = finiteNonNegative(gap);
  const cardWidth = Math.max(0, (width - (Math.max(0, safeColumns - 1) * safeGap)) / safeColumns);
  const cardHeight = Math.max(0, (height - (Math.max(0, rows - 1) * safeGap)) / rows);
  const meetsHardFloor = agents.every((agent) => {
    const floor = floorForMode(agent.cardMode);
    return cardWidth >= floor.width && cardHeight >= floor.height;
  });
  const minimumCardArea = cardWidth * cardHeight;

  return {
    columns: safeColumns,
    rows,
    cardWidth,
    cardHeight,
    minimumCardArea,
    emptyCells: (safeColumns * rows) - agents.length,
    orientation: orientationFor(safeColumns, rows),
    meetsHardFloor,
    // Candidate score is deliberately a continuous usable-area measure. Hard-floor,
    // empty-cell, and orientation policy remain explicit comparator dimensions.
    score: minimumCardArea,
  };
}

/** Generate every useful row/column shape in deterministic column order. */
export function generateAgentsOverviewCandidates(
  input: GenerateAgentsOverviewCandidatesInput,
): AgentsOverviewGridCandidate[] {
  const agents = stableAgents(input.agents);
  if (agents.length === 0) return [];
  const gap = input.gap ?? DEFAULT_AGENTS_OVERVIEW_GAP;

  return Array.from({ length: agents.length }, (_, index) =>
    evaluateCandidate(agents, input.containerSize, index + 1, gap));
}

function compareNumbersDescending(left: number, right: number): number {
  if (Math.abs(left - right) <= FLOAT_COMPARISON_EPSILON) return 0;
  return left > right ? -1 : 1;
}

/**
 * Compare candidates by the spec's strict ordering: hard floor, usable area,
 * empty cells, then the previous orientation. Column order is the stable final tie.
 */
export function compareAgentsOverviewCandidates(
  left: AgentsOverviewGridCandidate,
  right: AgentsOverviewGridCandidate,
  previousOrientation?: AgentsOverviewOrientation | null,
): number {
  if (left.meetsHardFloor !== right.meetsHardFloor) return left.meetsHardFloor ? -1 : 1;

  const areaComparison = compareNumbersDescending(left.minimumCardArea, right.minimumCardArea);
  if (areaComparison !== 0) return areaComparison;
  if (left.emptyCells !== right.emptyCells) return left.emptyCells - right.emptyCells;

  if (previousOrientation) {
    const leftMatches = left.orientation === previousOrientation;
    const rightMatches = right.orientation === previousOrientation;
    if (leftMatches !== rightMatches) return leftMatches ? -1 : 1;
  }

  return left.columns - right.columns;
}

export function selectBestAgentsOverviewCandidate(
  candidates: readonly AgentsOverviewGridCandidate[],
  previousOrientation?: AgentsOverviewOrientation | null,
): AgentsOverviewGridCandidate | null {
  if (candidates.length === 0) return null;
  return [...candidates].sort((left, right) =>
    compareAgentsOverviewCandidates(left, right, previousOrientation))[0] ?? null;
}

function resolveFocusedAgentId(
  orderedAgentIds: readonly string[],
  focusedAgentId: string | null | undefined,
  recentAgentIds: readonly string[] | undefined,
): string | null {
  const available = new Set(orderedAgentIds);
  if (focusedAgentId && available.has(focusedAgentId)) return focusedAgentId;

  const recent = recentAgentIds?.find((agentId) => available.has(agentId));
  return recent ?? orderedAgentIds[0] ?? null;
}

function emptyResult(mode: AgentsOverviewMode): AgentsOverviewLayoutResult {
  return {
    requestedMode: mode,
    presentationMode: "empty",
    orderedAgentIds: [],
    focusedAgentId: null,
    visibleAgentIds: [],
    columns: 0,
    rows: 0,
    cardWidth: 0,
    cardHeight: 0,
    contentWidth: 0,
    contentHeight: 0,
    requiresScroll: false,
    candidate: null,
  };
}

function singleResult(
  mode: AgentsOverviewMode,
  orderedAgentIds: string[],
  focusedAgentId: string,
  containerSize: AgentsOverviewContainerSize,
): AgentsOverviewLayoutResult {
  const cardWidth = finiteNonNegative(containerSize.width);
  const cardHeight = finiteNonNegative(containerSize.height);

  return {
    requestedMode: mode,
    presentationMode: "single",
    orderedAgentIds,
    focusedAgentId,
    visibleAgentIds: [focusedAgentId],
    columns: 1,
    rows: 1,
    cardWidth,
    cardHeight,
    contentWidth: cardWidth,
    contentHeight: cardHeight,
    requiresScroll: false,
    candidate: null,
  };
}

function gridResult(
  mode: AgentsOverviewMode,
  agents: readonly AgentsOverviewLayoutAgent[],
  focusedAgentId: string,
  candidate: AgentsOverviewGridCandidate,
  gap: number,
): AgentsOverviewLayoutResult {
  const strictFloor = requiredFloor(agents);
  const explicitGrid = mode === "grid";
  const cardWidth = explicitGrid ? Math.max(candidate.cardWidth, strictFloor.width) : candidate.cardWidth;
  const cardHeight = explicitGrid ? Math.max(candidate.cardHeight, strictFloor.height) : candidate.cardHeight;
  const contentWidth = (candidate.columns * cardWidth) + (Math.max(0, candidate.columns - 1) * gap);
  const contentHeight = (candidate.rows * cardHeight) + (Math.max(0, candidate.rows - 1) * gap);

  return {
    requestedMode: mode,
    presentationMode: "grid",
    orderedAgentIds: agents.map(({ id }) => id),
    focusedAgentId,
    visibleAgentIds: agents.map(({ id }) => id),
    columns: candidate.columns,
    rows: candidate.rows,
    cardWidth,
    cardHeight,
    contentWidth,
    contentHeight,
    requiresScroll: explicitGrid && !candidate.meetsHardFloor,
    candidate,
  };
}

function chooseAutoCandidate(
  agents: readonly AgentsOverviewLayoutAgent[],
  containerSize: AgentsOverviewContainerSize,
  gap: number,
  previousLayout: AgentsOverviewLayoutResult | null | undefined,
): AgentsOverviewGridCandidate | null {
  const previousOrientation = previousLayout?.candidate?.orientation;
  // Auto has only two useful presentations for multiple agents: a broad grid
  // or the focused singleton. A one-column multi-agent grid recreates the
  // cramped stacked-card failure that Single is meant to avoid.
  const candidates = generateAgentsOverviewCandidates({ agents, containerSize, gap })
    .filter((candidate) => agents.length === 1 || candidate.columns >= 2);
  const best = selectBestAgentsOverviewCandidate(
    candidates,
    previousOrientation,
  );
  if (!best?.meetsHardFloor) return null;

  if (
    previousLayout?.requestedMode !== "auto"
    || previousLayout.presentationMode !== "grid"
    || !previousLayout.candidate
    || (agents.length > 1 && previousLayout.candidate.columns < 2)
  ) {
    return best;
  }

  const current = evaluateCandidate(
    agents,
    containerSize,
    previousLayout.candidate.columns,
    gap,
  );
  if (!current.meetsHardFloor) return best;
  if (current.columns === best.columns && current.rows === best.rows) return current;

  const improvement = current.score > 0 ? (best.score - current.score) / current.score : Number.POSITIVE_INFINITY;
  return improvement >= AGENTS_OVERVIEW_SCORE_IMPROVEMENT_THRESHOLD ? best : current;
}

/** Resolve presentation only; this function has no runtime, selection, or navigation effects. */
export function resolveAgentsOverviewLayout(
  input: ResolveAgentsOverviewLayoutInput,
): AgentsOverviewLayoutResult {
  const agents = stableAgents(input.agents);
  if (agents.length === 0) return emptyResult(input.mode);

  const orderedAgentIds = agents.map(({ id }) => id);
  const focusedAgentId = resolveFocusedAgentId(
    orderedAgentIds,
    input.focusedAgentId,
    input.recentAgentIds,
  ) ?? orderedAgentIds[0];
  const gap = finiteNonNegative(input.gap ?? DEFAULT_AGENTS_OVERVIEW_GAP);

  if (input.mode === "single") {
    return singleResult(input.mode, orderedAgentIds, focusedAgentId, input.containerSize);
  }

  const candidate = input.mode === "auto"
    ? chooseAutoCandidate(agents, input.containerSize, gap, input.previousLayout)
    : selectBestAgentsOverviewCandidate(
        generateAgentsOverviewCandidates({ agents, containerSize: input.containerSize, gap }),
        input.previousLayout?.candidate?.orientation,
      );

  if (!candidate) {
    return singleResult(input.mode, orderedAgentIds, focusedAgentId, input.containerSize);
  }

  return gridResult(input.mode, agents, focusedAgentId, candidate, gap);
}

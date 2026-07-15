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
  /** Maximum number of floor-sized cards simultaneously visible in the viewport. */
  viewportCapacity: number;
  /** Normalized distance from the preferred width and height; lower is better. */
  preferredSizeDeviation: number;
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

/** Preferred working bounds for a terminal card before Auto begins compressing. */
export const TERMINAL_CARD_PREFERRED: Readonly<AgentsOverviewCardFloor> = Object.freeze({
  width: 640,
  height: 450,
});

/** Preferred working bounds for a chat card before Auto begins compressing. */
export const CHAT_CARD_PREFERRED: Readonly<AgentsOverviewCardFloor> = Object.freeze({
  width: 480,
  height: 450,
});

export const AGENTS_OVERVIEW_CARD_CHROME_HEIGHT = 52;
export const AGENTS_OVERVIEW_RESIZE_DEBOUNCE_MS = 120;
export const AGENTS_OVERVIEW_SCORE_IMPROVEMENT_THRESHOLD = 0.1;
export const DEFAULT_AGENTS_OVERVIEW_GAP = 6;

/** Top offset of a persisted Grid row inside its padded grid content. */
export function agentsOverviewGridRowOrigin(
  rowIndex: number,
  rowHeight: number,
  gap = DEFAULT_AGENTS_OVERVIEW_GAP,
): number {
  return gap + (Math.max(0, rowIndex) * (rowHeight + gap));
}

/** Bottom boundary of a persisted Grid row inside its padded grid content. */
export function agentsOverviewGridRowBoundary(
  rowIndex: number,
  rowHeight: number,
  gap = DEFAULT_AGENTS_OVERVIEW_GAP,
): number {
  return agentsOverviewGridRowOrigin(rowIndex, rowHeight, gap) + rowHeight;
}

const FLOAT_COMPARISON_EPSILON = 0.0001;

function finiteNonNegative(value: number): number {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}

function floorForMode(mode: AgentsOverviewCardMode): Readonly<AgentsOverviewCardFloor> {
  return mode === "terminal" ? TERMINAL_CARD_FLOOR : CHAT_CARD_FLOOR;
}

function preferredForMode(mode: AgentsOverviewCardMode): Readonly<AgentsOverviewCardFloor> {
  return mode === "terminal" ? TERMINAL_CARD_PREFERRED : CHAT_CARD_PREFERRED;
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

function requiredPreferredSize(agents: readonly AgentsOverviewLayoutAgent[]): AgentsOverviewCardFloor {
  return agents.reduce<AgentsOverviewCardFloor>(
    (preferred, agent) => {
      const agentPreferred = preferredForMode(agent.cardMode);
      return {
        width: Math.max(preferred.width, agentPreferred.width),
        height: Math.max(preferred.height, agentPreferred.height),
      };
    },
    { width: 0, height: 0 },
  );
}

function preferredSizeDeviation(
  width: number,
  height: number,
  preferred: AgentsOverviewCardFloor,
): number {
  const widthDeviation = preferred.width > 0 ? Math.abs(width - preferred.width) / preferred.width : 0;
  const heightDeviation = preferred.height > 0 ? Math.abs(height - preferred.height) / preferred.height : 0;
  return widthDeviation + heightDeviation;
}

function preferredViewportRows(
  height: number,
  preferredHeight: number,
  floorHeight: number,
  gap: number,
): number {
  const innerHeight = Math.max(0, height - (2 * gap));
  const hardFloorCapacity = Math.max(1, Math.floor((innerHeight + gap) / (floorHeight + gap)));
  const idealRows = Math.max(1, Math.round((innerHeight + gap) / (preferredHeight + gap)));
  return Math.min(hardFloorCapacity, idealRows);
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
  const floor = requiredFloor(agents);
  const preferred = requiredPreferredSize(agents);
  const viewportRows = floor.height > 0
    ? Math.max(0, Math.floor((height + safeGap) / (floor.height + safeGap)))
    : rows;

  return {
    columns: safeColumns,
    rows,
    cardWidth,
    cardHeight,
    minimumCardArea,
    emptyCells: (safeColumns * rows) - agents.length,
    orientation: orientationFor(safeColumns, rows),
    meetsHardFloor,
    viewportCapacity: Math.min(agents.length, safeColumns * viewportRows),
    preferredSizeDeviation: preferredSizeDeviation(cardWidth, cardHeight, preferred),
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
  containerSize: AgentsOverviewContainerSize,
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
    requiresScroll: contentWidth > finiteNonNegative(containerSize.width)
      || contentHeight > finiteNonNegative(containerSize.height),
    candidate,
  };
}

function evaluateAutoCandidate(
  agents: readonly AgentsOverviewLayoutAgent[],
  containerSize: AgentsOverviewContainerSize,
  columns: number,
  gap: number,
): AgentsOverviewGridCandidate | null {
  const floor = requiredFloor(agents);
  const preferred = requiredPreferredSize(agents);
  const width = finiteNonNegative(containerSize.width);
  const height = finiteNonNegative(containerSize.height);
  const innerHeight = Math.max(0, height - (2 * gap));
  // The hard floor is a last-resort constraint, not Auto's target. Choose the
  // visible row count nearest the preferred working height, then compress only
  // when the pane is too short to retain that height.
  const viewportRows = preferredViewportRows(height, preferred.height, floor.height, gap);

  const rows = Math.ceil(agents.length / columns);
  const simultaneousRows = Math.min(rows, viewportRows);
  const availableCardWidth = (width - (Math.max(0, columns - 1) * gap)) / columns;
  // Auto is the responsive multi-agent mode. When the pane cannot fit two
  // useful cards side by side, preserve the roster as a one-column scrolling
  // grid instead of silently turning Auto into focused Single.
  const cardWidth = columns === 1
    ? Math.max(floor.width, availableCardWidth)
    : availableCardWidth;
  const cardHeight = Math.max(
    floor.height,
    Math.min(
      preferred.height,
      (innerHeight - (Math.max(0, simultaneousRows - 1) * gap)) / simultaneousRows,
    ),
  );
  if (columns > 1 && cardWidth < floor.width) return null;

  return {
    columns,
    rows,
    cardWidth,
    cardHeight,
    minimumCardArea: cardWidth * cardHeight,
    emptyCells: (columns * rows) - agents.length,
    orientation: orientationFor(columns, rows),
    meetsHardFloor: true,
    viewportCapacity: Math.min(agents.length, columns * viewportRows),
    preferredSizeDeviation: preferredSizeDeviation(cardWidth, cardHeight, preferred),
    score: 1 / (1 + preferredSizeDeviation(cardWidth, cardHeight, preferred)),
  };
}

function compareAutoCandidates(
  left: AgentsOverviewGridCandidate,
  right: AgentsOverviewGridCandidate,
  previousOrientation?: AgentsOverviewOrientation | null,
): number {
  if (left.viewportCapacity !== right.viewportCapacity) {
    return right.viewportCapacity - left.viewportCapacity;
  }
  if (Math.abs(left.preferredSizeDeviation - right.preferredSizeDeviation) > FLOAT_COMPARISON_EPSILON) {
    return left.preferredSizeDeviation - right.preferredSizeDeviation;
  }
  return compareAgentsOverviewCandidates(left, right, previousOrientation);
}

function chooseAutoCandidate(
  agents: readonly AgentsOverviewLayoutAgent[],
  containerSize: AgentsOverviewContainerSize,
  gap: number,
  previousLayout: AgentsOverviewLayoutResult | null | undefined,
): AgentsOverviewGridCandidate | null {
  if (agents.length <= 1) return null;
  const previousOrientation = previousLayout?.candidate?.orientation;
  const preferred = requiredPreferredSize(agents);
  const maxColumns = Math.min(
    agents.length,
    Math.max(
      1,
      Math.floor((finiteNonNegative(containerSize.width) + gap) / (preferred.width + gap)),
    ),
  );
  const candidateColumns = maxColumns < 2
    ? [1]
    : Array.from({ length: maxColumns - 1 }, (_, index) => index + 2);

  const candidates = candidateColumns
    .map((columns) => evaluateAutoCandidate(agents, containerSize, columns, gap))
    .filter((candidate): candidate is AgentsOverviewGridCandidate => candidate !== null);
  const best = [...candidates].sort((left, right) =>
    compareAutoCandidates(left, right, previousOrientation))[0] ?? null;
  if (!best) return null;

  if (
    previousLayout?.requestedMode !== "auto"
    || previousLayout.presentationMode !== "grid"
    || !previousLayout.candidate
    || (best.columns >= 2 && previousLayout.candidate.columns < 2)
  ) {
    return best;
  }

  const current = evaluateAutoCandidate(
    agents,
    containerSize,
    previousLayout.candidate.columns,
    gap,
  );
  if (!current) return best;
  if (current.columns === best.columns && current.rows === best.rows) return current;
  if (current.viewportCapacity !== best.viewportCapacity) return best;

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

  return gridResult(input.mode, agents, focusedAgentId, candidate, gap, input.containerSize);
}

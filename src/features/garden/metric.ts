/**
 * The Garden map's distance metric.
 *
 * Three components, each measuring a different kind of relatedness, composed
 * into one distance and then cut to a sparse k-nearest-neighbour graph:
 *
 * - `d_affil`  — weighted cosine over facet vectors (see `facets.ts`).
 * - `d_interact` — personalized-PageRank affinity over agent communication.
 * - `d_use`    — pointwise mutual information over co-use in the same thread.
 *
 * ## Why personalized PageRank rather than shortest path
 *
 * Hub agents wreck shortest-path distance. An orchestrator with degree 15 puts
 * every one of its neighbours 2 hops from every other, so the entire cluster
 * collapses into a hairball — exactly the failure the current spring layout
 * shows. PPR splits probability mass at high-degree nodes, so hub-mediated
 * adjacency is discounted automatically, while genuinely multiple independent
 * paths *do* register as closer.
 *
 * ## Why PMI rather than raw co-occurrence
 *
 * Raw co-occurrence counts just re-rank by popularity, which would make the
 * busiest agent "close to" everything. PMI normalizes by each entity's own
 * frequency, so it measures surprise rather than volume.
 *
 * ## Composition renormalizes over applicable terms
 *
 * A folder has no communication history. Scoring `d_interact = 1` for it would
 * push every folder pair uniformly far apart for a reason that says nothing
 * about folders. So each term declares applicability and the weighted mean is
 * taken over applicable terms only.
 *
 * ## The weights are a scene property, not a live control
 *
 * `lambda` is fixed per scene. Lens toggles change visibility, tint, and edge
 * rendering — never geometry. This is not a preference: per
 * `docs/specs/2026-07-14-entity-oriented-agent-semantics.md`, Garden spatial
 * proximity is interpretive and never a claim about system state, so a map
 * whose shape shifts when a filter toggles would be asserting something it
 * cannot back.
 */

import type { FacetCorpus, FacetToken, FacetVector, GardenEntityFacets } from "./facets";
import { cosine, facetVector } from "./facets";
import { entityKey, type EntityKind } from "./entityRef";

/**
 * Bumping this invalidates persisted layouts. The scene records the version it
 * was derived under; on mismatch the map offers re-derivation with a diff
 * rather than silently reflowing, because a silent reflow destroys the user's
 * learned sense of where things live.
 */
export const METRIC_VERSION = 1;

export interface MetricWeights {
  affil: number;
  interact: number;
}

export const DEFAULT_METRIC_WEIGHTS: MetricWeights = {
  affil: 1.0,
  interact: 0.8,
};

/**
 * Distance above which a pair is treated as unrelated (`Infinity`).
 *
 * Genuine `Infinity` rather than a large finite number is what keeps every
 * downstream stage linear: the layout runs on a sparse k-NN graph, never a
 * dense matrix.
 */
export const DEFAULT_THETA_CUT = 0.86;

/** Neighbours retained per entity in the sparse graph. */
export const DEFAULT_KNN = 8;

/**
 * Additive penalty for a cross-kind pair.
 *
 * Without it, an agent and a skill sharing one facet would read as exactly as
 * close as two agents sharing one facet, and heterogeneous kinds would
 * interleave rather than forming legible neighbourhoods. Naturally coupled
 * pairs get a reduced offset because their coupling facets (`deployed:*`,
 * `origin:*`) are real canonical edges rather than incidental overlap.
 */
export const CROSS_KIND_OFFSET = 0.12;
const COUPLED_KIND_OFFSET = 0.04;

const COUPLED_KIND_PAIRS: ReadonlySet<string> = new Set([
  pairTag("agent", "artifact"),
  pairTag("agent", "skill"),
  pairTag("agent", "class"),
  pairTag("agent", "worktree"),
  pairTag("agent", "automation"),
  pairTag("class", "skill"),
  pairTag("automation", "automation_run"),
  pairTag("folder", "worktree"),
  pairTag("agent", "folder"),
]);

function pairTag(a: EntityKind, b: EntityKind): string {
  return a <= b ? `${a}|${b}` : `${b}|${a}`;
}

export function crossKindOffset(a: EntityKind, b: EntityKind): number {
  if (a === b) return 0;
  return COUPLED_KIND_PAIRS.has(pairTag(a, b)) ? COUPLED_KIND_OFFSET : CROSS_KIND_OFFSET;
}

/**
 * Repulsion for explicit user negatives.
 *
 * `ignored_pairs` and `suppressed_seed_pairs` in `topology.json` are the user
 * saying "these two are not related" — the second is a tombstone recording a
 * deleted team-seeded edge. Both are rare and high-value. A map where deleting
 * a link moves nothing trains people to stop correcting it, so these must be
 * large enough to push a pair out of each other's k-NN neighbourhood.
 */
export const IGNORE_REPULSION = 0.55;
export const SEED_SUPPRESSION_REPULSION = 0.35;
/** Repulsion from a district the user explicitly rejected for this entity. */
export const EXCLUDE_REPULSION = 0.5;

/** Floor inside the PPR log, and the normalizer that maps it onto [0, 1]. */
const PPR_EPSILON = 1e-6;
const PPR_LOG_RANGE = -Math.log(PPR_EPSILON);

/**
 * Hard ceiling on PPR input size.
 *
 * PPR is dense O(n^2) per source. At roster scale (tens of agents) that is
 * microseconds; extended to thousands of file entities it is not. Interaction
 * affinity is therefore confined to the agent subgraph by construction — files
 * relate through facets, never through diffusion. Exceeding the cap disables
 * the term rather than degrading silently into a frame-time cliff.
 */
export const MAX_PPR_NODES = 400;

// --- Personalized PageRank ------------------------------------------------

export interface WeightedEdge {
  source: string;
  target: string;
  weight: number;
}

/**
 * Edge weights for the interaction graph.
 *
 * Manual topology edges and observed activity are separate signals and must not
 * be blended into one number upstream: a manual edge is durable and binary
 * (`topology.json` is undirected, untyped, and unweighted — an explicit
 * non-goal of the communication-topology spec), while activity is derived,
 * volatile, and recomputed from the `interactions` table on every read.
 */
export interface InteractionWeightInput {
  manual: boolean;
  /** `CommunicationEdge.recency` in [0, 1], 1 = just now. */
  recency: number;
  /** Heaviest `InteractionKind` observed for the pair. */
  kind?: "Task" | "Reply" | "Message" | "Notification";
}

const KIND_WEIGHT: Record<NonNullable<InteractionWeightInput["kind"]>, number> = {
  Task: 1.0,
  Reply: 0.8,
  Message: 0.6,
  Notification: 0.2,
};

export function interactionWeight(input: InteractionWeightInput): number {
  const activity = input.recency * (input.kind ? KIND_WEIGHT[input.kind] : KIND_WEIGHT.Message);
  return (input.manual ? 1.0 : 0) + 0.6 * activity;
}

export type PprMatrix = ReadonlyMap<string, ReadonlyMap<string, number>>;

/**
 * All-pairs personalized PageRank by power iteration.
 *
 * Deterministic: nodes are visited in sorted order and there is no random
 * restart vector. Returns an empty matrix above `MAX_PPR_NODES`.
 */
export function personalizedPageRank(
  nodeIds: readonly string[],
  edges: readonly WeightedEdge[],
  options: { alpha?: number; iterations?: number } = {},
): PprMatrix {
  const ids = [...new Set(nodeIds)].sort();
  if (ids.length === 0 || ids.length > MAX_PPR_NODES) return new Map();

  const alpha = options.alpha ?? 0.15;
  const iterations = options.iterations ?? 20;
  const index = new Map(ids.map((id, i) => [id, i]));
  const size = ids.length;

  // Row-normalized transition matrix, symmetrized: layout needs a metric, and
  // an undirected structural edge carries no direction to preserve.
  const weights = new Float64Array(size * size);
  for (const edge of edges) {
    const from = index.get(edge.source);
    const to = index.get(edge.target);
    if (from === undefined || to === undefined || from === to) continue;
    if (!(edge.weight > 0)) continue;
    weights[from * size + to] += edge.weight;
    weights[to * size + from] += edge.weight;
  }
  const rowSums = new Float64Array(size);
  for (let row = 0; row < size; row += 1) {
    let sum = 0;
    for (let col = 0; col < size; col += 1) sum += weights[row * size + col];
    rowSums[row] = sum;
  }

  const result = new Map<string, Map<string, number>>();
  const current = new Float64Array(size);
  const next = new Float64Array(size);

  for (let seed = 0; seed < size; seed += 1) {
    current.fill(0);
    current[seed] = 1;
    for (let iteration = 0; iteration < iterations; iteration += 1) {
      next.fill(0);
      for (let row = 0; row < size; row += 1) {
        const mass = current[row];
        if (mass === 0) continue;
        const rowSum = rowSums[row];
        if (rowSum === 0) {
          // Dangling node: all mass returns to the seed rather than leaking,
          // which keeps every row a probability distribution.
          next[seed] += mass;
          continue;
        }
        const spread = (1 - alpha) * mass;
        for (let col = 0; col < size; col += 1) {
          const weight = weights[row * size + col];
          if (weight !== 0) next[col] += (spread * weight) / rowSum;
        }
        next[seed] += alpha * mass;
      }
      current.set(next);
    }
    const row = new Map<string, number>();
    for (let col = 0; col < size; col += 1) {
      if (col !== seed && current[col] > 0) row.set(ids[col], current[col]);
    }
    result.set(ids[seed], row);
  }
  return result;
}

/**
 * Symmetrized PPR affinity mapped onto [0, 1], where 0 is adjacent and 1 is
 * unreachable. The log compresses the heavy tail so a pair with ten times the
 * affinity is meaningfully rather than dramatically closer.
 *
 * The average is load-bearing, not defensive: PPR is genuinely asymmetric even
 * on an undirected graph, because the walk depends on the seed's degree — a
 * leaf sees its hub as much closer than the hub sees the leaf. Layout needs a
 * metric, so the asymmetry is resolved once, here.
 */
export function interactionDistance(a: string, b: string, ppr: PprMatrix): number | null {
  const forward = ppr.get(a)?.get(b);
  const backward = ppr.get(b)?.get(a);
  if (forward === undefined && backward === undefined) {
    // Both endpoints present but no affinity: genuinely unreachable.
    if (ppr.has(a) && ppr.has(b)) return 1;
    return null; // Not an agent pair — term does not apply.
  }
  const affinity = ((forward ?? 0) + (backward ?? 0)) / 2;
  const distance = -Math.log(affinity + PPR_EPSILON) / PPR_LOG_RANGE;
  return clamp01(distance);
}

// --- Composition ----------------------------------------------------------

export interface MetricContext {
  corpus: FacetCorpus;
  weights: MetricWeights;
  /** Decayed weights for `scene_anchor:*` tokens, keyed by token. */
  sceneWeights?: ReadonlyMap<FacetToken, number>;
  ppr?: PprMatrix;
  /** Canonical `a b` keys (sorted) from `topology.ignored_pairs`. */
  ignoredPairs?: ReadonlySet<string>;
  /** Canonical keys from `topology.suppressed_seed_pairs`. */
  suppressedSeedPairs?: ReadonlySet<string>;
  /** Current district per entity key, for evaluating exclusion repulsion. */
  districtOf?: ReadonlyMap<string, string>;
  thetaCut?: number;
}

export function canonicalPairKey(a: string, b: string): string {
  return a <= b ? `${a} ${b}` : `${b} ${a}`;
}

export interface DistanceTerm {
  name: "affil" | "interact";
  distance: number;
  weight: number;
}

export interface DistanceResult {
  distance: number;
  /** Applicable terms only, so an inapplicable term is visibly absent. */
  terms: DistanceTerm[];
  offsets: Array<{ name: string; amount: number }>;
  cut: boolean;
}

/**
 * Cached facet vectors. Vector construction is O(|tokens|) and every entity is
 * compared against many others, so building them once per layout pass matters
 * more than any micro-optimization inside `cosine`.
 */
export class FacetVectorCache {
  private readonly vectors = new Map<string, FacetVector>();

  constructor(
    private readonly entities: ReadonlyMap<string, GardenEntityFacets>,
    private readonly corpus: FacetCorpus,
    private readonly sceneWeights?: ReadonlyMap<FacetToken, number>,
  ) {}

  get(key: string): FacetVector | null {
    const cached = this.vectors.get(key);
    if (cached) return cached;
    const facets = this.entities.get(key);
    if (!facets) return null;
    const vector = facetVector(facets, this.corpus, this.sceneWeights);
    this.vectors.set(key, vector);
    return vector;
  }
}

export function distanceBetween(
  a: GardenEntityFacets,
  b: GardenEntityFacets,
  context: MetricContext,
  cache?: FacetVectorCache,
): DistanceResult {
  const keyA = entityKey(a.ref);
  const keyB = entityKey(b.ref);

  const vectorA = cache?.get(keyA) ?? facetVector(a, context.corpus, context.sceneWeights);
  const vectorB = cache?.get(keyB) ?? facetVector(b, context.corpus, context.sceneWeights);

  const terms: DistanceTerm[] = [
    { name: "affil", distance: 1 - cosine(vectorA, vectorB), weight: context.weights.affil },
  ];

  if (context.ppr) {
    const interact = interactionDistance(keyA, keyB, context.ppr);
    if (interact !== null) {
      terms.push({ name: "interact", distance: interact, weight: context.weights.interact });
    }
  }

  const totalWeight = terms.reduce((sum, term) => sum + term.weight, 0);
  const weighted =
    totalWeight > 0
      ? terms.reduce((sum, term) => sum + term.weight * term.distance, 0) / totalWeight
      : 1;

  const offsets: Array<{ name: string; amount: number }> = [];
  const kindOffset = crossKindOffset(a.ref.kind, b.ref.kind);
  if (kindOffset > 0) offsets.push({ name: "cross-kind", amount: kindOffset });

  const pairKey = canonicalPairKey(keyA, keyB);
  if (context.ignoredPairs?.has(pairKey)) {
    offsets.push({ name: "ignored-pair", amount: IGNORE_REPULSION });
  }
  if (context.suppressedSeedPairs?.has(pairKey)) {
    offsets.push({ name: "seed-suppressed", amount: SEED_SUPPRESSION_REPULSION });
  }
  const exclusion = exclusionPenalty(a, b, context.districtOf);
  if (exclusion > 0) offsets.push({ name: "excluded-district", amount: exclusion });

  const raw = weighted + offsets.reduce((sum, offset) => sum + offset.amount, 0);
  const theta = context.thetaCut ?? DEFAULT_THETA_CUT;
  const cut = raw > theta;
  return { distance: cut ? Infinity : raw, terms, offsets, cut };
}

function exclusionPenalty(
  a: GardenEntityFacets,
  b: GardenEntityFacets,
  districtOf?: ReadonlyMap<string, string>,
): number {
  if (!districtOf) return 0;
  const districtA = districtOf.get(entityKey(a.ref));
  const districtB = districtOf.get(entityKey(b.ref));
  const aRejectsB = districtB !== undefined && a.excludes.includes(districtB);
  const bRejectsA = districtA !== undefined && b.excludes.includes(districtA);
  return aRejectsB || bRejectsA ? EXCLUDE_REPULSION : 0;
}

// --- Sparse neighbour graph ----------------------------------------------

export interface Neighbour {
  key: string;
  distance: number;
}

export type NeighbourGraph = ReadonlyMap<string, Neighbour[]>;

/**
 * Build the sparse k-NN graph the layout runs on.
 *
 * All-pairs, so this must be called per district rather than over the whole
 * map. Districts exist partly as a computational firewall for exactly this
 * reason: they cap `n` in every superlinear stage. A district exceeding roughly
 * 60 members should be split into parcels instead of enlarging this input.
 */
export function buildNeighbourGraph(
  entities: readonly GardenEntityFacets[],
  context: MetricContext,
  k = DEFAULT_KNN,
): NeighbourGraph {
  const byKey = new Map(entities.map((entity) => [entityKey(entity.ref), entity]));
  const cache = new FacetVectorCache(byKey, context.corpus, context.sceneWeights);
  const candidates = new Map<string, Neighbour[]>();
  for (const entity of entities) candidates.set(entityKey(entity.ref), []);

  for (let i = 0; i < entities.length; i += 1) {
    for (let j = i + 1; j < entities.length; j += 1) {
      const keyI = entityKey(entities[i].ref);
      const keyJ = entityKey(entities[j].ref);
      const { distance } = distanceBetween(entities[i], entities[j], context, cache);
      if (!Number.isFinite(distance)) continue;
      candidates.get(keyI)!.push({ key: keyJ, distance });
      candidates.get(keyJ)!.push({ key: keyI, distance });
    }
  }

  const graph = new Map<string, Neighbour[]>();
  for (const [key, neighbours] of candidates) {
    // Tie-break on key so the graph is fully deterministic.
    neighbours.sort((l, r) => l.distance - r.distance || l.key.localeCompare(r.key));
    graph.set(key, neighbours.slice(0, k));
  }
  return graph;
}

/**
 * Symmetrize a k-NN graph: A keeping B implies B keeps A.
 *
 * Stress majorization needs a symmetric distance set, and pure top-k is
 * asymmetric around hubs — a hub appears in many neighbour lists while its own
 * list holds only its k closest.
 */
export function symmetrizeGraph(graph: NeighbourGraph): Map<string, Neighbour[]> {
  const merged = new Map<string, Map<string, number>>();
  for (const [key, neighbours] of graph) {
    if (!merged.has(key)) merged.set(key, new Map());
    for (const neighbour of neighbours) {
      merged.get(key)!.set(neighbour.key, neighbour.distance);
      if (!merged.has(neighbour.key)) merged.set(neighbour.key, new Map());
      merged.get(neighbour.key)!.set(key, neighbour.distance);
    }
  }
  const result = new Map<string, Neighbour[]>();
  for (const [key, neighbours] of merged) {
    result.set(
      key,
      [...neighbours.entries()]
        .map(([neighbourKey, distance]) => ({ key: neighbourKey, distance }))
        .sort((l, r) => l.distance - r.distance || l.key.localeCompare(r.key)),
    );
  }
  return result;
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 1;
  return Math.max(0, Math.min(1, value));
}

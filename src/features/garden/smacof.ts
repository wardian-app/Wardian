/**
 * Stress majorization (SMACOF) with a drift penalty.
 *
 * ## Why this replaces the spring simulation
 *
 * The existing layout in `features/graph/graphProjection.ts` is a spring-electric
 * force simulation. It minimizes an energy with no relationship to the semantic
 * metric, so rendered pixel distance means nothing in particular — which is why
 * `gardenProjection.ts` had to discard those positions and substitute a
 * phyllotaxis spiral. Stress majorization minimizes
 *
 *   sigma(X) = sum over pairs of  w_ij * (||x_i - x_j|| - d_ij)^2
 *
 * so rendered distance *converges to* semantic distance. That is the difference
 * between a graph drawing and a map with a scale bar.
 *
 * `w_ij = d_ij^-2` is the Kamada-Kawai weighting: it prioritizes getting short
 * distances right, which is what readability actually needs.
 *
 * ## Why the drift penalty
 *
 * Adding a term
 *
 *   + sum over i of  rho_i * ||x_i - p_i||^2
 *
 * where `p_i` is the entity's previous position turns layout from a
 * recomputation into an incremental optimization. Warm-started from the scene's
 * stored positions, insertion perturbs the neighbourhood and leaves the rest of
 * the district in place. Without it, every insertion visibly rearranges the map
 * and the user's learned sense of where things live is destroyed on every tick.
 *
 * Per-node stiffness encodes authority: pinned entities are immovable, entities
 * the user recently visited resist strongly, settled ones resist mildly, and
 * brand-new ones are free to find their place.
 *
 * ## Majorization step
 *
 * Per node, majorizing the stress term and adding the (already quadratic) drift
 * term gives a closed-form update:
 *
 *   x_i <- [ sum_j w_ij (x_j + d_ij * u_ij) + r_i * p_i ] / [ sum_j w_ij + r_i ]
 *
 * with `u_ij` the unit vector from `x_j` toward `x_i` and `r_i = rho_i * sum_j
 * w_ij` (see the drift constants on why stiffness is relative). Applied
 * Gauss-Seidel in sorted key order: deterministic, and it converges faster than
 * Jacobi.
 */

import type { GardenPosition } from "./garden.types";
import type { NeighbourGraph } from "./metric";

/**
 * Drift stiffness presets, expressed *relative* to a node's own total stress
 * weight rather than in absolute units.
 *
 * This matters: stress weights are `w_ij = 1/d_ij^2` in world units, so at a
 * layout scale of 240 a typical `w` is around 4e-4. An absolute `rho` of 1.0
 * would outweigh every stress term by three orders of magnitude and freeze the
 * layout at its seed positions. Scaling by `sum_j w_ij` makes the number
 * dimensionless and readable: `1.0` means "drift pulls as hard as all this
 * node's distance targets combined", `0.05` means "5% as hard".
 */
export const DRIFT_PINNED = Infinity;
export const DRIFT_VISITED = 8.0;
export const DRIFT_SETTLED = 1.0;
export const DRIFT_NEW = 0.05;
/**
 * No drift at all — the node is free to go wherever the metric wants it.
 *
 * @internal Test support, no production caller: tests use it for an unanchored layout.
 */
export const DRIFT_FREE = 0;

/** World units per unit of semantic distance. */
export const DEFAULT_LAYOUT_SCALE = 240;

/** Default iteration budget, and the batch size for interruptible running. */
export const DEFAULT_MAX_ITERATIONS = 50;
export const DEFAULT_BATCH_ITERATIONS = 5;

/** Relative stress improvement below which iteration stops. */
export const DEFAULT_TOLERANCE = 1e-4;

const COINCIDENT_EPSILON = 1e-9;
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

export interface SmacofNode {
  key: string;
  /** Drift stiffness. `DRIFT_PINNED` makes the node a fixed point. */
  rho: number;
  /**
   * Anchor the drift term pulls toward — the entity's previous position. Absent
   * for entities that have never been laid out, which get `rho = DRIFT_NEW` and
   * are seeded from their neighbours.
   */
  anchor?: GardenPosition;
}

export interface SmacofInput {
  nodes: readonly SmacofNode[];
  /** Symmetric neighbour graph. Run `symmetrizeGraph` first. */
  graph: NeighbourGraph;
  /** Fallback centre for nodes with no placed neighbour to seed from. */
  center?: GardenPosition;
  scale?: number;
  tolerance?: number;
}

export interface SmacofState {
  readonly keys: readonly string[];
  readonly positions: ReadonlyMap<string, GardenPosition>;
  readonly iterations: number;
  /**
   * The full objective being minimized: weighted stress plus the drift term.
   * Monotonically non-increasing across iterations. Use `distanceError` for the
   * scale-bar quality of the drawing itself.
   */
  readonly stress: number;
  readonly converged: boolean;
}

interface InternalState {
  keys: string[];
  index: Map<string, number>;
  x: Float64Array;
  y: Float64Array;
  anchorX: Float64Array;
  anchorY: Float64Array;
  rho: Float64Array;
  /** Flattened symmetric pair list: [i, j] with target distance and weight. */
  pairI: Int32Array;
  pairJ: Int32Array;
  pairDistance: Float64Array;
  pairWeight: Float64Array;
  /** Total stress weight per node; drift stiffness is relative to it. */
  weightSum: Float64Array;
  /** CSR adjacency over the pair list. Constant, so built once. */
  neighbourStart: Int32Array;
  neighbourOf: Int32Array;
  neighbourPair: Int32Array;
  iterations: number;
  stress: number;
  converged: boolean;
  tolerance: number;
}

const internals = new WeakMap<SmacofState, InternalState>();

/**
 * Seed positions and precompute the pair list.
 *
 * A node with an anchor starts there (warm start). A node without one is placed
 * at the centroid of its already-anchored neighbours, offset along the golden
 * angle so several new nodes cannot coincide; with no anchored neighbour it
 * lands on a deterministic ring around `center`. Seeding from neighbours rather
 * than the centre is what stops a new entity from having to cross the district
 * to reach its place, which would read as unexplained motion.
 */
export function initSmacof(input: SmacofInput): SmacofState {
  const scale = input.scale ?? DEFAULT_LAYOUT_SCALE;
  const center = input.center ?? { x: 0, y: 0 };
  const nodes = [...input.nodes].sort((left, right) => left.key.localeCompare(right.key));
  const keys = nodes.map((node) => node.key);
  const index = new Map(keys.map((key, position) => [key, position]));
  const size = keys.length;

  const x = new Float64Array(size);
  const y = new Float64Array(size);
  const anchorX = new Float64Array(size);
  const anchorY = new Float64Array(size);
  const rho = new Float64Array(size);

  nodes.forEach((node, position) => {
    rho[position] = node.rho;
    if (node.anchor) {
      x[position] = node.anchor.x;
      y[position] = node.anchor.y;
      anchorX[position] = node.anchor.x;
      anchorY[position] = node.anchor.y;
    }
  });

  const anchored = nodes.map((node) => node.anchor !== undefined);
  let unanchoredSeen = 0;
  nodes.forEach((node, position) => {
    if (anchored[position]) return;
    const neighbours = input.graph.get(node.key) ?? [];
    let sumX = 0;
    let sumY = 0;
    let count = 0;
    for (const neighbour of neighbours) {
      const neighbourIndex = index.get(neighbour.key);
      if (neighbourIndex === undefined || !anchored[neighbourIndex]) continue;
      sumX += x[neighbourIndex];
      sumY += y[neighbourIndex];
      count += 1;
    }
    const angle = unanchoredSeen * GOLDEN_ANGLE;
    unanchoredSeen += 1;
    const radius = scale * 0.35;
    if (count > 0) {
      x[position] = sumX / count + Math.cos(angle) * radius;
      y[position] = sumY / count + Math.sin(angle) * radius;
    } else {
      // Vogel's model: radius grows as sqrt(i), which is what keeps a
      // phyllotactic spiral at uniform density.
      //
      // This previously grew *linearly*, so the extent of a seeded group scaled
      // with n rather than sqrt(n). It only shows up when a parcel is
      // metrically degenerate — entities that share no distinguishing facet
      // have no neighbours to be pulled toward, so they stay near their seeds
      // and the seed pattern becomes the layout. Thirty automations with nothing
      // but their own ids smeared across ~1800 world units, which then set the
      // grid pitch for every district on the map.
      const ringRadius = scale * 0.3 * Math.sqrt(unanchoredSeen);
      x[position] = center.x + Math.cos(angle) * ringRadius;
      y[position] = center.y + Math.sin(angle) * ringRadius;
    }
    // A node with no stored position has no anchor to drift toward, so its
    // drift term is centred on its seed and contributes almost nothing at
    // DRIFT_NEW stiffness.
    anchorX[position] = x[position];
    anchorY[position] = y[position];
  });

  const pairs = collectPairs(input.graph, index, scale);
  const adjacency = buildAdjacency(pairs, size);

  const internal: InternalState = {
    keys,
    index,
    x,
    y,
    anchorX,
    anchorY,
    rho,
    ...pairs,
    ...adjacency,
    iterations: 0,
    stress: 0,
    converged: size <= 1 || pairs.pairI.length === 0,
    tolerance: input.tolerance ?? DEFAULT_TOLERANCE,
  };
  internal.stress = computeObjective(internal);

  const state: SmacofState = {
    keys,
    positions: toPositionMap(keys, x, y),
    iterations: 0,
    stress: internal.stress,
    converged: internal.converged,
  };
  internals.set(state, internal);
  return state;
}

/**
 * CSR adjacency plus per-node total stress weight. Both are constant for the
 * lifetime of a state, so they are built once at init rather than per batch.
 */
function buildAdjacency(pairs: PairArrays, size: number) {
  const { pairI, pairJ, pairWeight } = pairs;
  const neighbourStart = new Int32Array(size + 1);
  const weightSum = new Float64Array(size);
  for (let pair = 0; pair < pairI.length; pair += 1) {
    neighbourStart[pairI[pair] + 1] += 1;
    neighbourStart[pairJ[pair] + 1] += 1;
    weightSum[pairI[pair]] += pairWeight[pair];
    weightSum[pairJ[pair]] += pairWeight[pair];
  }
  for (let node = 0; node < size; node += 1) neighbourStart[node + 1] += neighbourStart[node];

  const cursor = Int32Array.from(neighbourStart);
  const neighbourOf = new Int32Array(pairI.length * 2);
  const neighbourPair = new Int32Array(pairI.length * 2);
  for (let pair = 0; pair < pairI.length; pair += 1) {
    const i = pairI[pair];
    const j = pairJ[pair];
    neighbourOf[cursor[i]] = j;
    neighbourPair[cursor[i]] = pair;
    cursor[i] += 1;
    neighbourOf[cursor[j]] = i;
    neighbourPair[cursor[j]] = pair;
    cursor[j] += 1;
  }
  return { weightSum, neighbourStart, neighbourOf, neighbourPair };
}

/**
 * Run a bounded batch of iterations.
 *
 * Batching is what makes recomputation interruptible: 3-5 iterations per
 * animation frame keeps a worst-case district reflow off the critical path, and
 * the map visibly settles instead of popping — which doubles as the explanation
 * for why something moved.
 */
export function smacofStep(
  state: SmacofState,
  iterations = DEFAULT_BATCH_ITERATIONS,
): SmacofState {
  const internal = internals.get(state);
  if (!internal) throw new Error("smacofStep called on a state not produced by initSmacof");
  if (internal.converged) return state;

  const {
    x,
    y,
    anchorX,
    anchorY,
    rho,
    pairDistance,
    pairWeight,
    weightSum,
    neighbourStart,
    neighbourOf,
    neighbourPair,
  } = internal;
  const size = internal.keys.length;

  let converged = false;
  let completed = 0;
  for (let iteration = 0; iteration < iterations; iteration += 1) {
    // Gauss-Seidel in sorted index order: deterministic and faster-converging
    // than Jacobi.
    for (let node = 0; node < size; node += 1) {
      if (rho[node] === Infinity) continue; // pinned: a hard fixed point

      const drift = rho[node] * weightSum[node];
      let numeratorX = anchorX[node] * drift;
      let numeratorY = anchorY[node] * drift;
      let denominator = drift;

      for (let slot = neighbourStart[node]; slot < neighbourStart[node + 1]; slot += 1) {
        const other = neighbourOf[slot];
        const pair = neighbourPair[slot];
        const weight = pairWeight[pair];
        const target = pairDistance[pair];

        const dx = x[node] - x[other];
        const dy = y[node] - y[other];
        const length = Math.hypot(dx, dy);

        let unitX: number;
        let unitY: number;
        if (length < COINCIDENT_EPSILON) {
          // Coincident nodes have no direction to separate along. Derive one
          // deterministically from the pair index so the result is reproducible
          // rather than dependent on floating-point noise.
          const angle = (pair + 1) * GOLDEN_ANGLE;
          unitX = Math.cos(angle);
          unitY = Math.sin(angle);
        } else {
          unitX = dx / length;
          unitY = dy / length;
        }

        numeratorX += weight * (x[other] + target * unitX);
        numeratorY += weight * (y[other] + target * unitY);
        denominator += weight;
      }

      if (denominator > 0) {
        x[node] = numeratorX / denominator;
        y[node] = numeratorY / denominator;
      }
    }

    completed += 1;
    const objective = computeObjective(internal);
    const improvement =
      internal.stress > 0 ? (internal.stress - objective) / internal.stress : 0;
    internal.stress = objective;
    if (improvement >= 0 && improvement < internal.tolerance) {
      converged = true;
      break;
    }
  }

  internal.iterations += completed;
  internal.converged = converged;

  const next: SmacofState = {
    keys: internal.keys,
    positions: toPositionMap(internal.keys, x, y),
    iterations: internal.iterations,
    stress: internal.stress,
    converged,
  };
  internals.set(next, internal);
  return next;
}

/** Run to convergence or the iteration budget, whichever comes first. */
export function runSmacof(
  input: SmacofInput,
  maxIterations = DEFAULT_MAX_ITERATIONS,
): SmacofState {
  let state = initSmacof(input);
  while (!state.converged && state.iterations < maxIterations) {
    const remaining = maxIterations - state.iterations;
    const next = smacofStep(state, Math.min(DEFAULT_BATCH_ITERATIONS, remaining));
    if (next.iterations === state.iterations) break; // no progress possible
    state = next;
  }
  return state;
}

/**
 * Root-mean-square error between rendered and target distances, in world units.
 *
 * The honest quality measure for a map: it says how far off the scale bar is.
 * Raw stress is scale-dependent and not comparable across districts.
 *
 * @internal Test support, no production caller: layout tests measure map quality with it.
 */
export function distanceError(state: SmacofState): number {
  const internal = internals.get(state);
  if (!internal || internal.pairI.length === 0) return 0;
  let total = 0;
  for (let pair = 0; pair < internal.pairI.length; pair += 1) {
    const i = internal.pairI[pair];
    const j = internal.pairJ[pair];
    const rendered = Math.hypot(internal.x[i] - internal.x[j], internal.y[i] - internal.y[j]);
    const error = rendered - internal.pairDistance[pair];
    total += error * error;
  }
  return Math.sqrt(total / internal.pairI.length);
}

/**
 * Largest displacement of any node between two layout passes, in world units.
 *
 * @internal Test support, no production caller: layout tests check stability with it.
 */
export function maxDisplacement(
  before: ReadonlyMap<string, GardenPosition>,
  after: ReadonlyMap<string, GardenPosition>,
): number {
  let worst = 0;
  for (const [key, position] of before) {
    const next = after.get(key);
    if (!next) continue;
    worst = Math.max(worst, Math.hypot(next.x - position.x, next.y - position.y));
  }
  return worst;
}

interface PairArrays {
  pairI: Int32Array;
  pairJ: Int32Array;
  pairDistance: Float64Array;
  pairWeight: Float64Array;
}

function collectPairs(
  graph: NeighbourGraph,
  index: ReadonlyMap<string, number>,
  scale: number,
): PairArrays {
  const seen = new Set<string>();
  const rows: Array<{ i: number; j: number; distance: number }> = [];
  for (const [key, neighbours] of graph) {
    const i = index.get(key);
    if (i === undefined) continue;
    for (const neighbour of neighbours) {
      const j = index.get(neighbour.key);
      if (j === undefined || i === j) continue;
      if (!Number.isFinite(neighbour.distance)) continue;
      const pairKey = i < j ? `${i}:${j}` : `${j}:${i}`;
      if (seen.has(pairKey)) continue;
      seen.add(pairKey);
      rows.push({ i: Math.min(i, j), j: Math.max(i, j), distance: neighbour.distance * scale });
    }
  }
  // Sorted so the pair index — and therefore the coincident-node tie-break — is
  // stable across runs.
  rows.sort((left, right) => left.i - right.i || left.j - right.j);

  const pairI = new Int32Array(rows.length);
  const pairJ = new Int32Array(rows.length);
  const pairDistance = new Float64Array(rows.length);
  const pairWeight = new Float64Array(rows.length);
  rows.forEach((row, position) => {
    pairI[position] = row.i;
    pairJ[position] = row.j;
    // Guard a zero target: identical entities would otherwise give an infinite
    // weight and dominate the whole district.
    const target = Math.max(row.distance, scale * 1e-3);
    pairDistance[position] = target;
    pairWeight[position] = 1 / (target * target);
  });
  return { pairI, pairJ, pairDistance, pairWeight };
}

/**
 * The full objective the update actually minimizes: weighted stress *plus* the
 * drift term.
 *
 * Reporting only the stress term would be actively misleading — the majorization
 * decreases the sum, so the distance term alone can rise while the objective
 * falls, and a convergence check built on it would oscillate and never settle.
 * `distanceError` remains the pure-distance quality measure.
 */
function computeObjective(internal: InternalState): number {
  const { x, y, anchorX, anchorY, rho, weightSum, pairI, pairJ, pairDistance, pairWeight } =
    internal;
  let objective = 0;
  for (let pair = 0; pair < pairI.length; pair += 1) {
    const i = pairI[pair];
    const j = pairJ[pair];
    const rendered = Math.hypot(x[i] - x[j], y[i] - y[j]);
    const error = rendered - pairDistance[pair];
    objective += pairWeight[pair] * error * error;
  }
  for (let node = 0; node < rho.length; node += 1) {
    if (!Number.isFinite(rho[node]) || rho[node] === 0) continue; // pinned or free
    const drift = rho[node] * weightSum[node];
    if (drift === 0) continue;
    const dx = x[node] - anchorX[node];
    const dy = y[node] - anchorY[node];
    objective += drift * (dx * dx + dy * dy);
  }
  return objective;
}

function toPositionMap(
  keys: readonly string[],
  x: Float64Array,
  y: Float64Array,
): Map<string, GardenPosition> {
  const positions = new Map<string, GardenPosition>();
  keys.forEach((key, position) => {
    positions.set(key, { x: x[position], y: y[position] });
  });
  return positions;
}

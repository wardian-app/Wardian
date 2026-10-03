/**
 * Overlap removal by separation constraints (VPSC).
 *
 * ## Why not push-apart
 *
 * Garden units have wildly different footprints — an agent dot, an automation
 * tile, a folder card. Naive push-apart reorders neighbours, so the map
 * visibly changes shape when a label gets longer or a unit gains a badge. On a
 * canvas that is cosmetic. On a map it destroys the property the whole design
 * exists to provide: that the same object is always in the same place.
 *
 * The variable placement with separation constraints formulation solves, per
 * axis,
 *
 *   minimize   sum_i w_i (x_i - d_i)^2
 *   subject to x_j - x_i >= gap_ij   for each constraint (i, j)
 *
 * which is minimal weighted displacement subject to provable non-overlap, with
 * the constraint order preserved by construction.
 *
 * ## What this implements, precisely
 *
 * The block-merging *satisfaction* algorithm of Dwyer, Marriott and Stuckey,
 * without block splitting. It always terminates, always produces a feasible
 * (non-overlapping, order-preserving) result, and minimizes displacement within
 * each block. Omitting the split step means a merged block is occasionally held
 * together tighter than a globally optimal solution would — the result can be
 * slightly conservative, never infeasible. Splitting is the remaining half of
 * the algorithm and is deliberately deferred: it roughly doubles the code and
 * the failure surface for a refinement invisible at the sizes the Garden
 * actually lays out.
 *
 * ## Axis passes
 *
 * Overlap is two-dimensional but the solver is one-dimensional, so a horizontal
 * pass runs first, then a vertical pass over whatever still overlaps. Each pair
 * is assigned to the axis needing the smaller displacement, which keeps units
 * near where the metric put them instead of smearing everything sideways.
 *
 * Pair enumeration is O(n^2). That is deliberate and bounded: districts cap
 * membership (see `MAX_DISTRICT_MEMBERS`), so this runs over tens of units, and
 * a scanline would add complexity for no measurable gain at that size.
 */

import type { GardenPosition } from "./garden.types";

/** Minimum clear space between two unit footprints, in world units. */
export const DEFAULT_UNIT_PADDING = 14;

/**
 * Slack when testing clearance.
 *
 * The solver satisfies a constraint to within floating-point error, so a pair
 * separated by exactly the required gap lands a few ulps short of it. Without
 * slack those pairs read as overlapping forever: each round re-solves them,
 * nothing moves because the constraint is already satisfied, and the loop burns
 * its whole budget reporting phantom overlaps.
 */
const CLEARANCE_EPSILON = 1e-6;

export interface UnitBox {
  key: string;
  /** Centre position, typically straight from the layout stage. */
  position: GardenPosition;
  width: number;
  height: number;
  /**
   * Displacement resistance. `Infinity` pins the unit: constraints route
   * around it rather than moving it.
   */
  weight?: number;
}

export interface SeparationConstraint {
  left: string;
  right: string;
  gap: number;
}

const PINNED_WEIGHT = 1e9;

// --- One-dimensional solver ----------------------------------------------

interface Variable {
  key: string;
  desired: number;
  weight: number;
  /** Position within the owning block, relative to the block's reference. */
  offset: number;
  block: number;
}

interface Block {
  variables: number[];
  position: number;
  weight: number;
  weightedPosition: number;
  alive: boolean;
}

/** Re-solve passes allowed before `solveAxis` returns a possibly-infeasible result. */
export const DEFAULT_AXIS_PASSES = 16;

/**
 * Solve one axis to feasibility.
 *
 * Wraps the block-merging solver in a fixed-point loop. One pass is not enough:
 * omitting block splitting means two variables can end up in the same block via
 * a chain of merges, at relative offsets that satisfy the merging constraints
 * but violate some *other* constraint between them — and an intra-block
 * violation is invisible to the merge loop, which only ever compares distinct
 * blocks.
 *
 * Feeding each pass's output back in as the next pass's desired positions
 * rebuilds the block structure around the improved layout, exposing and fixing
 * those hidden violations. Every pass strictly reduces total violation, so this
 * settles in two or three passes. Displacement from the original desired
 * positions grows slightly with each pass, which is the price of not
 * implementing block splitting.
 */
export function solveAxis(
  desired: ReadonlyMap<string, number>,
  weights: ReadonlyMap<string, number>,
  constraints: readonly SeparationConstraint[],
  maxPasses = DEFAULT_AXIS_PASSES,
): Map<string, number> {
  let positions = new Map(desired);
  for (let pass = 0; pass < maxPasses; pass += 1) {
    const next = solveAxisOnce(positions, weights, constraints);
    if (!hasViolation(next, constraints)) return next;
    if (unchanged(positions, next)) {
      // Stalled at a fixed point: every remaining violation is intra-block and
      // therefore invisible to the merge loop, so re-solving cannot escape it.
      // Fall back to a construction that is feasible by definition.
      return repairByLongestPath(next, weights, constraints);
    }
    positions = next;
  }
  return repairByLongestPath(positions, weights, constraints);
}

function unchanged(before: ReadonlyMap<string, number>, after: ReadonlyMap<string, number>): boolean {
  for (const [key, value] of before) {
    const next = after.get(key);
    if (next === undefined || Math.abs(next - value) > 1e-9) return false;
  }
  return true;
}

/**
 * Guaranteed-feasible fallback: longest-path assignment over the constraint DAG.
 *
 * Processes variables in topological order, pushing each to the furthest
 * position any incoming constraint demands. Every constraint is satisfied by
 * construction. It only ever moves variables in the positive direction, so it
 * is more spread out than the optimum — acceptable for a path taken only when
 * the block solver stalls.
 *
 * Constraint directions are assigned from positions and never revised, so the
 * graph is acyclic in practice; a cycle would mean two units each required to
 * be left of the other. Any edge closing a cycle is dropped rather than looping
 * forever, and pinned variables are held fixed, which can leave a violation in
 * a genuinely over-constrained arrangement. Both cases surface to the caller as
 * a residual overlap rather than as silently wrong geometry.
 */
function repairByLongestPath(
  positions: ReadonlyMap<string, number>,
  weights: ReadonlyMap<string, number>,
  constraints: readonly SeparationConstraint[],
): Map<string, number> {
  const result = new Map(positions);
  const outgoing = new Map<string, Array<{ right: string; gap: number }>>();
  const inDegree = new Map<string, number>();
  for (const key of result.keys()) inDegree.set(key, 0);

  for (const constraint of constraints) {
    if (!result.has(constraint.left) || !result.has(constraint.right)) continue;
    if (constraint.left === constraint.right) continue;
    const edges = outgoing.get(constraint.left) ?? [];
    edges.push({ right: constraint.right, gap: constraint.gap });
    outgoing.set(constraint.left, edges);
    inDegree.set(constraint.right, (inDegree.get(constraint.right) ?? 0) + 1);
  }

  // Kahn's algorithm, seeded in sorted order so the result is deterministic.
  const ready = [...inDegree.entries()]
    .filter(([, degree]) => degree === 0)
    .map(([key]) => key)
    .sort();
  const order: string[] = [];
  while (ready.length > 0) {
    const key = ready.shift()!;
    order.push(key);
    for (const edge of outgoing.get(key) ?? []) {
      const remaining = (inDegree.get(edge.right) ?? 0) - 1;
      inDegree.set(edge.right, remaining);
      if (remaining === 0) {
        ready.push(edge.right);
        ready.sort();
      }
    }
  }

  const reachable = new Set(order);
  for (const key of order) {
    for (const edge of outgoing.get(key) ?? []) {
      if (!reachable.has(edge.right)) continue; // edge closes a cycle; dropped
      if (!Number.isFinite(weights.get(edge.right) ?? 1)) continue; // pinned
      const required = result.get(key)! + edge.gap;
      if (result.get(edge.right)! < required) result.set(edge.right, required);
    }
  }
  return result;
}

function hasViolation(
  positions: ReadonlyMap<string, number>,
  constraints: readonly SeparationConstraint[],
): boolean {
  for (const constraint of constraints) {
    const left = positions.get(constraint.left);
    const right = positions.get(constraint.right);
    if (left === undefined || right === undefined) continue;
    if (right - left < constraint.gap - 1e-7) return true;
  }
  return false;
}

/** One block-merging pass. Feasible with respect to inter-block constraints. */
function solveAxisOnce(
  desired: ReadonlyMap<string, number>,
  weights: ReadonlyMap<string, number>,
  constraints: readonly SeparationConstraint[],
): Map<string, number> {
  // Sorted so block-merge order — and therefore the result — does not depend on
  // Map insertion order.
  const keys = [...desired.keys()].sort();
  const index = new Map(keys.map((key, position) => [key, position]));

  const variables: Variable[] = keys.map((key, position) => ({
    key,
    desired: desired.get(key)!,
    weight: normalizeWeight(weights.get(key)),
    offset: 0,
    block: position,
  }));
  const blocks: Block[] = variables.map((variable, position) => ({
    variables: [position],
    position: variable.desired,
    weight: variable.weight,
    weightedPosition: variable.weight * variable.desired,
    alive: true,
  }));

  const active = constraints
    .map((constraint) => ({
      left: index.get(constraint.left),
      right: index.get(constraint.right),
      gap: constraint.gap,
    }))
    .filter(
      (constraint): constraint is { left: number; right: number; gap: number } =>
        constraint.left !== undefined &&
        constraint.right !== undefined &&
        constraint.left !== constraint.right,
    )
    // Deterministic processing order for equally violated constraints.
    .sort((l, r) => l.left - r.left || l.right - r.right || l.gap - r.gap);

  const violation = (constraint: { left: number; right: number; gap: number }) => {
    const left = variables[constraint.left];
    const right = variables[constraint.right];
    return (
      blocks[left.block].position +
      left.offset +
      constraint.gap -
      (blocks[right.block].position + right.offset)
    );
  };

  // Each iteration merges two blocks, so at most n - 1 iterations can occur.
  // The bound is a termination guarantee, not a heuristic cutoff.
  for (let iteration = 0; iteration < keys.length; iteration += 1) {
    let worst: { left: number; right: number; gap: number } | null = null;
    let worstViolation = 1e-9;
    for (const constraint of active) {
      const left = variables[constraint.left];
      const right = variables[constraint.right];
      if (left.block === right.block) continue; // already fixed inside a block
      const amount = violation(constraint);
      if (amount > worstViolation) {
        worstViolation = amount;
        worst = constraint;
      }
    }
    if (!worst) break;

    mergeBlocks(variables, blocks, worst);
  }

  const result = new Map<string, number>();
  for (const variable of variables) {
    result.set(variable.key, blocks[variable.block].position + variable.offset);
  }
  return result;
}

function mergeBlocks(
  variables: Variable[],
  blocks: Block[],
  constraint: { left: number; right: number; gap: number },
): void {
  const left = variables[constraint.left];
  const right = variables[constraint.right];
  const leftBlockIndex = left.block;
  const rightBlockIndex = right.block;
  const leftBlock = blocks[leftBlockIndex];
  const rightBlock = blocks[rightBlockIndex];

  // Offset of the right block's reference relative to the left block's, at the
  // point where the constraint is exactly satisfied.
  const distance = left.offset + constraint.gap - right.offset;

  for (const variableIndex of rightBlock.variables) {
    const variable = variables[variableIndex];
    variable.offset += distance;
    variable.block = leftBlockIndex;
    leftBlock.variables.push(variableIndex);
  }

  // Re-expressing the right block's variables against the left reference shifts
  // every desired-minus-offset term by `distance`.
  leftBlock.weightedPosition += rightBlock.weightedPosition - rightBlock.weight * distance;
  leftBlock.weight += rightBlock.weight;
  leftBlock.position = leftBlock.weightedPosition / leftBlock.weight;

  rightBlock.alive = false;
  rightBlock.variables = [];
}

function normalizeWeight(weight: number | undefined): number {
  if (weight === undefined) return 1;
  if (!Number.isFinite(weight)) return PINNED_WEIGHT; // pinned
  return Math.max(1e-6, weight);
}

// --- Two-dimensional overlap removal -------------------------------------

export interface RemoveOverlapsOptions {
  padding?: number;
  /** Skip the vertical pass. Useful for laying out a single row. */
  horizontalOnly?: boolean;
  /** Safety bound on refinement rounds. */
  maxRounds?: number;
}

export interface RemoveOverlapsResult {
  positions: Map<string, GardenPosition>;
  /** Pairs still overlapping when the round budget ran out. Normally empty. */
  residualOverlaps: Array<[string, string]>;
  rounds: number;
}

/**
 * Rounds of refinement before giving up and reporting residual overlap.
 *
 * More than one is required, and the reason is not obvious: a round constrains
 * only the pairs overlapping *at that moment*, so moving a unit can push it
 * onto a third that was previously clear and therefore unconstrained.
 *
 * Crucially, constraints **accumulate** across rounds rather than being
 * regenerated. Regenerating oscillates: a pair separated in round k is no
 * longer overlapping at the start of round k+1, so it is unconstrained, so the
 * solver is free to collapse it again while satisfying some newer constraint.
 * With an accumulating set every round satisfies every decision made so far,
 * the set grows monotonically, and termination follows. Two to four rounds in
 * practice.
 *
 * Accumulation also pins each constraint's *direction* to the layout order at
 * the moment the pair was first separated, which is what makes the result
 * order-preserving rather than merely non-overlapping.
 */
export const DEFAULT_MAX_ROUNDS = 8;

/**
 * Remove overlaps between unit footprints with minimal displacement.
 *
 * The first round splits each overlapping pair onto whichever axis needs the
 * smaller correction, so a column of units separates vertically instead of
 * being smeared into a row. Later rounds resolve vertically: by then the
 * horizontal arrangement carries the metric's information and should not be
 * disturbed further.
 */
export function removeOverlaps(
  units: readonly UnitBox[],
  options: RemoveOverlapsOptions = {},
): RemoveOverlapsResult {
  const padding = options.padding ?? DEFAULT_UNIT_PADDING;
  const maxRounds = options.maxRounds ?? DEFAULT_MAX_ROUNDS;
  // Sorted so pair enumeration and constraint direction are deterministic.
  const sorted = [...units].sort((left, right) => left.key.localeCompare(right.key));
  if (sorted.length === 0) return { positions: new Map(), residualOverlaps: [], rounds: 0 };

  const weights = new Map(sorted.map((unit) => [unit.key, unit.weight ?? 1]));
  const current = new Map(sorted.map((unit) => [unit.key, { ...unit.position }]));
  const boxes = new Map(sorted.map((unit) => [unit.key, { width: unit.width, height: unit.height }]));

  const overlappingPairs = () => {
    const pairs: Array<{ a: string; b: string; requiredX: number; requiredY: number }> = [];
    for (let i = 0; i < sorted.length; i += 1) {
      for (let j = i + 1; j < sorted.length; j += 1) {
        const a = sorted[i].key;
        const b = sorted[j].key;
        const boxA = boxes.get(a)!;
        const boxB = boxes.get(b)!;
        const requiredX = (boxA.width + boxB.width) / 2 + padding;
        const requiredY = (boxA.height + boxB.height) / 2 + padding;
        const positionA = current.get(a)!;
        const positionB = current.get(b)!;
        if (Math.abs(positionA.x - positionB.x) >= requiredX - CLEARANCE_EPSILON) continue;
        if (Math.abs(positionA.y - positionB.y) >= requiredY - CLEARANCE_EPSILON) continue;
        pairs.push({ a, b, requiredX, requiredY });
      }
    }
    return pairs;
  };

  const solveOn = (axis: "x" | "y", constraints: readonly SeparationConstraint[]) => {
    if (constraints.length === 0) return;
    const solved = solveAxis(
      new Map([...current].map(([key, position]) => [key, position[axis]])),
      weights,
      constraints,
    );
    for (const [key, value] of solved) current.get(key)![axis] = value;
  };

  // Constraint direction comes from a total order fixed once, from the incoming
  // layout — never from positions as they move.
  //
  // Orienting by current position looks natural and is wrong: a constraint added
  // in a later round can contradict one added earlier (A left-of B, then B
  // left-of A), and the accumulated set stops being a DAG. Those cycles cannot
  // be satisfied at all, which is exactly how a handful of pairs survive every
  // round untouched. Ranking once also *is* the order-preservation guarantee:
  // the output preserves the order the metric produced, not some intermediate
  // shuffling.
  const rank = { x: rankBy(sorted, "x"), y: rankBy(sorted, "y") };
  const constraintFor = (a: string, b: string, axis: "x" | "y", gap: number) =>
    rank[axis].get(a)! < rank[axis].get(b)!
      ? { left: a, right: b, gap }
      : { left: b, right: a, gap };

  // Accumulating constraint sets, keyed by pair so a pair is assigned an axis
  // exactly once and never reassigned.
  const assigned = new Map<string, "x" | "y">();
  const horizontal: SeparationConstraint[] = [];
  const vertical: SeparationConstraint[] = [];

  let rounds = 0;
  let pairs = overlappingPairs();
  while (pairs.length > 0 && rounds < maxRounds) {
    for (const pair of pairs) {
      const pairKey = `${pair.a} ${pair.b}`;
      if (assigned.has(pairKey)) continue;
      if (options.horizontalOnly) {
        assigned.set(pairKey, "x");
        horizontal.push(constraintFor(pair.a, pair.b, "x", pair.requiredX));
        continue;
      }
      // Move along whichever axis needs less correction, so a column of units
      // separates vertically instead of being smeared into a row.
      const positionA = current.get(pair.a)!;
      const positionB = current.get(pair.b)!;
      const costX = pair.requiredX - Math.abs(positionA.x - positionB.x);
      const costY = pair.requiredY - Math.abs(positionA.y - positionB.y);
      if (costX <= costY) {
        assigned.set(pairKey, "x");
        horizontal.push(constraintFor(pair.a, pair.b, "x", pair.requiredX));
      } else {
        assigned.set(pairKey, "y");
        vertical.push(constraintFor(pair.a, pair.b, "y", pair.requiredY));
      }
    }

    // Solving one axis cannot disturb the other, so separations established in
    // earlier rounds survive.
    solveOn("x", horizontal);
    solveOn("y", vertical);

    rounds += 1;
    pairs = overlappingPairs();
  }

  return {
    positions: current,
    residualOverlaps: pairs.map((pair) => [pair.a, pair.b] as [string, string]),
    rounds,
  };
}

/**
 * Rank units along one axis of the incoming layout. Ties break on key so
 * coincident units get a stable, reproducible order rather than one that
 * depends on floating-point noise.
 */
function rankBy(units: readonly UnitBox[], axis: "x" | "y"): Map<string, number> {
  const ordered = [...units].sort(
    (left, right) =>
      left.position[axis] - right.position[axis] || left.key.localeCompare(right.key),
  );
  return new Map(ordered.map((unit, position) => [unit.key, position]));
}

/**
 * True when two footprints overlap, padding included.
 *
 * @internal Test support, no production caller: layout tests check the no-overlap invariant with it.
 */
export function overlaps(a: UnitBox, b: UnitBox, padding = DEFAULT_UNIT_PADDING): boolean {
  return (
    Math.abs(a.position.x - b.position.x) <
      (a.width + b.width) / 2 + padding - CLEARANCE_EPSILON &&
    Math.abs(a.position.y - b.position.y) <
      (a.height + b.height) / 2 + padding - CLEARANCE_EPSILON
  );
}

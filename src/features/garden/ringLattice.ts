/**
 * Concentric ring lattice: the frame districts are arranged in.
 *
 * ## Why rings rather than a grid
 *
 * A grid has no centre. Every cell is equivalent, so nothing on the map can say
 * "this is the place everything else is organized around" — and the Garden does
 * have such a place: the commons, where shared automations and unaffiliated
 * entities live. On a grid the commons was wherever the curve happened to put
 * it, usually a corner, which reads as *peripheral* — the precise opposite of
 * what it is.
 *
 * Rings make centrality expressible. Slot 0 is the origin and belongs to the
 * commons; everything else sits in a ring around it, and a district's ring is a
 * legible statement about how far it stands from the shared centre.
 *
 * ## Why six slots per ring
 *
 * Ring `r` sits at radius `r * spacing` and holds `6r` slots. That number is not
 * arbitrary: the arc between neighbours in ring `r` is
 *
 *     2π(r · spacing) / 6r  =  π · spacing / 3  ≈  1.047 · spacing
 *
 * which is independent of `r` and almost exactly the radial gap between rings.
 * So neighbours are equidistant in every direction, at every radius — a
 * hexagonal packing in polar clothing. One pitch controls the whole map, exactly
 * as it did on the grid, and `spacingFor` needs no adjustment.
 *
 * Rings are unbounded. The grid was a fixed 2^order square that could in
 * principle fill up, with a fallback that parked a district on top of the
 * commons; there is no such case here.
 */

/** Slots in a ring grow by this many per ring; see the module note. */
const SLOTS_PER_RING_STEP = 6;

/** The origin slot. Reserved for the district everything else is arranged around. */
export const CENTER_SLOT = 0;

/**
 * Ring extents are rounded up to a multiple of this before radii are derived.
 *
 * A ring's radius depends on its widest district, so without a step the map
 * would breathe continuously: one unit settling a pixel further out would slide
 * every ring outside it. The stability contract forbids that.
 */
export const RING_EXTENT_QUANTUM = 60;

/**
 * Bumped when the slot-to-position mapping changes.
 *
 * Slot indices are persisted, so their *meaning* is part of the stored format.
 * A stale arrangement is discarded and re-placed rather than reinterpreted —
 * reading a Hilbert index as a ring slot would scatter districts to positions
 * that never meant anything.
 */
export const RING_ARRANGEMENT = 1;

/** Slots in ring `r`. Ring 0 is the single centre slot. */
export function slotsInRing(ring: number): number {
  return ring <= 0 ? 1 : SLOTS_PER_RING_STEP * ring;
}

/**
 * Index of the first slot in ring `r`.
 *
 * Ring 0 holds one slot, and rings 1..r-1 hold 6k each, so the offset is
 * 1 + 6·(1 + 2 + … + (r−1)) = 1 + 3r(r−1).
 */
export function firstSlotOfRing(ring: number): number {
  return ring <= 0 ? 0 : 1 + 3 * ring * (ring - 1);
}

export interface RingSlot {
  ring: number;
  /** Position within the ring, counter-clockwise from the positive x axis. */
  position: number;
}

/** Slot index to its ring and position. Inverse of `slotIndex`. */
export function ringSlotOf(index: number): RingSlot {
  const slot = Math.max(0, Math.floor(index));
  if (slot === 0) return { ring: 0, position: 0 };
  // Invert 1 + 3r(r-1) <= slot, i.e. the largest r with 3r² - 3r + 1 <= slot.
  const ring = Math.max(1, Math.floor((3 + Math.sqrt(12 * slot - 3)) / 6));
  return { ring, position: slot - firstSlotOfRing(ring) };
}

/**
 * Ring and position back to a slot index. Inverse of `ringSlotOf`.
 *
 * @internal Test support, no production caller: tests check the lattice round-trips.
 */
export function slotIndex(slot: RingSlot): number {
  return firstSlotOfRing(slot.ring) + slot.position;
}

/**
 * Where a slot sits.
 *
 * `radii` gives each ring its own radius; without it, ring `r` falls back to
 * `r * spacing` — a uniform pitch, which is only correct when every district is
 * about the same size.
 *
 * Rings are rotated by half a step on odd rings so a district is not always
 * directly behind the one inside it, which would leave visible spokes of empty
 * space running out from the centre.
 */
export function slotPoint(
  index: number,
  spacing = 1,
  radii?: readonly number[],
): { x: number; y: number } {
  const { ring, position } = ringSlotOf(index);
  if (ring === 0) return { x: 0, y: 0 };
  const count = slotsInRing(ring);
  const stagger = ring % 2 === 0 ? 0 : 0.5;
  const angle = (2 * Math.PI * (position + stagger)) / count;
  const radius = radii?.[ring] ?? ring * spacing;
  return { x: Math.cos(angle) * radius, y: Math.sin(angle) * radius };
}

/**
 * Radius of each ring, sized to the districts it actually holds.
 *
 * A single pitch has to satisfy the largest district anywhere on the map, which
 * is fine when districts are uniform and wasteful when they are not: a roster
 * with one busy project and thirty one-agent workspaces gave every one of those
 * singletons a cell scaled for the busiest, and the map read as empty.
 *
 * Two constraints decide each radius, and the larger wins:
 *
 *   - **Radial.** Ring `r` must clear the ring inside it — half of each ring's
 *     widest district, plus a margin.
 *   - **Angular.** Neighbours within a ring sit on a chord of
 *     `2·R·sin(π / 6r)`, which must also clear their widths. This is what stops
 *     an inner ring from being packed too tightly to hold what is in it.
 *
 * Radii are recomputed every pass rather than persisted. They are a pure
 * function of which districts are where and how big they are, and deriving them
 * fresh is what keeps stored geometry from feeding back into the frame it is
 * later read in.
 *
 * Extents are quantized before they are used. Without that, every ring's radius
 * is a continuous function of its widest district, so a single unit settling a
 * pixel further out slides every ring outside it — and the stability contract
 * says adding or removing one entity must not move the others. Quantizing makes
 * a ring move only when its contents change by a visible amount, and then by a
 * definite step.
 */
export function ringRadii(
  extentBySlot: ReadonlyMap<number, number>,
  margin = 0,
  quantum = RING_EXTENT_QUANTUM,
): number[] {
  const extentByRing: number[] = [];
  for (const [slot, extent] of extentBySlot) {
    const { ring } = ringSlotOf(slot);
    extentByRing[ring] = Math.max(extentByRing[ring] ?? 0, extent);
  }
  for (let ring = 0; ring < extentByRing.length; ring += 1) {
    const extent = extentByRing[ring];
    if (extent !== undefined) extentByRing[ring] = Math.ceil(extent / quantum) * quantum;
  }

  const radii: number[] = [0];
  for (let ring = 1; ring < extentByRing.length; ring += 1) {
    const here = extentByRing[ring] ?? 0;
    const inside = extentByRing[ring - 1] ?? 0;
    const radial = radii[ring - 1] + inside + here + margin;
    // Half the chord between neighbours must clear half a district.
    const angular = (here + margin / 2) / Math.sin(Math.PI / slotsInRing(ring));
    radii[ring] = Math.max(radial, angular);
  }
  return radii;
}

/**
 * Distance between two slots, in pitches.
 *
 * Straight-line distance between the slot positions rather than a difference of
 * ring indices: placement cares where districts end up on screen, and two slots
 * in the same ring can be adjacent or diametrically opposite.
 */
export function slotDistance(a: number, b: number): number {
  const left = slotPoint(a);
  const right = slotPoint(b);
  return Math.hypot(left.x - right.x, left.y - right.y);
}

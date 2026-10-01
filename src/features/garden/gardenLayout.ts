/**
 * Garden layout pipeline: entities in, placed units out.
 *
 *   facets -> IDF corpus -> districts -> per-district k-NN -> SMACOF -> VPSC
 *
 * ## The district is the unit of recomputation
 *
 * Every superlinear stage runs *inside* a district: the k-NN build is all-pairs
 * and stress majorization is iterative, so both depend on district size, which
 * `MAX_DISTRICT_MEMBERS` caps. Insertion therefore costs one district, not the
 * map. Nothing here is global except facet extraction and district placement,
 * which are linear and near-constant respectively.
 *
 * ## Telemetry is not an input
 *
 * `LayoutInput` accepts no status, no telemetry, no lens state. That is enforced
 * by the type rather than by discipline, because the invariant it protects is
 * the whole design: status changes, message ticks, lens toggles, and window
 * resizes must change colour, tint, and emphasis — never geometry. A map whose
 * shape shifts when a filter toggles is asserting something it cannot back, and
 * `docs/specs/2026-07-14-entity-oriented-agent-semantics.md` is explicit that
 * Garden spatial proximity is interpretive and never a claim about system state.
 */

import type { GardenPosition } from "./garden.types";
import { entityKey, type EntityRef } from "./entityRef";
import {
  buildCorpus,
  withPlacement,
  type FacetCorpus,
  type GardenEntityFacets,
} from "./facets";
import {
  COMMONS_DISTRICT_ID,
  DEFAULT_DISTRICT_SPACING,
  MAX_DISTRICT_MEMBERS,
  districtCenter,
  parcelsFor,
  placeDistricts,
  retireDistricts,
  spacingFor,
  type DistrictLayout,
  type DistrictReachTiers,
} from "./districts";
import { DISTRICT_GROUND_MARGIN, districtFootprint } from "./terrain";
import {
  DEFAULT_KNN,
  DEFAULT_METRIC_WEIGHTS,
  buildNeighbourGraph,
  symmetrizeGraph,
  type MetricContext,
  type MetricWeights,
  type PprMatrix,
} from "./metric";
import {
  DEFAULT_LAYOUT_SCALE,
  DEFAULT_MAX_ITERATIONS,
  runSmacof,
  type SmacofNode,
} from "./smacof";
import { removeOverlaps, type UnitBox } from "./vpsc";
import { ringRadii } from "./ringLattice";
import {
  anchoredDistrict,
  driftFor,
  recordPositions,
  resolvePin,
  sceneAnchorWeights,
  stalePins,
  warmStartAnchor,
  type GardenScene,
} from "./gardenScene";

export interface LayoutEntity {
  ref: EntityRef;
  facets: GardenEntityFacets;
  /** Canonical district for this entity, resolved by the caller. */
  districtId: string;
  /** Footprint used for overlap removal. */
  width: number;
  height: number;
}

export interface LayoutInput {
  entities: readonly LayoutEntity[];
  scene: GardenScene;
  /**
   * districtId -> reach tier, seating coordinating districts nearer the centre.
   *
   * Admissible here where telemetry is not, and the difference is worth being
   * precise about. A tier is derived from turn records — history that has
   * already happened — over the whole roster, not from live status and not from
   * what is currently on screen. It is discrete, it is read once per session,
   * and it moves a district only when that district gains a whole class of
   * collaborators. See `reachTier`.
   */
  reachTiers?: DistrictReachTiers;
  /** Interaction affinity, agent subgraph only. */
  ppr?: PprMatrix;
  ignoredPairs?: ReadonlySet<string>;
  suppressedSeedPairs?: ReadonlySet<string>;
  weights?: MetricWeights;
  scale?: number;
  knn?: number;
  maxIterations?: number;
  now?: number;
}

export interface PlacedUnit {
  ref: EntityRef;
  key: string;
  districtId: string;
  parcelId: string;
  position: GardenPosition;
  pinned: boolean;
}

export interface LayoutResult {
  units: PlacedUnit[];
  /** Scene with refreshed district cells and warm-start positions. */
  scene: GardenScene;
  /** Entities whose pin no longer matches their district. */
  stalePinKeys: string[];
  /** Pairs still overlapping after constraint solving. Normally empty. */
  residualOverlaps: Array<[string, string]>;
  corpus: FacetCorpus;
  districts: DistrictLayout;
  /**
   * World-space origin per district. Callers need this to convert a drag into a
   * district-relative pin, and it is the same origin pins resolve against.
   */
  districtOrigins: Map<string, GardenPosition>;
  /**
   * How far each district reaches from its origin, measured from the units the
   * metric placed there.
   *
   * Published so a drag can be held inside the district it belongs to. A drop
   * beyond this is not a placement the map can honour: it would put the unit in
   * a neighbour's territory, where it reads as a member of a district it is not
   * in.
   */
  districtExtents: Map<string, number>;
}

/**
 * Run the whole pipeline.
 *
 * Deterministic: given the same entities and scene, the output is identical.
 * There is no `Math.random`, no `Date.now` outside the explicit `now`, and every
 * stage sorts its inputs.
 */
export function layoutGarden(input: LayoutInput): LayoutResult {
  const now = input.now ?? Date.now();
  const scale = input.scale ?? DEFAULT_LAYOUT_SCALE;

  // District membership drives placement facets, so resolve it before facets.
  const districtOf = new Map<string, string>(
    input.entities.map((entity) => [entityKey(entity.ref), entity.districtId]),
  );

  // Fold the user's placements into the facet sets. Anchors join the cosine
  // vector; exclusions stay out of it and act as repulsion in the metric.
  const placed = input.entities.map((entity) => {
    const key = entityKey(entity.ref);
    return {
      ...entity,
      facets: withPlacement(entity.facets, {
        anchoredDistrictId: anchoredDistrict(input.scene, key),
        excludedDistrictIds: input.scene.exclusions[key],
      }),
    };
  });

  const corpus = buildCorpus(placed.map((entity) => entity.facets));
  const anchorWeights = sceneAnchorWeights(input.scene, now);

  const context: MetricContext = {
    corpus,
    weights: input.weights ?? DEFAULT_METRIC_WEIGHTS,
    sceneWeights: anchorWeights,
    ppr: input.ppr,
    ignoredPairs: input.ignoredPairs,
    suppressedSeedPairs: input.suppressedSeedPairs,
    districtOf,
  };

  // --- District cells -----------------------------------------------------
  const byDistrict = new Map<string, typeof placed>();
  for (const entity of placed) {
    const members = byDistrict.get(entity.districtId);
    if (members) members.push(entity);
    else byDistrict.set(entity.districtId, [entity]);
  }
  const activeDistricts = [...byDistrict.keys()].sort();

  const placement = placeDistricts(
    input.scene.districts,
    activeDistricts,
    districtSimilarity(byDistrict, context),
    input.reachTiers,
  );
  const districts = retireDistricts(placement.layout, activeDistricts, now);

  // --- Per-district layout ------------------------------------------------
  //
  // Solved in each district's *own* frame, centred on (0, 0), then translated
  // onto the grid. Solving in world coordinates would fix the grid pitch before
  // anything is known about how much room a district needs, which is precisely
  // how districts came to overlap: the pitch was a constant, and overlap
  // removal — which runs per parcel — has no concept of a cell boundary.
  // Measuring first lets the pitch be derived rather than hoped for.
  const units: PlacedUnit[] = [];
  const residualOverlaps: Array<[string, string]> = [];

  const localById = new Map<string, Map<string, GardenPosition>>();
  const parcelOfKey = new Map<string, string>();
  // Half-width each district needs about its own centre, footprints included.
  // Kept per district rather than collapsed to a maximum, because ring radii are
  // derived from what each ring actually contains: a ring of one-agent districts
  // has no business being as wide as the ring holding the commons.
  const extentByDistrict = new Map<string, number>();

  for (const districtId of activeDistricts) {
    const members = byDistrict.get(districtId)!;
    const local = new Map<string, GardenPosition>();
    localById.set(districtId, local);

    // Oversized districts split into parcels so k-NN and SMACOF stay bounded.
    const parcels = parcelsFor(
      districtId,
      members.map((entity) => entityKey(entity.ref)),
      MAX_DISTRICT_MEMBERS,
    );
    const memberByKey = new Map(members.map((entity) => [entityKey(entity.ref), entity]));

    let parcelIndex = 0;
    for (const [parcelId, memberKeys] of parcels) {
      const parcelMembers = memberKeys
        .map((key) => memberByKey.get(key))
        .filter((entity): entity is (typeof placed)[number] => entity !== undefined);
      if (parcelMembers.length === 0) continue;

      // Parcels of one district fan out around its centre so they do not stack.
      // The ring radius is a constant rather than a fraction of the pitch: the
      // pitch is derived from the extent this contributes to, and reading it
      // here would make that circular.
      const parcelOrigin = offsetForParcel(ORIGIN, parcelIndex);
      parcelIndex += 1;

      const graph = symmetrizeGraph(
        buildNeighbourGraph(
          parcelMembers.map((entity) => entity.facets),
          context,
          input.knn ?? DEFAULT_KNN,
        ),
      );

      const nodes: SmacofNode[] = parcelMembers.map((entity) => {
        const key = entityKey(entity.ref);
        // Pins resolve against the *district* origin, never the parcel origin.
        // Parcels are an internal subdivision that exists to bound computation,
        // and their membership shifts as entities come and go — anchoring a
        // user's placement to one would silently relocate it whenever the split
        // changed. In the local frame that origin is (0, 0), so a pin resolves
        // to exactly the district-relative offset it is stored as.
        const pinnedAt = resolvePin(input.scene, key, districtId, ORIGIN);
        return {
          key,
          rho: driftFor(input.scene, key, now),
          anchor: pinnedAt ?? warmStartAnchor(input.scene, key, districtId),
        };
      });

      const solved = runSmacof(
        { nodes, graph, center: parcelOrigin, scale },
        input.maxIterations ?? DEFAULT_MAX_ITERATIONS,
      );

      const boxes: UnitBox[] = parcelMembers.map((entity) => {
        const key = entityKey(entity.ref);
        return {
          key,
          position: solved.positions.get(key) ?? parcelOrigin,
          width: entity.width,
          height: entity.height,
          // A pinned unit is immovable here too: constraints route around it
          // rather than sliding it off the spot the user chose.
          weight: input.scene.pins[key] ? Infinity : 1,
        };
      });
      const cleared = removeOverlaps(boxes);
      residualOverlaps.push(...cleared.residualOverlaps);

      for (const entity of parcelMembers) {
        const key = entityKey(entity.ref);
        const position = cleared.positions.get(key) ?? parcelOrigin;
        local.set(key, position);
        parcelOfKey.set(key, parcelId);
        // How far the district reaches from its own centre, footprints included,
        // so neighbours are sized against what is actually drawn.
        //
        // Pinned units are excluded, and the reason is worth stating precisely
        // because letting them count was tried and was worse.
        //
        // A pin sets an offset that can be arbitrarily large — a user drags a
        // unit wherever they like. Feeding that into the extent makes the ring
        // radius grow to match, which moves every district in and outside that
        // ring, which moves this district's own origin. The pin is an offset
        // from that origin, so the unit travels with it: dropping a unit on a
        // neighbouring district put it 600 units from where it was released, and
        // inflated the whole map by 2.3x in a single drag. Both were reported.
        //
        // Districts are therefore sized by what the *metric* placed in them, and
        // a drag that would land outside its district is clamped to it instead
        // (see `districtExtents`). The district holds what was dragged by
        // keeping it, not by chasing it.
        if (input.scene.pins[key]) continue;
        extentByDistrict.set(
          districtId,
          Math.max(
            extentByDistrict.get(districtId) ?? 0,
            Math.abs(position.x) + entity.width / 2,
            Math.abs(position.y) + entity.height / 2,
          ),
        );
      }
    }
  }

  // Rings are sized to what they hold. A single global pitch had to satisfy the
  // largest district on the map, so thirty-odd one-agent districts each sat
  // alone in a cell scaled for the commons, and the result read as empty. Each
  // ring now only has to clear its own widest member and the ring inside it.
  //
  // Measured as the *drawn* footprint rather than the raw unit extent: a
  // district's ground is what occupies its slot, and sizing slots against the
  // units alone left the ground to fit into whatever was left over. See
  // `districtFootprint`.
  const radii = ringRadii(
    new Map(
      activeDistricts.map((id) => [
        cellFor(districts, id),
        districtFootprint(extentByDistrict.get(id) ?? 0),
      ]),
    ),
    DISTRICT_GROUND_MARGIN,
  );
  const spacedDistricts: DistrictLayout = {
    ...districts,
    spacing: spacingFor(2 * Math.max(0, ...extentByDistrict.values()), districts.spacing),
  };

  // --- Translate onto the lattice -----------------------------------------
  const settled = new Map<string, GardenPosition>();
  const districtOrigins = new Map<string, GardenPosition>();

  for (const districtId of activeDistricts) {
    const origin = districtCenter(cellFor(spacedDistricts, districtId), { radii });
    districtOrigins.set(districtId, origin);

    for (const entity of byDistrict.get(districtId)!) {
      const key = entityKey(entity.ref);
      const point = localById.get(districtId)?.get(key);
      if (!point) continue;
      // Stored district-relative. An absolute coordinate means nothing without
      // the origin it was measured from, and reconstructing that origin later —
      // from a pitch that had itself been derived from stored positions — is
      // exactly the loop that once inflated the map past drawing.
      settled.set(key, point);
      units.push({
        ref: entity.ref,
        key,
        districtId,
        parcelId: parcelOfKey.get(key) ?? districtId,
        position: { x: origin.x + point.x, y: origin.y + point.y },
        pinned: Boolean(input.scene.pins[key]),
      });
    }
  }

  units.sort((left, right) => left.key.localeCompare(right.key));

  return {
    units,
    scene: recordPositions({ ...input.scene, districts: spacedDistricts }, settled, districtOf),
    stalePinKeys: stalePins(input.scene, districtOf),
    residualOverlaps,
    corpus,
    districts: spacedDistricts,
    districtOrigins,
    districtExtents: extentByDistrict,
  };
}

const ORIGIN: GardenPosition = { x: 0, y: 0 };

/**
 * Cell for a district, falling back to the commons cell when the grid is full.
 * Parking a district on top of the commons is worse than a clean placement, and
 * far better than dropping its members off the map entirely.
 */
function cellFor(districts: DistrictLayout, districtId: string): number {
  const cell = districts.cells[districtId];
  if (cell !== undefined) return cell;
  return districts.cells[COMMONS_DISTRICT_ID] ?? 0;
}

/**
 * Similarity between two districts, from their members' shared facets.
 *
 * Used only to seat a *new* district beside the ones it most resembles, so a
 * cheap Jaccard over the districts' pooled facet sets is enough; the expensive
 * metric is reserved for placing entities within a district.
 */
function districtSimilarity(
  byDistrict: ReadonlyMap<string, ReadonlyArray<{ facets: GardenEntityFacets }>>,
  context: MetricContext,
): (a: string, b: string) => number {
  const pooled = new Map<string, Set<string>>();
  for (const [districtId, members] of byDistrict) {
    const tokens = new Set<string>();
    for (const member of members) {
      for (const token of member.facets.tokens) {
        // Universal facets carry no information and would make every district
        // look alike.
        if ((context.corpus.df.get(token) ?? 0) >= context.corpus.entityCount) continue;
        tokens.add(token);
      }
    }
    pooled.set(districtId, tokens);
  }

  return (a, b) => {
    const left = pooled.get(a);
    const right = pooled.get(b);
    if (!left || !right || left.size === 0 || right.size === 0) return 0;
    let shared = 0;
    const [shorter, longer] = left.size <= right.size ? [left, right] : [right, left];
    for (const token of shorter) if (longer.has(token)) shared += 1;
    return shared / (left.size + right.size - shared);
  };
}

/**
 * Fan parcels around their district origin on a ring, so a split district reads
 * as one place with several parts rather than as unrelated clusters.
 */
function offsetForParcel(origin: GardenPosition, index: number): GardenPosition {
  if (index === 0) return origin;
  const angle = (Math.PI * 2 * (index - 1)) / 6;
  const radius = DEFAULT_DISTRICT_SPACING * 0.32;
  return { x: origin.x + Math.cos(angle) * radius, y: origin.y + Math.sin(angle) * radius };
}

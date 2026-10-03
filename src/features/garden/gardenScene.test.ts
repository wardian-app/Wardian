import { describe, expect, it } from "vitest";
import { METRIC_VERSION } from "./metric";
import { DRIFT_NEW, DRIFT_PINNED, DRIFT_SETTLED, DRIFT_VISITED } from "./smacof";
import { SCENE_ANCHOR_MAX_WEIGHT, sceneAnchorToken } from "./facets";
import { RING_ARRANGEMENT } from "./ringLattice";
import {
  GARDEN_SCENE_SCHEMA,
  anchoredDistrict,
  authoritativeLiveKeys,
  createScene,
  driftFor,
  excludeFromDistrict,
  markVisited,
  pinEntity,
  pruneScene,
  recordPositions,
  resolvePin,
  reviveScene,
  sceneAnchorWeights,
  stalePins,
  unpinEntity,
} from "./gardenScene";

const now = 1_700_000_000_000;

describe("pins", () => {
  it("stores placement relative to the district, so a district move carries it", () => {
    // Absolute pins rot: if the district's cell shifts, the entity is stranded
    // in the wrong neighbourhood and the map lies about affiliation.
    const scene = pinEntity(
      createScene(),
      "agent:a1",
      "team:hw",
      { x: 640, y: 300 },
      { x: 600, y: 250 },
      now,
    );
    expect(scene.pins["agent:a1"]).toMatchObject({ district_id: "team:hw", dx: 40, dy: 50 });

    // Same district, relocated origin: the pin follows.
    expect(resolvePin(scene, "agent:a1", "team:hw", { x: 1000, y: 1000 })).toEqual({
      x: 1040,
      y: 1050,
    });
  });

  it("invalidates a pin when the entity changes district", () => {
    // Honouring it would misplace the entity; dropping it silently would
    // destroy the user's work. The caller surfaces a re-place prompt.
    const scene = pinEntity(
      createScene(),
      "agent:a1",
      "team:hw",
      { x: 640, y: 300 },
      { x: 600, y: 250 },
      now,
    );
    expect(resolvePin(scene, "agent:a1", "team:web", { x: 600, y: 250 })).toBeNull();
    expect(stalePins(scene, new Map([["agent:a1", "team:web"]]))).toEqual(["agent:a1"]);
    expect(stalePins(scene, new Map([["agent:a1", "team:hw"]]))).toEqual([]);
  });

  it("unpins without disturbing other pins", () => {
    let scene = pinEntity(createScene(), "agent:a1", "d1", { x: 1, y: 1 }, { x: 0, y: 0 }, now);
    scene = pinEntity(scene, "agent:a2", "d1", { x: 2, y: 2 }, { x: 0, y: 0 }, now);
    scene = unpinEntity(scene, "agent:a1");
    expect(scene.pins["agent:a1"]).toBeUndefined();
    expect(scene.pins["agent:a2"]).toBeDefined();
    expect(unpinEntity(scene, "missing")).toBe(scene);
  });

  it("reports the anchored district for facet emission", () => {
    const scene = pinEntity(createScene(), "agent:a1", "team:hw", { x: 0, y: 0 }, { x: 0, y: 0 });
    expect(anchoredDistrict(scene, "agent:a1")).toBe("team:hw");
    expect(anchoredDistrict(scene, "agent:a2")).toBeUndefined();
  });
});

describe("driftFor", () => {
  it("ranks authority as resistance to motion", () => {
    let scene = createScene();
    expect(driftFor(scene, "agent:new", now)).toBe(DRIFT_NEW);

    scene = recordPositions(scene, new Map([["agent:settled", { x: 0, y: 0 }]]));
    expect(driftFor(scene, "agent:settled", now)).toBe(DRIFT_SETTLED);

    scene = markVisited(scene, "agent:settled", now);
    expect(driftFor(scene, "agent:settled", now)).toBe(DRIFT_VISITED);

    scene = pinEntity(scene, "agent:settled", "d1", { x: 0, y: 0 }, { x: 0, y: 0 }, now);
    expect(driftFor(scene, "agent:settled", now)).toBe(DRIFT_PINNED);
  });

  it("lets a stale visit decay back to settled", () => {
    let scene = recordPositions(createScene(), new Map([["agent:a1", { x: 0, y: 0 }]]));
    scene = markVisited(scene, "agent:a1", now);
    expect(driftFor(scene, "agent:a1", now + 2 * 60 * 60 * 1000)).toBe(DRIFT_SETTLED);
  });
});

describe("sceneAnchorWeights", () => {
  it("caps a district's anchor weight rather than accumulating placements", () => {
    // Several entities pinned into one district must not compound into a
    // gravity well that swallows the map.
    let scene = createScene();
    for (let i = 0; i < 8; i += 1) {
      scene = pinEntity(scene, `agent:a${i}`, "team:hw", { x: 0, y: 0 }, { x: 0, y: 0 }, now);
    }
    const weights = sceneAnchorWeights(scene, now);
    expect(weights.get(sceneAnchorToken("team:hw"))).toBeCloseTo(SCENE_ANCHOR_MAX_WEIGHT, 6);
  });

  it("decays an abandoned placement", () => {
    const scene = pinEntity(createScene(), "agent:a1", "d1", { x: 0, y: 0 }, { x: 0, y: 0 }, now);
    const halfLife = 30 * 24 * 60 * 60 * 1000;
    expect(sceneAnchorWeights(scene, now + halfLife).get(sceneAnchorToken("d1"))).toBeCloseTo(
      SCENE_ANCHOR_MAX_WEIGHT / 2,
      6,
    );
  });
});

describe("exclusions", () => {
  it("records a rejected district once, sorted", () => {
    let scene = excludeFromDistrict(createScene(), "agent:a1", "team:web");
    scene = excludeFromDistrict(scene, "agent:a1", "team:hw");
    expect(scene.exclusions["agent:a1"]).toEqual(["team:hw", "team:web"]);
    expect(excludeFromDistrict(scene, "agent:a1", "team:hw")).toBe(scene);
  });
});

describe("reviveScene", () => {
  it("round-trips a serialized scene", () => {
    let scene = pinEntity(createScene(), "agent:a1", "d1", { x: 5, y: 6 }, { x: 0, y: 0 }, now);
    scene = recordPositions(scene, new Map([["agent:a1", { x: 5, y: 6 }]]));
    scene = excludeFromDistrict(scene, "agent:a1", "d2");

    const revived = reviveScene(JSON.parse(JSON.stringify(scene)));
    expect(revived.scene).toEqual(scene);
    expect(revived.needsRederive).toBe(false);
  });

  it("degrades a corrupt payload to a fresh scene instead of throwing", () => {
    // Matches how the backend treats a corrupt topology.json: failing to open
    // the map is worse than losing an arrangement.
    for (const payload of [null, undefined, 42, "nonsense", { schema: 99 }, []]) {
      const revived = reviveScene(payload);
      expect(revived.scene.schema).toBe(GARDEN_SCENE_SCHEMA);
      expect(revived.scene.pins).toEqual({});
    }
  });

  it("discards malformed entries but keeps valid siblings", () => {
    const revived = reviveScene({
      schema: GARDEN_SCENE_SCHEMA,
      metric_version: METRIC_VERSION,
      districts: {
        arrangement: RING_ARRANGEMENT,
        cells: { "team:a": 3, bad: "x" },
        tombstones: {},
      },
      pins: {
        good: { district_id: "d1", dx: 1, dy: 2, placed_at_ms: now },
        bad: { district_id: 7 },
      },
      exclusions: { good: ["d2"], bad: [1, 2] },
      positions: { good: { x: 1, y: 2 }, bad: { x: "nope", y: 0 } },
      visited: { good: now, bad: "soon" },
    });
    expect(Object.keys(revived.scene.pins)).toEqual(["good"]);
    expect(Object.keys(revived.scene.exclusions)).toEqual(["good"]);
    expect(Object.keys(revived.scene.positions)).toEqual(["good"]);
    expect(Object.keys(revived.scene.visited)).toEqual(["good"]);
    expect(revived.scene.districts.cells).toEqual({ "team:a": 3 });
  });

  it("re-places districts written under a different arrangement, keeping pins", () => {
    // A slot index only means something under the mapping that assigned it.
    // Reading Hilbert-grid indices as ring slots would not relocate districts so
    // much as scatter them to positions that never meant anything. Pins are
    // district-relative offsets and survive the move by construction — which is
    // the reason they are stored that way.
    const revived = reviveScene({
      schema: GARDEN_SCENE_SCHEMA,
      metric_version: METRIC_VERSION,
      districts: { order: 5, cells: { "team:a": 812, "team:b": 47 }, tombstones: {} },
      pins: { "agent:a1": { district_id: "team:a", dx: 30, dy: -40, placed_at_ms: now } },
    });
    expect(revived.scene.districts.cells).toEqual({});
    expect(revived.scene.districts.arrangement).toBe(RING_ARRANGEMENT);
    expect(revived.scene.pins["agent:a1"]).toEqual({
      district_id: "team:a",
      dx: 30,
      dy: -40,
      placed_at_ms: now,
    });
  });

  it("flags a metric version change for re-derivation instead of reflowing silently", () => {
    const stale = { ...createScene(), metric_version: METRIC_VERSION - 1 };
    const revived = reviveScene(JSON.parse(JSON.stringify(stale)));
    expect(revived.needsRederive).toBe(true);
  });

  it("keeps pins valid across a metric version change", () => {
    // Pins are district-relative offsets, not metric-derived coordinates, so
    // they survive by construction.
    const pinned = pinEntity(createScene(), "agent:a1", "d1", { x: 9, y: 9 }, { x: 0, y: 0 }, now);
    const stale = { ...pinned, metric_version: METRIC_VERSION - 1 };
    const revived = reviveScene(JSON.parse(JSON.stringify(stale)));
    expect(revived.needsRederive).toBe(true);
    expect(revived.scene.pins["agent:a1"]).toMatchObject({ dx: 9, dy: 9 });
  });
});

describe("pruning", () => {
  function sceneWithLiveAndDead() {
    let scene = pinEntity(createScene(), "agent:dead", "team:hw", { x: 3, y: 4 }, { x: 0, y: 0 }, now);
    scene = excludeFromDistrict(scene, "agent:dead", "team:web");
    scene = recordPositions(
      scene,
      new Map([
        ["agent:live", { x: 1, y: 2 }],
        ["agent:dead", { x: 5, y: 6 }],
      ]),
      new Map([
        ["agent:live", "team:hw"],
        ["agent:dead", "team:hw"],
      ]),
    );
    scene = markVisited(scene, "agent:live", now);
    return markVisited(scene, "agent:dead", now);
  }

  it("drops derived state for dead keys and leaves live keys untouched", () => {
    const scene = sceneWithLiveAndDead();
    const pruned = pruneScene(scene, new Set(["agent:live"]));

    expect(pruned.positions).toEqual({ "agent:live": { x: 1, y: 2 } });
    expect(pruned.position_districts).toEqual({ "agent:live": "team:hw" });
    expect(pruned.visited).toEqual({ "agent:live": now });
  });

  it("keeps pins and exclusions, which are user intent, by identity", () => {
    // A deleted agent may come back. Identity matters too: the layout depends
    // on pins and exclusions, so a new reference would lay the map out again.
    const scene = sceneWithLiveAndDead();
    const pruned = pruneScene(scene, new Set(["agent:live"]));

    expect(pruned.pins).toBe(scene.pins);
    expect(pruned.exclusions).toBe(scene.exclusions);
    expect(pruned.pins["agent:dead"]).toBeDefined();
    expect(pruned.exclusions["agent:dead"]).toEqual(["team:web"]);
    expect(pruned.districts).toBe(scene.districts);
  });

  it("is idempotent, and returns the same scene when nothing is stale", () => {
    const live = new Set(["agent:live"]);
    const once = pruneScene(sceneWithLiveAndDead(), live);
    expect(pruneScene(once, live)).toBe(once);
  });

  it("does not mutate the scene it was given", () => {
    const scene = sceneWithLiveAndDead();
    const before = JSON.parse(JSON.stringify(scene));
    pruneScene(scene, new Set(["agent:live"]));
    expect(scene).toEqual(before);
  });
});

describe("authoritative live keys", () => {
  const roster = { kind: "agent", status: "loaded" as const, ids: ["a1", "a2"], complete: true };

  it("keys every id of every loaded, complete source", () => {
    expect(authoritativeLiveKeys([
      roster,
      { kind: "automation", status: "loaded", ids: ["w1"], complete: true },
    ])).toEqual(new Set(["agent:a1", "agent:a2", "automation:w1"]));
  });

  it.each([
    ["unloaded", { ...roster, status: "unloaded" as const, ids: [] }],
    ["loading", { ...roster, status: "loading" as const }],
    ["failed", { ...roster, status: "failed" as const }],
    ["partial", { ...roster, complete: false }],
    ["empty", { ...roster, ids: [] }],
  ])("refuses a %s source", (_label, source) => {
    expect(authoritativeLiveKeys([source])).toBeNull();
  });

  it("refuses when any one source is not authoritative", () => {
    // Pruning against the keys of the sources that did load would treat every
    // entity of the one that did not as deleted.
    expect(authoritativeLiveKeys([
      roster,
      { kind: "automation", status: "loading", ids: [], complete: true },
    ])).toBeNull();
  });

  it("refuses when there are no sources at all", () => {
    expect(authoritativeLiveKeys([])).toBeNull();
  });
});

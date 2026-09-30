import { describe, expect, it } from "vitest";
import { agentRef, folderRef, libraryEntryRef } from "./entityRef";
import { buildCorpus, type GardenEntityFacets } from "./facets";
import {
  CROSS_KIND_OFFSET,
  DEFAULT_METRIC_WEIGHTS,
  EXCLUDE_REPULSION,
  IGNORE_REPULSION,
  MAX_PPR_NODES,
  buildNeighbourGraph,
  canonicalPairKey,
  crossKindOffset,
  distanceBetween,
  interactionDistance,
  interactionWeight,
  personalizedPageRank,
  symmetrizeGraph,
  type MetricContext,
} from "./metric";

function facets(ref: GardenEntityFacets["ref"], tokens: string[], excludes: string[] = []) {
  return { ref, tokens: [...tokens].sort(), excludes };
}

function contextFor(entities: GardenEntityFacets[], overrides: Partial<MetricContext> = {}) {
  return {
    corpus: buildCorpus(entities),
    weights: DEFAULT_METRIC_WEIGHTS,
    ...overrides,
  } satisfies MetricContext;
}

describe("personalizedPageRank", () => {
  it("discounts hub-mediated adjacency instead of collapsing the cluster", () => {
    // The failure this replaces: under shortest path every spoke is 2 hops from
    // every other spoke, so a degree-15 orchestrator fuses its whole
    // neighbourhood into a hairball.
    const spokes = Array.from({ length: 12 }, (_, i) => `s${i}`);
    const edges = spokes.map((spoke) => ({ source: "hub", target: spoke, weight: 1 }));
    // One spoke pair also talks directly.
    edges.push({ source: "s0", target: "s1", weight: 1 });

    const ppr = personalizedPageRank(["hub", ...spokes], edges);
    const direct = ppr.get("s0")!.get("s1")!;
    const viaHub = ppr.get("s0")!.get("s5")!;
    expect(direct).toBeGreaterThan(viaHub * 2);
  });

  it("rewards multiple independent paths", () => {
    const ppr = personalizedPageRank(
      ["a", "b", "m1", "m2"],
      [
        { source: "a", target: "m1", weight: 1 },
        { source: "m1", target: "b", weight: 1 },
        { source: "a", target: "m2", weight: 1 },
        { source: "m2", target: "b", weight: 1 },
        { source: "a", target: "m3", weight: 1 }, // dangling, ignored
      ],
    );
    const twoPaths = ppr.get("a")!.get("b")!;

    const single = personalizedPageRank(
      ["a", "b", "m1"],
      [
        { source: "a", target: "m1", weight: 1 },
        { source: "m1", target: "b", weight: 1 },
      ],
    );
    expect(twoPaths).toBeGreaterThan(single.get("a")!.get("b")!);
  });

  it("is deterministic under input reordering", () => {
    const edges = [
      { source: "a", target: "b", weight: 0.7 },
      { source: "b", target: "c", weight: 0.3 },
    ];
    const first = personalizedPageRank(["a", "b", "c"], edges);
    const second = personalizedPageRank(["c", "b", "a"], edges);
    expect(first.get("a")!.get("c")).toBeCloseTo(second.get("a")!.get("c")!, 12);
  });

  it("is asymmetric, which is why interactionDistance averages both directions", () => {
    // PPR is not symmetric even on an undirected graph: the walk depends on the
    // seed's degree. A layout needs a metric, so the asymmetry is resolved once
    // in interactionDistance rather than being papered over here.
    const ppr = personalizedPageRank(
      ["hub", "leaf", "x", "y"],
      [
        { source: "hub", target: "leaf", weight: 1 },
        { source: "hub", target: "x", weight: 1 },
        { source: "hub", target: "y", weight: 1 },
      ],
    );
    expect(ppr.get("leaf")!.get("hub")!).toBeGreaterThan(ppr.get("hub")!.get("leaf")!);
    expect(interactionDistance("leaf", "hub", ppr)).toBeCloseTo(
      interactionDistance("hub", "leaf", ppr)!,
      12,
    );
  });

  it("refuses inputs above the node cap instead of degrading into a frame-time cliff", () => {
    const ids = Array.from({ length: MAX_PPR_NODES + 1 }, (_, i) => `n${i}`);
    expect(personalizedPageRank(ids, []).size).toBe(0);
  });

  it("keeps each row a probability distribution when nodes dangle", () => {
    const ppr = personalizedPageRank(["a", "b", "isolated"], [
      { source: "a", target: "b", weight: 1 },
    ]);
    expect(ppr.get("isolated")!.size).toBe(0);
  });
});

describe("interactionDistance", () => {
  it("reports 1 for a reachable-graph pair with no affinity and null for non-agents", () => {
    const ppr = personalizedPageRank(["a", "b"], [{ source: "a", target: "b", weight: 1 }]);
    expect(interactionDistance("a", "b", ppr)).toBeLessThan(1);
    // Absent from the matrix entirely: term does not apply.
    expect(interactionDistance("folder:x", "folder:y", ppr)).toBeNull();
  });

  it("maps affinity onto [0, 1]", () => {
    const ppr = personalizedPageRank(["a", "b", "c"], [
      { source: "a", target: "b", weight: 1 },
      { source: "b", target: "c", weight: 1 },
    ]);
    const near = interactionDistance("a", "b", ppr)!;
    const far = interactionDistance("a", "c", ppr)!;
    expect(near).toBeGreaterThanOrEqual(0);
    expect(far).toBeLessThanOrEqual(1);
    expect(near).toBeLessThan(far);
  });
});

describe("interactionWeight", () => {
  it("keeps a durable manual edge dominant over volatile activity", () => {
    const manualOnly = interactionWeight({ manual: true, recency: 0 });
    const activityOnly = interactionWeight({ manual: false, recency: 1, kind: "Task" });
    expect(manualOnly).toBeGreaterThan(activityOnly);
  });

  it("ranks interaction kinds by how much work they imply", () => {
    const task = interactionWeight({ manual: false, recency: 1, kind: "Task" });
    const notification = interactionWeight({ manual: false, recency: 1, kind: "Notification" });
    expect(task).toBeGreaterThan(notification);
  });
});

describe("distanceBetween", () => {
  it("scores shared-team agents closer than unrelated ones", () => {
    const a1 = facets(agentRef("a1"), ["team:t1", "path:d:/dev/ward", "path:d:/"]);
    const a2 = facets(agentRef("a2"), ["team:t1", "path:d:/dev/ward", "path:d:/"]);
    const a3 = facets(agentRef("a3"), ["team:t2", "path:d:/elsewhere", "path:d:/"]);
    const context = contextFor([a1, a2, a3]);
    expect(distanceBetween(a1, a2, context).distance).toBeLessThan(
      distanceBetween(a1, a3, context).distance,
    );
  });

  it("renormalizes over applicable terms so folders are not penalized for silence", () => {
    // A folder has no communication history. Scoring d_interact = 1 for it would
    // push every folder pair uniformly apart for a reason unrelated to folders.
    const f1 = facets(folderRef("D:/dev/ward/src")!, ["path:d:/dev/ward/src", "path:d:/dev/ward"]);
    const f2 = facets(folderRef("D:/dev/ward/lib")!, ["path:d:/dev/ward/lib", "path:d:/dev/ward"]);
    const context = contextFor([f1, f2], {
      ppr: personalizedPageRank(["agent:a1", "agent:a2"], [
        { source: "agent:a1", target: "agent:a2", weight: 1 },
      ]),
    });
    const result = distanceBetween(f1, f2, context);
    expect(result.terms.map((term) => term.name)).toEqual(["affil"]);
  });

  it("adds a cross-kind offset so heterogeneous kinds form legible neighbourhoods", () => {
    const agentA = facets(agentRef("a1"), ["tag:shared"]);
    const agentB = facets(agentRef("a2"), ["tag:shared"]);
    const prompt = facets(libraryEntryRef("prompts/x.md")!, ["tag:shared"]);
    // Filler entities that lack the shared tag: without them the tag would be
    // universal, its IDF exactly zero, and every distance would collapse to the
    // cut. A degenerate corpus proves nothing about the offset.
    const filler = Array.from({ length: 7 }, (_, i) => facets(agentRef(`f${i}`), [`tag:other${i}`]));
    const context = contextFor([agentA, agentB, prompt, ...filler]);

    const sameKind = distanceBetween(agentA, agentB, context).distance;
    const crossKind = distanceBetween(agentA, prompt, context).distance;
    expect(Number.isFinite(sameKind)).toBe(true);
    expect(crossKind).toBeGreaterThan(sameKind);
    expect(crossKind - sameKind).toBeCloseTo(CROSS_KIND_OFFSET, 10);
  });

  it("gives naturally coupled kinds a lighter offset than incidental ones", () => {
    expect(crossKindOffset("agent", "artifact")).toBeLessThan(crossKindOffset("agent", "prompt"));
    expect(crossKindOffset("agent", "agent")).toBe(0);
  });

  it("pushes an ignored pair out of the neighbourhood", () => {
    // A map where deleting a link moves nothing teaches people to stop
    // correcting it.
    const a1 = facets(agentRef("a1"), ["team:t1", "path:d:/dev/ward"]);
    const a2 = facets(agentRef("a2"), ["team:t1", "path:d:/dev/ward"]);
    const plain = contextFor([a1, a2]);
    const ignored = contextFor([a1, a2], {
      ignoredPairs: new Set([canonicalPairKey("agent:a1", "agent:a2")]),
    });
    const before = distanceBetween(a1, a2, plain).distance;
    const after = distanceBetween(a1, a2, ignored);
    // Either it moved further by the full repulsion, or it crossed the cut.
    expect(after.offsets.some((offset) => offset.name === "ignored-pair")).toBe(true);
    expect(after.cut || after.distance - before >= IGNORE_REPULSION - 1e-9).toBe(true);
  });

  it("repels an entity from a district it was explicitly excluded from", () => {
    const a1 = facets(agentRef("a1"), ["team:t1"], ["district-b"]);
    const a2 = facets(agentRef("a2"), ["team:t1"]);
    const context = contextFor([a1, a2], {
      districtOf: new Map([
        ["agent:a1", "district-a"],
        ["agent:a2", "district-b"],
      ]),
    });
    const result = distanceBetween(a1, a2, context);
    expect(result.offsets).toEqual(
      expect.arrayContaining([{ name: "excluded-district", amount: EXCLUDE_REPULSION }]),
    );
  });

  it("cuts unrelated pairs to Infinity rather than a large finite number", () => {
    // Genuine Infinity is what keeps downstream stages linear.
    const a = facets(agentRef("a1"), ["team:t1"]);
    const b = facets(agentRef("a2"), ["team:t2"]);
    const result = distanceBetween(a, b, contextFor([a, b]));
    expect(result.cut).toBe(true);
    expect(result.distance).toBe(Infinity);
  });

  it("is symmetric", () => {
    const a = facets(agentRef("a1"), ["team:t1", "path:d:/dev/ward"]);
    const b = facets(agentRef("a2"), ["team:t1", "path:d:/dev/lib"]);
    const context = contextFor([a, b]);
    expect(distanceBetween(a, b, context).distance).toBeCloseTo(
      distanceBetween(b, a, context).distance,
      12,
    );
  });
});

describe("buildNeighbourGraph", () => {
  const entities = [
    facets(agentRef("a1"), ["team:t1", "path:d:/dev/ward"]),
    facets(agentRef("a2"), ["team:t1", "path:d:/dev/ward"]),
    facets(agentRef("a3"), ["team:t1", "path:d:/dev/ward"]),
    facets(agentRef("b1"), ["team:t2", "path:d:/other"]),
  ];

  it("keeps at most k neighbours, nearest first", () => {
    const graph = buildNeighbourGraph(entities, contextFor(entities), 2);
    const neighbours = graph.get("agent:a1")!;
    expect(neighbours.length).toBeLessThanOrEqual(2);
    for (let i = 1; i < neighbours.length; i += 1) {
      expect(neighbours[i - 1].distance).toBeLessThanOrEqual(neighbours[i].distance);
    }
  });

  it("omits cut pairs entirely", () => {
    const graph = buildNeighbourGraph(entities, contextFor(entities), 8);
    expect(graph.get("agent:a1")!.some((n) => n.key === "agent:b1")).toBe(false);
  });

  it("is deterministic under input reordering", () => {
    const forward = buildNeighbourGraph(entities, contextFor(entities), 3);
    const reversed = buildNeighbourGraph([...entities].reverse(), contextFor(entities), 3);
    expect(forward.get("agent:a1")).toEqual(reversed.get("agent:a1"));
  });
});

describe("symmetrizeGraph", () => {
  it("makes A-keeps-B imply B-keeps-A", () => {
    // Pure top-k is asymmetric around hubs, and stress majorization needs a
    // symmetric distance set.
    const asymmetric = new Map([
      ["a", [{ key: "hub", distance: 0.2 }]],
      ["hub", [{ key: "b", distance: 0.1 }]],
      ["b", [{ key: "hub", distance: 0.1 }]],
    ]);
    const symmetric = symmetrizeGraph(asymmetric);
    expect(symmetric.get("hub")!.some((n) => n.key === "a")).toBe(true);
    expect(symmetric.get("a")!.some((n) => n.key === "hub")).toBe(true);
  });
});

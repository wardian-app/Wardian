import { describe, expect, it } from "vitest";
import type { AgentConfig } from "../../types";
import { agentRef, folderRef, libraryEntryRef } from "./entityRef";
import {
  MAX_PATH_FACET_DEPTH,
  SCENE_ANCHOR_MAX_WEIGHT,
  admit,
  buildCorpus,
  cosine,
  createCorpus,
  emitAgentFacets,
  emitSkillFacets,
  facetClassOf,
  facetVector,
  idf,
  pathAncestorFacets,
  sceneAnchorToken,
  sceneFacetWeight,
  withPlacement,
} from "./facets";

function agent(overrides: Partial<AgentConfig> = {}): AgentConfig {
  return {
    session_id: "a1",
    session_name: "Alpha",
    agent_class: "Architect",
    folder: "D:\\Development\\Wardian",
    is_off: false,
    ...overrides,
  } as AgentConfig;
}

describe("pathAncestorFacets", () => {
  it("emits the full ancestor chain deepest-first, preserving the drive root", () => {
    expect(pathAncestorFacets("D:\\Dev\\Ward\\src")).toEqual([
      "path:d:/dev/ward/src",
      "path:d:/dev/ward",
      "path:d:/dev",
      "path:d:/",
    ]);
  });

  it("preserves a POSIX root instead of emitting an empty parent", () => {
    expect(pathAncestorFacets("/home/u/proj")).toEqual([
      "path:/home/u/proj",
      "path:/home/u",
      "path:/home",
      "path:/",
    ]);
  });

  it("stops at the UNC share, since a bare server is not a scope", () => {
    expect(pathAncestorFacets("//server/share/proj/src")).toEqual([
      "path://server/share/proj/src",
      "path://server/share/proj",
      "path://server/share",
    ]);
  });

  it("caps chain length so a deep path cannot emit unbounded tokens", () => {
    const deep = `/${Array.from({ length: 40 }, (_, i) => `d${i}`).join("/")}`;
    expect(pathAncestorFacets(deep)).toHaveLength(MAX_PATH_FACET_DEPTH);
  });

  it("returns nothing for blank input", () => {
    expect(pathAncestorFacets("")).toEqual([]);
    expect(pathAncestorFacets(null)).toEqual([]);
  });
});

describe("idf", () => {
  it("is exactly zero for a universal facet, so drive roots are free", () => {
    // This is why full ancestor chains can be emitted without a depth
    // heuristic to suppress the shallow ones.
    const corpus = createCorpus();
    for (let i = 0; i < 5; i += 1) {
      admit(corpus, { ref: agentRef(`a${i}`), tokens: ["path:d:/"], excludes: [] });
    }
    expect(idf(corpus, "path:d:/")).toBe(0);
  });

  it("gives a rare facet a large weight", () => {
    const corpus = createCorpus();
    admit(corpus, { ref: agentRef("a0"), tokens: ["skill:skills/rare", "path:d:/"], excludes: [] });
    for (let i = 1; i < 50; i += 1) {
      admit(corpus, { ref: agentRef(`a${i}`), tokens: ["path:d:/"], excludes: [] });
    }
    expect(idf(corpus, "skill:skills/rare")).toBeGreaterThan(3);
    expect(idf(corpus, "path:d:/")).toBe(0);
  });

  it("is insensitive to corpus growth but sensitive to df on rare facets", () => {
    // The bound the incremental layout relies on: adding an entity perturbs
    // only the rare facets it carries.
    const base = buildCorpus(
      Array.from({ length: 200 }, (_, i) => ({
        ref: agentRef(`a${i}`),
        tokens: ["path:d:/", i < 2 ? "skill:skills/rare" : "tag:common"],
        excludes: [],
      })),
    );
    const beforeRare = idf(base, "skill:skills/rare");
    const beforeCommon = idf(base, "tag:common");

    admit(base, { ref: agentRef("new"), tokens: ["skill:skills/rare"], excludes: [] });

    expect(Math.abs(idf(base, "tag:common") - beforeCommon)).toBeLessThan(0.01);
    expect(beforeRare - idf(base, "skill:skills/rare")).toBeGreaterThan(0.28);
  });

});

describe("cosine", () => {
  const corpus = buildCorpus([
    { ref: agentRef("a1"), tokens: ["team:t1", "path:d:/dev/ward", "path:d:/"], excludes: [] },
    { ref: agentRef("a2"), tokens: ["team:t1", "path:d:/dev/ward", "path:d:/"], excludes: [] },
    { ref: agentRef("a3"), tokens: ["team:t2", "path:d:/other", "path:d:/"], excludes: [] },
  ]);

  it("scores shared-team peers closer than strangers", () => {
    const a1 = facetVector(
      { ref: agentRef("a1"), tokens: ["team:t1", "path:d:/dev/ward", "path:d:/"], excludes: [] },
      corpus,
    );
    const a2 = facetVector(
      { ref: agentRef("a2"), tokens: ["team:t1", "path:d:/dev/ward", "path:d:/"], excludes: [] },
      corpus,
    );
    const a3 = facetVector(
      { ref: agentRef("a3"), tokens: ["team:t2", "path:d:/other", "path:d:/"], excludes: [] },
      corpus,
    );
    expect(cosine(a1, a2)).toBeGreaterThan(cosine(a1, a3));
  });

  it("returns 0 rather than NaN for an entity with no affiliations", () => {
    const empty = facetVector({ ref: agentRef("x"), tokens: [], excludes: [] }, corpus);
    const a1 = facetVector({ ref: agentRef("a1"), tokens: ["team:t1"], excludes: [] }, corpus);
    expect(cosine(empty, a1)).toBe(0);
  });

  it("is symmetric regardless of which vector is shorter", () => {
    const short = facetVector({ ref: agentRef("a1"), tokens: ["team:t1"], excludes: [] }, corpus);
    const long = facetVector(
      { ref: agentRef("a2"), tokens: ["team:t1", "path:d:/dev/ward", "path:d:/"], excludes: [] },
      corpus,
    );
    expect(cosine(short, long)).toBeCloseTo(cosine(long, short), 12);
  });
});

describe("emitAgentFacets", () => {
  it("covers every directory the agent can reach", () => {
    const facets = emitAgentFacets(
      agent({
        folder: "D:\\Dev\\Ward",
        git_worktree_source: "D:\\Dev\\WardMain",
        include_directories: ["D:\\Shared\\Docs"],
      }),
      agentRef("a1"),
      { teamIds: ["t1"], deployedSkillRefs: ["skills/dev/planner"], worktreeId: "wt-1" },
    );
    expect(facets.tokens).toContain("path:d:/dev/ward");
    expect(facets.tokens).toContain("path:d:/dev/wardmain");
    expect(facets.tokens).toContain("path:d:/shared/docs");
    expect(facets.tokens).toContain("team:t1");
    expect(facets.tokens).toContain("class:architect");
    expect(facets.tokens).toContain("skill:skills/dev/planner");
    expect(facets.tokens).toContain("worktree:wt-1");
  });

  it("dedupes and sorts for deterministic iteration", () => {
    const facets = emitAgentFacets(
      agent({ folder: "D:\\Dev\\Ward", git_worktree_source: "D:\\Dev\\Ward" }),
      agentRef("a1"),
    );
    expect(facets.tokens).toEqual([...new Set(facets.tokens)].sort());
  });
});

describe("emitSkillFacets", () => {
  it("distinguishes a live junction from the copy fallback", () => {
    // A copy is a fork whose edits do not sync, so it is weaker evidence of
    // relatedness than a junction and must not be flattened together.
    const ref = libraryEntryRef("skills/dev/planner")!;
    const linked = emitSkillFacets(ref, {
      deployments: [{ targetType: "agent", targetId: "a1", linked: true }],
    });
    const copied = emitSkillFacets(ref, {
      deployments: [{ targetType: "agent", targetId: "a1", linked: false }],
    });
    expect(linked.tokens).toContain("deployed:agent:a1");
    expect(copied.tokens).toContain("deployed:agent:a1~copy");
    expect(copied.tokens).not.toContain("deployed:agent:a1");
  });

  it("section-qualifies library path facets so sections cannot collide", () => {
    const facets = emitSkillFacets(libraryEntryRef("skills/dev/planner")!);
    expect(facets.tokens).toContain("path:skills/dev");
    expect(facets.tokens).toContain("path:skills/dev/planner");
  });
});

describe("scene placement", () => {
  it("caps a placement's weight and decays it when the scene is abandoned", () => {
    const now = 1_000_000_000_000;
    expect(sceneFacetWeight(now, now)).toBeCloseTo(SCENE_ANCHOR_MAX_WEIGHT, 10);
    // One half-life later the placement contributes half as much.
    const halfLife = 30 * 24 * 60 * 60 * 1000;
    expect(sceneFacetWeight(now, now + halfLife)).toBeCloseTo(SCENE_ANCHOR_MAX_WEIGHT / 2, 6);
  });

  it("does not compound with repeated placement", () => {
    // Placing twice must not double the weight, or the map ossifies around one
    // afternoon's arrangement.
    const base = emitAgentFacets(agent(), agentRef("a1"));
    const once = withPlacement(base, { anchoredDistrictId: "d1" });
    const twice = withPlacement(once, { anchoredDistrictId: "d1" });
    expect(twice.tokens.filter((t) => t === sceneAnchorToken("d1"))).toHaveLength(1);
  });

  it("keeps exclusions out of the cosine vector", () => {
    // Exclusion is repulsion from a place, not dissimilarity to peers.
    const base = emitAgentFacets(agent(), agentRef("a1"));
    const placed = withPlacement(base, { excludedDistrictIds: ["d2"] });
    expect(placed.excludes).toEqual(["d2"]);
    const vector = facetVector(placed, buildCorpus([placed]));
    for (const token of vector.keys()) expect(token.startsWith("scene_exclude")).toBe(false);
  });

  it("weights an anchor from the supplied decayed map rather than the corpus", () => {
    const base = emitAgentFacets(agent(), agentRef("a1"));
    const placed = withPlacement(base, { anchoredDistrictId: "d1" });
    const vector = facetVector(placed, buildCorpus([placed]), new Map([["scene_anchor:d1", 1.25]]));
    expect(vector.get("scene_anchor:d1")).toBe(1.25);
  });
});

describe("facetClassOf", () => {
  it("recovers the class even when the value contains a drive letter", () => {
    expect(facetClassOf("path:d:/dev/ward")).toBe("path");
    expect(facetClassOf("deployed:agent:a1")).toBe("deployed");
  });

  it("rejects unknown classes", () => {
    expect(facetClassOf("memory:x")).toBeNull();
    expect(facetClassOf("nocolon")).toBeNull();
  });
});

describe("folder facets", () => {
  it("identifies a folder by its normalized path chain", () => {
    const ref = folderRef("D:\\Dev\\Ward\\src")!;
    const facets = buildCorpus([{ ref, tokens: pathAncestorFacets(ref.path), excludes: [] }]);
    expect(facets.df.has("path:d:/dev/ward/src")).toBe(true);
  });
});

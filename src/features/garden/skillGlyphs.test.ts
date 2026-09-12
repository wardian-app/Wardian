import { describe, expect, it } from "vitest";
import {
  CROWN_CAP,
  GLYPH_RADIUS,
  agentsCarrying,
  assignMonograms,
  buildSkillCrowns,
  crownExtent,
  crownPositions,
  crownReveal,
  crownLabelLayout,
  crownConvergence,
  gardenDetailForScale,
  skillHue,
  type CrownAgent,
} from "./skillGlyphs";
import type { GardenSkillInput } from "./useGardenSkills";

function skill(
  entryRef: string,
  deployments: GardenSkillInput["deployments"],
  label = entryRef.split("/").pop()!,
): GardenSkillInput {
  return { entryRef, label, tags: [], deployments };
}

const agents: CrownAgent[] = [
  { id: "a1", agentClass: "Architect" },
  { id: "a2", agentClass: "Architect" },
  { id: "b1", agentClass: "Coder" },
  { id: "c1", agentClass: null },
];

describe("buildSkillCrowns", () => {
  it("hangs a direct deployment on exactly its target", () => {
    const crowns = buildSkillCrowns(
      [skill("skills/kicad", [{ targetType: "agent", targetId: "a1", linked: true }])],
      agents,
    );
    expect(crowns.get("a1")).toMatchObject([{ entryRef: "skills/kicad", provenance: "direct" }]);
    expect(crowns.has("a2")).toBe(false);
  });

  it("reaches every agent of a class", () => {
    // A class deployment has no single agent to sit beside, which is why a skill
    // *unit* could not express it at all.
    const crowns = buildSkillCrowns(
      [skill("skills/spec", [{ targetType: "class", targetId: "Architect", linked: true }])],
      agents,
    );
    expect([...crowns.keys()].sort()).toEqual(["a1", "a2"]);
    expect(crowns.get("a1")![0].provenance).toBe("class");
  });

  it("matches a class target case-insensitively", () => {
    // `target_id` is the class directory name and `agent_class` is typed by
    // hand; a case mismatch must not silently drop an inherited skill.
    const crowns = buildSkillCrowns(
      [skill("skills/spec", [{ targetType: "class", targetId: "architect", linked: true }])],
      agents,
    );
    expect([...crowns.keys()].sort()).toEqual(["a1", "a2"]);
  });

  it("reaches every agent from the user scope", () => {
    const crowns = buildSkillCrowns(
      [skill("skills/all", [{ targetType: "user", targetId: "global", linked: true }])],
      agents,
    );
    expect([...crowns.keys()].sort()).toEqual(["a1", "a2", "b1", "c1"]);
    expect(crowns.get("c1")![0].provenance).toBe("global");
  });

  it("ignores an unrecognised user-scope target rather than painting everyone", () => {
    const crowns = buildSkillCrowns(
      [skill("skills/odd", [{ targetType: "user", targetId: "someone", linked: true }])],
      agents,
    );
    expect(crowns.size).toBe(0);
  });

  it("keeps the strongest tie when an agent picks a skill up more than one way", () => {
    // Direct beats inherited: it is agent-specific and does not change when the
    // class does, so that is what the glyph must say.
    const crowns = buildSkillCrowns(
      [
        skill("skills/spec", [
          { targetType: "class", targetId: "Architect", linked: true },
          { targetType: "agent", targetId: "a1", linked: false },
        ]),
      ],
      agents,
    );
    expect(crowns.get("a1")).toMatchObject([{ provenance: "direct", copied: true }]);
    expect(crowns.get("a2")).toMatchObject([{ provenance: "class", copied: false }]);
  });

  it("drops a deployment to an agent that is not on the map", () => {
    const crowns = buildSkillCrowns(
      [skill("skills/ghost", [{ targetType: "agent", targetId: "gone", linked: true }])],
      agents,
    );
    expect(crowns.size).toBe(0);
  });

  it("orders a crown by IDF, so the distinctive skill leads", () => {
    // Otherwise the skill everybody has renders on everybody and crowds out the
    // one that says something about this agent.
    const crowns = buildSkillCrowns(
      [
        skill("skills/everywhere", [{ targetType: "user", targetId: "global", linked: true }]),
        skill("skills/shared", [{ targetType: "class", targetId: "Architect", linked: true }]),
        skill("skills/rare", [{ targetType: "agent", targetId: "a1", linked: true }]),
      ],
      agents,
    );
    expect(crowns.get("a1")!.map((glyph) => glyph.entryRef)).toEqual([
      "skills/rare",
      "skills/shared",
      "skills/everywhere",
    ]);
  });

  it("counts a carrier once when the skill arrives twice, so IDF is not skewed", () => {
    const crowns = buildSkillCrowns(
      [
        skill("skills/both", [
          { targetType: "class", targetId: "Architect", linked: true },
          { targetType: "agent", targetId: "a1", linked: true },
        ]),
        skill("skills/rare", [{ targetType: "agent", targetId: "a1", linked: true }]),
      ],
      agents,
    );
    expect(crowns.get("a1")).toHaveLength(2);
    // `both` has two carriers and `rare` has one, so `rare` still leads.
    expect(crowns.get("a1")![0].entryRef).toBe("skills/rare");
  });

  it("is stable under input reordering", () => {
    const inputs = [
      skill("skills/a", [{ targetType: "agent", targetId: "a1", linked: true }]),
      skill("skills/b", [{ targetType: "agent", targetId: "a1", linked: true }]),
    ];
    const forward = buildSkillCrowns(inputs, agents).get("a1")!;
    const reversed = buildSkillCrowns([...inputs].reverse(), agents).get("a1")!;
    expect(reversed).toEqual(forward);
  });
});

describe("assignMonograms", () => {
  it("takes the initials of the first two words", () => {
    // Initials rather than a prefix of the first word, so "KiCad Review" and
    // "KiCad Export" stay distinguishable.
    expect(assignMonograms([{ entryRef: "skills/x", label: "KiCad Review" }]).get("skills/x")).toBe(
      "KR",
    );
    expect(assignMonograms([{ entryRef: "skills/x", label: "KiCad Export" }]).get("skills/x")).toBe(
      "KE",
    );
  });

  it("takes two letters from a single-word label", () => {
    expect(assignMonograms([{ entryRef: "skills/x", label: "planner" }]).get("skills/x")).toBe("PL");
  });

  it("falls back to the last word when two labels open the same way", () => {
    // The failure mode that matters in practice: near-duplicate names differ at
    // the end, not the start.
    const assigned = assignMonograms([
      { entryRef: "skills/auto", label: "Trident LEAPS Automation" },
      { entryRef: "skills/refresh", label: "Trident LEAPS Refresh" },
    ]);
    expect(assigned.get("skills/auto")).toBe("TL");
    expect(assigned.get("skills/refresh")).toBe("RE");
  });

  it("gives one skill one monogram regardless of which agents carry it", () => {
    // A glyph has to mean the same skill wherever it appears, so assignment is
    // global rather than per-crown, and independent of input order.
    const entries = [
      { entryRef: "skills/b", label: "Trident LEAPS Refresh" },
      { entryRef: "skills/a", label: "Trident LEAPS Automation" },
    ];
    expect(assignMonograms(entries)).toEqual(assignMonograms([...entries].reverse()));
  });

  it("still produces something for a label with no letters or digits", () => {
    expect(assignMonograms([{ entryRef: "skills/x", label: "***" }]).get("skills/x")).toBe("?1");
  });
});

describe("skillHue", () => {
  it("is deterministic, in range, and case-insensitive", () => {
    expect(skillHue("skills/kicad")).toBe(skillHue("skills/KiCad"));
    expect(skillHue("skills/kicad")).toBeGreaterThanOrEqual(0);
    expect(skillHue("skills/kicad")).toBeLessThan(360);
  });
});

describe("agentsCarrying", () => {
  it("answers the reverse question with a set", () => {
    // Instancing removes the ability to point at *the* place a skill lives, so
    // this has to exist or the change is a net loss.
    const crowns = buildSkillCrowns(
      [skill("skills/spec", [{ targetType: "class", targetId: "Architect", linked: true }])],
      agents,
    );
    expect([...agentsCarrying(crowns, "skills/spec")].sort()).toEqual(["a1", "a2"]);
    expect(agentsCarrying(crowns, "skills/absent").size).toBe(0);
  });
});

describe("crown geometry", () => {
  it("keeps neighbouring glyphs from overlapping at every count", () => {
    // A fixed *angular* step looks right at one count and smears at the next,
    // because the chord an angle subtends depends on the radius.
    for (const count of [2, 3, 6, 8, 12, 13]) {
      const positions = crownPositions(count);
      for (let index = 1; index < positions.length; index += 1) {
        const gap = Math.hypot(
          positions[index].x - positions[index - 1].x,
          positions[index].y - positions[index - 1].y,
        );
        expect(gap).toBeGreaterThan(2 * GLYPH_RADIUS);
      }
    }
  });

  it("clears the agent's status halo", () => {
    for (const position of crownPositions(4)) {
      expect(Math.hypot(position.x, position.y)).toBeGreaterThanOrEqual(18 + GLYPH_RADIUS);
    }
  });

  it("is symmetric about straight up", () => {
    const positions = crownPositions(5);
    expect(positions[0].x).toBeCloseTo(-positions[4].x);
    expect(positions[2].x).toBeCloseTo(0);
    expect(positions[2].y).toBeLessThan(0);
  });

  it("reserves nothing for an agent with no skills", () => {
    // A sparse roster must not pay for a feature it is not using.
    expect(crownExtent(0)).toBe(0);
    expect(crownPositions(0)).toEqual([]);
  });

  it("grows the reserved extent only once the arc runs out of room", () => {
    expect(crownExtent(6)).toBe(crownExtent(2));
    expect(crownExtent(12)).toBeGreaterThan(crownExtent(6));
    // Past the cap the crown stops growing except for the overflow slot.
    expect(crownExtent(40)).toBe(crownExtent(13));
  });

  it("covers every drawn glyph, including the overflow slot", () => {
    const reach = (count: number) =>
      Math.max(...crownPositions(count).map((p) => Math.hypot(p.x, p.y))) + GLYPH_RADIUS;
    expect(crownExtent(6)).toBeCloseTo(reach(6));
    // 20 skills draws CROWN_CAP.near glyphs plus a "+8" counter.
    expect(crownExtent(20)).toBeCloseTo(reach(CROWN_CAP.near + 1));
  });
});

describe("gardenDetailForScale", () => {
  it("hides the crown at map scale and opens it up close", () => {
    expect(gardenDetailForScale(0.5)).toBe("far");
    expect(gardenDetailForScale(1)).toBe("mid");
    expect(gardenDetailForScale(2)).toBe("near");
    expect(CROWN_CAP.far).toBe(0);
    expect(CROWN_CAP.mid).toBeLessThan(CROWN_CAP.near);
  });
});

describe("continuous crown disclosure", () => {
  it("interpolates each stable slot continuously into its Capabilities grid", () => {
    crownPositions(13).forEach((position, index) => {
      const start = crownConvergence(position, index);
      expect(start).toEqual({ ...position, glyphScale: 1, labelOpacity: 1 });
      const end = crownConvergence(position, index, 1);
      expect(end).toEqual({ x: -9.5 + (index % 3) * 2.4, y: -7.4 + Math.floor(index / 3) * 3.25, glyphScale: 0.085, labelOpacity: 0 });
      expect(GLYPH_RADIUS * end.glyphScale).toBeCloseTo(0.5525);
      const middle = crownConvergence(position, index, 0.5);
      for (const key of ["x", "y", "glyphScale", "labelOpacity"] as const) {
        expect(middle[key]).toBeCloseTo((start[key] + end[key]) / 2);
        for (const boundary of [0, 1]) {
          const before = crownConvergence(position, index, boundary - 0.000001);
          const after = crownConvergence(position, index, boundary + 0.000001);
          expect(Math.abs(after[key] - before[key])).toBeLessThan(0.0001);
        }
      }
      expect(crownConvergence(position, index, -1)).toEqual(start);
      expect(crownConvergence(position, index, 2)).toEqual(end);
      expect(crownConvergence(position, index, NaN)).toEqual(start);
    });
  });

  it("reveals earlier slots first, monotonically, with a bounded glyph size", () => {
    for (let index = 0; index <= CROWN_CAP.near; index += 1) {
      let previous = 0;
      for (let scale = 0; scale <= 3; scale += 0.01) {
        const reveal = crownReveal(scale, index);
        expect(reveal.opacity).toBeGreaterThanOrEqual(previous);
        expect(reveal.opacity).toBeLessThanOrEqual(crownReveal(scale, 0).opacity);
        expect(reveal.glyphScale).toBeGreaterThanOrEqual(0.65);
        expect(reveal.glyphScale).toBeLessThanOrEqual(1);
        if (reveal.monogramOpacity > 0) expect(8 * scale * reveal.glyphScale).toBeGreaterThan(10);
        previous = reveal.opacity;
      }
      expect(crownReveal(3, index).opacity).toBe(1);
    }
  });

  it("has no jumps or slope discontinuities at reveal endpoints or old detail thresholds", () => {
    const epsilon = 0.00001;
    for (let index = 0; index <= CROWN_CAP.near; index += 1) {
      const start = 0.55 + index * 0.07;
      for (const threshold of [start, start + 0.4, 0.7, 1.3, 1.25, 1.5, 2, 2.6]) {
        const before = crownReveal(threshold - epsilon, index);
        const at = crownReveal(threshold, index);
        const after = crownReveal(threshold + epsilon, index);
        for (const key of ["opacity", "glyphScale", "monogramOpacity", "labelOpacity"] as const) {
          expect(Math.abs(after[key] - before[key])).toBeLessThan(0.001);
          const incoming = (at[key] - before[key]) / epsilon;
          const outgoing = (after[key] - at[key]) / epsilon;
          expect(Math.abs(incoming - outgoing)).toBeLessThan(0.01);
        }
      }
    }
  });

  it("keeps invalid camera values finite and hidden", () => {
    for (const scale of [NaN, Infinity, -Infinity, -1, 0]) {
      expect(crownReveal(scale, 0).opacity).toBe(0);
      expect(crownReveal(scale, 0).labelOpacity).toBe(0);
    }
  });

  it("centers a horizontal 72px single-skill label with a 4px gap above the glyph", () => {
    for (const scale of [2.01, 2.6, 4, 8, 10]) {
      const label = crownLabelLayout(crownPositions(1)[0], scale, 1);
      expect(label.rotation).toBe(0);
      expect(label.align).toBe("center");
      expect(label.width * scale).toBeCloseTo(72);
      expect(label.height * scale).toBeCloseTo(12);
      expect(label.fontSize * scale).toBeCloseTo(10);
      expect(label.x - label.offsetX + label.width / 2).toBeCloseTo(0);
      expect((-GLYPH_RADIUS - (label.y - label.offsetY + label.height)) * scale).toBeCloseTo(4);
    }
  });

  it("preserves the radial layout for every multi-skill crown", () => {
    for (const count of [2, 3, 6, 12, 15]) {
      for (const position of crownPositions(Math.min(count, 13))) {
        expect(crownLabelLayout(position, 2.6, count)).toEqual(crownLabelLayout(position, 2.6));
      }
    }
  });

  it("keeps label boxes separated at every crown count and readable zoom", () => {
    // Separating-axis test on the actual rotated Konva text boxes, including
    // their maximum width: long labels ellipsize inside these same bounds.
    for (let count = 1; count <= CROWN_CAP.near + 1; count += 1) {
      for (const scale of [2, 2.01, 2.3, 2.6, 4, 8]) {
        const boxes = crownPositions(count).map((position) => {
          const label = crownLabelLayout(position, scale, count);
          expect(label.fontSize * scale).toBe(10);
          const angle = label.rotation * Math.PI / 180;
          return [[0, 0], [label.width, 0], [label.width, label.height], [0, label.height]].map(([x, y]) => ({
            x: position.x + label.x + (x - label.offsetX) * Math.cos(angle) - (y - label.offsetY) * Math.sin(angle),
            y: position.y + label.y + (x - label.offsetX) * Math.sin(angle) + (y - label.offsetY) * Math.cos(angle),
          }));
        });
        boxes.forEach((box, index) => {
          for (const other of boxes.slice(index + 1)) {
            const axes = [box, other].flatMap((corners) => [0, 1].map((edge) => ({
              x: -(corners[edge + 1].y - corners[edge].y),
              y: corners[edge + 1].x - corners[edge].x,
            })));
            expect(axes.some((axis) => {
              const a = box.map((p) => p.x * axis.x + p.y * axis.y);
              const b = other.map((p) => p.x * axis.x + p.y * axis.y);
              return Math.max(...a) <= Math.min(...b) || Math.max(...b) <= Math.min(...a);
            })).toBe(true);
          }
        });
      }
    }
  });
});

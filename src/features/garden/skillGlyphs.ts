/**
 * Skills as attributes of an agent rather than units on the map.
 *
 * ## Why a skill is not a place
 *
 * A skill deployed to six agents is one library object that would have to sit
 * in one location. It cannot be near all six, so the metric puts it near the
 * centroid of its targets — a spot where it is relevant to nobody. Picking the
 * most-referenced district instead is a tie-break for an unanswerable question:
 * the skill is genuinely in six districts.
 *
 * The error is upstream of the layout. `deployed:agent:a1` is a fact *about
 * a1*, not a fact about a location, so it belongs on a1. The same reasoning
 * already applies to containment and team membership, which become geometry or
 * decoration and are never drawn as separate nodes. The general rule:
 *
 * > An entity that is an **attribute** of another renders *on* it. An entity
 * > with **independent existence and its own lifecycle** gets a unit.
 *
 * Agents, automation blueprints, and change sets pass that test. A skill
 * *deployment* does not, so it renders as a glyph. The skill *itself* still has
 * independent existence — it just lives in the Library, and is found on the map
 * by highlighting the agents that carry it rather than by being placed.
 *
 * ## What instancing buys
 *
 * Class-inherited skills become expressible, which the unit model structurally
 * could not do: a skill deployed to a class has no single agent to sit beside.
 * Glyphs also leave the layout entirely — they are decoration attached to a
 * position, so they cost nothing in SMACOF or overlap removal, and they may
 * change with zoom without any risk of moving a unit.
 *
 * Skills stay in the *metric* even though they leave the *unit set*: an agent's
 * `skill:<entry_ref>` facets are among the best signals in the corpus, because
 * two agents carrying the same rare skill genuinely are close. See
 * `emitAgentFacets`.
 */

import { idf, type FacetCorpus } from "./facets";
import type { GardenSkillDeployment, GardenSkillInput } from "./useGardenSkills";

/**
 * How an agent came to have a skill.
 *
 * A skill from a class is not the same object as one deployed directly: the
 * first changes when the class changes, the second is agent-specific. Flatten
 * them and the map lies about where a capability comes from.
 */
// --- Crown geometry -------------------------------------------------------

/**
 * How much of an agent the canvas is currently drawing.
 *
 * Progressive disclosure is a zoom concern rather than a layout one — the
 * footprint reserved for the crown is constant (see `AGENT_UNIT_SIZE`), so
 * detail can change freely without moving anything.
 */
export type GardenDetail = "far" | "mid" | "near";

/**
 * Glyphs drawn per detail level.
 *
 * Far is deliberately zero: at map scale you are reading territory, and a ring
 * of 6px discs on every agent is texture rather than information. The mid cap
 * is small enough to stay scannable — it answers "what kind of agent is this"
 * and defers "exactly which skills" to a closer look or the selection panel.
 */
export const CROWN_CAP: Record<GardenDetail, number> = { far: 0, mid: 6, near: 12 };

const FAR_SCALE = 0.7;
const NEAR_SCALE = 1.3;

export function gardenDetailForScale(scale: number): GardenDetail {
  if (scale < FAR_SCALE) return "far";
  if (scale < NEAR_SCALE) return "mid";
  return "near";
}

/** Smallest arc radius, clear of the agent's 18px status halo. */
const CROWN_RADIUS = 27;
export const GLYPH_RADIUS = 6.5;

/**
 * Move the existing crown glyph into its Capabilities row in cell world space.
 * The caller supplies eased progress; interpolation here stays linear so it
 * matches the DOM handoff. Scale the whole mark, including text and rings.
 */
export function crownConvergence(position: { x: number; y: number }, index: number, convergence = 0) {
  const progress = Number.isFinite(convergence) ? Math.max(0, Math.min(1, convergence)) : 0;
  return {
    x: position.x * (1 - progress) + (-9.5 + (index % 3) * 2.4) * progress,
    y: position.y * (1 - progress) + (-7.4 + Math.floor(index / 3) * 3.25) * progress,
    glyphScale: 1 * (1 - progress) + 0.085 * progress,
    labelOpacity: 1 - progress,
  };
}

/** Cubic easing with zero slope at both ends, including when zoom reverses. */
function smoothReveal(value: number, start: number, end: number): number {
  const t = Math.max(0, Math.min(1, (value - start) / (end - start)));
  return t * t * (3 - 2 * t);
}

/**
 * Continuous disclosure for a stable near-crown slot (overflow uses slot 12).
 * The first skills arrive around workstream scale; later ones follow in order.
 * Text fades only after its effective screen size is readable.
 */
export function crownReveal(scale: number, index: number) {
  const zoom = Number.isFinite(scale) ? Math.max(0, scale) : 0;
  const start = 0.55 + Math.max(0, index) * 0.07;
  const opacity = smoothReveal(zoom, start, start + 0.4);
  const glyphScale = 0.65 + 0.35 * opacity;
  return {
    opacity,
    glyphScale,
    monogramOpacity: smoothReveal(8 * zoom * glyphScale, 10, 12),
    labelOpacity: smoothReveal(zoom, 2, 2.6),
  };
}

/**
 * Outward radial label lane, fixed to its glyph's angle. Ten screen-pixel text
 * in a bounded single line fits between neighboring rays at label zoom (>=2).
 * Left-side labels are flipped upright. Width stays bounded in screen pixels.
 * A single-skill crown instead centers a horizontal label above its glyph.
 * Pass total crown size, never the changing number of revealed glyphs.
 */
export function crownLabelLayout(position: { x: number; y: number }, scale: number, crownLength = 0) {
  const zoom = Number.isFinite(scale) ? Math.max(0.01, scale) : 0.01;
  if (crownLength === 1) {
    return {
      x: 0,
      y: -GLYPH_RADIUS - 16 / zoom,
      rotation: 0,
      width: 72 / zoom,
      height: 12 / zoom,
      offsetX: 36 / zoom,
      offsetY: 0,
      fontSize: 10 / zoom,
      align: "center" as const,
    };
  }
  const angle = Math.atan2(position.y, position.x);
  const left = position.x < -0.000001;
  const distance = GLYPH_RADIUS + 4 / zoom;
  return {
    x: Math.cos(angle) * distance,
    y: Math.sin(angle) * distance,
    rotation: angle * 180 / Math.PI + (left ? 180 : 0),
    width: 72 / zoom,
    height: 12 / zoom,
    offsetX: left ? 72 / zoom : 0,
    offsetY: 6 / zoom,
    fontSize: 10 / zoom,
    align: left ? "right" as const : "left" as const,
  };
}
/**
 * Centre-to-centre spacing along the arc. Slightly more than a glyph diameter,
 * so neighbours read as separate marks rather than a smear.
 */
const GLYPH_CHORD = 16;
/** Widest the crown may sweep before it grows outward instead of sideways. */
const MAX_SWEEP = (200 * Math.PI) / 180;

/**
 * Glyph centres on an arc centred straight up, so the crown grows symmetrically.
 *
 * Spacing is held constant in *pixels* rather than degrees. A fixed angular
 * step looks right at one count and overlaps at the next, because the chord a
 * given angle subtends is fixed by the radius. Holding the chord and letting
 * the radius grow once the arc would exceed `MAX_SWEEP` keeps every glyph
 * legible at any count, and keeps a small crown tight against its agent.
 */
export function crownPositions(count: number): Array<{ x: number; y: number }> {
  if (count <= 0) return [];
  const radius = Math.max(CROWN_RADIUS, (GLYPH_CHORD * (count - 1)) / MAX_SWEEP);
  const step = GLYPH_CHORD / radius;
  const positions: Array<{ x: number; y: number }> = [];
  const start = -Math.PI / 2 - ((count - 1) * step) / 2;
  for (let index = 0; index < count; index += 1) {
    const angle = start + index * step;
    positions.push({ x: Math.cos(angle) * radius, y: Math.sin(angle) * radius });
  }
  return positions;
}

/**
 * How far the crown for `crownLength` skills reaches from the agent's centre.
 *
 * The layout reserves this so units never collide through their crowns. It is
 * measured at the *near* cap rather than at the current zoom: detail changes
 * with the viewport, and geometry that moved when you zoomed would break the
 * rule that only canonical records may move a unit. An agent with no skills
 * reserves nothing, so a sparse roster does not pay for a feature it is not
 * using.
 */
export function crownExtent(crownLength: number): number {
  if (crownLength <= 0) return 0;
  // The overflow counter takes a slot of its own.
  const slots = Math.min(crownLength, CROWN_CAP.near) + (crownLength > CROWN_CAP.near ? 1 : 0);
  return Math.max(CROWN_RADIUS, (GLYPH_CHORD * (slots - 1)) / MAX_SWEEP) + GLYPH_RADIUS;
}

// --- Provenance -----------------------------------------------------------

export type SkillProvenance = "direct" | "class" | "global";

/** Strongest tie wins when an agent picks up the same skill more than one way. */
const PROVENANCE_RANK: Record<SkillProvenance, number> = { direct: 0, class: 1, global: 2 };

/** `DeploymentTarget.target_id` used by the `user` scope; see `scan_deployments`. */
const GLOBAL_TARGET_ID = "global";

export interface GardenSkillGlyph {
  /**
   * The library's `<section>/<rel_path>` identity, verbatim.
   *
   * Case is preserved because the Library's own lookup is case-sensitive, while
   * everything comparing refs here lowercases first.
   */
  entryRef: string;
  label: string;
  /** One or two characters, unique across the library. See `assignMonograms`. */
  monogram: string;
  /** 0-359, deterministic from `entryRef`. */
  hue: number;
  provenance: SkillProvenance;
  /** True when the winning deployment is a copy rather than a live junction. */
  copied: boolean;
}

export interface CrownAgent {
  id: string;
  /** `AgentConfig.agent_class`, matched against a `class` deployment target. */
  agentClass?: string | null;
}

/**
 * Deterministic hue from a skill's identity.
 *
 * FNV-1a, because the requirement is only that two different skills usually
 * differ and that the same skill never changes colour between sessions. Drawn
 * at a mid lightness so the monogram can be painted in the *background* colour
 * and stay legible in both themes without a per-theme palette.
 */
export function skillHue(entryRef: string): number {
  let hash = 0x811c9dc5;
  const value = entryRef.toLowerCase();
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0) % 360;
}

/**
 * Monogram candidates for a label, most preferred first.
 *
 * Dwarf Fortress works because its symbol set is fixed and learned. Wardian
 * skills are user-named, so no generated glyph set can be distinguishable in
 * general — the honest fallback is a monogram, which is at least legible and
 * learnable over a *user's own* library. Collisions are real and expected:
 * "Trident LEAPS Automation" and "Trident LEAPS Refresh" both open on "TL",
 * so the second tier switches to the last word, which is where near-duplicate
 * names actually differ.
 */
function monogramCandidates(label: string): string[] {
  const words = label.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  if (words.length === 0) return [];

  const candidates: string[] = [];
  const push = (value: string) => {
    const trimmed = value.toUpperCase().slice(0, 2);
    if (trimmed && !candidates.includes(trimmed)) candidates.push(trimmed);
  };

  if (words.length === 1) push(words[0].slice(0, 2));
  else push(words[0][0] + words[1][0]);

  const last = words[words.length - 1];
  push(last.slice(0, 2));
  push(words[0][0] + last[0]);
  // Initials of every word, for names that differ only in the middle.
  push(words.map((word) => word[0]).join("").slice(0, 2));

  return candidates;
}

/**
 * One monogram per skill, unique across the whole library.
 *
 * Assignment is global rather than per-agent on purpose. A glyph has to mean
 * the same skill wherever it appears, so resolving collisions only within one
 * agent's crown would let the same skill render as "TL" on one agent and "AU"
 * on another. Input is sorted by `entryRef` so the result depends on the
 * library's contents and not on the order they arrived in.
 */
export function assignMonograms(entries: ReadonlyArray<{ entryRef: string; label: string }>): Map<string, string> {
  const assigned = new Map<string, string>();
  const taken = new Set<string>();
  const ordered = [...entries].sort((left, right) => left.entryRef.localeCompare(right.entryRef));

  for (const [index, entry] of ordered.entries()) {
    const candidates = monogramCandidates(entry.label);
    let chosen = candidates.find((candidate) => !taken.has(candidate));
    if (!chosen) {
      // Every readable form is spoken for. A digit is ugly but unambiguous,
      // and it is bounded: a library big enough to exhaust two-letter space
      // would be unreadable as glyphs anyway.
      const initial = candidates[0]?.[0] ?? "?";
      chosen = `${initial}${(index + 1) % 10}`;
    }
    taken.add(chosen);
    assigned.set(entry.entryRef, chosen);
  }
  return assigned;
}

/** Agents a single deployment reaches, with the provenance it confers. */
function resolveDeployment(
  deployment: GardenSkillDeployment,
  agents: readonly CrownAgent[],
  agentsByClass: ReadonlyMap<string, string[]>,
): Array<{ agentId: string; provenance: SkillProvenance }> {
  switch (deployment.targetType) {
    case "agent":
      return [{ agentId: deployment.targetId, provenance: "direct" }];
    case "class":
      return (agentsByClass.get(deployment.targetId.toLowerCase()) ?? []).map((agentId) => ({
        agentId,
        provenance: "class" as const,
      }));
    case "user":
      // Only the documented `global` scope. An unrecognised user-scope id would
      // otherwise silently paint every agent.
      if (deployment.targetId.toLowerCase() !== GLOBAL_TARGET_ID) return [];
      return agents.map((agent) => ({ agentId: agent.id, provenance: "global" as const }));
    default:
      return [];
  }
}

/**
 * Build every agent's skill crown, ordered most distinctive first.
 *
 * Ordering is IDF descending over the crown corpus, which is the same smoothed
 * statistic the distance metric uses. This is what keeps a crown informative:
 * a skill deployed to all 51 agents would otherwise render 51 times and swamp
 * the visual field with zero information, whereas at `df === N` its IDF is
 * exactly 0 and it sinks to the truncated tail. Provenance is not ranked
 * directly — direct skills lead naturally because they are the rare ones.
 */
export function buildSkillCrowns(
  skills: readonly GardenSkillInput[],
  agents: readonly CrownAgent[],
): Map<string, GardenSkillGlyph[]> {
  const agentsByClass = new Map<string, string[]>();
  for (const agent of agents) {
    const className = agent.agentClass?.toLowerCase();
    if (!className) continue;
    const existing = agentsByClass.get(className);
    if (existing) existing.push(agent.id);
    else agentsByClass.set(className, [agent.id]);
  }

  const knownAgentIds = new Set(agents.map((agent) => agent.id));
  const monograms = assignMonograms(skills);

  // entryRef -> agentId -> winning tie. Two passes so IDF can be computed from
  // the finished membership rather than from deployment records, which
  // over-count a skill an agent picks up both directly and through its class.
  const carriers = new Map<string, Map<string, { provenance: SkillProvenance; copied: boolean }>>();
  for (const skill of skills) {
    for (const deployment of skill.deployments) {
      for (const reached of resolveDeployment(deployment, agents, agentsByClass)) {
        if (!knownAgentIds.has(reached.agentId)) continue;
        let perAgent = carriers.get(skill.entryRef);
        if (!perAgent) {
          perAgent = new Map();
          carriers.set(skill.entryRef, perAgent);
        }
        const current = perAgent.get(reached.agentId);
        if (current && PROVENANCE_RANK[current.provenance] <= PROVENANCE_RANK[reached.provenance]) {
          continue;
        }
        perAgent.set(reached.agentId, {
          provenance: reached.provenance,
          copied: !deployment.linked,
        });
      }
    }
  }

  // A document is an agent and a token is a skill, so `df` counts carriers.
  const corpus: FacetCorpus = {
    entityCount: agents.length,
    df: new Map([...carriers].map(([entryRef, perAgent]) => [entryRef, perAgent.size])),
  };

  const labels = new Map(skills.map((skill) => [skill.entryRef, skill.label]));
  const crowns = new Map<string, GardenSkillGlyph[]>();
  for (const [entryRef, perAgent] of carriers) {
    for (const [agentId, tie] of perAgent) {
      const glyph: GardenSkillGlyph = {
        entryRef,
        label: labels.get(entryRef) ?? entryRef,
        monogram: monograms.get(entryRef) ?? "?",
        hue: skillHue(entryRef),
        provenance: tie.provenance,
        copied: tie.copied,
      };
      const existing = crowns.get(agentId);
      if (existing) existing.push(glyph);
      else crowns.set(agentId, [glyph]);
    }
  }

  for (const crown of crowns.values()) {
    crown.sort(
      (left, right) =>
        idf(corpus, right.entryRef) - idf(corpus, left.entryRef) ||
        left.label.localeCompare(right.label) ||
        left.entryRef.localeCompare(right.entryRef),
    );
  }
  return crowns;
}

/**
 * Agents carrying a given skill, for the reverse highlight.
 *
 * Instancing removes the ability to point at *the* place a skill lives, so this
 * has to exist alongside it or the change is a net loss of capability. The
 * answer is a set rather than a point, which is what the question deserves.
 */
export function agentsCarrying(
  crowns: ReadonlyMap<string, readonly GardenSkillGlyph[]>,
  entryRef: string,
): Set<string> {
  const target = entryRef.toLowerCase();
  const carriers = new Set<string>();
  for (const [agentId, crown] of crowns) {
    if (crown.some((glyph) => glyph.entryRef.toLowerCase() === target)) carriers.add(agentId);
  }
  return carriers;
}

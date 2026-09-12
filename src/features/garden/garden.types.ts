import type { RunStatusKind } from "../automations/run/runTypes";
import type { GardenSkillGlyph } from "./skillGlyphs";

/**
 * Kinds the Garden can address.
 *
 * Agents have authored placement. Situated automations derive their locations
 * from participants or workspace anchors. `skill` is addressable and deep-linked,
 * but a skill has no position of its own — it renders on the agents that carry
 * it. See `skillGlyphs.ts` for why.
 *
 * `path` is a piece of ground rather than an entity. It is addressable for the
 * same reason a skill is — the operator can point at it, and the summary bar
 * has to say what they pointed at — but a file is an attribute of its folder
 * and never enters the layout. Its id is a normalized absolute path, so the
 * key space stays the one `entityRef.ts` established.
 */
export type GardenEntityKind = "agent" | "automation" | "skill" | "path" | "district" | "workspace" | "memory" | "stage" | "identity";

/** Persisted world-to-screen transform shared by canvas and DOM cutaways. */
export interface GardenCamera {
  scale: number;
  position: GardenPosition;
}

export interface GardenEntityRef {
  kind: GardenEntityKind;
  id: string;
}

export interface GardenPosition {
  x: number;
  y: number;
}

export interface GardenAgentUnit {
  ref: GardenEntityRef; // kind === "agent"
  label: string;
  status: string;
  color: string; // may be a CSS var() expression; resolve before Konva fill
  position: GardenPosition;
  /**
   * Skills this agent carries, most distinctive first.
   *
   * Decoration, not geometry: the crown is attached to the agent's position and
   * never enters the layout, which is what lets it expand and contract with
   * zoom without any risk of moving a unit.
   */
  crown: GardenSkillGlyph[];
}

export type GardenAutomationRunStatus = RunStatusKind | "none";

export interface GardenAutomationUnit {
  ref: GardenEntityRef; // kind === "automation"
  label: string;
  runStatus: GardenAutomationRunStatus;
  nodeCount: number;
  position: GardenPosition;
}

export function unitKey(ref: GardenEntityRef): string {
  return `${ref.kind}:${ref.id}`;
}

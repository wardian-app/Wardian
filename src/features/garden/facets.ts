/**
 * Facet extraction and the IDF corpus for the Garden map's distance metric.
 *
 * ## Why facets rather than a direct distance function
 *
 * There is no natural distance between an agent and a PDF. But both have
 * *affiliations* with a shared vocabulary of scopes — a folder, a team, a
 * skill, a class — so distance is computed in facet space and the
 * heterogeneity dissolves. Every entity emits a sparse set of facet tokens;
 * similarity is weighted cosine over those sets.
 *
 * ## Why IDF
 *
 * Sharing `path:d:/` is worthless. Sharing `skill:skills/kicad-review` is
 * nearly identifying. Inverse document frequency separates the two without
 * hand-tuned rules, and it gives path-depth weighting *for free*: deeper
 * directories have fewer members, hence higher IDF. There is deliberately no
 * `gamma^depth` term — the corpus statistics already encode it and they adapt
 * as the workspace grows.
 *
 * ## The corpus is the materialized entity set, not the disk
 *
 * Wardian has no recursive filesystem enumeration in either language
 * (`get_directory_tree` in `src-tauri/src/commands/fs.rs` does one `read_dir`
 * per call, and the explorer expands lazily). So `df` cannot span the
 * filesystem without a crawl. It doesn't need to: files are not corpus members
 * until a folder parcel is expanded. Expansion admits children, increments
 * `df` along the affected ancestor chain only, and the layout's drift penalty
 * absorbs the local perturbation instead of reflowing the map. The lazy
 * ingestion boundary and the level-of-detail boundary are the same mechanism.
 *
 * ## Garden-local facets
 *
 * User placement enters through the same channel as canonical facts, in the
 * `scene_anchor` / `scene_exclude` namespaces. Per
 * `docs/specs/2026-07-14-entity-oriented-agent-semantics.md`, spatial
 * proximity is interpretive: these facets change geometry and ranking only and
 * never create a team membership, deployment, or binding. The namespace split
 * is what makes that checkable rather than a convention people drift from.
 */

import type { AgentConfig } from "../../types";
import type { EntityRef } from "./entityRef";
import { normalizeEntityPath } from "./entityRef";

/**
 * Facet classes. The prefix of every token, and the key for the semantic prior
 * `kappa`. No class contains ":", so `token.slice(0, token.indexOf(":"))`
 * recovers the class unambiguously even when the value holds drive letters.
 */
export type FacetClass =
  | "path"
  | "team"
  | "watchlist"
  | "class"
  | "provider"
  | "model"
  | "mcp"
  | "ext"
  | "skill"
  | "deployed"
  | "tag"
  | "section"
  | "automation"
  | "role"
  | "origin"
  | "worktree"
  | "scene_anchor";

/**
 * Semantic priors, independent of rarity.
 *
 * IDF already handles *how rare* a facet is; `kappa` encodes how much sharing
 * that kind of thing should mean. Provider is the clearest case: it is both
 * common and unimportant, so it gets a low prior on top of a low IDF. Path is
 * common but important, so it keeps a full prior and lets IDF do the
 * discriminating.
 */
export const FACET_KAPPA: Record<FacetClass, number> = {
  path: 1.0,
  team: 1.4, // declared intent — the strongest human signal available
  watchlist: 0.8,
  class: 1.0,
  provider: 0.4,
  model: 0.3,
  mcp: 0.7,
  ext: 0.5,
  skill: 1.3,
  deployed: 1.3, // canonical cross-kind edge from the library index
  tag: 0.9,
  section: 0.4,
  automation: 1.1,
  role: 1.0,
  origin: 1.5, // artifact -> producing agent; provenance is near-identifying
  worktree: 1.2,
  scene_anchor: 1.6,
};

/**
 * Ceiling on how much a single user placement can bend the metric.
 *
 * Anchor facets participate in cosine like any other facet, which is the point:
 * placing A in parcel P makes A metrically closer to P's members, so its
 * neighbours, ranking, and future placement follow without a pin. The cap plus
 * the decay in `sceneFacetWeight` stops that becoming runaway feedback — place
 * A near B, the metric pulls A toward B, which then reads as confirmation. A
 * placement never compounds with repetition, and it fades if the scene is
 * abandoned, so the map cannot ossify around one afternoon's arrangement.
 */
export const SCENE_ANCHOR_MAX_WEIGHT = 3.2;

/** Half-life of an untouched scene placement, in milliseconds (~30 days). */
export const SCENE_ANCHOR_DECAY_MS = 30 * 24 * 60 * 60 * 1000;

/**
 * Ancestor chain depth cap. Universal facets are already free (see `idf`), so
 * this bounds memory rather than influence: a deeply nested path would
 * otherwise emit an unbounded token list per entity.
 */
export const MAX_PATH_FACET_DEPTH = 16;

export type FacetToken = string;

/** A sparse, weighted facet vector. Keys are tokens, values are kappa * idf. */
export type FacetVector = Map<FacetToken, number>;

export interface GardenEntityFacets {
  ref: EntityRef;
  /** Attraction facets. Deduped and sorted for deterministic iteration. */
  tokens: FacetToken[];
  /**
   * District ids the user explicitly rejected for this entity. Kept out of the
   * cosine vector: exclusion is repulsion from a *place*, not dissimilarity to
   * a set of peers, so the metric applies it as an additive penalty instead.
   */
  excludes: string[];
}

export function facetClassOf(token: FacetToken): FacetClass | null {
  const separator = token.indexOf(":");
  if (separator <= 0) return null;
  const candidate = token.slice(0, separator) as FacetClass;
  return candidate in FACET_KAPPA ? candidate : null;
}

// --- Corpus ---------------------------------------------------------------

export interface FacetCorpus {
  /** Number of materialized entities. Not the number of files on disk. */
  entityCount: number;
  /** Document frequency per token. Mutated incrementally by admit/retract. */
  df: Map<FacetToken, number>;
}

export function createCorpus(): FacetCorpus {
  return { entityCount: 0, df: new Map() };
}

/** Admit an entity to the corpus. O(|tokens|). */
export function admit(corpus: FacetCorpus, facets: GardenEntityFacets): void {
  corpus.entityCount += 1;
  for (const token of facets.tokens) {
    corpus.df.set(token, (corpus.df.get(token) ?? 0) + 1);
  }
}

export function buildCorpus(entities: readonly GardenEntityFacets[]): FacetCorpus {
  const corpus = createCorpus();
  for (const entity of entities) admit(corpus, entity);
  return corpus;
}

/**
 * Smoothed inverse document frequency.
 *
 *   idf(f) = ln((N + 1) / (df + 1))
 *
 * Laplace-smoothed so a freshly expanded directory is not treated as
 * infinitely rare, and it reaches exactly 0 when `df === N`. That zero matters:
 * universal facets such as a drive root cost nothing, so full ancestor chains
 * can be emitted without a depth heuristic to suppress them.
 *
 * Sensitivity is what bounds insertion cost:
 *
 *   d(idf)/dN  = 1 / (N + 1)                  -> negligible for any real N
 *   d(idf)/ddf = -1 / (df + 1)                -> large only when df is small
 *
 * So inserting an entity meaningfully perturbs only the facets it carries, and
 * only the rare ones — which by definition have few holders. That is the
 * dirty-set bound the incremental layout relies on.
 */
export function idf(corpus: FacetCorpus, token: FacetToken): number {
  const df = corpus.df.get(token) ?? 0;
  return Math.log((corpus.entityCount + 1) / (df + 1));
}

/**
 * Weight of a scene anchor facet, capped and time-decayed.
 *
 * `placedAt` is when the user last placed the entity; `now` defaults to call
 * time. Passing both explicitly keeps callers deterministic in tests.
 */
export function sceneFacetWeight(placedAt: number, now: number): number {
  const age = Math.max(0, now - placedAt);
  const decay = Math.exp((-age * Math.LN2) / SCENE_ANCHOR_DECAY_MS);
  return SCENE_ANCHOR_MAX_WEIGHT * decay;
}

/**
 * Build the weighted facet vector for an entity.
 *
 * `sceneWeights` maps a scene anchor token to its decayed weight; anchors
 * absent from the map fall back to the uncapped kappa * idf path, which is
 * only correct for canonical facets, so callers that support placement must
 * always supply it.
 */
export function facetVector(
  facets: GardenEntityFacets,
  corpus: FacetCorpus,
  sceneWeights?: ReadonlyMap<FacetToken, number>,
): FacetVector {
  const vector: FacetVector = new Map();
  for (const token of facets.tokens) {
    const facetClass = facetClassOf(token);
    if (!facetClass) continue;
    if (facetClass === "scene_anchor") {
      const weight = sceneWeights?.get(token) ?? SCENE_ANCHOR_MAX_WEIGHT;
      if (weight > 0) vector.set(token, weight);
      continue;
    }
    const weight = FACET_KAPPA[facetClass] * idf(corpus, token);
    if (weight > 0) vector.set(token, weight);
  }
  return vector;
}

/**
 * Cosine similarity over sparse vectors, iterating the shorter one.
 *
 * Returns 0 for an empty vector rather than NaN: a brand-new entity with no
 * affiliations is dissimilar to everything, which is the correct reading. Its
 * visibility is handled by salience, not by distance — conflating the two is
 * what drags new nodes into the centre of a force layout where they do not
 * belong.
 */
export function cosine(a: FacetVector, b: FacetVector): number {
  if (a.size === 0 || b.size === 0) return 0;
  const [shorter, longer] = a.size <= b.size ? [a, b] : [b, a];
  let dot = 0;
  for (const [token, weight] of shorter) {
    const other = longer.get(token);
    if (other !== undefined) dot += weight * other;
  }
  if (dot === 0) return 0;
  return dot / (norm(a) * norm(b));
}

function norm(vector: FacetVector): number {
  let sum = 0;
  for (const weight of vector.values()) sum += weight * weight;
  return Math.sqrt(sum);
}

// --- Emitters -------------------------------------------------------------

/**
 * Ancestor chain for a path, deepest first, including the path itself.
 *
 * Emitting the *set of ancestors* is what makes filesystem tree distance fall
 * out of the same cosine used for teams and skills: two files under
 * `src/features` share their parents but not their leaves, and IDF grades the
 * shared prefix by how many entities live under it.
 */
export function pathAncestorFacets(rawPath: string | undefined | null): FacetToken[] {
  const normalized = normalizeEntityPath(rawPath);
  if (!normalized) return [];

  const tokens: FacetToken[] = [];
  let current = normalized;
  for (let depth = 0; depth < MAX_PATH_FACET_DEPTH; depth += 1) {
    tokens.push(`path:${current}`);
    const separator = current.lastIndexOf("/");
    if (separator < 0) break;
    // Preserve roots: "d:/x" -> "d:/", "//server/share/x" -> "//server/share",
    // "/a" -> "/". Slicing blindly would produce "d:" and "" which are not
    // paths and would collide across drives.
    const parent = separator === 0 ? "/" : current.slice(0, separator);
    if (/^[a-zA-Z]:$/.test(parent)) {
      tokens.push(`path:${parent}/`);
      break;
    }
    if (parent === current || !parent) break;
    if (/^\/\/[^/]+$/.test(parent)) break; // bare UNC server is not a scope
    current = parent;
  }
  return tokens;
}

export interface AgentFacetContext {
  /** Team ids this agent belongs to, from `watchlists/index.json`. */
  teamIds?: readonly string[];
  /** Watchlist ids currently containing this agent. */
  watchlistIds?: readonly string[];
  /** Library entry_refs of skills deployed to this agent or its class. */
  deployedSkillRefs?: readonly string[];
  /** `AgentWorktreeSummary.id` when the agent sits in a tracked worktree. */
  worktreeId?: string;
}

/**
 * Facets for an agent.
 *
 * Path facets come from every directory the agent can actually reach —
 * `folder`, the worktree pair, and both include-directory lists — because
 * shared reach is what makes two agents neighbours in practice. `folder` and
 * `git_worktree_source` reproduce the existing "same folder" / "same project"
 * lens semantics from `graphProjection.ts` without a separate edge pass.
 */
export function emitAgentFacets(
  agent: AgentConfig,
  ref: EntityRef,
  context: AgentFacetContext = {},
): GardenEntityFacets {
  const tokens: FacetToken[] = [];
  const pushPath = (value: string | undefined | null) => {
    tokens.push(...pathAncestorFacets(value));
  };

  pushPath(agent.folder);
  pushPath(agent.git_worktree_source);
  pushPath(agent.git_worktree_folder);
  for (const dir of agent.include_directories ?? []) pushPath(dir);
  for (const dir of agent.system_include_directories ?? []) pushPath(dir);

  if (agent.agent_class) tokens.push(`class:${agent.agent_class.toLowerCase()}`);
  if (agent.provider) tokens.push(`provider:${String(agent.provider).toLowerCase()}`);
  if (agent.model) tokens.push(`model:${agent.model.toLowerCase()}`);
  for (const server of agent.allowed_mcp_server_names ?? []) {
    tokens.push(`mcp:${server.toLowerCase()}`);
  }
  for (const extension of agent.extensions ?? []) {
    tokens.push(`ext:${extension.toLowerCase()}`);
  }
  for (const teamId of context.teamIds ?? []) tokens.push(`team:${teamId}`);
  for (const listId of context.watchlistIds ?? []) tokens.push(`watchlist:${listId}`);
  for (const skillRef of context.deployedSkillRefs ?? []) {
    tokens.push(`skill:${skillRef.toLowerCase()}`);
  }
  if (context.worktreeId) tokens.push(`worktree:${context.worktreeId}`);

  return { ref, tokens: dedupeTokens(tokens), excludes: [] };
}

export interface SkillFacetContext {
  tags?: readonly string[];
  /**
   * Where this skill is deployed, from `LibraryIndex.deployments`.
   *
   * `linked` distinguishes a live junction from the copy fallback (which drops
   * a `.wardian-skill-source` marker and whose edits do not sync). A copy is a
   * fork, so it is weaker evidence of relatedness than a junction and gets a
   * separate, lighter token rather than being flattened together.
   */
  deployments?: ReadonlyArray<{ targetType: string; targetId: string; linked: boolean }>;
}

/**
 * Facets for a skill entity.
 *
 * Not used by the Garden's layout: a skill is an attribute of the agents that
 * carry it, so it renders as a glyph on each of them and never takes a position
 * of its own (see `skillGlyphs.ts`). The emitter stays because the facet
 * vocabulary is shared with the entity model, and because a Library-side or
 * lens-side surface may yet want to rank skills by similarity — a use that
 * needs no geometry.
 *
 * A previous revision also emitted the skill's own `skill:<id>` identity so a
 * skill *unit* would share a token with its agents and be pulled toward them.
 * That was a workaround for placing the skill at all; nothing needs it now.
 *
 * @internal Test support, no production caller: Garden tests build fixtures with it.
 */
export function emitSkillFacets(
  ref: EntityRef,
  context: SkillFacetContext = {},
): GardenEntityFacets {
  const tokens: FacetToken[] = ["section:skills"];
  if (ref.path) tokens.push(...libraryPathFacets("skills", ref.path));
  for (const tag of context.tags ?? []) tokens.push(`tag:${tag.toLowerCase()}`);
  for (const deployment of context.deployments ?? []) {
    const suffix = deployment.linked ? "" : "~copy";
    tokens.push(
      `deployed:${deployment.targetType.toLowerCase()}:${deployment.targetId.toLowerCase()}${suffix}`,
    );
  }
  return { ref, tokens: dedupeTokens(tokens), excludes: [] };
}

export interface AutomationFacetContext {
  /** Role names referenced by `task`/`decision` nodes. */
  roleNames?: readonly string[];
  /** Agent ids bound through `AutomationAssignments`. */
  assignedAgentIds?: readonly string[];
  /** Class refs referenced as `class:<name>` in an agent field. */
  classNames?: readonly string[];
  /**
   * Directories the automation operates on, from `path`-kind node fields.
   *
   * These are what tie an automation to a place when it binds no agent at all: a
   * blueprint whose shell node runs in `D:/Trading/trident` shares the ancestor
   * chain of the agents living there, and IDF makes the deep, rare segments
   * decisive while the drive root costs nothing.
   */
  workspacePaths?: readonly string[];
  /** Section-relative library folder, which groups a family of blueprints. */
  libraryFolder?: string | null;
  tags?: readonly string[];
}

export function emitAutomationFacets(
  ref: EntityRef,
  context: AutomationFacetContext = {},
): GardenEntityFacets {
  const tokens: FacetToken[] = ["section:automations", `automation:${ref.id.toLowerCase()}`];
  for (const role of context.roleNames ?? []) tokens.push(`role:${role.toLowerCase()}`);
  for (const agentId of context.assignedAgentIds ?? []) tokens.push(`deployed:agent:${agentId}`);
  for (const className of context.classNames ?? []) tokens.push(`class:${className.toLowerCase()}`);
  for (const path of context.workspacePaths ?? []) tokens.push(...pathAncestorFacets(path));
  if (context.libraryFolder) {
    tokens.push(...libraryPathFacets("automations", context.libraryFolder));
  }
  for (const tag of context.tags ?? []) tokens.push(`tag:${tag.toLowerCase()}`);
  return { ref, tokens: dedupeTokens(tokens), excludes: [] };
}

/**
 * Library paths get their own ancestor chain, section-qualified so the same
 * folder name in two sections cannot collide — the same reason
 * `libraryListUtils.folderKey` is section-qualified.
 */
function libraryPathFacets(section: string, relativePath: string): FacetToken[] {
  const segments = relativePath.split("/").filter(Boolean);
  const tokens: FacetToken[] = [];
  let accumulated = section;
  for (const segment of segments) {
    accumulated = `${accumulated}/${segment}`;
    tokens.push(`path:${accumulated.toLowerCase()}`);
  }
  return tokens;
}

// --- Garden-local placement ----------------------------------------------

export function sceneAnchorToken(districtId: string): FacetToken {
  return `scene_anchor:${districtId}`;
}

/**
 * Apply user placements to an already-emitted facet set.
 *
 * Anchors are added to `tokens` so they participate in cosine; exclusions are
 * kept separate because they are repulsion from a place rather than
 * dissimilarity to a set of peers.
 */
export function withPlacement(
  facets: GardenEntityFacets,
  placement: { anchoredDistrictId?: string; excludedDistrictIds?: readonly string[] },
): GardenEntityFacets {
  const tokens = placement.anchoredDistrictId
    ? dedupeTokens([...facets.tokens, sceneAnchorToken(placement.anchoredDistrictId)])
    : facets.tokens;
  const excludes = placement.excludedDistrictIds?.length
    ? [...new Set([...facets.excludes, ...placement.excludedDistrictIds])].sort()
    : facets.excludes;
  return { ...facets, tokens, excludes };
}

function dedupeTokens(tokens: readonly FacetToken[]): FacetToken[] {
  return [...new Set(tokens)].sort();
}

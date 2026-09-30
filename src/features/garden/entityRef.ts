/**
 * Canonical entity identity for the Garden map.
 *
 * Wardian already has five live key schemes for things the map must treat as
 * one keyspace:
 *
 * | Scheme            | Form                        | Owner                        |
 * | ----------------- | --------------------------- | ---------------------------- |
 * | `unitKey`         | `agent:<uuid>`              | `garden.types.ts`            |
 * | `entry_ref`       | `skills/dev/planner`        | `LibraryEntry` (path-derived)|
 * | `folderKey`       | `<section>/<path>`          | `libraryListUtils.ts`        |
 * | `fileResourceKey` | `file:<id>` / `artifact:<id>`| `files/fileResourceKey.ts`  |
 * | `Blueprint.id`    | opaque id                   | `automation/blueprint.rs`      |
 *
 * A list view tolerates that fragmentation. A map does not: the same object
 * arriving under two keys renders as two units, at two positions, with two
 * facet vectors — which corrupts distance rather than merely duplicating a row.
 * The worst offender is automations, which genuinely carry two identities at
 * once (`Blueprint.id` and library `entry_ref = automations/<file>.md`,
 * reconciled today by ad-hoc path matching in `detail/AutomationDetail.tsx`).
 *
 * This module is the single place that collapses those schemes. Every producer
 * feeding the Garden converts to an `EntityRef` first; nothing downstream sees
 * a raw scheme key.
 *
 * Identity is deliberately *not* a new store. Per
 * `docs/specs/2026-07-14-entity-oriented-agent-semantics.md`, an entity is a
 * projection across existing canonical records, so an `EntityRef` is a handle
 * that can always be resolved back through the owning command.
 */

/**
 * Entity kinds the map can place.
 *
 * Narrower than the 14-kind proposal in
 * `docs/specs/2026-06-02-malleable-garden.md`: `memory` and `queue_item` are
 * omitted because no memory feature, command, or DTO exists in the codebase —
 * every reference to it is aspirational spec language, and designing facets
 * around a fiction would produce distances nothing can verify. `artifact` is
 * the implemented analogue and is included.
 */
export type EntityKind =
  | "agent"
  | "automation"
  | "automation_run"
  | "skill"
  | "prompt"
  | "class"
  | "team"
  | "watchlist"
  | "worktree"
  | "folder"
  | "artifact";

/** Which canonical record owns this entity, for resolution back to a command. */
export type EntitySource =
  | "backend"      // live control endpoint / state.db
  | "wardian_home" // files under the active Wardian home
  | "library"      // <home>/library and <home>/classes
  | "logs"         // <home>/logs/automations
  | "workspace";   // user filesystem outside the Wardian home

export interface EntityRef {
  kind: EntityKind;
  /**
   * Canonical id within `kind`. Stable across sessions and machines wherever
   * the underlying record has a stable identity: agent/artifact UUIDs and
   * `Blueprint.id` are opaque and portable; folders and worktrees are
   * normalized absolute paths and are therefore machine-local by nature.
   */
  id: string;
  source: EntitySource;
  /** Present for path-backed entities. Normalized, never display-formatted. */
  path?: string;
}

/**
 * Canonical string key. `kind` is a closed vocabulary with no `:`, and ids may
 * contain `:` (drive letters) or `/` (library refs), so a single leading
 * `kind:` prefix parses unambiguously by splitting on the *first* separator.
 *
 * Deliberately identical in shape to the existing `unitKey` (`agent:<id>`) so
 * persisted Garden positions keyed by the old scheme still resolve for agents
 * and automations without a migration.
 */
export function entityKey(ref: EntityRef): string {
  return `${ref.kind}:${ref.id}`;
}

/** Inverse of `entityKey`. Returns null for malformed or unknown-kind keys. */
export function parseEntityKey(key: string): { kind: EntityKind; id: string } | null {
  const separator = key.indexOf(":");
  if (separator <= 0) return null;
  const kind = key.slice(0, separator);
  const id = key.slice(separator + 1);
  if (!id) return null;
  if (!ENTITY_KINDS.has(kind as EntityKind)) return null;
  return { kind: kind as EntityKind, id };
}

const ENTITY_KINDS: ReadonlySet<EntityKind> = new Set<EntityKind>([
  "agent",
  "automation",
  "automation_run",
  "skill",
  "prompt",
  "class",
  "team",
  "watchlist",
  "worktree",
  "folder",
  "artifact",
]);

/**
 * Normalize a filesystem path for identity comparison.
 *
 * Mirrors `normalizeGraphPath` (`features/graph/graphProjection.ts`) rather
 * than reimplementing it: backslashes to forward, collapse duplicate
 * separators (preserving a UNC `//` prefix), strip trailing separators, and
 * lowercase drive-letter and UNC paths because Windows path comparison is
 * case-insensitive. Kept local to avoid a Garden -> Graph import cycle; the
 * shared behavior is locked by `entityRef.test.ts`.
 */
export function normalizeEntityPath(value: string | undefined | null): string | null {
  const trimmed = value?.trim();
  if (!trimmed) return null;

  let normalized = trimmed.replace(/\\/g, "/");
  const hasUncPrefix = normalized.startsWith("//");
  const prefix = hasUncPrefix ? "//" : "";
  const body = hasUncPrefix ? normalized.slice(2) : normalized;
  normalized = prefix + body.replace(/\/{2,}/g, "/");
  normalized = stripTrailingSeparators(normalized);

  if (/^[a-zA-Z]:\//.test(normalized) || normalized.startsWith("//")) {
    normalized = normalized.toLowerCase();
  }
  return normalized;
}

/**
 * True when `path` is `root` itself or sits beneath it.
 *
 * The obvious spelling — `path.startsWith(`${root}/`)` — is wrong at a root, and
 * wrong in a way that survives review because roots are rare. `normalizeEntityPath`
 * deliberately *preserves* `/` and `d:/` (see `stripTrailingSeparators`), so a
 * root that already ends in a separator produces `//` or `d://` and matches none
 * of its own children.
 *
 * That bug was written independently in four places — reach containment, the
 * attribution walk, the change-paint ancestor chain, and the frontier prune — so
 * it lives here now and the callers share it.
 */
export function isUnderPath(root: string, path: string): boolean {
  if (path === root) return true;
  return path.startsWith(root.endsWith("/") ? root : `${root}/`);
}

/**
 * Split a path into its root and the part below it.
 *
 * Three shapes, matching `split_root` in `commands/agent_reach.rs`: a UNC share
 * is two segments (`//server/share`), a drive root is three characters (`d:/`),
 * and a POSIX root is one (`/`). Anything else is relative and has no root.
 */
function splitRoot(path: string): [root: string, rest: string] {
  if (path.startsWith("//")) {
    const segments = path.slice(2).split("/");
    if (segments.length >= 2 && segments[0] && segments[1]) {
      return [`//${segments[0]}/${segments[1]}`, segments.slice(2).join("/")];
    }
    return ["", path];
  }
  if (/^[a-zA-Z]:\//.test(path)) return [path.slice(0, 3), path.slice(3)];
  if (path.startsWith("/")) return ["/", path.slice(1)];
  return ["", path];
}

/**
 * Resolve `.` and `..` without touching the disk.
 *
 * `normalizeEntityPath` only rewrites separators and case, so a joined path
 * keeps its dot segments and `isUnderPath` — a prefix test — reads
 * `d:/repo/../other/a.ts` as living inside `d:/repo`. A write into a sibling
 * repository then lands on the workspace it left, and change paint credits churn
 * to a root that never saw it.
 *
 * `..` clamps at the root, matching what the filesystem would have done. The
 * Rust reach path already resolves lexically; this is the same rule on the
 * frontend, and the two are exercised by equivalent cases in their tests.
 */
export function lexicalNormalizePath(path: string): string {
  const [root, rest] = splitRoot(path.replace(/\\/g, "/"));
  const segments: string[] = [];
  for (const segment of rest.split("/")) {
    if (!segment || segment === ".") continue;
    if (segment === "..") {
      if (segments.length > 0 && segments[segments.length - 1] !== "..") {
        segments.pop();
      } else if (!root) {
        // A relative path has nothing to clamp against, so a leading `..` is
        // still meaningful and is kept for whoever joins it onto a base.
        segments.push("..");
      }
      continue;
    }
    segments.push(segment);
  }
  const below = segments.join("/");
  if (!root) return below;
  if (root.endsWith("/")) return `${root}${below}`;
  return below ? `${root}/${below}` : root;
}

/**
 * The containing directory, or `null` once a root is reached.
 *
 * Returns the root *including* its separator — `/` and `d:/`, never the empty
 * string or a bare `d:` — so the result is a path `isUnderPath` and
 * `normalizeEntityPath` both recognize.
 */
export function parentPath(path: string): string | null {
  if (path === "/" || /^[a-zA-Z]:\/$/.test(path)) return null;
  const uncRoot = path.match(/^\/\/[^/]+\/[^/]+$/);
  if (uncRoot) return null;
  const separator = path.lastIndexOf("/");
  if (separator < 0) return null;
  if (separator === 0) return "/";
  const parent = path.slice(0, separator);
  return /^[a-zA-Z]:$/.test(parent) ? `${parent}/` : parent;
}

function stripTrailingSeparators(path: string): string {
  if (path === "/") return path;
  if (/^[a-zA-Z]:\/$/.test(path)) return path;
  const uncRoot = path.match(/^\/\/[^/]+\/[^/]+\/?$/);
  if (uncRoot) return path.endsWith("/") ? path.slice(0, -1) : path;
  return path.replace(/\/+$/g, "");
}

// --- Constructors, one per producing scheme -------------------------------

/** `AgentConfig.session_id` / `AgentIdentity.uuid` — one UUID keyspace. */
export function agentRef(sessionId: string): EntityRef {
  return { kind: "agent", id: sessionId, source: "backend" };
}

/**
 * Automations: the dual-identity case.
 *
 * `Blueprint.id` is the canonical id because it survives a file rename and is
 * what run evidence is keyed by (`logs/automations/<blueprint_id>/<run_id>/`).
 * The library `entry_ref` is retained as `path` so the same unit can be
 * matched from either direction. Callers holding only an `entry_ref` must go
 * through `resolveAutomationRef`, which is why that function takes an index
 * rather than guessing.
 */
export function automationRef(blueprintId: string, entryPath?: string): EntityRef {
  return {
    kind: "automation",
    id: blueprintId,
    source: "library",
    path: entryPath ? normalizeLibraryPath(entryPath) : undefined,
  };
}

/**
 * Library entries. `LibraryEntry.entry_ref` is `<section>/<rel_path>`, which is
 * already a stable within-library identity, so it becomes the id directly for
 * sections whose entries have no separate opaque id.
 *
 * Automations are the exception and are rejected here: they must be constructed
 * via `automationRef` so they collapse onto `Blueprint.id`. Returning null forces
 * the caller to resolve rather than silently minting a duplicate unit.
 */
export function libraryEntryRef(entryRef: string): EntityRef | null {
  const normalized = normalizeLibraryPath(entryRef);
  const separator = normalized.indexOf("/");
  if (separator <= 0) return null;
  const section = normalized.slice(0, separator);
  const rest = normalized.slice(separator + 1);
  if (!rest) return null;

  switch (section) {
    case "skills":
      return { kind: "skill", id: normalized, source: "library", path: rest };
    case "prompts":
      return { kind: "prompt", id: normalized, source: "library", path: rest };
    case "classes":
      return { kind: "class", id: rest, source: "library", path: rest };
    case "automations":
      // Caller must resolve to Blueprint.id — see resolveAutomationRef.
      return null;
    default:
      // `mcps` is stubbed in the backend and never has entries; unknown
      // sections are ignored rather than mapped to a bogus kind.
      return null;
  }
}

/** Folders are identified by normalized absolute path — machine-local by nature. */
export function folderRef(path: string): EntityRef | null {
  const normalized = normalizeEntityPath(path);
  if (!normalized) return null;
  return { kind: "folder", id: normalized, source: "workspace", path: normalized };
}

/**
 * Library refs and paths are compared case-insensitively and with forward
 * slashes: library paths are filesystem-derived, and on Windows `Skills/Foo`
 * and `skills/foo` are the same entry.
 */
function normalizeLibraryPath(value: string): string {
  return value.trim().replace(/\\/g, "/").replace(/\/{2,}/g, "/").replace(/^\/+|\/+$/g, "").toLowerCase();
}
